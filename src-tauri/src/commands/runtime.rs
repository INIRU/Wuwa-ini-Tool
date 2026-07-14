use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::Update;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    backup_store::{ApplyReason, BackupIntegrity, BackupStore},
    cache_cleanup::{
        CacheCleanupError, CacheCleanupService, CleanupPreview, CleanupReceipt, CleanupSelection,
        GameProcessProbe,
    },
    catalog::Catalog,
    game_discovery::{discover_installations, validate_game_executable, GameInstallation},
    ini_document::ManagedChange,
    maintenance::{MaintenanceGate, MaintenanceGuard, MaintenanceOperation},
    process_control::{
        ApplyReport, ApplyRequest, CpuTopology, FileFocusExclusionStore, FileFocusJournalStore,
        FocusActivationRequest, FocusCandidate, FocusConfig, FocusExclusionStore,
        FocusModeController, FocusProcessIdentity, FocusRuntimeAvailability, FocusThresholds,
        GameQosRequest, GameQosState, ProcessController, ProcessTarget, SystemFocusBackend,
    },
    profile_store::{
        CpuSelection, CustomProfile, ImportPreview, PriorityClass, ProcessProfile, ProfileStore,
        MAX_SHARE_BYTES,
    },
    supervisor::{
        FocusLifecycle, FocusLifecycleReport, GameQosLifecycle, GameQosLifecycleReport,
        GameQosLifecycleStatus, PreparedFocusLifecycle, PreparedGameQosLifecycle, Supervisor,
        SupervisorEvent, SupervisorState, SystemSupervisorBackend,
    },
};

use super::{
    ClientError, ConfigCommandError, EngineIniPicker, GameRunningProbe, IniCommandService,
    IniPreview, NativeEngineIniPicker,
};

type RuntimeFocus = PreparedFocusLifecycle<SystemFocusBackend, FileFocusJournalStore>;
type RuntimeGameQos = PreparedGameQosLifecycle<crate::process_control::FileGameQosJournalStore>;
type RuntimeSupervisor = Supervisor<SystemSupervisorBackend, RuntimeFocus, RuntimeGameQos>;
const TOKEN_LIFETIME: Duration = Duration::from_secs(5 * 60);
const MAX_PENDING_TOKENS: usize = 32;

#[derive(Clone)]
struct StoredCandidate {
    installation: GameInstallation,
    created_at: Instant,
}

#[derive(Clone)]
struct StoredProfileImport {
    preview: ImportPreview,
    created_at: Instant,
}

#[derive(Clone)]
struct StoredRestoreSource {
    backup_id: String,
    generation: u64,
    created_at: Instant,
}

#[derive(Clone)]
struct StoredFocusPreview {
    generation: u64,
    epoch: u64,
    controller_token: u64,
    candidates: Vec<FocusCandidate>,
    created_at: Instant,
}

pub struct RuntimeState {
    app_data: PathBuf,
    local_app_data: PathBuf,
    gate: MaintenanceGate,
    installation: Mutex<Option<GameInstallation>>,
    ini: Mutex<Option<IniCommandService<SystemGameProbe>>>,
    cache: Mutex<Option<CacheCleanupService<SystemGameProbe>>>,
    supervisor: Mutex<Option<RuntimeSupervisor>>,
    configuration_generation: AtomicU64,
    polling_owner: AtomicU64,
    polling_sequence: AtomicU64,
    candidates: Mutex<HashMap<Uuid, StoredCandidate>>,
    restore_sources: Mutex<HashMap<Uuid, StoredRestoreSource>>,
    profile_imports: Mutex<HashMap<Uuid, StoredProfileImport>>,
    pending_update: Mutex<Option<Update>>,
    shutdown_guard: Mutex<Option<MaintenanceGuard>>,
    pending_focus_reports: Mutex<Vec<FocusLifecycleReport>>,
    pending_qos_reports: Mutex<Vec<GameQosLifecycleReport>>,
    focus_previews: Mutex<HashMap<Uuid, StoredFocusPreview>>,
}

impl RuntimeState {
    pub fn new(app_data: PathBuf, local_app_data: PathBuf) -> Self {
        Self {
            app_data,
            local_app_data,
            gate: MaintenanceGate::new(),
            installation: Mutex::new(None),
            ini: Mutex::new(None),
            cache: Mutex::new(None),
            supervisor: Mutex::new(None),
            configuration_generation: AtomicU64::new(0),
            polling_owner: AtomicU64::new(0),
            polling_sequence: AtomicU64::new(0),
            candidates: Mutex::new(HashMap::new()),
            restore_sources: Mutex::new(HashMap::new()),
            profile_imports: Mutex::new(HashMap::new()),
            pending_update: Mutex::new(None),
            shutdown_guard: Mutex::new(None),
            pending_focus_reports: Mutex::new(Vec::new()),
            pending_qos_reports: Mutex::new(Vec::new()),
            focus_previews: Mutex::new(HashMap::new()),
        }
    }

