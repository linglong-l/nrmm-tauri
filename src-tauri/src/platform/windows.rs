//! Windows 平台实现模块
//!
//! 使用 Win32 API 实现：
//! - 按键模拟：SendInput 发送键盘事件（全局）或 PostMessageW 发送到指定窗口（定向）
//! - 前台窗口检测：GetForegroundWindow + GetWindowThreadProcessId + QueryFullProcessImageNameW
//! - 光标位置获取：GetCursorPos
//!
//! # unsafe 说明
//! 所有 Win32 API 调用都在 unsafe 块中，因为它们是 C FFI 调用。
//! 安全性通过正确传递指针和缓冲区大小保证：
//! - 缓冲区大小固定为 MAX_PATH (260)，避免溢出
//! - 句柄使用后调用 CloseHandle 释放
//! - 检查句柄有效性后再使用

use anyhow::{Result, anyhow, Context};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use std::thread;
use std::time::Duration;
use windows::Win32::System::Threading::*;
use windows::core::PWSTR;
use std::sync::Mutex;

static ENUM_WINDOWS_STATE: Mutex<Option<(String, Option<isize>)>> = Mutex::new(None);

struct SafeHWND(HWND);
unsafe impl Send for SafeHWND {}
unsafe impl Sync for SafeHWND {}

/// Windows 按键模拟器
#[derive(Default)]
pub struct WindowsKeySimulator {
    target_hwnd: Option<SafeHWND>,
}

unsafe impl Send for WindowsKeySimulator {}
unsafe impl Sync for WindowsKeySimulator {}

impl WindowsKeySimulator {
    fn dispatch_key(&mut self, vk: VIRTUAL_KEY) -> Result<()> {
        if let Some(safe_hwnd) = &self.target_hwnd {
            match send_key_to_window(vk, safe_hwnd.0) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::warn!("PostMessage 失败 target_hwnd，fallback to SendInput: {}", e);
                }
            }
        }
        send_key(vk)
    }

    fn dispatch_key_down_only(&mut self, vk: VIRTUAL_KEY) -> Result<()> {
        // 选择序列必须与 SPACE/RETURN 一致走 SendInput（全局输入队列），
        // 确保 3Dmigoto/xxmi 的底层键盘钩子能捕获 VK_CLEAR 按下状态，
        // 从而匹配 [KeyMod]/[KeyGroup] 的组合条件（VK_CLEAR + VK_RETURN / VK_CLEAR + VK_SPACE）
        send_key_down_only(vk)
    }

    fn dispatch_key_up_only(&mut self, vk: VIRTUAL_KEY) -> Result<()> {
        send_key_up_only(vk)
    }

    fn dispatch_simulate_key_with_cursor(&mut self, vk: VIRTUAL_KEY, x: i32, y: i32) -> Result<()> {
        let hwnd_exists = self.target_hwnd.is_some();
        if let Some(safe_hwnd) = &self.target_hwnd {
            match simulate_key_with_cursor_window(vk, x, y, safe_hwnd.0) {
                Ok(()) => {
                    log::debug!("[dispatch_simulate_key_with_cursor] target_hwnd exists=true vk={:04x} fallback_triggered=false", vk.0);
                    return Ok(());
                }
                Err(e) => {
                    log::warn!("[dispatch_simulate_key_with_cursor] target_hwnd exists=true vk={:04x} fallback_triggered=true err={}", vk.0, e);
                }
            }
        }
        let result = simulate_key_with_cursor(vk, x, y);
        match &result {
            Ok(()) => {
                if hwnd_exists {
                    log::debug!("[dispatch_simulate_key_with_cursor] target_hwnd exists=true vk={:04x} fallback completed ok (window path failed, global path succeeded)", vk.0);
                } else {
                    log::debug!("[dispatch_simulate_key_with_cursor] target_hwnd exists=false vk={:04x} fallback completed ok (global path only)", vk.0);
                }
            }
            Err(e) => {
                log::warn!("[dispatch_simulate_key_with_cursor] target_hwnd exists={} vk={:04x} fallback completed with err={}", hwnd_exists, vk.0, e);
            }
        }
        result
    }
}

