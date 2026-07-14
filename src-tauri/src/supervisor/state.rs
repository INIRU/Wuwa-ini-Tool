use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorState {
    #[default]
    Idle,
    Launching,
    WaitingForGame,
    Applying,
    Active,
    Partial,
    Denied,
    Exited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseAction {
    HideToTray,
    Exit,
}
