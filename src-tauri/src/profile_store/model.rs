use serde::{Deserialize, Serialize};

use crate::catalog::ProfileIniChange;
use crate::ini_document::ManagedChange;

pub const PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityClass {
    Idle,
    BelowNormal,
    #[default]
    Normal,
    AboveNormal,
    High,
    Realtime,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum CpuSelection {
    #[default]
    All,
    PreferPerformance,
    ManualCpuSets {
        ids: Vec<u32>,
    },
    HardAffinity {
        group: u16,
        mask: u64,
    },
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessProfile {
    pub cpu_selection: CpuSelection,
    pub priority: PriorityClass,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePatch {
    pub schema_version: u32,
    pub managed_ini: Vec<ProfileIniChange>,
    pub process: ProcessProfile,
}

impl ProfilePatch {
    pub fn managed_changes(&self) -> Vec<ManagedChange> {
        self.managed_ini
            .iter()
            .map(|change| match &change.value {
                Some(value) => ManagedChange::set(&change.section, &change.key, value),
                None => ManagedChange::delete(&change.section, &change.key),
            })
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CustomProfile {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub patch: ProfilePatch,
}
