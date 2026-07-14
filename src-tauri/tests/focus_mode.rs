use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    rc::Rc,
};

use wuwa_ini_tool_lib::process_control::{
    background_headroom_cpu_sets, evaluate_focus_candidate, AdaptiveFocusPolicy, CpuSelection,
    CpuSetInfo, CpuTopology, FileFocusJournalStore, FocusActivationRequest, FocusAdaptiveAction,
    FocusAdaptiveDecision, FocusBackend, FocusConfig, FocusContentionKind, FocusError,
    FocusJournal, FocusJournalEntry, FocusJournalStore, FocusModeController, FocusProcessIdentity,
    FocusProcessLoad, FocusProcessSnapshot, FocusProcessStatus, FocusRestoreOutcome,
    FocusTelemetrySample, FocusThresholds, PriorityClass, ProcessorGroup,
    SystemFocusTelemetrySampler, FOCUS_JOURNAL_SCHEMA_VERSION,
};

fn identity(pid: u32, image: &str) -> FocusProcessIdentity {
    FocusProcessIdentity {
        pid,
        creation_time_100ns: u64::from(pid) * 100,
        canonical_image: PathBuf::from(image),
    }
}

fn normal_process(pid: u32, image: &str) -> FocusProcessSnapshot {
    FocusProcessSnapshot {
        identity: identity(pid, image),
        display_name: image.to_owned(),
        priority: PriorityClass::Normal,
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
    }
}

fn adaptive_decision(action: FocusAdaptiveAction) -> FocusAdaptiveDecision {
    FocusAdaptiveDecision {
        contention: FocusContentionKind::None,
        action,
        priority_targets: vec![identity(10, "one.exe")],
        background_cpu_set_ids: Vec::new(),
        game_cpu_selection: CpuSelection::All,
        thresholds: FocusThresholds::default(),
    }
}

#[derive(Clone, Default)]
struct FakeBackend {
    processes: BTreeMap<u32, FocusProcessSnapshot>,
    set_denied: BTreeSet<u32>,
    inspect_denied: BTreeSet<u32>,
    events: Rc<RefCell<Vec<String>>>,
}

impl FakeBackend {
    fn with_processes(processes: impl IntoIterator<Item = FocusProcessSnapshot>) -> Self {
        Self {
            processes: processes
                .into_iter()
                .map(|process| (process.identity.pid, process))
                .collect(),
            ..Self::default()
        }
    }
}

impl FocusBackend for FakeBackend {
    fn enumerate(&mut self) -> Result<Vec<FocusProcessSnapshot>, FocusError> {
        Ok(self.processes.values().cloned().collect())
    }

    fn inspect(&mut self, pid: u32) -> Result<Option<FocusProcessSnapshot>, FocusError> {
        if self.inspect_denied.contains(&pid) {
            Err(FocusError::AccessDenied)
        } else {
            Ok(self.processes.get(&pid).cloned())
        }
    }

    fn set_priority(
        &mut self,
        identity: &FocusProcessIdentity,
        priority: PriorityClass,
    ) -> Result<(), FocusError> {
        let pid = identity.pid;
        self.events.borrow_mut().push(format!("set:{pid}"));
        if self.set_denied.contains(&pid) {
            return Err(FocusError::AccessDenied);
        }
        let process = self
            .processes
            .get_mut(&pid)
            .ok_or(FocusError::BackendFailure)?;
        if process.identity != *identity {
            return Err(FocusError::BackendFailure);
        }
        process.priority = priority;
        Ok(())
    }
}

#[derive(Clone, Default)]
struct MemoryJournalStore {
    journal: Option<FocusJournal>,
    fail_save: bool,
    events: Rc<RefCell<Vec<String>>>,
}

impl FocusJournalStore for MemoryJournalStore {
    fn load(&self) -> Result<Option<FocusJournal>, FocusError> {
        Ok(self.journal.clone())
    }

    fn save(&mut self, journal: &FocusJournal) -> Result<(), FocusError> {
        self.events.borrow_mut().push("save".to_owned());
        if self.fail_save {
            return Err(FocusError::JournalFailure);
        }
        self.journal = Some(journal.clone());
        Ok(())
    }

