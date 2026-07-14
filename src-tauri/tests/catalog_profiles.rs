use std::{collections::BTreeMap, path::PathBuf};

use wuwa_ini_tool_lib::{
    catalog::{
        validate_builtin, validate_option, BilingualText, BuiltinPreset, Catalog, CatalogError,
        CatalogOption, CpuPresetMode, OptionConstraints, OptionEvidence, OptionStatus,
        OptionValueType, ProfileIniChange, RiskLevel, RuntimeObservation,
    },
    ini_document::IniDocument,
    profile_store::{
        CpuSelection, CustomEntryProvenance, CustomIniEntry, CustomProfile, ImportWarning,
        PriorityClass, ProcessProfile, ProfileError, ProfilePatch, ProfileShareEnvelope,
        ProfileStore, MAX_SHARE_BYTES, PROFILE_SCHEMA_VERSION,
    },
};

fn evidence(runtime_verified: bool) -> OptionEvidence {
    OptionEvidence {
        source_url: "https://dev.epicgames.com/documentation/".into(),
        tested_game_version: Some("2.5".into()),
        tested_date: Some("2026-07-14".into()),
        tested_hardware: Some("Windows 11 x64; Ryzen 7; 32 GB RAM".into()),
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
        cpu_presets: Vec::new(),
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
        revision: 0,
        patch: ProfilePatch {
            schema_version: PROFILE_SCHEMA_VERSION,
            managed_ini: vec![ProfileIniChange {
                section: "SystemSettings".into(),
                key: key.into(),
                value: Some("100".into()),
            }],
            custom_ini_entries: Vec::new(),
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
        assert_eq!(item.evidence.tested_game_version, None);
        assert_eq!(item.evidence.tested_date, None);
        assert_eq!(item.evidence.tested_hardware, None);
    }
    assert!(catalog
        .options
        .values()
        .all(|item| !item.evidence.runtime_verified));
}

#[test]
fn catalog_rejects_unsupported_schema_versions() {
    let options = r#"{"schema_version":99,"options":[]}"#;
    let presets = r#"{"schema_version":1,"presets":[],"cpu_presets":[]}"#;

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
          "source_url": "", "tested_game_version": null, "tested_date": null,
          "tested_hardware": null, "runtime_verified": false
        }
      }]
    }"#;
    let presets = r#"{"schema_version":1,"presets":[],"cpu_presets":[]}"#;

    assert!(matches!(
        Catalog::from_json(options, presets),
        Err(CatalogError::InvalidOption { .. })
    ));
}

