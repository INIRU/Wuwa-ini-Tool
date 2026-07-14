use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use tempfile::TempDir;
use uuid::Uuid;
use wuwa_ini_tool_lib::{
    backup_store::{ApplyReason, BackupStore},
    commands::{
        ClientError, ConfigCommandError, DiffKind, EngineIniPicker, GameRunningProbe,
        IniCommandService, SelectedEngineIni, MAX_ENGINE_INI_BYTES,
    },
    game_discovery::{GameInstallation, InstallationChannel},
    maintenance::MaintenanceGate,
};

struct FakeProbe(AtomicBool);

struct FakePicker(Option<SelectedEngineIni>);

impl EngineIniPicker for FakePicker {
    fn pick_engine_ini(&self) -> Result<Option<SelectedEngineIni>, ConfigCommandError> {
        Ok(self.0.clone())
    }
}

impl FakeProbe {
    fn stopped() -> Self {
        Self(AtomicBool::new(false))
    }

    fn running() -> Self {
        Self(AtomicBool::new(true))
    }
}

impl GameRunningProbe for FakeProbe {
    fn is_running(&self, _executable: &Path) -> Result<bool, ConfigCommandError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

struct StartsBeforeReplace(AtomicUsize);

impl GameRunningProbe for StartsBeforeReplace {
    fn is_running(&self, _executable: &Path) -> Result<bool, ConfigCommandError> {
        Ok(self.0.fetch_add(1, Ordering::SeqCst) >= 1)
    }
}

struct Fixture {
    _temp: TempDir,
    source: PathBuf,
    installation: GameInstallation,
    app_data: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game_root = temp.path().join("Wuthering Waves Game");
        let executable = game_root.join("Client/Binaries/Win64/Client-Win64-Shipping.exe");
        let source = game_root.join("Client/Saved/Config/WindowsNoEditor/Engine.ini");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&executable, b"fixture").unwrap();
        fs::write(
            &source,
            b"[/Script/Engine.Engine]\r\nbSmoothFrameRate=True\r\n",
        )
        .unwrap();
        let installation = GameInstallation {
            channel: InstallationChannel::Manual,
            requires_user_confirmation: false,
            game_root,
            executable,
            engine_ini: source.clone(),
        };
        Self {
            app_data: temp.path().join("app-data"),
            _temp: temp,
            source,
            installation,
        }
    }

    fn service(&self, probe: FakeProbe) -> IniCommandService<FakeProbe> {
        IniCommandService::new(
            self.installation.clone(),
            self.app_data.clone(),
            probe,
            MaintenanceGate::new(),
        )
    }
}

fn utf16le(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xfe];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

#[test]
fn paste_is_a_candidate_only_until_confirmed_apply() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());
    let before = fs::read(&fixture.source).unwrap();
    let replacement = "[SystemSettings]\nr.IniruFPSOpti=1\n";

    let preview = service.preview_paste(replacement.to_owned()).unwrap();

    assert_eq!(fs::read(&fixture.source).unwrap(), before);
    assert_eq!(preview.candidate_text, replacement);
    assert!(preview
        .diff
        .iter()
        .any(|line| line.kind == DiffKind::Removed));
    assert!(preview
        .diff
        .iter()
        .any(|line| line.kind == DiffKind::Added && line.text == "r.IniruFPSOpti=1"));
    service.apply_preview(preview.token, true).unwrap();
    assert_eq!(fs::read_to_string(&fixture.source).unwrap(), replacement);
    assert!(BackupStore::new(fixture.app_data.clone())
        .list(&fixture.source)
        .unwrap()
        .iter()
        .any(|entry| entry.backup.reason == ApplyReason::RawEditor));
}

#[test]
fn import_cancellation_is_not_an_error() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());

    assert!(service.preview_import(None).unwrap().is_none());
    assert!(service
        .preview_import_from_picker(&FakePicker(None))
        .unwrap()
        .is_none());
}

#[test]
fn import_requires_exact_engine_ini_filename() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());

    let result = service.preview_import_from_picker(&FakePicker(Some(SelectedEngineIni {
        file_name: "Scalability.ini".to_owned(),
        bytes: b"[SystemSettings]\nr.Test=1\n".to_vec(),
    })));

    assert!(matches!(result, Err(ConfigCommandError::InvalidFileName)));
}