    fn clear(&mut self) -> Result<(), FocusError> {
        self.events.borrow_mut().push("clear".to_owned());
        self.journal = None;
        Ok(())
    }
}

fn controller(
    processes: impl IntoIterator<Item = FocusProcessSnapshot>,
    enabled: bool,
) -> FocusModeController<FakeBackend, MemoryJournalStore> {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut backend = FakeBackend::with_processes(processes);
    backend.events = events.clone();
    let store = MemoryJournalStore {
        events,
        ..MemoryJournalStore::default()
    };
    FocusModeController::new(
        backend,
        store,
        FocusConfig {
            enabled,
            ..FocusConfig::default()
        },
    )
}

#[test]
fn focus_mode_is_default_off_and_preview_never_mutates() {
    assert!(!FocusConfig::default().enabled);
    let process = normal_process(10, "editor.exe");
    let mut controller = controller([process.clone()], false);

    let preview = controller.preview().unwrap();

    assert_eq!(preview.candidates[0].status, FocusProcessStatus::Eligible);
    assert_eq!(preview.thresholds, FocusThresholds::default());
    assert!(controller.backend().events.borrow().is_empty());
    assert_eq!(
        controller.activate(&FocusActivationRequest {
            preview_token: preview.token,
            selected: vec![process.identity],
            select_all_eligible: false,
            select_all_confirmed: false,
        }),
        Err(FocusError::Disabled)
    );
    assert!(controller.backend().events.borrow().is_empty());
}

#[test]
fn select_all_refreshes_new_eligible_processes_but_explicit_selection_does_not() {
    let first = normal_process(10, "first.exe");
    let second = normal_process(11, "second.exe");
    let mut all = controller([first.clone()], true);
    let preview = all.preview().unwrap();
    all.activate(&FocusActivationRequest {
        preview_token: preview.token,
        selected: Vec::new(),
        select_all_eligible: true,
        select_all_confirmed: true,
    })
    .unwrap();
    all.backend_mut()
        .processes
        .insert(second.identity.pid, second.clone());
    all.refresh_selected().unwrap();
    assert!(all.selected().contains(&first.identity));
    assert!(all.selected().contains(&second.identity));

    let mut explicit = controller([first.clone()], true);
    let preview = explicit.preview().unwrap();
    explicit
        .activate(&FocusActivationRequest {
            preview_token: preview.token,
            selected: vec![first.identity.clone()],
            select_all_eligible: false,
            select_all_confirmed: false,
        })
        .unwrap();
    explicit
        .backend_mut()
        .processes
        .insert(second.identity.pid, second.clone());
    explicit.refresh_selected().unwrap();
    assert!(explicit.selected().contains(&first.identity));
    assert!(!explicit.selected().contains(&second.identity));
}

#[test]
fn telemetry_thresholds_are_bounded_before_they_reach_preview_or_policy() {
    let unsafe_thresholds = FocusThresholds {
        sample_interval_ms: 1,
        aggregate_contention_basis_points: u16::MAX,
        release_basis_points: u16::MAX,
        game_hot_thread_basis_points: u16::MAX,
        competitor_basis_points: u16::MAX,
        sustained_samples: 0,
        release_samples: u8::MAX,
    };
    let bounded = unsafe_thresholds.bounded();

    assert_eq!(bounded.sample_interval_ms, 250);
    assert_eq!(bounded.aggregate_contention_basis_points, 10_000);
    assert_eq!(bounded.release_basis_points, 9_999);
    assert_eq!(bounded.game_hot_thread_basis_points, 10_000);
    assert_eq!(bounded.competitor_basis_points, 10_000);
    assert_eq!(bounded.sustained_samples, 2);
    assert_eq!(bounded.release_samples, 10);
}

#[cfg(not(target_os = "windows"))]
#[test]
fn system_telemetry_sampler_is_deterministically_unavailable_off_windows() {
    assert!(matches!(
        SystemFocusTelemetrySampler::new(FocusThresholds::default()),
        Err(FocusError::TelemetryUnavailable)
    ));
}