#[test]
fn catalog_rejects_malformed_source_urls_and_test_dates() {
    let presets = r#"{"schema_version":1,"presets":[],"cpu_presets":[]}"#;
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
fn source_reviewed_streaming_options_are_warning_gated_and_never_builtin() {
    let catalog = Catalog::load_embedded().unwrap();
    let expected = [
        ("r.Streaming.PoolSize", OptionStatus::CommunityReported),
        ("r.ParallelFrustumCull", OptionStatus::CommunityReported),
        ("r.ParallelOcclusionCull", OptionStatus::CommunityReported),
        ("r.Streaming.FullyLoadUsedTextures", OptionStatus::Regressed),
        ("r.Streaming.HLODStrategy", OptionStatus::Regressed),
        (
            "r.Streaming.UsingKuroStreamingPriority",
            OptionStatus::Experimental,
        ),
    ];

    for (key, status) in expected {
        let option = catalog
            .options
            .get(key)
            .unwrap_or_else(|| panic!("missing source-reviewed option: {key}"));
        assert_eq!(option.status, status);
        assert!(!option.evidence.runtime_verified);
        assert_eq!(option.evidence.tested_game_version, None);
        assert!(matches!(option.risk, RiskLevel::Medium | RiskLevel::High));
        assert!(catalog.presets.iter().all(|preset| preset
            .changes
            .iter()
            .filter(|change| change.key == key)
            .all(|change| preset.id == "vanilla" && change.value.is_none())));
    }
}

#[test]
fn schemas_are_valid_json_and_reject_unknown_safety_fields() {
    let catalog_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../catalog/schema");
    for name in [
        "options.schema.json",
        "presets.schema.json",
        "profile.schema.json",
        "share.schema.json",
    ] {
        let path = catalog_root.join(name);
        assert!(path.is_file(), "missing schema: {}", path.display());
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["additionalProperties"], false);
    }

    let options_schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(catalog_root.join("options.schema.json")).unwrap(),
    )
    .unwrap();
    let evidence = &options_schema["$defs"]["evidence"]["properties"];
    assert!(evidence.get("present_in_file").is_none());
    assert_eq!(
        evidence["tested_date"]["type"],
        serde_json::json!(["string", "null"])
    );

    let profile_schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(catalog_root.join("profile.schema.json")).unwrap(),
    )
    .unwrap();
    assert!(profile_schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field == "revision"));
    assert!(profile_schema["$defs"]["patch"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field == "custom_ini_entries"));

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
    let known = option("r.Known", OptionStatus::Experimental, false);
    let removed = option("r.Removed", OptionStatus::Experimental, false);
    let catalog = Catalog {
        options: BTreeMap::from([(known.key.clone(), known), (removed.key.clone(), removed)]),
        presets: Vec::new(),
        cpu_presets: Vec::new(),
    };
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
        custom_ini_entries: Vec::new(),
        process: ProcessProfile {
            cpu_selection: CpuSelection::ManualCpuSets { ids: vec![1, 3] },
            priority: PriorityClass::High,
        },
    };
    let document = IniDocument::parse(b"[SystemSettings]\r\nr.Known=1\r\nr.Removed=1\r\n").unwrap();

    let changes = patch.validated_managed_changes(&catalog).unwrap();
    let preview = document.merge(&changes).unwrap();

    assert_eq!(preview.after, b"[SystemSettings]\r\nr.Known=2\r\n".to_vec());
    assert_eq!(preview.semantic_changes.len(), 2);
}

#[test]
fn unsaved_profile_patches_cannot_bypass_full_managed_change_validation() {
    let catalog = Catalog::load_embedded().unwrap();
    let valid = profile("candidate", "Candidate", "r.ScreenPercentage").patch;
    assert_eq!(
        valid.validated_managed_changes(&catalog).unwrap(),
        vec![wuwa_ini_tool_lib::ini_document::ManagedChange::set(
            "SystemSettings",
            "r.ScreenPercentage",
            "100",
        )]
    );

    let mut unknown = valid.clone();
    unknown.managed_ini[0].key = "r.Unknown".into();
    assert!(matches!(
        unknown.validated_managed_changes(&catalog),
        Err(ProfileError::UnknownProfileKey(key)) if key == "r.Unknown"
    ));

    let mut out_of_range = valid.clone();
    out_of_range.managed_ini[0].value = Some("999".into());
    assert!(matches!(
        out_of_range.validated_managed_changes(&catalog),
        Err(ProfileError::InvalidProfile("invalid_option_value"))
    ));

    let mut injected = valid.clone();
    injected.managed_ini[0].value = Some("100\nr.Injected=1".into());
    assert!(matches!(
        injected.validated_managed_changes(&catalog),
        Err(ProfileError::InvalidProfile("invalid_option_value"))
    ));

    let mut spoofed = valid;
    let mut custom = custom_entry("SystemSettings", "r.Custom", "1");
    custom.runtime_verified = true;
    spoofed.custom_ini_entries.push(custom);
    assert!(matches!(
        spoofed.validated_managed_changes(&catalog),
        Err(ProfileError::InvalidProfile("custom_entry_provenance"))
    ));

    let invalid_provenance = serde_json::json!({
        "schema_version": PROFILE_SCHEMA_VERSION,
        "managed_ini": [],
        "custom_ini_entries": [{
            "section": "SystemSettings",
            "key": "r.Custom",
            "value": "1",
            "provenance": "catalog",
            "runtime_verified": false
        }],
        "process": {
            "cpu_selection": {"mode": "all"},
            "priority": "normal"
        }
    });
    assert!(serde_json::from_value::<ProfilePatch>(invalid_provenance).is_err());
}

