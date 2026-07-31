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
//! - 游戏未前台时可显示快速选择菜单
//!
//! # 平台适配
//! 前台检测和按键模拟通过 platform 模块的平台相关实现完成

use tauri::{AppHandle, State, Emitter, Manager};
use std::sync::{Arc, Mutex};
use anyhow::Result;
use serde::Serialize;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, Modifiers, Code};
use crate::platform;
use crate::config::settings_store;
use crate::models::enums::TargetGame;

/// 快捷键管理器
///
/// 管理全局快捷键的注册/注销，持有 AppHandle 和已注册快捷键列表
pub struct HotkeyManager {
    app_handle: AppHandle,
    registered_hotkeys: Mutex<Vec<String>>,
    window_hotkey: Mutex<Option<String>>,
}

impl HotkeyManager {
    /// 创建新的快捷键管理器
    pub fn new(app_handle: AppHandle) -> Self {
        HotkeyManager {
            app_handle,
            registered_hotkeys: Mutex::new(Vec::new()),
            window_hotkey: Mutex::new(None),
        }
    }

    /// 注册所有全局快捷键
    ///
    /// 先注销所有已注册的快捷键，再重新注册：
    /// - Ctrl+Alt+1~0 (选择分组 1-10，0 表示第 10 组)
    /// - Ctrl+1~0 (选择模组 1-10，0 表示第 10 个)
    /// - F5 (刷新)
    /// - windowHotkey (用户配置的窗口切换热键)
    pub fn register_all(&self) -> Result<()> {
        self.unregister_all();

        let gsm = self.app_handle.global_shortcut();

        for i in 1..=10 {
            let key = if i == 10 { "0".to_string() } else { i.to_string() };
            let accel = format!("Ctrl+Alt+{}", key);
            self.register_hotkey(&gsm, &accel);
        }

        for i in 1..=10 {
            let key = if i == 10 { "0".to_string() } else { i.to_string() };
            let accel = format!("Ctrl+{}", key);
            self.register_hotkey(&gsm, &accel);
        }

        self.register_hotkey(&gsm, "F5");

        // 注册用户配置的窗口切换热键
        let settings = settings_store::get_settings();
        if !settings.window_hotkey.is_empty() {
            self.register_hotkey(&gsm, &settings.window_hotkey);
            *self.window_hotkey.lock().unwrap() = Some(settings.window_hotkey);
        }

        Ok(())
    }

    /// 注册单个快捷键（辅助方法）
    fn register_hotkey(&self, gsm: &tauri_plugin_global_shortcut::GlobalShortcut<tauri::Wry>, accel: &str) {
        if gsm.is_registered(accel) {
            let _ = gsm.unregister(accel);
        }
        match gsm.register(accel) {
            Ok(_) => {
                self.registered_hotkeys.lock().unwrap().push(accel.to_string());
                log::debug!("Registered hotkey: {}", accel);
            }
            Err(e) => {
                log::warn!("Failed to register hotkey {}: {}", accel, e);
            }
        }
    }

    /// 获取当前注册的窗口热键
    pub fn get_window_hotkey(&self) -> Option<String> {
        self.window_hotkey.lock().unwrap().clone()
    }

    /// 注销所有已注册的快捷键
    pub fn unregister_all(&self) {
        let gsm = self.app_handle.global_shortcut();
        let _ = gsm.unregister_all();
        self.registered_hotkeys.lock().unwrap().clear();
        *self.window_hotkey.lock().unwrap() = None;
    }
}

/// 处理快捷键事件（全局快捷键回调）
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
                    return;
                }
            }
        }
    }
}

/// 热键触发「显示窗口」时前台进程未匹配到游戏 -> 通知前端弹游戏选择菜单的事件载荷
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NeedPickGamePayload {
    /// 光标屏幕坐标 X
    cursor_x: i32,
    /// 光标屏幕坐标 Y
    cursor_y: i32,
    /// 当前前台窗口的进程名，有则展示给用户提示，没有则为 None
    foreground_process_name: Option<String>,
}

/// 处理窗口切换热键
///
/// 逻辑：
/// - 如果窗口当前可见 → 隐藏窗口（发出 window-hidden 事件）
/// - 如果窗口当前隐藏 → 先检测前台进程名是否匹配某个游戏：
///   * 匹配成功 -> 发出 "window-show-with-game" 事件通知前端切换游戏，随后显示窗口
///   * 未匹配   -> 发出 "need-pick-game" 事件，携带光标坐标+前台进程名，前端展示选择菜单，随后显示窗口
fn handle_window_hotkey(app_handle: &AppHandle) {
    let is_visible = app_handle
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);

    // 先收集纯输入（副作用前置），再交给纯函数决策
    let detector = platform::get_foreground_detector();
    let fg_proc = detector.get_foreground_process_name().ok();
    let detected_game = fg_proc
        .as_deref()
        .and_then(|s| match_process_name_to_game(s));
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
        HotkeyDecision::ShowAndPickGame {
            cursor_x,
            cursor_y,
            foreground_process_name,
        } => {
            log::debug!(
                "Window hotkey: showing window, no game matched (fg={:?}, pos=({},{})) -> emitting need-pick-game",
                foreground_process_name, cursor_x, cursor_y
            );
            let payload = NeedPickGamePayload {
                cursor_x,
                cursor_y,
                foreground_process_name,
            };
            let _ = app_handle.emit("need-pick-game", payload);
            crate::window::show_main_window(app_handle);
        }
    }
}

