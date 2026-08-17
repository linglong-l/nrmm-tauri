//! 全局快捷键管理模块
//!
//! 负责注册和处理全局快捷键：
//! - Ctrl+Alt+1~0: 选择分组 1~10
//! - Ctrl+1~0: 选择模组 1~10
//! - F5: 刷新模组列表
//! - 用户配置的窗口切换热键（默认 Alt+D）
//!
//! # 功能特性
//! - 支持仅在游戏前台时响应热键
//! - 检测游戏前台窗口（跨平台实现）
//! - 模拟按键选择（用于 3Dmigoto 内置菜单）
//! - 游戏未前台时可弹出原生游戏选择菜单（`popup_menu` 方案，在光标位置弹出，不显示主窗口）
//!
//! # 热键决策
//! 窗口切换热键按下时，`handle_window_hotkey` 使用纯函数 `handle_window_hotkey_pure` 做出决策：
//! - 窗口可见 → `Hide`：隐藏窗口
//! - 窗口隐藏 + 前台匹配游戏 → `ShowWithGame`：显示窗口并切换游戏
//! - 窗口隐藏 + 前台未匹配游戏 → `PickGameWithMenu`：弹出原生菜单选择游戏
//!
//! # 平台适配
//! 前台检测和按键模拟通过 `platform` 模块的平台相关实现完成

use crate::config::settings_store;
use crate::models::enums::TargetGame;
use crate::platform;
use anyhow::Result;
use std::sync::{Arc, Mutex};
use tauri::menu::{MenuBuilder, MenuItem};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutEvent};

/// 快捷键管理器
///
/// 管理全局快捷键的注册/注销，持有 `AppHandle` 和已注册快捷键列表。
/// 通过 `Arc<HotkeyManager>` 在 Tauri 状态中共享，确保全局唯一。
pub struct HotkeyManager {
    /// Tauri 应用句柄，用于访问 `global_shortcut()` 插件注册/注销快捷键
    app_handle: AppHandle,
    /// 已注册的快捷键字符串列表（如 `["Ctrl+Alt+1", "Ctrl+1", "F5"]`）
    /// 使用 `Mutex<Vec<String>>` 保证并发安全，多个线程可同时读取
    registered_hotkeys: Mutex<Vec<String>>,
    /// 用户配置的窗口切换热键字符串（如 `Some("Alt+D")`），`None` 表示未配置
    /// 使用 `Mutex<Option<String>>` 保护，支持运行时更新
    window_hotkey: Mutex<Option<String>>,
}

impl HotkeyManager {
    /// 创建新的快捷键管理器
    ///
    /// 初始化时注册列表为空，需要调用 `register_all` 注册所有快捷键。
    pub fn new(app_handle: AppHandle) -> Self {
        HotkeyManager {
            app_handle,
            registered_hotkeys: Mutex::new(Vec::new()),
            window_hotkey: Mutex::new(None),
        }
    }

    /// 注册所有全局快捷键
    ///
    /// # 步骤
    /// 1. 先调用 `unregister_all` 注销所有已注册的快捷键（清空列表）
    /// 2. 注册 `Ctrl+Alt+1~0`：选择分组 1-10（0 表示第 10 组）
    /// 3. 注册 `Ctrl+1~0`：选择模组 1-10（0 表示第 10 个）
    /// 4. 注册 `F5`：刷新模组列表
    /// 5. 注册用户配置的 `window_hotkey`（如 `Alt+D`）
    ///
    /// # Errors
    /// - 快捷键注册失败时返回错误（底层 `global_shortcut().register()` 失败）
    /// - 单个快捷键注册失败不中断整体流程，仅 log warn
    pub fn register_all(&self) -> Result<()> {
        self.unregister_all();

        let gsm = self.app_handle.global_shortcut();

        for i in 1..=10 {
            let key = if i == 10 {
                "0".to_string()
            } else {
                i.to_string()
            };
            let accel = format!("Ctrl+Alt+{}", key);
            self.register_hotkey(gsm, &accel);
        }

        for i in 1..=10 {
            let key = if i == 10 {
                "0".to_string()
            } else {
                i.to_string()
            };
            let accel = format!("Ctrl+{}", key);
            self.register_hotkey(gsm, &accel);
        }

        self.register_hotkey(gsm, "F5");

        // 注册用户配置的窗口切换热键
        let settings = settings_store::get_settings();
        if !settings.window_hotkey.is_empty() {
            self.register_hotkey(gsm, &settings.window_hotkey);
            *crate::utils::lock_or_recover(&self.window_hotkey) = Some(settings.window_hotkey);
        }

        Ok(())
    }

