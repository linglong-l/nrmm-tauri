//! Windows 平台实现模块
//!
//! 使用 Win32 API 实现：
//! - 按键模拟：SendInput 发送键盘事件（全局统一，不走 PostMessageW 定向——
//!   3Dmigoto 底层键盘钩子与防作弊均忽略/不信任 PostMessage 合成的按键）
//! - 前台窗口检测：GetForegroundWindow + GetWindowThreadProcessId + QueryFullProcessImageNameW
//! - 光标位置获取：GetCursorPos
//!
//! 本文件为按键模拟域（WindowsKeySimulator / 键盘事件发送）。
//!
//! # unsafe 说明
//! 所有 Win32 API 调用都在 unsafe 块中，因为它们是 C FFI 调用。
//! 安全性通过正确传递指针和缓冲区大小保证：
//! - 缓冲区大小固定为 MAX_PATH (260)，避免溢出
//! - 句柄使用后调用 CloseHandle 释放
//! - 检查句柄有效性后再使用
//!
//! ## Send/Sync 安全性
//! - `WindowsKeySimulator` 无内部可变状态，所有 Win32 调用
//!   （SendInput / SetCursorPos / ClipCursor / GetCursorPos）均为线程安全。
//!
//! ## 发送策略（2026-09-23 起全局化）
//! 全部按键统一经 SendInput 全局输入队列发送，不再做 PostMessageW 定向投递：
//! - 3Dmigoto/xxmi 的底层键盘钩子只捕获真实输入队列事件；
//! - 游戏进程对 WM_KEYDOWN/UP 消息的防作弊/反注入校验会忽略合成消息。
//!
//! 故 `set_target_process` 保留为兼容空实现，不再查找窗口句柄。

use super::windows_cursor_sim::simulate_key_with_cursor;
use crate::sel_dbg;
use anyhow::Result;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Windows 按键模拟器
#[derive(Default)]
pub struct WindowsKeySimulator;

// SAFETY: WindowsKeySimulator 无内部可变状态，所有 Win32 调用
// （SendInput/SetCursorPos/ClipCursor/GetCursorPos）均为线程安全。
unsafe impl Send for WindowsKeySimulator {}
// SAFETY: &WindowsKeySimulator 仅提供不可变访问；Win32 API 调用线程安全。
unsafe impl Sync for WindowsKeySimulator {}

impl WindowsKeySimulator {
    fn dispatch_key(&mut self, vk: VIRTUAL_KEY) -> Result<()> {
        // 全局统一走 SendInput（与 simulate_f10 / simulate_combo 同路径）：
        // 3Dmigoto/xxmi 底层键盘钩子与防作弊均忽略 PostMessage 合成的按键
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
        // 全局统一走 SendInput（光标联动）：window 版定向路径已移除，
        // 与 simulate_key_with_cursor 同构（SetCursorPos + ClipCursor + SendInput）
        simulate_key_with_cursor(vk, x, y)
    }
}

impl super::KeySimulator for WindowsKeySimulator {
    fn set_target_process(&mut self, _process_name: &str) -> Result<()> {
        // 自 2026-09-23 起统一使用全局 SendInput（防作弊/底层钩子不捕获
        // PostMessage 定向消息），不再需要目标窗口句柄；保留接口以兼容
        // 调用方与跨平台 trait 契约。
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

    fn simulate_combo(&mut self, keys: &[u16]) -> Result<()> {
        // 对齐 NRMM `_simulateMultipleKeypresses` 时序：
        // 逐键按下（30ms 间隔）→ 保持 50ms → 逆序抬起（30ms 间隔）
        // 使用全局 SendInput（send_key_down_only/up），与 simulate_f10 同路径，
        // 不经过 dispatch_* 的 PostMessage 定向发送
        if keys.is_empty() {
            return Ok(());
        }
        for &k in keys {
            send_key_down_only(VIRTUAL_KEY(k))?;
            thread::sleep(Duration::from_millis(30));
        }
        thread::sleep(Duration::from_millis(50));
        for &k in keys.iter().rev() {
            send_key_up_only(VIRTUAL_KEY(k))?;
            thread::sleep(Duration::from_millis(30));
        }
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
            "调用链末端(最终执行函数) | 入口 | 全局 SendInput 模式 分组索引={} 模组索引={} 映射光标坐标 x(模组)={} y(分组)={}",
            group_idx,
            mod_idx,
            x,
            y
        );
        log::debug!(
            "[platform::windows] [simulate_select_full] 全局 SendInput 模式 | group={} mod={}",
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

pub(crate) fn send_key(vk: VIRTUAL_KEY) -> Result<()> {
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