#[test]
fn preview_excludes_every_non_overridable_safety_category() {
    type SafetyMutation = Box<dyn Fn(&mut FocusProcessSnapshot)>;

    let config = FocusConfig::default();
    let cases: Vec<(FocusProcessStatus, SafetyMutation)> = vec![
        (
            FocusProcessStatus::AccessDenied,
            Box::new(|p| p.access_denied = true),
        ),
        (
            FocusProcessStatus::DifferentUser,
            Box::new(|p| p.same_user = false),
        ),
        (
            FocusProcessStatus::DifferentSession,
            Box::new(|p| p.same_session = false),
        ),
        (
            FocusProcessStatus::SessionZero,
            Box::new(|p| p.session_zero = true),
        ),
        (
            FocusProcessStatus::System,
            Box::new(|p| p.system_process = true),
        ),
        (
            FocusProcessStatus::Protected,
            Box::new(|p| p.protected_process = true),
        ),
        (
            FocusProcessStatus::Critical,
            Box::new(|p| p.critical_process = true),
        ),
        (
            FocusProcessStatus::Game,
            Box::new(|p| p.game_process = true),
        ),
        (
            FocusProcessStatus::Tool,
            Box::new(|p| p.tool_process = true),
        ),
        (
            FocusProcessStatus::LauncherOrOverlay,
            Box::new(|p| p.launcher_or_overlay = true),
        ),
        (
            FocusProcessStatus::Foreground,
            Box::new(|p| p.foreground_family = true),
        ),
        (
            FocusProcessStatus::VisibleWindow,
            Box::new(|p| p.visible_window_family = true),
        ),
        (
            FocusProcessStatus::ActiveAudio,
            Box::new(|p| p.active_audio = true),
        ),
    ];
    for (expected, mutate) in cases {
        let mut process = normal_process(10, "background.exe");
        mutate(&mut process);
        assert_eq!(evaluate_focus_candidate(&process, &config).status, expected);
    }
}

#[test]
fn pinned_and_default_communication_capture_streaming_apps_are_protected() {
    let protected = [
        ("Discord.exe", FocusProcessStatus::Communication),
        ("obs64.exe", FocusProcessStatus::Recording),
        ("Streamlabs Desktop.exe", FocusProcessStatus::Streaming),
        ("XSplit.Core.exe", FocusProcessStatus::Streaming),
        ("XboxGameBar.exe", FocusProcessStatus::CaptureOverlay),
        ("NVIDIA Share.exe", FocusProcessStatus::CaptureOverlay),
        ("RadeonSoftware.exe", FocusProcessStatus::CaptureOverlay),
    ];
    for (name, expected) in protected {
        assert_eq!(
            evaluate_focus_candidate(&normal_process(10, name), &FocusConfig::default()).status,
            expected
        );
    }

    let pinned_path = PathBuf::from("C:/Apps/custom-recorder.exe");
    let config = FocusConfig {
        pinned_executables: BTreeSet::from([pinned_path.clone()]),
        ..FocusConfig::default()
    };
    assert_eq!(
        evaluate_focus_candidate(
            &normal_process(11, pinned_path.to_string_lossy().as_ref()),
            &config,
        )
        .status,
        FocusProcessStatus::Pinned
    );

    assert_eq!(
        evaluate_focus_candidate(&normal_process(12, "Discord.exe"), &FocusConfig::default())
            .status,
        FocusProcessStatus::Communication
    );
}

#[test]
fn recommended_policy_changes_only_normal_priority() {
    for priority in PriorityClass::ALL {
        let mut process = normal_process(10, "worker.exe");
        process.priority = priority;
        let expected = if priority == PriorityClass::Normal {
            FocusProcessStatus::Eligible
        } else {
            FocusProcessStatus::PriorityNotNormal
        };
        assert_eq!(
            evaluate_focus_candidate(&process, &FocusConfig::default()).status,
            expected
        );
    }
}

