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
//!
//! ## Send/Sync 安全性
//! - `SafeHWND`: HWND 是进程级窗口句柄，Win32 API 接受 HWND 的调用
//!   （PostMessageW 等）均为线程安全；HWND 在窗口生命周期内有效
//! - `WindowsKeySimulator`: 仅包含 SafeHWND，所有 Win32 调用线程安全
//!
//! ## 回调安全性
//! - `enum_proc`: Win32 `EnumWindows` 在调用线程上同步执行回调，
//!   无并发重入风险；回调内访问 `ENUM_WINDOWS_STATE` 的 Mutex 被外层
//!   `find_game_window` 的锁获取/释放正确包裹（状态在回调前设置、回调后读取）

use crate::sel_dbg;
use anyhow::{anyhow, Context, Result};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use windows::core::PWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static ENUM_WINDOWS_STATE: Mutex<Option<(String, Option<isize>)>> = Mutex::new(None);

struct SafeHWND(HWND);
// SAFETY: HWND is a process-scoped window handle (isize). All Win32 API calls
// that accept HWND are thread-safe for the lifetime of the window. Since we
// only use HWND to send window messages (PostMessageW is thread-safe), and
// the window is guaranteed to exist until the application exits, sending
// SafeHWND across threads is safe.
unsafe impl Send for SafeHWND {}
// SAFETY: HWND is an opaque handle; &HWND is safe to share across threads
// because Win32 API calls that accept HWND by value do not mutate the caller's
// copy. PostMessageW is documented as thread-safe.
unsafe impl Sync for SafeHWND {}

/// Windows 按键模拟器
#[derive(Default)]
pub struct WindowsKeySimulator {
    target_hwnd: Option<SafeHWND>,
}

