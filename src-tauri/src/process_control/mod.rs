#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod cpu_set_buffer;
mod error;
mod focus_exclusion_store;
mod focus_mode;
mod focus_mode_system;
mod focus_telemetry_system;
mod model;
mod qos_journal;
#[cfg(not(target_os = "windows"))]
mod unsupported;
mod validation;
#[cfg(target_os = "windows")]
mod windows;

pub use crate::profile_store::{CpuSelection, PriorityClass};
pub use error::ProcessError;
pub use focus_exclusion_store::{FileFocusExclusionStore, FocusExclusionStore};
pub use focus_mode::{
    background_headroom_cpu_sets, evaluate_focus_candidate, AdaptiveFocusPolicy,
    FileFocusJournalStore, FocusActivationReport, FocusActivationRequest, FocusAdaptiveAction,
    FocusAdaptiveDecision, FocusBackend, FocusCandidate, FocusConfig, FocusContentionKind,
    FocusError, FocusJournal, FocusJournalEntry, FocusJournalStore, FocusModeController,
    FocusPreview, FocusProcessIdentity, FocusProcessLoad, FocusProcessResult, FocusProcessSnapshot,
    FocusProcessStatus, FocusRestoreOutcome, FocusRestoreReport, FocusRuntimeAvailability,
    FocusTelemetrySample, FocusThresholds, FOCUS_JOURNAL_SCHEMA_VERSION,
};
pub use focus_mode_system::SystemFocusBackend;
pub use focus_telemetry_system::SystemFocusTelemetrySampler;
pub use model::{
    AffinityReadback, ApplyReport, ApplyRequest, ApplyStatus, CpuReadback, CpuSetInfo, CpuSetPlan,
    CpuTopology, FieldReport, GameQosApplyReport, GameQosRequest, GameQosRestoreGuard,
    GameQosRestoreOutcome, GameQosRestoreRecord, GameQosState, ProcessReadback, ProcessTarget,
    ProcessorGroup,
};
pub use qos_journal::{FileGameQosJournalStore, GameQosJournalStore};
pub use validation::{
    classify_apply_status, validate_priority, validate_selection, validate_selection_for_mask_bits,
    verify_cpu_plan,
};

impl PriorityClass {
    pub const ALL: [Self; 6] = [
        Self::Idle,
        Self::BelowNormal,
        Self::Normal,
        Self::AboveNormal,
        Self::High,
        Self::Realtime,
    ];

    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::BelowNormal => "below_normal",
            Self::Normal => "normal",
            Self::AboveNormal => "above_normal",
            Self::High => "high",
            Self::Realtime => "realtime",
        }
    }

    pub fn from_wire(wire: &str) -> Result<Self, ProcessError> {
        match wire {
            "idle" => Ok(Self::Idle),
            "below_normal" => Ok(Self::BelowNormal),
            "normal" => Ok(Self::Normal),
            "above_normal" => Ok(Self::AboveNormal),
            "high" => Ok(Self::High),
            "realtime" => Ok(Self::Realtime),
            _ => Err(ProcessError::OperationFailed),
        }
    }

    pub const fn win32_value(self) -> u32 {
        match self {
            Self::Idle => 0x0000_0040,
            Self::BelowNormal => 0x0000_4000,
            Self::Normal => 0x0000_0020,
            Self::AboveNormal => 0x0000_8000,
            Self::High => 0x0000_0080,
            Self::Realtime => 0x0000_0100,
        }
    }

    pub fn from_win32(value: u32) -> Result<Self, ProcessError> {
        match value {
            0x0000_0040 => Ok(Self::Idle),
            0x0000_4000 => Ok(Self::BelowNormal),
            0x0000_0020 => Ok(Self::Normal),
            0x0000_8000 => Ok(Self::AboveNormal),
            0x0000_0080 => Ok(Self::High),
            0x0000_0100 => Ok(Self::Realtime),
            _ => Err(ProcessError::OperationFailed),
        }
    }

    pub const fn requires_dangerous_ack(self) -> bool {
        matches!(self, Self::High | Self::Realtime)
    }
}

pub const fn classify_game_qos_restore(
    expected_creation_time_100ns: u64,
    actual_creation_time_100ns: u64,
    current: GameQosState,
    applied: GameQosState,
) -> GameQosRestoreGuard {
    if expected_creation_time_100ns != actual_creation_time_100ns {
        GameQosRestoreGuard::IdentityChanged
    } else if current.execution_speed_throttled != applied.execution_speed_throttled {
        GameQosRestoreGuard::ExternallyChanged
    } else {
        GameQosRestoreGuard::Restore
    }
}

pub const fn classify_game_qos_restore_error(error: ProcessError) -> Option<GameQosRestoreOutcome> {
    match error {
        ProcessError::ProcessExited => Some(GameQosRestoreOutcome::Exited),
        ProcessError::InvalidExecutableIdentity => Some(GameQosRestoreOutcome::IdentityChanged),
        _ => None,
    }
}

pub struct ProcessController;

impl ProcessController {
    pub fn topology() -> Result<CpuTopology, ProcessError> {
        backend::topology()
    }

    pub fn apply(
        target: &ProcessTarget,
        request: &ApplyRequest,
    ) -> Result<ApplyReport, ProcessError> {
        validation::validate_priority(request.priority, request.dangerous_priority_acknowledged)?;
        backend::apply(target, request)
    }

    pub fn readback(target: &ProcessTarget) -> Result<ProcessReadback, ProcessError> {
        backend::readback(target)
    }

    pub fn apply_game_qos<S: GameQosJournalStore>(
        target: &ProcessTarget,
        request: GameQosRequest,
        journal: &mut S,
    ) -> Result<GameQosApplyReport, ProcessError> {
        if journal.load()?.is_some() {
            return Err(ProcessError::RecoveryRequired);
        }
        backend::apply_game_qos(target, request, &mut |record| journal.save(record))
    }

    pub fn restore_game_qos<S: GameQosJournalStore>(
        installation: &crate::game_discovery::GameInstallation,
        record: &GameQosRestoreRecord,
        journal: &mut S,
    ) -> Result<GameQosRestoreOutcome, ProcessError> {
        if record.creation_time_100ns == 0
            || record.canonical_image != installation.executable
            || !record.prior.execution_speed_throttled
            || record.applied.execution_speed_throttled
        {
            return Err(ProcessError::InvalidExecutableIdentity);
        }
        let target = ProcessTarget::from_installation_with_creation(
            record.pid,
            record.creation_time_100ns,
            installation,
        )?;
        let outcome = backend::restore_game_qos(&target, record)?;
        journal.clear()?;
        Ok(outcome)
    }

    pub fn restore_pending_game_qos<S: GameQosJournalStore>(
        installation: &crate::game_discovery::GameInstallation,
        journal: &mut S,
    ) -> Result<Option<GameQosRestoreOutcome>, ProcessError> {
        let Some(record) = journal.load()? else {
            return Ok(None);
        };
        if record.creation_time_100ns == 0
            || record.canonical_image != installation.executable
            || !record.prior.execution_speed_throttled
            || record.applied.execution_speed_throttled
        {
            return Err(ProcessError::InvalidExecutableIdentity);
        }
        match Self::restore_game_qos(installation, &record, journal) {
            Ok(outcome) => Ok(Some(outcome)),
            Err(error) => {
                if let Some(outcome) = classify_game_qos_restore_error(error) {
                    journal.clear()?;
                    Ok(Some(outcome))
                } else {
                    Err(error)
                }
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
use unsupported as backend;
#[cfg(target_os = "windows")]
use windows as backend;
