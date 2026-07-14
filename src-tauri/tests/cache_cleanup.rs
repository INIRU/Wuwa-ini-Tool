use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use tempfile::TempDir;
use wuwa_ini_tool_lib::cache_cleanup::{
    CacheCleanupError, CacheCleanupService, CacheCleanupWarning, CleanupRootOutcome,
    CleanupSelection, CleanupStopReason, GameProcessProbe,
};
use wuwa_ini_tool_lib::maintenance::{MaintenanceGate, MaintenanceOperation};

struct FakeProbe(AtomicBool);

impl FakeProbe {
    fn stopped() -> Self {
        Self(AtomicBool::new(false))
    }

    fn running() -> Self {
        Self(AtomicBool::new(true))
    }
}

struct ErrorProbe;

impl GameProcessProbe for ErrorProbe {
    fn is_running(&self, _executable: &Path) -> Result<bool, CacheCleanupError> {
        Err(CacheCleanupError::StateUnavailable)
    }
}

struct OneShotMutationProbe<F> {
    mutated: AtomicBool,
    mutation: F,
}

impl<F> OneShotMutationProbe<F> {
    fn new(mutation: F) -> Self {
        Self {
            mutated: AtomicBool::new(false),
            mutation,
        }
    }
}

impl<F> GameProcessProbe for OneShotMutationProbe<F>
where
    F: Fn() + Send + Sync,
{
    fn is_running(&self, _executable: &Path) -> Result<bool, CacheCleanupError> {
        if !self.mutated.swap(true, Ordering::SeqCst) {
            (self.mutation)();
        }
        Ok(false)
    }
}

struct StartsOnProbe {
    calls: AtomicUsize,
    start_on: usize,
}

impl StartsOnProbe {
    fn new(start_on: usize) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            start_on,
        }
    }
}

impl GameProcessProbe for StartsOnProbe {
    fn is_running(&self, _executable: &Path) -> Result<bool, CacheCleanupError> {
        Ok(self.calls.fetch_add(1, Ordering::SeqCst) + 1 >= self.start_on)
    }
}

struct FailsOnProbe {
    calls: AtomicUsize,
    fail_on: usize,
}

impl FailsOnProbe {
    fn new(fail_on: usize) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            fail_on,
        }
    }
}

impl GameProcessProbe for FailsOnProbe {
    fn is_running(&self, _executable: &Path) -> Result<bool, CacheCleanupError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) + 1 >= self.fail_on {
            Err(CacheCleanupError::StateUnavailable)
        } else {
            Ok(false)
        }
    }
}

impl GameProcessProbe for FakeProbe {
    fn is_running(&self, _executable: &Path) -> Result<bool, CacheCleanupError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

struct Fixture {
    _temp: TempDir,
    executable: std::path::PathBuf,
    game_root: std::path::PathBuf,
    local_app_data: std::path::PathBuf,
    receipts: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game_root = temp.path().join("Wuthering Waves Game");
        let executable = game_root.join("Client/Binaries/Win64/Client-Win64-Shipping.exe");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"fixture").unwrap();
        let local_app_data = temp.path().join("LocalAppData");
        fs::create_dir_all(&local_app_data).unwrap();
        let receipts = temp.path().join("app-data");
        fs::create_dir_all(&receipts).unwrap();
        Self {
            _temp: temp,
            executable,
            game_root,
            local_app_data,
            receipts,
        }
    }

    fn service<P: GameProcessProbe>(&self, probe: P) -> CacheCleanupService<P> {
        CacheCleanupService::new(
            self.executable.clone(),
            self.local_app_data.clone(),
            self.receipts.clone(),
            probe,
        )
        .unwrap()
    }
}

