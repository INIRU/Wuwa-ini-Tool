use std::fs;

use wuwa_ini_tool_lib::{
    game_discovery::{GameInstallation, InstallationChannel},
    process_control::{
        classify_apply_status, validate_priority, validate_selection,
        validate_selection_for_mask_bits, verify_cpu_plan, AffinityReadback, ApplyRequest,
        ApplyStatus, CpuReadback, CpuSelection, CpuSetInfo, CpuSetPlan, CpuTopology, PriorityClass,
        ProcessController, ProcessError, ProcessTarget, ProcessorGroup,
    },
};

fn cpu_set(id: u32, group: u16, logical: u8, efficiency: u8) -> CpuSetInfo {
    CpuSetInfo {
        id,
        group,
        logical_processor_index: logical,
        core_index: logical / 2,
        last_level_cache_index: 0,
        numa_node_index: 0,
        efficiency_class: efficiency,
        parked: false,
        allocated: false,
        allocated_to_target: false,
        realtime: false,
    }
}

fn topology(sets: Vec<CpuSetInfo>, groups: &[(u16, u64)]) -> CpuTopology {
    CpuTopology {
        cpu_sets: sets,
        groups: groups
            .iter()
            .map(|&(group, active_mask)| ProcessorGroup { group, active_mask })
            .collect(),
    }
}

#[test]
fn every_priority_round_trips_without_changing_the_default() {
    let expected = [
        (PriorityClass::Idle, "idle", 0x0000_0040),
        (PriorityClass::BelowNormal, "below_normal", 0x0000_4000),
        (PriorityClass::Normal, "normal", 0x0000_0020),
        (PriorityClass::AboveNormal, "above_normal", 0x0000_8000),
        (PriorityClass::High, "high", 0x0000_0080),
        (PriorityClass::Realtime, "realtime", 0x0000_0100),
    ];

    assert_eq!(PriorityClass::ALL.len(), expected.len());
    for (priority, wire, win32) in expected {
        assert_eq!(priority.as_wire(), wire);
        assert_eq!(PriorityClass::from_wire(wire), Ok(priority));
        assert_eq!(priority.win32_value(), win32);
        assert_eq!(PriorityClass::from_win32(win32), Ok(priority));
    }
    assert_eq!(PriorityClass::default(), PriorityClass::Normal);
    assert_eq!(ApplyRequest::default().priority, PriorityClass::Normal);
    assert_eq!(ApplyRequest::default().cpu_selection, CpuSelection::All);
}

#[test]
fn invalid_priority_representations_are_rejected() {
    assert_eq!(
        PriorityClass::from_wire("HIGH_PRIORITY_CLASS"),
        Err(ProcessError::OperationFailed)
    );
    assert_eq!(
        PriorityClass::from_win32(0xDEAD_BEEF),
        Err(ProcessError::OperationFailed)
    );
}

#[test]
fn elevated_priorities_require_an_explicit_acknowledgement() {
    for priority in [PriorityClass::High, PriorityClass::Realtime] {
        assert!(priority.requires_dangerous_ack());
        assert_eq!(
            validate_priority(priority, false),
            Err(ProcessError::DangerousPriorityNotAcknowledged)
        );
        assert_eq!(validate_priority(priority, true), Ok(()));
    }
    for priority in [
        PriorityClass::Idle,
        PriorityClass::BelowNormal,
        PriorityClass::Normal,
        PriorityClass::AboveNormal,
    ] {
        assert!(!priority.requires_dangerous_ack());
        assert_eq!(validate_priority(priority, false), Ok(()));
    }
}

#[test]
fn performance_mode_chooses_only_the_relative_highest_efficiency_class() {
    let topology = topology(
        vec![
            cpu_set(10, 0, 0, 2),
            cpu_set(11, 0, 1, 8),
            cpu_set(12, 0, 2, 8),
            cpu_set(13, 0, 3, 4),
        ],
        &[(0, 0b1111)],
    );

    assert_eq!(
        validate_selection(&topology, &CpuSelection::PreferPerformance),
        Ok(CpuSetPlan::CpuSets(vec![11, 12]))
    );
}

