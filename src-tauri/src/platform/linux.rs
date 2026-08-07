//! Linux 平台实现模块
//!
//! 根据 XDG_SESSION_TYPE 自动选择实现方式：
//! - X11 会话：使用 xdotool 进行按键模拟和前台检测（支持窗口定向发送）
//! - Wayland 会话：使用 ydotool 按键，wlrctl 前台检测（需用户安装）
//!
//! 如果所需工具未安装，返回不支持错误并提示安装命令。

use anyhow::{Result, anyhow};
use std::process::Command;
use std::thread;
use std::time::Duration;

use super::KeySimulator;

/// 按键模拟方法
#[derive(Debug, Clone)]
enum KeyMethod {
    /// X11 下使用 xdotool
    XTest,
    /// Wayland 下使用 ydotool
    Ydotool,
    /// 不支持，附带错误信息
    Unsupported(String),
}

#[derive(Debug, Clone)]
enum WaylandMethod {
    WlrctlWtype { app_id: String },
    Ydotool,
}

/// Linux 按键模拟器
pub struct LinuxKeySimulator {
    method: KeyMethod,
    target_window_id: Option<String>,
    wayland_method: Option<WaylandMethod>,
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
        LinuxKeySimulator {
            method,
            target_window_id: None,
            wayland_method: None,
        }
    }

    fn dispatch_xtest(&self, key: &str) -> Result<()> {
        if let Some(wid) = &self.target_window_id {
            match Self::send_key_xtest_window(key, wid) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::warn!("xdotool --window 发送失败，fallback global: {}", e);
                }
            }
        }
        Self::send_key_xtest(key)
    }

    fn dispatch_xtest_keydown(&self, key: &str) -> Result<()> {
        if let Some(wid) = &self.target_window_id {
            match Self::xtest_keydown_window(key, wid) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::warn!("xdotool keydown --window 失败，fallback global: {}", e);
                }
            }
        }
        Self::xtest_keydown(key)
    }

    fn dispatch_xtest_keyup(&self, key: &str) -> Result<()> {
        if let Some(wid) = &self.target_window_id {
            match Self::xtest_keyup_window(key, wid) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::warn!("xdotool keyup --window 失败，fallback global: {}", e);
                }
            }
        }
        Self::xtest_keyup(key)
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

    fn send_key_xtest_window(key: &str, wid: &str) -> Result<()> {
        let key_name = match key {
            "clear" => "KP_Begin",
            "space" => "space",
            "return" => "Return",
            _ => key,
        };
        let output = Command::new("xdotool")
            .args(["key", "--window", wid, "--clearmodifiers", key_name])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("xdotool --window failed: {}", stderr);
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

    fn dispatch_wayland_send(&self, key_name: &str, key_code: u16) -> Result<()> {
        if let Some(WaylandMethod::WlrctlWtype { app_id }) = &self.wayland_method {
            match Self::send_wlrctl_wtype(key_name, app_id) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::warn!("wlrctl+wtype 发送失败，fallback ydotool: {}", e);
                }
            }
        }
        Self::send_key_ydotool(key_code)
    }

    fn send_wlrctl_wtype(key: &str, app_id: &str) -> Result<()> {
        let focus = Command::new("wlrctl")
            .args(["window", "focus", app_id])
            .output();
        if let Err(e) = focus {
            log::warn!("wlrctl window focus skipped: {}", e);
        } else if let Ok(o) = focus {
            if !o.status.success() {
                log::warn!(
                    "wlrctl window focus {} failed (non-zero): {}",
                    app_id,
                    String::from_utf8_lossy(&o.stderr)
                );
            }
        }

        let key_name = match key {
            "clear" => "KP_Begin",
            "space" => "space",
            "return" => "Return",
            _ => key,
        };
        let output = Command::new("wtype")
            .args(["-k", key_name])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("wtype -k failed: {}", stderr);
        }
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

    fn xtest_keydown_window(key: &str, wid: &str) -> Result<()> {
        let o = Command::new("xdotool")
            .args(["keydown", "--window", wid, key])
            .output()?;
        if !o.status.success() {
            anyhow::bail!(
                "xdotool keydown --window {} {} failed: {}",
                wid,
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

    fn xtest_keyup_window(key: &str, wid: &str) -> Result<()> {
        let o = Command::new("xdotool")
            .args(["keyup", "--window", wid, key])
            .output()?;
        if !o.status.success() {
            anyhow::bail!(
                "xdotool keyup --window {} {} failed: {}",
                wid,
                key,
                String::from_utf8_lossy(&o.stderr)
            );
        }
        Ok(())
    }

    fn send_select_full_xtest(&self, group_idx: u32, mod_idx: u32) -> Result<()> {
        let _ = Command::new("xdotool")
            .args(["mousemove", &mod_idx.to_string(), &group_idx.to_string()])
            .output();

        self.dispatch_xtest_keydown("KP_Begin")?;
        thread::sleep(Duration::from_millis(10));

        self.dispatch_xtest("space")?;
        thread::sleep(Duration::from_millis(30));

        self.dispatch_xtest("Return")?;
        thread::sleep(Duration::from_millis(10));

        self.dispatch_xtest_keyup("KP_Begin")?;
        Ok(())
    }

    fn send_select_full_ydotool(&self, group_idx: u32, mod_idx: u32) -> Result<()> {
        let _g = group_idx;
        let _m = mod_idx;

        let do_clear_down = || -> Result<()> {
            if let Some(WaylandMethod::WlrctlWtype { app_id }) = &self.wayland_method {
                let _ = Command::new("wlrctl")
                    .args(["window", "focus", app_id])
                    .output();
                let o = Command::new("wtype").args(["-k", "-M", "KP_Begin"]).output();
                if let Ok(o) = o {
                    if o.status.success() {
                        return Ok(());
                    } else {
                        log::warn!(
                            "wtype -M KP_Begin failed, fallback ydotool: {}",
                            String::from_utf8_lossy(&o.stderr)
                        );
                    }
                }
            }
            let o1 = Command::new("ydotool").args(["key", "72:1"]).output()?;
            if !o1.status.success() {
                anyhow::bail!(
                    "ydotool CLEAR down failed: {}",
                    String::from_utf8_lossy(&o1.stderr)
                );
            }
            Ok(())
        };

        let do_clear_up = || -> Result<()> {
            if let Some(WaylandMethod::WlrctlWtype { app_id }) = &self.wayland_method {
                let _ = Command::new("wlrctl")
                    .args(["window", "focus", app_id])
                    .output();
                let o = Command::new("wtype").args(["-k", "-m", "KP_Begin"]).output();
                if let Ok(o) = o {
                    if o.status.success() {
                        return Ok(());
                    } else {
                        log::warn!(
                            "wtype -m KP_Begin failed, fallback ydotool: {}",
                            String::from_utf8_lossy(&o.stderr)
                        );
                    }
                }
            }
            let o2 = Command::new("ydotool").args(["key", "72:0"]).output()?;
            if !o2.status.success() {
                anyhow::bail!(
                    "ydotool CLEAR up failed: {}",
                    String::from_utf8_lossy(&o2.stderr)
                );
            }
            Ok(())
        };

        do_clear_down()?;
        thread::sleep(Duration::from_millis(10));

        self.dispatch_wayland_send("space", 57)?;
        thread::sleep(Duration::from_millis(30));

        self.dispatch_wayland_send("Return", 28)?;
        thread::sleep(Duration::from_millis(10));

        do_clear_up()?;
        Ok(())
    }
}

impl super::KeySimulator for LinuxKeySimulator {
    fn set_target_process(&mut self, process_name: &str) -> Result<()> {
        match &self.method {
            KeyMethod::XTest => {
                let search = Command::new("xdotool")
                    .args(["search", "--name", process_name])
                    .output();
                if let Ok(output) = search {
                    if output.status.success() {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        if let Some(first_line) = stdout.lines().next() {
                            let trimmed = first_line.trim();
                            if !trimmed.is_empty() {
                                self.target_window_id = Some(trimmed.to_string());
                                return Ok(());
                            }
                        }
                    }
                }
                self.target_window_id = None;
            }
            KeyMethod::Ydotool => {
                let has_wlrctl = Command::new("wlrctl").arg("--version").output().is_ok();
                let has_wtype = Command::new("wtype").output().is_ok()
                    || Command::new("which").arg("wtype").output().map(|o| o.status.success()).unwrap_or(false);
                if has_wlrctl && has_wtype {
                    self.wayland_method = Some(WaylandMethod::WlrctlWtype {
                        app_id: process_name.to_lowercase(),
                    });
                } else {
                    self.wayland_method = Some(WaylandMethod::Ydotool);
                }
            }
            KeyMethod::Unsupported(_) => {}
        }
        Ok(())
    }

    fn simulate_select_group(&mut self) -> Result<()> {
        match &self.method {
            KeyMethod::XTest => {
                self.dispatch_xtest("clear")?;
                thread::sleep(Duration::from_millis(30));
                self.dispatch_xtest("space")?;
                Ok(())
            }
            KeyMethod::Ydotool => {
                self.dispatch_wayland_send("clear", 72)?;
                thread::sleep(Duration::from_millis(30));
                self.dispatch_wayland_send("space", 57)?;
                Ok(())
            }
            KeyMethod::Unsupported(reason) => Err(anyhow!(reason.clone())),
        }
    }
    
    fn simulate_select_mod(&mut self) -> Result<()> {
        match &self.method {
            KeyMethod::XTest => {
                self.dispatch_xtest("clear")?;
                thread::sleep(Duration::from_millis(30));
                self.dispatch_xtest("return")?;
                Ok(())
            }
            KeyMethod::Ydotool => {
                self.dispatch_wayland_send("clear", 72)?;
                thread::sleep(Duration::from_millis(30));
                self.dispatch_wayland_send("Return", 28)?;
                Ok(())
            }
            KeyMethod::Unsupported(reason) => Err(anyhow!(reason.clone())),
        }
    }
    
    fn simulate_f10(&mut self) -> Result<()> {
        match &self.method {
            KeyMethod::XTest => {
                if let Some(wid) = &self.target_window_id {
                    let r = Command::new("xdotool")
                        .args(["key", "--window", wid, "F10"])
                        .output();
                    match r {
                        Ok(o) if o.status.success() => return Ok(()),
                        Ok(o) => log::warn!(
                            "xdotool key --window F10 fallback global: {}",
                            String::from_utf8_lossy(&o.stderr)
                        ),
                        Err(e) => log::warn!("xdotool key --window F10 fallback global: {}", e),
                    }
                }
                Command::new("xdotool")
                    .args(["key", "F10"])
                    .output()
                    .map_err(|e| anyhow!("xdotool key F10 failed: {}", e))?;
                Ok(())
            }
            KeyMethod::Ydotool => {
                if let Some(WaylandMethod::WlrctlWtype { app_id }) = &self.wayland_method {
                    let r = (|| -> Result<()> {
                        let _ = Command::new("wlrctl")
                            .args(["window", "focus", app_id])
                            .output();
                        let o = Command::new("wtype").args(["-k", "F10"]).output()?;
                        if !o.status.success() {
                            anyhow::bail!("wtype -k F10 failed: {}", String::from_utf8_lossy(&o.stderr));
                        }
                        Ok(())
                    })();
                    match r {
                        Ok(()) => return Ok(()),
                        Err(e) => log::warn!("wtype F10 failed fallback ydotool: {}", e),
                    }
                }
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

    fn simulate_select_full(&mut self, group_idx: u32, mod_idx: u32) -> Result<()> {
        match &self.method {
            KeyMethod::XTest => self.send_select_full_xtest(group_idx, mod_idx),
            KeyMethod::Ydotool => self.send_select_full_ydotool(group_idx, mod_idx),
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
