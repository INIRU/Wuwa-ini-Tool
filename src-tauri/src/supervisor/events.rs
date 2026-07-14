use serde::{Deserialize, Serialize};

use super::{FocusLifecycleReport, ObservedGame, SupervisorState};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorEventReason {
    LaunchRequested,
    LaunchStarted,
    ProcessDetected,
    ApplyCompleted,
    ApplyFailed,
    ProcessRestarted,
    ProcessExited,
    QuitRequested,
    MaintenanceSuspended,
    FocusChanged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorEvent {
    pub sequence: u64,
    pub state: SupervisorState,
    pub process: Option<ObservedGame>,
    pub reason: SupervisorEventReason,
    pub focus_report: Option<FocusLifecycleReport>,
}