    fn configure(&self, candidate: GameInstallation) -> Result<(), ClientError> {
        let _maintenance = self
            .gate
            .try_acquire(MaintenanceOperation::RuntimeConfigure)
            .map_err(|_| ClientError::new("maintenance_busy"))?;
        let validated = validate_game_executable(&candidate.executable)
            .map_err(|_| ClientError::new("invalid_game_installation"))?;
        if validated.game_root != candidate.game_root
            || validated.executable != candidate.executable
            || validated.engine_ini != candidate.engine_ini
        {
            return Err(ClientError::new("invalid_game_installation"));
        }
        let probe = SystemGameProbe(validated.clone());
        let ini = IniCommandService::new(
            validated.clone(),
            self.app_data.clone(),
            probe.clone(),
            self.gate.clone(),
        );
        let cache = CacheCleanupService::new_with_gate(
            validated.executable.clone(),
            self.local_app_data.clone(),
            self.app_data.join("cleanup-receipts"),
            probe,
            self.gate.clone(),
        )
        .map_err(|_| ClientError::new("cache_service_unavailable"))?;
        let focus_backend = SystemFocusBackend::new(&validated)
            .map_err(|_| ClientError::new("focus_mode_unavailable"))?;
        let pinned_executables = FileFocusExclusionStore::new(self.app_data.clone())
            .load()
            .map_err(|_| ClientError::new("focus_config_unavailable"))?;
        let mut focus = PreparedFocusLifecycle::new(FocusModeController::new(
            focus_backend,
            FileFocusJournalStore::new(self.app_data.clone()),
            FocusConfig {
                enabled: true,
                pinned_executables,
                ..FocusConfig::default()
            },
        ));
        let mut supervisor_state = self
            .supervisor
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        if let Some(current) = supervisor_state.as_mut() {
            current
                .request_quit()
                .map_err(|_| ClientError::new("supervisor_shutdown_failed"))?;
        }
        let recovery = focus
            .recover()
            .map_err(|_| ClientError::new("focus_recovery_failed"))?;
        let mut game_qos =
            PreparedGameQosLifecycle::for_app_data(validated.clone(), self.app_data.clone());
        let qos_recovery = game_qos
            .recover()
            .map_err(|_| ClientError::new("game_qos_recovery_failed"))?;
        let supervisor = Supervisor::with_focus_and_qos(
            SystemSupervisorBackend::new(validated.clone()),
            focus,
            game_qos,
            validated.clone(),
            ProcessProfile::default(),
            self.gate.clone(),
        );
        let mut installation_state = self
            .installation
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        let mut ini_state = self
            .ini
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        let mut cache_state = self
            .cache
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        let mut restore_sources = self
            .restore_sources
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        let mut focus_previews = self
            .focus_previews
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        let mut reports = self
            .pending_focus_reports
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        *installation_state = Some(validated);
        *ini_state = Some(ini);
        *cache_state = Some(cache);
        *supervisor_state = Some(supervisor);
        restore_sources.clear();
        focus_previews.clear();
        self.configuration_generation.fetch_add(1, Ordering::SeqCst);
        if !recovery.process_results.is_empty() || recovery.recovery_required {
            if reports.len() == MAX_PENDING_TOKENS {
                reports.remove(0);
            }
            reports.push(recovery);
        }
        if qos_recovery.status != GameQosLifecycleStatus::NoChange {
            let mut qos_reports = self
                .pending_qos_reports
                .lock()
                .map_err(|_| ClientError::new("state_unavailable"))?;
            if qos_reports.len() == MAX_PENDING_TOKENS {
                qos_reports.remove(0);
            }
            qos_reports.push(qos_recovery);
        }
        Ok(())
    }

    fn select(&self, token: Uuid, confirmed: bool) -> Result<(), ClientError> {
        let candidate = {
            let mut candidates = self
                .candidates
                .lock()
                .map_err(|_| ClientError::new("state_unavailable"))?;
            candidates.retain(|_, value| value.created_at.elapsed() <= TOKEN_LIFETIME);
            let candidate = candidates
                .get(&token)
                .ok_or(ClientError::new("unknown_game_candidate"))?
                .installation
                .clone();
            if candidate.requires_user_confirmation && !confirmed {
                return Err(ClientError::new("game_selection_confirmation_required"));
            }
            candidate
        };
        self.configure(candidate)?;
        self.candidates
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?
            .remove(&token);
        Ok(())
    }

    pub fn shutdown(&self) -> Result<(), ClientError> {
        let mut shutdown_guard = self
            .shutdown_guard
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        if shutdown_guard.is_some() {
            return Ok(());
        }
        let guard = self
            .gate
            .try_acquire(MaintenanceOperation::Shutdown)
            .map_err(|_| ClientError::new("maintenance_busy"))?;
        let mut supervisor = self
            .supervisor
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        if let Some(supervisor) = supervisor.as_mut() {
            supervisor
                .request_quit()
                .map_err(|_| ClientError::new("supervisor_shutdown_failed"))?;
        }
        self.polling_owner.store(0, Ordering::SeqCst);
        *shutdown_guard = Some(guard);
        Ok(())
    }

    pub fn prepare_update(&self) -> Result<MaintenanceGuard, ClientError> {
        let guard = self
            .gate
            .try_acquire(MaintenanceOperation::UpdateInstall)
            .map_err(|_| ClientError::new("maintenance_busy"))?;
        self.ensure_game_stopped_for_update()?;
        Ok(guard)
    }

    fn ensure_game_stopped_for_update(&self) -> Result<(), ClientError> {
        if let Some(installation) = self
            .installation
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?
            .as_ref()
        {
            if SystemSupervisorBackend::is_running(installation)
                .map_err(|_| ClientError::new("process_state_unavailable"))?
            {
                return Err(ClientError::new("game_is_running"));
            }
        }
        Ok(())
    }

    fn suspend_for_update(&self) -> Result<(), ClientError> {
        if let Some(supervisor) = self
            .supervisor
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?
            .as_mut()
        {
            supervisor
                .suspend_for_maintenance()
                .map_err(|_| ClientError::new("supervisor_suspend_failed"))?;
        }
        Ok(())
    }

