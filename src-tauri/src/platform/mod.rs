//! 平台抽象模块
//!
//! 提供跨平台的前台窗口检测和按键模拟功能：
//! - windows: Windows 平台实现（使用 Win32 API）
//! - linux: Linux 平台实现（使用 X11/Wayland）
//! - macos: macOS 平台实现（受限支持）
//!
//! # Trait 抽象
//! - `KeySimulator`: 按键模拟 trait（选择分组/模组）
//! - `ForegroundDetector`: 前台窗口检测 trait（判断游戏是否在前台，获取光标位置）
//!
//! 工厂函数 get_key_simulator() 和 get_foreground_detector() 根据编译目标平台返回对应实现。

use anyhow::Result;
use crate::models::enums::TargetGame;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;

/// 平台信息结构体
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub os: String,
    pub session_type: Option<String>,
    pub keypress_supported: bool,
    pub keypress_error: Option<String>,
    pub foreground_detection_supported: bool,
}

/// 按键模拟 trait
///
/// 实现模拟键盘按键来操作 3Dmigoto 内置菜单
pub trait KeySimulator: Send + Sync {
    /// 模拟选择分组的按键序列
    fn simulate_select_group(&self) -> Result<()>;
    /// 模拟选择模组的按键序列
    fn simulate_select_mod(&self) -> Result<()>;
    /// 模拟 F10 按键（3Dmigoto 重载快捷键）
    fn simulate_f10(&self) -> Result<()>;
    /// 模拟选择分组（带光标坐标绑定）
    /// x, y 为目标光标位置（屏幕坐标），用于将光标移动到 3Dmigoto 内置菜单对应位置
    /// 默认实现 fallback 到无坐标版本
    fn simulate_select_group_at(&self, _x: i32, _y: i32) -> Result<()> {
        self.simulate_select_group()
    }
    /// 模拟选择模组（带光标坐标绑定）
    /// x, y 为目标光标位置（屏幕坐标），用于将光标移动到 3Dmigoto 内置菜单对应位置
    /// 默认实现 fallback 到无坐标版本
    fn simulate_select_mod_at(&self, _x: i32, _y: i32) -> Result<()> {
        self.simulate_select_mod()
    }
    /// 检查平台是否支持按键模拟
    fn check_support(&self) -> Result<(), String>;
}

/// 前台窗口检测 trait
///
/// 检测当前前台窗口是否为目标游戏进程，获取光标位置
pub trait ForegroundDetector: Send + Sync {
    /// 获取当前前台进程名
    fn get_foreground_process_name(&self) -> Result<String>;
    
    /// 判断目标游戏是否在前台
    fn is_game_foreground(&self, game: TargetGame) -> bool {
        match self.get_foreground_process_name() {
            Ok(name) => {
                let lower = name.to_lowercase();
                game.process_names().iter().any(|pn| pn.to_lowercase() == lower)
            }
            Err(_) => false,
        }
    }
    
    /// 获取当前光标位置（屏幕坐标）
    fn get_cursor_position(&self) -> Result<(i32, i32)>;
}

/// 获取当前平台的按键模拟器实例
pub fn get_key_simulator() -> Box<dyn KeySimulator> {
    #[cfg(target_os = "windows")]
    { Box::new(windows::WindowsKeySimulator) }
    #[cfg(target_os = "linux")]
    { Box::new(linux::LinuxKeySimulator::new()) }
    #[cfg(target_os = "macos")]
    { Box::new(macos::MacOSKeySimulator) }
}

/// 获取当前平台的前台窗口检测器实例
pub fn get_foreground_detector() -> Box<dyn ForegroundDetector> {
    #[cfg(target_os = "windows")]
    { Box::new(windows::WindowsForegroundDetector) }
    #[cfg(target_os = "linux")]
    { Box::new(linux::LinuxForegroundDetector::new()) }
    #[cfg(target_os = "macos")]
    { Box::new(macos::MacOSForegroundDetector) }
}

/// 获取平台信息（Tauri 命令）
#[tauri::command]
pub fn get_platform_info() -> PlatformInfo {
    #[cfg(target_os = "windows")]
    {
        PlatformInfo {
            os: "windows".to_string(),
            session_type: None,
            keypress_supported: true,
            keypress_error: None,
            foreground_detection_supported: true,
        }
    }
    #[cfg(target_os = "linux")]
    { linux::get_linux_platform_info() }
    #[cfg(target_os = "macos")]
    {
        PlatformInfo {
            os: "macos".to_string(),
            session_type: None,
            keypress_supported: false,
            keypress_error: Some("macOS keypress simulation requires assistive access permission".to_string()),
            foreground_detection_supported: false,
        }
    }
}
