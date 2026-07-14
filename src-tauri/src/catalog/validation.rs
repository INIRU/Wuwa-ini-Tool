use std::collections::BTreeSet;

use super::{BuiltinPreset, Catalog, CatalogError, CatalogOption, OptionStatus, OptionValueType};

const MAX_VALUE_CHARS: usize = 8192;

pub fn validate_option(option: &CatalogOption, value: &str) -> Result<(), CatalogError> {
    if value.chars().any(char::is_control) {
        return invalid(&option.key, "control_character");
    }
    let value = value.trim_matches(|character: char| character.is_ascii_whitespace());
    if value.is_empty() || value.chars().count() > MAX_VALUE_CHARS {
        return invalid(&option.key, "empty_value");
    }

    if !option.constraints.allowed_values.is_empty()
        && !option
            .constraints
            .allowed_values
            .iter()
            .any(|allowed| allowed == value)
    {
        return invalid(&option.key, "value_not_allowed");
    }

    let numeric = match option.value_type {
        OptionValueType::Boolean => {
            if matches!(value, "true" | "false" | "0" | "1") {
                None
            } else {
                return invalid(&option.key, "expected_boolean");
            }
        }
        OptionValueType::Integer => {
            let parsed = value
                .parse::<i64>()
                .map_err(|_| CatalogError::InvalidOption {
                    key: option.key.clone(),
                    reason: "expected_integer",
                })?;
            Some(parsed as f64)
        }
        OptionValueType::Float => {
            let parsed = value
                .parse::<f64>()
                .map_err(|_| CatalogError::InvalidOption {
                    key: option.key.clone(),
                    reason: "expected_float",
                })?;
            if !parsed.is_finite() {
                return invalid(&option.key, "expected_finite_float");
            }
            Some(parsed)
        }
        OptionValueType::Text => None,
    };

    if let Some(number) = numeric {
        if option
            .constraints
            .minimum
            .is_some_and(|minimum| number < minimum)
        {
            return invalid(&option.key, "below_minimum");
        }
        if option
            .constraints
            .maximum
            .is_some_and(|maximum| number > maximum)
        {
            return invalid(&option.key, "above_maximum");
        }
    }
    Ok(())
}

pub fn validate_builtin(catalog: &Catalog, preset: &BuiltinPreset) -> Result<(), CatalogError> {
    let is_vanilla = preset.id == "vanilla";
    let mut seen = BTreeSet::new();
    for change in &preset.changes {
        let option = catalog
            .options
            .get(&change.key)
            .ok_or_else(|| CatalogError::UnknownProfileKey(change.key.clone()))?;
        if option.section != change.section {
            return invalid(&change.key, "section_mismatch");
        }
        if !seen.insert(change.key.to_ascii_lowercase()) {
            return invalid(&change.key, "duplicate_preset_key");
        }
        if is_vanilla && change.value.is_some() {
            return invalid(&change.key, "vanilla_must_delete_values");
        }
        if let Some(value) = change.value.as_deref() {
            if option.status != OptionStatus::Verified || !option.evidence.runtime_verified {
                return Err(CatalogError::UnverifiedPresetOption(change.key.clone()));
            }
            validate_option(option, value)?;
        } else if !is_vanilla
            && (option.status != OptionStatus::Verified || !option.evidence.runtime_verified)
        {
            return Err(CatalogError::UnverifiedPresetOption(change.key.clone()));
        }
    }
    if is_vanilla
        && (preset.changes.len() != catalog.options.len()
            || catalog
                .options
                .keys()
                .any(|key| !seen.contains(&key.to_ascii_lowercase())))
    {
        return invalid(&preset.id, "vanilla_must_delete_every_catalog_key_once");
    }
    Ok(())
}

fn invalid(key: &str, reason: &'static str) -> Result<(), CatalogError> {
    Err(CatalogError::InvalidOption {
        key: key.to_owned(),
        reason,
    })
}