#[test]
fn activation_requires_fresh_preview_and_explicit_select_all_confirmation() {
    let mut controller = controller([normal_process(10, "one.exe")], true);
    assert_eq!(
        controller.activate(&FocusActivationRequest {
            preview_token: 1,
            selected: Vec::new(),
            select_all_eligible: true,
            select_all_confirmed: true,
        }),
        Err(FocusError::PreviewRequired)
    );
    let first = controller.preview().unwrap();
    let second = controller.preview().unwrap();
    assert_eq!(
        controller.activate(&FocusActivationRequest {
            preview_token: first.token,
            selected: Vec::new(),
            select_all_eligible: true,
            select_all_confirmed: true,
        }),
        Err(FocusError::StalePreview)
    );
    assert_eq!(
        controller.activate(&FocusActivationRequest {
            preview_token: second.token,
            selected: Vec::new(),
            select_all_eligible: true,
            select_all_confirmed: false,
        }),
        Err(FocusError::ExplicitConfirmationRequired)
    );
}

#[test]
fn activation_only_selects_then_adaptive_restraint_journals_before_mutation() {
    let first = normal_process(10, "one.exe");
    let excluded = normal_process(11, "Discord.exe");
    let mut controller = controller([first.clone(), excluded], true);
    let preview = controller.preview().unwrap();

    let report = controller
        .activate(&FocusActivationRequest {
            preview_token: preview.token,
            selected: vec![first.identity.clone()],
            select_all_eligible: false,
            select_all_confirmed: false,
        })
        .unwrap();

    assert_eq!(report.results[0].outcome, FocusRestoreOutcome::Selected);
    assert!(!report.recovery_required);
    assert_eq!(
        controller.backend().processes.get(&10).unwrap().priority,
        PriorityClass::Normal
    );
    assert!(controller.backend().events.borrow().is_empty());

    let observe = adaptive_decision(FocusAdaptiveAction::Observe);
    assert!(controller
        .apply_priority_restraint(&observe)
        .unwrap()
        .results
        .is_empty());
    assert!(controller.backend().events.borrow().is_empty());

    let report = controller
        .apply_priority_restraint(&adaptive_decision(FocusAdaptiveAction::RestrainPriority))
        .unwrap();
    assert_eq!(report.results[0].outcome, FocusRestoreOutcome::Applied);
    assert_eq!(
        controller.backend().processes.get(&10).unwrap().priority,
        PriorityClass::BelowNormal
    );
    assert_eq!(
        controller.backend().events.borrow().as_slice(),
        ["save", "set:10", "save"]
    );
    let journal = controller.journal_store().journal.as_ref().unwrap();
    assert_eq!(journal.schema_version, FOCUS_JOURNAL_SCHEMA_VERSION);
    assert_eq!(journal.entries[0].prior_priority, PriorityClass::Normal);
    assert_eq!(
        journal.entries[0].applied_priority,
        PriorityClass::BelowNormal
    );
}

#[test]
fn failed_journal_write_prevents_any_mutation() {
    let process = normal_process(10, "one.exe");
    let mut controller = controller([process.clone()], true);
    let preview = controller.preview().unwrap();

    controller
        .activate(&FocusActivationRequest {
            preview_token: preview.token,
            selected: vec![process.identity],
            select_all_eligible: false,
            select_all_confirmed: false,
        })
        .unwrap();
    controller.journal_store_mut().fail_save = true;
    assert_eq!(
        controller
            .apply_priority_restraint(&adaptive_decision(FocusAdaptiveAction::RestrainPriority,)),
        Err(FocusError::JournalFailure)
    );
    assert!(!controller
        .backend()
        .events
        .borrow()
        .iter()
        .any(|event| event.starts_with("set:")));
}

#[test]
fn activation_rejects_unpreviewed_or_excluded_selection_and_existing_recovery() {
    let eligible = normal_process(10, "one.exe");
    let excluded = normal_process(11, "obs64.exe");
    let mut controller = controller([eligible, excluded.clone()], true);
    let preview = controller.preview().unwrap();
    assert_eq!(
        controller.activate(&FocusActivationRequest {
            preview_token: preview.token,
            selected: vec![excluded.identity],
            select_all_eligible: false,
            select_all_confirmed: false,
        }),
        Err(FocusError::InvalidSelection)
    );

    controller.journal_store_mut().journal = Some(FocusJournal {
        schema_version: FOCUS_JOURNAL_SCHEMA_VERSION,
        entries: vec![],
    });
    assert_eq!(
        controller.activate(&FocusActivationRequest {
            preview_token: preview.token,
            selected: Vec::new(),
            select_all_eligible: true,
            select_all_confirmed: true,
        }),
        Err(FocusError::RecoveryRequired)
    );
}

