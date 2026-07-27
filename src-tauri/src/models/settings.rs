use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use super::enums::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyKeyboard {
    pub mod1: String,
    pub mod2: String,
    pub key_next: String,
    pub key_prev: String,
    pub key_hide: String,
    pub key_select: String,
    pub key_cancel: String,
    pub key_scrollup: String,
    pub key_scrolldown: String,
}

impl Default for HotkeyKeyboard {
    fn default() -> Self {
        Self {
            mod1: "ctrlshift".to_string(),
            mod2: "alt".to_string(),
            key_next: "right".to_string(),
            key_prev: "left".to_string(),
            key_hide: "oemcomma".to_string(),
            key_select: "oemperiod".to_string(),
            key_cancel: "oemminus".to_string(),
            key_scrollup: "up".to_string(),
            key_scrolldown: "down".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyGamepad {
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    pub button_next: String,
    pub button_prev: String,
    pub button_hide: String,
    pub button_select: String,
    pub button_cancel: String,
}

impl Default for HotkeyGamepad {
    fn default() -> Self {
        Self {
            dpad_up: false,
            dpad_down: false,
            dpad_left: true,
            dpad_right: true,
            button_next: "b".to_string(),
            button_prev: "x".to_string(),
            button_hide: "start".to_string(),
            button_select: "a".to_string(),
            button_cancel: "y".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub target_game: TargetGame,
    pub hotkey: HotkeyKeyboard,
    pub gamepad_hotkey: HotkeyGamepad,
    pub game_mods_path: HashMap<TargetGame, String>,
    pub game_profile: HashMap<String, KeybindProfile>,
    pub interface_scale: f64,
    pub bg_transparency: f64,
    pub dynamic_background: bool,
    pub mod_grouping_mode: LayoutMode,
    pub mods_sorting_type: SortingType,
    pub reverse_sort: bool,
    pub cursor_type: CursorType,
    pub language: String,
    pub dark_mode: bool,
    pub auto_folder_icon: bool,
    pub auto_priority_index: bool,
    pub check_update_on_start: bool,
    pub auto_top_window: bool,
    pub is_window_fullscreen: bool,
    pub show_keypress_on_screen: bool,
    pub simulate_key_on_selection: bool,
    pub use_precise_hotkey: bool,
    pub swap_cancel_keybind: bool,
    pub always_show_menu_on_hotkey: bool,
    pub hotkey_only_in_migoto: bool,
    pub folder_icon_blacklist: Vec<String>,
    pub disabled_kb_inputs: Vec<String>,
    pub disabled_gamepad_inputs: Vec<String>,
    pub selected_mod_index: HashMap<String, u32>,
    pub selected_group_index: HashMap<String, u32>,
    pub enabled_kb: bool,
    pub enabled_gamepad: bool,
    pub show_errored_mods: bool,
    pub show_favorites_only: bool,
    pub check_namespace_conflict: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            target_game: TargetGame::GenshinImpact,
            hotkey: HotkeyKeyboard::default(),
            gamepad_hotkey: HotkeyGamepad::default(),
            game_mods_path: HashMap::new(),
            game_profile: HashMap::new(),
            interface_scale: 1.0,
            bg_transparency: 0.7,
            dynamic_background: true,
            mod_grouping_mode: LayoutMode::Automatic,
            mods_sorting_type: SortingType::Default,
            reverse_sort: false,
            cursor_type: CursorType::Normal,
            language: "en".to_string(),
            dark_mode: true,
            auto_folder_icon: true,
            auto_priority_index: true,
            check_update_on_start: true,
            auto_top_window: true,
            is_window_fullscreen: false,
            show_keypress_on_screen: false,
            simulate_key_on_selection: true,
            use_precise_hotkey: false,
            swap_cancel_keybind: false,
            always_show_menu_on_hotkey: false,
            hotkey_only_in_migoto: false,
            folder_icon_blacklist: Vec::new(),
            disabled_kb_inputs: Vec::new(),
            disabled_gamepad_inputs: Vec::new(),
            selected_mod_index: HashMap::new(),
            selected_group_index: HashMap::new(),
            enabled_kb: true,
            enabled_gamepad: false,
            show_errored_mods: false,
            show_favorites_only: false,
            check_namespace_conflict: true,
        }
    }
}
