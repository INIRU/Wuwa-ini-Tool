use serde_json::Value;
use wuwa_ini_tool_lib::commands::ExternalLinkKind;

#[test]
fn tauri_csp_and_capabilities_are_restrictive() {
    let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let csp = config["app"]["security"]["csp"]
        .as_str()
        .expect("a restrictive CSP is required");
    assert!(csp.contains("default-src 'self'"));
    assert!(csp.contains("object-src 'none'"));
    assert!(!csp.contains("unsafe-eval"));
    assert!(!csp.contains("https:"));

    let capability: Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    let permissions = capability["permissions"].as_array().unwrap();
    let permissions = permissions
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    assert!(!permissions.iter().any(|permission| {
        permission.starts_with("fs:")
            || permission.starts_with("shell:")
            || permission.ends_with(":default") && *permission != "core:default"
    }));
    assert!(permissions.contains(&"dialog:allow-open"));
    assert!(!permissions
        .iter()
        .any(|permission| permission.starts_with("opener:")));
}

#[test]
fn external_links_are_server_owned_and_allowlisted() {
    assert_eq!(
        ExternalLinkKind::SourceCode.url(),
        "https://github.com/INIRU/Wuwa-ini-Tool"
    );
    assert_eq!(
        ExternalLinkKind::Releases.url(),
        "https://github.com/INIRU/Wuwa-ini-Tool/releases"
    );
    assert_eq!(
        ExternalLinkKind::ReportIssue.url(),
        "https://github.com/INIRU/Wuwa-ini-Tool/issues/new/choose"
    );
    assert_eq!(
        ExternalLinkKind::KuroGamesOfficial.url(),
        "https://wutheringwaves.kurogames.com/en/main/"
    );
}

#[test]
fn single_instance_is_the_first_registered_plugin() {
    let source = include_str!("../src/lib.rs");
    let single_instance = source
        .find(".plugin(tauri_plugin_single_instance::init")
        .expect("single-instance plugin must be registered");
    let dialog = source
        .find(".plugin(tauri_plugin_dialog::init")
        .expect("dialog plugin must be registered");

    assert!(single_instance < dialog);
}