// SAFETY: WindowsKeySimulator contains only SafeHWND (already Send) and
// uses Win32 API calls (SendInput/PostMessageW) that are thread-safe.
// The struct is #[derive(Default)] and has no unsynchronized mutable state
// that would be UB to share across threads.
unsafe impl Send for WindowsKeySimulator {}
// SAFETY: &WindowsKeySimulator provides only immutable access to SafeHWND;
// all Win32 API calls through it are thread-safe as documented by MSDN.
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
        // 对齐 NRMM：F10 使用 SendInput（全局输入队列），
        // 不使用 PostMessage —— 3Dmigoto 的底层键盘钩子不捕获 PostMessage 合成的按键
        send_key(VK_F10)?;
        Ok(())
    }

    fn simulate_select_full(&mut self, group_idx: u32, mod_idx: u32) -> Result<()> {
        let start = std::time::Instant::now();
        // 注意：NRMM 约定 x = 模组索引(mod_idx)，y = 分组索引(group_idx)
        let x = mod_idx as i32;
        let y = group_idx as i32;
        sel_dbg!(
            "platform::windows",
            "simulate_select_full",
            "调用链末端(最终执行函数) | 入口 | target_hwnd={:?} 分组索引={} 模组索引={} 映射光标坐标 x(模组)={} y(分组)={}",
            self.target_hwnd.is_some(),
            group_idx,
            mod_idx,
            x,
            y
        );
        log::debug!(
            "[platform::windows] [simulate_select_full] target_hwnd exists: {:?} | group={} mod={}",
            self.target_hwnd.is_some(),
            group_idx,
            mod_idx
        );
        sel_dbg!(
            "platform::windows",
            "simulate_select_full",
            "阶段=按下 VK_CLEAR（组合键起始，保持按住）"
        );
        let t_vkclear_down_before = std::time::Instant::now();
        self.dispatch_key_down_only(VK_CLEAR)?;
        let t_vkclear_down_after = std::time::Instant::now();
        log::debug!(
            "[simulate_select_full] phase=VK_CLEAR_DOWN x={} y={} duration_ms={} result=Ok",
            x,
            y,
            t_vkclear_down_after
                .duration_since(t_vkclear_down_before)
                .as_millis()
        );
        thread::sleep(Duration::from_millis(10));

        log::debug!(
            "[simulate_select_full] phase=VK_SPACE before | x={} y={}",
            x,
            y
        );
        let t_vkspace_before = std::time::Instant::now();
        let r1 = self.dispatch_simulate_key_with_cursor(VK_SPACE, x, y);
        let t_vkspace_after = std::time::Instant::now();
        log::debug!(
            "[simulate_select_full] phase=VK_SPACE after | x={} y={} duration_ms={} result={:?}",
            x,
            y,
            t_vkspace_after.duration_since(t_vkspace_before).as_millis(),
            r1
        );
        if let Err(e) = &r1 {
            log::warn!("simulate_select_full SPACE phase failed: {}", e);
        }
        sel_dbg!(
            "platform::windows",
            "simulate_select_full",
            "阶段=发送 SPACE + 移动光标到 ({}, {})（选择分组）结果={:?}",
            x,
            y,
            r1.is_ok()
        );
        thread::sleep(Duration::from_millis(30));

        log::debug!(
            "[simulate_select_full] phase=VK_RETURN before | x={} y={}",
            x,
            y
        );
        let t_vkreturn_before = std::time::Instant::now();
        let r2 = self.dispatch_simulate_key_with_cursor(VK_RETURN, x, y);
        let t_vkreturn_after = std::time::Instant::now();
        log::debug!(
            "[simulate_select_full] phase=VK_RETURN after | x={} y={} duration_ms={} result={:?}",
            x,
            y,
            t_vkreturn_after
                .duration_since(t_vkreturn_before)
                .as_millis(),
            r2
        );
        if let Err(e) = &r2 {
            log::warn!("simulate_select_full RETURN phase failed: {}", e);
        }
        sel_dbg!(
            "platform::windows",
            "simulate_select_full",
            "阶段=发送 RETURN + 移动光标到 ({}, {})（选择模组）结果={:?}",
            x,
            y,
            r2.is_ok()
        );

        sel_dbg!(
            "platform::windows",
            "simulate_select_full",
            "阶段=抬起 VK_CLEAR（组合键结束，松开）"
        );
        let t_vkclear_up_before = std::time::Instant::now();
        self.dispatch_key_up_only(VK_CLEAR)?;
        let t_vkclear_up_after = std::time::Instant::now();
        log::debug!(
            "[simulate_select_full] phase=VK_CLEAR_UP x={} y={} duration_ms={} result=Ok",
            x,
            y,
            t_vkclear_up_after
                .duration_since(t_vkclear_up_before)
                .as_millis()
        );

        let result = r1.and(r2);
        let elapsed = start.elapsed().as_millis();
        log::debug!(
            "[platform::windows] [simulate_select_full] completed | elapsed={}ms result={:?}",
            elapsed,
            result
        );
        sel_dbg!(
            "platform::windows",
            "simulate_select_full",
            "调用链完成 | 耗时={}ms 最终结果={:?}",
            elapsed,
            result
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

        let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
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
            let mut state_guard = match ENUM_WINDOWS_STATE.lock() {
                Ok(g) => g,
                Err(_) => return TRUE,
            };
            if let Some((target_lower, found)) = state_guard.as_mut() {
                if found.is_some() {
                    return FALSE;
                }
                if name
                    .to_string_lossy()
                    .eq_ignore_ascii_case(target_lower.as_str())
                {
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
    unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) as u32 }
}

fn send_key_to_window(vk: VIRTUAL_KEY, hwnd: HWND) -> Result<()> {
    let scan_code = get_scan_code(vk);
    let lparam_down: LPARAM = LPARAM(0x00000001 | ((scan_code as isize) << 16));
    let lparam_up: LPARAM = LPARAM(0xC0000001 | ((scan_code as isize) << 16));
    let wparam = WPARAM(vk.0 as usize);

    let down_ok = unsafe { PostMessageW(Some(hwnd), WM_KEYDOWN, wparam, lparam_down).is_ok() };
    if !down_ok {
        log::warn!(
            "[send_key_to_window] vk={:04x} hwnd={:?} down_ok=false up_ok=skipped | WM_KEYDOWN failed, bailing",
            vk.0, hwnd
        );
        anyhow::bail!("PostMessageW WM_KEYDOWN failed");
    }
    thread::sleep(Duration::from_millis(30));

    let up_ok = unsafe { PostMessageW(Some(hwnd), WM_KEYUP, wparam, lparam_up).is_ok() };
    if !up_ok {
        log::warn!(
            "[send_key_to_window] vk={:04x} hwnd={:?} down_ok=true up_ok=false | WM_KEYUP failed, bailing",
            vk.0, hwnd
        );
        anyhow::bail!("PostMessageW WM_KEYUP failed");
    }
    log::debug!(
        "[send_key_to_window] vk={:04x} hwnd={:?} down_ok={} up_ok={}",
        vk.0,
        hwnd,
        down_ok,
        up_ok
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
        // 诊断：SendInput down 后立即 GetCursorPos，验证光标是否在预期位置
        let mut cursor_after_down = POINT { x: 0, y: 0 };
        let got_after_down = GetCursorPos(&mut cursor_after_down).is_ok();
        log::debug!(
            "[send_key] vk={:04x} down_sent={} cursor_after_down=({}, {}) got={}",
            vk.0,
            down_events,
            cursor_after_down.x,
            cursor_after_down.y,
            got_after_down
        );
        thread::sleep(Duration::from_millis(50));

        // 诊断：sleep 50ms 后、SendInput up 前再次 GetCursorPos，
        // 与 down 后对比，验证按键保持期间光标是否漂移（3Dmigoto 可能读取到错误坐标）
        let mut cursor_before_up = POINT { x: 0, y: 0 };
        let got_before_up = GetCursorPos(&mut cursor_before_up).is_ok();
        if got_after_down
            && got_before_up
            && (cursor_after_down.x != cursor_before_up.x
                || cursor_after_down.y != cursor_before_up.y)
        {
            log::warn!("[send_key] Cursor drifted during key hold | after_down=({}, {}) before_up=({}, {})", cursor_after_down.x, cursor_after_down.y, cursor_before_up.x, cursor_before_up.y);
        }

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
        // 诊断：SendInput up 后 GetCursorPos，记录按键抬起后的光标位置
        let mut cursor_after_up = POINT { x: 0, y: 0 };
        let got_after_up = GetCursorPos(&mut cursor_after_up).is_ok();
        thread::sleep(Duration::from_millis(50));
        log::debug!(
            "[send_key] vk={:04x} down_sent={} up_sent={} result={} cursor_before_up=({}, {}) cursor_after_up=({}, {}) got_after_up={}",
            vk.0, down_events, up_events, if down_events == 1 && up_events == 1 { "Ok" } else { "Failed" }, cursor_before_up.x, cursor_before_up.y, cursor_after_up.x, cursor_after_up.y, got_after_up
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
            vk.0,
            events_sent,
            if events_sent == 1 { "Ok" } else { "Failed" }
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
            vk.0,
            events_sent,
            if events_sent == 1 { "Ok" } else { "Failed" }
        );
    }
    Ok(())
}

fn simulate_key_with_cursor_window(vk: VIRTUAL_KEY, x: i32, y: i32, _hwnd: HWND) -> Result<()> {
    unsafe {
        let mut initial_pos = POINT { x: 0, y: 0 };
        let has_initial = GetCursorPos(&mut initial_pos).is_ok();
        sel_dbg!(
            "platform::windows",
            "simulate_key_with_cursor_window",
            "移动鼠标前光标坐标=({},{}) | 即将移动到目标=({},{})（vk={:04x}）",
            if has_initial { initial_pos.x } else { 0 },
            if has_initial { initial_pos.y } else { 0 },
            x,
            y,
            vk.0
        );

        let set_cursor_result = SetCursorPos(x, y).is_ok();
        sel_dbg!(
            "platform::windows",
            "simulate_key_with_cursor_window",
            "移动鼠标后光标坐标=({},{})（SetCursorPos 结果={}）",
            x,
            y,
            set_cursor_result
        );
        if !set_cursor_result {
            log::warn!("[simulate_key_with_cursor_window] fallback_triggered=true reason=SetCursorPos failed | vk={:04x} target=({}, {})", vk.0, x, y);
            return send_key(vk);
        }

        // 验证 SetCursorPos 是否实际生效：调用后立即 GetCursorPos 对比期望坐标
        let mut actual_after_set = POINT { x: 0, y: 0 };
        let got_after_set = GetCursorPos(&mut actual_after_set).is_ok();
        log::debug!(
            "[simulate_key_with_cursor_window] vk={:04x} SetCursorPos verify | expected_cursor=({}, {}) actual_cursor=({}, {}) got={}",
            vk.0, x, y, actual_after_set.x, actual_after_set.y, got_after_set
        );
        if got_after_set && (actual_after_set.x != x || actual_after_set.y != y) {
            log::warn!("[simulate_key_with_cursor_window] SetCursorPos did not take effect | expected=({}, {}) actual=({}, {})", x, y, actual_after_set.x, actual_after_set.y);
        }

        thread::sleep(Duration::from_millis(10));

        let rect = RECT {
            left: x,
            top: y,
            right: x + 1,
            bottom: y + 1,
        };
        let clip_result = ClipCursor(Some(&rect)).is_ok();

        // 验证 ClipCursor 锁定是否生效：锁定后 GetCursorPos 检查光标是否仍在矩形内
        let mut cursor_after_clip = POINT { x: 0, y: 0 };
        let got_after_clip = GetCursorPos(&mut cursor_after_clip).is_ok();
        log::debug!(
            "[simulate_key_with_cursor_window] vk={:04x} ClipCursor verify | clip_rect=({}, {}, {}, {}) cursor_after_clip=({}, {}) got={}",
            vk.0, rect.left, rect.top, rect.right, rect.bottom, cursor_after_clip.x, cursor_after_clip.y, got_after_clip
        );
        if got_after_clip
            && (cursor_after_clip.x < rect.left
                || cursor_after_clip.x >= rect.right
                || cursor_after_clip.y < rect.top
                || cursor_after_clip.y >= rect.bottom)
        {
            log::warn!("[simulate_key_with_cursor_window] Cursor escaped clip region | clip_rect=({}, {}, {}, {}) cursor=({}, {})", rect.left, rect.top, rect.right, rect.bottom, cursor_after_clip.x, cursor_after_clip.y);
        }

        // 与 NRMM 一致使用 SendInput（全局输入队列），
        // 确保 3Dmigoto/xxmi 的底层键盘钩子能捕获按键并读取光标坐标
        // send_key 前后记录光标位置 + 时序诊断
        let mut cursor_before_sendkey = POINT { x: 0, y: 0 };
        let got_before_sendkey = GetCursorPos(&mut cursor_before_sendkey).is_ok();
        log::debug!(
            "[simulate_key_with_cursor_window] vk={:04x} cursor_before_sendkey=({}, {}) got={}",
            vk.0,
            cursor_before_sendkey.x,
            cursor_before_sendkey.y,
            got_before_sendkey
        );
        let t_sendkey_before = std::time::Instant::now();
        let result = send_key(vk);
        let t_sendkey_after = std::time::Instant::now();
        let sendkey_duration = t_sendkey_after.duration_since(t_sendkey_before).as_millis();
        let mut cursor_after_sendkey = POINT { x: 0, y: 0 };
        let got_after_sendkey = GetCursorPos(&mut cursor_after_sendkey).is_ok();
        log::debug!(
            "[simulate_key_with_cursor_window] vk={:04x} cursor_after_sendkey=({}, {}) got={} sendkey_duration_ms={}",
            vk.0, cursor_after_sendkey.x, cursor_after_sendkey.y, got_after_sendkey, sendkey_duration
        );

        let clip_restore_result = ClipCursor(None).is_ok();
        let t_clip_restore_after = std::time::Instant::now();
        let delay_sendkey_to_clip_restore = t_clip_restore_after
            .duration_since(t_sendkey_after)
            .as_millis();
        let mut cursor_after_clip_restore = POINT { x: 0, y: 0 };
        let got_after_clip_restore = GetCursorPos(&mut cursor_after_clip_restore).is_ok();
        log::debug!(
            "[simulate_key_with_cursor_window] vk={:04x} ClipCursor(None) done | cursor_after_clip_restore=({}, {}) got={} clip_restore_result={} delay_sendkey_to_clip_restore_ms={}",
            vk.0, cursor_after_clip_restore.x, cursor_after_clip_restore.y, got_after_clip_restore, clip_restore_result, delay_sendkey_to_clip_restore
        );

        let restore_result = if has_initial {
            SetCursorPos(initial_pos.x, initial_pos.y).is_ok()
        } else {
            false
        };
        let t_cursor_restore_after = std::time::Instant::now();
        let delay_sendkey_to_cursor_restore = t_cursor_restore_after
            .duration_since(t_sendkey_after)
            .as_millis();
        // 验证光标恢复是否成功：调用后立即 GetCursorPos 对比期望坐标
        let mut cursor_after_restore = POINT { x: 0, y: 0 };
        let got_after_restore = GetCursorPos(&mut cursor_after_restore).is_ok();
        log::debug!(
            "[simulate_key_with_cursor_window] vk={:04x} cursor restore verify | expected=({}, {}) actual=({}, {}) got={} restore_result={} delay_sendkey_to_cursor_restore_ms={}",
            vk.0, initial_pos.x, initial_pos.y, cursor_after_restore.x, cursor_after_restore.y, got_after_restore, restore_result, delay_sendkey_to_cursor_restore
        );
        if has_initial
            && got_after_restore
            && (cursor_after_restore.x != initial_pos.x || cursor_after_restore.y != initial_pos.y)
        {
            log::warn!("[simulate_key_with_cursor_window] SetCursorPos(initial) did not take effect | expected=({}, {}) actual=({}, {})", initial_pos.x, initial_pos.y, cursor_after_restore.x, cursor_after_restore.y);
        }

        sel_dbg!(
            "platform::windows",
            "simulate_key_with_cursor_window",
            "光标已还原至原位置=({},{}) 还原结果={}（clip 恢复结果={}）",
            if has_initial { initial_pos.x } else { 0 },
            if has_initial { initial_pos.y } else { 0 },
            restore_result,
            clip_restore_result
        );

        log::debug!(
            "[simulate_key_with_cursor_window] vk={:04x} target=({}, {}) set_cursor_result={} clip_result={} clip_restore_result={} restore_result={} sendkey_duration_ms={} delay_sendkey_to_clip_restore_ms={} delay_sendkey_to_cursor_restore_ms={}",
            vk.0, x, y, set_cursor_result, clip_result, clip_restore_result, restore_result, sendkey_duration, delay_sendkey_to_clip_restore, delay_sendkey_to_cursor_restore
        );

        result
    }
}

fn simulate_key_with_cursor(vk: VIRTUAL_KEY, x: i32, y: i32) -> Result<()> {
    unsafe {
        let mut initial_pos = POINT { x: 0, y: 0 };
        let has_initial = GetCursorPos(&mut initial_pos).is_ok();
        sel_dbg!(
            "platform::windows",
            "simulate_key_with_cursor",
            "移动鼠标前光标坐标=({},{}) | 即将移动到目标=({},{})（vk={:04x}）",
            if has_initial { initial_pos.x } else { 0 },
            if has_initial { initial_pos.y } else { 0 },
            x,
            y,
            vk.0
        );

        let set_cursor_result = SetCursorPos(x, y).is_ok();
        sel_dbg!(
            "platform::windows",
            "simulate_key_with_cursor",
            "移动鼠标后光标坐标=({},{})（SetCursorPos 结果={}）",
            x,
            y,
            set_cursor_result
        );
        if !set_cursor_result {
            log::warn!("[simulate_key_with_cursor] fallback_triggered=true reason=SetCursorPos failed | vk={:04x} target=({}, {})", vk.0, x, y);
            return send_key(vk);
        }

        // 验证 SetCursorPos 是否实际生效：调用后立即 GetCursorPos 对比期望坐标
        let mut actual_after_set = POINT { x: 0, y: 0 };
        let got_after_set = GetCursorPos(&mut actual_after_set).is_ok();
        log::debug!(
            "[simulate_key_with_cursor] vk={:04x} SetCursorPos verify | expected_cursor=({}, {}) actual_cursor=({}, {}) got={}",
            vk.0, x, y, actual_after_set.x, actual_after_set.y, got_after_set
        );
        if got_after_set && (actual_after_set.x != x || actual_after_set.y != y) {
            log::warn!("[simulate_key_with_cursor] SetCursorPos did not take effect | expected=({}, {}) actual=({}, {})", x, y, actual_after_set.x, actual_after_set.y);
        }

        thread::sleep(Duration::from_millis(10));

        let rect = RECT {
            left: x,
            top: y,
            right: x + 1,
            bottom: y + 1,
        };
        let clip_result = ClipCursor(Some(&rect)).is_ok();

        // 验证 ClipCursor 锁定是否生效：锁定后 GetCursorPos 检查光标是否仍在矩形内
        let mut cursor_after_clip = POINT { x: 0, y: 0 };
        let got_after_clip = GetCursorPos(&mut cursor_after_clip).is_ok();
        log::debug!(
            "[simulate_key_with_cursor] vk={:04x} ClipCursor verify | clip_rect=({}, {}, {}, {}) cursor_after_clip=({}, {}) got={}",
            vk.0, rect.left, rect.top, rect.right, rect.bottom, cursor_after_clip.x, cursor_after_clip.y, got_after_clip
        );
        if got_after_clip
            && (cursor_after_clip.x < rect.left
                || cursor_after_clip.x >= rect.right
                || cursor_after_clip.y < rect.top
                || cursor_after_clip.y >= rect.bottom)
        {
            log::warn!("[simulate_key_with_cursor] Cursor escaped clip region | clip_rect=({}, {}, {}, {}) cursor=({}, {})", rect.left, rect.top, rect.right, rect.bottom, cursor_after_clip.x, cursor_after_clip.y);
        }

        // send_key 前后记录光标位置 + 时序诊断
        let mut cursor_before_sendkey = POINT { x: 0, y: 0 };
        let got_before_sendkey = GetCursorPos(&mut cursor_before_sendkey).is_ok();
        log::debug!(
            "[simulate_key_with_cursor] vk={:04x} cursor_before_sendkey=({}, {}) got={}",
            vk.0,
            cursor_before_sendkey.x,
            cursor_before_sendkey.y,
            got_before_sendkey
        );
        let t_sendkey_before = std::time::Instant::now();
        let result = send_key(vk);
        let t_sendkey_after = std::time::Instant::now();
        let sendkey_duration = t_sendkey_after.duration_since(t_sendkey_before).as_millis();
        let mut cursor_after_sendkey = POINT { x: 0, y: 0 };
        let got_after_sendkey = GetCursorPos(&mut cursor_after_sendkey).is_ok();
        log::debug!(
            "[simulate_key_with_cursor] vk={:04x} cursor_after_sendkey=({}, {}) got={} sendkey_duration_ms={}",
            vk.0, cursor_after_sendkey.x, cursor_after_sendkey.y, got_after_sendkey, sendkey_duration
        );

        let clip_restore_result = ClipCursor(None).is_ok();
        let t_clip_restore_after = std::time::Instant::now();
        let delay_sendkey_to_clip_restore = t_clip_restore_after
            .duration_since(t_sendkey_after)
            .as_millis();
        let mut cursor_after_clip_restore = POINT { x: 0, y: 0 };
        let got_after_clip_restore = GetCursorPos(&mut cursor_after_clip_restore).is_ok();
        log::debug!(
            "[simulate_key_with_cursor] vk={:04x} ClipCursor(None) done | cursor_after_clip_restore=({}, {}) got={} clip_restore_result={} delay_sendkey_to_clip_restore_ms={}",
            vk.0, cursor_after_clip_restore.x, cursor_after_clip_restore.y, got_after_clip_restore, clip_restore_result, delay_sendkey_to_clip_restore
        );

        let restore_result = if has_initial {
            SetCursorPos(initial_pos.x, initial_pos.y).is_ok()
        } else {
            false
        };
        let t_cursor_restore_after = std::time::Instant::now();
        let delay_sendkey_to_cursor_restore = t_cursor_restore_after
            .duration_since(t_sendkey_after)
            .as_millis();
        // 验证光标恢复是否成功：调用后立即 GetCursorPos 对比期望坐标
        let mut cursor_after_restore = POINT { x: 0, y: 0 };
        let got_after_restore = GetCursorPos(&mut cursor_after_restore).is_ok();
        log::debug!(
            "[simulate_key_with_cursor] vk={:04x} cursor restore verify | expected=({}, {}) actual=({}, {}) got={} restore_result={} delay_sendkey_to_cursor_restore_ms={}",
            vk.0, initial_pos.x, initial_pos.y, cursor_after_restore.x, cursor_after_restore.y, got_after_restore, restore_result, delay_sendkey_to_cursor_restore
        );
        if has_initial
            && got_after_restore
            && (cursor_after_restore.x != initial_pos.x || cursor_after_restore.y != initial_pos.y)
        {
            log::warn!("[simulate_key_with_cursor] SetCursorPos(initial) did not take effect | expected=({}, {}) actual=({}, {})", initial_pos.x, initial_pos.y, cursor_after_restore.x, cursor_after_restore.y);
        }

        sel_dbg!(
            "platform::windows",
            "simulate_key_with_cursor",
            "光标已还原至原位置=({},{}) 还原结果={}（clip 恢复结果={}）",
            if has_initial { initial_pos.x } else { 0 },
            if has_initial { initial_pos.y } else { 0 },
            restore_result,
            clip_restore_result
        );

        log::debug!(
            "[simulate_key_with_cursor] vk={:04x} target=({}, {}) set_cursor_result={} clip_result={} clip_restore_result={} restore_result={} sendkey_duration_ms={} delay_sendkey_to_clip_restore_ms={} delay_sendkey_to_cursor_restore_ms={}",
            vk.0, x, y, set_cursor_result, clip_result, clip_restore_result, restore_result, sendkey_duration, delay_sendkey_to_clip_restore, delay_sendkey_to_cursor_restore
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
            )
            .context("QueryFullProcessImageNameW failed")?;
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
