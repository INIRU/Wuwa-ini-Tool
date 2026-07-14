mod model;
mod validation;

use std::collections::{BTreeMap, BTreeSet};

pub use model::{
    BilingualText, BuiltinPreset, CatalogOption, OptionConstraints, OptionEvidence, OptionStatus,
    OptionValueType, ProfileIniChange, RiskLevel,
};
pub use validation::{validate_builtin, validate_option};

use model::{CatalogDocument, PresetDocument};

const SCHEMA_VERSION: u32 = 1;
const EMBEDDED_OPTIONS: &str = include_str!("../../../catalog/options.json");
const EMBEDDED_PRESETS: &str = include_str!("../../../catalog/presets.json");

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub options: BTreeMap<String, CatalogOption>,
    pub presets: Vec<BuiltinPreset>,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum CatalogError {
    #[error("invalid_json: {0}")]
    InvalidJson(String),
    #[error("unsupported_schema_version: {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("invalid_option: {key}: {reason}")]
    InvalidOption { key: String, reason: &'static str },
    #[error("unknown_profile_key: {0}")]
    UnknownProfileKey(String),
    #[error("unverified_preset_option: {0}")]
    UnverifiedPresetOption(String),
}

impl Catalog {
    pub fn load_embedded() -> Result<Self, CatalogError> {
        Self::from_json(EMBEDDED_OPTIONS, EMBEDDED_PRESETS)
    }

    pub fn from_json(options: &str, presets: &str) -> Result<Self, CatalogError> {
        let options: CatalogDocument = serde_json::from_str(options)
            .map_err(|error| CatalogError::InvalidJson(error.to_string()))?;
        let presets: PresetDocument = serde_json::from_str(presets)
            .map_err(|error| CatalogError::InvalidJson(error.to_string()))?;
        ensure_schema_version(options.schema_version)?;
        ensure_schema_version(presets.schema_version)?;

        let mut indexed = BTreeMap::new();
        let mut normalized_keys = BTreeSet::new();
        for option in options.options {
            validate_metadata(&option)?;
            let normalized = option.key.to_ascii_lowercase();
            if !normalized_keys.insert(normalized) {
                return Err(CatalogError::InvalidOption {
                    key: option.key,
                    reason: "duplicate_key",
                });
            }
            indexed.insert(option.key.clone(), option);
        }

        let catalog = Self {
            options: indexed,
            presets: presets.presets,
        };
        let mut preset_ids = BTreeSet::new();
        for preset in &catalog.presets {
            if preset.id.is_empty() || !preset_ids.insert(preset.id.as_str()) {
                return Err(CatalogError::InvalidOption {
                    key: preset.id.clone(),
                    reason: "invalid_preset_id",
                });
            }
            ensure_bilingual(&preset.name, &preset.id)?;
            ensure_bilingual(&preset.description, &preset.id)?;
            validate_builtin(&catalog, preset)?;
        }
        Ok(catalog)
    }
}

fn ensure_schema_version(version: u32) -> Result<(), CatalogError> {
    if version != SCHEMA_VERSION {
        return Err(CatalogError::UnsupportedSchemaVersion(version));
    }
    Ok(())
}

fn validate_metadata(option: &CatalogOption) -> Result<(), CatalogError> {
    if option.section.trim().is_empty() || option.key.trim().is_empty() {
        return invalid_metadata(&option.key, "missing_identity");
    }
    ensure_bilingual(&option.description, &option.key)?;
    let source_host = option
        .evidence
        .source_url
        .strip_prefix("https://")
        .and_then(|remainder| remainder.split('/').next());
    if !source_host.is_some_and(|host| {
        !host.is_empty() && host.contains('.') && !host.chars().any(char::is_whitespace)
    }) {
        return invalid_metadata(&option.key, "invalid_source_url");
    }
    if option.evidence.tested_game_version.trim().is_empty()
        || option.evidence.tested_date.trim().is_empty()
        || option.evidence.tested_hardware.trim().is_empty()
    {
        return invalid_metadata(&option.key, "incomplete_evidence");
    }
    let date_format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")
        .expect("static date format is valid");
    if time::Date::parse(&option.evidence.tested_date, &date_format).is_err() {
        return invalid_metadata(&option.key, "invalid_tested_date");
    }
    if option.status == OptionStatus::Verified && !option.evidence.runtime_verified {
        return invalid_metadata(&option.key, "verified_without_runtime_evidence");
    }
    if option.evidence.runtime_verified && !option.evidence.present_in_file {
        return invalid_metadata(&option.key, "runtime_verified_without_presence");
    }
    if option
        .constraints
        .minimum
        .zip(option.constraints.maximum)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return invalid_metadata(&option.key, "invalid_range");
    }
    if option
        .constraints
        .allowed_values
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return invalid_metadata(&option.key, "empty_allowed_value");
    }
    Ok(())
}

fn ensure_bilingual(text: &BilingualText, key: &str) -> Result<(), CatalogError> {
    if text.en.trim().is_empty() || text.ko.trim().is_empty() {
        return invalid_metadata(key, "missing_bilingual_text");
    }
    Ok(())
}

fn invalid_metadata<T>(key: &str, reason: &'static str) -> Result<T, CatalogError> {
    Err(CatalogError::InvalidOption {
        key: key.to_owned(),
        reason,
    })
}
