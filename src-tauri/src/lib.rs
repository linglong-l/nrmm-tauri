//! NRMM (No-Reload Mod Manager) Tauri 应用库入口
//!
//! 本模块是整个应用的核心入口点，负责：
//! - 声明所有子模块（commands, config, core, hotkey, models 等）
//! - 初始化日志系统（debug 模式下 Debug 级别，release 模式下 Info 级别）
//! - 初始化设置存储
//! - 配置 Tauri 插件（shell, dialog, notification, updater, window_state, os, global_shortcut）
//! - 创建系统托盘
//! - 注册全局快捷键管理器
//! - 初始化文件监控器
//! - 启动后台云端数据刷新任务
//! - 注册所有 Tauri 命令处理器
//! - 处理窗口关闭事件（阻止主窗口关闭，仅隐藏）

pub mod commands;
pub mod config;
pub mod core;
pub mod hotkey;
pub mod models;
pub mod platform;
pub mod resources;
pub mod tray;
pub mod updater;
mod utils;
pub mod window;
use crate::core::file_watcher::FileWatcher;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{Emitter, Manager};

/// 全局应用启动时间点，用于前端报告UI就绪时计算总启动耗时
static BOOT_START: OnceLock<std::time::Instant> = OnceLock::new();

/// 前端报告UI就绪命令
///
/// 当前端完成首帧渲染和关键初始化后调用此命令，
/// 后端将输出从进程启动到UI完全就绪的总耗时。
///
/// # 参数
/// - `stage`: 当前就绪阶段描述（如 "dom-mounted", "settings-loaded", "notification-sent", "fully-ready"）
/// - `frontend_ms`: 前端从脚本执行到当前阶段的耗时（毫秒）
#[tauri::command]
fn report_frontend_ready(stage: String, frontend_ms: f64) {
    if let Some(&start) = BOOT_START.get() {
        let total_ms = start.elapsed().as_millis();
        log::info!(
            "[BOOT] T+{:>6}ms - 前端 [{}] 就绪（前端耗时: {:.0}ms）",
            total_ms, stage, frontend_ms
        );

        // 在 fully-ready 阶段输出汇总信息
        if stage == "fully-ready" {
            log::info!("[BOOT] ===== NRMM 启动完成，总耗时: {}ms（前端: {:.0}ms） =====",
                total_ms, frontend_ms);
        }
    }
}

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
    // ===== 启动计时开始 =====
    let boot_start = std::time::Instant::now();
    // 记录到全局OnceLock，供前端report_frontend_ready命令查询
    let _ = BOOT_START.set(boot_start);

    let boot_ts = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
    log::info!("[BOOT] ===== NRMM 启动开始 at {} =====", boot_ts);

    #[cfg(debug_assertions)]
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .init();

    #[cfg(not(debug_assertions))]
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();
    log::info!("[BOOT] T+{:>6}ms - 日志系统初始化完成", boot_start.elapsed().as_millis());

    config::settings_store::init_settings().expect("Failed to initialize settings");
    log::info!("[BOOT] T+{:>6}ms - 设置存储初始化完成", boot_start.elapsed().as_millis());

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        hotkey::handle_hotkey(app, shortcut, &event);
                    }
                })
                .build(),
        )
        .setup(move |app| {
            log::info!("[BOOT] T+{:>6}ms - setup() 回调开始（插件已加载）", boot_start.elapsed().as_millis());

            if let Err(e) = tray::create_tray(app.handle()) {
                log::warn!("Failed to create tray icon: {}", e);
            }
            log::info!("[BOOT] T+{:>6}ms - 系统托盘创建完成", boot_start.elapsed().as_millis());

            // 全局原生菜单事件：处理游戏选择菜单的点击（热键未匹配游戏时弹出）
            let app_handle_clone = app.app_handle().clone();
            app.on_menu_event(move |app_h, event| {
                let menu_id = event.id().0.as_str();
                if let Some(game_str) = menu_id.strip_prefix("game_menu:") {
                    use crate::hotkey::parse_game;

                    log::info!("[Menu] game_menu clicked: {}", game_str);
                    match parse_game(game_str) {
                        Ok(game) => {
                            let _ = app_h.emit("window-show-with-game", game);
                            crate::window::show_main_window(&app_handle_clone);
                        }
                        Err(e) => {
                            log::error!("[Menu] parse game failed for {}: {}", game_str, e);
                            crate::window::show_main_window(&app_handle_clone);
                        }
                    }
                }
            });

            let hotkey_mgr = hotkey::HotkeyManager::new(app.handle().clone());
            if let Err(e) = hotkey_mgr.register_all() {
                log::warn!("Failed to register hotkeys: {}", e);
            }
            app.manage(Arc::new(hotkey_mgr));
            log::info!("[BOOT] T+{:>6}ms - 全局快捷键注册完成", boot_start.elapsed().as_millis());

            let file_watcher = FileWatcher::new();
            app.manage(Arc::new(Mutex::new(file_watcher)));
            log::info!("[BOOT] T+{:>6}ms - 文件监控器初始化完成", boot_start.elapsed().as_millis());

            // 克隆启动时间用于异步任务和事件监听
            let boot_start_clone = boot_start;

            // 云端数据刷新延迟启动：避免在前端WebView关键加载期占用网络和tokio线程池
            // 前端完全就绪（fully-ready）后会提前触发
            tauri::async_runtime::spawn(async move {
                const CLOUD_REFRESH_DELAY_MS: u64 = 5000;
                log::info!("[BOOT] T+{:>6}ms - 后台云端数据刷新任务已调度（延迟{}ms启动）",
                    boot_start_clone.elapsed().as_millis(), CLOUD_REFRESH_DELAY_MS);
                tokio::time::sleep(std::time::Duration::from_millis(CLOUD_REFRESH_DELAY_MS)).await;
                log::info!("[BOOT] T+{:>6}ms - 开始执行云端数据刷新", boot_start_clone.elapsed().as_millis());
                let results = core::cloud_data::CloudDataManager::refresh_all_async().await;
                for (name, res) in results {
                    match res {
                        Ok(_) => log::info!("[BOOT] Cloud data refreshed: {} (T+{}ms)", name, boot_start_clone.elapsed().as_millis()),
                        Err(e) => log::warn!("[BOOT] Failed to refresh cloud data {}: {} (T+{}ms)", name, e, boot_start_clone.elapsed().as_millis()),
                    }
                }
            });

            // 监听窗口事件，记录窗口首次显示时间
            let window = app.get_webview_window("main").expect("Main window not found");
            let boot_start_for_window = boot_start;
            let first_resized_logged = Arc::new(AtomicBool::new(false));
            let first_focus_logged = Arc::new(AtomicBool::new(false));
            window.on_window_event(move |event| {
                match event {
                    tauri::WindowEvent::Resized(_) => {
                        if !first_resized_logged.swap(true, Ordering::SeqCst) {
                            log::info!("[BOOT] T+{:>6}ms - 窗口首次 Resized（即将可见）", boot_start_for_window.elapsed().as_millis());
                        }
                    }
                    tauri::WindowEvent::Focused(focused) if *focused && !first_focus_logged.swap(true, Ordering::SeqCst) => {
                        log::info!("[BOOT] T+{:>6}ms - 窗口首次获得焦点（可见）", boot_start_for_window.elapsed().as_millis());
                    }
                    _ => {}
                }
            });

            log::info!("[BOOT] T+{:>6}ms - setup() 完成，等待前端渲染", boot_start.elapsed().as_millis());
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                    // 点击关闭按钮隐藏窗口时，通知前端清除模组数据
                    let _ = window.app_handle().emit("window-hidden", ());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            report_frontend_ready,
            commands::settings_commands::get_settings,
            commands::settings_commands::save_settings,
            commands::settings_commands::reset_settings,
            commands::settings_commands::export_settings,
            commands::settings_commands::import_settings,
            commands::settings_commands::switch_target_game,
            commands::mod_commands::get_mods,
            commands::mod_commands::check_mods_path_status,
            commands::mod_commands::refresh_mods,
            commands::mod_commands::update_mod_data,
            commands::mod_commands::update_group_mod_data,
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
            commands::mod_commands::save_customizations,
            commands::mod_commands::batch_toggle_mods,
            commands::mod_commands::validate_subfolder_name,
            commands::mod_commands::create_subfolder,
            commands::mod_commands::disable_all_mods_in_group,
            commands::mod_commands::enable_all_mods_in_group,
            commands::mod_commands::remove_group_ex,
            commands::mod_commands::simulate_f10,
            hotkey::reregister_hotkeys,
            hotkey::unregister_hotkeys,
            hotkey::simulate_select_group,
            hotkey::simulate_select_mod,
            hotkey::check_keypress_support,
            hotkey::is_game_foreground,
            hotkey::get_cursor_position,
            core::file_watcher::start_file_watcher,
            core::file_watcher::stop_file_watcher,
            core::file_watcher::switch_file_watcher,
            core::file_watcher::pause_file_watcher,
            core::file_watcher::resume_file_watcher,
            core::file_watcher::is_file_watcher_running,
            core::file_watcher::current_watched_path,
            core::mod_cache::check_mod_cache_valid,
            core::archive_handler::is_supported_archive_cmd,
            core::archive_handler::import_mod_cmd,
            core::archive_handler::import_mod_auto_cmd,
            core::archive_handler::import_item_cmd,
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
            window::reset_window_position,
            window::show_main_window_cmd,
            window::toggle_main_window_cmd,
            window::hard_quit_app,
            window::get_foreground_process_name,
            platform::get_platform_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
