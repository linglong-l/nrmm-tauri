pub mod commands;
pub mod config;
pub mod core;
pub mod hotkey;
pub mod models;
pub mod platform;
pub mod resources;
pub mod tray;
pub mod updater;
pub mod window;

use crate::core::file_watcher::FileWatcher;
use std::sync::{Arc, Mutex};
use tauri::Manager;

/// Tauri 应用主入口函数
///
/// 负责初始化和配置 Tauri 应用，包括：
/// - 初始化设置存储
/// - 注册各类插件（shell、dialog、notification、updater 等）
/// - 配置全局快捷键处理
/// - 设置窗口效果和系统托盘
/// - 启动后台云数据刷新任务
/// - 注册 Tauri 命令处理器
///
/// 在应用关闭时阻止窗口真正关闭，仅隐藏窗口
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    config::settings_store::init_settings().expect("Failed to initialize settings");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_os::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        hotkey::handle_hotkey(app, shortcut, &event);
                    }
                })
                .build(),
        )
        .setup(|app| {
            if let Some(main_window) = app.get_webview_window("main") {
                window::apply_window_effects(&main_window);
            }

            if let Err(e) = tray::create_tray(app.handle()) {
                log::warn!("Failed to create tray icon: {}", e);
            }

            let hotkey_mgr = hotkey::HotkeyManager::new(app.handle().clone());
            if let Err(e) = hotkey_mgr.register_all() {
                log::warn!("Failed to register hotkeys: {}", e);
            }
            app.manage(Arc::new(hotkey_mgr));

            let file_watcher = FileWatcher::new();
            app.manage(Arc::new(Mutex::new(file_watcher)));

            tauri::async_runtime::spawn(async move {
                log::info!("Starting background cloud data refresh...");
                let results = core::cloud_data::CloudDataManager::refresh_all_async().await;
                for (name, res) in results {
                    match res {
                        Ok(_) => log::info!("Cloud data refreshed: {}", name),
                        Err(e) => log::warn!("Failed to refresh cloud data {}: {}", name, e),
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings_commands::get_settings,
            commands::settings_commands::save_settings,
            commands::settings_commands::reset_settings,
            commands::settings_commands::export_settings,
            commands::settings_commands::import_settings,
            commands::mod_commands::get_mods,
            commands::mod_commands::check_mods_path_status,
            commands::mod_commands::refresh_mods,
            commands::mod_commands::select_mod,
            commands::mod_commands::deselect_group_mod,
            commands::mod_commands::add_group,
            commands::mod_commands::remove_group,
            commands::mod_commands::remove_mod,
            commands::mod_commands::rename_mod,
            commands::mod_commands::rename_group,
            commands::mod_commands::toggle_mod_disabled,
            commands::mod_commands::toggle_favorite,
            commands::mod_commands::is_favorite,
            commands::mod_commands::open_mod_folder,
            commands::mod_commands::open_group_folder,
            commands::mod_commands::restore_all_inis,
            hotkey::reregister_hotkeys,
            hotkey::unregister_hotkeys,
            hotkey::simulate_select_group,
            hotkey::simulate_select_mod,
            hotkey::check_keypress_support,
            hotkey::is_game_foreground,
            hotkey::get_cursor_position,
            core::file_watcher::start_file_watcher,
            core::file_watcher::stop_file_watcher,
            core::archive_handler::is_supported_archive_cmd,
            core::archive_handler::import_mod_cmd,
            core::archive_handler::import_mod_auto_cmd,
            core::cloud_data::refresh_cloud_data,
            core::cloud_data::refresh_all_cloud_data,
            updater::check_for_updates,
            updater::compare_versions,
            updater::get_app_version,
            window::show_window,
            window::hide_window,
            window::set_window_position,
            window::close_window,
            window::minimize_window,
            window::toggle_maximize,
            window::center_window_cmd,
            window::is_window_visible,
            platform::get_platform_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
