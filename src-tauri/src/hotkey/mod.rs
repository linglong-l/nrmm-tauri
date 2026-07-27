use tauri::{AppHandle, State, Emitter};
use std::sync::{Arc, Mutex};
use anyhow::Result;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, Modifiers, Code};
use crate::platform;
use crate::config::settings_store;
use crate::models::enums::TargetGame;

pub struct HotkeyManager {
    app_handle: AppHandle,
    registered_hotkeys: Mutex<Vec<String>>,
}

impl HotkeyManager {
    pub fn new(app_handle: AppHandle) -> Self {
        HotkeyManager {
            app_handle,
            registered_hotkeys: Mutex::new(Vec::new()),
        }
    }

    pub fn register_all(&self) -> Result<()> {
        self.unregister_all();

        let gsm = self.app_handle.global_shortcut();

        for i in 1..=10 {
            let key = if i == 10 { "0".to_string() } else { i.to_string() };
            let accel = format!("Ctrl+Alt+{}", key);
            if gsm.is_registered(accel.as_str()) {
                let _ = gsm.unregister(accel.as_str());
            }
            match gsm.register(accel.as_str()) {
                Ok(_) => {
                    self.registered_hotkeys.lock().unwrap().push(accel);
                }
                Err(e) => {
                    log::warn!("Failed to register hotkey {}: {}", accel, e);
                }
            }
        }

        for i in 1..=10 {
            let key = if i == 10 { "0".to_string() } else { i.to_string() };
            let accel = format!("Ctrl+{}", key);
            if gsm.is_registered(accel.as_str()) {
                let _ = gsm.unregister(accel.as_str());
            }
            match gsm.register(accel.as_str()) {
                Ok(_) => {
                    self.registered_hotkeys.lock().unwrap().push(accel);
                }
                Err(e) => {
                    log::warn!("Failed to register hotkey {}: {}", accel, e);
                }
            }
        }

        let accel = "F5";
        if gsm.is_registered(accel) {
            let _ = gsm.unregister(accel);
        }
        match gsm.register(accel) {
            Ok(_) => {
                self.registered_hotkeys.lock().unwrap().push(accel.to_string());
            }
            Err(e) => {
                log::warn!("Failed to register hotkey {}: {}", accel, e);
            }
        }

        Ok(())
    }

    pub fn unregister_all(&self) {
        let gsm = self.app_handle.global_shortcut();
        let _ = gsm.unregister_all();
        self.registered_hotkeys.lock().unwrap().clear();
    }
}

pub fn handle_hotkey(app_handle: &AppHandle, shortcut: &Shortcut, _event: &ShortcutEvent) {
    if let Some(action) = shortcut_to_action(shortcut) {
        execute_action(app_handle, action);
    }
}

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

fn execute_action(app_handle: &AppHandle, action: HotkeyAction) {
    let detector = platform::get_foreground_detector();
    let settings = settings_store::get_settings();

    let game = settings.target_game;
    let only_in_game = settings.hotkey_only_in_migoto;
    let fallback_to_menu = settings.always_show_menu_on_hotkey;

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
            let _ = app_handle.emit("hotkey-refresh", ());
        }
    }
}

fn show_game_select_menu(app_handle: &AppHandle, action: &HotkeyAction) {
    let detector = platform::get_foreground_detector();
    let (x, y) = detector.get_cursor_position().unwrap_or((100, 100));
    let _ = app_handle.emit("show-game-select", serde_json::json!({
        "x": x,
        "y": y,
        "action": format!("{:?}", action),
    }));
}

#[derive(Debug, Clone)]
enum HotkeyAction {
    SelectGroup(u32),
    SelectMod(u32),
    Refresh,
}

#[tauri::command]
pub fn reregister_hotkeys(hotkey_mgr: State<'_, Arc<HotkeyManager>>) -> Result<(), String> {
    hotkey_mgr.register_all().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn unregister_hotkeys(hotkey_mgr: State<'_, Arc<HotkeyManager>>) -> Result<(), String> {
    hotkey_mgr.unregister_all();
    Ok(())
}

#[tauri::command]
pub fn simulate_select_group() -> Result<(), String> {
    let simulator = platform::get_key_simulator();
    simulator.simulate_select_group().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn simulate_select_mod() -> Result<(), String> {
    let simulator = platform::get_key_simulator();
    simulator.simulate_select_mod().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn check_keypress_support() -> Result<(), String> {
    let simulator = platform::get_key_simulator();
    simulator.check_support()
}

#[tauri::command]
pub fn is_game_foreground(game: String) -> bool {
    let game_enum = match parse_game(&game) {
        Ok(g) => g,
        Err(_) => return false,
    };
    let detector = platform::get_foreground_detector();
    detector.is_game_foreground(game_enum)
}

#[tauri::command]
pub fn get_cursor_position() -> Result<(i32, i32), String> {
    let detector = platform::get_foreground_detector();
    detector.get_cursor_position().map_err(|e| e.to_string())
}

fn parse_game(game: &str) -> Result<TargetGame, String> {
    match game.to_lowercase().as_str() {
        "genshinimpact" | "genshin" | "gi" => Ok(TargetGame::GenshinImpact),
        "honkaistarrail" | "starrail" | "hsr" => Ok(TargetGame::HonkaiStarRail),
        "wuwa" | "wutheringwaves" => Ok(TargetGame::Wuwa),
        "zzz" | "zenlesszonezero" => Ok(TargetGame::ZZZ),
        "honkaiimpact3rd" | "hi3" => Ok(TargetGame::HonkaiImpact3rd),
        _ => Err(format!("Unknown game: {}", game)),
    }
}