#[test]
fn restore_guards_pid_creation_image_and_current_applied_priority() {
    let restored = normal_process(10, "restored.exe");
    let mut reused = normal_process(12, "reused.exe");
    reused.identity.creation_time_100ns += 1;
    let changed_path = normal_process(13, "replacement.exe");
    let externally_changed = normal_process(14, "external.exe");
    let denied = normal_process(15, "denied.exe");
    let mut controller = controller(
        [
            restored.clone(),
            reused,
            changed_path,
            externally_changed,
            denied.clone(),
        ],
        true,
    );
    for process in controller.backend_mut().processes.values_mut() {
        if process.identity.pid != 14 {
            process.priority = PriorityClass::BelowNormal;
        }
    }
    controller.backend_mut().set_denied.insert(15);
    controller.journal_store_mut().journal = Some(FocusJournal {
        schema_version: FOCUS_JOURNAL_SCHEMA_VERSION,
        entries: vec![
            journal_entry(restored.identity),
            journal_entry(identity(11, "exited.exe")),
            journal_entry(identity(12, "reused.exe")),
            journal_entry(identity(13, "original.exe")),
            journal_entry(identity(14, "external.exe")),
            journal_entry(denied.identity),
        ],
    });

    let report = controller.restore().unwrap();
    let outcomes = report
        .results
        .iter()
        .map(|result| (result.identity.pid, result.outcome))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(outcomes[&10], FocusRestoreOutcome::Restored);
    assert_eq!(outcomes[&11], FocusRestoreOutcome::Exited);
    assert_eq!(outcomes[&12], FocusRestoreOutcome::IdentityChanged);
    assert_eq!(outcomes[&13], FocusRestoreOutcome::IdentityChanged);
    assert_eq!(outcomes[&14], FocusRestoreOutcome::ExternallyChanged);
    assert_eq!(outcomes[&15], FocusRestoreOutcome::Denied);
    assert!(report.recovery_required);
    assert_eq!(
        controller
            .journal_store()
            .journal
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.identity.pid)
            .collect::<Vec<_>>(),
        vec![15]
    );
}

#[test]
fn crash_recovery_journal_restores_on_a_fresh_controller() {
    let mut process = normal_process(10, "worker.exe");
    process.priority = PriorityClass::BelowNormal;
    let mut controller = controller([process.clone()], true);
    controller.journal_store_mut().journal = Some(FocusJournal {
        schema_version: FOCUS_JOURNAL_SCHEMA_VERSION,
        entries: vec![journal_entry(process.identity)],
    });

    let report = controller.restore().unwrap();

    assert_eq!(report.results[0].outcome, FocusRestoreOutcome::Restored);
    assert!(!report.recovery_required);
    assert!(controller.journal_store().journal.is_none());
}

#[test]
fn file_journal_round_trips_and_clears_versioned_state() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = FileFocusJournalStore::new(temp.path());
    let journal = FocusJournal {
        schema_version: FOCUS_JOURNAL_SCHEMA_VERSION,
        entries: vec![journal_entry(identity(10, "C:/Apps/worker.exe"))],
    };

    store.save(&journal).unwrap();
    assert_eq!(store.load().unwrap(), Some(journal));
    store.clear().unwrap();
    assert_eq!(store.load().unwrap(), None);
}

#[test]
fn adaptive_priority_restraint_requires_sustained_aggregate_contention() {
    let selected = identity(10, "worker.exe");
    let topology = hybrid_topology();
    let mut policy = AdaptiveFocusPolicy::new(FocusThresholds::default());
    let sample = telemetry(8500, 6000, 1000, selected.clone());

    assert_eq!(
        policy.evaluate(&sample, &topology).action,
        FocusAdaptiveAction::Observe
    );
    assert_eq!(
        policy.evaluate(&sample, &topology).action,
        FocusAdaptiveAction::Observe
    );
    let decision = policy.evaluate(&sample, &topology);
    assert_eq!(decision.contention, FocusContentionKind::Aggregate);
    assert_eq!(decision.action, FocusAdaptiveAction::RestrainPriority);
    assert_eq!(decision.priority_targets, vec![selected]);
    assert_eq!(decision.game_cpu_selection, CpuSelection::All);
    assert_eq!(decision.thresholds.sample_interval_ms, 1000);
}