    /// 注册单个快捷键（辅助方法）
    ///
    /// 先检查是否已注册，是则先注销再重新注册。
    /// 注册成功后将快捷键字符串加入 `registered_hotkeys` 列表。
    /// 注册失败仅 log warn，不中断流程。
    ///
    /// # 参数
    /// - `gsm`: 全局快捷键插件实例
    /// - `accel`: 快捷键字符串（如 `"Ctrl+Alt+1"`、`"F5"`）
    fn register_hotkey(
        &self,
        gsm: &tauri_plugin_global_shortcut::GlobalShortcut<tauri::Wry>,
        accel: &str,
    ) {
        if gsm.is_registered(accel) {
            let _ = gsm.unregister(accel);
        }
        match gsm.register(accel) {
            Ok(_) => {
                crate::utils::lock_or_recover(&self.registered_hotkeys).push(accel.to_string());
                log::debug!("Registered hotkey: {}", accel);
            }
            Err(e) => {
                log::warn!("Failed to register hotkey {}: {}", accel, e);
            }
        }
    }

    /// 获取当前注册的窗口切换热键
    ///
    /// 返回 `Some(accel_string)` 表示已配置，`None` 表示未配置。
    /// 用于 `handle_hotkey` 中判断触发快捷键是否匹配窗口热键。
    pub fn get_window_hotkey(&self) -> Option<String> {
        crate::utils::lock_or_recover(&self.window_hotkey).clone()
    }

    /// 注销所有已注册的快捷键
    ///
    /// 调用 `global_shortcut().unregister_all()` 批量注销，然后清空内部列表。
    /// 用于 `register_all` 前的清理，以及应用退出前的资源释放。
    pub fn unregister_all(&self) {
        let gsm = self.app_handle.global_shortcut();
        let _ = gsm.unregister_all();
        crate::utils::lock_or_recover(&self.registered_hotkeys).clear();
        *crate::utils::lock_or_recover(&self.window_hotkey) = None;
    }
}

/// 处理快捷键事件（全局快捷键回调入口）
///
/// 由 `tauri-plugin-global-shortcut` 的事件回调调用。
/// 处理逻辑：
/// 1. 先尝试匹配硬编码快捷键（`shortcut_to_action`）：Ctrl+Alt+N、Ctrl+N、F5
/// 2. 未匹配到硬编码快捷键时，检查是否为用户配置的窗口切换热键
/// 3. 匹配窗口热键则调用 `handle_window_hotkey`
///
/// # 参数
/// - `app_handle`: Tauri 应用句柄
/// - `shortcut`: 触发的快捷键
/// - `_event`: 快捷键事件类型（按下/释放，当前未使用）
pub fn handle_hotkey(app_handle: &AppHandle, shortcut: &Shortcut, _event: &ShortcutEvent) {
    // 先尝试硬编码的快捷键
    if let Some(action) = shortcut_to_action(shortcut) {
        execute_action(app_handle, action);
        return;
    }

    // 检查是否是窗口切换热键
    if let Some(mgr) = app_handle.try_state::<Arc<HotkeyManager>>() {
        if let Some(win_hk) = mgr.get_window_hotkey() {
            if let Ok(parsed) = win_hk.parse::<Shortcut>() {
                if shortcuts_equal(shortcut, &parsed) {
                    handle_window_hotkey(app_handle);
                }
            }
        }
    }
}

