//! 应用设置数据模型
//!
//! 定义应用设置相关的所有数据结构，使用 serde 序列化/反序列化（camelCase）

use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use super::enums::*;

/// 键盘快捷键配置
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

/// 手柄快捷键配置
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

/// 应用全局设置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// 当前目标游戏
    pub target_game: TargetGame,
    /// 键盘快捷键配置
    pub hotkey: HotkeyKeyboard,
    /// 手柄快捷键配置
    pub gamepad_hotkey: HotkeyGamepad,
    /// 窗口切换快捷键（键盘）
    pub window_hotkey: String,
    /// 窗口切换快捷键（手柄）
    pub gamepad_hotkey_toggle: String,
    /// 搜索快捷键
    pub search_hotkey: String,
    /// 各游戏的 Mods 目录路径
    pub game_mods_path: HashMap<TargetGame, String>,
    /// 各游戏的目标进程名
    pub target_process_per_game: HashMap<TargetGame, String>,
    /// 按键配置方案
    pub game_profile: HashMap<String, KeybindProfile>,
    /// UI 缩放比例
    pub interface_scale: f64,
    /// 背景透明度
    pub bg_transparency: f64,
    /// 是否启用动态背景
    pub dynamic_background: bool,
    /// 模组分组布局模式
    pub mod_grouping_mode: LayoutMode,
    /// 模组排序方式
    pub mods_sorting_type: SortingType,
    /// 是否反向排序
    pub reverse_sort: bool,
    /// 光标类型
    pub cursor_type: CursorType,
    /// 界面语言
    pub language: String,
    /// 是否启用暗色模式
    pub dark_mode: bool,
    /// 是否自动设置文件夹图标
    pub auto_folder_icon: bool,
    /// 是否自动设置优先级索引
    pub auto_priority_index: bool,
    /// 是否启动时检查更新
    pub check_update_on_start: bool,
    /// 是否热键时自动置顶窗口
    pub auto_top_window: bool,
    /// 窗口是否全屏
    pub is_window_fullscreen: bool,
    /// 是否在屏幕上显示按键
    pub show_keypress_on_screen: bool,
    /// 选择模组时是否模拟按键
    pub simulate_key_on_selection: bool,
    /// 是否使用精确热键
    pub use_precise_hotkey: bool,
    /// 是否交换取消键绑定
    pub swap_cancel_keybind: bool,
    /// 热键时是否总是显示菜单
    pub always_show_menu_on_hotkey: bool,
    /// 是否仅在 3Dmigoto 中响应热键
    pub hotkey_only_in_migoto: bool,
    /// 文件夹图标黑名单
    pub folder_icon_blacklist: Vec<String>,
    /// 禁用的键盘输入列表
    pub disabled_kb_inputs: Vec<String>,
    /// 禁用的手柄输入列表
    pub disabled_gamepad_inputs: Vec<String>,
    /// 各分组选中的模组索引
    pub selected_mod_index: HashMap<String, u32>,
    /// 选中的分组索引
    pub selected_group_index: HashMap<String, u32>,
    /// 是否启用键盘热键
    pub enabled_kb: bool,
    /// 是否启用手柄热键
    pub enabled_gamepad: bool,
    /// 是否显示有错误的模组
    pub show_errored_mods: bool,
    /// 是否仅显示收藏
    pub show_favorites_only: bool,
    /// 是否检查命名空间冲突
    pub check_namespace_conflict: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            target_game: TargetGame::GenshinImpact,
            hotkey: HotkeyKeyboard::default(),
            gamepad_hotkey: HotkeyGamepad::default(),
            window_hotkey: "Alt+D".to_string(),
            gamepad_hotkey_toggle: "LB+RB".to_string(),
            search_hotkey: "Alt+S".to_string(),
            game_mods_path: HashMap::new(),
            target_process_per_game: HashMap::new(),
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
