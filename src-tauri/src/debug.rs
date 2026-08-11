//! 选择逻辑 DEBUG 日志工具
//!
//! 提供统一的 `sel_dbg!` 宏，用于在选择逻辑（`select_mod` 调用链）中输出
//! 中文调试日志。每条日志自动包含：
//! - 标签前缀 `[选择逻辑]`
//! - 模块名（`$mod` 字面量，如 `"mod_commands"`）
//! - 函数名（`$fn` 字面量，如 `"select_mod"`）
//! - 本地时间戳（精确到毫秒，格式 `HH:MM:SS.mmm`）
//!
//! 设计目标：在不干扰正常逻辑的前提下，便于追踪选择逻辑的完整调用链条、
//! 关键函数触发时机与参数、以及鼠标坐标变化。所有调试输出均为简体中文。

use chrono::Local;

/// 返回当前本地时间字符串，格式 `HH:MM:SS.mmm`。
pub fn sel_ts() -> String {
    Local::now().format("%H:%M:%S%.3f").to_string()
}

/// 选择逻辑 DEBUG 日志宏。
///
/// 用法：
/// ```ignore
/// sel_dbg!("mod_commands", "select_mod", "入口 | 模组名称={}", name);
/// ```
///
/// 输出示例：
/// `[选择逻辑][mod_commands][select_mod][14:05:23.123] 入口 | 模组名称=MyMod`
#[macro_export]
macro_rules! sel_dbg {
    ($mod:expr, $fn:expr, $($arg:tt)*) => {
        ::log::debug!(
            "[选择逻辑][{}][{}][{}] {}",
            $mod,
            $fn,
            $crate::debug::sel_ts(),
            format_args!($($arg)*)
        )
    };
}