/// 处理窗口切换热键
///
/// 使用纯函数 `handle_window_hotkey_pure` 做出决策后执行副作用操作。
///
/// # 逻辑
/// - 如果窗口当前可见 → 隐藏窗口（`hide_main_window`）
/// - 如果窗口当前隐藏 → 检测前台进程名是否匹配某个游戏：
///   * 匹配成功 → emit `window-show-with-game` 事件通知前端切换游戏，随后显示窗口
///   * 未匹配 → 弹出原生游戏选择菜单（`popup_menu` 在光标位置），主窗口保持隐藏
///     菜单项 ID 格式为 `game_menu:{game_id}`，由全局 `on_menu_event` 处理选择结果
///
/// # Emits
/// - `window-show-with-game`（Payload: `TargetGame`）：通知前端切换到指定游戏
///
/// # Panics
/// 不会 panic，所有可能失败的操作（获取窗口、弹出菜单等）均使用 `log::error` 记录
fn handle_window_hotkey(app_handle: &AppHandle) {
    let is_visible = app_handle
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);

    let detector = platform::get_foreground_detector();
    let mut fg_proc = detector.get_foreground_process_name().ok();
    // 首次获取失败时短暂等待后重试一次（覆盖热键按下时窗口切换过渡态）
    if fg_proc.is_none() {
        std::thread::sleep(std::time::Duration::from_millis(50));
        fg_proc = detector.get_foreground_process_name().ok();
    }
    let detected_game = fg_proc.as_deref().and_then(match_process_name_to_game);
    let cursor_pos = detector.get_cursor_position().unwrap_or((0, 0));

    let decision = handle_window_hotkey_pure(is_visible, detected_game, cursor_pos, fg_proc);
    match decision {
        HotkeyDecision::Hide => {
            log::debug!("Window hotkey: hiding window");
            crate::window::hide_main_window(app_handle);
        }
        HotkeyDecision::ShowWithGame(game) => {
            log::info!(
                "Window hotkey: showing window, detected foreground game: {:?}",
                game
            );
            let _ = app_handle.emit("window-show-with-game", game);
            crate::window::show_main_window(app_handle);
        }
        HotkeyDecision::PickGameWithMenu {
            foreground_process_name,
        } => {
            log::info!(
                "[Hotkey] pick-game native menu triggered (fg={:?}), main window stays hidden",
                foreground_process_name
            );
            if let Some(win) = app_handle.get_webview_window("main") {
                match build_game_menu(app_handle, foreground_process_name.as_deref()) {
                    Ok(menu) => {
                        if let Err(e) = win.popup_menu(&menu) {
                            log::error!("[Hotkey] popup_menu failed: {}", e);
                            crate::window::show_main_window(app_handle);
                        }
                    }
                    Err(e) => {
                        log::error!("[Hotkey] build_game_menu failed: {}", e);
                        crate::window::show_main_window(app_handle);
                    }
                }
            } else {
                log::warn!("[Hotkey] main window not found, showing main as fallback");
                crate::window::show_main_window(app_handle);
            }
        }
    }
}

/// 比较两个 `Shortcut` 是否相等（比较 `key` 和 `mods`）
///
/// 不比较 `shortcut` 的字符串表示，直接比较内部字段。
fn shortcuts_equal(a: &Shortcut, b: &Shortcut) -> bool {
    a.key == b.key && a.mods == b.mods
}

/// 快捷键动作类型
///
/// 由 `shortcut_to_action` 将原始快捷键映射为具体动作。
#[derive(Debug, Clone)]
enum HotkeyAction {
    /// 选择分组（参数为分组编号 1-10）
    SelectGroup(u32),
    /// 选择模组（参数为模组索引 0-9，对应 Ctrl+1~0）
    SelectMod(u32),
    /// 刷新模组列表（对应 F5）
    Refresh,
}