#[test]
fn paste_and_import_reject_nul_unsupported_encoding_and_oversize() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());

    assert!(matches!(
        service.preview_paste("[SystemSettings]\nkey=\0value".to_owned()),
        Err(ConfigCommandError::ContainsNul)
    ));
    assert!(matches!(
        service.preview_import(Some(SelectedEngineIni {
            file_name: "Engine.ini".to_owned(),
            bytes: vec![0xfe, 0xff, 0, 91],
        })),
        Err(ConfigCommandError::UnsupportedEncoding)
    ));
    assert!(matches!(
        service.preview_paste("x".repeat(MAX_ENGINE_INI_BYTES + 1)),
        Err(ConfigCommandError::InputTooLarge)
    ));
}

#[test]
fn utf16le_import_preserves_encoding_and_does_not_write_during_preview() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());
    let before = fs::read(&fixture.source).unwrap();
    let text = "[SystemSettings]\r\nr.IniruFPSOpti=1\r\n";
    let bytes = utf16le(text);

    let preview = service
        .preview_import(Some(SelectedEngineIni {
            file_name: "ENGINE.INI".to_owned(),
            bytes: bytes.clone(),
        }))
        .unwrap()
        .unwrap();

    assert_eq!(preview.candidate_text, text);
    assert_eq!(fs::read(&fixture.source).unwrap(), before);
    service.apply_preview(preview.token, true).unwrap();
    assert_eq!(fs::read(&fixture.source).unwrap(), bytes);
}

#[test]
fn preview_tokens_are_opaque_single_use_and_confirmation_is_required() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());
    let preview = service.preview_paste("[A]\nx=1\n".to_owned()).unwrap();

    assert!(matches!(
        service.apply_preview(preview.token, false),
        Err(ConfigCommandError::ConfirmationRequired)
    ));
    service.apply_preview(preview.token, true).unwrap();
    assert!(matches!(
        service.apply_preview(preview.token, true),
        Err(ConfigCommandError::UnknownPreview)
    ));
    assert!(matches!(
        service.apply_preview(Uuid::new_v4(), true),
        Err(ConfigCommandError::UnknownPreview)
    ));
}

#[test]
fn apply_rejects_game_running_without_consuming_or_writing_candidate() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::running());
    let before = fs::read(&fixture.source).unwrap();
    let preview = service.preview_paste("[A]\nx=1\n".to_owned()).unwrap();

    assert!(matches!(
        service.apply_preview(preview.token, true),
        Err(ConfigCommandError::GameRunning)
    ));
    assert_eq!(fs::read(&fixture.source).unwrap(), before);
}

#[test]
fn client_errors_never_serialize_internal_paths() {
    let error: ClientError = ConfigCommandError::OperationFailed.into();
    let serialized = serde_json::to_string(&error).unwrap();

    assert_eq!(serialized, r#"{"code":"config_operation_failed"}"#);
    assert!(!serialized.contains("Users"));
    assert!(!serialized.contains("Program Files"));
}

#[test]
fn game_start_after_backup_but_before_replace_leaves_engine_ini_untouched() {
    let fixture = Fixture::new();
    let service = IniCommandService::new(
        fixture.installation.clone(),
        fixture.app_data.clone(),
        StartsBeforeReplace(AtomicUsize::new(0)),
        MaintenanceGate::new(),
    );
    let before = fs::read(&fixture.source).unwrap();
    let preview = service.preview_paste("[A]\nx=1\n".to_owned()).unwrap();

    assert!(matches!(
        service.apply_preview(preview.token, true),
        Err(ConfigCommandError::OperationFailed)
    ));
    assert_eq!(fs::read(&fixture.source).unwrap(), before);
}

#[test]
fn byte_only_encoding_or_line_ending_changes_are_visible_in_diff() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());
    let same_text_with_lf = "[/Script/Engine.Engine]\nbSmoothFrameRate=True\n";

    let preview = service.preview_paste(same_text_with_lf.to_owned()).unwrap();

    assert!(preview.byte_only_change);
    assert_eq!(preview.before_line_endings, "crlf");
    assert_eq!(preview.after_line_endings, "lf");
    assert!(preview
        .diff
        .iter()
        .any(|line| line.kind == DiffKind::Metadata));
}