#[test]
fn equal_efficiency_classes_reset_to_all_instead_of_fake_p_core_selection() {
    let topology = topology(
        vec![cpu_set(10, 0, 0, 0), cpu_set(11, 0, 1, 0)],
        &[(0, 0b11)],
    );

    assert_eq!(
        validate_selection(&topology, &CpuSelection::PreferPerformance),
        Ok(CpuSetPlan::ResetAll)
    );
}

#[test]
fn empty_cpu_sets_are_unsupported_for_cpu_set_modes() {
    let topology = topology(Vec::new(), &[(0, 0b11)]);

    assert_eq!(
        validate_selection(&topology, &CpuSelection::PreferPerformance),
        Err(ProcessError::EmptyCpuSets)
    );
    assert_eq!(
        validate_selection(&topology, &CpuSelection::ManualCpuSets { ids: vec![1] }),
        Err(ProcessError::EmptyCpuSets)
    );
    assert_eq!(
        validate_selection(&topology, &CpuSelection::All),
        Ok(CpuSetPlan::ResetAll)
    );
}

#[test]
fn manual_cpu_sets_must_be_nonempty_unique_available_and_in_topology() {
    let mut allocated_elsewhere = cpu_set(12, 0, 2, 4);
    allocated_elsewhere.allocated = true;
    let mut allocated_to_target = cpu_set(13, 0, 3, 4);
    allocated_to_target.allocated = true;
    allocated_to_target.allocated_to_target = true;
    let topology = topology(
        vec![
            cpu_set(10, 0, 0, 4),
            cpu_set(11, 0, 1, 4),
            allocated_elsewhere,
            allocated_to_target,
        ],
        &[(0, 0b1111)],
    );

    assert_eq!(
        validate_selection(&topology, &CpuSelection::ManualCpuSets { ids: Vec::new() }),
        Err(ProcessError::EmptyCpuSets)
    );
    assert_eq!(
        validate_selection(
            &topology,
            &CpuSelection::ManualCpuSets { ids: vec![10, 10] }
        ),
        Err(ProcessError::DuplicateCpuSet)
    );
    assert_eq!(
        validate_selection(&topology, &CpuSelection::ManualCpuSets { ids: vec![99] }),
        Err(ProcessError::CpuSetNotFound)
    );
    assert_eq!(
        validate_selection(&topology, &CpuSelection::ManualCpuSets { ids: vec![12] }),
        Err(ProcessError::CpuSetUnavailable)
    );
    assert_eq!(
        validate_selection(
            &topology,
            &CpuSelection::ManualCpuSets { ids: vec![13, 10] }
        ),
        Ok(CpuSetPlan::CpuSets(vec![13, 10]))
    );
}

#[test]
fn hard_affinity_requires_nonzero_subset_of_one_validated_group() {
    let topology = topology(
        vec![cpu_set(10, 0, 0, 0), cpu_set(11, 0, 1, 0)],
        &[(0, 0b1111)],
    );

    assert_eq!(
        validate_selection(&topology, &CpuSelection::HardAffinity { group: 0, mask: 0 }),
        Err(ProcessError::InvalidAffinityMask)
    );
    assert_eq!(
        validate_selection(
            &topology,
            &CpuSelection::HardAffinity {
                group: 0,
                mask: 0b1_0000,
            }
        ),
        Err(ProcessError::InvalidAffinityMask)
    );
    assert_eq!(
        validate_selection(
            &topology,
            &CpuSelection::HardAffinity {
                group: 0,
                mask: 0b0101,
            }
        ),
        Ok(CpuSetPlan::HardAffinity {
            group: 0,
            mask: 0b0101,
        })
    );
}

#[test]
fn hard_affinity_rejects_multiple_groups_wrong_group_and_mask_overflow() {
    let multiple = topology(
        vec![cpu_set(1, 0, 0, 0), cpu_set(2, 1, 0, 0)],
        &[(0, 0b1), (1, 0b1)],
    );
    assert_eq!(
        validate_selection(&multiple, &CpuSelection::HardAffinity { group: 0, mask: 1 }),
        Err(ProcessError::MultipleProcessorGroups)
    );

    let single = topology(vec![cpu_set(1, 3, 0, 0)], &[(3, u64::MAX)]);
    assert_eq!(
        validate_selection(&single, &CpuSelection::HardAffinity { group: 2, mask: 1 }),
        Err(ProcessError::UnsupportedTopology)
    );
    assert_eq!(
        validate_selection_for_mask_bits(
            &single,
            &CpuSelection::HardAffinity {
                group: 3,
                mask: 1_u64 << 40,
            },
            32,
        ),
        Err(ProcessError::InvalidAffinityMask)
    );
}

