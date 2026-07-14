use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::PathBuf,
    time::Duration,
};

use wuwa_ini_tool_lib::{
    game_discovery::{validate_game_executable, GameInstallation},
    maintenance::{MaintenanceError, MaintenanceGate, MaintenanceOperation},
    process_control::{
        CpuTopology, FileFocusJournalStore, FocusActivationRequest, FocusAdaptiveAction,
        FocusBackend, FocusConfig, FocusError, FocusJournal, FocusJournalEntry, FocusJournalStore,
        FocusModeController, FocusProcessIdentity, FocusProcessLoad, FocusProcessSnapshot,
        FocusRestoreOutcome, FocusRuntimeAvailability, FocusTelemetrySample, GameQosRequest,
        PriorityClass, FOCUS_JOURNAL_SCHEMA_VERSION,
    },
    profile_store::ProcessProfile,
    supervisor::{
        CloseAction, FocusLifecycle, FocusLifecycleReport, FocusLifecycleStatus, GameQosLifecycle,
        GameQosLifecycleReport, GameQosLifecycleStatus, ObservedGame, PreparedFocusLifecycle,
        Supervisor, SupervisorApplyOutcome, SupervisorBackend, SupervisorError,
        SupervisorRestoreOutcome, SupervisorState,
    },
};

#[derive(Default)]
struct FakeBackend {
    observations: VecDeque<Option<ObservedGame>>,
    launch_calls: usize,
    apply_calls: Vec<u32>,
    applied_profiles: Vec<ProcessProfile>,
    dangerous_acks: Vec<bool>,
    restore_calls: Vec<u32>,
    apply_outcomes: VecDeque<SupervisorApplyOutcome>,
    restore_failures: usize,
    apply_failures: usize,
}

struct AdapterFocusBackend {
    processes: BTreeMap<u32, FocusProcessSnapshot>,
    deny_priority: bool,
}

impl FocusBackend for AdapterFocusBackend {
    fn enumerate(&mut self) -> Result<Vec<FocusProcessSnapshot>, FocusError> {
        Ok(self.processes.values().cloned().collect())
    }

    fn inspect(&mut self, pid: u32) -> Result<Option<FocusProcessSnapshot>, FocusError> {
        Ok(self.processes.get(&pid).cloned())
    }

    fn set_priority(
        &mut self,
        identity: &FocusProcessIdentity,
        priority: PriorityClass,
    ) -> Result<(), FocusError> {
        if self.deny_priority {
            return Err(FocusError::AccessDenied);
        }
        self.processes
            .get_mut(&identity.pid)
            .ok_or(FocusError::BackendFailure)?
            .priority = priority;
        Ok(())
    }
}

impl SupervisorBackend for FakeBackend {
    fn launch(&mut self, _installation: &GameInstallation) -> Result<(), SupervisorError> {
        self.launch_calls += 1;
        Ok(())
    }

    fn observe(&mut self) -> Result<Option<ObservedGame>, SupervisorError> {
        Ok(self.observations.pop_front().flatten())
    }

    fn apply(
        &mut self,
        process: &ObservedGame,
        profile: &ProcessProfile,
        dangerous_priority_acknowledged: bool,
    ) -> Result<SupervisorApplyOutcome, SupervisorError> {
        self.apply_calls.push(process.pid);
        self.applied_profiles.push(profile.clone());
        self.dangerous_acks.push(dangerous_priority_acknowledged);
        if self.apply_failures > 0 {
            self.apply_failures -= 1;
            return Err(SupervisorError::BackendFailure);
        }
        Ok(self
            .apply_outcomes
            .pop_front()
            .unwrap_or(SupervisorApplyOutcome::Success))
    }

    fn restore(
        &mut self,
        process: &ObservedGame,
    ) -> Result<SupervisorRestoreOutcome, SupervisorError> {
        self.restore_calls.push(process.pid);
        if self.restore_failures > 0 {
            self.restore_failures -= 1;
            return Err(SupervisorError::BackendFailure);
        }
        Ok(SupervisorRestoreOutcome::Restored)
    }
}

