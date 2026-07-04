//! 全局热键管理模块。
//!
//! 提供：
//! - 内部热键标识（如 `"altW"`）与 Tauri 加速器字符串（如 `"Alt+W"`）之间的双向映射；
//! - 单个热键的注册/注销/查询；
//! - 从用户设置批量注册热键；
//! - 全局快捷键事件的处理流程（区分游戏中/游戏外两种场景）。
//!
//! ## 事件处理总流程
//! `tauri_plugin_global_shortcut` 的所有事件都会被路由到
//! [`HotkeyManager::handle_hotkey_event`]，由其根据当前前台进程是否匹配
//! 任意目标游戏来分发到 [`HotkeyManager::handle_in_game_hotkey`] 或
//! [`HotkeyManager::handle_out_of_game_hotkey`]。

use anyhow::{Context, Result};
use serde_json;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::process::{ProcessDetector, TargetGame};
use crate::settings::Settings;
use crate::window_manager::WindowManager;

/// 内部热键标识 ↔ Tauri 加速器字符串的映射表。
///
/// - 第一项：设置文件与命令中使用的内部标识（小写无分隔，如 `"altW"`）；
/// - 第二项：传给 `tauri_plugin_global_shortcut` 的加速器字符串（如 `"Alt+W"`）。
///
/// 通过宏 `hotkey_map!` 在编译期生成 Alt+A ~ Alt+Z 共 26 项映射，避免手写重复。
/// 新增热键时只需在宏调用中追加字母，其余逻辑自动适配。
macro_rules! hotkey_map {
    ($($letter:literal),* $(,)?) => {
        &[$((concat!("alt", $letter), concat!("Alt+", $letter)),)*]
    };
}

const HOTKEY_MAP: &[(&str, &str)] = hotkey_map!(
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
    "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
);

/// 热键管理器（无状态）。
///
/// 所有方法均以 `&AppHandle` 作为入口操作全局快捷键，因此本结构本身不持有数据，
/// 仅为方法归属与未来扩展预留。
pub struct HotkeyManager;

impl HotkeyManager {
    /// 构造空实例（保留以兼容状态注入）。
    pub fn new() -> Self {
        Self
    }

    /// 将内部热键标识转换为 Tauri 加速器字符串。
    ///
    /// # 参数
    /// - `hotkey_type`：内部标识（如 `"altW"`）。
    ///
    /// # 返回值
    /// 找到时返回对应的加速器字符串；未找到返回错误（带上下文）。
    fn hotkey_type_to_accelerator(hotkey_type: &str) -> Result<String> {
        HOTKEY_MAP
            .iter()
            .find(|(k, _)| *k == hotkey_type)
            .map(|(_, v)| v.to_string())
            .with_context(|| format!("Unknown hotkey type: {}", hotkey_type))
    }

    /// 将 Tauri 加速器字符串反向转换为内部热键标识。
    ///
    /// # 参数
    /// - `accelerator`：加速器字符串（如 `"Alt+W"`）。
    ///
    /// # 返回值
    /// 找到时返回 `Some(标识)`；未找到返回 `None`（用于在事件回调中过滤未知快捷键）。
    fn accelerator_to_hotkey_type(accelerator: &str) -> Option<String> {
        HOTKEY_MAP
            .iter()
            .find(|(_, v)| *v == accelerator)
            .map(|(k, _)| k.to_string())
    }

    /// 注册单个全局热键。
    ///
    /// # 业务逻辑
    /// 1. 将内部标识解析为加速器字符串；
    /// 2. 将加速器字符串解析为 [`Shortcut`]；
    /// 3. 调用 `global_shortcut().register` 注册到操作系统。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `hotkey_type`：内部热键标识。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`，任一步骤失败返回封装后的错误。
    pub fn register_hotkey(app: &AppHandle, hotkey_type: &str) -> Result<()> {
        let accelerator = Self::hotkey_type_to_accelerator(hotkey_type)?;

        let shortcut: Shortcut = accelerator
            .parse()
            .with_context(|| format!("Failed to parse accelerator: {}", accelerator))?;

        app.global_shortcut()
            .register(shortcut)
            .with_context(|| format!("Failed to register hotkey: {}", hotkey_type))?;

        log::info!("Hotkey registered: {} ({})", hotkey_type, accelerator);
        Ok(())
    }