#[test]
fn shared_maintenance_gate_blocks_cleanup_during_game_launch() {
    let fixture = Fixture::new();
    let cache = fixture.game_root.join("Client/Saved/PSO/cache.bin");
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    fs::write(&cache, b"cache").unwrap();
    let gate = MaintenanceGate::new();
    let service = CacheCleanupService::new_with_gate(
        fixture.executable.clone(),
        fixture.local_app_data.clone(),
        fixture.receipts.clone(),
        FakeProbe::stopped(),
        gate.clone(),
    )
    .unwrap();
    let preview = service.preview(CleanupSelection::wuwa_only()).unwrap();
    let _launch = gate.try_acquire(MaintenanceOperation::GameLaunch).unwrap();

    assert!(matches!(
        service.execute(preview.token(), true),
        Err(CacheCleanupError::MaintenanceBusy)
    ));
    assert_eq!(fs::read(cache).unwrap(), b"cache");
}

#[test]
fn preview_requires_an_explicit_non_empty_target_selection() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());

    let result = service.preview(CleanupSelection::default());

    assert!(matches!(result, Err(CacheCleanupError::EmptySelection)));
}

#[test]
fn wuwa_preview_counts_only_exact_shader_cache_roots() {
    let fixture = Fixture::new();
    let pso = fixture.game_root.join("Client/Saved/PSO/D3D12/rton");
    let report = fixture.game_root.join("Client/Saved/PSOReport");
    let local_storage = fixture.game_root.join("Client/Saved/LocalStorage");
    fs::create_dir_all(&pso).unwrap();
    fs::create_dir_all(&report).unwrap();
    fs::create_dir_all(&local_storage).unwrap();
    fs::write(pso.join("cache.bin"), vec![1_u8; 7]).unwrap();
    fs::write(report.join("report.bin"), vec![2_u8; 11]).unwrap();
    fs::write(local_storage.join("account.json"), vec![3_u8; 13]).unwrap();
    let service = fixture.service(FakeProbe::stopped());

    let preview = service.preview(CleanupSelection::wuwa_only()).unwrap();

    assert_eq!(preview.total_files(), 2);
    assert_eq!(preview.total_bytes(), 18);
    assert!(preview
        .roots()
        .iter()
        .all(|root| root.path().ends_with("PSO") || root.path().ends_with("PSOReport")));
    assert!(preview
        .roots()
        .iter()
        .all(|root| !root.path().to_string_lossy().contains("LocalStorage")));
    assert!(preview
        .warnings()
        .contains(&CacheCleanupWarning::TroubleshootingOnly));
    assert!(preview
        .warnings()
        .contains(&CacheCleanupWarning::ShaderRebuildMayStutter));
    assert!(preview
        .warnings()
        .contains(&CacheCleanupWarning::NoBackupOrRestore));
    assert!(!preview
        .warnings()
        .contains(&CacheCleanupWarning::NvidiaCacheIsDriverWide));
}

#[test]
fn nvidia_preview_is_explicitly_labeled_as_driver_wide() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());

    let preview = service.preview(CleanupSelection::nvidia_only()).unwrap();

    assert!(preview
        .warnings()
        .contains(&CacheCleanupWarning::NvidiaCacheIsDriverWide));
}

#[test]
fn execution_requires_confirmation_and_refuses_if_game_started() {
    let fixture = Fixture::new();
    let cache = fixture.game_root.join("Client/Saved/PSO/cache.bin");
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    fs::write(&cache, b"cache").unwrap();
    let service = fixture.service(FakeProbe::running());
    let preview = service.preview(CleanupSelection::wuwa_only()).unwrap();

    assert!(matches!(
        service.execute(preview.token(), false),
        Err(CacheCleanupError::ConfirmationRequired)
    ));
    assert!(matches!(
        service.execute(preview.token(), true),
        Err(CacheCleanupError::GameRunning)
    ));
    assert!(cache.exists());
}