#[test]
fn profile_store_saves_lists_gets_renames_and_clones_profiles() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();
    let mut candidate = profile("daily", "Daily", "r.ScreenPercentage");
    candidate.patch.custom_ini_entries = vec![custom_entry("System", "r.Daily", "1")];
    let saved = store.save(&candidate, &catalog).unwrap();

    assert_eq!(store.get("daily").unwrap(), saved);
    assert_eq!(store.list().unwrap(), vec![saved.clone()]);

    let renamed = store.rename("daily", "Every day").unwrap();
    assert_eq!(renamed.name, "Every day");
    assert_eq!(
        renamed.patch.custom_ini_entries,
        saved.patch.custom_ini_entries
    );
    let cloned = store
        .clone_profile("daily", "daily-copy", "Daily copy")
        .unwrap();
    assert_eq!(cloned.id, "daily-copy");
    assert_eq!(
        cloned.patch.custom_ini_entries,
        saved.patch.custom_ini_entries
    );
    assert_eq!(store.list().unwrap().len(), 2);
}

#[test]
fn portable_export_and_external_import_preview_do_not_persist_or_leak_internal_fields() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();
    let mut candidate = profile("portable", "Portable", "r.ScreenPercentage");
    candidate.patch.custom_ini_entries = vec![custom_entry("System", "r.Portable", "1")];
    store.save(&candidate, &catalog).unwrap();

    let exported = store.export("portable").unwrap();
    assert_eq!(exported.suggested_file_name, "portable.wuwaprofile.json");
    let envelope: ProfileShareEnvelope = serde_json::from_slice(&exported.bytes).unwrap();
    let json = String::from_utf8(exported.bytes.clone()).unwrap();
    assert_eq!(envelope.profile.name, "Portable");
    assert_eq!(envelope.creating_app_version, "1.0.0");
    assert!(!envelope.exported_at.is_empty());
    assert_eq!(envelope.profile.patch.custom_ini_entries.len(), 1);
    assert!(!json.contains("\"id\""));
    assert!(!json.contains("revision"));
    assert!(!json.contains("backup"));
    assert!(!json.contains(directory.path().to_string_lossy().as_ref()));

    let outside = directory.path().join("outside.json");
    std::fs::write(&outside, &exported.bytes).unwrap();
    let preview = store.import(&outside, &catalog).unwrap();
    assert_eq!(preview.display_name, "Portable");
    assert_eq!(preview.patch.custom_ini_entries.len(), 1);
    assert!(!catalog.options.contains_key("r.Portable"));
    assert_eq!(store.list().unwrap().len(), 1, "import must only preview");

    let imported = store
        .save_import(&preview, "imported", "Imported", &catalog)
        .unwrap();
    assert_eq!(imported.id, "imported");
    assert_eq!(imported.patch.custom_ini_entries.len(), 1);
    assert_eq!(store.list().unwrap().len(), 2);
}

#[test]
fn profile_store_rejects_schema_versions_unknown_keys_and_unsafe_identifiers() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();

    let mut unsupported = profile("unsupported", "Unsupported", "r.ScreenPercentage");
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
        store.save(
            &profile("../escape", "Escape", "r.ScreenPercentage"),
            &catalog
        ),
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
    let catalog = Catalog::load_embedded().unwrap();

    assert!(matches!(
        store.save(
            &profile("escaped", "Escaped", "r.ScreenPercentage"),
            &catalog
        ),
        Err(ProfileError::PathOutsideStore(_))
    ));
    assert!(matches!(
        store.list(),
        Err(ProfileError::PathOutsideStore(_))
    ));
    assert!(!outside.path().join("escaped.json").exists());
}

fn custom_entry(section: &str, key: &str, value: &str) -> CustomIniEntry {
    CustomIniEntry {
        section: section.into(),
        key: key.into(),
        value: value.into(),
        provenance: CustomEntryProvenance::Custom,
        runtime_verified: false,
    }
}

#[test]
fn custom_ini_entries_round_trip_and_merge_as_unverified_ini_data() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();
    let mut candidate = profile("custom", "Custom", "r.ScreenPercentage");
    candidate.patch.custom_ini_entries =
        vec![custom_entry("SystemSettings", "r.IniruFPSOpti", "1")];

    let saved = store.save(&candidate, &catalog).unwrap();
    let loaded = store.get("custom").unwrap();
    assert_eq!(
        loaded.patch.custom_ini_entries,
        saved.patch.custom_ini_entries
    );
    assert_eq!(
        loaded.patch.custom_ini_entries[0].provenance,
        CustomEntryProvenance::Custom
    );
    assert!(!loaded.patch.custom_ini_entries[0].runtime_verified);

    let document = IniDocument::parse(b"[SystemSettings]\r\nr.ScreenPercentage=100\r\n").unwrap();
    let changes = loaded.patch.validated_managed_changes(&catalog).unwrap();
    let preview = document.merge(&changes).unwrap();
    let rendered = String::from_utf8(preview.after).unwrap();
    assert!(rendered.contains("r.IniruFPSOpti=1"));
}