#[derive(Default)]
struct FakeFocus {
    calls: Vec<(&'static str, u32, u64)>,
}

#[derive(Default)]
struct FakeGameQos {
    calls: Vec<(&'static str, u32)>,
}

impl GameQosLifecycle for FakeGameQos {
    fn recover(&mut self) -> Result<GameQosLifecycleReport, SupervisorError> {
        self.calls.push(("recover", 0));
        Ok(GameQosLifecycleReport::no_change())
    }

    fn normalize(
        &mut self,
        process: &ObservedGame,
        _request: GameQosRequest,
    ) -> Result<GameQosLifecycleReport, SupervisorError> {
        self.calls.push(("normalize", process.pid));
        Ok(GameQosLifecycleReport {
            status: GameQosLifecycleStatus::Applied,
            prior: Some(wuwa_ini_tool_lib::process_control::GameQosState {
                execution_speed_throttled: true,
            }),
            applied: Some(wuwa_ini_tool_lib::process_control::GameQosState {
                execution_speed_throttled: false,
            }),
            restore_pending: true,
        })
    }

    fn restore(
        &mut self,
        process: &ObservedGame,
    ) -> Result<GameQosLifecycleReport, SupervisorError> {
        self.calls.push(("restore", process.pid));
        Ok(GameQosLifecycleReport {
            status: GameQosLifecycleStatus::Restored,
            prior: None,
            applied: None,
            restore_pending: false,
        })
    }
}

impl FocusLifecycle for FakeFocus {
    fn recover(&mut self) -> Result<FocusLifecycleReport, SupervisorError> {
        self.calls.push(("recover", 0, 0));
        Ok(FocusLifecycleReport::no_changes(0, None))
    }

    fn activate(
        &mut self,
        process: &ObservedGame,
        epoch: u64,
    ) -> Result<FocusLifecycleReport, SupervisorError> {
        self.calls.push(("activate", process.pid, epoch));
        Ok(FocusLifecycleReport {
            epoch,
            process: Some(process.clone()),
            status: FocusLifecycleStatus::Activated,
            process_results: Vec::new(),
            recovery_required: false,
            telemetry: None,
            adaptive_decision: None,
        })
    }

