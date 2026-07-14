use serde::{Deserialize, Serialize};

use super::{ObservedGame, SupervisorError};
use crate::{
    game_discovery::GameInstallation,
    process_control::{
        FileGameQosJournalStore, GameQosApplyReport, GameQosJournalStore, GameQosRequest,
        GameQosRestoreOutcome, GameQosState, ProcessController, ProcessError, ProcessTarget,
    },
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GameQosLifecycleStatus {
    NoChange,
    Applied,
    Restored,
    Exited,
    IdentityChanged,
    ExternallyChanged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameQosLifecycleReport {
    pub status: GameQosLifecycleStatus,
    pub prior: Option<GameQosState>,
    pub applied: Option<GameQosState>,
    pub restore_pending: bool,
}

impl GameQosLifecycleReport {
    pub const fn no_change() -> Self {
        Self {
            status: GameQosLifecycleStatus::NoChange,
            prior: None,
            applied: None,
            restore_pending: false,
        }
    }

    fn from_apply(report: GameQosApplyReport) -> Self {
        let restore_pending = report.restore_record.is_some();
        Self {
            status: if restore_pending {
                GameQosLifecycleStatus::Applied
            } else {
                GameQosLifecycleStatus::NoChange
            },
            prior: Some(report.prior),
            applied: Some(report.applied),
            restore_pending,
        }
    }

    fn from_restore(outcome: Option<GameQosRestoreOutcome>) -> Self {
        let status = match outcome {
            None => GameQosLifecycleStatus::NoChange,
            Some(GameQosRestoreOutcome::Restored) => GameQosLifecycleStatus::Restored,
            Some(GameQosRestoreOutcome::Exited) => GameQosLifecycleStatus::Exited,
            Some(GameQosRestoreOutcome::IdentityChanged) => GameQosLifecycleStatus::IdentityChanged,
            Some(GameQosRestoreOutcome::ExternallyChanged) => {
                GameQosLifecycleStatus::ExternallyChanged
            }
        };
        Self {
            status,
            prior: None,
            applied: None,
            restore_pending: false,
        }
    }
}

pub trait GameQosLifecycle {
    fn recover(&mut self) -> Result<GameQosLifecycleReport, SupervisorError>;
    fn normalize(
        &mut self,
        process: &ObservedGame,
        request: GameQosRequest,
    ) -> Result<GameQosLifecycleReport, SupervisorError>;
    fn restore(
        &mut self,
        process: &ObservedGame,
    ) -> Result<GameQosLifecycleReport, SupervisorError>;
}

pub struct PreparedGameQosLifecycle<S> {
    installation: GameInstallation,
    journal: S,
}

impl<S> PreparedGameQosLifecycle<S> {
    pub fn new(installation: GameInstallation, journal: S) -> Self {
        Self {
            installation,
            journal,
        }
    }
}

impl PreparedGameQosLifecycle<FileGameQosJournalStore> {
    pub fn for_app_data(
        installation: GameInstallation,
        app_data: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self::new(installation, FileGameQosJournalStore::new(app_data))
    }
}

impl<S: GameQosJournalStore> GameQosLifecycle for PreparedGameQosLifecycle<S> {
    fn recover(&mut self) -> Result<GameQosLifecycleReport, SupervisorError> {
        ProcessController::restore_pending_game_qos(&self.installation, &mut self.journal)
            .map(GameQosLifecycleReport::from_restore)
            .map_err(map_process_error)
    }

    fn normalize(
        &mut self,
        process: &ObservedGame,
        request: GameQosRequest,
    ) -> Result<GameQosLifecycleReport, SupervisorError> {
        if process.canonical_image != self.installation.executable {
            return Err(SupervisorError::InvalidGameIdentity);
        }
        let target = ProcessTarget::from_installation_with_creation(
            process.pid,
            process.creation_time_100ns,
            &self.installation,
        )
        .map_err(map_process_error)?;
        ProcessController::apply_game_qos(&target, request, &mut self.journal)
            .map(GameQosLifecycleReport::from_apply)
            .map_err(map_process_error)
    }

    fn restore(
        &mut self,
        process: &ObservedGame,
    ) -> Result<GameQosLifecycleReport, SupervisorError> {
        if process.canonical_image != self.installation.executable {
            return Err(SupervisorError::InvalidGameIdentity);
        }
        ProcessController::restore_pending_game_qos(&self.installation, &mut self.journal)
            .map(GameQosLifecycleReport::from_restore)
            .map_err(map_process_error)
    }
}

fn map_process_error(error: ProcessError) -> SupervisorError {
    match error {
        ProcessError::InvalidExecutableIdentity | ProcessError::InvalidProcessId => {
            SupervisorError::InvalidGameIdentity
        }
        _ => SupervisorError::GameQosFailure,
    }
}

#[derive(Default)]
pub struct NoopGameQos;

impl GameQosLifecycle for NoopGameQos {
    fn recover(&mut self) -> Result<GameQosLifecycleReport, SupervisorError> {
        Ok(GameQosLifecycleReport::no_change())
    }

    fn normalize(
        &mut self,
        _process: &ObservedGame,
        _request: GameQosRequest,
    ) -> Result<GameQosLifecycleReport, SupervisorError> {
        Ok(GameQosLifecycleReport::no_change())
    }

    fn restore(
        &mut self,
        _process: &ObservedGame,
    ) -> Result<GameQosLifecycleReport, SupervisorError> {
        Ok(GameQosLifecycleReport::no_change())
    }
}