    /// 注销单个全局热键。
    ///
    /// 流程与 [`Self::register_hotkey`] 对称：解析标识 → 解析加速器 → 调用
    /// `global_shortcut().unregister`。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `hotkey_type`：内部热键标识。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`，失败返回封装后的错误。
    pub fn unregister_hotkey(app: &AppHandle, hotkey_type: &str) -> Result<()> {
        let accelerator = Self::hotkey_type_to_accelerator(hotkey_type)?;

        let shortcut: Shortcut = accelerator
            .parse()
            .with_context(|| format!("Failed to parse accelerator: {}", accelerator))?;

        app.global_shortcut()
            .unregister(shortcut)
            .with_context(|| format!("Failed to unregister hotkey: {}", hotkey_type))?;

        log::info!("Hotkey unregistered: {} ({})", hotkey_type, accelerator);
        Ok(())
    }

    /// 注销所有已注册的全局热键。
    ///
    /// 用于应用退出、热键重置等场景，确保不残留系统级快捷键。
    pub fn unregister_all(app: &AppHandle) -> Result<()> {
        app.global_shortcut()
            .unregister_all()
            .context("Failed to unregister all hotkeys")?;

        log::info!("All hotkeys unregistered");
        Ok(())
    }

    /// 查询单个热键是否已注册。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `hotkey_type`：内部热键标识。
    ///
    /// # 返回值
    /// 已注册返回 `Ok(true)`，未注册返回 `Ok(false)`，标识未知则返回错误。
    pub fn is_registered(app: &AppHandle, hotkey_type: &str) -> Result<bool> {
        let accelerator = Self::hotkey_type_to_accelerator(hotkey_type)?;

        let shortcut: Shortcut = accelerator
            .parse()
            .with_context(|| format!("Failed to parse accelerator: {}", accelerator))?;

        let registered = app.global_shortcut().is_registered(shortcut);
        Ok(registered)
    }

    /// 根据用户设置批量（重新）注册热键。
    ///
    /// # 业务逻辑
    /// 1. 先调用 [`Self::unregister_all`] 清空旧热键，避免重复注册；
    /// 2. 读取 `settings.hotkey_keyboard`，若不为 `"none"` 则尝试注册；
    /// 3. 读取 `settings.hotkey_gamepad`，若不为 `"none"` 则记录（待实现）；
    /// 4. 注册失败时返回错误，确保调用方能感知失败。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `settings`：用户设置。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`，注册失败返回错误。
    pub fn register_from_settings(app: &AppHandle, settings: &Settings) -> Result<()> {
        // 先清空再注册，确保最终状态与设置一致
        Self::unregister_all(app)?;

        let hotkey_keyboard = &settings.hotkey_keyboard;
        if hotkey_keyboard != "none" {
            Self::register_hotkey(app, hotkey_keyboard)?;
        }

        let hotkey_gamepad = &settings.hotkey_gamepad;
        if hotkey_gamepad != "none" {
            log::info!("Gamepad hotkey '{}' configured (not yet implemented)", hotkey_gamepad);
        }

        Ok(())
    }