#[test]
fn nvidia_only_cleanup_preserves_wuwa_and_unrelated_local_app_data() {
    let fixture = Fixture::new();
    let wuwa = fixture.game_root.join("Client/Saved/PSO/cache.bin");
    let dx = fixture.local_app_data.join("NVIDIA/DXCache/cache.bin");
    let unrelated = fixture.local_app_data.join("Discord/state.json");
    for path in [&wuwa, &dx, &unrelated] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"data").unwrap();
    }
    let service = fixture.service(FakeProbe::stopped());
    let preview = service.preview(CleanupSelection::nvidia_only()).unwrap();

    let receipt = service.execute(preview.token(), true).unwrap();

    assert_eq!(receipt.deleted_files(), 1);
    assert_eq!(receipt.roots()[0].outcome(), CleanupRootOutcome::Complete);
    assert!(!dx.exists());
    assert!(dx.parent().unwrap().exists());
    assert!(wuwa.exists());
    assert!(unrelated.exists());
}

#[test]
fn both_selection_deletes_all_five_allowlisted_roots_and_preserves_roots() {
    let fixture = Fixture::new();
    let files = [
        fixture.game_root.join("Client/Saved/PSO/cache.bin"),
        fixture.game_root.join("Client/Saved/PSOReport/report.bin"),
        fixture.local_app_data.join("NVIDIA/DXCache/dx.bin"),
        fixture.local_app_data.join("NVIDIA/GLCache/gl.bin"),
        fixture
            .local_app_data
            .join("NVIDIA Corporation/NV_Cache/nv.bin"),
    ];
    for path in &files {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"cache").unwrap();
    }
    let service = fixture.service(FakeProbe::stopped());
    let preview = service
        .preview(CleanupSelection {
            wuwa: true,
            nvidia: true,
        })
        .unwrap();

    let receipt = service.execute(preview.token(), true).unwrap();

    assert_eq!(receipt.deleted_files(), 5);
    for path in files {
        assert!(!path.exists());
        assert!(path.parent().unwrap().exists());
    }
}

#[cfg(target_os = "windows")]
#[test]
fn windows_nested_directory_deletes_all_planned_siblings_after_parent_mtime_changes() {
    let fixture = Fixture::new();
    let nested = fixture.game_root.join("Client/Saved/PSO/nested");
    let first = nested.join("first.bin");
    let second = nested.join("second.bin");
    fs::create_dir_all(&nested).unwrap();
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();
    let service = fixture.service(FakeProbe::stopped());
    let preview = service.preview(CleanupSelection::wuwa_only()).unwrap();

    let receipt = service.execute(preview.token(), true).unwrap();

    assert!(!first.exists());
    assert!(!second.exists());
    assert!(!nested.exists());
    assert_eq!(receipt.roots()[0].outcome(), CleanupRootOutcome::Complete);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_boundary_to_root_pins_block_intermediate_parent_rename() {
    let fixture = Fixture::new();
    let cache = fixture.game_root.join("Client/Saved/PSO/cache.bin");
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    fs::write(&cache, b"cache").unwrap();
    let saved = fixture.game_root.join("Client/Saved");
    let moved = fixture.game_root.join("Client/Saved-race-target");
    let rename_was_blocked = std::sync::Arc::new(AtomicBool::new(false));
    let result_flag = rename_was_blocked.clone();
    let service = fixture.service(OneShotMutationProbe::new(move || {
        result_flag.store(fs::rename(&saved, &moved).is_err(), Ordering::SeqCst);
    }));
    let preview = service.preview(CleanupSelection::wuwa_only()).unwrap();

    let receipt = service.execute(preview.token(), true).unwrap();

    assert!(rename_was_blocked.load(Ordering::SeqCst));
    assert!(!cache.exists());
    assert_eq!(receipt.roots()[0].outcome(), CleanupRootOutcome::Complete);
}

#[test]
fn game_start_aborts_both_selection_before_nvidia_is_touched() {
    let fixture = Fixture::new();
    let nvidia = fixture.local_app_data.join("NVIDIA/DXCache/cache.bin");
    fs::create_dir_all(nvidia.parent().unwrap()).unwrap();
    fs::write(&nvidia, b"cache").unwrap();
    let service = fixture.service(FakeProbe::running());
    let preview = service
        .preview(CleanupSelection {
            wuwa: true,
            nvidia: true,
        })
        .unwrap();

    assert!(matches!(
        service.execute(preview.token(), true),
        Err(CacheCleanupError::GameRunning)
    ));
    assert!(nvidia.exists());
}

#[test]
fn game_start_between_wuwa_roots_stops_every_remaining_target_with_a_receipt() {
    let fixture = Fixture::new();
    let pso = fixture.game_root.join("Client/Saved/PSO/cache.bin");
    let report = fixture.game_root.join("Client/Saved/PSOReport/report.bin");
    let nvidia = fixture.local_app_data.join("NVIDIA/DXCache/cache.bin");
    for path in [&pso, &report, &nvidia] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"cache").unwrap();
    }
    let service = fixture.service(StartsOnProbe::new(3));
    let preview = service
        .preview(CleanupSelection {
            wuwa: true,
            nvidia: true,
        })
        .unwrap();

    let receipt = service.execute(preview.token(), true).unwrap();

    assert!(!pso.exists());
    assert!(report.exists());
    assert!(nvidia.exists());
    assert_eq!(receipt.stop_reason(), Some(CleanupStopReason::GameStarted));
    assert_eq!(receipt.roots()[0].outcome(), CleanupRootOutcome::Complete);
    assert!(receipt.roots()[1..]
        .iter()
        .all(|root| root.outcome() == CleanupRootOutcome::Skipped));
}