/// 比较两个 Shortcut 是否相等（比较 key 和 modifiers）
fn shortcuts_equal(a: &Shortcut, b: &Shortcut) -> bool {
    a.key == b.key && a.mods == b.mods
}

/// 快捷键动作类型
#[derive(Debug, Clone)]
enum HotkeyAction {
    SelectGroup(u32),
    SelectMod(u32),
    Refresh,
}

/// 将快捷键转换为对应的动作
fn shortcut_to_action(shortcut: &Shortcut) -> Option<HotkeyAction> {
    let mods = shortcut.mods;
    let key = shortcut.key;

    let has_ctrl = mods.contains(Modifiers::CONTROL);
    let has_alt = mods.contains(Modifiers::ALT);
    let has_shift = mods.contains(Modifiers::SHIFT);
    let has_super = mods.contains(Modifiers::SUPER);

    if !has_ctrl && !has_alt && !has_shift && !has_super {
        if key == Code::F5 {
            return Some(HotkeyAction::Refresh);
        }
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
fn execute_action(app_handle: &AppHandle, action: HotkeyAction) {
    let detector = platform::get_foreground_detector();
    let settings = settings_store::get_settings();

    let game = settings.target_game;
    let only_in_game = settings.hotkey_only_in_migoto;
    let fallback_to_menu = settings.always_show_menu_on_hotkey;

    // 如果设置了仅在游戏内响应，且游戏不在前台
    if only_in_game {
        if !detector.is_game_foreground(game) {
            if fallback_to_menu {
                show_game_select_menu(app_handle, &action);
            }
            return;
        }
    }

    let simulator = platform::get_key_simulator();
    match action {
        HotkeyAction::SelectGroup(gid) => {
            log::debug!("Hotkey triggered: SelectGroup {}", gid);
            if let Err(e) = simulator.simulate_select_group() {
                log::error!("Failed to simulate select group: {}", e);
            }
        }
        HotkeyAction::SelectMod(midx) => {
            log::debug!("Hotkey triggered: SelectMod {}", midx);
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

/// 显示游戏选择菜单（在光标位置弹出）
fn show_game_select_menu(app_handle: &AppHandle, action: &HotkeyAction) {
    let detector = platform::get_foreground_detector();
    let (x, y) = detector.get_cursor_position().unwrap_or((100, 100));
    let _ = app_handle.emit("show-game-select", serde_json::json!({
        "x": x,
        "y": y,
        "action": format!("{:?}", action),
    }));
}

/// 重新注册所有快捷键（Tauri 命令）
#[tauri::command]
pub fn reregister_hotkeys(hotkey_mgr: State<'_, Arc<HotkeyManager>>, window_hotkey: Option<String>) -> Result<(), String> {
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
#[tauri::command]
pub fn unregister_hotkeys(hotkey_mgr: State<'_, Arc<HotkeyManager>>) -> Result<(), String> {
    hotkey_mgr.unregister_all();
    Ok(())
}

/// 模拟选择分组按键（Tauri 命令）
#[tauri::command]
pub fn simulate_select_group() -> Result<(), String> {
    let simulator = platform::get_key_simulator();
    simulator.simulate_select_group().map_err(|e| e.to_string())
}

/// 模拟选择模组按键（Tauri 命令）
#[tauri::command]
pub fn simulate_select_mod() -> Result<(), String> {
    let simulator = platform::get_key_simulator();
    simulator.simulate_select_mod().map_err(|e| e.to_string())
}

/// 检查按键模拟支持情况（Tauri 命令）
#[tauri::command]
pub fn check_keypress_support() -> Result<(), String> {
    let simulator = platform::get_key_simulator();
    simulator.check_support()
}

/// 检查游戏是否在前台（Tauri 命令）
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
#[tauri::command]
pub fn get_cursor_position() -> Result<(i32, i32), String> {
    let detector = platform::get_foreground_detector();
    detector.get_cursor_position().map_err(|e| e.to_string())
}

/// 解析游戏字符串为 TargetGame 枚举
fn parse_game(game: &str) -> Result<TargetGame, String> {
    match game.to_lowercase().as_str() {
        "genshinimpact" | "genshin" | "gi" => Ok(TargetGame::GenshinImpact),
        "honkaistarrail" | "starrail" | "hsr" => Ok(TargetGame::HonkaiStarRail),
        "wuwa" | "wutheringwaves" => Ok(TargetGame::Wuwa),
        "zzz" | "zenlesszonezero" => Ok(TargetGame::ZZZ),
        "honkaiimpact3rd" | "hi3" => Ok(TargetGame::HonkaiImpact3rd),
        "arknightsendfield" | "endfield" | "af" | "arknights endfield" => Ok(TargetGame::ArknightsEndfield),
        _ => Err(format!("Unknown game: {}", game)),
    }
}

/// 将进程名字符串匹配到目标游戏（大小写不敏感）
///
/// 提取为纯函数以便单元测试。
///
/// # 参数
///
/// * `proc_name` - 前台窗口进程名（大小写任意，大小写不敏感匹配）
///
/// # 返回值
///
/// 匹配成功返回 `Some(TargetGame)`，未匹配返回 `None`
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

    /// 测试进程名匹配：已知游戏进程名 -> 正确返回 Some(game)
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

    /// 测试进程名匹配：非游戏进程（如记事本）返回 None
    #[test]
    fn test_match_process_name_returns_none_for_non_game() {
        assert_eq!(match_process_name_to_game("notepad.exe"), None);
        assert_eq!(match_process_name_to_game("explorer.exe"), None);
        assert_eq!(match_process_name_to_game("chrome.exe"), None);
        assert_eq!(match_process_name_to_game(""), None);
    }

    /// 纯函数窗口热键决策：可见=true → Hide
    #[test]
    fn test_hotkey_decision_visible_hides() {
        let r = handle_window_hotkey_pure(true, None, (0, 0), None);
        assert!(matches!(r, HotkeyDecision::Hide));
    }

    /// 纯函数窗口热键决策：不可见 + 前台匹配 StarRail.exe → ShowWithGame(HonkaiStarRail)
    #[test]
    fn test_hotkey_decision_invisible_with_game_shows_game() {
        let r = handle_window_hotkey_pure(
            false,
            Some(TargetGame::HonkaiStarRail),
            (500, 500),
            Some("StarRail.exe".to_string()),
        );
        assert!(matches!(r, HotkeyDecision::ShowWithGame(TargetGame::HonkaiStarRail)));
    }

    /// 纯函数窗口热键决策：不可见 + 前台未匹配（notepad.exe）→ ShowAndPickGame，保留坐标和进程名
    #[test]
    fn test_hotkey_decision_invisible_no_game_triggers_pick() {
        let r = handle_window_hotkey_pure(
            false,
            None,
            (123, 456),
            Some("notepad.exe".to_string()),
        );
        match r {
            HotkeyDecision::ShowAndPickGame {
                cursor_x,
                cursor_y,
                foreground_process_name,
            } => {
                assert_eq!(cursor_x, 123);
                assert_eq!(cursor_y, 456);
                assert_eq!(foreground_process_name.as_deref(), Some("notepad.exe"));
            }
            other => panic!("Expected ShowAndPickGame, got {:?}", other),
        }
    }
}

/// 纯函数式的窗口热键决策结果枚举，用于 TDD 单元测试
///
/// 说明：handle_window_hotkey 内部逻辑（不包括 AppHandle 操作）由纯函数 handle_window_hotkey_pure
/// 产出决策，真实的 emit/show/hide 操作在外部按此决策执行
#[derive(Debug, PartialEq)]
enum HotkeyDecision {
    Hide,
    ShowWithGame(TargetGame),
    ShowAndPickGame {
        cursor_x: i32,
        cursor_y: i32,
        foreground_process_name: Option<String>,
    },
}

/// 窗口热键逻辑的纯函数核心（TDD 可测试）
///
/// 只做决策判断，不做副作用操作，避免测试时 mock AppHandle
///
/// # 参数
/// * `is_visible` - 窗口当前是否可见
/// * `detected_game` - 前台进程匹配到的游戏（None = 未匹配到任何游戏）
/// * `cursor_pos` - 当前光标屏幕坐标 (x, y)
/// * `foreground_process_name` - 当前前台进程名（有则传递，用于 UI 提示）
///
/// # 返回值
/// 热键下一步动作：`Hide` / `ShowWithGame` / `ShowAndPickGame`
fn handle_window_hotkey_pure(
    is_visible: bool,
    detected_game: Option<TargetGame>,
    cursor_pos: (i32, i32),
    foreground_process_name: Option<String>,
) -> HotkeyDecision {
    if is_visible {
        HotkeyDecision::Hide
    } else if let Some(game) = detected_game {
        HotkeyDecision::ShowWithGame(game)
    } else {
        HotkeyDecision::ShowAndPickGame {
            cursor_x: cursor_pos.0,
            cursor_y: cursor_pos.1,
            foreground_process_name,
        }
    }
}