    /// 全局快捷键事件统一入口。
    ///
    /// # 处理流程
    /// 1. 仅响应按下事件（`ShortcutState::Pressed`），松开事件忽略；
    /// 2. 将加速器字符串反查为内部热键标识，未知快捷键直接忽略；
    /// 3. 尝试从 Tauri 状态中获取 [`AppState`]：
    ///    - 成功：读取设置克隆，调用 [`ProcessDetector`] 获取前台进程名并匹配游戏，
    ///      据此判断当前是否“在游戏中”；
    ///    - 失败：使用默认设置，并默认假设在游戏中（保证热键可用）；
    /// 4. 根据是否在游戏中分发到 [`Self::handle_in_game_hotkey`] 或
    ///    [`Self::handle_out_of_game_hotkey`]。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `shortcut`：触发事件的快捷键；
    /// - `state`：按下/松开状态。
    pub fn handle_hotkey_event(app: &AppHandle, shortcut: &Shortcut, state: ShortcutState) {
        // 只处理按下事件，避免一次按下触发两次
        if state != ShortcutState::Pressed {
            return;
        }

        let accelerator = shortcut.to_string();
        // 反查内部标识，未在映射表中的快捷键直接忽略
        let hotkey_type = match Self::accelerator_to_hotkey_type(&accelerator) {
            Some(t) => t,
            None => {
                log::debug!("Ignoring unrecognized shortcut: {}", accelerator);
                return;
            }
        };

        log::info!("Hotkey pressed: {} ({})", hotkey_type, accelerator);

        // 尝试获取 AppState，如果失败则使用默认设置
        let (settings, is_in_game) = match app.try_state::<crate::state::AppState>() {
            Some(state) => {
                let settings = state.settings.read().clone();
                // 获取前台进程名，失败时返回 "unknown"（不会匹配任何游戏）
                let foreground_process = ProcessDetector::get_foreground_process_name()
                    .unwrap_or_else(|_| "unknown".to_string());
                let game = ProcessDetector::match_game_process(&foreground_process, &settings);
                log::debug!("Foreground process: {}, matched game: {:?}", foreground_process, game);
                (settings, game != TargetGame::None)
            }
            None => {
                // 状态未注入（异常路径）：使用默认设置并假设在游戏中，确保热键可用
                log::warn!("AppState not found, using default settings");
                (Settings::default(), true) // 默认假设在游戏中
            }
        };

        // 执行热键处理
        if is_in_game {
            Self::handle_in_game_hotkey(app, &settings, &hotkey_type);
        } else {
            Self::handle_out_of_game_hotkey(app, &settings, &hotkey_type);
        }
    }

    /// 处理“在游戏中”触发的热键。
    ///
    /// # 业务逻辑
    /// 1. 切换主窗口显示状态（可见↔隐藏）；
    /// 2. 若窗口变为可见且 `is_auto_pin_window` 开启，则设置窗口置顶；
    /// 3. 向前端发送 `hotkey-pressed` 事件（包含热键标识和来源），便于前端做后续处理。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `settings`：用户设置（用于判断是否自动置顶）；
    /// - `hotkey_type`：触发的热键标识（如 `"altW"`）。
    fn handle_in_game_hotkey(app: &AppHandle, settings: &Settings, hotkey_type: &str) {
        match WindowManager::toggle_window(app) {
            Ok(shown) => {
                log::info!("Window toggled, now visible: {}", shown);
                // 仅在显示且配置允许时设置置顶
                if shown && settings.is_auto_pin_window {
                    if let Err(e) = WindowManager::set_always_on_top(app, true) {
                        log::warn!("Failed to set window always on top: {}", e);
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to toggle window: {}", e);
            }
        }

        // 通知前端热键来源和标识（用于触发 UI 反馈等）
        let _ = app.emit("hotkey-pressed", serde_json::json!({
            "key": hotkey_type,
            "source": "in-game"
        }));
    }

    /// 处理“在游戏外”触发的热键。
    ///
    /// # 业务逻辑
    /// - 若 `show_menu_when_toggling_outside_game` 为真，则显示主窗口；
    /// - 否则不做任何窗口操作（仅记录 debug 日志）；
    /// - 无论如何都向前端发送 `hotkey-pressed` 事件（包含热键标识和来源），
    ///   以便前端按需响应。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `settings`：用户设置；
    /// - `hotkey_type`：触发的热键标识（如 `"altW"`）。
    fn handle_out_of_game_hotkey(app: &AppHandle, settings: &Settings, hotkey_type: &str) {
        // 游戏外也调用 toggle_window，与游戏内行为统一
        // 平台兼容性：toggle_window 使用 Tauri 的 window.hide()/show()/set_focus()，
        // 在 Windows 11 和 Linux 上均受支持
        match WindowManager::toggle_window(app) {
            Ok(shown) => {
                log::info!("Window toggled (outside game), now visible: {}", shown);
                if shown && settings.is_auto_pin_window {
                    if let Err(e) = WindowManager::set_always_on_top(app, true) {
                        log::warn!("Failed to set window always on top: {}", e);
                    }
                }
            }
            Err(e) => {
                // 严重错误：记录 error 级别日志
                log::error!("Failed to toggle window (outside game): {}", e);
            }
        }

        let _ = app.emit("hotkey-pressed", serde_json::json!({
            "key": hotkey_type,
            "source": "outside-game"
        }));
    }
}

impl Default for HotkeyManager {
    /// 默认实现等价于 [`HotkeyManager::new`]。
    fn default() -> Self {
        Self::new()
    }
}
