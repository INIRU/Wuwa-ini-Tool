use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::Runtime;
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

use crate::{
    backup_store::{ApplyReason, ApplyResult, BackupStore},
    game_discovery::GameInstallation,
    ini_document::{IniDocument, IniError, ManagedChange, MergePreview},
    maintenance::{MaintenanceGate, MaintenanceOperation},
};

pub const MAX_ENGINE_INI_BYTES: usize = 4 * 1024 * 1024;
const PREVIEW_LIFETIME: Duration = Duration::from_secs(5 * 60);
const MAX_PENDING_PREVIEWS: usize = 32;

pub trait GameRunningProbe: Send + Sync {
    fn is_running(&self, executable: &Path) -> Result<bool, ConfigCommandError>;
}

#[derive(Clone, Debug)]
pub struct SelectedEngineIni {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

pub trait EngineIniPicker {
    fn pick_engine_ini(&self) -> Result<Option<SelectedEngineIni>, ConfigCommandError>;
}

pub struct NativeEngineIniPicker<R: Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: Runtime> NativeEngineIniPicker<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> EngineIniPicker for NativeEngineIniPicker<R> {
    fn pick_engine_ini(&self) -> Result<Option<SelectedEngineIni>, ConfigCommandError> {
        let selected = self
            .app
            .dialog()
            .file()
            .add_filter("Engine.ini", &["ini"])
            .blocking_pick_file();
        let Some(selected) = selected else {
            return Ok(None);
        };
        let path = selected
            .into_path()
            .map_err(|_| ConfigCommandError::OperationFailed)?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(ConfigCommandError::InvalidFileName)?
            .to_owned();
        let bytes = read_bounded_selected_file(&path)?;
        Ok(Some(SelectedEngineIni { file_name, bytes }))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Context,
    Removed,
    Added,
    Metadata,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewSemanticChange {
    pub section: String,
    pub key: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IniPreview {
    pub token: Uuid,
    pub before_bytes: usize,
    pub after_bytes: usize,
    pub candidate_text: String,
    pub diff: Vec<DiffLine>,
    pub semantic_changes: Vec<PreviewSemanticChange>,
    pub before_encoding: String,
    pub after_encoding: String,
    pub before_line_endings: String,
    pub after_line_endings: String,
    pub byte_only_change: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigCommandError {
    #[error("engine_ini_input_too_large")]
    InputTooLarge,
    #[error("invalid_engine_ini_filename")]
    InvalidFileName,
    #[error("unsupported_engine_ini_encoding")]
    UnsupportedEncoding,
    #[error("engine_ini_contains_nul")]
    ContainsNul,
    #[error("engine_ini_confirmation_required")]
    ConfirmationRequired,
    #[error("unknown_or_expired_engine_ini_preview")]
    UnknownPreview,
    #[error("game_is_running")]
    GameRunning,
    #[error("maintenance_busy")]
    MaintenanceBusy,
    #[error("config_state_unavailable")]
    StateUnavailable,
    #[error("config_operation_failed")]
    OperationFailed,
}

impl ConfigCommandError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InputTooLarge => "engine_ini_input_too_large",
            Self::InvalidFileName => "invalid_engine_ini_filename",
            Self::UnsupportedEncoding => "unsupported_engine_ini_encoding",
            Self::ContainsNul => "engine_ini_contains_nul",
            Self::ConfirmationRequired => "engine_ini_confirmation_required",
            Self::UnknownPreview => "unknown_or_expired_engine_ini_preview",
            Self::GameRunning => "game_is_running",
            Self::MaintenanceBusy => "maintenance_busy",
            Self::StateUnavailable => "config_state_unavailable",
            Self::OperationFailed => "config_operation_failed",
        }
    }
}

#[derive(Clone)]
struct StoredPreview {
    preview: MergePreview,
    reason: ApplyReason,
    created_at: Instant,
}

pub struct IniCommandService<P> {
    installation: GameInstallation,
    backup_store: BackupStore,
    probe: P,
    gate: MaintenanceGate,
    previews: Mutex<HashMap<Uuid, StoredPreview>>,
}

impl<P: GameRunningProbe> IniCommandService<P> {
    pub fn new(
        installation: GameInstallation,
        app_data_dir: PathBuf,
        probe: P,
        gate: MaintenanceGate,
    ) -> Self {
        Self {
            installation,
            backup_store: BackupStore::new(app_data_dir),
            probe,
            gate,
            previews: Mutex::new(HashMap::new()),
        }
    }