#[test]
fn stable_error_codes_classify_denied_exited_unsupported_and_generic_failure() {
    assert_eq!(ProcessError::from_win32(5), ProcessError::AccessDenied);
    assert_eq!(
        ProcessError::from_open_process_win32(87),
        ProcessError::ProcessExited
    );
    assert_eq!(
        ProcessError::from_open_process_win32(6),
        ProcessError::ProcessExited
    );
    assert_eq!(
        ProcessError::from_win32(1234),
        ProcessError::OperationFailed
    );
    assert_eq!(ProcessError::AccessDenied.code(), "access_denied");
    assert_eq!(ProcessError::ProcessExited.code(), "process_exited");
    assert_eq!(
        ProcessError::UnsupportedPlatform.code(),
        "unsupported_platform"
    );
    assert!(!ProcessError::OperationFailed.to_string().contains('\\'));
}

#[test]
fn apply_status_preserves_success_and_stable_failure_categories() {
    assert_eq!(classify_apply_status(None, None), ApplyStatus::Success);
    assert_eq!(
        classify_apply_status(
            Some(ProcessError::AccessDenied),
            Some(ProcessError::AccessDenied),
        ),
        ApplyStatus::Denied
    );
    assert_eq!(
        classify_apply_status(
            Some(ProcessError::UnsupportedTopology),
            Some(ProcessError::UnsupportedPlatform),
        ),
        ApplyStatus::Unsupported
    );
    assert_eq!(
        classify_apply_status(
            Some(ProcessError::ProcessExited),
            Some(ProcessError::ProcessExited),
        ),
        ApplyStatus::Exited
    );
    assert_eq!(
        classify_apply_status(None, Some(ProcessError::AccessDenied)),
        ApplyStatus::Partial
    );
    assert_eq!(
        classify_apply_status(
            Some(ProcessError::AccessDenied),
            Some(ProcessError::OperationFailed),
        ),
        ApplyStatus::Partial
    );
}

#[test]
fn cpu_readback_must_prove_cpu_sets_and_hard_affinity_were_applied() {
    let restricted = CpuReadback {
        default_cpu_sets: Vec::new(),
        affinity: Some(AffinityReadback {
            process_mask: 0b0011,
            system_mask: 0b1111,
            groups: vec![0],
        }),
    };
    assert_eq!(
        verify_cpu_plan(&CpuSetPlan::ResetAll, &restricted),
        Err(ProcessError::OperationFailed)
    );

    let cpu_sets_with_stale_affinity = CpuReadback {
        default_cpu_sets: vec![7, 9],
        affinity: restricted.affinity.clone(),
    };
    assert_eq!(
        verify_cpu_plan(
            &CpuSetPlan::CpuSets(vec![9, 7]),
            &cpu_sets_with_stale_affinity,
        ),
        Err(ProcessError::OperationFailed)
    );

    let reset = CpuReadback {
        default_cpu_sets: Vec::new(),
        affinity: Some(AffinityReadback {
            process_mask: 0b1111,
            system_mask: 0b1111,
            groups: vec![0],
        }),
    };
    assert_eq!(verify_cpu_plan(&CpuSetPlan::ResetAll, &reset), Ok(()));

    let hard = CpuReadback {
        default_cpu_sets: Vec::new(),
        affinity: Some(AffinityReadback {
            process_mask: 0b0101,
            system_mask: 0b1111,
            groups: vec![0],
        }),
    };
    assert_eq!(
        verify_cpu_plan(
            &CpuSetPlan::HardAffinity {
                group: 0,
                mask: 0b0101,
            },
            &hard,
        ),
        Ok(())
    );
}