#[test]
fn game_start_after_wuwa_roots_still_stops_every_nvidia_target() {
    let fixture = Fixture::new();
    let pso = fixture.game_root.join("Client/Saved/PSO/cache.bin");
    let report = fixture.game_root.join("Client/Saved/PSOReport/report.bin");
    let nvidia = fixture.local_app_data.join("NVIDIA/DXCache/cache.bin");
    for path in [&pso, &report, &nvidia] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"cache").unwrap();
    }
    let service = fixture.service(StartsOnProbe::new(4));
    let preview = service
        .preview(CleanupSelection {
            wuwa: true,
            nvidia: true,
        })
        .unwrap();

    let receipt = service.execute(preview.token(), true).unwrap();

    assert!(!pso.exists());
    assert!(!report.exists());
    assert!(nvidia.exists());
    assert_eq!(receipt.stop_reason(), Some(CleanupStopReason::GameStarted));
    assert!(receipt.roots()[2..]
        .iter()
        .all(|root| root.outcome() == CleanupRootOutcome::Skipped));
}

#[test]
fn mid_cleanup_process_probe_failure_returns_a_partial_receipt_and_stops() {
    let fixture = Fixture::new();
    let pso = fixture.game_root.join("Client/Saved/PSO/cache.bin");
    let report = fixture.game_root.join("Client/Saved/PSOReport/report.bin");
    for path in [&pso, &report] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"cache").unwrap();
    }
    let service = fixture.service(FailsOnProbe::new(3));
    let preview = service.preview(CleanupSelection::wuwa_only()).unwrap();

    let receipt = service.execute(preview.token(), true).unwrap();

    assert!(!pso.exists());
    assert!(report.exists());
    assert_eq!(
        receipt.stop_reason(),
        Some(CleanupStopReason::ProcessStateUnavailable)
    );
    assert_eq!(receipt.roots()[0].outcome(), CleanupRootOutcome::Complete);
    assert_eq!(receipt.roots()[1].outcome(), CleanupRootOutcome::Skipped);
}

#[test]
fn process_probe_failure_aborts_before_any_selected_root_is_touched() {
    let fixture = Fixture::new();
    let wuwa = fixture.game_root.join("Client/Saved/PSO/cache.bin");
    let nvidia = fixture.local_app_data.join("NVIDIA/DXCache/cache.bin");
    for path in [&wuwa, &nvidia] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"cache").unwrap();
    }
    let service = fixture.service(ErrorProbe);
    let preview = service
        .preview(CleanupSelection {
            wuwa: true,
            nvidia: true,
        })
        .unwrap();

    assert!(matches!(
        service.execute(preview.token(), true),
        Err(CacheCleanupError::StateUnavailable)
    ));
    assert!(wuwa.exists());
    assert!(nvidia.exists());
}