impl super::KeySimulator for WindowsKeySimulator {
    fn set_target_process(&mut self, process_name: &str) -> Result<()> {
        self.target_hwnd = find_game_window(process_name).map(SafeHWND);
        Ok(())
    }

    fn simulate_select_group(&mut self) -> Result<()> {
        self.dispatch_key(VK_CLEAR)?;
        thread::sleep(Duration::from_millis(30));
        self.dispatch_key(VK_SPACE)?;
        Ok(())
    }
    
    fn simulate_select_mod(&mut self) -> Result<()> {
        self.dispatch_key(VK_CLEAR)?;
        thread::sleep(Duration::from_millis(30));
        self.dispatch_key(VK_RETURN)?;
        Ok(())
    }

    fn simulate_f10(&mut self) -> Result<()> {
        self.dispatch_key(VK_F10)?;
        Ok(())
    }

    fn simulate_select_full(&mut self, group_idx: u32, mod_idx: u32) -> Result<()> {
        let start = std::time::Instant::now();
        log::debug!("[platform::windows] [simulate_select_full] target_hwnd exists: {:?} | group={} mod={}", self.target_hwnd.is_some(), group_idx, mod_idx);
        self.dispatch_key_down_only(VK_CLEAR)?;
        thread::sleep(Duration::from_millis(10));

        let x = mod_idx as i32;
        let y = group_idx as i32;

        let r1 = self.dispatch_simulate_key_with_cursor(VK_SPACE, x, y);
        if let Err(e) = &r1 {
            log::warn!("simulate_select_full SPACE phase failed: {}", e);
        }
        thread::sleep(Duration::from_millis(30));

        let r2 = self.dispatch_simulate_key_with_cursor(VK_RETURN, x, y);
        if let Err(e) = &r2 {
            log::warn!("simulate_select_full RETURN phase failed: {}", e);
        }

        self.dispatch_key_up_only(VK_CLEAR)?;

        let result = r1.and(r2);
        let elapsed = start.elapsed().as_millis();
        log::debug!(
            "[platform::windows] [simulate_select_full] completed | elapsed={}ms result={:?}",
            elapsed, result
        );
        result
    }

    fn simulate_select_group_at(&mut self, x: i32, y: i32) -> Result<()> {
        self.dispatch_simulate_key_with_cursor(VK_SPACE, x, y)
    }

    fn simulate_select_mod_at(&mut self, x: i32, y: i32) -> Result<()> {
        self.dispatch_simulate_key_with_cursor(VK_RETURN, x, y)
    }

    fn check_support(&self) -> Result<(), String> {
        Ok(())
    }
}

fn find_game_window(process_name: &str) -> Option<HWND> {
    {
        let mut state = ENUM_WINDOWS_STATE.lock().ok()?;
        *state = Some((process_name.to_lowercase(), None));
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, _lparam: LPARAM) -> windows::core::BOOL {
        let mut pid: u32 = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        if pid == 0 {
            return TRUE;
        }

        let handle = match unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
        } {
            Ok(h) => h,
            Err(_) => return TRUE,
        };
        if handle.is_invalid() {
            return TRUE;
        }

        let mut exe_name = [0u16; 260];
        let mut buf_len = 260u32;
        let result = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_FORMAT(0),
                PWSTR(exe_name.as_mut_ptr()),
                &mut buf_len,
            )
        };
        let _ = unsafe { CloseHandle(handle) };

        if result.is_err() {
            return TRUE;
        }

        let path = String::from_utf16_lossy(&exe_name[..buf_len as usize]);
        if let Some(name) = std::path::Path::new(&path).file_name() {
            let exe_lower = name.to_string_lossy().to_lowercase();
            let mut state_guard = match ENUM_WINDOWS_STATE.lock() {
                Ok(g) => g,
                Err(_) => return TRUE,
            };
            if let Some((target_lower, found)) = state_guard.as_mut() {
                if found.is_some() {
                    return FALSE;
                }
                if exe_lower == *target_lower {
                    *found = Some(hwnd.0 as isize);
                    return FALSE;
                }
            }
        }

        TRUE
    }

    let _ = unsafe { EnumWindows(Some(enum_proc), LPARAM(0)) };

    let state = ENUM_WINDOWS_STATE.lock().ok()?;
    let raw = state.as_ref().and_then(|s| s.1)?;
    Some(HWND(raw as *mut _))
}

