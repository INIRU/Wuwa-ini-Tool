mod events;
mod focus;
mod state;
mod system;

use std::{collections::VecDeque, path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    game_discovery::GameInstallation,
    maintenance::{MaintenanceError, MaintenanceGate},
    profile_store::ProcessProfile,
};

pub use events::{SupervisorEvent, SupervisorEventReason};
pub use focus::{
    FocusDecisionSummary, FocusLifecycle, FocusLifecycleReport, FocusLifecycleStatus,
    FocusTelemetrySummary, NoopFocus, PreparedFocusLifecycle,
};
pub use state::{CloseAction, SupervisorState};
pub use system::SystemSupervisorBackend;

const MIN_POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(5);
const EXIT_CONFIRMATION_POLLS: u8 = 2;
const MAX_PENDING_EVENTS: usize = 128;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedGame {
    pub pid: u32,
    pub creation_time_100ns: u64,
    pub canonical_image: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupervisorApplyOutcome {
    Success,
    Partial,
    Denied,
    /// No mutation was committed and retrying the same epoch is explicitly safe.
    Retryable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupervisorRestoreOutcome {
    Restored,
    ProcessGone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SupervisorError {
    #[error("supervisor_backend_failure")]
    BackendFailure,
    #[error("invalid_game_identity")]
    InvalidGameIdentity,
    #[error("supervisor_quitting")]
    Quitting,
    #[error("maintenance_busy")]
    MaintenanceBusy,
    #[error("supervisor_state_unavailable")]
    StateUnavailable,
    #[error("focus_lifecycle_failure")]
    FocusFailure,
    #[error("validated_game_process_not_active")]
    NotActive,
}

impl From<MaintenanceError> for SupervisorError {
    fn from(error: MaintenanceError) -> Self {
        match error {
            MaintenanceError::Busy => Self::MaintenanceBusy,
            MaintenanceError::StateUnavailable => Self::StateUnavailable,
        }
    }
}

pub trait SupervisorBackend {
    fn launch(&mut self, installation: &GameInstallation) -> Result<(), SupervisorError>;
    fn observe(&mut self) -> Result<Option<ObservedGame>, SupervisorError>;
    fn apply(
        &mut self,
        process: &ObservedGame,
        profile: &ProcessProfile,
        dangerous_priority_acknowledged: bool,
    ) -> Result<SupervisorApplyOutcome, SupervisorError>;
    fn restore(
        &mut self,
        process: &ObservedGame,
    ) -> Result<SupervisorRestoreOutcome, SupervisorError>;
}

pub struct Supervisor<B, F = NoopFocus> {
    backend: B,
    focus: F,
    installation: GameInstallation,
    profile: ProcessProfile,
    dangerous_priority_acknowledged: bool,
    gate: MaintenanceGate,
    state: SupervisorState,
    active_process: Option<ObservedGame>,
    close_to_tray: bool,
    quitting: bool,
    sequence: u64,
    events: VecDeque<SupervisorEvent>,
    poll_delay: Duration,
    missing_polls: u8,
    epoch: u64,
    focus_enabled: bool,
}

impl<B: SupervisorBackend> Supervisor<B, NoopFocus> {
    pub fn new(
        backend: B,
        installation: GameInstallation,
        profile: ProcessProfile,
        gate: MaintenanceGate,
    ) -> Self {
        Self::with_focus(backend, NoopFocus, installation, profile, gate)
    }
}

impl<B: SupervisorBackend, F: FocusLifecycle> Supervisor<B, F> {
    pub fn with_focus(
        backend: B,
        focus: F,
        installation: GameInstallation,
        profile: ProcessProfile,
        gate: MaintenanceGate,
    ) -> Self {
        Self {
            backend,
            focus,
            installation,
            profile,
            dangerous_priority_acknowledged: false,
            gate,
            state: SupervisorState::Idle,
            active_process: None,
            close_to_tray: true,
            quitting: false,
            sequence: 0,
            events: VecDeque::new(),
            poll_delay: MIN_POLL_INTERVAL,
            missing_polls: 0,
            epoch: 0,
            focus_enabled: false,
        }
    }

    pub fn request_launch(&mut self) -> Result<(), SupervisorError> {
        if self.quitting {
            return Err(SupervisorError::Quitting);
        }
        if matches!(self.state, SupervisorState::Idle | SupervisorState::Exited) {
            self.transition(
                SupervisorState::Launching,
                SupervisorEventReason::LaunchRequested,
            );
        }
        Ok(())
    }

    pub fn tick(&mut self) -> Result<(), SupervisorError> {
        if self.quitting {
            return Err(SupervisorError::Quitting);
        }
        match self.state {
            SupervisorState::Idle => self.observe_waiting_game(),
            SupervisorState::Launching => {
                let _guard = self
                    .gate
                    .try_acquire(crate::maintenance::MaintenanceOperation::GameLaunch)?;
                self.backend.launch(&self.installation)?;
                self.reset_polling();
                self.transition(
                    SupervisorState::WaitingForGame,
                    SupervisorEventReason::LaunchStarted,
                );
                Ok(())
            }
            SupervisorState::WaitingForGame | SupervisorState::Exited => {
                self.observe_waiting_game()
            }
            SupervisorState::Applying => {
                let process = self
                    .active_process
                    .as_ref()
                    .ok_or(SupervisorError::InvalidGameIdentity)?
                    .clone();
                let outcome = match self.backend.apply(
                    &process,
                    &self.profile,
                    self.dangerous_priority_acknowledged,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        self.transition(
                            SupervisorState::Denied,
                            SupervisorEventReason::ApplyFailed,
                        );
                        return Err(error);
                    }
                };
                let state = match outcome {
                    SupervisorApplyOutcome::Success => SupervisorState::Active,
                    SupervisorApplyOutcome::Partial => SupervisorState::Partial,
                    SupervisorApplyOutcome::Denied => SupervisorState::Denied,
                    SupervisorApplyOutcome::Retryable => {
                        self.backoff_polling();
                        return Ok(());
                    }
                };
                self.reset_polling();
                self.transition(state, SupervisorEventReason::ApplyCompleted);
                if self.focus_enabled {
                    let report = self.focus.activate(&process, self.epoch)?;
                    let recovery_required = report.recovery_required;
                    self.emit_focus_report(report);
                    if recovery_required {
                        return Err(SupervisorError::FocusFailure);
                    }
                }
                Ok(())
            }
            SupervisorState::Active | SupervisorState::Partial | SupervisorState::Denied => {
                self.monitor_active_process()
            }
        }
    }

    pub fn request_quit(&mut self) -> Result<(), SupervisorError> {
        if self.quitting {
            return Ok(());
        }
        self.restore_active()?;
        self.quitting = true;
        self.transition(
            SupervisorState::Exited,
            SupervisorEventReason::QuitRequested,
        );
        Ok(())
    }

    pub fn suspend_for_maintenance(&mut self) -> Result<(), SupervisorError> {
        if self.quitting {
            return Err(SupervisorError::Quitting);
        }
        self.restore_active()?;
        self.transition(
            SupervisorState::Idle,
            SupervisorEventReason::MaintenanceSuspended,
        );
        Ok(())
    }

    pub fn handle_close_requested(&self) -> CloseAction {
        if self.close_to_tray {
            CloseAction::HideToTray
        } else {
            CloseAction::Exit
        }
    }

    pub fn set_close_to_tray(&mut self, enabled: bool) {
        self.close_to_tray = enabled;
    }

    pub const fn state(&self) -> SupervisorState {
        self.state
    }

    pub fn active_process(&self) -> Option<&ObservedGame> {
        self.active_process.as_ref()
    }

    pub const fn next_poll_delay(&self) -> Duration {
        self.poll_delay
    }

    pub fn events(&self) -> &VecDeque<SupervisorEvent> {
        &self.events
    }

    pub fn drain_events(&mut self) -> Vec<SupervisorEvent> {
        self.events.drain(..).collect()
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn focus(&self) -> &F {
        &self.focus
    }

    pub fn focus_mut(&mut self) -> &mut F {
        &mut self.focus
    }

    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn set_process_profile(
        &mut self,
        profile: ProcessProfile,
        dangerous_priority_acknowledged: bool,
    ) {
        self.profile = profile;
        self.dangerous_priority_acknowledged = dangerous_priority_acknowledged;
    }

    pub fn activate_focus_mode(&mut self) -> Result<FocusLifecycleReport, SupervisorError> {
        if !matches!(
            self.state,
            SupervisorState::Active | SupervisorState::Partial
        ) {
            return Err(SupervisorError::NotActive);
        }
        let process = self
            .active_process
            .as_ref()
            .ok_or(SupervisorError::NotActive)?
            .clone();
        let recovery = self.focus.recover()?;
        if !recovery.process_results.is_empty() || recovery.recovery_required {
            self.emit_focus_report(recovery.clone());
        }
        if recovery.recovery_required {
            return Ok(recovery);
        }
        let report = self.focus.activate(&process, self.epoch)?;
        self.focus_enabled = !report.recovery_required;
        self.emit_focus_report(report.clone());
        Ok(report)
    }

    pub fn deactivate_focus_mode(&mut self) -> Result<FocusLifecycleReport, SupervisorError> {
        let process = self
            .active_process
            .as_ref()
            .ok_or(SupervisorError::NotActive)?
            .clone();
        if !self.focus_enabled {
            return Ok(FocusLifecycleReport::no_changes(self.epoch, Some(process)));
        }
        let report = self.focus.restore(&process, self.epoch)?;
        self.focus_enabled = report.recovery_required;
        self.emit_focus_report(report.clone());
        Ok(report)
    }

    fn monitor_active_process(&mut self) -> Result<(), SupervisorError> {
        let observed = self.backend.observe()?;
        match observed {
            Some(process) if self.is_expected_process(&process) => {
                self.missing_polls = 0;
                self.reset_polling();
                if self.active_process.as_ref() != Some(&process) {
                    self.restore_active()?;
                    self.set_active_process(process);
                    self.transition(
                        SupervisorState::Applying,
                        SupervisorEventReason::ProcessRestarted,
                    );
                } else if self.focus_enabled {
                    let report = self.focus.tick(&process, self.epoch)?;
                    let recovery_required = report.recovery_required;
                    if !report.process_results.is_empty()
                        || report.adaptive_decision.is_some()
                        || recovery_required
                    {
                        self.emit_focus_report(report);
                    }
                    if recovery_required {
                        return Err(SupervisorError::FocusFailure);
                    }
                }
            }
            Some(_) | None => {
                self.missing_polls = self.missing_polls.saturating_add(1);
                self.backoff_polling();
                if self.missing_polls >= EXIT_CONFIRMATION_POLLS {
                    self.restore_active()?;
                    self.missing_polls = 0;
                    self.transition(
                        SupervisorState::Exited,
                        SupervisorEventReason::ProcessExited,
                    );
                }
            }
        }
        Ok(())
    }

    fn is_expected_process(&self, process: &ObservedGame) -> bool {
        process.pid != 0
            && process.creation_time_100ns != 0
            && paths_match(&process.canonical_image, &self.installation.executable)
    }

    fn observe_waiting_game(&mut self) -> Result<(), SupervisorError> {
        match self.backend.observe()? {
            Some(process) if self.is_expected_process(&process) => {
                self.set_active_process(process);
                self.reset_polling();
                self.transition(
                    SupervisorState::Applying,
                    SupervisorEventReason::ProcessDetected,
                );
            }
            Some(_) | None => self.backoff_polling(),
        }
        Ok(())
    }

    fn set_active_process(&mut self, process: ObservedGame) {
        self.epoch = self.epoch.saturating_add(1);
        self.active_process = Some(process);
    }

    fn restore_active(&mut self) -> Result<(), SupervisorError> {
        let Some(process) = self.active_process.as_ref().cloned() else {
            return Ok(());
        };
        if self.focus_enabled {
            let report = self.focus.restore(&process, self.epoch)?;
            let recovery_required = report.recovery_required;
            self.emit_focus_report(report);
            if recovery_required {
                return Err(SupervisorError::FocusFailure);
            }
        }
        let _ = self.backend.restore(&process)?;
        self.active_process = None;
        Ok(())
    }

    fn transition(&mut self, state: SupervisorState, reason: SupervisorEventReason) {
        self.state = state;
        self.sequence = self.sequence.saturating_add(1);
        if self.events.len() == MAX_PENDING_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(SupervisorEvent {
            sequence: self.sequence,
            state,
            process: self.active_process.clone(),
            reason,
            focus_report: None,
        });
    }

    fn emit_focus_report(&mut self, report: FocusLifecycleReport) {
        self.sequence = self.sequence.saturating_add(1);
        if self.events.len() == MAX_PENDING_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(SupervisorEvent {
            sequence: self.sequence,
            state: self.state,
            process: self.active_process.clone(),
            reason: SupervisorEventReason::FocusChanged,
            focus_report: Some(report),
        });
    }

    fn reset_polling(&mut self) {
        self.poll_delay = MIN_POLL_INTERVAL;
    }

    fn backoff_polling(&mut self) {
        self.poll_delay = self
            .poll_delay
            .checked_mul(2)
            .unwrap_or(MAX_POLL_INTERVAL)
            .min(MAX_POLL_INTERVAL);
    }
}

fn paths_match(left: &std::path::Path, right: &std::path::Path) -> bool {
    let left = left.to_string_lossy();
    let right = right.to_string_lossy();
    if cfg!(target_os = "windows") {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}
