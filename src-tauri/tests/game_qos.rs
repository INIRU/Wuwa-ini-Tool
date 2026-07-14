use wuwa_ini_tool_lib::process_control::{
    classify_game_qos_restore, classify_game_qos_restore_error, FileGameQosJournalStore,
    GameQosJournalStore, GameQosRequest, GameQosRestoreGuard, GameQosRestoreOutcome,
    GameQosRestoreRecord, GameQosState, ProcessController, ProcessError,
};

#[derive(Default)]
struct MemoryQosJournal(Option<GameQosRestoreRecord>);

impl GameQosJournalStore for MemoryQosJournal {
    fn load(&self) -> Result<Option<GameQosRestoreRecord>, ProcessError> {
        Ok(self.0.clone())
    }

    fn save(&mut self, record: &GameQosRestoreRecord) -> Result<(), ProcessError> {
        self.0 = Some(record.clone());
        Ok(())
    }

    fn clear(&mut self) -> Result<(), ProcessError> {
        self.0 = None;
        Ok(())
    }
}

#[test]
fn execution_speed_qos_is_explicit_opt_in() {
    assert!(!GameQosRequest::default().disable_execution_speed_throttling);
}

#[test]
fn terminal_qos_restore_errors_are_reported_without_targeting_a_reused_process() {
    assert_eq!(
        classify_game_qos_restore_error(ProcessError::ProcessExited),
        Some(GameQosRestoreOutcome::Exited)
    );
    assert_eq!(
        classify_game_qos_restore_error(ProcessError::InvalidExecutableIdentity),
        Some(GameQosRestoreOutcome::IdentityChanged)
    );
    assert_eq!(
        classify_game_qos_restore_error(ProcessError::AccessDenied),
        None
    );
}

#[test]
fn durable_qos_restore_records_cannot_target_a_different_image_or_state() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp
        .path()
        .join("game/Client/Binaries/Win64/Client-Win64-Shipping.exe");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, b"fixture").unwrap();
    let installation =
        wuwa_ini_tool_lib::game_discovery::validate_game_executable(&executable).unwrap();
    let record = GameQosRestoreRecord {
        pid: 42,
        creation_time_100ns: 100,
        canonical_image: installation.executable.with_file_name("other.exe"),
        prior: GameQosState {
            execution_speed_throttled: true,
        },
        applied: GameQosState {
            execution_speed_throttled: false,
        },
    };

    assert_eq!(
        ProcessController::restore_game_qos(
            &installation,
            &record,
            &mut MemoryQosJournal::default()
        ),
        Err(ProcessError::InvalidExecutableIdentity)
    );
}

#[test]
fn pending_qos_recovery_keeps_a_tampered_identity_journal_for_explicit_repair() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp
        .path()
        .join("game/Client/Binaries/Win64/Client-Win64-Shipping.exe");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, b"fixture").unwrap();
    let installation =
        wuwa_ini_tool_lib::game_discovery::validate_game_executable(&executable).unwrap();
    let record = GameQosRestoreRecord {
        pid: 42,
        creation_time_100ns: 100,
        canonical_image: installation.executable.with_file_name("other.exe"),
        prior: GameQosState {
            execution_speed_throttled: true,
        },
        applied: GameQosState {
            execution_speed_throttled: false,
        },
    };
    let mut journal = MemoryQosJournal(Some(record.clone()));

    assert_eq!(
        ProcessController::restore_pending_game_qos(&installation, &mut journal),
        Err(ProcessError::InvalidExecutableIdentity)
    );
    assert_eq!(journal.0, Some(record));
}

#[test]
fn qos_journal_round_trips_versioned_recovery_state() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = FileGameQosJournalStore::new(temp.path());
    let record = GameQosRestoreRecord {
        pid: 42,
        creation_time_100ns: 100,
        canonical_image: temp.path().join("Client-Win64-Shipping.exe"),
        prior: GameQosState {
            execution_speed_throttled: true,
        },
        applied: GameQosState {
            execution_speed_throttled: false,
        },
    };

    store.save(&record).unwrap();
    assert_eq!(store.load().unwrap(), Some(record));
    store.clear().unwrap();
    assert_eq!(store.load().unwrap(), None);
}

#[test]
fn qos_restore_requires_same_process_identity_and_unchanged_applied_state() {
    let throttled = GameQosState {
        execution_speed_throttled: true,
    };
    let unthrottled = GameQosState {
        execution_speed_throttled: false,
    };

    assert_eq!(
        classify_game_qos_restore(100, 101, unthrottled, unthrottled),
        GameQosRestoreGuard::IdentityChanged
    );
    assert_eq!(
        classify_game_qos_restore(100, 100, throttled, unthrottled),
        GameQosRestoreGuard::ExternallyChanged
    );
    assert_eq!(
        classify_game_qos_restore(100, 100, unthrottled, unthrottled),
        GameQosRestoreGuard::Restore
    );
}