    pub fn preview_paste(&self, _text: String) -> Result<IniPreview, ConfigCommandError> {
        let text = _text;
        if text.len() > MAX_ENGINE_INI_BYTES {
            return Err(ConfigCommandError::InputTooLarge);
        }
        if text.contains('\0') {
            return Err(ConfigCommandError::ContainsNul);
        }
        self.store_candidate(text.into_bytes(), ApplyReason::RawEditor)
    }

    pub fn preview_import(
        &self,
        selected: Option<SelectedEngineIni>,
    ) -> Result<Option<IniPreview>, ConfigCommandError> {
        let Some(selected) = selected else {
            return Ok(None);
        };
        if !selected.file_name.eq_ignore_ascii_case("Engine.ini") {
            return Err(ConfigCommandError::InvalidFileName);
        }
        Ok(Some(
            self.store_candidate(selected.bytes, ApplyReason::Manual)?,
        ))
    }

    pub fn preview_import_from_picker(
        &self,
        picker: &impl EngineIniPicker,
    ) -> Result<Option<IniPreview>, ConfigCommandError> {
        self.preview_import(picker.pick_engine_ini()?)
    }

    pub fn preview_managed(
        &self,
        changes: &[ManagedChange],
    ) -> Result<IniPreview, ConfigCommandError> {
        let before = read_bounded_source(&self.installation.engine_ini)?;
        let document = IniDocument::parse(&before).map_err(map_ini_error)?;
        let preview = document.merge(changes).map_err(map_ini_error)?;
        let text = decode_candidate(&preview.after)?;
        self.insert_preview(preview, text, ApplyReason::Preset)
    }

    pub fn preview_restore_candidate(
        &self,
        bytes: Vec<u8>,
    ) -> Result<IniPreview, ConfigCommandError> {
        self.store_candidate(bytes, ApplyReason::Restore)
    }

    pub fn apply_preview(
        &self,
        token: Uuid,
        confirmed: bool,
    ) -> Result<ApplyResult, ConfigCommandError> {
        if !confirmed {
            return Err(ConfigCommandError::ConfirmationRequired);
        }
        let _guard = self
            .gate
            .try_acquire(MaintenanceOperation::IniWrite)
            .map_err(|_| ConfigCommandError::MaintenanceBusy)?;
        if self.probe.is_running(&self.installation.executable)? {
            return Err(ConfigCommandError::GameRunning);
        }
        let preview = {
            let mut previews = self
                .previews
                .lock()
                .map_err(|_| ConfigCommandError::StateUnavailable)?;
            prune_previews(&mut previews);
            previews
                .remove(&token)
                .ok_or(ConfigCommandError::UnknownPreview)?
        };
        if preview.created_at.elapsed() > PREVIEW_LIFETIME {
            return Err(ConfigCommandError::UnknownPreview);
        }
        self.backup_store
            .apply_guarded(
                &self.installation.engine_ini,
                &preview.preview,
                preview.reason,
                || match self.probe.is_running(&self.installation.executable) {
                    Ok(false) => Ok(()),
                    Ok(true) | Err(_) => {
                        Err(crate::backup_store::BackupError::MutationGuardRejected)
                    }
                },
            )
            .map_err(|_| ConfigCommandError::OperationFailed)
    }