    fn restore(
        &mut self,
        process: &ObservedGame,
        epoch: u64,
    ) -> Result<FocusLifecycleReport, SupervisorError> {
        self.calls.push(("restore", process.pid, epoch));
        Ok(FocusLifecycleReport {
            epoch,
            process: Some(process.clone()),
            status: FocusLifecycleStatus::Restored,
            process_results: Vec::new(),
            recovery_required: false,
            telemetry: None,
            adaptive_decision: None,
        })
    }
}

fn installation() -> (tempfile::TempDir, GameInstallation) {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp
        .path()
        .join("game/Client/Binaries/Win64/Client-Win64-Shipping.exe");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, b"fixture").unwrap();
    let installation = validate_game_executable(&executable).unwrap();
    (temp, installation)
}

fn observed(installation: &GameInstallation, pid: u32, creation: u64) -> ObservedGame {
    ObservedGame {
        pid,
        creation_time_100ns: creation,
        canonical_image: installation.executable.clone(),
    }
}

#[test]
fn observed_game_creation_time_uses_a_lossless_wire_string() {
    let (_temp, installation) = installation();
    let game = observed(&installation, 42, u64::MAX);
    let encoded = serde_json::to_value(&game).unwrap();

    assert_eq!(
        encoded["creation_time_100ns"],
        serde_json::Value::String(u64::MAX.to_string())
    );
    assert_eq!(
        serde_json::from_value::<ObservedGame>(encoded).unwrap(),
        game
    );
}

fn supervisor(backend: FakeBackend, installation: GameInstallation) -> Supervisor<FakeBackend> {
    Supervisor::new(
        backend,
        installation,
        ProcessProfile::default(),
        MaintenanceGate::new(),
    )
}

#[test]
fn supervisor_launches_and_applies_once_to_the_validated_game_process() {
    let (_temp, installation) = installation();
    let process = observed(&installation, 42, 100);
    let mut backend = FakeBackend::default();
    backend
        .observations
        .extend([Some(process.clone()), Some(process)]);
    let mut supervisor = supervisor(backend, installation);

    supervisor.request_launch().unwrap();
    assert_eq!(supervisor.state(), SupervisorState::Launching);
    supervisor.tick().unwrap();
    assert_eq!(supervisor.state(), SupervisorState::WaitingForGame);
    supervisor.tick().unwrap();
    assert_eq!(supervisor.state(), SupervisorState::Applying);
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();

    assert_eq!(supervisor.state(), SupervisorState::Active);
    assert_eq!(supervisor.backend().launch_calls, 1);
    assert_eq!(supervisor.backend().apply_calls, vec![42]);
}

#[test]
fn same_name_wrong_path_is_never_applied() {
    let (_temp, installation) = installation();
    let mut wrong = observed(&installation, 42, 100);
    wrong.canonical_image = PathBuf::from("C:/Other/Client-Win64-Shipping.exe");
    let mut backend = FakeBackend::default();
    backend.observations.push_back(Some(wrong));
    let mut supervisor = supervisor(backend, installation);

    supervisor.request_launch().unwrap();
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();

    assert_eq!(supervisor.state(), SupervisorState::WaitingForGame);
    assert!(supervisor.backend().apply_calls.is_empty());
}

#[test]
fn process_restart_restores_once_then_reapplies_once() {
    let (_temp, installation) = installation();
    let first = observed(&installation, 42, 100);
    let second = observed(&installation, 43, 200);
    let mut backend = FakeBackend::default();
    backend.observations.extend([
        Some(first.clone()),
        Some(first),
        Some(second.clone()),
        Some(second),
    ]);
    let mut supervisor = supervisor(backend, installation);

    supervisor.request_launch().unwrap();
    for _ in 0..8 {
        supervisor.tick().unwrap();
    }

    assert_eq!(supervisor.backend().apply_calls, vec![42, 43]);
    assert_eq!(supervisor.backend().restore_calls, vec![42]);
}

#[test]
fn apply_outcomes_map_to_partial_and_denied_states() {
    for (outcome, expected) in [
        (SupervisorApplyOutcome::Partial, SupervisorState::Partial),
        (SupervisorApplyOutcome::Denied, SupervisorState::Denied),
    ] {
        let (_temp, installation) = installation();
        let mut backend = FakeBackend::default();
        backend
            .observations
            .push_back(Some(observed(&installation, 42, 100)));
        backend.apply_outcomes.push_back(outcome);
        let mut supervisor = supervisor(backend, installation);
        supervisor.request_launch().unwrap();
        for _ in 0..3 {
            supervisor.tick().unwrap();
        }
        assert_eq!(supervisor.state(), expected);
    }
}

#[test]
fn game_exit_and_explicit_quit_restore_exactly_once() {
    let (_temp, installation) = installation();
    let process = observed(&installation, 42, 100);
    let mut backend = FakeBackend::default();
    backend
        .observations
        .extend([Some(process), None, None, None]);
    let mut supervisor = supervisor(backend, installation);
    supervisor.request_launch().unwrap();
    for _ in 0..5 {
        supervisor.tick().unwrap();
    }
    assert_eq!(supervisor.state(), SupervisorState::Exited);
    assert_eq!(supervisor.backend().restore_calls, vec![42]);

    supervisor.request_quit().unwrap();
    supervisor.request_quit().unwrap();
    assert_eq!(supervisor.backend().restore_calls, vec![42]);
}

#[test]
fn explicit_game_qos_normalization_is_restored_by_supervisor_quit() {
    let (_temp, installation) = installation();
    let process = observed(&installation, 42, 100);
    let mut backend = FakeBackend::default();
    backend.observations.push_back(Some(process));
    let mut supervisor = Supervisor::with_focus_and_qos(
        backend,
        FakeFocus::default(),
        FakeGameQos::default(),
        installation,
        ProcessProfile::default(),
        MaintenanceGate::new(),
    );
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();

    let report = supervisor
        .normalize_game_qos(GameQosRequest {
            disable_execution_speed_throttling: true,
        })
        .unwrap();
    assert_eq!(report.status, GameQosLifecycleStatus::Applied);
    assert!(report.restore_pending);
    supervisor.request_quit().unwrap();
    assert_eq!(
        supervisor.game_qos().calls,
        vec![("normalize", 42), ("restore", 42)]
    );
    assert_eq!(
        supervisor
            .drain_game_qos_reports()
            .into_iter()
            .map(|report| report.status)
            .collect::<Vec<_>>(),
        vec![
            GameQosLifecycleStatus::Applied,
            GameQosLifecycleStatus::Restored
        ]
    );
}

#[test]
fn explicit_game_qos_normalization_remains_available_after_process_settings_are_denied() {
    let (_temp, installation) = installation();
    let process = observed(&installation, 42, 100);
    let mut backend = FakeBackend::default();
    backend.observations.push_back(Some(process));
    backend
        .apply_outcomes
        .push_back(SupervisorApplyOutcome::Denied);
    let mut supervisor = Supervisor::with_focus_and_qos(
        backend,
        FakeFocus::default(),
        FakeGameQos::default(),
        installation,
        ProcessProfile::default(),
        MaintenanceGate::new(),
    );
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();
    assert_eq!(supervisor.state(), SupervisorState::Denied);

    assert_eq!(
        supervisor
            .normalize_game_qos(GameQosRequest {
                disable_execution_speed_throttling: true,
            })
            .unwrap()
            .status,
        GameQosLifecycleStatus::Applied
    );
}

#[test]
fn close_to_tray_is_distinct_from_explicit_exit() {
    let (_temp, installation) = installation();
    let mut supervisor = supervisor(FakeBackend::default(), installation);
    assert_eq!(supervisor.handle_close_requested(), CloseAction::HideToTray);
    supervisor.set_close_to_tray(false);
    assert_eq!(supervisor.handle_close_requested(), CloseAction::Exit);
}

#[test]
fn polling_delay_is_bounded_and_never_busy_loops() {
    let (_temp, installation) = installation();
    let supervisor = supervisor(FakeBackend::default(), installation);
    assert!(supervisor.next_poll_delay() >= Duration::from_millis(250));
    assert!(supervisor.next_poll_delay() <= Duration::from_secs(5));
}

#[test]
fn launch_and_cache_cleanup_share_one_exclusive_maintenance_gate() {
    let gate = MaintenanceGate::new();
    let launch = gate
        .try_acquire(MaintenanceOperation::GameLaunch)
        .expect("first operation acquires gate");
    assert!(matches!(
        gate.try_acquire(MaintenanceOperation::CacheCleanup),
        Err(MaintenanceError::Busy)
    ));
    drop(launch);
    assert!(gate.try_acquire(MaintenanceOperation::CacheCleanup).is_ok());
}

#[test]
fn startup_observes_an_already_running_validated_game() {
    let (_temp, installation) = installation();
    let mut backend = FakeBackend::default();
    backend
        .observations
        .push_back(Some(observed(&installation, 42, 100)));
    let mut supervisor = supervisor(backend, installation);

    supervisor.tick().unwrap();
    supervisor.tick().unwrap();

    assert_eq!(supervisor.state(), SupervisorState::Active);
    assert_eq!(supervisor.backend().launch_calls, 0);
    assert_eq!(supervisor.backend().apply_calls, vec![42]);
}

#[test]
fn failed_restore_keeps_old_identity_and_retries_before_replacement() {
    let (_temp, installation) = installation();
    let first = observed(&installation, 42, 100);
    let second = observed(&installation, 43, 200);
    let mut backend = FakeBackend {
        restore_failures: 1,
        ..FakeBackend::default()
    };
    backend
        .observations
        .extend([Some(first), Some(second.clone()), Some(second)]);
    let mut supervisor = supervisor(backend, installation);
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();

    assert_eq!(supervisor.tick(), Err(SupervisorError::BackendFailure));
    assert_eq!(supervisor.active_process().unwrap().pid, 42);
    assert_eq!(supervisor.backend().apply_calls, vec![42]);

    supervisor.tick().unwrap();
    supervisor.tick().unwrap();
    assert_eq!(supervisor.backend().apply_calls, vec![42, 43]);
    assert_eq!(supervisor.backend().restore_calls, vec![42, 42]);
}

#[test]
fn backend_apply_error_is_terminal_for_the_epoch_and_never_retried_implicitly() {
    let (_temp, installation) = installation();
    let mut backend = FakeBackend {
        apply_failures: 1,
        ..FakeBackend::default()
    };
    backend
        .observations
        .push_back(Some(observed(&installation, 42, 100)));
    let mut supervisor = supervisor(backend, installation);
    supervisor.tick().unwrap();

    assert_eq!(supervisor.tick(), Err(SupervisorError::BackendFailure));
    assert_eq!(supervisor.state(), SupervisorState::Denied);
    supervisor.tick().unwrap();
    assert_eq!(supervisor.backend().apply_calls, vec![42]);
}

#[test]
fn focus_mode_is_bound_to_validated_epochs_and_restored_on_restart_disable_and_quit() {
    let (_temp, installation) = installation();
    let first = observed(&installation, 42, 100);
    let second = observed(&installation, 43, 200);
    let mut backend = FakeBackend::default();
    backend.observations.extend([
        Some(first.clone()),
        Some(second.clone()),
        Some(second.clone()),
    ]);
    let mut supervisor = Supervisor::with_focus(
        backend,
        FakeFocus::default(),
        installation,
        ProcessProfile::default(),
        MaintenanceGate::new(),
    );

    assert_eq!(
        supervisor.activate_focus_mode(),
        Err(SupervisorError::NotActive)
    );
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();
    let first_report = supervisor.activate_focus_mode().unwrap();
    assert_eq!(first_report.process.unwrap().pid, 42);
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();
    supervisor.deactivate_focus_mode().unwrap();
    supervisor.request_quit().unwrap();

    assert_eq!(
        supervisor.focus().calls,
        vec![
            ("recover", 0, 0),
            ("activate", 42, 1),
            ("restore", 42, 1),
            ("activate", 43, 2),
            ("restore", 43, 2),
        ]
    );
    assert!(supervisor
        .events()
        .iter()
        .any(|event| event.focus_report.is_some()));
}

#[test]
fn supervisor_events_are_bounded_and_drainable() {
    let (_temp, installation) = installation();
    let mut backend = FakeBackend::default();
    for epoch in 1..=300 {
        backend.observations.extend([
            Some(observed(&installation, epoch, u64::from(epoch))),
            None,
            None,
        ]);
    }
    let mut supervisor = supervisor(backend, installation);
    for _ in 0..300 {
        supervisor.tick().unwrap();
        supervisor.tick().unwrap();
        supervisor.tick().unwrap();
        supervisor.tick().unwrap();
    }

    let events = supervisor.drain_events();
    assert!(events.len() <= 128);
    assert!(supervisor.drain_events().is_empty());
}

#[test]
fn maintenance_state_unavailable_is_not_reported_as_busy() {
    assert_eq!(
        SupervisorError::from(MaintenanceError::StateUnavailable),
        SupervisorError::StateUnavailable
    );
}

#[test]
fn prepared_focus_adapter_returns_typed_per_process_activation_report() {
    let (_temp, installation) = installation();
    let game = observed(&installation, 42, 100);
    let identity = FocusProcessIdentity {
        pid: 77,
        creation_time_100ns: 700,
        canonical_image: PathBuf::from("C:/Apps/background.exe"),
    };
    let background = FocusProcessSnapshot {
        identity: identity.clone(),
        display_name: "background.exe".to_owned(),
        priority: PriorityClass::BelowNormal,
        same_user: true,
        same_session: true,
        session_zero: false,
        system_process: false,
        protected_process: false,
        critical_process: false,
        access_denied: false,
        game_process: false,
        tool_process: false,
        launcher_or_overlay: false,
        foreground_family: false,
        visible_window_family: false,
        active_audio: false,
    };
    let journal_root = tempfile::tempdir().unwrap();
    let mut journal = FileFocusJournalStore::new(journal_root.path());
    journal
        .save(&FocusJournal {
            schema_version: FOCUS_JOURNAL_SCHEMA_VERSION,
            entries: vec![FocusJournalEntry {
                identity: identity.clone(),
                prior_priority: PriorityClass::Normal,
                applied_priority: PriorityClass::BelowNormal,
            }],
        })
        .unwrap();
    let controller = FocusModeController::new(
        AdapterFocusBackend {
            processes: BTreeMap::from([(identity.pid, background)]),
            deny_priority: false,
        },
        journal,
        FocusConfig {
            enabled: true,
            ..FocusConfig::default()
        },
    );
    let mut focus = PreparedFocusLifecycle::new(controller);
    let recovery = focus.recover().unwrap();
    assert_eq!(recovery.status, FocusLifecycleStatus::Recovered);
    assert_eq!(recovery.process_results.len(), 1);
    assert_eq!(
        recovery.process_results[0].outcome,
        FocusRestoreOutcome::Restored
    );
    let preview = focus.preview().unwrap();
    assert_eq!(
        focus
            .controller()
            .backend()
            .processes
            .get(&identity.pid)
            .unwrap()
            .priority,
        PriorityClass::Normal
    );
    focus.arm(FocusActivationRequest {
        preview_token: preview.token,
        selected: vec![identity.clone()],
        select_all_eligible: false,
        select_all_confirmed: false,
    });
    let mut backend = FakeBackend::default();
    backend.observations.push_back(Some(game));
    let mut supervisor = Supervisor::with_focus(
        backend,
        focus,
        installation,
        ProcessProfile::default(),
        MaintenanceGate::new(),
    );
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();

    let report = supervisor.activate_focus_mode().unwrap();

    assert_eq!(report.epoch, 1);
    assert_eq!(report.process_results.len(), 1);
    assert_eq!(report.process_results[0].identity, identity);
    assert_eq!(
        report.process_results[0].outcome,
        FocusRestoreOutcome::Selected
    );
    let sample = FocusTelemetrySample {
        game_foreground: true,
        protection_triggered: false,
        total_cpu_basis_points: 9_000,
        per_logical_cpu_basis_points: Vec::new(),
        game_hot_thread_basis_points: 0,
        selected_process_loads: vec![FocusProcessLoad {
            identity: identity.clone(),
            cpu_basis_points: 1_000,
        }],
    };
    let process = supervisor.active_process().unwrap().clone();
    let mut adaptive = None;
    for _ in 0..3 {
        adaptive = Some(
            supervisor
                .focus_mut()
                .apply_sample(&process, 1, &sample, &CpuTopology::default())
                .unwrap(),
        );
    }
    let adaptive = adaptive.unwrap();
    assert_eq!(adaptive.status, FocusLifecycleStatus::Applied);
    assert_eq!(
        adaptive.adaptive_decision.unwrap().action,
        FocusAdaptiveAction::RestrainPriority
    );
    assert_eq!(adaptive.telemetry.unwrap().competitor_count, 1);
    assert_eq!(
        supervisor
            .focus()
            .controller()
            .backend()
            .processes
            .get(&identity.pid)
            .unwrap()
            .priority,
        PriorityClass::BelowNormal
    );

    supervisor.focus_mut().restore(&process, 1).unwrap();
    let mut reapplied = None;
    for _ in 0..3 {
        reapplied = Some(
            supervisor
                .focus_mut()
                .apply_sample(&process, 1, &sample, &CpuTopology::default())
                .unwrap(),
        );
    }
    assert_eq!(reapplied.unwrap().status, FocusLifecycleStatus::Applied);
}

#[test]
fn focus_preview_is_read_only_even_when_a_recovery_journal_exists() {
    let identity = FocusProcessIdentity {
        pid: 77,
        creation_time_100ns: 700,
        canonical_image: PathBuf::from("C:/Apps/background.exe"),
    };
    let snapshot = FocusProcessSnapshot {
        identity: identity.clone(),
        display_name: "background.exe".to_owned(),
        priority: PriorityClass::BelowNormal,
        same_user: true,
        same_session: true,
        session_zero: false,
        system_process: false,
        protected_process: false,
        critical_process: false,
        access_denied: false,
        game_process: false,
        tool_process: false,
        launcher_or_overlay: false,
        foreground_family: false,
        visible_window_family: false,
        active_audio: false,
    };
    let journal_root = tempfile::tempdir().unwrap();
    let mut journal = FileFocusJournalStore::new(journal_root.path());
    journal
        .save(&FocusJournal {
            schema_version: FOCUS_JOURNAL_SCHEMA_VERSION,
            entries: vec![FocusJournalEntry {
                identity: identity.clone(),
                prior_priority: PriorityClass::Normal,
                applied_priority: PriorityClass::BelowNormal,
            }],
        })
        .unwrap();
    let controller = FocusModeController::new(
        AdapterFocusBackend {
            processes: BTreeMap::from([(identity.pid, snapshot)]),
            deny_priority: false,
        },
        journal,
        FocusConfig {
            enabled: true,
            ..FocusConfig::default()
        },
    );
    let mut focus = PreparedFocusLifecycle::new(controller);

    let preview = focus.preview().unwrap();

    assert_eq!(
        focus
            .controller()
            .backend()
            .processes
            .get(&identity.pid)
            .unwrap()
            .priority,
        PriorityClass::BelowNormal
    );
    assert!(focus.controller().journal_store().load().unwrap().is_some());
    #[cfg(not(target_os = "windows"))]
    assert_eq!(
        preview.runtime_availability,
        FocusRuntimeAvailability::Unavailable
    );
}

#[test]
fn denied_focus_recovery_returns_a_typed_report_instead_of_dropping_results() {
    let identity = FocusProcessIdentity {
        pid: 77,
        creation_time_100ns: 700,
        canonical_image: PathBuf::from("C:/Apps/background.exe"),
    };
    let snapshot = FocusProcessSnapshot {
        identity: identity.clone(),
        display_name: "background.exe".to_owned(),
        priority: PriorityClass::BelowNormal,
        same_user: true,
        same_session: true,
        session_zero: false,
        system_process: false,
        protected_process: false,
        critical_process: false,
        access_denied: false,
        game_process: false,
        tool_process: false,
        launcher_or_overlay: false,
        foreground_family: false,
        visible_window_family: false,
        active_audio: false,
    };
    let journal_root = tempfile::tempdir().unwrap();
    let mut journal = FileFocusJournalStore::new(journal_root.path());
    journal
        .save(&FocusJournal {
            schema_version: FOCUS_JOURNAL_SCHEMA_VERSION,
            entries: vec![FocusJournalEntry {
                identity: identity.clone(),
                prior_priority: PriorityClass::Normal,
                applied_priority: PriorityClass::BelowNormal,
            }],
        })
        .unwrap();
    let controller = FocusModeController::new(
        AdapterFocusBackend {
            processes: BTreeMap::from([(identity.pid, snapshot)]),
            deny_priority: true,
        },
        journal,
        FocusConfig {
            enabled: true,
            ..FocusConfig::default()
        },
    );
    let mut focus = PreparedFocusLifecycle::new(controller);

    let report = focus.recover().unwrap();

    assert_eq!(report.status, FocusLifecycleStatus::RecoveryRequired);
    assert!(report.recovery_required);
    assert_eq!(report.process_results.len(), 1);
    assert_eq!(
        report.process_results[0].outcome,
        FocusRestoreOutcome::Denied
    );
    assert!(focus.controller().journal_store().load().unwrap().is_some());
}

#[test]
fn updated_process_profile_and_ack_are_reapplied_to_the_next_epoch() {
    let (_temp, installation) = installation();
    let first = observed(&installation, 42, 100);
    let second = observed(&installation, 43, 200);
    let mut backend = FakeBackend::default();
    backend
        .observations
        .extend([Some(first), Some(second.clone()), Some(second)]);
    let mut supervisor = supervisor(backend, installation);
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();
    let profile = ProcessProfile {
        priority: PriorityClass::High,
        ..ProcessProfile::default()
    };
    supervisor.set_process_profile(profile.clone(), true);
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();

    assert_eq!(supervisor.backend().applied_profiles[1], profile);
    assert!(supervisor.backend().dangerous_acks[1]);
}

#[test]
fn failed_maintenance_after_suspend_can_resume_monitoring() {
    let (_temp, installation) = installation();
    let first = observed(&installation, 42, 100);
    let second = observed(&installation, 43, 200);
    let mut backend = FakeBackend::default();
    backend
        .observations
        .extend([Some(first), Some(second.clone())]);
    let mut supervisor = supervisor(backend, installation);
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();

    supervisor.suspend_for_maintenance().unwrap();
    assert_eq!(supervisor.state(), SupervisorState::Idle);
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();

    assert_eq!(supervisor.state(), SupervisorState::Active);
    assert_eq!(supervisor.backend().apply_calls, vec![42, 43]);
}
