//! XXMI-NRMM 后端核心库入口模块。
//!
//! 本模块是 Tauri 应用的 Rust 侧根模块，负责：
//! - 声明各功能子模块
//! - 提供 [`run`] 作为应用启动入口（供 `main.rs` 或移动端入口调用）
//! - 初始化日志系统
//! - 组装 Tauri 应用（状态、插件、setup 回调、命令注册）
//!
//! 整体执行顺序：`main()` → [`run`] → `init_logging()` → 构造 `AppState` →
//! Tauri Builder 装配插件 → setup 回调加载设置/窗口/热键/托盘 → 注册命令并启动事件循环。

// 子模块声明：每个模块对应一类业务能力
mod cloud_data; // 云端数据同步
mod commands;
mod file_watcher; // 文件系统监听（Mods 目录变更通知）
mod hotkey; // 全局热键注册与事件分发
mod ini_handler; // INI 文件读写与语法检查
mod init_xx; // 初始化辅助逻辑（含日志格式化）
mod keypress_simulator; // 按键/鼠标动作模拟
mod mod_manager; // Mod 管理器：加载、刷新、分组、收藏等
mod process; // 目标游戏进程检测与匹配
mod settings; // 用户设置持久化
mod state; // 全局应用状态容器 AppState
mod task_queue; // 后台任务队列
mod tray; // 系统托盘菜单与图标事件
mod window_manager; // 主窗口的显示/隐藏/尺寸/置顶等管理 // 暴露给前端的 Tauri 命令

use fern::Dispatch;
use state::AppState;
use tauri::Manager;

/// 获取应用数据目录（%LOCALAPPDATA%\xxmi-nrmm）。
///
/// 用于统一配置文件、日志等所有应用数据的存储位置。
/// 日志目录为 `app_data_dir()/logs`，配置文件为 `app_data_dir()/settings.json`。
///
/// # 返回值
/// 成功返回 `Some(PathBuf)`，失败返回 `None`（罕见平台差异）。
pub fn get_app_data_dir() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|d| d.join("xxmi-nrmm"))
}