    fn store_candidate(
        &self,
        bytes: Vec<u8>,
        reason: ApplyReason,
    ) -> Result<IniPreview, ConfigCommandError> {
        if bytes.len() > MAX_ENGINE_INI_BYTES {
            return Err(ConfigCommandError::InputTooLarge);
        }
        let text = decode_candidate(&bytes)?;
        IniDocument::parse(&bytes).map_err(map_ini_error)?;
        let before = read_bounded_source(&self.installation.engine_ini)?;
        IniDocument::parse(&before).map_err(map_ini_error)?;
        self.insert_preview(
            MergePreview {
                before,
                after: bytes,
                semantic_changes: Vec::new(),
            },
            text,
            reason,
        )
    }

    fn insert_preview(
        &self,
        preview: MergePreview,
        candidate_text: String,
        reason: ApplyReason,
    ) -> Result<IniPreview, ConfigCommandError> {
        let token = Uuid::new_v4();
        let before_bytes = preview.before.len();
        let after_bytes = preview.after.len();
        let before_text = decode_candidate(&preview.before)?;
        let before_encoding = encoding_label(&preview.before).to_owned();
        let after_encoding = encoding_label(&preview.after).to_owned();
        let before_line_endings = line_ending_label(&before_text).to_owned();
        let after_line_endings = line_ending_label(&candidate_text).to_owned();
        let mut diff = line_diff(&before_text, &candidate_text);
        let byte_only_change = preview.before != preview.after
            && !diff
                .iter()
                .any(|line| matches!(line.kind, DiffKind::Added | DiffKind::Removed));
        if byte_only_change
            || before_encoding != after_encoding
            || before_line_endings != after_line_endings
        {
            diff.insert(
                0,
                DiffLine {
                    kind: DiffKind::Metadata,
                    old_line: None,
                    new_line: None,
                    text: format!(
                        "encoding {before_encoding} -> {after_encoding}; line endings {before_line_endings} -> {after_line_endings}"
                    ),
                },
            );
        }
        let semantic_changes = preview
            .semantic_changes
            .iter()
            .map(|change| PreviewSemanticChange {
                section: change.section.clone(),
                key: change.key.clone(),
                before: change.before.clone(),
                after: change.after.clone(),
            })
            .collect();
        let mut previews = self
            .previews
            .lock()
            .map_err(|_| ConfigCommandError::StateUnavailable)?;
        prune_previews(&mut previews);
        while previews.len() >= MAX_PENDING_PREVIEWS {
            let Some(oldest) = previews
                .iter()
                .min_by_key(|(_, preview)| preview.created_at)
                .map(|(token, _)| *token)
            else {
                break;
            };
            previews.remove(&oldest);
        }
        previews.insert(
            token,
            StoredPreview {
                preview,
                reason,
                created_at: Instant::now(),
            },
        );
        Ok(IniPreview {
            token,
            before_bytes,
            after_bytes,
            candidate_text,
            diff,
            semantic_changes,
            before_encoding,
            after_encoding,
            before_line_endings,
            after_line_endings,
            byte_only_change,
        })
    }
}

fn prune_previews(previews: &mut HashMap<Uuid, StoredPreview>) {
    previews.retain(|_, preview| preview.created_at.elapsed() <= PREVIEW_LIFETIME);
}

fn read_bounded_source(path: &Path) -> Result<Vec<u8>, ConfigCommandError> {
    read_bounded_file(path)
}

fn read_bounded_selected_file(path: &Path) -> Result<Vec<u8>, ConfigCommandError> {
    read_bounded_file(path)
}

fn read_bounded_file(path: &Path) -> Result<Vec<u8>, ConfigCommandError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ConfigCommandError::OperationFailed)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ConfigCommandError::OperationFailed);
    }
    let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if length > MAX_ENGINE_INI_BYTES {
        return Err(ConfigCommandError::InputTooLarge);
    }
    let mut bytes = Vec::with_capacity(length);
    fs::File::open(path)
        .map_err(|_| ConfigCommandError::OperationFailed)?
        .take((MAX_ENGINE_INI_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ConfigCommandError::OperationFailed)?;
    if bytes.len() > MAX_ENGINE_INI_BYTES {
        return Err(ConfigCommandError::InputTooLarge);
    }
    Ok(bytes)
}

