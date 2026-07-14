use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceOperation {
    GameLaunch,
    RuntimeConfigure,
    CacheCleanup,
    IniWrite,
    ProfileWrite,
    UpdateInstall,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MaintenanceError {
    #[error("maintenance_busy")]
    Busy,
    #[error("maintenance_state_unavailable")]
    StateUnavailable,
}

#[derive(Clone, Default)]
pub struct MaintenanceGate {
    state: Arc<Mutex<Option<MaintenanceOperation>>>,
}

pub struct MaintenanceGuard {
    state: Arc<Mutex<Option<MaintenanceOperation>>>,
    operation: MaintenanceOperation,
}

impl MaintenanceGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn try_acquire(
        &self,
        operation: MaintenanceOperation,
    ) -> Result<MaintenanceGuard, MaintenanceError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| MaintenanceError::StateUnavailable)?;
        if state.is_some() {
            return Err(MaintenanceError::Busy);
        }
        *state = Some(operation);
        drop(state);
        Ok(MaintenanceGuard {
            state: self.state.clone(),
            operation,
        })
    }

    pub fn active(&self) -> Result<Option<MaintenanceOperation>, MaintenanceError> {
        self.state
            .lock()
            .map(|state| *state)
            .map_err(|_| MaintenanceError::StateUnavailable)
    }
}

impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            if *state == Some(self.operation) {
                *state = None;
            }
        }
    }
}