/// Tauri 应用启动入口。
///
/// 执行流程：
/// 1. 调用 [`init_logging`] 初始化 fern 日志系统；
/// 2. 构造全局状态 [`AppState`]；
/// 3. 通过 `tauri::Builder` 装配：
///    - 注入 `AppState`（`.manage`）；
///    - 注册对话框插件、全局快捷键插件；
///    - 在 `setup` 回调中完成设置加载、初始窗口、热键注册、托盘创建以及窗口事件监听；
///    - 通过 `invoke_handler` 注册所有暴露给前端的命令。
/// 4. 调用 `.run` 启动 Tauri 事件循环，失败时 panic。
///
/// # 参数
/// 无。
///
/// # 返回值
/// 无（返回 `()`）；启动失败时通过 `expect` 直接 panic。
///
/// # 限制
/// - `#[cfg_attr(mobile, tauri::mobile_entry_point)]`：移动端使用本函数作为入口点。
/// - setup 回调中的任何失败均被记录但不会中断启动（除窗口/托盘关键路径会记录 error）。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化日志系统（使用 fern 替代 tauri-plugin-log）
    init_logging();

    // 构造全局共享状态，所有命令通过 Tauri 的 State 注入获取
    let app_state = AppState::new();

    tauri::Builder::default()
        .manage(app_state) // 将 AppState 注入 Tauri 状态管理
        .plugin(tauri_plugin_dialog::init()) // 前端需要对话框（选择目录/文件）
        .plugin(
            // 全局快捷键插件：所有快捷键事件统一路由到 HotkeyManager::handle_hotkey_event
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    crate::hotkey::HotkeyManager::handle_hotkey_event(app, shortcut, event.state);
                })
                .build(),
        )
        .setup(|app| {
            // setup 回调在应用初始化阶段执行一次，用于装配运行时所需资源
            let app_handle = app.handle();
            let state = app_handle.state::<AppState>();

            // 1) 加载持久化设置：失败时回退到默认值并记录警告
            match get_app_data_dir() {
                Some(app_data_dir) => {
                    let settings = crate::settings::Settings::load(&app_data_dir);
                    {
                        // 写入状态时获取写锁，写入后立即释放
                        let mut state_settings = state.settings.write();
                        *state_settings = settings;
                    }
                    log::info!("Settings loaded during startup");
                }
                None => {
                    log::warn!("Failed to get app data dir during startup, using default settings");
                }
            }

            // 2) 应用初始窗口状态（最小尺寸 + 恢复上次的位置/尺寸/置顶）
            if let Err(e) = crate::window_manager::WindowManager::setup_initial_window(
                &app_handle,
                &state.settings,
            ) {
                log::error!("Failed to setup initial window state: {}", e);
            }

            // 3) 根据已加载设置注册全局热键（先清空再注册，避免重复）
            {
                let settings = state.settings.read();
                if let Err(e) =
                    crate::hotkey::HotkeyManager::register_from_settings(&app_handle, &settings)
                {
                    log::error!("Failed to register hotkeys from settings: {}", e);
                }
            }

            // 4) 创建系统托盘菜单与图标
            if let Err(e) = crate::tray::TrayManager::setup_tray(&app_handle) {
                log::error!("Failed to setup tray: {}", e);
            }

            // 5) 注册托盘菜单点击事件分发器
            app_handle.on_menu_event(|app, event| {
                if let Err(e) =
                    crate::tray::TrayManager::handle_menu_event(app, event.id().as_ref())
                {
                    log::error!("Failed to handle menu event: {}", e);
                }
            });

            // 6) 注册托盘图标交互事件（如左键单击切换窗口显示）
            let app_handle_for_tray = app_handle.clone();
            app_handle.on_tray_icon_event(move |_tray, event| {
                if let Err(e) =
                    crate::tray::TrayManager::handle_tray_icon_event(&app_handle_for_tray, &event)
                {
                    log::error!("Failed to handle tray icon event: {}", e);
                }
            });

            // 7) 监听主窗口生命周期事件，用于持久化窗口状态与设置
            //    为避免 Move/Resized 高频触发导致写入风暴，使用独立线程 + sleep 500ms 进行防抖
            let settings_arc = state.settings.clone();
            let app_handle_clone = app_handle.clone();
            app_handle.get_webview_window("main").map(|window| {
                window.on_window_event(move |event| {
                    let settings = settings_arc.clone();
                    let app = app_handle_clone.clone();
                    match event {
                        // 关闭请求：保存窗口状态，并在独立线程中保存设置文件
                        tauri::WindowEvent::CloseRequested { .. } => {
                            if let Err(e) = crate::window_manager::WindowManager::save_window_state(
                                &app, &settings,
                            ) {
                                log::error!("Failed to save window state on close: {}", e);
                            }
                            if let Some(app_data_dir) = get_app_data_dir() {
                                let settings_clone = settings.clone();
                                // 在独立线程执行文件 IO，避免阻塞 UI 线程
                                std::thread::spawn(move || {
                                    let s = {
                                        // 仅短时持有读锁以克隆设置，随后立即释放
                                        let guard = settings_clone.read();
                                        guard.clone()
                                    };
                                    if let Err(e) = s.save(&app_data_dir) {
                                        log::error!("Failed to save settings file on close: {}", e);
                                    }
                                });
                            }
                        }
                        // 窗口移动：防抖 500ms 后保存窗口状态
                        tauri::WindowEvent::Moved(_) => {
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(500));
                                if let Err(e) =
                                    crate::window_manager::WindowManager::save_window_state(
                                        &app, &settings,
                                    )
                                {
                                    log::error!("Failed to save window state on move: {}", e);
                                }
                            });
                        }
                        // 窗口尺寸变化：防抖 500ms 后保存窗口状态
                        tauri::WindowEvent::Resized(_) => {
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(500));
                                if let Err(e) =
                                    crate::window_manager::WindowManager::save_window_state(
                                        &app, &settings,
                                    )
                                {
                                    log::error!("Failed to save window state on resize: {}", e);
                                }
                            });
                        }
                        _ => {}
                    }
                });
            });

            Ok(())
        })
        // 注册所有暴露给前端的 Tauri 命令，前端通过 invoke 调用
        .invoke_handler(tauri::generate_handler![
            commands::load_mods,
            commands::refresh_mods,
            commands::refresh_mod_data,
            commands::update_mod_data,
            commands::validate_mods_path,
            commands::get_group_name,
            commands::set_group_name,
            commands::get_selected_mod,
            commands::set_selected_mod,
            commands::get_selected_group,
            commands::set_selected_group,
            commands::is_favorite,
            commands::toggle_favorite,
            commands::toggle_group_favorite,
            commands::toggle_mod_favorite,
            commands::get_icon_path,
            commands::set_icon,
            commands::remove_icon,
            commands::toggle_mod_disabled,
            commands::toggle_tree_node_mod_disabled,
            commands::add_group,
            commands::remove_group,
            commands::rename_group,
            commands::search_mods,
            commands::load_ini,
            commands::save_ini,
            commands::extract_namespaces,
            commands::start_file_watcher,
            commands::stop_file_watcher,
            commands::is_file_watcher_running,
            commands::register_hotkey,
            commands::unregister_hotkey,
            commands::unregister_all_hotkeys,
            commands::is_hotkey_registered,
            commands::show_window,
            commands::hide_window,
            commands::toggle_window,
            commands::set_always_on_top,
            commands::is_always_on_top,
            commands::set_window_size,
            commands::get_window_size,
            commands::set_window_position,
            commands::reset_window_position,
            commands::save_window_state,
            commands::setup_tray,
            commands::update_tray_tooltip,
            commands::is_process_running,
            commands::get_process_list,
            commands::get_foreground_process,
            commands::get_foreground_game,
            commands::get_settings,
            commands::save_settings,
            commands::reset_settings,
            commands::get_cloud_data,
            commands::fetch_cloud_data,
            commands::sync_cloud_data,
            commands::simulate_key_press,
            commands::simulate_key_combination,
            commands::simulate_mouse_move,
            commands::check_ini_syntax,
            commands::check_all_mods_syntax,
            commands::select_directory,
            commands::open_path,
            commands::open_mod_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 初始化 fern 日志系统。
///
/// 配置两级日志输出链：
/// 1. **stdout**：始终启用，级别为 `Debug`，格式由 [`init_xx::logger::custom_log_format`] 决定；
/// 2. **文件**：尝试在 `dirs::data_local_dir()/xxmi-nrmm/logs/app.log` 创建/追加日志文件。
///    - 目录不存在时自动创建；
///    - 文件创建或打开失败时回退到仅 stdout 输出。
///
/// # 参数
/// 无。
///
/// # 返回值
/// 无。
///
/// # 边界情况
/// - 当 `dirs::data_local_dir()` 返回 `None`（罕见平台差异）时，仅输出到 stdout；
/// - `apply()` 仅在第一次成功设置全局 logger 时生效，后续调用会被忽略。
///
/// # 限制
/// - 日志级别固定为 `Debug`，目前未通过设置动态调整；
/// - 日志文件按日期分层存储（year/month/day.log），无自动清理。
fn init_logging() {
    // 构造基础 Dispatch：设置全局级别与统一格式化器，并链接到 stdout
    let dispatch = Dispatch::new()
        .level({
            if cfg!(dev) {
                log::LevelFilter::Debug
            } else {
                log::LevelFilter::Info
            }
        })
        .format(|out, message, record| {
            init_xx::logger::custom_log_format(out, message, record);
        })
        .chain(std::io::stdout());

    // 尝试添加文件日志（可选）：失败则回退到仅 stdout
    if let Some(log_dir) = dirs::data_local_dir() {
        let app_log_dir = log_dir.join("xxmi-nrmm").join("logs");
        // 确保日志根目录存在
        if std::fs::create_dir_all(&app_log_dir).is_ok() {
            // 按日期生成日志文件路径：logs/2026/06/30.log
            let now = time::OffsetDateTime::now_local()
                .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
            let year_dir = app_log_dir.join(format!("{}", now.year()));
            let month_dir = year_dir.join(format!("{:02}", now.month() as u8));
            let log_file = month_dir.join(format!("{:02}.log", now.day()));

            // 确保年月目录存在，以追加模式打开日志文件
            if std::fs::create_dir_all(&month_dir).is_ok() {
                if let Ok(file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_file)
                {
                    // 同时输出到 stdout 与文件，并应用为全局 logger
                    let _ = dispatch.chain(file).apply();
                    return;
                }
            }
        }
    }

    // 如果文件日志失败，只输出到 stdout
    let _ = dispatch.apply();
}