/// 将快捷键转换为对应的动作
///
/// 匹配规则（按优先级顺序）：
/// 1. 单独 F5（无修饰键）→ `Refresh`
/// 2. Ctrl+Alt+1~0 → `SelectGroup(1~10)`
/// 3. Ctrl+1~0 → `SelectMod(0~9)`
///
/// 注意：Ctrl+1 对应 `SelectMod(0)`，Ctrl+0 对应 `SelectMod(9)`。
/// 这与 3Dmigoto 的索引约定一致（0-based）。
fn shortcut_to_action(shortcut: &Shortcut) -> Option<HotkeyAction> {
    let mods = shortcut.mods;
    let key = shortcut.key;

    let has_ctrl = mods.contains(Modifiers::CONTROL);
    let has_alt = mods.contains(Modifiers::ALT);
    let has_shift = mods.contains(Modifiers::SHIFT);
    let has_super = mods.contains(Modifiers::SUPER);

    if !has_ctrl && !has_alt && !has_shift && !has_super && key == Code::F5 {
        return Some(HotkeyAction::Refresh);
    }

    if has_ctrl && has_alt && !has_shift && !has_super {
        match key {
            Code::Digit1 => return Some(HotkeyAction::SelectGroup(1)),
            Code::Digit2 => return Some(HotkeyAction::SelectGroup(2)),
            Code::Digit3 => return Some(HotkeyAction::SelectGroup(3)),
            Code::Digit4 => return Some(HotkeyAction::SelectGroup(4)),
            Code::Digit5 => return Some(HotkeyAction::SelectGroup(5)),
            Code::Digit6 => return Some(HotkeyAction::SelectGroup(6)),
            Code::Digit7 => return Some(HotkeyAction::SelectGroup(7)),
            Code::Digit8 => return Some(HotkeyAction::SelectGroup(8)),
            Code::Digit9 => return Some(HotkeyAction::SelectGroup(9)),
            Code::Digit0 => return Some(HotkeyAction::SelectGroup(10)),
            _ => {}
        }
    }

    if has_ctrl && !has_alt && !has_shift && !has_super {
        match key {
            Code::Digit1 => return Some(HotkeyAction::SelectMod(0)),
            Code::Digit2 => return Some(HotkeyAction::SelectMod(1)),
            Code::Digit3 => return Some(HotkeyAction::SelectMod(2)),
            Code::Digit4 => return Some(HotkeyAction::SelectMod(3)),
            Code::Digit5 => return Some(HotkeyAction::SelectMod(4)),
            Code::Digit6 => return Some(HotkeyAction::SelectMod(5)),
            Code::Digit7 => return Some(HotkeyAction::SelectMod(6)),
            Code::Digit8 => return Some(HotkeyAction::SelectMod(7)),
            Code::Digit9 => return Some(HotkeyAction::SelectMod(8)),
            Code::Digit0 => return Some(HotkeyAction::SelectMod(9)),
            _ => {}
        }
    }

    None
}

/// 执行快捷键动作
///
/// 根据用户设置判断是否仅在游戏前台时响应。
/// - `hotkey_only_in_migoto = true` 且游戏不在前台 → 如果 `always_show_menu_on_hotkey = true` 则弹出游戏选择菜单，否则忽略
/// - 游戏在前台或无限制 → 执行相应的按键模拟或事件发射
///
/// # Emits
/// - `hotkey-refresh`（Payload: `()`）：F5 刷新快捷键触发时 emit
fn execute_action(app_handle: &AppHandle, action: HotkeyAction) {
    log::debug!("[hotkey] [execute_action] action={:?}", action);
    let detector = platform::get_foreground_detector();
    let settings = settings_store::get_settings();

    let game = settings.target_game;
    let only_in_game = settings.hotkey_only_in_migoto;
    let fallback_to_menu = settings.always_show_menu_on_hotkey;

    // 如果设置了仅在游戏内响应，且游戏不在前台
    if only_in_game && !detector.is_game_foreground(game) {
        if fallback_to_menu {
            show_game_select_menu(app_handle, &action);
        }
        return;
    }

    let mut simulator = platform::get_key_simulator();
    if only_in_game {
        let process_names = game.process_names();
        if let Some(first_pn) = process_names.first() {
            let _ = simulator.set_target_process(first_pn);
        }
    }
    match action {
        HotkeyAction::SelectGroup(_gid) => {
            log::debug!("Hotkey triggered: SelectGroup");
            if let Err(e) = simulator.simulate_select_group() {
                log::error!("Failed to simulate select group: {}", e);
            }
        }
        HotkeyAction::SelectMod(_midx) => {
            log::debug!("Hotkey triggered: SelectMod");
            if let Err(e) = simulator.simulate_select_mod() {
                log::error!("Failed to simulate select mod: {}", e);
            }
        }
        HotkeyAction::Refresh => {
            log::debug!("Hotkey triggered: Refresh");
            let _ = app_handle.emit("hotkey-refresh", ());
        }
    }
}