#[test]
fn custom_ini_syntax_and_case_insensitive_duplicates_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();
    let invalid = [
        custom_entry("", "r.Key", "1"),
        custom_entry("   ", "r.Key", "1"),
        custom_entry("[System]", "r.Key", "1"),
        custom_entry("System\nSettings", "r.Key", "1"),
        custom_entry("System", "", "1"),
        custom_entry("System", "   ", "1"),
        custom_entry("System", "r.Key=2", "1"),
        custom_entry(" System", "r.Key", "1"),
        custom_entry("System ", "r.Key", "1"),
        custom_entry("System", " r.Key", "1"),
        custom_entry("System", "r.Key ", "1"),
        custom_entry("System", ";r.Key", "1"),
        custom_entry("System", "#r.Key", "1"),
        custom_entry("System", "r.Key\0", "1"),
        custom_entry("System", "r.Key", "one\ntwo"),
        custom_entry("System", "r.Key", " value"),
        custom_entry("System", "r.Key", "value "),
        custom_entry("System", "r.Key", ";comment"),
        custom_entry("System", "r.Key", "#comment"),
        custom_entry("System", "r.Key", "value ;comment"),
        custom_entry("System", "r.Key", "value #comment"),
    ];
    for (index, entry) in invalid.into_iter().enumerate() {
        let mut candidate = profile(&format!("invalid-{index}"), "Invalid", "r.ScreenPercentage");
        candidate.patch.custom_ini_entries = vec![entry];
        assert!(matches!(
            store.save(&candidate, &catalog),
            Err(ProfileError::InvalidProfile(_))
        ));
    }

    let mut duplicate = profile("duplicate", "Duplicate", "r.ScreenPercentage");
    duplicate.patch.custom_ini_entries = vec![
        custom_entry("SystemSettings", "r.Custom", "1"),
        custom_entry("systemsettings", "R.CUSTOM", "2"),
    ];
    assert!(matches!(
        store.save(&duplicate, &catalog),
        Err(ProfileError::InvalidProfile(_))
    ));
}

#[test]
fn validated_custom_changes_are_parser_idempotent_without_overrejecting_punctuation() {
    let catalog = Catalog::load_embedded().unwrap();
    let patch = ProfilePatch {
        schema_version: PROFILE_SCHEMA_VERSION,
        managed_ini: Vec::new(),
        custom_ini_entries: vec![
            custom_entry("SystemSettings", "r.Custom-Name:V2", "alpha;beta#gamma"),
            custom_entry("ConsoleVariables", "custom.path", "C:/Games/WuWa"),
        ],
        process: ProcessProfile::default(),
    };
    let changes = patch.validated_managed_changes(&catalog).unwrap();
    assert_eq!(
        changes,
        vec![
            wuwa_ini_tool_lib::ini_document::ManagedChange::set(
                "SystemSettings",
                "r.Custom-Name:V2",
                "alpha;beta#gamma",
            ),
            wuwa_ini_tool_lib::ini_document::ManagedChange::set(
                "ConsoleVariables",
                "custom.path",
                "C:/Games/WuWa",
            ),
        ]
    );
    let first = IniDocument::parse(b"; preserved\r\n")
        .unwrap()
        .merge(&changes)
        .unwrap();
    let second = IniDocument::parse(&first.after)
        .unwrap()
        .merge(&changes)
        .unwrap();

    assert_eq!(second.after, first.after);
    assert!(second.semantic_changes.is_empty());
}

#[test]
fn community_reported_catalog_status_requires_non_upstream_community_evidence() {
    let embedded = Catalog::load_embedded().unwrap();
    assert!(embedded.options.values().all(|item| {
        item.status != OptionStatus::CommunityReported
            || !item.evidence.source_url.contains("dev.epicgames.com")
    }));
}

