pub mod backup_store;
pub mod cache_cleanup;
pub mod catalog;
pub mod commands;
pub mod game_discovery;
pub mod ini_document;
pub mod maintenance;
pub mod process_control;
pub mod profile_store;
pub mod supervisor;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("unsupported_platform")]
    UnsupportedPlatform,
    #[error("tauri_runtime: {0}")]
    Tauri(#[from] tauri::Error),
}

#[cfg(target_os = "windows")]
pub fn run() -> Result<(), RunError> {
    use tauri::{
        menu::{Menu, MenuItem},
        tray::TrayIconBuilder,
        Emitter, Manager, WindowEvent,
    };
    use tauri_plugin_updater::UpdaterExt;

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            let local_app_data = app.path().local_data_dir()?;
            app.manage(commands::RuntimeState::new(app_data, local_app_data));

            let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::new()
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        let restored = app
                            .try_state::<commands::RuntimeState>()
                            .is_none_or(|state| state.shutdown().is_ok());
                        if restored {
                            app.exit(0);
                        } else {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                            let _ = app.emit(
                                "supervisor://error",
                                commands::SupervisorRuntimeError {
                                    code: "supervisor_shutdown_failed",
                                },
                            );
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            let updater_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let Ok(updater) = updater_app.updater() else {
                    return;
                };
                let Ok(Some(update)) = updater.check().await else {
                    return;
                };
                let state = updater_app.state::<commands::RuntimeState>();
                let version = update.version.clone();
                if state.set_pending_update(update).is_ok() {
                    let _ = updater_app
                        .emit("update://available", commands::UpdateAvailable { version });
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_snapshot,
            commands::preview_ini,
            commands::preview_ini_import,
            commands::apply_ini,
            commands::preview_restore_backup,
            commands::restore_backup,
            commands::save_profile,
            commands::discover_game,
            commands::discover_game_manual,
            commands::select_game,
            commands::launch_game,
            commands::get_cpu_topology,
            commands::apply_process_settings,
            commands::preview_focus_mode,
            commands::activate_focus_mode,
            commands::deactivate_focus_mode,
            commands::preview_cache_cleanup,
            commands::run_cache_cleanup,
            commands::list_profiles,
            commands::export_profile,
            commands::import_profile,
            commands::save_imported_profile,
            commands::list_backups,
            commands::pin_backup,
            commands::install_update,
            commands::external::open_external_link,
        ])
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

    #[test]
    fn typed_command_surface_generates_an_invoke_handler() {
        let _builder =
            tauri::Builder::<tauri::Wry>::default().invoke_handler(tauri::generate_handler![
                crate::commands::get_app_snapshot,
                crate::commands::preview_ini,
                crate::commands::preview_ini_import,
                crate::commands::apply_ini,
                crate::commands::preview_restore_backup,
                crate::commands::restore_backup,
                crate::commands::save_profile,
                crate::commands::discover_game,
                crate::commands::discover_game_manual,
                crate::commands::select_game,
                crate::commands::launch_game,
                crate::commands::get_cpu_topology,
                crate::commands::apply_process_settings,
                crate::commands::preview_focus_mode,
                crate::commands::activate_focus_mode,
                crate::commands::deactivate_focus_mode,
                crate::commands::preview_cache_cleanup,
                crate::commands::run_cache_cleanup,
                crate::commands::list_profiles,
                crate::commands::export_profile,
                crate::commands::import_profile,
                crate::commands::save_imported_profile,
                crate::commands::list_backups,
                crate::commands::pin_backup,
                crate::commands::install_update,
                crate::commands::external::open_external_link,
            ]);
    }

    #[test]
    fn single_instance_plugin_compiles_with_the_desktop_builder() {
        let _builder = tauri::Builder::<tauri::Wry>::default()
            .plugin(tauri_plugin_single_instance::init(|_app, _argv, _cwd| {}));
    }
}
