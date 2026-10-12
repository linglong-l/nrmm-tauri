//! 应用设置数据模型
//!
//! 定义应用设置相关的所有数据结构，使用 serde 序列化/反序列化（camelCase）

use super::enums::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_true() -> bool {
    true
}

/// 窗口是否透明化的平台默认值
///
/// Windows / 其它平台默认透明化窗口（transparent = true）；
/// Linux 默认不透明（transparent = false），以避免 Wayland 合成器下的渲染异常。
/// 该默认值仅作用于「配置文件缺失该字段」的场景（旧 settings.json 兼容 / 用户未显式选择）。
fn default_window_transparent() -> bool {
    !cfg!(target_os = "linux")
}

/// 日志时间戳时区的默认值（跟随系统本地时区）
fn default_log_timezone() -> String {
    "system".to_string()
}

/// 键盘快捷键配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyKeyboard {
    pub mod1: String,
    pub mod2: String,
}

impl Default for HotkeyKeyboard {
    fn default() -> Self {
        Self {
            mod1: "ctrlshift".to_string(),
            mod2: "alt".to_string(),
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
    /// 窗口切换快捷键（键盘）
    pub window_hotkey: String,
    /// 窗口切换快捷键（手柄）
    pub gamepad_hotkey_toggle: String,
    /// 各游戏的 Mods 目录路径
    pub game_mods_path: HashMap<TargetGame, String>,
    /// 各游戏的目标进程名
    pub target_process_per_game: HashMap<TargetGame, String>,
    /// UI 缩放比例
    pub interface_scale: f64,
    /// 背景透明度
    pub bg_transparency: f64,
    /// 窗口是否透明化（Windows/其它平台默认 true，Linux 默认 false 以避免 Wayland 合成器异常）
    #[serde(default = "default_window_transparent")]
    pub window_transparent: bool,
    /// 是否启用动态背景
    pub dynamic_background: bool,
    /// 界面语言
    pub language: String,
    /// 是否启用暗色模式
    pub dark_mode: bool,
    /// 是否在模组选择时模拟按键（与 NRMM 的 simulate_key_on_selection 对齐）
    #[serde(default = "default_true")]
    pub simulate_key_on_selection: bool,
    /// 热键时是否总是显示菜单
    pub always_show_menu_on_hotkey: bool,
    /// 是否仅在 3Dmigoto 中响应热键
    pub hotkey_only_in_migoto: bool,
    /// 各分组选中的模组索引
    pub selected_mod_index: HashMap<String, u32>,
    /// 是否仅显示收藏
    pub show_favorites_only: bool,
    /// 是否在启动时打开控制台窗口查看日志（默认 false）
    #[serde(default)]
    pub show_console_on_start: bool,
    /// 日志时间戳时区（默认 system=系统本地时区）
    ///
    /// 支持值（不区分大小写）：`system` / `local` / 空 = 系统本地时区；
    /// `UTC`；固定偏移如 `UTC+8`、`+08:00`、`-05:30`。
    /// 背景：env_logger 默认格式时间戳恒为 UTC，与日志文件名（本地日期）不一致；
    /// 修复后默认本地时区，本字段允许在配置文件中显式指定（无 UI 项）。
    #[serde(default = "default_log_timezone")]
    pub log_timezone: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            target_game: TargetGame::GenshinImpact,
            hotkey: HotkeyKeyboard::default(),
            window_hotkey: "Alt+D".to_string(),
            gamepad_hotkey_toggle: "LB+RB".to_string(),
            game_mods_path: HashMap::new(),
            target_process_per_game: HashMap::new(),
            interface_scale: 1.0,
            bg_transparency: 0.7,
            window_transparent: default_window_transparent(),
            dynamic_background: true,
            language: "en".to_string(),
            dark_mode: true,
            simulate_key_on_selection: true,
            always_show_menu_on_hotkey: false,
            hotkey_only_in_migoto: false,
            selected_mod_index: HashMap::new(),
            show_favorites_only: false,
            show_console_on_start: false,
            log_timezone: default_log_timezone(),
        }
    }
}
