//! 游戏交互协议模块
//!
//! 该模块封装了 XXMI-NRMM 与 3DMigoto 游戏模组框架之间的交互协议。
//! 所有与游戏内按键序列相关的业务逻辑集中在此，不依赖底层 `keypress_simulator`
//! 模块以外的工具层。
//!
//! 职责范围：
//! - NRMM「选择模组」按键序列（VK_CLEAR + 光标坐标 + VK_SPACE / VK_RETURN）
//! - 3DMigoto 重载模组按键序列（F10）
//!
//! 该模块属于业务层，位于 `mod_manager` 子模块内。
//! 底层通用的键鼠模拟能力由 `keypress_simulator` 模块提供。

use std::time::Duration;

use anyhow::Result;

use crate::keypress_simulator::{
    KeypressSimulator,
    VK_CLEAR,
    VK_F10,
    VK_RETURN,
    VK_SPACE,
};

/// 模拟「选择模组」的完整按键序列，向游戏发送模组切换信号。
///
/// 序列对应 NRMM `simulateKeySelectMod(realGroupIndex, realModIndex)`:
/// 1. 保存当前光标位置
/// 2. SetCursorPos(modIndex, groupIndex) — 光标 x=modIndex, y=groupIndex
/// 3. ClipCursor 锁定在 1 像素区域
/// 4. VK_CLEAR 按下 → VK_SPACE 按下/释放（触发 [KeyGroup]，设 $active_group_id）
/// 5. VK_RETURN 按下/释放（触发 [KeyMod]，设 $active_slot）
/// 6. VK_CLEAR 释放
/// 7. 恢复光标位置和剪裁
///
/// 光标坐标携带信息：
/// - x = modIndex → 3DMigoto 的 `$cursor_screen_x` 读取
/// - y = groupIndex → 3DMigoto 的 `$cursor_screen_y` 读取
///
/// 游戏侧通过 `manager_group.ini` 的 `[KeyGroup]` 和 `group_<X>.ini` 的 `[KeyMod]`
/// 监听 VK_CLEAR+VK_SPACE/VK_RETURN 组合键，读取光标坐标后更新
/// `$active_group_id` / `$active_slot` 变量完成模组切换。
///
/// 参数：
/// - `group_index`: 分组索引（传入光标 y 坐标）。
/// - `mod_index`: 模组索引（传入光标 x 坐标）。
///
/// 错误：
/// - 非 Windows 平台调用时返回错误：按键模拟仅支持 Windows 平台。
///
/// 示例：
/// ```ignore
/// select_mod_key_sequence(1, 3).await?; // 选择第 2 组第 4 个模组
/// ```
pub async fn select_mod_key_sequence(group_index: i32, mod_index: i32) -> Result<()> {
    #[cfg(windows)]
    {
        let simulator = KeypressSimulator::new();
        use windows::Win32::UI::WindowsAndMessaging::{ClipCursor, SetCursorPos};
        use windows::Win32::Foundation::RECT;

        // 保存当前光标位置，用于完成后恢复
        let saved_pos = KeypressSimulator::get_cursor_pos_windows().unwrap_or((0, 0));

        // 将光标移到目标坐标（携带 (modIndex, groupIndex) 信息）
        unsafe { let _ = SetCursorPos(mod_index, group_index); }

        // 锁定光标在 1 像素区域内，防止抖动
        let lock_rect = RECT {
            left: mod_index,
            top: group_index,
            right: mod_index + 1,
            bottom: group_index + 1,
        };
        unsafe { let _ = ClipCursor(Some(&lock_rect)); }

        // 按下 VK_CLEAR 修饰键
        simulator.simulate_key_down(VK_CLEAR).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // VK_SPACE → 触发游戏的 [KeyGroup]，设置 $active_group_id = cursor_screen_y
        simulator.simulate_key_down(VK_SPACE).await?;
        tokio::time::sleep(Duration::from_millis(30)).await;
        simulator.simulate_key_up(VK_SPACE).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // VK_RETURN → 触发游戏的 [KeyMod]，设置 $active_slot = cursor_screen_x
        simulator.simulate_key_down(VK_RETURN).await?;
        tokio::time::sleep(Duration::from_millis(30)).await;
        simulator.simulate_key_up(VK_RETURN).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 释放 VK_CLEAR
        simulator.simulate_key_up(VK_CLEAR).await?;

        // 恢复光标位置和剪裁
        unsafe {
            let _ = ClipCursor(None);
            let _ = SetCursorPos(saved_pos.0, saved_pos.1);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = group_index;
        let _ = mod_index;
        anyhow::bail!("按键模拟仅支持 Windows 平台");
    }
    Ok(())
}

/// 模拟「重载模组」的按键序列。
///
/// 向游戏发送 F10 重载快捷键，对应 3DMigoto 默认的模组重载键。
/// 用于在手动修改 INI 文件后请求 3DMigoto 重新加载配置。
///
/// 实现方式：调用 `KeypressSimulator::simulate_key_press_vk(VK_F10)`。
///
/// 错误：
/// - 非 Windows 平台调用时返回错误。
#[allow(dead_code)]
pub async fn reload_mod_key_sequence() -> Result<()> {
    let simulator = KeypressSimulator::new();
    simulator.simulate_key_press_vk(VK_F10).await
}