#[test]
fn text_and_custom_values_reject_line_injection_and_enforce_limits() {
    let mut text = option("r.Text", OptionStatus::Experimental, false);
    text.value_type = OptionValueType::Text;
    text.constraints = OptionConstraints::default();
    for value in [
        "line\nnext",
        "line\rnext",
        "nul\0next",
        "tab\tnext",
        "value\n",
    ] {
        assert!(validate_option(&text, value).is_err());
    }

    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();
    let mut candidate = profile("large-value", "Large", "r.ScreenPercentage");
    candidate.patch.custom_ini_entries = vec![custom_entry("System", "r.Large", &"x".repeat(8193))];
    assert!(matches!(
        store.save(&candidate, &catalog),
        Err(ProfileError::InvalidProfile(_))
    ));
}

#[test]
fn preset_deletes_are_evidence_gated_and_vanilla_is_exact() {
    let first = option("r.First", OptionStatus::CommunityReported, false);
    let second = option("r.Second", OptionStatus::Experimental, false);
    let catalog = Catalog {
        options: BTreeMap::from([(first.key.clone(), first), (second.key.clone(), second)]),
        presets: Vec::new(),
        cpu_presets: Vec::new(),
    };
    let mut non_vanilla = preset("r.First");
    non_vanilla.changes[0].value = None;
    assert_eq!(
        validate_builtin(&catalog, &non_vanilla),
        Err(CatalogError::UnverifiedPresetOption("r.First".into()))
    );

    let mut vanilla = non_vanilla.clone();
    vanilla.id = "vanilla".into();
    assert!(validate_builtin(&catalog, &vanilla).is_err(), "missing key");
    vanilla.changes.push(ProfileIniChange {
        section: "SystemSettings".into(),
        key: "r.Second".into(),
        value: None,
    });
    assert_eq!(validate_builtin(&catalog, &vanilla), Ok(()));
    vanilla.changes.push(vanilla.changes[0].clone());
    assert!(
        validate_builtin(&catalog, &vanilla).is_err(),
        "duplicate key"
    );

    let verified = option("r.Verified", OptionStatus::Verified, true);
    let verified_catalog = catalog_with(verified);
    let mut invalid_vanilla = preset("r.Verified");
    invalid_vanilla.id = "vanilla".into();
    assert!(
        validate_builtin(&verified_catalog, &invalid_vanilla).is_err(),
        "Vanilla must delete rather than set every managed key"
    );
}

#[test]
fn cpu_builtins_are_safe_bilingual_and_never_auto_select_elevated_priority() {
    let catalog = Catalog::load_embedded().unwrap();
    let ids = catalog
        .cpu_presets
        .iter()
        .map(|preset| (preset.id.as_str(), preset.mode))
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            ("system-default", CpuPresetMode::All),
            ("prefer-performance", CpuPresetMode::PreferPerformance),
            ("custom", CpuPresetMode::Custom),
        ]
    );
    for preset in &catalog.cpu_presets {
        assert!(!preset.name.en.is_empty() && !preset.name.ko.is_empty());
        assert!(!preset.description.en.is_empty() && !preset.description.ko.is_empty());
        assert_eq!(preset.default_priority, "normal");
        assert!(!preset.auto_select_elevated);
    }
}

#[test]
fn evidence_uses_nullable_untested_fields_and_runtime_observation_is_separate() {
    let catalog = Catalog::load_embedded().unwrap();
    assert!(catalog.options.values().all(|item| {
        item.evidence.tested_game_version.is_none()
            && item.evidence.tested_date.is_none()
            && item.evidence.tested_hardware.is_none()
            && !item.evidence.runtime_verified
    }));
    let observation = RuntimeObservation {
        present_in_file: true,
        runtime_verified: false,
    };
    assert_eq!(
        serde_json::from_str::<RuntimeObservation>(&serde_json::to_string(&observation).unwrap())
            .unwrap(),
        observation
    );
    assert!(
        !serde_json::to_string(&catalog.options.values().next().unwrap().evidence)
            .unwrap()
            .contains("present_in_file")
    );
}

