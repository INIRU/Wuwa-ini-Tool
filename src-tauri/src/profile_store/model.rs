use serde::{Deserialize, Serialize};

use crate::catalog::ProfileIniChange;
use crate::ini_document::ManagedChange;

pub const PROFILE_SCHEMA_VERSION: u32 = 1;
pub const SHARE_SCHEMA_VERSION: u32 = 1;
pub const MAX_SHARE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomEntryProvenance {
    Custom,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CustomIniEntry {
    pub section: String,
    pub key: String,
    pub value: String,
    pub provenance: CustomEntryProvenance,
    pub runtime_verified: bool,
}

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
    pub custom_ini_entries: Vec<CustomIniEntry>,
    pub process: ProcessProfile,
}

impl ProfilePatch {
    pub fn managed_changes(&self) -> Vec<ManagedChange> {
        let mut changes = self
            .managed_ini
            .iter()
            .map(|change| match &change.value {
                Some(value) => ManagedChange::set(&change.section, &change.key, value),
                None => ManagedChange::delete(&change.section, &change.key),
            })
            .collect::<Vec<_>>();
        changes.extend(
            self.custom_ini_entries
                .iter()
                .map(|entry| ManagedChange::set(&entry.section, &entry.key, &entry.value)),
        );
        changes
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CustomProfile {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub revision: u64,
    pub patch: ProfilePatch,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareProvenance {
    WuwaIniToolProfile,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareWarning {
    DeviceSpecificCpuExcluded,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableProfile {
    pub name: String,
    pub patch: ProfilePatch,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileShareEnvelope {
    pub schema_version: u32,
    pub creating_app_version: String,
    pub provenance: ShareProvenance,
    pub exported_at: String,
    pub portability_warnings: Vec<ShareWarning>,
    pub profile: PortableProfile,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProfileExport {
    pub suggested_file_name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportWarning {
    DeviceSpecificCpuReset,
    ElevatedPriority,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImportPreview {
    pub display_name: String,
    pub patch: ProfilePatch,
    pub warnings: Vec<ImportWarning>,
    pub source_app_version: String,
    pub exported_at: String,
}
