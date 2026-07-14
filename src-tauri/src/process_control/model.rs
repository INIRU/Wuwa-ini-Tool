use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    game_discovery::{validate_game_executable, GameInstallation},
    profile_store::{CpuSelection, PriorityClass},
};

use super::ProcessError;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CpuSetInfo {
    pub id: u32,
    pub group: u16,
    pub logical_processor_index: u8,
    pub core_index: u8,
    pub last_level_cache_index: u8,
    pub numa_node_index: u8,
    pub efficiency_class: u8,
    pub parked: bool,
    pub allocated: bool,
    pub allocated_to_target: bool,
    pub realtime: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProcessorGroup {
    pub group: u16,
    pub active_mask: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CpuTopology {
    pub cpu_sets: Vec<CpuSetInfo>,
    pub groups: Vec<ProcessorGroup>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CpuSetPlan {
    ResetAll,
    CpuSets(Vec<u32>),
    HardAffinity { group: u16, mask: u64 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AffinityReadback {
    pub process_mask: u64,
    pub system_mask: u64,
    pub groups: Vec<u16>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CpuReadback {
    pub default_cpu_sets: Vec<u32>,
    pub affinity: Option<AffinityReadback>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessReadback {
    pub cpu: CpuReadback,
    pub priority: PriorityClass,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    Success,
    Partial,
    Denied,
    Unsupported,
    Exited,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FieldReport<T> {
    pub requested: T,
    pub applied: Option<T>,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApplyReport {
    pub status: ApplyStatus,
    pub cpu: FieldReport<CpuSelection>,
    pub priority: FieldReport<PriorityClass>,
}

#[derive(Clone, Debug)]
pub struct ProcessTarget {
    pid: u32,
    expected_executable: PathBuf,
}

impl ProcessTarget {
    pub fn from_installation(
        pid: u32,
        installation: &GameInstallation,
    ) -> Result<Self, ProcessError> {
        if pid == 0 {
            return Err(ProcessError::InvalidProcessId);
        }
        let validated = validate_game_executable(&installation.executable)
            .map_err(|_| ProcessError::InvalidExecutableIdentity)?;
        if validated.game_root != installation.game_root
            || validated.executable != installation.executable
            || validated.engine_ini != installation.engine_ini
        {
            return Err(ProcessError::InvalidExecutableIdentity);
        }
        Ok(Self {
            pid,
            expected_executable: installation.executable.clone(),
        })
    }

    pub const fn pid(&self) -> u32 {
        self.pid
    }

    pub fn expected_executable(&self) -> &Path {
        &self.expected_executable
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApplyRequest {
    pub cpu_selection: CpuSelection,
    pub priority: PriorityClass,
    pub dangerous_priority_acknowledged: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameQosRequest {
    pub disable_execution_speed_throttling: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameQosState {
    pub execution_speed_throttled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameQosRestoreRecord {
    pub pid: u32,
    pub creation_time_100ns: u64,
    pub canonical_image: PathBuf,
    pub prior: GameQosState,
    pub applied: GameQosState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameQosApplyReport {
    pub prior: GameQosState,
    pub applied: GameQosState,
    pub restore_record: Option<GameQosRestoreRecord>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GameQosRestoreGuard {
    Restore,
    IdentityChanged,
    ExternallyChanged,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GameQosRestoreOutcome {
    Restored,
    IdentityChanged,
    ExternallyChanged,
}