fn decode_candidate(bytes: &[u8]) -> Result<String, ConfigCommandError> {
    if bytes.len() > MAX_ENGINE_INI_BYTES {
        return Err(ConfigCommandError::InputTooLarge);
    }
    if bytes.starts_with(&[0xff, 0xfe, 0x00, 0x00])
        || bytes.starts_with(&[0x00, 0x00, 0xfe, 0xff])
        || bytes.starts_with(&[0xfe, 0xff])
    {
        return Err(ConfigCommandError::UnsupportedEncoding);
    }
    if let Some(payload) = bytes.strip_prefix(&[0xff, 0xfe]) {
        if payload.len() % 2 != 0 {
            return Err(ConfigCommandError::UnsupportedEncoding);
        }
        let units = payload
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        if units.contains(&0) {
            return Err(ConfigCommandError::ContainsNul);
        }
        return String::from_utf16(&units).map_err(|_| ConfigCommandError::UnsupportedEncoding);
    }
    let payload = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    if payload.contains(&0) {
        return Err(ConfigCommandError::ContainsNul);
    }
    std::str::from_utf8(payload)
        .map(str::to_owned)
        .map_err(|_| ConfigCommandError::UnsupportedEncoding)
}

fn line_diff(before: &str, after: &str) -> Vec<DiffLine> {
    let before = before.lines().collect::<Vec<_>>();
    let after = after.lines().collect::<Vec<_>>();
    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < before.len().saturating_sub(prefix)
        && suffix < after.len().saturating_sub(prefix)
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let mut lines = Vec::with_capacity(before.len() + after.len());
    for (index, text) in before.iter().take(prefix).enumerate() {
        lines.push(DiffLine {
            kind: DiffKind::Context,
            old_line: Some(index + 1),
            new_line: Some(index + 1),
            text: (*text).to_owned(),
        });
    }
    for (offset, text) in before[prefix..before.len() - suffix].iter().enumerate() {
        lines.push(DiffLine {
            kind: DiffKind::Removed,
            old_line: Some(prefix + offset + 1),
            new_line: None,
            text: (*text).to_owned(),
        });
    }
    for (offset, text) in after[prefix..after.len() - suffix].iter().enumerate() {
        lines.push(DiffLine {
            kind: DiffKind::Added,
            old_line: None,
            new_line: Some(prefix + offset + 1),
            text: (*text).to_owned(),
        });
    }
    for offset in (0..suffix).rev() {
        let old_index = before.len() - 1 - offset;
        let new_index = after.len() - 1 - offset;
        lines.push(DiffLine {
            kind: DiffKind::Context,
            old_line: Some(old_index + 1),
            new_line: Some(new_index + 1),
            text: before[old_index].to_owned(),
        });
    }
    lines
}

fn map_ini_error(error: IniError) -> ConfigCommandError {
    match error {
        IniError::UnsupportedEncoding => ConfigCommandError::UnsupportedEncoding,
        IniError::AmbiguousManagedKey { .. } => ConfigCommandError::OperationFailed,
    }
}

fn encoding_label(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xff, 0xfe]) {
        "utf16le"
    } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        "utf8_bom"
    } else {
        "utf8"
    }
}

fn line_ending_label(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let bare_lf = text.matches('\n').count().saturating_sub(crlf);
    match (crlf > 0, bare_lf > 0) {
        (true, true) => "mixed",
        (true, false) => "crlf",
        (false, true) => "lf",
        (false, false) => "none",
    }
}
