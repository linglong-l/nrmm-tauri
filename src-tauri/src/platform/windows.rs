//! Windows 平台实现模块
//!
//! 使用 Win32 API 实现：
//! - 按键模拟：SendInput 发送键盘事件
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

/// Windows 按键模拟器
pub struct WindowsKeySimulator;

impl super::KeySimulator for WindowsKeySimulator {
    fn simulate_select_group(&self) -> Result<()> {
        send_key(VK_CLEAR)?;
        thread::sleep(Duration::from_millis(30));
        send_key(VK_SPACE)?;
        Ok(())
    }
    
    fn simulate_select_mod(&self) -> Result<()> {
        send_key(VK_CLEAR)?;
        thread::sleep(Duration::from_millis(30));
        send_key(VK_RETURN)?;
        Ok(())
    }

    fn simulate_f10(&self) -> Result<()> {
        send_key(VK_F10)?;
        Ok(())
    }

    fn simulate_select_group_at(&self, x: i32, y: i32) -> Result<()> {
        simulate_key_with_cursor(VK_SPACE, x, y)
    }

    fn simulate_select_mod_at(&self, x: i32, y: i32) -> Result<()> {
        simulate_key_with_cursor(VK_RETURN, x, y)
    }

    fn check_support(&self) -> Result<(), String> {
        Ok(())
    }
}

/// 发送单个按键事件（按下+释放）
///
/// 使用 SendInput 发送 KEYUP → KEYDOWN(EXTENDEDKEY) → KEYUP 序列，
/// 模拟真实按键行为，间隔 50ms 确保游戏能正确接收。
fn send_key(vk: VIRTUAL_KEY) -> Result<()> {
    unsafe {
        let mut inputs = [
            INPUT {
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
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: KEYEVENTF_EXTENDEDKEY,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        thread::sleep(Duration::from_millis(50));
        
        inputs[1].Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
        SendInput(&inputs[1..], std::mem::size_of::<INPUT>() as i32);
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// 发送按键并锁定光标在目标位置（与 NRMM 的 _simulateSelectGroupMod 对齐）
///
/// 执行流程：
/// 1. 获取当前光标位置保存
/// 2. 移动光标到目标坐标
/// 3. 锁定光标到 1x1 像素区域
/// 4. 发送按键
/// 5. 解锁光标
/// 6. 恢复光标到初始位置
///
/// 光标绑定失败时降级为无绑定模式
fn simulate_key_with_cursor(vk: VIRTUAL_KEY, x: i32, y: i32) -> Result<()> {
    unsafe {
        // 保存初始光标位置
        let mut initial_pos = POINT { x: 0, y: 0 };
        let has_initial = GetCursorPos(&mut initial_pos).is_ok();

        // 移动光标到目标位置
        if SetCursorPos(x, y).is_err() {
            log::warn!("SetCursorPos failed, falling back to cursor-free key simulation");
            return send_key(vk);
        }

        thread::sleep(Duration::from_millis(10));

        // 锁定光标到 1x1 像素区域
        let rect = RECT {
            left: x,
            top: y,
            right: x + 1,
            bottom: y + 1,
        };
        let _ = ClipCursor(Some(&rect));

        // 发送按键
        let result = send_key(vk);

        // 解锁光标
        let _ = ClipCursor(None);

        // 恢复光标位置
        if has_initial {
            let _ = SetCursorPos(initial_pos.x, initial_pos.y);
        }

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
