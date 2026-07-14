use serde::{Deserialize, Serialize};

use super::{ObservedGame, SupervisorError};
use crate::process_control::{
    evaluate_focus_candidate, AdaptiveFocusPolicy, CpuSelection, CpuTopology,
    FocusActivationRequest, FocusAdaptiveAction, FocusAdaptiveDecision, FocusBackend,
    FocusContentionKind, FocusError, FocusJournalStore, FocusModeController, FocusPreview,
    FocusProcessIdentity, FocusProcessResult, FocusRuntimeAvailability, FocusTelemetrySample,
    ProcessController, SystemFocusTelemetrySampler,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusLifecycleStatus {
    NoChanges,
    Recovered,
    Activated,
    Armed,
    Applied,
    Restored,
    RecoveryRequired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusLifecycleReport {
    pub epoch: u64,
    pub process: Option<ObservedGame>,
    pub status: FocusLifecycleStatus,
    pub process_results: Vec<FocusProcessResult>,
    pub recovery_required: bool,
    pub telemetry: Option<FocusTelemetrySummary>,
    pub adaptive_decision: Option<FocusDecisionSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusTelemetrySummary {
    pub game_foreground: bool,
    pub protection_triggered: bool,
    pub total_cpu_basis_points: u16,
    pub game_hot_thread_basis_points: u16,
    pub competitor_count: usize,
    pub max_competitor_basis_points: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusDecisionSummary {
    pub contention: FocusContentionKind,
    pub action: FocusAdaptiveAction,
    pub priority_target_count: usize,
    pub background_cpu_set_ids: Vec<u32>,
    pub game_cpu_selection: CpuSelection,
}

impl FocusLifecycleReport {
    pub fn no_changes(epoch: u64, process: Option<ObservedGame>) -> Self {
        Self {
            epoch,
            process,
            status: FocusLifecycleStatus::NoChanges,
            process_results: Vec::new(),
            recovery_required: false,
            telemetry: None,
            adaptive_decision: None,
        }
    }
}

pub struct PreparedFocusLifecycle<B, S> {
    controller: FocusModeController<B, S>,
    activation: Option<FocusActivationRequest>,
    telemetry: Option<SystemFocusTelemetrySampler>,
    policy: AdaptiveFocusPolicy,
    topology: Option<crate::process_control::CpuTopology>,
}

impl<B: FocusBackend, S: FocusJournalStore> PreparedFocusLifecycle<B, S> {
    pub fn new(controller: FocusModeController<B, S>) -> Self {
        let thresholds = controller.config().thresholds;
        Self {
            controller,
            activation: None,
            telemetry: SystemFocusTelemetrySampler::new(thresholds).ok(),
            policy: AdaptiveFocusPolicy::new(thresholds),
            topology: ProcessController::topology().ok(),
        }
    }

    pub fn preview(&mut self) -> Result<FocusPreview, SupervisorError> {
        let mut preview = self
            .controller
            .preview()
            .map_err(|_| SupervisorError::FocusFailure)?;
        preview.runtime_availability = self.runtime_availability();
        Ok(preview)
    }

    pub fn arm(&mut self, activation: FocusActivationRequest) {
        self.activation = Some(activation);
    }

    pub fn controller(&self) -> &FocusModeController<B, S> {
        &self.controller
    }

    pub fn controller_mut(&mut self) -> &mut FocusModeController<B, S> {
        &mut self.controller
    }

    pub fn apply_sample(
        &mut self,
        process: &ObservedGame,
        epoch: u64,
        sample: &FocusTelemetrySample,
        topology: &CpuTopology,
    ) -> Result<FocusLifecycleReport, SupervisorError> {
        let decision = self.policy.evaluate(sample, topology);
        let (status, process_results, recovery_required) = match decision.action {
            FocusAdaptiveAction::Restore => {
                let report = self
                    .controller
                    .restore()
                    .map_err(|_| SupervisorError::FocusFailure)?;
                (
                    lifecycle_status(report.recovery_required, FocusLifecycleStatus::Restored),
                    report.results,
                    report.recovery_required,
                )
            }
            FocusAdaptiveAction::RestrainPriority => {
                let report = self
                    .controller
                    .apply_priority_restraint(&decision)
                    .map_err(|_| SupervisorError::FocusFailure)?;
                (
                    lifecycle_status(report.recovery_required, FocusLifecycleStatus::Applied),
                    report.results,
                    report.recovery_required,
                )
            }
            _ => (FocusLifecycleStatus::NoChanges, Vec::new(), false),
        };
        Ok(FocusLifecycleReport {
            epoch,
            process: Some(process.clone()),
            status,
            process_results,
            recovery_required,
            telemetry: Some(FocusTelemetrySummary::from_sample(sample)),
            adaptive_decision: Some(FocusDecisionSummary::from_decision(&decision)),
        })
    }

    pub const fn runtime_availability(&self) -> FocusRuntimeAvailability {
        match (self.topology.is_some(), self.telemetry.is_some()) {
            (true, true) => FocusRuntimeAvailability::Available,
            (false, true) => FocusRuntimeAvailability::TopologyUnavailable,
            (true, false) => FocusRuntimeAvailability::TelemetryUnavailable,
            (false, false) => FocusRuntimeAvailability::Unavailable,
        }
    }

    fn reset_adaptive_runtime(&mut self) {
        let thresholds = self.controller.config().thresholds;
        self.policy = AdaptiveFocusPolicy::new(thresholds);
        self.telemetry = SystemFocusTelemetrySampler::new(thresholds).ok();
        self.topology = ProcessController::topology().ok();
    }
}

impl<B: FocusBackend, S: FocusJournalStore> FocusLifecycle for PreparedFocusLifecycle<B, S> {
    fn recover(&mut self) -> Result<FocusLifecycleReport, SupervisorError> {
        let report = self
            .controller
            .restore()
            .map_err(|_| SupervisorError::FocusFailure)?;
        self.reset_adaptive_runtime();
        Ok(FocusLifecycleReport {
            epoch: 0,
            process: None,
            status: if report.recovery_required {
                FocusLifecycleStatus::RecoveryRequired
            } else if report.results.is_empty() {
                FocusLifecycleStatus::NoChanges
            } else {
                FocusLifecycleStatus::Recovered
            },
            process_results: report.results,
            recovery_required: report.recovery_required,
            telemetry: None,
            adaptive_decision: None,
        })
    }

    fn activate(
        &mut self,
        process: &ObservedGame,
        epoch: u64,
    ) -> Result<FocusLifecycleReport, SupervisorError> {
        self.reset_adaptive_runtime();
        let activation = self
            .activation
            .as_ref()
            .ok_or(SupervisorError::FocusFailure)?;
        let report = self
            .controller
            .activate(activation)
            .map_err(|_| SupervisorError::FocusFailure)?;
        Ok(FocusLifecycleReport {
            epoch,
            process: Some(process.clone()),
            status: lifecycle_status(report.recovery_required, FocusLifecycleStatus::Armed),
            process_results: report.results,
            recovery_required: report.recovery_required,
            telemetry: None,
            adaptive_decision: None,
        })
    }

    fn restore(
        &mut self,
        process: &ObservedGame,
        epoch: u64,
    ) -> Result<FocusLifecycleReport, SupervisorError> {
        let report = self
            .controller
            .restore()
            .map_err(|_| SupervisorError::FocusFailure)?;
        self.reset_adaptive_runtime();
        Ok(FocusLifecycleReport {
            epoch,
            process: Some(process.clone()),
            status: if report.recovery_required {
                FocusLifecycleStatus::RecoveryRequired
            } else if report.results.is_empty() {
                FocusLifecycleStatus::NoChanges
            } else {
                FocusLifecycleStatus::Restored
            },
            process_results: report.results,
            recovery_required: report.recovery_required,
            telemetry: None,
            adaptive_decision: None,
        })
    }

    fn tick(
        &mut self,
        process: &ObservedGame,
        epoch: u64,
    ) -> Result<FocusLifecycleReport, SupervisorError> {
        let Some(topology) = self.topology.clone() else {
            return Ok(FocusLifecycleReport::no_changes(
                epoch,
                Some(process.clone()),
            ));
        };
        let Some(telemetry) = self.telemetry.as_mut() else {
            return Ok(FocusLifecycleReport::no_changes(
                epoch,
                Some(process.clone()),
            ));
        };
        let previous_selected = self.controller.selected().clone();
        self.controller
            .refresh_selected()
            .map_err(|_| SupervisorError::FocusFailure)?;
        if previous_selected
            .iter()
            .any(|identity| !self.controller.selected().contains(identity))
        {
            let report = self
                .controller
                .restore()
                .map_err(|_| SupervisorError::FocusFailure)?;
            return Ok(FocusLifecycleReport {
                epoch,
                process: Some(process.clone()),
                status: lifecycle_status(report.recovery_required, FocusLifecycleStatus::Restored),
                process_results: report.results,
                recovery_required: report.recovery_required,
                telemetry: None,
                adaptive_decision: None,
            });
        }
        let selected = self
            .controller
            .selected()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Ok(FocusLifecycleReport::no_changes(
                epoch,
                Some(process.clone()),
            ));
        }
        let snapshots = self
            .controller
            .backend_mut()
            .enumerate()
            .map_err(|_| SupervisorError::FocusFailure)?;
        let game_identity = FocusProcessIdentity {
            pid: process.pid,
            creation_time_100ns: process.creation_time_100ns,
            canonical_image: process.canonical_image.clone(),
        };
        let game_foreground = snapshots
            .iter()
            .any(|snapshot| snapshot.identity == game_identity && snapshot.foreground_family);
        let config = self.controller.config().clone();
        let protection_triggered = selected.iter().any(|identity| {
            snapshots
                .iter()
                .find(|snapshot| snapshot.identity == *identity)
                .is_none_or(|snapshot| {
                    let mut normalized = snapshot.clone();
                    normalized.priority = crate::profile_store::PriorityClass::Normal;
                    !evaluate_focus_candidate(&normalized, &config).is_eligible()
                })
        });
        let sample = match telemetry.sample(
            &game_identity,
            &selected,
            game_foreground,
            protection_triggered,
        ) {
            Ok(Some(sample)) => sample,
            Ok(None) | Err(FocusError::SampleTooSoon) => {
                return Ok(FocusLifecycleReport::no_changes(
                    epoch,
                    Some(process.clone()),
                ));
            }
            Err(_) => return Err(SupervisorError::FocusFailure),
        };
        self.apply_sample(process, epoch, &sample, &topology)
    }
}

impl FocusTelemetrySummary {
    fn from_sample(sample: &FocusTelemetrySample) -> Self {
        Self {
            game_foreground: sample.game_foreground,
            protection_triggered: sample.protection_triggered,
            total_cpu_basis_points: sample.total_cpu_basis_points,
            game_hot_thread_basis_points: sample.game_hot_thread_basis_points,
            competitor_count: sample.selected_process_loads.len(),
            max_competitor_basis_points: sample
                .selected_process_loads
                .iter()
                .map(|load| load.cpu_basis_points)
                .max()
                .unwrap_or(0),
        }
    }
}

impl FocusDecisionSummary {
    fn from_decision(decision: &FocusAdaptiveDecision) -> Self {
        Self {
            contention: decision.contention,
            action: decision.action,
            priority_target_count: decision.priority_targets.len(),
            background_cpu_set_ids: decision.background_cpu_set_ids.clone(),
            game_cpu_selection: decision.game_cpu_selection.clone(),
        }
    }
}

const fn lifecycle_status(
    recovery_required: bool,
    success: FocusLifecycleStatus,
) -> FocusLifecycleStatus {
    if recovery_required {
        FocusLifecycleStatus::RecoveryRequired
    } else {
        success
    }
}

/// Reversible Focus Mode effects owned by one validated game-process epoch.
pub trait FocusLifecycle {
    /// Recovers a durable journal before a new Focus Mode activation is allowed.
    fn recover(&mut self) -> Result<FocusLifecycleReport, SupervisorError>;
    fn activate(
        &mut self,
        process: &ObservedGame,
        epoch: u64,
    ) -> Result<FocusLifecycleReport, SupervisorError>;
    fn restore(
        &mut self,
        process: &ObservedGame,
        epoch: u64,
    ) -> Result<FocusLifecycleReport, SupervisorError>;
    fn tick(
        &mut self,
        process: &ObservedGame,
        epoch: u64,
    ) -> Result<FocusLifecycleReport, SupervisorError> {
        Ok(FocusLifecycleReport::no_changes(
            epoch,
            Some(process.clone()),
        ))
    }
}

#[derive(Default)]
pub struct NoopFocus;

impl FocusLifecycle for NoopFocus {
    fn recover(&mut self) -> Result<FocusLifecycleReport, SupervisorError> {
        Ok(FocusLifecycleReport::no_changes(0, None))
    }

    fn activate(
        &mut self,
        process: &ObservedGame,
        epoch: u64,
    ) -> Result<FocusLifecycleReport, SupervisorError> {
        Ok(FocusLifecycleReport::no_changes(
            epoch,
            Some(process.clone()),
        ))
    }

    fn restore(
        &mut self,
        process: &ObservedGame,
        epoch: u64,
    ) -> Result<FocusLifecycleReport, SupervisorError> {
        Ok(FocusLifecycleReport::no_changes(
            epoch,
            Some(process.clone()),
        ))
    }
}
