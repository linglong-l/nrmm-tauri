//! Linux 平台实现模块
//!
//! 根据 XDG_SESSION_TYPE 自动选择实现方式：
//! - X11 会话：使用 xdotool 进行按键模拟和前台检测
//! - Wayland 会话：使用 ydotool 按键，wlrctl 前台检测（需用户安装）
//!
//! 如果所需工具未安装，返回不支持错误并提示安装命令。

use anyhow::{Result, anyhow};
use std::process::Command;
use std::thread;
use std::time::Duration;

/// Linux 按键模拟器
pub struct LinuxKeySimulator {
    method: KeyMethod,
}

/// 按键模拟方法
#[derive(Debug)]
enum KeyMethod {
    /// X11 下使用 xdotool
    XTest,
    /// Wayland 下使用 ydotool
    Ydotool,
    /// 不支持，附带错误信息
    Unsupported(String),
}

impl LinuxKeySimulator {
    pub fn new() -> Self {
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        let method = if session == "wayland" {
            if Command::new("ydotool").arg("--version").output().is_ok() {
                KeyMethod::Ydotool
            } else {
                KeyMethod::Unsupported("ydotool not installed. Install with: sudo dnf install ydotool (Fedora) or sudo apt install ydotool (Debian/Ubuntu)".to_string())
            }
        } else {
            if Command::new("xdotool").arg("--version").output().is_ok() {
                KeyMethod::XTest
            } else {
                KeyMethod::Unsupported("xdotool not installed. Install with: sudo dnf install xdotool (Fedora) or sudo apt install xdotool (Debian/Ubuntu)".to_string())
            }
        };
        LinuxKeySimulator { method }
    }
    
    fn send_key_xtest(key: &str) -> Result<()> {
        let key_name = match key {
            "clear" => "KP_Begin",
            "space" => "space",
            "return" => "Return",
            _ => key,
        };
        let output = Command::new("xdotool")
            .args(["key", "--clearmodifiers", key_name])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("xdotool failed: {}", stderr);
        }
        Ok(())
    }
    
    fn send_key_ydotool(key: u16) -> Result<()> {
        let output = Command::new("ydotool")
            .args(["key", &key.to_string()])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ydotool failed: {}", stderr);
        }
        Ok(())
    }
}

impl super::KeySimulator for LinuxKeySimulator {
    fn simulate_select_group(&self) -> Result<()> {
        match &self.method {
            KeyMethod::XTest => {
                Self::send_key_xtest("clear")?;
                thread::sleep(Duration::from_millis(30));
                Self::send_key_xtest("space")?;
                Ok(())
            }
            KeyMethod::Ydotool => {
                Self::send_key_ydotool(72)?;
                thread::sleep(Duration::from_millis(30));
                Self::send_key_ydotool(57)?;
                Ok(())
            }
            KeyMethod::Unsupported(reason) => Err(anyhow!(reason.clone())),
        }
    }
    
    fn simulate_select_mod(&self) -> Result<()> {
        match &self.method {
            KeyMethod::XTest => {
                Self::send_key_xtest("clear")?;
                thread::sleep(Duration::from_millis(30));
                Self::send_key_xtest("return")?;
                Ok(())
            }
            KeyMethod::Ydotool => {
                Self::send_key_ydotool(72)?;
                thread::sleep(Duration::from_millis(30));
                Self::send_key_ydotool(28)?;
                Ok(())
            }
            KeyMethod::Unsupported(reason) => Err(anyhow!(reason.clone())),
        }
    }
    
    fn check_support(&self) -> Result<(), String> {
        match &self.method {
            KeyMethod::Unsupported(reason) => Err(reason.clone()),
            _ => Ok(()),
        }
    }
}

/// Linux 前台窗口检测器
pub struct LinuxForegroundDetector {
    method: ForegroundMethod,
}

/// 前台检测方法
enum ForegroundMethod {
    X11,
    WlCtrl,
    Unsupported,
}

impl LinuxForegroundDetector {
    pub fn new() -> Self {
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        let method = if session == "wayland" {
            if Command::new("wlrctl").arg("--version").output().is_ok() {
                ForegroundMethod::WlCtrl
            } else {
                ForegroundMethod::Unsupported
            }
        } else {
            if Command::new("xdotool").arg("--version").output().is_ok() {
                ForegroundMethod::X11
            } else {
                ForegroundMethod::Unsupported
            }
        };
        LinuxForegroundDetector { method }
    }
}

impl super::ForegroundDetector for LinuxForegroundDetector {
    fn get_foreground_process_name(&self) -> Result<String> {
        match &self.method {
            ForegroundMethod::X11 => {
                let pid_output = Command::new("xdotool")
                    .args(["getactivewindow", "getwindowpid"])
                    .output()?;
                if !pid_output.status.success() {
                    anyhow::bail!("xdotool getactivewindow failed");
                }
                let pid_str = String::from_utf8_lossy(&pid_output.stdout).trim().to_string();
                let pid: u32 = pid_str.parse().map_err(|_| anyhow!("Invalid PID"))?;
                let comm_path = format!("/proc/{}/comm", pid);
                let comm = std::fs::read_to_string(&comm_path)?;
                // comm 文件内容带换行，需要 trim
                Ok(comm.trim().to_string())
            }
            ForegroundMethod::WlCtrl | ForegroundMethod::Unsupported => {
                Err(anyhow!("Foreground detection not available on this compositor"))
            }
        }
    }
    
    fn get_cursor_position(&self) -> Result<(i32, i32)> {
        match &self.method {
            ForegroundMethod::X11 => {
                let output = Command::new("xdotool")
                    .args(["getmouselocation"])
                    .output()?;
                let stdout = String::from_utf8_lossy(&output.stdout);
                let mut x = 0i32;
                let mut y = 0i32;
                for part in stdout.split_whitespace() {
                    if let Some(val) = part.strip_prefix("x:") {
                        x = val.parse().unwrap_or(0);
                    } else if let Some(val) = part.strip_prefix("y:") {
                        y = val.parse().unwrap_or(0);
                    }
                }
                Ok((x, y))
            }
            _ => Err(anyhow!("Cursor position not available")),
        }
    }
}

/// 获取 Linux 平台信息
pub fn get_linux_platform_info() -> super::PlatformInfo {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    
    let key_sim = LinuxKeySimulator::new();
    let (key_supported, key_error) = match key_sim.check_support() {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e)),
    };
    
    let fg = LinuxForegroundDetector::new();
    let fg_supported = match fg.method {
        ForegroundMethod::X11 => true,
        _ => false,
    };
    
    super::PlatformInfo {
        os: "linux".to_string(),
        session_type: if session.is_empty() { None } else { Some(session) },
        keypress_supported: key_supported,
        keypress_error: key_error,
        foreground_detection_supported: fg_supported,
    }
}
