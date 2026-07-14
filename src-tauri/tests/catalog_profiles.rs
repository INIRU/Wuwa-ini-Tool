use std::{collections::BTreeMap, path::PathBuf};

use wuwa_ini_tool_lib::{
    catalog::{
        validate_builtin, validate_option, BilingualText, BuiltinPreset, Catalog, CatalogError,
        CatalogOption, OptionConstraints, OptionEvidence, OptionStatus, OptionValueType,
        ProfileIniChange, RiskLevel,
    },
    ini_document::IniDocument,
    profile_store::{
        CpuSelection, CustomProfile, PriorityClass, ProcessProfile, ProfileError, ProfilePatch,
        ProfileStore, PROFILE_SCHEMA_VERSION,
    },
};

fn evidence(runtime_verified: bool) -> OptionEvidence {
    OptionEvidence {
        source_url: "https://dev.epicgames.com/documentation/".into(),
        tested_game_version: "2.5".into(),
        tested_date: "2026-07-14".into(),
        tested_hardware: "Windows 11 x64; Ryzen 7; 32 GB RAM".into(),
        present_in_file: true,
        runtime_verified,
    }
}

fn option(key: &str, status: OptionStatus, runtime_verified: bool) -> CatalogOption {
    CatalogOption {
        section: "SystemSettings".into(),
        key: key.into(),
        description: BilingualText {
            en: "Original English explanation.".into(),
            ko: "독자적으로 작성한 한국어 설명입니다.".into(),
        },
        value_type: OptionValueType::Integer,
        constraints: OptionConstraints {
            minimum: Some(0.0),
            maximum: Some(4.0),
            allowed_values: Vec::new(),
        },
        risk: RiskLevel::Low,
        status,
        evidence: evidence(runtime_verified),
    }
}

fn catalog_with(option: CatalogOption) -> Catalog {
    Catalog {
        options: BTreeMap::from([(option.key.clone(), option)]),
        presets: Vec::new(),
    }
}

fn preset(key: &str) -> BuiltinPreset {
    BuiltinPreset {
        id: "performance".into(),
        name: BilingualText {
            en: "Performance".into(),
            ko: "성능".into(),
        },
        description: BilingualText {
            en: "Evidence-gated profile.".into(),
            ko: "검증 근거를 요구하는 프로필입니다.".into(),
        },
        changes: vec![ProfileIniChange {
            section: "SystemSettings".into(),
            key: key.into(),
            value: Some("1".into()),
        }],
    }
}

fn profile(id: &str, name: &str, key: &str) -> CustomProfile {
    CustomProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        id: id.into(),
        name: name.into(),
        patch: ProfilePatch {
            schema_version: PROFILE_SCHEMA_VERSION,
            managed_ini: vec![ProfileIniChange {
                section: "SystemSettings".into(),
                key: key.into(),
                value: Some("2".into()),
            }],
            process: ProcessProfile::default(),
        },
    }
}

#[test]
fn embedded_catalog_has_bilingual_evidence_and_distinct_verification_flags() {
    let catalog = Catalog::load_embedded();

    assert!(
        catalog.is_ok(),
        "embedded catalog should be valid: {catalog:?}"
    );
    let catalog = catalog.unwrap();
    assert!(!catalog.options.is_empty());
    for item in catalog.options.values() {
        assert!(!item.description.en.trim().is_empty());
        assert!(!item.description.ko.trim().is_empty());
        assert!(item.evidence.source_url.starts_with("https://"));
        assert!(!item.evidence.tested_game_version.trim().is_empty());
        assert!(!item.evidence.tested_date.trim().is_empty());
        assert!(!item.evidence.tested_hardware.trim().is_empty());
    }
    assert!(catalog
        .options
        .values()
        .any(|item| { item.evidence.present_in_file && !item.evidence.runtime_verified }));
}

#[test]
fn catalog_rejects_unsupported_schema_versions() {
    let options = r#"{"schema_version":99,"options":[]}"#;
    let presets = r#"{"schema_version":1,"presets":[]}"#;

    assert!(matches!(
        Catalog::from_json(options, presets),
        Err(CatalogError::UnsupportedSchemaVersion(99))
    ));
}

#[test]
fn catalog_rejects_incomplete_evidence_metadata() {
    let options = r#"{
      "schema_version": 1,
      "options": [{
        "section": "SystemSettings", "key": "r.Test",
        "description": {"en": "English", "ko": "한국어"},
        "value_type": "integer",
        "constraints": {"minimum": 0, "maximum": 4, "allowed_values": []},
        "risk": "low", "status": "community_reported",
        "evidence": {
          "source_url": "", "tested_game_version": "", "tested_date": "",
          "tested_hardware": "", "present_in_file": true, "runtime_verified": false
        }
      }]
    }"#;
    let presets = r#"{"schema_version":1,"presets":[]}"#;

    assert!(matches!(
        Catalog::from_json(options, presets),
        Err(CatalogError::InvalidOption { .. })
    ));
}

