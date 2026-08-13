//! 分组/模组上限动态计算模块
//!
//! 设计依据（对齐 NRMM `group_int` 绑定 xy 轴的语义）：
//! - 3Dmigoto 按键模板中 `$active_group_id = cursor_screen_y`（分组绑定 **Y 轴**）
//! - `$active_slot = cursor_screen_x`（模组/槽位绑定 **X 轴**）
//! - 3Dmigoto 的 `cursor_screen_x` / `cursor_screen_y` 均为正数
//!
//! 因此：
//! - **分组上限** = 屏幕最小高度（min y，多显示器取各显示器最小 y）
//! - **模组上限** = 屏幕最小宽度（min x，多显示器取各显示器最小 x）
//!
//! 整数类型选择（优先最小类型，节省存储/对齐 NRMM 变量宽度）：
//! - 上限 ≤ 255 → `u8`
//! - 上限 ≤ u32::MAX → `u32`
//! - 否则 → `u64`
//!
//! 假设基准分辨率（无法获取真实分辨率时的回退）：1080 × 720。

use serde::Serialize;

/// 槽位/分组 ID 整数宽度（优先最小表达类型）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum IntWidth {
    U8,
    U32,
    U64,
}

impl IntWidth {
    /// 根据所需表示的最大值选择最小整数类型
    pub fn for_max(max: u64) -> IntWidth {
        if max <= u8::MAX as u64 {
            IntWidth::U8
        } else if max <= u32::MAX as u64 {
            IntWidth::U32
        } else {
            IntWidth::U64
        }
    }

    /// 返回类型名（用于日志/前端展示）
    pub fn as_str(&self) -> &'static str {
        match self {
            IntWidth::U8 => "u8",
            IntWidth::U32 => "u32",
            IntWidth::U64 => "u64",
        }
    }
}

/// 计算得到的分辨率上限
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ResolutionLimits {
    /// 分组上限（绑定 Y 轴 = 屏幕最小高度）
    pub max_groups: u32,
    /// 模组上限（绑定 X 轴 = 屏幕最小宽度）
    pub max_mods: u32,
    /// 选用的最小整数宽度（容纳 max_groups / max_mods 两者）
    pub width: IntWidth,
}

/// 获取屏幕分辨率 `(width, height)`。
///
/// - Windows：枚举所有显示器，取各显示器宽/高的最小值（多显示器取 min x / min y）。
///   若枚举失败则回退到主显示器 `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)`。
/// - 其他平台：暂返回基准分辨率 `(1080, 720)`。
#[cfg(windows)]
pub fn screen_resolution() -> (u32, u32) {
    use windows::Win32::Foundation::{LPARAM, RECT, TRUE};
    use windows::core::BOOL;
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HMONITOR, HDC};
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    extern "system" fn monitor_enum_proc(
        _hmon: HMONITOR,
        _hdc: HDC,
        lprc: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        unsafe {
            if data.0 != 0 && !lprc.is_null() {
                let state = &mut *(data.0 as *mut (i32, i32));
                let r = &*lprc;
                let w = r.right - r.left;
                let h = r.bottom - r.top;
                if w > 0 && w < state.0 {
                    state.0 = w;
                }
                if h > 0 && h < state.1 {
                    state.1 = h;
                }
            }
            TRUE
        }
    }

    unsafe {
        let mut state = (i32::MAX, i32::MAX);
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(monitor_enum_proc),
            LPARAM(&mut state as *mut _ as isize),
        );
        if state.0 != i32::MAX && state.1 != i32::MAX {
            return (state.0 as u32, state.1 as u32);
        }
        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        if w > 0 && h > 0 {
            return (w as u32, h as u32);
        }
    }
    (1080, 720)
}

/// 非 Windows 平台回退基准分辨率
#[cfg(not(windows))]
pub fn screen_resolution() -> (u32, u32) {
    (1080, 720)
}

/// 依据当前屏幕分辨率计算分组/模组上限及选用整数宽度。
pub fn compute_limits() -> ResolutionLimits {
    let (w, h) = screen_resolution();
    compute_limits_for(w, h)
}

/// 纯函数：依据给定分辨率 `(width, height)` 计算上限，便于测试（不依赖真实显示器）。
pub fn compute_limits_for(w: u32, h: u32) -> ResolutionLimits {
    let max_mods = w.max(1);
    let max_groups = h.max(1);
    let width = IntWidth::for_max(max_mods.max(max_groups) as u64);
    ResolutionLimits {
        max_groups,
        max_mods,
        width,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_width_selection() {
        assert_eq!(IntWidth::for_max(1), IntWidth::U8);
        assert_eq!(IntWidth::for_max(255), IntWidth::U8);
        assert_eq!(IntWidth::for_max(256), IntWidth::U32);
        assert_eq!(IntWidth::for_max(1080), IntWidth::U32);
        assert_eq!(IntWidth::for_max(720), IntWidth::U32);
    }

    #[test]
    fn baseline_resolution_limits() {
        // 基准 1080x720：max_mods=1080(>255)->U32, max_groups=720(>255)->U32
        let limits = compute_limits_for(1080, 720);
        assert_eq!(limits.max_mods, 1080);
        assert_eq!(limits.max_groups, 720);
        assert_eq!(limits.width, IntWidth::U32);
    }
}
