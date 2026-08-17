//! macOS 平台实现模块（占位）
//!
//! 目前 macOS 平台的按键模拟和前台检测尚未实现，
//! 需要辅助功能权限（Assistive Access）才能实现这些功能。

use anyhow::{anyhow, Result};

/// macOS 按键模拟器（未实现）
pub struct MacOSKeySimulator;

impl super::KeySimulator for MacOSKeySimulator {
    fn simulate_select_group(&mut self) -> Result<()> {
        Err(anyhow!("macOS keypress simulation not yet implemented"))
    }

    fn simulate_select_mod(&mut self) -> Result<()> {
        Err(anyhow!("macOS keypress simulation not yet implemented"))
    }

    fn simulate_f10(&mut self) -> Result<()> {
        Err(anyhow!("macOS keypress simulation not supported"))
    }

    fn check_support(&self) -> Result<(), String> {
        Err(
            "macOS keypress simulation requires assistive access permission, not yet implemented"
                .to_string(),
        )
    }
}

/// macOS 前台窗口检测器（未实现）
pub struct MacOSForegroundDetector;

impl super::ForegroundDetector for MacOSForegroundDetector {
    fn get_foreground_process_name(&self) -> Result<String> {
        Err(anyhow!("macOS foreground detection not yet implemented"))
    }

    fn get_cursor_position(&self) -> Result<(i32, i32)> {
        Err(anyhow!("macOS cursor position not yet implemented"))
    }
}