#[test]
fn catalog_rejects_malformed_source_urls_and_test_dates() {
    let presets = r#"{"schema_version":1,"presets":[]}"#;
    let mut item =
        serde_json::to_value(option("r.Metadata", OptionStatus::CommunityReported, false)).unwrap();
    item["evidence"]["source_url"] = serde_json::json!("https://");
    let options = serde_json::json!({"schema_version": 1, "options": [item]});
    assert!(matches!(
        Catalog::from_json(&options.to_string(), presets),
        Err(CatalogError::InvalidOption { .. })
    ));

    let mut item =
        serde_json::to_value(option("r.Metadata", OptionStatus::CommunityReported, false)).unwrap();
    item["evidence"]["tested_date"] = serde_json::json!("July 14, 2026");
    let options = serde_json::json!({"schema_version": 1, "options": [item]});
    assert!(matches!(
        Catalog::from_json(&options.to_string(), presets),
        Err(CatalogError::InvalidOption { .. })
    ));
}

#[test]
fn option_validation_enforces_type_range_and_allowed_values() {
    let integer = option("r.Integer", OptionStatus::CommunityReported, false);
    assert_eq!(validate_option(&integer, "2"), Ok(()));
    assert!(validate_option(&integer, "5").is_err());
    assert!(validate_option(&integer, "2.5").is_err());

    let mut boolean = option("r.Boolean", OptionStatus::CommunityReported, false);
    boolean.value_type = OptionValueType::Boolean;
    boolean.constraints = OptionConstraints::default();
    assert_eq!(validate_option(&boolean, "true"), Ok(()));
    assert!(validate_option(&boolean, "yes").is_err());

    let mut text = option("r.Text", OptionStatus::Experimental, false);
    text.value_type = OptionValueType::Text;
    text.constraints = OptionConstraints {
        minimum: None,
        maximum: None,
        allowed_values: vec!["low".into(), "high".into()],
    };
    assert_eq!(validate_option(&text, "high"), Ok(()));
    assert!(validate_option(&text, "medium").is_err());
}

#[test]
fn only_runtime_verified_options_can_enter_builtin_presets() {
    for status in [
        OptionStatus::CommunityReported,
        OptionStatus::Experimental,
        OptionStatus::Ignored,
        OptionStatus::Regressed,
    ] {
        let key = "r.Unverified";
        let catalog = catalog_with(option(key, status, false));
        assert_eq!(
            validate_builtin(&catalog, &preset(key)),
            Err(CatalogError::UnverifiedPresetOption(key.into()))
        );
    }

    let key = "r.PresenceOnly";
    let catalog = catalog_with(option(key, OptionStatus::Verified, false));
    assert_eq!(
        validate_builtin(&catalog, &preset(key)),
        Err(CatalogError::UnverifiedPresetOption(key.into()))
    );

    let key = "r.Verified";
    let catalog = catalog_with(option(key, OptionStatus::Verified, true));
    assert_eq!(validate_builtin(&catalog, &preset(key)), Ok(()));
}

#[test]
fn builtin_profiles_exist_and_unverified_non_default_profiles_are_conservative() {
    let catalog = Catalog::load_embedded().unwrap();
    let ids = catalog
        .presets
        .iter()
        .map(|preset| preset.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec!["vanilla", "balanced", "performance", "visual-quality"]
    );
    for preset in &catalog.presets {
        assert!(validate_builtin(&catalog, preset).is_ok());
        assert!(!preset.description.en.trim().is_empty());
        assert!(!preset.description.ko.trim().is_empty());
    }
    assert!(catalog
        .presets
        .iter()
        .filter(|preset| preset.id != "vanilla")
        .all(|preset| preset.changes.is_empty()));
}

#[test]
fn schemas_are_valid_json_and_reject_unknown_safety_fields() {
    let catalog_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../catalog/schema");
    for name in [
        "options.schema.json",
        "presets.schema.json",
        "profile.schema.json",
    ] {
        let path = catalog_root.join(name);
        assert!(path.is_file(), "missing schema: {}", path.display());
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["additionalProperties"], false);
    }

    let json = serde_json::json!({
        "schema_version": PROFILE_SCHEMA_VERSION,
        "id": "unsafe",
        "name": "Unsafe",
        "patch": {
            "schema_version": PROFILE_SCHEMA_VERSION,
            "managed_ini": [],
            "process": {
                "cpu_selection": {"mode": "all"},
                "priority": "normal",
                "auto_select_realtime": true
            }
        }
    });
    assert!(serde_json::from_value::<CustomProfile>(json).is_err());
}

#[test]
fn cpu_selections_and_all_priority_classes_round_trip_without_engine_ini_keys() {
    let selections = [
        CpuSelection::All,
        CpuSelection::PreferPerformance,
        CpuSelection::ManualCpuSets { ids: vec![0, 3] },
        CpuSelection::HardAffinity {
            group: 1,
            mask: 0b1010,
        },
    ];
    let priorities = [
        PriorityClass::Idle,
        PriorityClass::BelowNormal,
        PriorityClass::Normal,
        PriorityClass::AboveNormal,
        PriorityClass::High,
        PriorityClass::Realtime,
    ];

    assert_eq!(ProcessProfile::default().priority, PriorityClass::Normal);
    for cpu_selection in selections {
        for priority in priorities {
            let process = ProcessProfile {
                cpu_selection: cpu_selection.clone(),
                priority,
            };
            let encoded = serde_json::to_string(&process).unwrap();
            assert!(!encoded.contains("MaxCPUCores"));
            assert!(!encoded.contains("AsyncLoadingThreadPriority"));
            assert_eq!(
                serde_json::from_str::<ProcessProfile>(&encoded).unwrap(),
                process
            );
        }
    }
}