/// 构建游戏选择原生菜单（6 个游戏 + 分隔符）
///
/// 菜单项 ID 格式：`game_menu:{game_id}`，供全局 `on_menu_event` 回调解析。
/// 菜单在光标位置弹出（通过 `win.popup_menu`），不显示主窗口。
///
/// 支持的 6 个游戏：
/// - GenshinImpact（原神）
/// - HonkaiStarRail（崩坏：星穹铁道）
/// - ZZZ（绝区零）
/// - Wuwa（鸣潮）
/// - HonkaiImpact3rd（崩坏 3rd）
/// - ArknightsEndfield（明日方舟：终末地）
///
/// # 参数
/// - `manager`: Tauri Manager 实例（用于构建 Menu）
/// - `foreground_process_name`: 当前前台进程名（仅用于日志记录）
///
/// # Errors
/// - `MenuItem::with_id` 或 `MenuBuilder::build` 构建失败时返回 Tauri 错误
fn build_game_menu<R: tauri::Runtime, M: tauri::Manager<R>>(
    manager: &M,
    foreground_process_name: Option<&str>,
) -> tauri::Result<tauri::menu::Menu<R>> {
    let mut builder = MenuBuilder::new(manager);

    if let Some(pn) = foreground_process_name {
        log::info!("[Hotkey] pick-game menu: foreground process = {}", pn);
    }

    let games_order = [
        ("game_menu:GenshinImpact", "原神 (Genshin Impact)"),
        (
            "game_menu:HonkaiStarRail",
            "崩坏：星穹铁道 (Honkai: Star Rail)",
        ),
        ("game_menu:ZZZ", "绝区零 (Zenless Zone Zero)"),
        ("game_menu:Wuwa", "鸣潮 (Wuthering Waves)"),
        ("game_menu:HonkaiImpact3rd", "崩坏 3rd (Honkai Impact 3rd)"),
        (
            "game_menu:ArknightsEndfield",
            "明日方舟：终末地 (Arknights: Endfield)",
        ),
    ];

    for (id, label) in games_order {
        let item = MenuItem::with_id(manager, id, label, true, None::<&str>)?;
        builder = builder.item(&item);
    }

    builder.build()
}

/// 显示游戏选择菜单（在光标位置弹出）
///
/// 通过 emit `show-game-select` 事件通知前端在指定坐标弹出游戏选择 UI。
/// 用于 `always_show_menu_on_hotkey` 设置开启时，快捷键在游戏外触发时弹出选择菜单。
///
/// # Emits
/// - `show-game-select`（Payload: `{ x, y, action }`）：通知前端在 `(x, y)` 位置显示游戏选择菜单
fn show_game_select_menu(app_handle: &AppHandle, action: &HotkeyAction) {
    let detector = platform::get_foreground_detector();
    let (x, y) = detector.get_cursor_position().unwrap_or((100, 100));
    let _ = app_handle.emit(
        "show-game-select",
        serde_json::json!({
            "x": x,
            "y": y,
            "action": format!("{:?}", action),
        }),
    );
}

/// 重新注册所有快捷键（Tauri 命令）
///
/// 前端调用 `reregisterHotkeys` 触发。
/// 如果传入了新的 `window_hotkey`，先更新到设置中再重新注册所有快捷键。
///
/// # Errors
/// - 快捷键注册失败时返回错误
#[tauri::command]
pub fn reregister_hotkeys(
    hotkey_mgr: State<'_, Arc<HotkeyManager>>,
    window_hotkey: Option<String>,
) -> Result<(), String> {
    // 如果传入了新的窗口热键，先更新设置中的值（让 register_all 读取到最新值）
    if let Some(ref hk) = window_hotkey {
        let mut settings = settings_store::get_settings();
        settings.window_hotkey = hk.clone();
        if let Err(e) = settings_store::save_settings(&settings) {
            log::warn!("Failed to save window hotkey to settings: {}", e);
        }
    }
    hotkey_mgr.register_all().map_err(|e| e.to_string())
}

