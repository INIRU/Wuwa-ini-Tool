pub mod ini_document;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("unsupported_platform")]
    UnsupportedPlatform,
    #[error("tauri_runtime: {0}")]
    Tauri(#[from] tauri::Error),
}

#[cfg(target_os = "windows")]
pub fn run() -> Result<(), RunError> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .run(tauri::generate_context!())
        .map_err(RunError::Tauri)
}

#[cfg(not(target_os = "windows"))]
pub fn run() -> Result<(), RunError> {
    Err(RunError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_entrypoint_returns_unsupported_platform() {
        let error = super::run().expect_err("non-Windows entrypoint must not start Tauri");

        assert!(matches!(error, super::RunError::UnsupportedPlatform));
        assert_eq!(error.to_string(), "unsupported_platform");
    }

    #[test]
    fn package_version_matches_product_version() {
        assert_eq!(env!("CARGO_PKG_VERSION"), "1.0.0");
    }
}