#[test]
fn adaptive_priority_targets_only_each_sustained_competitor() {
    let hot = identity(10, "hot.exe");
    let idle = identity(11, "idle.exe");
    let topology = hybrid_topology();
    let mut policy = AdaptiveFocusPolicy::new(FocusThresholds::default());
    let sample = FocusTelemetrySample {
        game_foreground: true,
        protection_triggered: false,
        total_cpu_basis_points: 8_500,
        per_logical_cpu_basis_points: vec![8_500; 4],
        game_hot_thread_basis_points: 6_000,
        selected_process_loads: vec![
            FocusProcessLoad {
                identity: hot.clone(),
                cpu_basis_points: 1_000,
            },
            FocusProcessLoad {
                identity: idle,
                cpu_basis_points: 100,
            },
        ],
    };
    policy.evaluate(&sample, &topology);
    policy.evaluate(&sample, &topology);
    let decision = policy.evaluate(&sample, &topology);

    assert_eq!(decision.action, FocusAdaptiveAction::RestrainPriority);
    assert_eq!(decision.priority_targets, vec![hot]);
}

#[test]
fn changing_sustained_target_restores_old_set_before_restraining_new_set() {
    let first = identity(10, "first.exe");
    let second = identity(11, "second.exe");
    let topology = hybrid_topology();
    let mut policy = AdaptiveFocusPolicy::new(FocusThresholds::default());
    let sample_for = |identity: FocusProcessIdentity| FocusTelemetrySample {
        game_foreground: true,
        protection_triggered: false,
        total_cpu_basis_points: 8_500,
        per_logical_cpu_basis_points: vec![8_500; 4],
        game_hot_thread_basis_points: 2_000,
        selected_process_loads: vec![FocusProcessLoad {
            identity,
            cpu_basis_points: 1_000,
        }],
    };
    for _ in 0..2 {
        policy.evaluate(&sample_for(first.clone()), &topology);
    }
    assert_eq!(
        policy.evaluate(&sample_for(first), &topology).action,
        FocusAdaptiveAction::RestrainPriority
    );

    assert_eq!(
        policy
            .evaluate(&sample_for(second.clone()), &topology)
            .action,
        FocusAdaptiveAction::Restore
    );
    assert_eq!(
        policy
            .evaluate(&sample_for(second.clone()), &topology)
            .action,
        FocusAdaptiveAction::Observe
    );
    assert_eq!(
        policy
            .evaluate(&sample_for(second.clone()), &topology)
            .action,
        FocusAdaptiveAction::Observe
    );
    let decision = policy.evaluate(&sample_for(second.clone()), &topology);
    assert_eq!(decision.action, FocusAdaptiveAction::RestrainPriority);
    assert_eq!(decision.priority_targets, vec![second]);
}

#[test]
fn hot_game_thread_at_modest_total_cpu_uses_soft_headroom_not_blanket_priority() {
    let topology = hybrid_topology();
    let mut policy = AdaptiveFocusPolicy::new(FocusThresholds::default());
    let sample = telemetry(4500, 9500, 900, identity(10, "worker.exe"));
    assert_eq!(
        policy.evaluate(&sample, &topology).action,
        FocusAdaptiveAction::Observe
    );
    assert_eq!(
        policy.evaluate(&sample, &topology).action,
        FocusAdaptiveAction::Observe
    );
    let decision = policy.evaluate(&sample, &topology);

    assert_eq!(decision.contention, FocusContentionKind::GameHotThread);
    assert_eq!(decision.action, FocusAdaptiveAction::RecommendSoftCpuSets);
    assert_eq!(decision.background_cpu_set_ids, vec![3, 4]);
    assert_eq!(decision.game_cpu_selection, CpuSelection::All);
}