/// 注销所有快捷键（Tauri 命令）
///
/// 前端调用 `unregisterHotkeys` 触发。
/// 用于应用退出前或切换配置时清理资源。
#[tauri::command]
pub fn unregister_hotkeys(hotkey_mgr: State<'_, Arc<HotkeyManager>>) -> Result<(), String> {
    hotkey_mgr.unregister_all();
    Ok(())
}

/// 模拟选择分组按键（Tauri 命令）
///
/// 前端调用 `simulateSelectGroup` 触发。
/// 用于 3Dmigoto 内置菜单的按键模拟。
///
/// # Errors
/// - 按键模拟失败（平台相关，如权限不足）
#[tauri::command]
pub fn simulate_select_group() -> Result<(), String> {
    let mut simulator = platform::get_key_simulator();
    simulator.simulate_select_group().map_err(|e| e.to_string())
}

/// 模拟选择模组按键（Tauri 命令）
///
/// 前端调用 `simulateSelectMod` 触发。
/// 用于 3Dmigoto 内置菜单的按键模拟。
///
/// # Errors
/// - 按键模拟失败（平台相关，如权限不足）
#[tauri::command]
pub fn simulate_select_mod() -> Result<(), String> {
    let mut simulator = platform::get_key_simulator();
    simulator.simulate_select_mod().map_err(|e| e.to_string())
}

/// 检查按键模拟支持情况（Tauri 命令）
///
/// 前端调用 `checkKeypressSupport` 触发。
/// 检查当前平台是否支持按键模拟（如 macOS 需要辅助功能权限）。
///
/// # Errors
/// - 平台不支持按键模拟时返回错误
#[tauri::command]
pub fn check_keypress_support() -> Result<(), String> {
    let simulator = platform::get_key_simulator();
    simulator.check_support()
}

/// 检查指定游戏是否在前台（Tauri 命令）
///
/// 前端调用 `isGameForeground` 触发。
/// 先通过 `parse_game` 解析游戏字符串，再用前台检测器判断。
///
/// # 参数
/// - `game`: 游戏字符串（如 `"GenshinImpact"`, `"StarRail"`）
///
/// # 返回
/// `true` 表示该游戏窗口当前在前台
#[tauri::command]
pub fn is_game_foreground(game: String) -> bool {
    let game_enum = match parse_game(&game) {
        Ok(g) => g,
        Err(_) => return false,
    };
    let detector = platform::get_foreground_detector();
    detector.is_game_foreground(game_enum)
}

/// 获取光标位置（Tauri 命令）
///
/// 前端调用 `getCursorPosition` 触发。
/// 用于定位 `popup_menu` 弹出位置。
///
/// # 返回
/// `(x, y)` 屏幕坐标（左上角为原点）
///
/// # Errors
/// - 获取光标位置失败（平台相关）
#[tauri::command]
pub fn get_cursor_position() -> Result<(i32, i32), String> {
    let detector = platform::get_foreground_detector();
    detector.get_cursor_position().map_err(|e| e.to_string())
}

/// 解析游戏字符串为 `TargetGame` 枚举
///
/// 支持多种别名，大小写不敏感：
/// - `"genshinimpact"`, `"genshin"`, `"gi"` → `GenshinImpact`
/// - `"honkaistarrail"`, `"starrail"`, `"hsr"` → `HonkaiStarRail`
/// - `"wuwa"`, `"wutheringwaves"` → `Wuwa`
/// - `"zzz"`, `"zenlesszonezero"` → `ZZZ`
/// - `"honkaiimpact3rd"`, `"hi3"` → `HonkaiImpact3rd`
/// - `"arknightsendfield"`, `"endfield"`, `"af"`, `"arknights endfield"` → `ArknightsEndfield`
///
/// `pub(crate)` 可见性供 `lib.rs` 的 `on_menu_event` 回调使用。
///
/// # 参数
/// - `game`: 游戏字符串
///
/// # 返回
/// `Ok(TargetGame)` 或 `Err(String)`（未知游戏名）
///
/// # Panics
/// 不会 panic
pub(crate) fn parse_game(game: &str) -> Result<TargetGame, String> {
    match game.to_lowercase().as_str() {
        "genshinimpact" | "genshin" | "gi" => Ok(TargetGame::GenshinImpact),
        "honkaistarrail" | "starrail" | "hsr" => Ok(TargetGame::HonkaiStarRail),
        "wuwa" | "wutheringwaves" => Ok(TargetGame::Wuwa),
        "zzz" | "zenlesszonezero" => Ok(TargetGame::ZZZ),
        "honkaiimpact3rd" | "hi3" => Ok(TargetGame::HonkaiImpact3rd),
        "arknightsendfield" | "endfield" | "af" | "arknights endfield" => {
            Ok(TargetGame::ArknightsEndfield)
        }
        _ => Err(format!("Unknown game: {}", game)),
    }
}

