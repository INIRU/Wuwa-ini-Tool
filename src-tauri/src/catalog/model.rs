use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BilingualText {
    pub en: String,
    pub ko: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionStatus {
    Verified,
    CommunityReported,
    Experimental,
    Ignored,
    Regressed,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionValueType {
    Boolean,
    Integer,
    Float,
    Text,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OptionConstraints {
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    #[serde(default)]
    pub allowed_values: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OptionEvidence {
    pub source_url: String,
    pub tested_game_version: String,
    pub tested_date: String,
    pub tested_hardware: String,
    pub present_in_file: bool,
    pub runtime_verified: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogOption {
    pub section: String,
    pub key: String,
    pub description: BilingualText,
    pub value_type: OptionValueType,
    pub constraints: OptionConstraints,
    pub risk: RiskLevel,
    pub status: OptionStatus,
    pub evidence: OptionEvidence,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileIniChange {
    pub section: String,
    pub key: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinPreset {
    pub id: String,
    pub name: BilingualText,
    pub description: BilingualText,
    pub changes: Vec<ProfileIniChange>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogDocument {
    pub schema_version: u32,
    pub options: Vec<CatalogOption>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PresetDocument {
    pub schema_version: u32,
    pub presets: Vec<BuiltinPreset>,
}
