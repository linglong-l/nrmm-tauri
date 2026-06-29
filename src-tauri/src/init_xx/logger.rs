//! 日志格式化模块
//!
//! 该模块提供自定义的日志输出格式，供 `fern` 日志框架使用。
//!
//! 输出格式：`[时间] [级别] [线程名] [模块路径:行号] 消息`
//! 例如：`[2026-06-29 12:34:56] [INFO  ] [main] [xxmi_nrmm::init_xx::logger:8] 日志内容`
//!
//! 时间使用本地时区（无法获取时回退到 UTC），精确到秒。

use std::fmt::Arguments;
// logger.rs
use time::{format_description, OffsetDateTime};
// 使用 fern::FormatCallback 替代不存在的 tauri_plugin_log::format::FormatCallback
use fern::FormatCallback;

/// 自定义的日志格式化函数。
///
/// 作为 `fern` 日志框架的格式化回调使用，将每条日志格式化为统一的多字段格式。
///
/// 格式化流程：
/// 1. **时间格式化**：使用 `time` 库将当前本地时间格式化为 `YYYY-MM-DD HH:MM:SS`。
///    若无法获取本地时间（少数环境），则回退到 UTC 时间。
/// 2. **线程名获取**：通过 `std::thread::current().name()` 获取当前线程名，
///    未命名线程显示为 `"unnamed"`。
/// 3. **模块路径与行号**：从 `log::Record` 中提取模块路径（`module_path`）和源码行号，
///    便于定位日志来源。无法获取时分别显示 `"unknown"` 和 `0`。
/// 4. **日志级别**：从 `log::Record` 中获取级别（`Level`），格式化为左对齐 5 字符宽。
/// 5. **最终输出**：通过 `out.finish()` 将格式化后的字符串交给 `fern` 输出到目标（文件/控制台）。
///
/// 参数：
/// - `out`: `fern` 的格式化回调句柄，调用 `finish` 后日志才会被实际输出。
/// - `message`: 日志消息内容（`Arguments` 形式，延迟格式化以提升性能）。
/// - `record`: `log` 框架的日志记录，包含级别、模块路径、行号等元信息。
pub fn custom_log_format(out: FormatCallback, message: &Arguments, record: &log::Record) {
    // 1. 格式化时间：YYYY-MM-DD HH:MM:SS
    let format = format_description::parse_borrowed::<2>("[year]-[month]-[day] [hour]:[minute]:[second]").unwrap();
    let time_str = OffsetDateTime::now_local()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(&format)
        .unwrap_or_default();

    // 2. 获取当前线程名（用于区分主线程与阻塞工作线程）
    let current_thread = std::thread::current();
    let thread_name = current_thread.name().unwrap_or("unnamed");

    // 3. 提取模块路径和行号（定位日志来源）
    let module = record.module_path().unwrap_or("unknown");
    let line = record.line().unwrap_or(0);

    // 4. 获取日志级别（INFO/WARN/ERROR/DEBUG/TRACE）
    let level = record.level();

    // 5. 使用 .finish() 输出最终格式
    //    格式：[时间] [级别(左对齐5)] [线程名] [模块:行号] 消息
    out.finish(format_args!(
        "[{}] [{:<5}] [{}] [{}:{}] {}",
        time_str,
        level,
        thread_name,
        module,
        line,
        message
    ));
}
