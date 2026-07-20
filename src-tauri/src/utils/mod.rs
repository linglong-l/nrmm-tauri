//! 工具模块集合。
//!
//! 收纳后端通用的辅助工具（如日志采样器），供其他业务模块复用。

pub mod log_sampler;

use std::env;

/// 检测当前是否运行在 Linux 环境下。
///
/// 通过检查目标操作系统类型判断，编译时决定。
#[inline]
pub fn is_linux() -> bool {
    #[cfg(target_os = "linux")]
    {
        true
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// 检测当前是否运行在 WSL（Windows Subsystem for Linux）环境下。
///
/// 检查方式：
/// 1. 检查 `/proc/version` 文件内容是否包含 "microsoft"
/// 2. 检查环境变量 `WSLENV` 或 `WSL_DISTRO_NAME` 是否存在
///
/// 返回 `true` 表示当前运行在 WSL 环境。
pub fn is_wsl() -> bool {
    if !is_linux() {
        return false;
    }

    if env::var("WSLENV").is_ok() || env::var("WSL_DISTRO_NAME").is_ok() {
        return true;
    }

    if let Ok(content) = std::fs::read_to_string("/proc/version") {
        if content.to_lowercase().contains("microsoft") {
            return true;
        }
    }

    false
}
