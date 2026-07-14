use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::profile_store::PriorityClass;

use super::{CpuSelection, CpuTopology};

pub const FOCUS_JOURNAL_SCHEMA_VERSION: u32 = 1;
const MAX_FOCUS_JOURNAL_BYTES: u64 = 1024 * 1024;
const MAX_FOCUS_JOURNAL_ENTRIES: usize = 4096;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FocusConfig {
    pub enabled: bool,
    pub pinned_executables: BTreeSet<PathBuf>,
    pub thresholds: FocusThresholds,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusThresholds {
    pub sample_interval_ms: u32,
    pub aggregate_contention_basis_points: u16,
    pub release_basis_points: u16,
    pub game_hot_thread_basis_points: u16,
    pub competitor_basis_points: u16,
    pub sustained_samples: u8,
    pub release_samples: u8,
}

impl Default for FocusThresholds {
    fn default() -> Self {
        Self {
            sample_interval_ms: 1000,
            aggregate_contention_basis_points: 8000,
            release_basis_points: 6500,
            game_hot_thread_basis_points: 9000,
            competitor_basis_points: 500,
            sustained_samples: 3,
            release_samples: 3,
        }
    }
}

impl FocusThresholds {
    pub fn bounded(self) -> Self {
        let aggregate = self.aggregate_contention_basis_points.clamp(100, 10_000);
        Self {
            sample_interval_ms: self.sample_interval_ms.clamp(250, 5_000),
            aggregate_contention_basis_points: aggregate,
            release_basis_points: self.release_basis_points.min(aggregate.saturating_sub(1)),
            game_hot_thread_basis_points: self.game_hot_thread_basis_points.clamp(100, 10_000),
            competitor_basis_points: self.competitor_basis_points.clamp(1, 10_000),
            sustained_samples: self.sustained_samples.clamp(2, 10),
            release_samples: self.release_samples.clamp(2, 10),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusProcessLoad {
    pub identity: FocusProcessIdentity,
    pub cpu_basis_points: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusTelemetrySample {
    pub game_foreground: bool,
    pub protection_triggered: bool,
    pub total_cpu_basis_points: u16,
    pub per_logical_cpu_basis_points: Vec<u16>,
    pub game_hot_thread_basis_points: u16,
    pub selected_process_loads: Vec<FocusProcessLoad>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusContentionKind {
    None,
    Aggregate,
    GameHotThread,
    EngineBound,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusAdaptiveAction {
    Observe,
    RestrainPriority,
    RecommendSoftCpuSets,
    Restore,
    EngineBoundNoAction,
    UnsupportedTopologyNoAction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusAdaptiveDecision {
    pub contention: FocusContentionKind,
    pub action: FocusAdaptiveAction,
    pub priority_targets: Vec<FocusProcessIdentity>,
    pub background_cpu_set_ids: Vec<u32>,
    pub game_cpu_selection: CpuSelection,
    pub thresholds: FocusThresholds,
}

#[derive(Clone, Debug)]
pub struct AdaptiveFocusPolicy {
    thresholds: FocusThresholds,
    contention_streak: u8,
    competitor_streaks: BTreeMap<FocusProcessIdentity, u8>,
    hot_thread_streak: u8,
    release_streak: u8,
    restrained_targets: BTreeSet<FocusProcessIdentity>,
}

impl AdaptiveFocusPolicy {
    pub fn new(thresholds: FocusThresholds) -> Self {
        Self {
            thresholds: thresholds.bounded(),
            contention_streak: 0,
            competitor_streaks: BTreeMap::new(),
            hot_thread_streak: 0,
            release_streak: 0,
            restrained_targets: BTreeSet::new(),
        }
    }

    pub fn evaluate(
        &mut self,
        sample: &FocusTelemetrySample,
        topology: &CpuTopology,
    ) -> FocusAdaptiveDecision {
        let current_competitors = sample
            .selected_process_loads
            .iter()
            .filter(|load| load.cpu_basis_points >= self.thresholds.competitor_basis_points)
            .map(|load| load.identity.clone())
            .collect::<BTreeSet<_>>();
        self.competitor_streaks
            .retain(|identity, _| current_competitors.contains(identity));
        for identity in &current_competitors {
            let streak = self.competitor_streaks.entry(identity.clone()).or_default();
            *streak = streak.saturating_add(1);
        }
        let priority_targets = self
            .competitor_streaks
            .iter()
            .filter(|(_, streak)| **streak >= self.thresholds.sustained_samples)
            .map(|(identity, _)| identity.clone())
            .collect::<Vec<_>>();
        let competitor = !current_competitors.is_empty();
        let hot_thread =
            sample.game_hot_thread_basis_points >= self.thresholds.game_hot_thread_basis_points;
        if hot_thread {
            self.hot_thread_streak = self.hot_thread_streak.saturating_add(1);
        } else {
            self.hot_thread_streak = 0;
        }
        if !sample.game_foreground || sample.protection_triggered {
            self.contention_streak = 0;
            self.competitor_streaks.clear();
            self.hot_thread_streak = 0;
            self.release_streak = 0;
            self.restrained_targets.clear();
            return self.decision(
                FocusContentionKind::None,
                FocusAdaptiveAction::Restore,
                vec![],
                vec![],
            );
        }
        if sample.total_cpu_basis_points >= self.thresholds.aggregate_contention_basis_points
            && competitor
        {
            self.release_streak = 0;
            self.contention_streak = self.contention_streak.saturating_add(1);
            let target_set = priority_targets.iter().cloned().collect::<BTreeSet<_>>();
            if !self.restrained_targets.is_empty() && target_set != self.restrained_targets {
                self.contention_streak = 0;
                self.restrained_targets.clear();
                return self.decision(
                    FocusContentionKind::Aggregate,
                    FocusAdaptiveAction::Restore,
                    vec![],
                    vec![],
                );
            }
            if self.restrained_targets.is_empty()
                && self.contention_streak >= self.thresholds.sustained_samples
                && !priority_targets.is_empty()
            {
                self.restrained_targets = target_set;
                return self.decision(
                    FocusContentionKind::Aggregate,
                    FocusAdaptiveAction::RestrainPriority,
                    priority_targets,
                    vec![],
                );
            }
            return self.decision(
                FocusContentionKind::Aggregate,
                FocusAdaptiveAction::Observe,
                vec![],
                vec![],
            );
        }
        self.contention_streak = 0;
        if !self.restrained_targets.is_empty()
            && (sample.total_cpu_basis_points <= self.thresholds.release_basis_points
                || !competitor)
        {
            self.release_streak = self.release_streak.saturating_add(1);
            if self.release_streak >= self.thresholds.release_samples {
                self.release_streak = 0;
                self.restrained_targets.clear();
                return self.decision(
                    FocusContentionKind::None,
                    FocusAdaptiveAction::Restore,
                    vec![],
                    vec![],
                );
            }
        } else {
            self.release_streak = 0;
        }
        if hot_thread && self.hot_thread_streak >= self.thresholds.sustained_samples {
            if !competitor {
                return self.decision(
                    FocusContentionKind::EngineBound,
                    FocusAdaptiveAction::EngineBoundNoAction,
                    vec![],
                    vec![],
                );
            }
            return match background_headroom_cpu_sets(topology) {
                Some(ids) => self.decision(
                    FocusContentionKind::GameHotThread,
                    FocusAdaptiveAction::RecommendSoftCpuSets,
                    vec![],
                    ids,
                ),
                None => self.decision(
                    FocusContentionKind::GameHotThread,
                    FocusAdaptiveAction::UnsupportedTopologyNoAction,
                    vec![],
                    vec![],
                ),
            };
        }
        self.decision(
            FocusContentionKind::None,
            FocusAdaptiveAction::Observe,
            vec![],
            vec![],
        )
    }

    fn decision(
        &self,
        contention: FocusContentionKind,
        action: FocusAdaptiveAction,
        priority_targets: Vec<FocusProcessIdentity>,
        background_cpu_set_ids: Vec<u32>,
    ) -> FocusAdaptiveDecision {
        FocusAdaptiveDecision {
            contention,
            action,
            priority_targets,
            background_cpu_set_ids,
            game_cpu_selection: CpuSelection::All,
            thresholds: self.thresholds,
        }
    }
}

pub fn background_headroom_cpu_sets(topology: &CpuTopology) -> Option<Vec<u32>> {
    let available = topology
        .cpu_sets
        .iter()
        .filter(|cpu| !cpu.allocated || cpu.allocated_to_target)
        .collect::<Vec<_>>();
    if available.len() < 4 {
        return None;
    }
    let minimum = available.iter().map(|cpu| cpu.efficiency_class).min()?;
    let maximum = available.iter().map(|cpu| cpu.efficiency_class).max()?;
    if minimum != maximum {
        let selected = available
            .iter()
            .filter(|cpu| cpu.efficiency_class < maximum)
            .map(|cpu| cpu.id)
            .collect::<Vec<_>>();
        let reserved = available.len().saturating_sub(selected.len());
        return (selected.len() >= 2 && reserved >= 2).then_some(selected);
    }

    let mut by_core = BTreeMap::<(u16, u8), Vec<u32>>::new();
    for cpu in available {
        by_core
            .entry((cpu.group, cpu.core_index))
            .or_default()
            .push(cpu.id);
    }
    if by_core.len() < 6 {
        return None;
    }
    let background_core_count = by_core.len() - 4;
    let mut selected = by_core
        .into_values()
        .take(background_core_count)
        .flatten()
        .collect::<Vec<_>>();
    selected.sort_unstable();
    (!selected.is_empty()).then_some(selected)
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusProcessIdentity {
    pub pid: u32,
    pub creation_time_100ns: u64,
    pub canonical_image: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusProcessSnapshot {
    pub identity: FocusProcessIdentity,
    pub display_name: String,
    pub priority: PriorityClass,
    pub same_user: bool,
    pub same_session: bool,
    pub session_zero: bool,
    pub system_process: bool,
    pub protected_process: bool,
    pub critical_process: bool,
    pub access_denied: bool,
    pub game_process: bool,
    pub tool_process: bool,
    pub launcher_or_overlay: bool,
    pub foreground_family: bool,
    pub visible_window_family: bool,
    pub active_audio: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusProcessStatus {
    Eligible,
    AccessDenied,
    DifferentUser,
    DifferentSession,
    SessionZero,
    System,
    Protected,
    Critical,
    Game,
    Tool,
    LauncherOrOverlay,
    Foreground,
    VisibleWindow,
    ActiveAudio,
    Pinned,
    Communication,
    Recording,
    Streaming,
    CaptureOverlay,
    PriorityNotNormal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusCandidate {
    pub identity: FocusProcessIdentity,
    pub display_name: String,
    pub current_priority: PriorityClass,
    pub status: FocusProcessStatus,
}

impl FocusCandidate {
    pub const fn is_eligible(&self) -> bool {
        matches!(self.status, FocusProcessStatus::Eligible)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusPreview {
    pub token: u64,
    pub thresholds: FocusThresholds,
    pub candidates: Vec<FocusCandidate>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusActivationRequest {
    pub preview_token: u64,
    pub selected: Vec<FocusProcessIdentity>,
    pub select_all_eligible: bool,
    pub select_all_confirmed: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusRestoreOutcome {
    Applied,
    Selected,
    Restored,
    Skipped,
    Exited,
    Denied,
    IdentityChanged,
    ExternallyChanged,
    RecoveryRequired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusProcessResult {
    pub identity: FocusProcessIdentity,
    pub outcome: FocusRestoreOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusActivationReport {
    pub results: Vec<FocusProcessResult>,
    pub recovery_required: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusRestoreReport {
    pub results: Vec<FocusProcessResult>,
    pub recovery_required: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusJournalEntry {
    pub identity: FocusProcessIdentity,
    pub prior_priority: PriorityClass,
    pub applied_priority: PriorityClass,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusJournal {
    pub schema_version: u32,
    pub entries: Vec<FocusJournalEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FocusError {
    #[error("focus_mode_disabled")]
    Disabled,
    #[error("preview_required")]
    PreviewRequired,
    #[error("stale_preview")]
    StalePreview,
    #[error("explicit_confirmation_required")]
    ExplicitConfirmationRequired,
    #[error("invalid_focus_selection")]
    InvalidSelection,
    #[error("focus_recovery_required")]
    RecoveryRequired,
    #[error("focus_access_denied")]
    AccessDenied,
    #[error("focus_backend_failure")]
    BackendFailure,
    #[error("focus_journal_failure")]
    JournalFailure,
    #[error("focus_sample_too_soon")]
    SampleTooSoon,
    #[error("focus_telemetry_unavailable")]
    TelemetryUnavailable,
}

pub trait FocusBackend {
    fn enumerate(&mut self) -> Result<Vec<FocusProcessSnapshot>, FocusError>;
    fn inspect(&mut self, pid: u32) -> Result<Option<FocusProcessSnapshot>, FocusError>;
    fn set_priority(
        &mut self,
        identity: &FocusProcessIdentity,
        priority: PriorityClass,
    ) -> Result<(), FocusError>;
}

pub trait FocusJournalStore {
    fn load(&self) -> Result<Option<FocusJournal>, FocusError>;
    fn save(&mut self, journal: &FocusJournal) -> Result<(), FocusError>;
    fn clear(&mut self) -> Result<(), FocusError>;
}

#[derive(Clone, Debug)]
pub struct FileFocusJournalStore {
    path: PathBuf,
}

impl FileFocusJournalStore {
    pub fn new(app_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            path: app_data_dir.into().join("focus-mode-journal.json"),
        }
    }
}

impl FocusJournalStore for FileFocusJournalStore {
    fn load(&self) -> Result<Option<FocusJournal>, FocusError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(FocusError::JournalFailure),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(FocusError::JournalFailure);
        }
        if metadata.len() > MAX_FOCUS_JOURNAL_BYTES {
            return Err(FocusError::JournalFailure);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        File::open(&self.path)
            .map_err(|_| FocusError::JournalFailure)?
            .take(MAX_FOCUS_JOURNAL_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| FocusError::JournalFailure)?;
        if bytes.len() as u64 > MAX_FOCUS_JOURNAL_BYTES {
            return Err(FocusError::JournalFailure);
        }
        let journal = serde_json::from_slice::<FocusJournal>(&bytes)
            .map_err(|_| FocusError::JournalFailure)?;
        validate_journal(&journal)?;
        Ok(Some(journal))
    }

    fn save(&mut self, journal: &FocusJournal) -> Result<(), FocusError> {
        validate_journal(journal)?;
        let bytes = serde_json::to_vec(journal).map_err(|_| FocusError::JournalFailure)?;
        if bytes.len() as u64 > MAX_FOCUS_JOURNAL_BYTES {
            return Err(FocusError::JournalFailure);
        }
        let parent = self.path.parent().ok_or(FocusError::JournalFailure)?;
        fs::create_dir_all(parent).map_err(|_| FocusError::JournalFailure)?;
        let temporary = parent.join(format!(".focus-mode-journal.{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| FocusError::JournalFailure)?;
        let result = (|| {
            file.write_all(&bytes)
                .map_err(|_| FocusError::JournalFailure)?;
            file.sync_all().map_err(|_| FocusError::JournalFailure)?;
            replace_journal(&temporary, &self.path)?;
            sync_parent(parent)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn clear(&mut self) -> Result<(), FocusError> {
        match fs::remove_file(&self.path) {
            Ok(()) => self
                .path
                .parent()
                .ok_or(FocusError::JournalFailure)
                .and_then(sync_parent),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(FocusError::JournalFailure),
        }
    }
}

pub fn evaluate_focus_candidate(
    snapshot: &FocusProcessSnapshot,
    config: &FocusConfig,
) -> FocusCandidate {
    let status = if snapshot.access_denied {
        FocusProcessStatus::AccessDenied
    } else if !snapshot.same_user {
        FocusProcessStatus::DifferentUser
    } else if !snapshot.same_session {
        FocusProcessStatus::DifferentSession
    } else if snapshot.session_zero {
        FocusProcessStatus::SessionZero
    } else if snapshot.system_process {
        FocusProcessStatus::System
    } else if snapshot.protected_process {
        FocusProcessStatus::Protected
    } else if snapshot.critical_process {
        FocusProcessStatus::Critical
    } else if snapshot.game_process {
        FocusProcessStatus::Game
    } else if snapshot.tool_process {
        FocusProcessStatus::Tool
    } else if snapshot.launcher_or_overlay {
        FocusProcessStatus::LauncherOrOverlay
    } else if snapshot.foreground_family {
        FocusProcessStatus::Foreground
    } else if snapshot.visible_window_family {
        FocusProcessStatus::VisibleWindow
    } else if snapshot.active_audio {
        FocusProcessStatus::ActiveAudio
    } else if config
        .pinned_executables
        .iter()
        .any(|path| paths_match(path, &snapshot.identity.canonical_image))
    {
        FocusProcessStatus::Pinned
    } else if let Some(status) = default_protection(snapshot) {
        status
    } else {
        priority_status(snapshot.priority)
    };
    FocusCandidate {
        identity: snapshot.identity.clone(),
        display_name: snapshot.display_name.clone(),
        current_priority: snapshot.priority,
        status,
    }
}

pub struct FocusModeController<B, S> {
    backend: B,
    journal_store: S,
    config: FocusConfig,
    latest_preview: Option<FocusPreview>,
    selected: BTreeSet<FocusProcessIdentity>,
    next_token: u64,
}

impl<B: FocusBackend, S: FocusJournalStore> FocusModeController<B, S> {
    pub fn new(backend: B, journal_store: S, config: FocusConfig) -> Self {
        Self {
            backend,
            journal_store,
            config,
            latest_preview: None,
            selected: BTreeSet::new(),
            next_token: 1,
        }
    }

    pub fn preview(&mut self) -> Result<FocusPreview, FocusError> {
        let preview = FocusPreview {
            token: self.next_token,
            thresholds: self.config.thresholds.bounded(),
            candidates: self
                .backend
                .enumerate()?
                .iter()
                .map(|snapshot| evaluate_focus_candidate(snapshot, &self.config))
                .collect(),
        };
        self.next_token = self.next_token.saturating_add(1);
        self.latest_preview = Some(preview.clone());
        Ok(preview)
    }

    pub fn activate(
        &mut self,
        request: &FocusActivationRequest,
    ) -> Result<FocusActivationReport, FocusError> {
        if !self.config.enabled {
            return Err(FocusError::Disabled);
        }
        let preview = self
            .latest_preview
            .as_ref()
            .ok_or(FocusError::PreviewRequired)?;
        if preview.token != request.preview_token {
            return Err(FocusError::StalePreview);
        }
        if self.journal_store.load()?.is_some() {
            return Err(FocusError::RecoveryRequired);
        }

        let selected = if request.select_all_eligible {
            if !request.select_all_confirmed {
                return Err(FocusError::ExplicitConfirmationRequired);
            }
            preview
                .candidates
                .iter()
                .filter(|candidate| candidate.is_eligible())
                .map(|candidate| candidate.identity.clone())
                .collect::<Vec<_>>()
        } else {
            if request.selected.is_empty() {
                return Err(FocusError::InvalidSelection);
            }
            let unique = request.selected.iter().collect::<BTreeSet<_>>();
            if unique.len() != request.selected.len()
                || request.selected.iter().any(|identity| {
                    !preview.candidates.iter().any(|candidate| {
                        candidate.is_eligible() && identities_match(&candidate.identity, identity)
                    })
                })
            {
                return Err(FocusError::InvalidSelection);
            }
            request.selected.clone()
        };

        self.selected = selected.iter().cloned().collect();
        Ok(FocusActivationReport {
            results: selected
                .into_iter()
                .map(|identity| process_result(identity, FocusRestoreOutcome::Selected))
                .collect(),
            recovery_required: false,
        })
    }

    /// Applies the reversible priority restraint selected by the adaptive policy.
    /// Selection alone never mutates another process.
    pub fn apply_priority_restraint(
        &mut self,
        decision: &FocusAdaptiveDecision,
    ) -> Result<FocusActivationReport, FocusError> {
        if decision.action != FocusAdaptiveAction::RestrainPriority {
            return Ok(FocusActivationReport {
                results: Vec::new(),
                recovery_required: false,
            });
        }
        if !self.config.enabled {
            return Err(FocusError::Disabled);
        }
        if self.journal_store.load()?.is_some() {
            return Err(FocusError::RecoveryRequired);
        }

        let selected = decision
            .priority_targets
            .iter()
            .filter(|identity| self.selected.contains(*identity))
            .cloned()
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Err(FocusError::InvalidSelection);
        }
        let mut journal = FocusJournal {
            schema_version: FOCUS_JOURNAL_SCHEMA_VERSION,
            entries: Vec::new(),
        };
        let mut results = Vec::with_capacity(selected.len());
        let mut recovery_required = false;
        for identity in selected {
            let snapshot = match self.backend.inspect(identity.pid) {
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => {
                    results.push(process_result(identity, FocusRestoreOutcome::Exited));
                    continue;
                }
                Err(FocusError::AccessDenied) => {
                    results.push(process_result(identity, FocusRestoreOutcome::Denied));
                    continue;
                }
                Err(_) => {
                    results.push(process_result(
                        identity,
                        FocusRestoreOutcome::RecoveryRequired,
                    ));
                    recovery_required = true;
                    continue;
                }
            };
            if !identities_match(&snapshot.identity, &identity)
                || !evaluate_focus_candidate(&snapshot, &self.config).is_eligible()
            {
                results.push(process_result(identity, FocusRestoreOutcome::Skipped));
                continue;
            }

            let entry = FocusJournalEntry {
                identity: identity.clone(),
                prior_priority: snapshot.priority,
                applied_priority: PriorityClass::BelowNormal,
            };
            journal.entries.push(entry);
            self.journal_store.save(&journal)?;
            match self
                .backend
                .set_priority(&identity, PriorityClass::BelowNormal)
            {
                Ok(()) => {}
                Err(FocusError::AccessDenied) => {
                    results.push(process_result(identity, FocusRestoreOutcome::Denied));
                    recovery_required = true;
                    continue;
                }
                Err(_) => {
                    results.push(process_result(
                        identity,
                        FocusRestoreOutcome::RecoveryRequired,
                    ));
                    recovery_required = true;
                    continue;
                }
            }
            match self.backend.inspect(identity.pid) {
                Ok(Some(readback))
                    if identities_match(&readback.identity, &identity)
                        && readback.priority == PriorityClass::BelowNormal =>
                {
                    self.journal_store.save(&journal)?;
                    results.push(process_result(identity, FocusRestoreOutcome::Applied));
                }
                _ => {
                    results.push(process_result(
                        identity,
                        FocusRestoreOutcome::RecoveryRequired,
                    ));
                    recovery_required = true;
                }
            }
        }
        Ok(FocusActivationReport {
            results,
            recovery_required,
        })
    }

    pub fn restore(&mut self) -> Result<FocusRestoreReport, FocusError> {
        let Some(journal) = self.journal_store.load()? else {
            return Ok(FocusRestoreReport {
                results: Vec::new(),
                recovery_required: false,
            });
        };
        validate_journal(&journal)?;
        let mut remaining = Vec::new();
        let mut results = Vec::with_capacity(journal.entries.len());
        for entry in journal.entries {
            let identity = entry.identity.clone();
            let snapshot = match self.backend.inspect(identity.pid) {
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => {
                    results.push(process_result(identity, FocusRestoreOutcome::Exited));
                    continue;
                }
                Err(FocusError::AccessDenied) => {
                    remaining.push(entry);
                    results.push(process_result(identity, FocusRestoreOutcome::Denied));
                    continue;
                }
                Err(_) => {
                    remaining.push(entry);
                    results.push(process_result(
                        identity,
                        FocusRestoreOutcome::RecoveryRequired,
                    ));
                    continue;
                }
            };
            if !identities_match(&snapshot.identity, &identity) {
                results.push(process_result(
                    identity,
                    FocusRestoreOutcome::IdentityChanged,
                ));
                continue;
            }
            if snapshot.priority != entry.applied_priority {
                results.push(process_result(
                    identity,
                    FocusRestoreOutcome::ExternallyChanged,
                ));
                continue;
            }
            match self.backend.set_priority(&identity, entry.prior_priority) {
                Ok(()) => {}
                Err(FocusError::AccessDenied) => {
                    remaining.push(entry);
                    results.push(process_result(identity, FocusRestoreOutcome::Denied));
                    continue;
                }
                Err(_) => {
                    remaining.push(entry);
                    results.push(process_result(
                        identity,
                        FocusRestoreOutcome::RecoveryRequired,
                    ));
                    continue;
                }
            }
            match self.backend.inspect(identity.pid) {
                Ok(Some(readback))
                    if identities_match(&readback.identity, &identity)
                        && readback.priority == entry.prior_priority =>
                {
                    results.push(process_result(identity, FocusRestoreOutcome::Restored));
                }
                _ => {
                    remaining.push(entry);
                    results.push(process_result(
                        identity,
                        FocusRestoreOutcome::RecoveryRequired,
                    ));
                }
            }
        }
        let recovery_required = !remaining.is_empty();
        if recovery_required {
            self.journal_store.save(&FocusJournal {
                schema_version: FOCUS_JOURNAL_SCHEMA_VERSION,
                entries: remaining,
            })?;
        } else {
            self.journal_store.clear()?;
        }
        Ok(FocusRestoreReport {
            results,
            recovery_required,
        })
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn journal_store(&self) -> &S {
        &self.journal_store
    }

    pub fn journal_store_mut(&mut self) -> &mut S {
        &mut self.journal_store
    }
}

fn priority_status(priority: PriorityClass) -> FocusProcessStatus {
    if priority == PriorityClass::Normal {
        FocusProcessStatus::Eligible
    } else {
        FocusProcessStatus::PriorityNotNormal
    }
}

fn default_protection(snapshot: &FocusProcessSnapshot) -> Option<FocusProcessStatus> {
    let name = snapshot
        .identity
        .canonical_image
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&snapshot.display_name)
        .to_ascii_lowercase();
    if name.contains("discord")
        || name.contains("teamspeak")
        || name.contains("mumble")
        || name == "teams.exe"
    {
        Some(FocusProcessStatus::Communication)
    } else if name == "obs.exe" || name == "obs64.exe" || name.starts_with("obs-") {
        Some(FocusProcessStatus::Recording)
    } else if name.contains("streamlabs") || name.contains("xsplit") {
        Some(FocusProcessStatus::Streaming)
    } else if name.contains("xboxgamebar")
        || name.contains("gamebar")
        || name.contains("nvidia share")
        || name == "nvcontainer.exe"
        || name.contains("radeonsoftware")
        || name.contains("amdow")
    {
        Some(FocusProcessStatus::CaptureOverlay)
    } else {
        None
    }
}

fn identities_match(left: &FocusProcessIdentity, right: &FocusProcessIdentity) -> bool {
    left.pid == right.pid
        && left.creation_time_100ns == right.creation_time_100ns
        && paths_match(&left.canonical_image, &right.canonical_image)
}

fn paths_match(left: &Path, right: &Path) -> bool {
    let left = left.to_string_lossy();
    let right = right.to_string_lossy();
    if cfg!(target_os = "windows") {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

fn process_result(
    identity: FocusProcessIdentity,
    outcome: FocusRestoreOutcome,
) -> FocusProcessResult {
    FocusProcessResult { identity, outcome }
}

fn validate_journal(journal: &FocusJournal) -> Result<(), FocusError> {
    if journal.schema_version != FOCUS_JOURNAL_SCHEMA_VERSION
        || journal.entries.len() > MAX_FOCUS_JOURNAL_ENTRIES
    {
        return Err(FocusError::JournalFailure);
    }
    let mut identities = BTreeSet::new();
    for entry in &journal.entries {
        if entry.identity.pid == 0
            || entry.identity.canonical_image.as_os_str().is_empty()
            || entry.prior_priority != PriorityClass::Normal
            || entry.applied_priority != PriorityClass::BelowNormal
            || !identities.insert((entry.identity.pid, entry.identity.creation_time_100ns))
        {
            return Err(FocusError::JournalFailure);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn replace_journal(temporary: &Path, destination: &Path) -> Result<(), FocusError> {
    fs::rename(temporary, destination).map_err(|_| FocusError::JournalFailure)
}

#[cfg(target_os = "windows")]
fn replace_journal(temporary: &Path, destination: &Path) -> Result<(), FocusError> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let existing = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replacement = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // SAFETY: both paths are NUL-terminated UTF-16 buffers valid for the duration of the call.
    let moved = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            replacement.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    (moved != 0).then_some(()).ok_or(FocusError::JournalFailure)
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> Result<(), FocusError> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| FocusError::JournalFailure)
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> Result<(), FocusError> {
    Ok(())
}
