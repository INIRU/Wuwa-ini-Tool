#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .run(tauri::generate_context!())
        .expect("error while running Wuwa ini Tool");
}

#[cfg(test)]
mod tests {
    #[test]
    fn package_version_matches_product_version() {
        assert_eq!(env!("CARGO_PKG_VERSION"), "1.0.0");
    }
}