    pub fn set_pending_update(&self, update: Update) -> Result<UpdateAvailable, ClientError> {
        let metadata = update_available_metadata(
            update.version.clone(),
            update.body.clone(),
            update.date.and_then(|date| date.format(&Rfc3339).ok()),
        );
        *self
            .pending_update
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))? = Some(update);
        Ok(metadata)
    }

    fn claim_polling(&self) -> Option<u64> {
        let token = self
            .polling_sequence
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1)
            .max(1);
        self.polling_owner
            .compare_exchange(0, token, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| token)
    }

    fn owns_polling(&self, token: u64) -> bool {
        self.polling_owner.load(Ordering::SeqCst) == token
    }

    fn release_polling(&self, token: u64) {
        let _ = self
            .polling_owner
            .compare_exchange(token, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct SystemGameProbe(GameInstallation);

impl GameRunningProbe for SystemGameProbe {
    fn is_running(&self, _executable: &Path) -> Result<bool, ConfigCommandError> {
        SystemSupervisorBackend::is_running(&self.0)
            .map_err(|_| ConfigCommandError::StateUnavailable)
    }
}

impl GameProcessProbe for SystemGameProbe {
    fn is_running(&self, _executable: &Path) -> Result<bool, CacheCleanupError> {
        SystemSupervisorBackend::is_running(&self.0)
            .map_err(|_| CacheCleanupError::StateUnavailable)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IniPreviewRequest {
    Paste { text: String },
    Managed { changes: Vec<ManagedChangeRequest> },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedChangeRequest {
    pub section: String,
    pub key: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AppSnapshot {
    pub version: &'static str,
    pub installation: Option<GameInstallation>,
    pub supervisor_state: SupervisorState,
    pub active_process: Option<crate::supervisor::ObservedGame>,
    pub focus_runtime_availability: Option<FocusRuntimeAvailability>,
    pub focus_pinned_executables: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GameCandidate {
    pub token: Uuid,
    pub installation: GameInstallation,
}

#[derive(Clone, Debug, Serialize)]
pub struct ApplyIniResult {
    pub applied_sha256: String,
    pub backup_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RestoreBackupResult {
    pub applied_sha256: String,
    pub restored_from_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSettingsRequest {
    pub cpu_selection: CpuSelection,
    pub priority: PriorityClass,
    pub dangerous_priority_acknowledged: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct FocusPreviewEnvelope {
    pub token: Uuid,
    pub thresholds: FocusThresholds,
    pub candidates: Vec<FocusCandidate>,
    pub runtime_availability: FocusRuntimeAvailability,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusActivationCommand {
    pub token: Uuid,
    pub selected: Vec<crate::process_control::FocusProcessIdentity>,
    pub select_all_eligible: bool,
    pub select_all_confirmed: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusExclusionCommand {
    pub token: Uuid,
    pub candidate: FocusProcessIdentity,
    pub excluded: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct FocusExclusionStatus {
    pub executable: PathBuf,
    pub excluded: bool,
    pub pinned_executables: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameQosNormalizationCommand {
    pub disable_execution_speed_throttling: bool,
    pub confirmed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GameQosNormalizationStatus {
    NoChange,
    Applied,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GameQosNormalizationReport {
    pub status: GameQosNormalizationStatus,
    pub prior: GameQosState,
    pub applied: GameQosState,
    pub restore_pending: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct BackupSummary {
    pub id: String,
    pub created_at: String,
    pub sha256: String,
    pub reason: ApplyReason,
    pub pinned: bool,
    pub integrity: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProfileImportCandidate {
    pub token: Uuid,
    pub preview: ImportPreview,
}

#[derive(Clone, Debug, Serialize)]
pub struct SupervisorRuntimeError {
    pub code: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateAvailable {
    pub version: String,
    pub notes: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateDownloadProgress {
    pub downloaded: String,
    pub total: Option<String>,
}

fn update_download_progress(downloaded: u64, total: Option<u64>) -> UpdateDownloadProgress {
    UpdateDownloadProgress {
        downloaded: downloaded.to_string(),
        total: total.map(|value| value.to_string()),
    }
}

fn update_available_metadata(
    version: String,
    notes: Option<String>,
    published_at: Option<String>,
) -> UpdateAvailable {
    UpdateAvailable {
        version,
        notes,
        published_at,
    }
}

#[tauri::command]
pub fn get_pending_update(
    state: State<'_, RuntimeState>,
) -> Result<Option<UpdateAvailable>, ClientError> {
    let update = state
        .pending_update
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .as_ref()
        .map(|update| {
            update_available_metadata(
                update.version.clone(),
                update.body.clone(),
                update.date.and_then(|date| date.format(&Rfc3339).ok()),
            )
        });
    Ok(update)
}

#[tauri::command]
pub fn get_app_snapshot(state: State<'_, RuntimeState>) -> Result<AppSnapshot, ClientError> {
    let supervisor = state
        .supervisor
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    let installation = state
        .installation
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .clone();
    let focus_pinned_executables = supervisor
        .as_ref()
        .map(|supervisor| {
            supervisor
                .focus()
                .controller()
                .config()
                .pinned_executables
                .iter()
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    Ok(AppSnapshot {
        version: env!("CARGO_PKG_VERSION"),
        installation,
        supervisor_state: supervisor
            .as_ref()
            .map_or(SupervisorState::Idle, Supervisor::state),
        active_process: supervisor
            .as_ref()
            .and_then(|supervisor| supervisor.active_process().cloned()),
        focus_runtime_availability: supervisor
            .as_ref()
            .map(|supervisor| supervisor.focus().runtime_availability()),
        focus_pinned_executables,
    })
}

#[tauri::command]
pub fn preview_ini(
    state: State<'_, RuntimeState>,
    request: IniPreviewRequest,
) -> Result<IniPreview, ClientError> {
    let service = state
        .ini
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    let service = service
        .as_ref()
        .ok_or(ClientError::new("game_not_configured"))?;
    match request {
        IniPreviewRequest::Paste { text } => service.preview_paste(text),
        IniPreviewRequest::Managed { changes } => {
            let changes = changes
                .into_iter()
                .map(|change| match change.value {
                    Some(value) => ManagedChange::set(change.section, change.key, value),
                    None => ManagedChange::delete(change.section, change.key),
                })
                .collect::<Vec<_>>();
            service.preview_managed(&changes)
        }
    }
    .map_err(ClientError::from)
}

#[tauri::command]
pub async fn preview_ini_import(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<Option<IniPreview>, ClientError> {
    let selected = NativeEngineIniPicker::new(app)
        .pick_engine_ini()
        .map_err(ClientError::from)?;
    let service = state
        .ini
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    service
        .as_ref()
        .ok_or(ClientError::new("game_not_configured"))?
        .preview_import(selected)
        .map_err(ClientError::from)
}

#[tauri::command]
pub fn apply_ini(
    state: State<'_, RuntimeState>,
    token: Uuid,
    confirmed: bool,
) -> Result<ApplyIniResult, ClientError> {
    let service = state
        .ini
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    let result = service
        .as_ref()
        .ok_or(ClientError::new("game_not_configured"))?
        .apply_preview(token, confirmed)
        .map_err(ClientError::from)?;
    Ok(ApplyIniResult {
        applied_sha256: result.applied_sha256,
        backup_id: result.backup.id,
    })
}

#[tauri::command]
pub fn restore_backup(
    state: State<'_, RuntimeState>,
    token: Uuid,
    confirmed: bool,
) -> Result<RestoreBackupResult, ClientError> {
    if !confirmed {
        return Err(ClientError::new("engine_ini_confirmation_required"));
    }
    let service = state
        .ini
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    let service = service
        .as_ref()
        .ok_or(ClientError::new("game_not_configured"))?;
    let generation = state.configuration_generation.load(Ordering::SeqCst);
    let mut sources = state
        .restore_sources
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    sources.retain(|_, source| source.created_at.elapsed() <= TOKEN_LIFETIME);
    let source = sources
        .get(&token)
        .filter(|source| restore_binding_is_current(source, generation))
        .cloned()
        .ok_or(ClientError::new("unknown_restore_preview"))?;
    let result = service
        .apply_preview(token, confirmed)
        .map_err(ClientError::from)?;
    sources.remove(&token);
    Ok(RestoreBackupResult {
        applied_sha256: result.applied_sha256,
        restored_from_id: source.backup_id,
    })
}

fn restore_binding_is_current(source: &StoredRestoreSource, generation: u64) -> bool {
    source.generation == generation && source.created_at.elapsed() <= TOKEN_LIFETIME
}

#[tauri::command]
pub fn preview_restore_backup(
    state: State<'_, RuntimeState>,
    backup_id: String,
) -> Result<IniPreview, ClientError> {
    let installation_state = state
        .installation
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    let installation = installation_state
        .as_ref()
        .ok_or(ClientError::new("game_not_configured"))?;
    let ini_state = state
        .ini
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    let service = ini_state
        .as_ref()
        .ok_or(ClientError::new("game_not_configured"))?;
    let entry = BackupStore::new(state.app_data.clone())
        .list(&installation.engine_ini)
        .map_err(|_| ClientError::new("backup_list_failed"))?
        .into_iter()
        .find(|entry| entry.backup.id == backup_id && entry.integrity == BackupIntegrity::Verified)
        .ok_or(ClientError::new("backup_not_found"))?;
    let metadata = fs::symlink_metadata(&entry.backup_path)
        .map_err(|_| ClientError::new("backup_read_failed"))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > super::MAX_ENGINE_INI_BYTES as u64
    {
        return Err(ClientError::new("backup_read_failed"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(&entry.backup_path)
        .map_err(|_| ClientError::new("backup_read_failed"))?
        .take((super::MAX_ENGINE_INI_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ClientError::new("backup_read_failed"))?;
    if format!("{:x}", Sha256::digest(&bytes)) != entry.backup.sha256 {
        return Err(ClientError::new("backup_read_failed"));
    }
    let preview = service
        .preview_restore_candidate(bytes)
        .map_err(ClientError::from)?;
    let generation = state.configuration_generation.load(Ordering::SeqCst);
    let mut sources = state
        .restore_sources
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    sources.retain(|_, source| source.created_at.elapsed() <= TOKEN_LIFETIME);
    while sources.len() >= MAX_PENDING_TOKENS {
        let Some(oldest) = sources
            .iter()
            .min_by_key(|(_, source)| source.created_at)
            .map(|(token, _)| *token)
        else {
            break;
        };
        sources.remove(&oldest);
    }
    sources.insert(
        preview.token,
        StoredRestoreSource {
            backup_id,
            generation,
            created_at: Instant::now(),
        },
    );
    Ok(preview)
}

#[tauri::command]
pub fn save_profile(
    state: State<'_, RuntimeState>,
    profile: CustomProfile,
) -> Result<CustomProfile, ClientError> {
    let _guard = state
        .gate
        .try_acquire(MaintenanceOperation::ProfileWrite)
        .map_err(|_| ClientError::new("maintenance_busy"))?;
    ProfileStore::new(state.app_data.clone())
        .save(
            &profile,
            &Catalog::load_embedded().map_err(|_| ClientError::new("catalog_unavailable"))?,
        )
        .map_err(|_| ClientError::new("profile_save_failed"))
}

#[tauri::command]
pub fn discover_game(state: State<'_, RuntimeState>) -> Result<Vec<GameCandidate>, ClientError> {
    let installations =
        discover_installations().map_err(|_| ClientError::new("game_discovery_failed"))?;
    store_candidates(&state, installations)
}

#[tauri::command]
pub async fn discover_game_manual(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<Option<GameCandidate>, ClientError> {
    use tauri_plugin_dialog::DialogExt;
    let Some(selected) = app
        .dialog()
        .file()
        .add_filter("Wuthering Waves executable", &["exe"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let executable = selected
        .into_path()
        .map_err(|_| ClientError::new("invalid_game_installation"))?;
    let installation = validate_game_executable(executable)
        .map_err(|_| ClientError::new("invalid_game_installation"))?;
    Ok(store_candidates(&state, vec![installation])?.pop())
}

#[tauri::command]
pub fn select_game(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    candidate_token: Uuid,
    confirmed: bool,
) -> Result<(), ClientError> {
    state.select(candidate_token, confirmed)?;
    start_supervisor_polling(app, &state);
    Ok(())
}

#[tauri::command]
pub fn launch_game(app: AppHandle, state: State<'_, RuntimeState>) -> Result<(), ClientError> {
    state
        .supervisor
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .as_mut()
        .ok_or(ClientError::new("game_not_configured"))?
        .request_launch()
        .map_err(|_| ClientError::new("game_launch_failed"))?;
    start_supervisor_polling(app, &state);
    Ok(())
}

fn store_candidates(
    state: &RuntimeState,
    installations: Vec<GameInstallation>,
) -> Result<Vec<GameCandidate>, ClientError> {
    let mut candidates = state
        .candidates
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    candidates.retain(|_, value| value.created_at.elapsed() <= TOKEN_LIFETIME);
    let mut output = Vec::new();
    for installation in installations.into_iter().take(MAX_PENDING_TOKENS) {
        while candidates.len() >= MAX_PENDING_TOKENS {
            let Some(oldest) = candidates
                .iter()
                .min_by_key(|(_, value)| value.created_at)
                .map(|(token, _)| *token)
            else {
                break;
            };
            candidates.remove(&oldest);
        }
        let token = Uuid::new_v4();
        candidates.insert(
            token,
            StoredCandidate {
                installation: installation.clone(),
                created_at: Instant::now(),
            },
        );
        output.push(GameCandidate {
            token,
            installation,
        });
    }
    Ok(output)
}

fn start_supervisor_polling(app: AppHandle, state: &RuntimeState) {
    let Some(owner) = state.claim_polling() else {
        return;
    };
    std::thread::spawn(move || {
        loop {
            let state = app.state::<RuntimeState>();
            if !state.owns_polling(owner) {
                break;
            }
            let (delay, events, error) = match state.supervisor.lock() {
                Ok(mut supervisor) => match supervisor.as_mut() {
                    Some(supervisor) => {
                        let error = supervisor.tick().err();
                        (
                            supervisor.next_poll_delay(),
                            supervisor.drain_events(),
                            error,
                        )
                    }
                    None => break,
                },
                Err(_) => break,
            };
            for event in events {
                let _ = app.emit("supervisor://status", event);
            }
            if let Ok(mut reports) = state.pending_focus_reports.lock() {
                for report in reports.drain(..) {
                    let _ = app.emit("focus://report", report);
                }
            }
            if let Ok(mut reports) = state.pending_qos_reports.lock() {
                for report in reports.drain(..) {
                    let _ = app.emit("qos://report", report);
                }
            }
            if let Ok(mut supervisor) = state.supervisor.lock() {
                if let Some(supervisor) = supervisor.as_mut() {
                    for report in supervisor.drain_game_qos_reports() {
                        let _ = app.emit("qos://report", report);
                    }
                }
            }
            if error.is_some() {
                let _ = app.emit(
                    "supervisor://error",
                    SupervisorRuntimeError {
                        code: "supervisor_operation_failed",
                    },
                );
            }
            std::thread::sleep(delay);
        }
        app.state::<RuntimeState>().release_polling(owner);
    });
}

fn resume_supervisor_polling(app: AppHandle, state: &RuntimeState) {
    if has_configured_supervisor(state) {
        start_supervisor_polling(app, state);
    }
}

fn has_configured_supervisor(state: &RuntimeState) -> bool {
    state
        .supervisor
        .lock()
        .is_ok_and(|supervisor| supervisor.is_some())
}

#[tauri::command]
pub fn get_cpu_topology() -> Result<CpuTopology, ClientError> {
    ProcessController::topology().map_err(|_| ClientError::new("cpu_topology_unavailable"))
}

#[tauri::command]
pub fn apply_process_settings(
    state: State<'_, RuntimeState>,
    request: ProcessSettingsRequest,
) -> Result<ApplyReport, ClientError> {
    let mut supervisor = state
        .supervisor
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    let supervisor = supervisor
        .as_mut()
        .ok_or(ClientError::new("game_not_configured"))?;
    let process = supervisor
        .active_process()
        .ok_or(ClientError::new("game_not_running"))?;
    let installation = state
        .installation
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .clone()
        .ok_or(ClientError::new("game_not_configured"))?;
    let target = ProcessTarget::from_installation_with_creation(
        process.pid,
        process.creation_time_100ns,
        &installation,
    )
    .map_err(|_| ClientError::new("invalid_game_identity"))?;
    let profile = ProcessProfile {
        cpu_selection: request.cpu_selection.clone(),
        priority: request.priority,
    };
    let report = ProcessController::apply(
        &target,
        &ApplyRequest {
            cpu_selection: request.cpu_selection,
            priority: request.priority,
            dangerous_priority_acknowledged: request.dangerous_priority_acknowledged,
        },
    )
    .map_err(|_| ClientError::new("process_settings_failed"))?;
    if should_remember_process_profile(report.status) {
        supervisor.set_process_profile(profile, request.dangerous_priority_acknowledged);
    }
    Ok(report)
}

fn validate_game_qos_command(
    request: GameQosNormalizationCommand,
) -> Result<GameQosRequest, ClientError> {
    if !request.disable_execution_speed_throttling {
        return Err(ClientError::new("game_qos_opt_in_required"));
    }
    if !request.confirmed {
        return Err(ClientError::new("game_qos_confirmation_required"));
    }
    Ok(GameQosRequest {
        disable_execution_speed_throttling: true,
    })
}

#[tauri::command]
pub fn normalize_game_qos(
    state: State<'_, RuntimeState>,
    request: GameQosNormalizationCommand,
) -> Result<GameQosNormalizationReport, ClientError> {
    let request = validate_game_qos_command(request)?;
    let report = state
        .supervisor
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .as_mut()
        .ok_or(ClientError::new("game_not_configured"))?
        .normalize_game_qos(request)
        .map_err(|error| match error {
            crate::supervisor::SupervisorError::NotActive => ClientError::new("game_not_running"),
            crate::supervisor::SupervisorError::InvalidGameIdentity => {
                ClientError::new("invalid_game_identity")
            }
            _ => ClientError::new("game_qos_normalization_failed"),
        })?;
    let prior = report
        .prior
        .ok_or(ClientError::new("game_qos_normalization_failed"))?;
    let applied = report
        .applied
        .ok_or(ClientError::new("game_qos_normalization_failed"))?;
    let status = match report.status {
        GameQosLifecycleStatus::NoChange => GameQosNormalizationStatus::NoChange,
        GameQosLifecycleStatus::Applied => GameQosNormalizationStatus::Applied,
        _ => return Err(ClientError::new("game_qos_normalization_failed")),
    };
    Ok(GameQosNormalizationReport {
        status,
        prior,
        applied,
        restore_pending: report.restore_pending,
    })
}

fn should_remember_process_profile(status: crate::process_control::ApplyStatus) -> bool {
    matches!(
        status,
        crate::process_control::ApplyStatus::Success | crate::process_control::ApplyStatus::Partial
    )
}

#[tauri::command]
pub fn preview_focus_mode(
    state: State<'_, RuntimeState>,
) -> Result<FocusPreviewEnvelope, ClientError> {
    let mut supervisor = state
        .supervisor
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    let supervisor = supervisor
        .as_mut()
        .ok_or(ClientError::new("game_not_configured"))?;
    let epoch = supervisor.epoch();
    let preview = supervisor
        .focus_mut()
        .preview()
        .map_err(|_| ClientError::new("focus_preview_failed"))?;
    let generation = state.configuration_generation.load(Ordering::SeqCst);
    let authorization_token = Uuid::new_v4();
    let mut previews = state
        .focus_previews
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    previews.retain(|_, preview| preview.created_at.elapsed() <= TOKEN_LIFETIME);
    while previews.len() >= MAX_PENDING_TOKENS {
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
        authorization_token,
        StoredFocusPreview {
            generation,
            epoch,
            controller_token: preview.token,
            candidates: preview.candidates.clone(),
            created_at: Instant::now(),
        },
    );
    Ok(FocusPreviewEnvelope {
        token: authorization_token,
        thresholds: preview.thresholds,
        candidates: preview.candidates,
        runtime_availability: preview.runtime_availability,
    })
}

#[tauri::command]
pub fn activate_focus_mode(
    state: State<'_, RuntimeState>,
    request: FocusActivationCommand,
) -> Result<FocusLifecycleReport, ClientError> {
    let mut supervisor = state
        .supervisor
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    let supervisor = supervisor
        .as_mut()
        .ok_or(ClientError::new("game_not_configured"))?;
    let binding = {
        let mut previews = state
            .focus_previews
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        previews.retain(|_, preview| preview.created_at.elapsed() <= TOKEN_LIFETIME);
        previews
            .remove(&request.token)
            .ok_or(ClientError::new("stale_focus_preview"))?
    };
    let generation = state.configuration_generation.load(Ordering::SeqCst);
    if !focus_binding_is_current(&binding, generation, supervisor.epoch()) {
        return Err(ClientError::new("stale_focus_preview"));
    }
    supervisor.focus_mut().arm(FocusActivationRequest {
        preview_token: binding.controller_token,
        selected: request.selected,
        select_all_eligible: request.select_all_eligible,
        select_all_confirmed: request.select_all_confirmed,
    });
    supervisor
        .activate_focus_mode()
        .map_err(|_| ClientError::new("focus_activation_failed"))
}

fn focus_binding_is_current(binding: &StoredFocusPreview, generation: u64, epoch: u64) -> bool {
    binding.generation == generation
        && binding.epoch == epoch
        && binding.created_at.elapsed() <= TOKEN_LIFETIME
}

fn focus_exclusion_candidate(
    binding: &StoredFocusPreview,
    generation: u64,
    epoch: u64,
    identity: &FocusProcessIdentity,
) -> Result<PathBuf, ClientError> {
    if !focus_binding_is_current(binding, generation, epoch) {
        return Err(ClientError::new("stale_focus_preview"));
    }
    binding
        .candidates
        .iter()
        .find(|candidate| candidate.identity == *identity)
        .map(|candidate| candidate.identity.canonical_image.clone())
        .ok_or(ClientError::new("invalid_focus_candidate"))
}

#[tauri::command]
pub fn set_focus_exclusion(
    state: State<'_, RuntimeState>,
    request: FocusExclusionCommand,
) -> Result<FocusExclusionStatus, ClientError> {
    let mut supervisor = state
        .supervisor
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    let supervisor = supervisor
        .as_mut()
        .ok_or(ClientError::new("game_not_configured"))?;
    let binding = {
        let mut previews = state
            .focus_previews
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        previews.retain(|_, preview| preview.created_at.elapsed() <= TOKEN_LIFETIME);
        previews
            .remove(&request.token)
            .ok_or(ClientError::new("stale_focus_preview"))?
    };
    let generation = state.configuration_generation.load(Ordering::SeqCst);
    let executable =
        focus_exclusion_candidate(&binding, generation, supervisor.epoch(), &request.candidate)?;
    let controller = supervisor.focus_mut().controller_mut();
    let resolved = controller
        .preview_candidate_executable(binding.controller_token, &request.candidate)
        .map_err(|_| ClientError::new("invalid_focus_candidate"))?;
    if resolved != executable {
        return Err(ClientError::new("invalid_focus_candidate"));
    }
    let mut pinned = controller.config().pinned_executables.clone();
    if request.excluded {
        pinned.insert(executable.clone());
    } else {
        pinned.remove(&executable);
    }
    FileFocusExclusionStore::new(state.app_data.clone())
        .save(&pinned)
        .map_err(|_| ClientError::new("focus_config_save_failed"))?;
    controller.replace_pinned_executables(pinned.clone());
    state
        .focus_previews
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .clear();
    Ok(FocusExclusionStatus {
        executable,
        excluded: request.excluded,
        pinned_executables: pinned.into_iter().collect(),
    })
}

#[tauri::command]
pub fn deactivate_focus_mode(
    state: State<'_, RuntimeState>,
) -> Result<FocusLifecycleReport, ClientError> {
    state
        .supervisor
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .as_mut()
        .ok_or(ClientError::new("game_not_configured"))?
        .deactivate_focus_mode()
        .map_err(|_| ClientError::new("focus_restore_failed"))
}

#[tauri::command]
pub fn preview_cache_cleanup(
    state: State<'_, RuntimeState>,
    selection: CleanupSelection,
) -> Result<CleanupPreview, ClientError> {
    state
        .cache
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .as_ref()
        .ok_or(ClientError::new("game_not_configured"))?
        .preview(selection)
        .map_err(|_| ClientError::new("cache_preview_failed"))
}

#[tauri::command]
pub fn run_cache_cleanup(
    state: State<'_, RuntimeState>,
    token: Uuid,
    confirmed: bool,
) -> Result<CleanupReceipt, ClientError> {
    state
        .cache
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .as_ref()
        .ok_or(ClientError::new("game_not_configured"))?
        .execute(token, confirmed)
        .map_err(|_| ClientError::new("cache_cleanup_failed"))
}

#[tauri::command]
pub fn list_profiles(state: State<'_, RuntimeState>) -> Result<Vec<CustomProfile>, ClientError> {
    ProfileStore::new(state.app_data.clone())
        .list()
        .map_err(|_| ClientError::new("profile_list_failed"))
}

#[tauri::command]
pub async fn export_profile(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    id: String,
) -> Result<bool, ClientError> {
    use tauri_plugin_dialog::DialogExt;
    let export = ProfileStore::new(state.app_data.clone())
        .export(&id)
        .map_err(|_| ClientError::new("profile_export_failed"))?;
    let Some(selected) = app
        .dialog()
        .file()
        .set_file_name(&export.suggested_file_name)
        .add_filter("Wuwa profile", &["json"])
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = selected
        .into_path()
        .map_err(|_| ClientError::new("profile_export_failed"))?;
    let _guard = state
        .gate
        .try_acquire(MaintenanceOperation::ProfileWrite)
        .map_err(|_| ClientError::new("maintenance_busy"))?;
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|_| ClientError::new("profile_export_failed"))?;
    file.write_all(&export.bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| ClientError::new("profile_export_failed"))?;
    Ok(true)
}

#[tauri::command]
pub async fn import_profile(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<Option<ProfileImportCandidate>, ClientError> {
    use tauri_plugin_dialog::DialogExt;
    let Some(selected) = app
        .dialog()
        .file()
        .add_filter("Wuwa profile", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|_| ClientError::new("profile_import_failed"))?;
    let metadata =
        fs::symlink_metadata(&path).map_err(|_| ClientError::new("profile_import_failed"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_SHARE_BYTES
    {
        return Err(ClientError::new("profile_import_failed"));
    }
    let preview = ProfileStore::new(state.app_data.clone())
        .import(
            &path,
            &Catalog::load_embedded().map_err(|_| ClientError::new("catalog_unavailable"))?,
        )
        .map_err(|_| ClientError::new("profile_import_failed"))?;
    let token = Uuid::new_v4();
    let mut imports = state
        .profile_imports
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?;
    imports.retain(|_, value| value.created_at.elapsed() <= TOKEN_LIFETIME);
    while imports.len() >= MAX_PENDING_TOKENS {
        let Some(oldest) = imports
            .iter()
            .min_by_key(|(_, value)| value.created_at)
            .map(|(token, _)| *token)
        else {
            break;
        };
        imports.remove(&oldest);
    }
    imports.insert(
        token,
        StoredProfileImport {
            preview: preview.clone(),
            created_at: Instant::now(),
        },
    );
    Ok(Some(ProfileImportCandidate { token, preview }))
}

#[tauri::command]
pub fn save_imported_profile(
    state: State<'_, RuntimeState>,
    token: Uuid,
    id: String,
    name: String,
) -> Result<CustomProfile, ClientError> {
    let _guard = state
        .gate
        .try_acquire(MaintenanceOperation::ProfileWrite)
        .map_err(|_| ClientError::new("maintenance_busy"))?;
    let preview = {
        let mut imports = state
            .profile_imports
            .lock()
            .map_err(|_| ClientError::new("state_unavailable"))?;
        imports.retain(|_, value| value.created_at.elapsed() <= TOKEN_LIFETIME);
        imports
            .remove(&token)
            .ok_or(ClientError::new("unknown_profile_import"))?
            .preview
    };
    ProfileStore::new(state.app_data.clone())
        .save_import(
            &preview,
            &id,
            &name,
            &Catalog::load_embedded().map_err(|_| ClientError::new("catalog_unavailable"))?,
        )
        .map_err(|_| ClientError::new("profile_import_save_failed"))
}

#[tauri::command]
pub fn list_backups(state: State<'_, RuntimeState>) -> Result<Vec<BackupSummary>, ClientError> {
    let installation = state
        .installation
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .clone()
        .ok_or(ClientError::new("game_not_configured"))?;
    let summaries = BackupStore::new(state.app_data.clone())
        .list(&installation.engine_ini)
        .map_err(|_| ClientError::new("backup_list_failed"))?
        .into_iter()
        .map(|entry| BackupSummary {
            id: entry.backup.id,
            created_at: entry.backup.created_at,
            sha256: entry.backup.sha256,
            reason: entry.backup.reason,
            pinned: entry.backup.pinned,
            integrity: match entry.integrity {
                BackupIntegrity::Verified => "verified",
                BackupIntegrity::Corrupt => "corrupt",
                BackupIntegrity::Missing => "missing",
            }
            .to_owned(),
        })
        .collect::<Vec<_>>();
    Ok(summaries)
}

#[tauri::command]
pub fn pin_backup(
    state: State<'_, RuntimeState>,
    backup_id: String,
    pinned: bool,
) -> Result<(), ClientError> {
    let _guard = state
        .gate
        .try_acquire(MaintenanceOperation::ProfileWrite)
        .map_err(|_| ClientError::new("maintenance_busy"))?;
    let installation = state
        .installation
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .clone()
        .ok_or(ClientError::new("game_not_configured"))?;
    BackupStore::new(state.app_data.clone())
        .pin(&installation.engine_ini, &backup_id, pinned)
        .map(|_| ())
        .map_err(|_| ClientError::new("backup_pin_failed"))
}

#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    confirmed: bool,
) -> Result<(), ClientError> {
    if !confirmed {
        return Err(ClientError::new("update_confirmation_required"));
    }
    let update = state
        .pending_update
        .lock()
        .map_err(|_| ClientError::new("state_unavailable"))?
        .clone()
        .ok_or(ClientError::new("update_not_available"))?;
    let _guard = state.prepare_update()?;
    let progress_app = app.clone();
    let mut downloaded = 0_u64;
    let bytes = update
        .download(
            move |chunk_length, content_length| {
                downloaded = downloaded.saturating_add(chunk_length as u64);
                let _ = progress_app.emit(
                    "update://progress",
                    update_download_progress(downloaded, content_length),
                );
            },
            || {},
        )
        .await
        .map_err(|_| ClientError::new("update_download_failed"))?;
    state.ensure_game_stopped_for_update()?;
    state.suspend_for_update()?;
    if let Err(error) = state.ensure_game_stopped_for_update() {
        resume_supervisor_polling(app.clone(), state.inner());
        return Err(error);
    }
    if update.install(bytes).is_err() {
        resume_supervisor_polling(app.clone(), state.inner());
        return Err(ClientError::new("update_install_failed"));
    }
    app.restart();
}

pub fn emit_supervisor_events(app: &AppHandle, events: Vec<SupervisorEvent>) {
    for event in events {
        let _ = app.emit("supervisor://status", event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_preview_binding_rejects_restart_epochs_and_expiry() {
        let identity = crate::process_control::FocusProcessIdentity {
            pid: 10,
            creation_time_100ns: 100,
            canonical_image: PathBuf::from("/opt/tools/worker.exe"),
        };
        let current = StoredFocusPreview {
            generation: 3,
            epoch: 7,
            controller_token: 1,
            candidates: vec![FocusCandidate {
                identity: identity.clone(),
                display_name: "worker.exe".to_owned(),
                current_priority: PriorityClass::Normal,
                status: crate::process_control::FocusProcessStatus::Eligible,
            }],
            created_at: Instant::now(),
        };
        assert!(focus_binding_is_current(&current, 3, 7));
        assert_eq!(
            focus_exclusion_candidate(&current, 3, 7, &identity),
            Ok(identity.canonical_image.clone())
        );
        assert!(!focus_binding_is_current(&current, 4, 7));
        assert!(!focus_binding_is_current(&current, 3, 8));
        assert!(!focus_binding_is_current(
            &StoredFocusPreview {
                created_at: Instant::now() - TOKEN_LIFETIME - Duration::from_secs(1),
                ..current.clone()
            },
            3,
            7
        ));
        let mut unknown = identity;
        unknown.pid = 11;
        assert_eq!(
            focus_exclusion_candidate(&current, 3, 7, &unknown),
            Err(ClientError::new("invalid_focus_candidate"))
        );
        assert_eq!(
            focus_exclusion_candidate(&current, 4, 7, &unknown),
            Err(ClientError::new("stale_focus_preview"))
        );
    }

    #[test]
    fn shutdown_fails_closed_while_maintenance_is_active_and_retains_its_gate_on_success() {
        let temp = tempfile::tempdir().unwrap();
        let state = RuntimeState::new(temp.path().join("app"), temp.path().join("local"));
        let active = state
            .gate
            .try_acquire(MaintenanceOperation::IniWrite)
            .unwrap();

        assert_eq!(state.shutdown().unwrap_err().code, "maintenance_busy");
        drop(active);

        state.shutdown().unwrap();
        assert_eq!(
            state.gate.active().unwrap(),
            Some(MaintenanceOperation::Shutdown)
        );
    }

    #[test]
    fn restore_binding_rejects_a_different_installation_generation() {
        let source = StoredRestoreSource {
            backup_id: "backup".to_owned(),
            generation: 5,
            created_at: Instant::now(),
        };

        assert!(restore_binding_is_current(&source, 5));
        assert!(!restore_binding_is_current(&source, 6));
    }

    #[test]
    fn partial_process_apply_is_remembered_for_the_next_epoch() {
        assert!(should_remember_process_profile(
            crate::process_control::ApplyStatus::Success
        ));
        assert!(should_remember_process_profile(
            crate::process_control::ApplyStatus::Partial
        ));
        assert!(!should_remember_process_profile(
            crate::process_control::ApplyStatus::Denied
        ));
    }

    #[test]
    fn game_qos_command_requires_both_explicit_opt_in_and_confirmation() {
        assert_eq!(
            validate_game_qos_command(GameQosNormalizationCommand {
                disable_execution_speed_throttling: false,
                confirmed: true,
            }),
            Err(ClientError::new("game_qos_opt_in_required"))
        );
        assert_eq!(
            validate_game_qos_command(GameQosNormalizationCommand {
                disable_execution_speed_throttling: true,
                confirmed: false,
            }),
            Err(ClientError::new("game_qos_confirmation_required"))
        );
        assert_eq!(
            validate_game_qos_command(GameQosNormalizationCommand {
                disable_execution_speed_throttling: true,
                confirmed: true,
            }),
            Ok(GameQosRequest {
                disable_execution_speed_throttling: true
            })
        );
    }

    #[test]
    fn pending_update_metadata_keeps_backend_owned_notes_and_publish_time() {
        assert_eq!(
            update_available_metadata(
                "1.1.0".to_owned(),
                Some("Release notes".to_owned()),
                Some("2026-07-15T00:00:00Z".to_owned()),
            ),
            UpdateAvailable {
                version: "1.1.0".to_owned(),
                notes: Some("Release notes".to_owned()),
                published_at: Some("2026-07-15T00:00:00Z".to_owned()),
            }
        );
    }

    #[test]
    fn update_progress_uses_lossless_decimal_strings() {
        let progress = update_download_progress(u64::MAX - 1, Some(u64::MAX));
        assert_eq!(progress.downloaded, "18446744073709551614");
        assert_eq!(progress.total.as_deref(), Some("18446744073709551615"));
    }

    #[test]
    fn polling_ownership_releases_cleanly_and_an_old_worker_cannot_stop_a_new_one() {
        let temp = tempfile::tempdir().unwrap();
        let state = RuntimeState::new(temp.path().join("app"), temp.path().join("local"));
        assert!(!has_configured_supervisor(&state));
        assert_eq!(state.polling_owner.load(Ordering::SeqCst), 0);

        let old = state.claim_polling().unwrap();
        state.polling_owner.store(0, Ordering::SeqCst);
        let new = state.claim_polling().unwrap();
        state.release_polling(old);

        assert!(state.owns_polling(new));
        state.release_polling(new);
        assert!(state.claim_polling().is_some());
    }
}