#[test]
fn hot_game_thread_without_external_competitor_reports_engine_bound_no_action() {
    let topology = hybrid_topology();
    let mut policy = AdaptiveFocusPolicy::new(FocusThresholds::default());
    let sample = telemetry(4000, 9600, 100, identity(10, "worker.exe"));
    policy.evaluate(&sample, &topology);
    policy.evaluate(&sample, &topology);
    let decision = policy.evaluate(&sample, &topology);

    assert_eq!(decision.contention, FocusContentionKind::EngineBound);
    assert_eq!(decision.action, FocusAdaptiveAction::EngineBoundNoAction);
    assert!(decision.background_cpu_set_ids.is_empty());
}

#[test]
fn unrelated_saturated_core_is_not_labeled_as_a_wuwa_hot_thread() {
    let topology = hybrid_topology();
    let mut policy = AdaptiveFocusPolicy::new(FocusThresholds::default());
    let mut sample = telemetry(4_000, 2_000, 900, identity(10, "worker.exe"));
    sample.per_logical_cpu_basis_points = vec![10_000, 500, 500, 500];
    for _ in 0..3 {
        assert_eq!(
            policy.evaluate(&sample, &topology).action,
            FocusAdaptiveAction::Observe
        );
    }
}

#[test]
fn persistent_hot_thread_does_not_block_priority_release_hysteresis() {
    let topology = hybrid_topology();
    let mut policy = AdaptiveFocusPolicy::new(FocusThresholds::default());
    let selected = identity(10, "worker.exe");
    let aggregate = telemetry(8_500, 2_000, 1_000, selected.clone());
    for _ in 0..3 {
        policy.evaluate(&aggregate, &topology);
    }
    let hot = telemetry(4_000, 9_500, 1_000, selected);
    assert_ne!(
        policy.evaluate(&hot, &topology).action,
        FocusAdaptiveAction::Restore
    );
    assert_ne!(
        policy.evaluate(&hot, &topology).action,
        FocusAdaptiveAction::Restore
    );
    assert_eq!(
        policy.evaluate(&hot, &topology).action,
        FocusAdaptiveAction::Restore
    );
}

#[test]
fn adaptive_policy_uses_release_hysteresis_but_protection_restores_immediately() {
    let topology = hybrid_topology();
    let mut policy = AdaptiveFocusPolicy::new(FocusThresholds::default());
    let selected = identity(10, "worker.exe");
    let high = telemetry(8500, 7000, 1000, selected.clone());
    for _ in 0..3 {
        policy.evaluate(&high, &topology);
    }
    let low = telemetry(4000, 4000, 100, selected.clone());
    assert_eq!(
        policy.evaluate(&low, &topology).action,
        FocusAdaptiveAction::Observe
    );
    assert_eq!(
        policy.evaluate(&low, &topology).action,
        FocusAdaptiveAction::Observe
    );
    assert_eq!(
        policy.evaluate(&low, &topology).action,
        FocusAdaptiveAction::Restore
    );

    let mut protected = telemetry(8500, 7000, 1000, selected);
    protected.protection_triggered = true;
    assert_eq!(
        policy.evaluate(&protected, &topology).action,
        FocusAdaptiveAction::Restore
    );
}

#[test]
fn headroom_cpu_sets_skip_small_topologies_and_never_hard_affine_the_game() {
    assert_eq!(
        background_headroom_cpu_sets(&hybrid_topology()),
        Some(vec![3, 4])
    );
    let uniform = topology_with_sets((0..8).map(|index| cpu_set(index + 1, index as u8, 0)));
    assert_eq!(
        background_headroom_cpu_sets(&uniform),
        Some(vec![1, 2, 3, 4])
    );
    let small = topology_with_sets((0..4).map(|index| cpu_set(index + 1, index as u8, 0)));
    assert_eq!(background_headroom_cpu_sets(&small), None);
}

fn journal_entry(identity: FocusProcessIdentity) -> FocusJournalEntry {
    FocusJournalEntry {
        identity,
        prior_priority: PriorityClass::Normal,
        applied_priority: PriorityClass::BelowNormal,
    }
}