#[test]
fn profile_patch_converts_managed_ini_entries_without_mixing_process_settings() {
    let patch = ProfilePatch {
        schema_version: PROFILE_SCHEMA_VERSION,
        managed_ini: vec![
            ProfileIniChange {
                section: "SystemSettings".into(),
                key: "r.Known".into(),
                value: Some("2".into()),
            },
            ProfileIniChange {
                section: "SystemSettings".into(),
                key: "r.Removed".into(),
                value: None,
            },
        ],
        process: ProcessProfile {
            cpu_selection: CpuSelection::ManualCpuSets { ids: vec![1, 3] },
            priority: PriorityClass::High,
        },
    };
    let document = IniDocument::parse(b"[SystemSettings]\r\nr.Known=1\r\nr.Removed=1\r\n").unwrap();

    let preview = document.merge(&patch.managed_changes()).unwrap();

    assert_eq!(preview.after, b"[SystemSettings]\r\nr.Known=2\r\n".to_vec());
    assert_eq!(preview.semantic_changes.len(), 2);
}

#[test]
fn profile_store_saves_lists_gets_renames_and_clones_profiles() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = catalog_with(option("r.Known", OptionStatus::CommunityReported, false));
    let saved = store
        .save(&profile("daily", "Daily", "r.Known"), &catalog)
        .unwrap();

    assert_eq!(store.get("daily").unwrap(), saved);
    assert_eq!(store.list().unwrap(), vec![saved.clone()]);

    let renamed = store.rename("daily", "Every day").unwrap();
    assert_eq!(renamed.name, "Every day");
    let cloned = store
        .clone_profile("daily", "daily-copy", "Daily copy")
        .unwrap();
    assert_eq!(cloned.id, "daily-copy");
    assert_eq!(store.list().unwrap().len(), 2);
}

#[test]
fn profile_store_exports_and_imports_only_valid_contained_json() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = catalog_with(option("r.Known", OptionStatus::CommunityReported, false));
    store
        .save(&profile("portable", "Portable", "r.Known"), &catalog)
        .unwrap();

    let exported = store.export("portable", "portable.json").unwrap();
    assert!(exported.starts_with(directory.path().join("profiles/exports")));
    assert!(exported.is_file());
    assert!(matches!(
        store.export("portable", "../escape.json"),
        Err(ProfileError::InvalidFileName(_))
    ));

    let mut imported: CustomProfile =
        serde_json::from_slice(&std::fs::read(&exported).unwrap()).unwrap();
    imported.id = "imported".into();
    imported.name = "Imported".into();
    let import_dir = directory.path().join("profiles/imports");
    std::fs::create_dir_all(&import_dir).unwrap();
    let import_path = import_dir.join("imported.json");
    std::fs::write(&import_path, serde_json::to_vec(&imported).unwrap()).unwrap();
    assert_eq!(store.import(&import_path, &catalog).unwrap(), imported);

    let outside = directory.path().join("outside.json");
    std::fs::write(&outside, serde_json::to_vec(&imported).unwrap()).unwrap();
    assert!(matches!(
        store.import(&outside, &catalog),
        Err(ProfileError::PathOutsideStore(_))
    ));
}

#[test]
fn profile_store_rejects_schema_versions_unknown_keys_and_unsafe_identifiers() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = catalog_with(option("r.Known", OptionStatus::CommunityReported, false));

    let mut unsupported = profile("unsupported", "Unsupported", "r.Known");
    unsupported.schema_version = 99;
    assert!(matches!(
        store.save(&unsupported, &catalog),
        Err(ProfileError::UnsupportedSchemaVersion(99))
    ));

    assert!(matches!(
        store.save(&profile("unknown", "Unknown", "r.Unknown"), &catalog),
        Err(ProfileError::UnknownProfileKey(key)) if key == "r.Unknown"
    ));
    assert!(matches!(
        store.save(&profile("../escape", "Escape", "r.Known"), &catalog),
        Err(ProfileError::InvalidName(_))
    ));
}

#[cfg(unix)]
#[test]
fn profile_store_rejects_symlinked_managed_directories() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let profile_root = directory.path().join("profiles");
    std::fs::create_dir_all(&profile_root).unwrap();
    symlink(outside.path(), profile_root.join("custom")).unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = catalog_with(option("r.Known", OptionStatus::CommunityReported, false));

    assert!(matches!(
        store.save(&profile("escaped", "Escaped", "r.Known"), &catalog),
        Err(ProfileError::PathOutsideStore(_))
    ));
    assert!(matches!(
        store.list(),
        Err(ProfileError::PathOutsideStore(_))
    ));
    assert!(!outside.path().join("escaped.json").exists());
}