#[test]
fn share_import_is_bounded_and_device_specific_cpu_is_explicitly_sanitized() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();
    let mut candidate = profile("device", "Device", "r.ScreenPercentage");
    candidate.patch.process = ProcessProfile {
        cpu_selection: CpuSelection::ManualCpuSets { ids: vec![1, 9] },
        priority: PriorityClass::High,
    };
    store.save(&candidate, &catalog).unwrap();
    let exported = store.export("device").unwrap();
    let text = String::from_utf8(exported.bytes.clone()).unwrap();
    assert!(!text.contains("manual_cpu_sets"));
    assert!(!text.contains("\"ids\""));

    let preview = store.import_bytes(&exported.bytes, &catalog).unwrap();
    assert_eq!(preview.patch.process.cpu_selection, CpuSelection::All);
    assert!(preview
        .warnings
        .contains(&ImportWarning::DeviceSpecificCpuReset));
    assert!(preview.warnings.contains(&ImportWarning::ElevatedPriority));

    let oversized = vec![b' '; MAX_SHARE_BYTES as usize + 1];
    assert!(matches!(
        store.import_bytes(&oversized, &catalog),
        Err(ProfileError::ShareTooLarge { .. })
    ));
    let outside = directory.path().join("oversized.wuwaprofile.json");
    std::fs::write(&outside, oversized).unwrap();
    assert!(matches!(
        store.import(&outside, &catalog),
        Err(ProfileError::ShareTooLarge { .. })
    ));
}

#[test]
fn share_array_limits_and_import_collisions_preserve_existing_profiles() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();
    store
        .save(&profile("source", "Source", "r.ScreenPercentage"), &catalog)
        .unwrap();
    let exported = store.export("source").unwrap();
    let mut envelope: serde_json::Value = serde_json::from_slice(&exported.bytes).unwrap();
    envelope["profile"]["patch"]["managed_ini"] = serde_json::Value::Array(
        (0..257)
            .map(|_| {
                serde_json::json!({
                    "section": "SystemSettings",
                    "key": "r.ScreenPercentage",
                    "value": "100"
                })
            })
            .collect(),
    );
    assert!(matches!(
        store.import_bytes(&serde_json::to_vec(&envelope).unwrap(), &catalog),
        Err(ProfileError::InvalidProfile(_))
    ));

    let mut warnings: serde_json::Value = serde_json::from_slice(&exported.bytes).unwrap();
    warnings["portability_warnings"] =
        serde_json::json!(
            std::iter::repeat_n("device_specific_cpu_excluded", 17).collect::<Vec<_>>()
        );
    assert!(matches!(
        store.import_bytes(&serde_json::to_vec(&warnings).unwrap(), &catalog),
        Err(ProfileError::InvalidProfile(_))
    ));

    let preview = store.import_bytes(&exported.bytes, &catalog).unwrap();
    let before = store.get("source").unwrap();
    assert!(matches!(
        store.save_import(&preview, "source", "Replacement", &catalog),
        Err(ProfileError::ProfileAlreadyExists(id)) if id == "source"
    ));
    assert_eq!(store.get("source").unwrap(), before);
    assert!(matches!(
        store.save_import(&preview, "different-id", "Source", &catalog),
        Err(ProfileError::ProfileNameAlreadyExists(name)) if name == "Source"
    ));
    assert_eq!(store.list().unwrap().len(), 1);
}

#[test]
fn reads_revalidate_payload_identity_and_corruption_is_not_silently_skipped() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();
    let saved = store
        .save(
            &profile("identity", "Identity", "r.ScreenPercentage"),
            &catalog,
        )
        .unwrap();
    let path = directory.path().join("profiles/custom/identity.json");
    let mut mismatched = saved.clone();
    mismatched.id = "different".into();
    std::fs::write(&path, serde_json::to_vec(&mismatched).unwrap()).unwrap();
    assert!(matches!(
        store.get("identity"),
        Err(ProfileError::InvalidProfile(_))
    ));
    assert!(store.list().is_err());

    let mut invalid_value = saved.clone();
    invalid_value.patch.managed_ini[0].value = Some("1".into());
    std::fs::write(&path, serde_json::to_vec(&invalid_value).unwrap()).unwrap();
    assert!(matches!(
        store.get("identity"),
        Err(ProfileError::InvalidProfile(_))
    ));

    let mut invalid_process = saved;
    invalid_process.patch.process.cpu_selection = CpuSelection::ManualCpuSets { ids: vec![] };
    std::fs::write(&path, serde_json::to_vec(&invalid_process).unwrap()).unwrap();
    assert!(matches!(
        store.get("identity"),
        Err(ProfileError::InvalidProfile(_))
    ));

    let mut invalid_revision = invalid_process;
    invalid_revision.patch.process = ProcessProfile::default();
    invalid_revision.revision = 0;
    std::fs::write(&path, serde_json::to_vec(&invalid_revision).unwrap()).unwrap();
    assert!(matches!(
        store.get("identity"),
        Err(ProfileError::InvalidProfile(_))
    ));

    std::fs::write(&path, b"{ corrupt").unwrap();
    assert!(matches!(store.list(), Err(ProfileError::Json(_))));
}