fn telemetry(
    total: u16,
    game_hot: u16,
    competitor: u16,
    selected: FocusProcessIdentity,
) -> FocusTelemetrySample {
    FocusTelemetrySample {
        game_foreground: true,
        protection_triggered: false,
        total_cpu_basis_points: total,
        per_logical_cpu_basis_points: vec![game_hot, 2500, 1000, 500],
        game_hot_thread_basis_points: game_hot,
        selected_process_loads: vec![FocusProcessLoad {
            identity: selected,
            cpu_basis_points: competitor,
        }],
    }
}

fn hybrid_topology() -> CpuTopology {
    topology_with_sets([
        cpu_set(1, 0, 8),
        cpu_set(2, 1, 8),
        cpu_set(3, 2, 2),
        cpu_set(4, 3, 2),
    ])
}

fn topology_with_sets(sets: impl IntoIterator<Item = CpuSetInfo>) -> CpuTopology {
    let cpu_sets = sets.into_iter().collect::<Vec<_>>();
    let mask = (1_u64 << cpu_sets.len()) - 1;
    CpuTopology {
        cpu_sets,
        groups: vec![ProcessorGroup {
            group: 0,
            active_mask: mask,
        }],
    }
}

fn cpu_set(id: u32, core: u8, efficiency: u8) -> CpuSetInfo {
    CpuSetInfo {
        id,
        group: 0,
        logical_processor_index: core,
        core_index: core,
        last_level_cache_index: 0,
        numa_node_index: 0,
        efficiency_class: efficiency,
        parked: false,
        allocated: false,
        allocated_to_target: false,
        realtime: false,
    }
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Windows process, window-family, WASAPI, and PDH APIs"]
fn windows_focus_fixture_selects_lowers_reads_back_and_restores_only_disposable_worker() {
    use std::{fs, process::Command, thread, time::Duration};
    use wuwa_ini_tool_lib::process_control::SystemFocusBackend;

    let temp = tempfile::tempdir().unwrap();
    let game_executable = temp
        .path()
        .join("game/Client/Binaries/Win64/Client-Win64-Shipping.exe");
    let worker_executable = temp.path().join("worker.exe");
    fs::create_dir_all(game_executable.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_process_fixture"), &game_executable).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_process_fixture"), &worker_executable).unwrap();
    let installation =
        wuwa_ini_tool_lib::game_discovery::validate_game_executable(&game_executable).unwrap();
    let mut game = Command::new(&game_executable).spawn().unwrap();
    let mut worker = Command::new(&worker_executable).spawn().unwrap();
    thread::sleep(Duration::from_millis(150));

    let result = (|| {
        let backend = SystemFocusBackend::new(&installation)?;
        let store = FileFocusJournalStore::new(temp.path().join("app-data"));
        let mut controller = FocusModeController::new(
            backend,
            store,
            FocusConfig {
                enabled: true,
                ..FocusConfig::default()
            },
        );
        let preview = controller.preview()?;
        let worker_image = worker_executable.canonicalize().unwrap();
        let candidate = preview
            .candidates
            .iter()
            .find(|candidate| candidate.identity.canonical_image == worker_image)
            .cloned()
            .expect("fixture worker must be visible in Focus preview");
        assert!(candidate.is_eligible());
        controller.activate(&FocusActivationRequest {
            preview_token: preview.token,
            selected: vec![candidate.identity.clone()],
            select_all_eligible: false,
            select_all_confirmed: false,
        })?;
        let report = controller.apply_priority_restraint(&FocusAdaptiveDecision {
            contention: FocusContentionKind::Aggregate,
            action: FocusAdaptiveAction::RestrainPriority,
            priority_targets: vec![candidate.identity.clone()],
            background_cpu_set_ids: Vec::new(),
            game_cpu_selection: CpuSelection::All,
            thresholds: FocusThresholds::default(),
        })?;
        assert_eq!(report.results[0].outcome, FocusRestoreOutcome::Applied);
        assert_eq!(
            controller
                .backend_mut()
                .inspect(worker.id())?
                .unwrap()
                .priority,
            PriorityClass::BelowNormal
        );
        let restored = controller.restore()?;
        assert_eq!(restored.results[0].outcome, FocusRestoreOutcome::Restored);
        Ok::<(), FocusError>(())
    })();

    let _ = worker.kill();
    let _ = worker.wait();
    let _ = game.kill();
    let _ = game.wait();
    result.unwrap();
}