#[test]
fn a_cleanup_preview_token_is_single_use() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());
    let preview = service.preview(CleanupSelection::nvidia_only()).unwrap();

    service.execute(preview.token(), true).unwrap();

    assert!(matches!(
        service.execute(preview.token(), true),
        Err(CacheCleanupError::UnknownPreview)
    ));
}

#[test]
fn a_changed_cache_requires_a_new_preview_before_any_deletion() {
    let fixture = Fixture::new();
    let first = fixture.local_app_data.join("NVIDIA/DXCache/first.bin");
    let added = fixture.local_app_data.join("NVIDIA/DXCache/added.bin");
    fs::create_dir_all(first.parent().unwrap()).unwrap();
    fs::write(&first, b"first").unwrap();
    let service = fixture.service(FakeProbe::stopped());
    let preview = service.preview(CleanupSelection::nvidia_only()).unwrap();
    fs::write(&added, b"added-after-preview").unwrap();

    assert!(matches!(
        service.execute(preview.token(), true),
        Err(CacheCleanupError::CacheChanged)
    ));
    assert!(first.exists());
    assert!(added.exists());
}

#[test]
fn an_entry_added_after_secure_preparation_is_preserved_and_reported_changed() {
    let fixture = Fixture::new();
    let original = fixture.game_root.join("Client/Saved/PSO/original.bin");
    let late = fixture.game_root.join("Client/Saved/PSO/late.bin");
    fs::create_dir_all(original.parent().unwrap()).unwrap();
    fs::write(&original, b"original").unwrap();
    let mutation_path = late.clone();
    let service = fixture.service(OneShotMutationProbe::new(move || {
        fs::write(&mutation_path, b"late").unwrap();
    }));
    let preview = service.preview(CleanupSelection::wuwa_only()).unwrap();

    let receipt = service.execute(preview.token(), true).unwrap();

    assert!(!original.exists());
    assert!(late.exists());
    assert_eq!(receipt.roots()[0].outcome(), CleanupRootOutcome::Changed);
    assert!(receipt.roots()[0].changed_entries() >= 1);
}

#[cfg(unix)]
#[test]
fn a_directory_replaced_by_a_symlink_after_preparation_never_deletes_the_target() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let directory = fixture.game_root.join("Client/Saved/PSO/nested");
    let moved = fixture
        .game_root
        .join("Client/Saved/PSO/nested-before-swap");
    let original = directory.join("cache.bin");
    let outside = fixture.game_root.join("Client/SaveGames/cache.bin");
    fs::create_dir_all(&directory).unwrap();
    fs::create_dir_all(outside.parent().unwrap()).unwrap();
    fs::write(&original, b"cache").unwrap();
    fs::write(&outside, b"save").unwrap();
    let mutation_directory = directory.clone();
    let mutation_moved = moved.clone();
    let outside_directory = outside.parent().unwrap().to_path_buf();
    let service = fixture.service(OneShotMutationProbe::new(move || {
        fs::rename(&mutation_directory, &mutation_moved).unwrap();
        symlink(&outside_directory, &mutation_directory).unwrap();
    }));
    let preview = service.preview(CleanupSelection::wuwa_only()).unwrap();

    let receipt = service.execute(preview.token(), true).unwrap();

    assert_eq!(fs::read(&outside).unwrap(), b"save");
    assert_eq!(fs::read(moved.join("cache.bin")).unwrap(), b"cache");
    assert_eq!(receipt.roots()[0].outcome(), CleanupRootOutcome::Changed);
}