fn get_scan_code(vk: VIRTUAL_KEY) -> u32 {
    unsafe {
        MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) as u32
    }
}

fn send_key_to_window(vk: VIRTUAL_KEY, hwnd: HWND) -> Result<()> {
    let scan_code = get_scan_code(vk);
    let lparam_down: LPARAM = LPARAM(0x00000001 | ((scan_code as isize) << 16));
    let lparam_up: LPARAM = LPARAM(0xC0000001 | ((scan_code as isize) << 16));
    let wparam = WPARAM(vk.0 as usize);

    let down_ok = unsafe {
        PostMessageW(Some(hwnd), WM_KEYDOWN, wparam, lparam_down).is_ok()
    };
    if !down_ok {
        log::warn!(
            "[send_key_to_window] vk={:04x} hwnd={:?} down_ok=false up_ok=skipped | WM_KEYDOWN failed, bailing",
            vk.0, hwnd
        );
        anyhow::bail!("PostMessageW WM_KEYDOWN failed");
    }
    thread::sleep(Duration::from_millis(30));

    let up_ok = unsafe {
        PostMessageW(Some(hwnd), WM_KEYUP, wparam, lparam_up).is_ok()
    };
    if !up_ok {
        log::warn!(
            "[send_key_to_window] vk={:04x} hwnd={:?} down_ok=true up_ok=false | WM_KEYUP failed, bailing",
            vk.0, hwnd
        );
        anyhow::bail!("PostMessageW WM_KEYUP failed");
    }
    log::debug!(
        "[send_key_to_window] vk={:04x} hwnd={:?} down_ok={} up_ok={}",
        vk.0, hwnd, down_ok, up_ok
    );
    Ok(())
}

fn send_key(vk: VIRTUAL_KEY) -> Result<()> {
    unsafe {
        // 按键按下（对齐 NRMM：dwFlags = 0，即普通按下）
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let down_events = SendInput(&[down], std::mem::size_of::<INPUT>() as i32);
        thread::sleep(Duration::from_millis(50));

        // 按键抬起（对齐 NRMM：KEYEVENTF_KEYUP）
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let up_events = SendInput(&[up], std::mem::size_of::<INPUT>() as i32);
        thread::sleep(Duration::from_millis(50));
        log::debug!(
            "[send_key] vk={:04x} down_sent={} up_sent={} result={}",
            vk.0, down_events, up_events, if down_events == 1 && up_events == 1 { "Ok" } else { "Failed" }
        );
    }
    Ok(())
}

fn send_key_down_only(vk: VIRTUAL_KEY) -> Result<()> {
    unsafe {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let events_sent = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        thread::sleep(Duration::from_millis(5));
        log::debug!(
            "[send_key_down_only] vk={:04x} events_sent={} result={}",
            vk.0, events_sent, if events_sent == 1 { "Ok" } else { "Failed" }
        );
    }
    Ok(())
}

fn send_key_up_only(vk: VIRTUAL_KEY) -> Result<()> {
    unsafe {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let events_sent = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        thread::sleep(Duration::from_millis(5));
        log::debug!(
            "[send_key_up_only] vk={:04x} events_sent={} result={}",
            vk.0, events_sent, if events_sent == 1 { "Ok" } else { "Failed" }
        );
    }
    Ok(())
}