#[test]
fn revisions_and_no_clobber_creation_prevent_lost_updates() {
    use std::sync::{Arc, Barrier};

    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();
    let created = store
        .save(
            &profile("revision", "Revision", "r.ScreenPercentage"),
            &catalog,
        )
        .unwrap();
    assert_eq!(created.revision, 1);
    let first = store.save(&created, &catalog).unwrap();
    assert_eq!(first.revision, 2);
    assert!(matches!(
        store.save(&created, &catalog),
        Err(ProfileError::RevisionConflict {
            expected: 1,
            actual: 2
        })
    ));

    let race_store = Arc::new(ProfileStore::new(directory.path()));
    let race_catalog = Arc::new(catalog);
    let barrier = Arc::new(Barrier::new(3));
    let handles = (0..2)
        .map(|_| {
            let store = Arc::clone(&race_store);
            let catalog = Arc::clone(&race_catalog);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let candidate = profile("race", "Race", "r.ScreenPercentage");
                barrier.wait();
                store.save(&candidate, &catalog)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
}

#[test]
fn windows_reserved_ids_are_rejected_and_display_limit_counts_unicode_chars() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();
    for id in [
        "con", "PRN", "aux", "nul", "clock$", "com1", "LPT9", "con.txt", "aux ", "lpt1.",
    ] {
        assert!(matches!(
            store.save(&profile(id, "Reserved", "r.ScreenPercentage"), &catalog),
            Err(ProfileError::InvalidName(_))
        ));
    }
    let unicode_name = "한".repeat(80);
    let saved = store
        .save(
            &profile("unicode", &unicode_name, "r.ScreenPercentage"),
            &catalog,
        )
        .unwrap();
    assert_eq!(saved.name.chars().count(), 80);

    let collision_dir = directory.path().join("profiles/custom");
    std::fs::write(collision_dir.join("CASE.json"), b"external collision").unwrap();
    assert!(matches!(
        store.save(
            &profile("case", "Case", "r.ScreenPercentage"),
            &catalog
        ),
        Err(ProfileError::ProfileAlreadyExists(id)) if id == "case"
    ));
}

#[test]
fn custom_entry_and_cpu_id_arrays_are_strictly_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();

    let mut custom = profile("custom-limit", "Custom limit", "r.ScreenPercentage");
    custom.patch.custom_ini_entries = (0..257)
        .map(|index| custom_entry("System", &format!("r.Custom{index}"), "1"))
        .collect();
    assert!(matches!(
        store.save(&custom, &catalog),
        Err(ProfileError::InvalidProfile("too_many_custom_entries"))
    ));

    let mut cpu = profile("cpu-limit", "CPU limit", "r.ScreenPercentage");
    cpu.patch.process.cpu_selection = CpuSelection::ManualCpuSets {
        ids: (0..257).collect(),
    };
    assert!(matches!(
        store.save(&cpu, &catalog),
        Err(ProfileError::InvalidProfile("too_many_cpu_sets"))
    ));
}

#[cfg(unix)]
#[test]
fn list_propagates_directory_permission_errors() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path());
    let catalog = Catalog::load_embedded().unwrap();
    store
        .save(
            &profile("permission", "Permission", "r.ScreenPercentage"),
            &catalog,
        )
        .unwrap();
    let custom = directory.path().join("profiles/custom");
    std::fs::set_permissions(&custom, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = store.list();
    std::fs::set_permissions(&custom, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert!(matches!(result, Err(ProfileError::Io { .. })));
}