/// 将进程名字符串匹配到目标游戏（大小写不敏感）
///
/// 提取为纯函数以便单元测试。
///
/// # 参数
/// * `proc_name` - 前台窗口进程名（大小写任意，大小写不敏感匹配，如 `"StarRail.exe"`）
///
/// # 返回值
/// 匹配成功返回 `Some(TargetGame)`，未匹配返回 `None`
///
/// # Panics
/// 不会 panic
///
/// # Examples
/// ```ignore
/// // 内部辅助函数，使用 TargetGame::from_process_name(process_name) 作为公开 API
/// // 本函数仅为模块内部单元测试用例，无需 Doc-Test 执行
/// assert_eq!(match_process_name_to_game("StarRail.exe"), Some(TargetGame::HonkaiStarRail));
/// assert_eq!(match_process_name_to_game("notepad.exe"), None);
/// ```
fn match_process_name_to_game(proc_name: &str) -> Option<TargetGame> {
    let lower = proc_name.to_lowercase();
    for game in TargetGame::all().iter() {
        for pn in game.process_names().iter() {
            if pn.to_lowercase() == lower {
                return Some(*game);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试进程名匹配：已知游戏进程名应正确返回对应的 `Some(game)`
    ///
    /// 验证大小写不敏感匹配：
    /// - `"StarRail.exe"` → `HonkaiStarRail`
    /// - `"starrail.exe"` → `HonkaiStarRail`（全小写）
    /// - `"YuanShen.exe"` → `GenshinImpact`
    /// - `"ZenlessZoneZero.exe"` → `ZZZ`
    #[test]
    fn test_match_process_name_matches_known_games_case_insensitive() {
        assert_eq!(
            match_process_name_to_game("StarRail.exe"),
            Some(TargetGame::HonkaiStarRail)
        );
        assert_eq!(
            match_process_name_to_game("starrail.exe"),
            Some(TargetGame::HonkaiStarRail)
        );
        assert_eq!(
            match_process_name_to_game("YuanShen.exe"),
            Some(TargetGame::GenshinImpact)
        );
        assert_eq!(
            match_process_name_to_game("ZenlessZoneZero.exe"),
            Some(TargetGame::ZZZ)
        );
    }

    /// 测试进程名匹配：非游戏进程（如记事本、资源管理器、浏览器）应返回 `None`
    ///
    /// 验证负向匹配：
    /// - `"notepad.exe"` → `None`
    /// - `"explorer.exe"` → `None`
    /// - `"chrome.exe"` → `None`
    /// - 空字符串 `""` → `None`
    #[test]
    fn test_match_process_name_returns_none_for_non_game() {
        assert_eq!(match_process_name_to_game("notepad.exe"), None);
        assert_eq!(match_process_name_to_game("explorer.exe"), None);
        assert_eq!(match_process_name_to_game("chrome.exe"), None);
        assert_eq!(match_process_name_to_game(""), None);
    }

    /// 纯函数窗口热键决策：窗口可见时决策应为 `Hide`
    ///
    /// 验证：`is_visible = true`，无论前台进程是什么，都应返回 `Hide`。
    #[test]
    fn test_hotkey_decision_visible_hides() {
        let r = handle_window_hotkey_pure(true, None, (0, 0), None);
        assert!(matches!(r, HotkeyDecision::Hide));
    }

    /// 纯函数窗口热键决策：窗口不可见 + 前台匹配 `StarRail.exe` → `ShowWithGame(HonkaiStarRail)`
    ///
    /// 验证：`is_visible = false`，`detected_game = Some(HonkaiStarRail)`，
    /// 应返回 `ShowWithGame(HonkaiStarRail)`，保留进程名用于 UI 展示。
    #[test]
    fn test_hotkey_decision_invisible_with_game_shows_game() {
        let r = handle_window_hotkey_pure(
            false,
            Some(TargetGame::HonkaiStarRail),
            (500, 500),
            Some("StarRail.exe".to_string()),
        );
        assert!(matches!(
            r,
            HotkeyDecision::ShowWithGame(TargetGame::HonkaiStarRail)
        ));
    }

    /// 纯函数窗口热键决策：窗口不可见 + 前台未匹配（`notepad.exe`）→ `PickGameWithMenu`
    ///
    /// 验证：`is_visible = false`，`detected_game = None`，
    /// 应返回 `PickGameWithMenu { foreground_process_name: Some("notepad.exe") }`，
    /// 仅保留进程名，不包含游戏信息。
    #[test]
    fn test_hotkey_decision_invisible_no_game_triggers_pick() {
        let r = handle_window_hotkey_pure(false, None, (123, 456), Some("notepad.exe".to_string()));
        match r {
            HotkeyDecision::PickGameWithMenu {
                foreground_process_name,
            } => {
                assert_eq!(foreground_process_name.as_deref(), Some("notepad.exe"));
            }
            other => panic!("Expected PickGameWithMenu, got {:?}", other),
        }
    }
}

/// 纯函数式的窗口热键决策结果枚举，用于 TDD 单元测试
///
/// 说明：`handle_window_hotkey` 内部逻辑（不包括 AppHandle 操作）由纯函数 `handle_window_hotkey_pure`
/// 产出决策，真实的 `emit`/`show`/`hide` 操作在外部按此决策执行。
#[derive(Debug, PartialEq)]
enum HotkeyDecision {
    /// 窗口可见 → 隐藏窗口（不 emit 任何事件）
    Hide,
    /// 窗口隐藏 + 前台匹配到游戏 → 显示窗口并切换到该游戏
    ShowWithGame(TargetGame),
    /// 窗口隐藏 + 前台未匹配到游戏 → 弹出原生 `popup_menu` 让用户选择游戏
    /// 主窗口保持隐藏，菜单在光标位置弹出，`foreground_process_name` 用于 UI 展示
    PickGameWithMenu {
        foreground_process_name: Option<String>,
    },
}

/// 窗口热键逻辑的纯函数核心（TDD 可测试）
///
/// 只做决策判断，不做副作用操作，避免测试时 mock `AppHandle`。
///
/// # 决策矩阵
/// | is_visible | detected_game | 决策 |
/// |------------|---------------|------|
/// | `true`     | 任意          | `Hide` |
/// | `false`    | `Some(game)`  | `ShowWithGame(game)` |
/// | `false`    | `None`        | `PickGameWithMenu { foreground_process_name }` |
///
/// # 参数
/// * `is_visible` - 窗口当前是否可见
/// * `detected_game` - 前台进程匹配到的游戏（`None` = 未匹配到任何游戏）
/// * `cursor_pos` - 当前光标屏幕坐标 (x, y)（当前未使用，保留供将来扩展）
/// * `foreground_process_name` - 当前前台进程名（有则传递，用于 UI 提示）
///
/// # 返回值
/// 热键下一步动作：`Hide` / `ShowWithGame` / `PickGameWithMenu`
///
/// # Panics
/// 不会 panic
fn handle_window_hotkey_pure(
    is_visible: bool,
    detected_game: Option<TargetGame>,
    cursor_pos: (i32, i32),
    foreground_process_name: Option<String>,
) -> HotkeyDecision {
    let _ = cursor_pos;
    if is_visible {
        HotkeyDecision::Hide
    } else if let Some(game) = detected_game {
        HotkeyDecision::ShowWithGame(game)
    } else {
        HotkeyDecision::PickGameWithMenu {
            foreground_process_name,
        }
    }
}
