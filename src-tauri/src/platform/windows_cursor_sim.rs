//! Windows 平台实现模块——光标联动按键模拟域
//!
//! 将光标移动到目标坐标并合成按键（SendInput），对齐 NRMM `SetCursorPos`+
//! `ClipCursor` 时序，使 3Dmigoto/xxmi 底层钩子能读取到正确光标坐标：
//! - `simulate_key_with_cursor_window`: 定向路径（保留 `_hwnd` 参数以隔离裁剪，实际走全局输入）
//! - `simulate_key_with_cursor`: 全局回退路径（无 target_hwnd 时使用）
//!
//! # unsafe 说明
//! 所有 Win32 API 调用都在 unsafe 块中，因为它们是 C FFI 调用。
//! 安全性通过正确传递指针和缓冲区大小保证。

use super::windows_key_sim::send_key;
use crate::sel_dbg;
use anyhow::Result;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub(crate) fn simulate_key_with_cursor_window(
    vk: VIRTUAL_KEY,
    x: i32,
    y: i32,
    _hwnd: HWND,
) -> Result<()> {
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

pub(crate) fn simulate_key_with_cursor(vk: VIRTUAL_KEY, x: i32, y: i32) -> Result<()> {
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
