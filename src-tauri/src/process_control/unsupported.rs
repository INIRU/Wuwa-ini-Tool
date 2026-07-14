use super::{
    ApplyReport, ApplyRequest, CpuTopology, GameQosApplyReport, GameQosRequest,
    GameQosRestoreOutcome, GameQosRestoreRecord, ProcessError, ProcessReadback, ProcessTarget,
};

pub(crate) fn topology() -> Result<CpuTopology, ProcessError> {
    Err(ProcessError::UnsupportedPlatform)
}

pub(crate) fn apply(
    _target: &ProcessTarget,
    _request: &ApplyRequest,
) -> Result<ApplyReport, ProcessError> {
    Err(ProcessError::UnsupportedPlatform)
}

pub(crate) fn readback(_target: &ProcessTarget) -> Result<ProcessReadback, ProcessError> {
    Err(ProcessError::UnsupportedPlatform)
}

pub(crate) fn apply_game_qos(
    _target: &ProcessTarget,
    _request: GameQosRequest,
    _before_mutation: &mut dyn FnMut(&GameQosRestoreRecord) -> Result<(), ProcessError>,
) -> Result<GameQosApplyReport, ProcessError> {
    Err(ProcessError::UnsupportedPlatform)
}

pub(crate) fn restore_game_qos(
    _target: &ProcessTarget,
    _record: &GameQosRestoreRecord,
) -> Result<GameQosRestoreOutcome, ProcessError> {
    Err(ProcessError::UnsupportedPlatform)
}