fn simulate_key_with_cursor_window(vk: VIRTUAL_KEY, x: i32, y: i32, _hwnd: HWND) -> Result<()> {
    unsafe {
        let mut initial_pos = POINT { x: 0, y: 0 };
        let has_initial = GetCursorPos(&mut initial_pos).is_ok();

        let set_cursor_result = SetCursorPos(x, y).is_ok();
        if !set_cursor_result {
            log::warn!("[simulate_key_with_cursor_window] fallback_triggered=true reason=SetCursorPos failed | vk={:04x} target=({}, {})", vk.0, x, y);
            return send_key(vk);
        }

        thread::sleep(Duration::from_millis(10));

        let rect = RECT {
            left: x,
            top: y,
            right: x + 1,
            bottom: y + 1,
        };
        let clip_result = ClipCursor(Some(&rect)).is_ok();

        // 与 NRMM 一致使用 SendInput（全局输入队列），
        // 确保 3Dmigoto/xxmi 的底层键盘钩子能捕获按键并读取光标坐标
        let result = send_key(vk);

        let clip_restore_result = ClipCursor(None).is_ok();

        let restore_result = if has_initial {
            SetCursorPos(initial_pos.x, initial_pos.y).is_ok()
        } else {
            false
        };

        log::debug!(
            "[simulate_key_with_cursor_window] vk={:04x} target=({}, {}) set_cursor_result={} clip_result={} clip_restore_result={} restore_result={}",
            vk.0, x, y, set_cursor_result, clip_result, clip_restore_result, restore_result
        );

        result
    }
}

fn simulate_key_with_cursor(vk: VIRTUAL_KEY, x: i32, y: i32) -> Result<()> {
    unsafe {
        let mut initial_pos = POINT { x: 0, y: 0 };
        let has_initial = GetCursorPos(&mut initial_pos).is_ok();

        let set_cursor_result = SetCursorPos(x, y).is_ok();
        if !set_cursor_result {
            log::warn!("[simulate_key_with_cursor] fallback_triggered=true reason=SetCursorPos failed | vk={:04x} target=({}, {})", vk.0, x, y);
            return send_key(vk);
        }

        thread::sleep(Duration::from_millis(10));

        let rect = RECT {
            left: x,
            top: y,
            right: x + 1,
            bottom: y + 1,
        };
        let clip_result = ClipCursor(Some(&rect)).is_ok();

        let result = send_key(vk);

        let clip_restore_result = ClipCursor(None).is_ok();

        let restore_result = if has_initial {
            SetCursorPos(initial_pos.x, initial_pos.y).is_ok()
        } else {
            false
        };

        log::debug!(
            "[simulate_key_with_cursor] vk={:04x} target=({}, {}) set_cursor_result={} clip_result={} clip_restore_result={} restore_result={}",
            vk.0, x, y, set_cursor_result, clip_result, clip_restore_result, restore_result
        );

        result
    }
}

/// Windows 前台窗口检测器
pub struct WindowsForegroundDetector;

impl super::ForegroundDetector for WindowsForegroundDetector {
    fn get_foreground_process_name(&self) -> Result<String> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() {
                return Err(anyhow!("No foreground window"));
            }
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return Err(anyhow!("Failed to get process ID"));
            }
            
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .context("Failed to open process")?;
            if handle.is_invalid() {
                return Err(anyhow!("Failed to open process: invalid handle"));
            }
            
            let mut exe_name = [0u16; 260];
            let mut buf_len = 260u32;
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_FORMAT(0),
                PWSTR(exe_name.as_mut_ptr()),
                &mut buf_len,
            ).context("QueryFullProcessImageNameW failed")?;
            CloseHandle(handle).ok();
            
            let path = String::from_utf16_lossy(&exe_name[..buf_len as usize]);
            let name = std::path::Path::new(&path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            Ok(name.trim_end_matches('\0').to_string())
        }
    }
    
    fn get_cursor_position(&self) -> Result<(i32, i32)> {
        unsafe {
            let mut point = POINT { x: 0, y: 0 };
            GetCursorPos(&mut point).context("GetCursorPos failed")?;
            Ok((point.x, point.y))
        }
    }
}
