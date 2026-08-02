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

    /// X11 (xdotool): 模拟 VK_CLEAR(KP_Begin) 长按期间依次发送 SPACE 和 RETURN
    ///
    /// xdotool keydown/keyup 手动控制 CLEAR 修饰键的保持语义。
    /// 先 mousemove 到 (mod_idx, group_idx) 绑定光标位置，对齐 NRMM 的 SetCursorPos 行为。
    fn send_select_full_xtest(group_idx: u32, mod_idx: u32) -> Result<()> {
        // xdotool mousemove x y（光标绑定：mod_idx→X, group_idx→Y）
        let _ = Command::new("xdotool")
            .args(["mousemove", &mod_idx.to_string(), &group_idx.to_string()])
            .output();

        // CLEAR down
        Self::xtest_keydown("KP_Begin")?;
        thread::sleep(Duration::from_millis(10));

        // SPACE (完整按+释放)
        Self::send_key_xtest("space")?;
        thread::sleep(Duration::from_millis(30));

        // RETURN (完整按+释放)
        Self::send_key_xtest("Return")?;
        thread::sleep(Duration::from_millis(10));

        // CLEAR up
        Self::xtest_keyup("KP_Begin")?;
        Ok(())
    }

    fn xtest_keydown(key: &str) -> Result<()> {
        let o = Command::new("xdotool").args(["keydown", key]).output()?;
        if !o.status.success() {
            anyhow::bail!(
                "xdotool keydown {} failed: {}",
                key,
                String::from_utf8_lossy(&o.stderr)
            );
        }
        Ok(())
    }

    fn xtest_keyup(key: &str) -> Result<()> {
        let o = Command::new("xdotool").args(["keyup", key]).output()?;
        if !o.status.success() {
            anyhow::bail!(
                "xdotool keyup {} failed: {}",
                key,
                String::from_utf8_lossy(&o.stderr)
            );
        }
        Ok(())
    }

    /// Wayland (ydotool): CLEAR=72 SPACE=57 RETURN=28
    ///
    /// ydotool key 支持 `code:1`(down) / `code:0`(up) 语义，用于控制 CLEAR 的长按。
    /// 光标移动在 Wayland 下依赖 compositor，保守尝试失败则忽略（与 NRMM 行为一致）。
    fn send_select_full_ydotool(group_idx: u32, mod_idx: u32) -> Result<()> {
        let _g = group_idx;
        let _m = mod_idx;

        // CLEAR down (72:1 = 按下)
        let o1 = Command::new("ydotool").args(["key", "72:1"]).output()?;
        if !o1.status.success() {
            anyhow::bail!(
                "ydotool CLEAR down failed: {}",
                String::from_utf8_lossy(&o1.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));

        // SPACE (57 完整按+释放)
        Self::send_key_ydotool(57)?;
        thread::sleep(Duration::from_millis(30));

        // RETURN (28 完整按+释放)
        Self::send_key_ydotool(28)?;
        thread::sleep(Duration::from_millis(10));

        // CLEAR up (72:0 = 释放)
        let o2 = Command::new("ydotool").args(["key", "72:0"]).output()?;
        if !o2.status.success() {
            anyhow::bail!(
                "ydotool CLEAR up failed: {}",
                String::from_utf8_lossy(&o2.stderr)
            );
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
    
    fn simulate_f10(&self) -> Result<()> {
        match &self.method {
            KeyMethod::XTest => {
                Command::new("xdotool")
                    .args(["key", "F10"])
                    .output()
                    .map_err(|e| anyhow!("xdotool key F10 failed: {}", e))?;
                Ok(())
            }
            KeyMethod::Ydotool => {
                // F10 scan code: 121
                Command::new("ydotool")
                    .args(["key", "121:1", "121:0"])
                    .output()
                    .map_err(|e| anyhow!("ydotool key F10 failed: {}", e))?;
                Ok(())
            }
            KeyMethod::Unsupported(msg) => {
                Err(anyhow!("F10 keypress not supported: {}", msg))
            }
        }
    }

    fn simulate_select_full(&self, group_idx: u32, mod_idx: u32) -> Result<()> {
        match &self.method {
            KeyMethod::XTest => Self::send_select_full_xtest(group_idx, mod_idx),
            KeyMethod::Ydotool => Self::send_select_full_ydotool(group_idx, mod_idx),
            KeyMethod::Unsupported(r) => Err(anyhow!(r.clone())),
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