#[cfg(unix)]
#[test]
fn an_approved_file_moved_behind_a_reparse_parent_is_not_deleted_by_identity_alone() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let directory = fixture.game_root.join("Client/Saved/PSO/nested");
    let moved = fixture.game_root.join("Client/SaveGames/moved-cache");
    let original = directory.join("cache.bin");
    fs::create_dir_all(&directory).unwrap();
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::write(&original, b"cache").unwrap();
    let mutation_directory = directory.clone();
    let mutation_moved = moved.clone();
    let service = fixture.service(OneShotMutationProbe::new(move || {
        fs::rename(&mutation_directory, &mutation_moved).unwrap();
        symlink(&mutation_moved, &mutation_directory).unwrap();
    }));
    let preview = service.preview(CleanupSelection::wuwa_only()).unwrap();

    let receipt = service.execute(preview.token(), true).unwrap();

    assert_eq!(fs::read(moved.join("cache.bin")).unwrap(), b"cache");
    assert_eq!(receipt.roots()[0].outcome(), CleanupRootOutcome::Changed);
}

#[test]
fn pending_preview_storage_is_bounded_and_evicts_the_oldest_token() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());
    let oldest = service.preview(CleanupSelection::nvidia_only()).unwrap();
    for _ in 0..32 {
        service.preview(CleanupSelection::nvidia_only()).unwrap();
    }

    assert!(matches!(
        service.execute(oldest.token(), true),
        Err(CacheCleanupError::UnknownPreview)
    ));
}

#[test]
fn maintenance_receipts_are_bounded_and_do_not_store_paths_or_file_names() {
    let fixture = Fixture::new();
    let service = fixture.service(FakeProbe::stopped());
    for _ in 0..55 {
        let preview = service.preview(CleanupSelection::nvidia_only()).unwrap();
        service.execute(preview.token(), true).unwrap();
    }

    let receipt_files = fs::read_dir(&fixture.receipts)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("maintenance-")
        })
        .collect::<Vec<_>>();
    assert_eq!(receipt_files.len(), 50);
    for receipt_path in receipt_files {
        let text = fs::read_to_string(receipt_path).unwrap();
        let _: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(!text.contains("DXCache"));
        assert!(!text.contains("cache.bin"));
        assert!(!text.contains(fixture._temp.path().to_string_lossy().as_ref()));
    }
}

#[cfg(unix)]
#[test]
fn unsafe_receipt_store_aborts_before_cleanup() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let cache = fixture.local_app_data.join("NVIDIA/DXCache/cache.bin");
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    fs::write(&cache, b"cache").unwrap();
    let service = fixture.service(FakeProbe::stopped());
    let original_receipts = fixture._temp.path().join("old-receipts");
    let outside = fixture._temp.path().join("outside-receipts");
    fs::rename(&fixture.receipts, &original_receipts).unwrap();
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, &fixture.receipts).unwrap();
    let preview = service.preview(CleanupSelection::nvidia_only()).unwrap();

    let result = service.execute(preview.token(), true);

    assert!(matches!(
        result,
        Err(CacheCleanupError::InvalidReceiptStore)
    ));
    assert!(cache.exists());
}

#[cfg(unix)]
#[test]
fn service_rejects_an_allowlisted_root_replaced_with_a_symlink() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let outside = fixture.game_root.join("Client/SaveGames");
    let pso = fixture.game_root.join("Client/Saved/PSO");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(pso.parent().unwrap()).unwrap();
    symlink(&outside, &pso).unwrap();

    let result = CacheCleanupService::new(
        fixture.executable.clone(),
        fixture.local_app_data.clone(),
        fixture.receipts.clone(),
        FakeProbe::stopped(),
    );

    assert!(matches!(result, Err(CacheCleanupError::UnsafePath(_))));
}

#[cfg(unix)]
#[test]
fn cleanup_never_follows_a_symlink_child() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let pso = fixture.game_root.join("Client/Saved/PSO");
    let outside = fixture.game_root.join("Client/SaveGames/player.dat");
    fs::create_dir_all(&pso).unwrap();
    fs::create_dir_all(outside.parent().unwrap()).unwrap();
    fs::write(&outside, b"save").unwrap();
    symlink(&outside, pso.join("linked-save")).unwrap();
    let service = fixture.service(FakeProbe::stopped());
    let preview = service.preview(CleanupSelection::wuwa_only()).unwrap();

    let receipt = service.execute(preview.token(), true).unwrap();

    assert!(outside.exists());
    assert_eq!(receipt.skipped_entries(), 1);
}