#[test]
fn process_target_is_created_only_from_a_validated_installation_and_nonzero_pid() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp
        .path()
        .join("validated/Client/Binaries/Win64/Client-Win64-Shipping.exe");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, b"fixture").unwrap();
    let installation =
        wuwa_ini_tool_lib::game_discovery::validate_game_executable(&executable).unwrap();

    assert_eq!(
        ProcessTarget::from_installation(0, &installation).unwrap_err(),
        ProcessError::InvalidProcessId
    );
    let target = ProcessTarget::from_installation(42, &installation).unwrap();
    assert_eq!(target.pid(), 42);
    assert_eq!(
        target.expected_executable(),
        executable.canonicalize().unwrap()
    );
    assert_eq!(target.expected_creation_time_100ns(), None);
    assert_eq!(
        ProcessTarget::from_installation_with_creation(42, 0, &installation).unwrap_err(),
        ProcessError::InvalidExecutableIdentity
    );
    let epoch_target =
        ProcessTarget::from_installation_with_creation(42, 123_456, &installation).unwrap();
    assert_eq!(epoch_target.expected_creation_time_100ns(), Some(123_456));

    let forged = GameInstallation {
        channel: InstallationChannel::Manual,
        requires_user_confirmation: false,
        game_root: installation.game_root.clone(),
        executable: installation.executable.clone(),
        engine_ini: installation.engine_ini.with_file_name("UserEngine.ini"),
    };
    assert_eq!(
        ProcessTarget::from_installation(42, &forged).unwrap_err(),
        ProcessError::InvalidExecutableIdentity
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn non_windows_backend_is_deterministically_unsupported() {
    assert_eq!(
        ProcessController::topology(),
        Err(ProcessError::UnsupportedPlatform)
    );
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires a Windows runner and the repository fixture process"]
fn windows_fixture_applies_only_safe_priority_classes() {
    use std::{process::Command, thread, time::Duration};
    use wuwa_ini_tool_lib::process_control::{
        FileGameQosJournalStore, GameQosRequest, GameQosRestoreOutcome,
    };

    let temp = tempfile::tempdir().unwrap();
    let executable = temp
        .path()
        .join("game/Client/Binaries/Win64/Client-Win64-Shipping.exe");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_process_fixture"), &executable).unwrap();
    let installation =
        wuwa_ini_tool_lib::game_discovery::validate_game_executable(&executable).unwrap();
    let mut child = Command::new(&executable).spawn().unwrap();
    thread::sleep(Duration::from_millis(150));

    let result = (|| {
        let target = ProcessTarget::from_installation(child.id(), &installation)?;
        let other_executable = temp
            .path()
            .join("other/Client/Binaries/Win64/Client-Win64-Shipping.exe");
        fs::create_dir_all(other_executable.parent().unwrap()).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_process_fixture"), &other_executable).unwrap();
        let other_installation =
            wuwa_ini_tool_lib::game_discovery::validate_game_executable(&other_executable).unwrap();
        let wrong_target = ProcessTarget::from_installation(child.id(), &other_installation)?;
        assert_eq!(
            ProcessController::readback(&wrong_target),
            Err(ProcessError::InvalidExecutableIdentity)
        );

        for priority in [PriorityClass::Normal, PriorityClass::AboveNormal] {
            let report = ProcessController::apply(
                &target,
                &ApplyRequest {
                    cpu_selection: CpuSelection::All,
                    priority,
                    dangerous_priority_acknowledged: false,
                },
            )?;
            assert_eq!(report.status, ApplyStatus::Success);
            assert_eq!(report.priority.applied, Some(priority));
        }
        let readback = ProcessController::readback(&target)?;
        assert_eq!(readback.priority, PriorityClass::AboveNormal);

        let mut qos_journal = FileGameQosJournalStore::new(temp.path().join("app-data"));
        let inspected = ProcessController::apply_game_qos(
            &target,
            GameQosRequest::default(),
            &mut qos_journal,
        )?;
        assert_eq!(inspected.prior, inspected.applied);
        assert!(inspected.restore_record.is_none());
        let normalized = ProcessController::apply_game_qos(
            &target,
            GameQosRequest {
                disable_execution_speed_throttling: true,
            },
            &mut qos_journal,
        )?;
        assert!(!normalized.applied.execution_speed_throttled);
        if let Some(record) = normalized.restore_record {
            assert_eq!(
                ProcessController::restore_game_qos(&installation, &record, &mut qos_journal)?,
                GameQosRestoreOutcome::Restored
            );
        }
        Ok::<(), ProcessError>(())
    })();

    let _ = child.kill();
    let _ = child.wait();
    result.unwrap();
}
