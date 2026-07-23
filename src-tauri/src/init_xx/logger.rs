//! 日志格式化模块
//!
//! 该模块提供自定义的日志输出格式，供 `fern` 日志框架使用。
//!
//! 输出格式：`[时间] [级别] [线程名] [模块路径:行号] 消息`
//! 例如：`[2026-06-29 12:34:56] [INFO  ] [main] [xxmi_nrmm::init_xx::logger:8] 日志内容`
//!
//! 时间使用本地时区（无法获取时回退到 UTC），精确到秒。

use std::fmt::Arguments;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};
use time::{format_description, OffsetDateTime};
use fern::FormatCallback;

use crate::utils::DirWalker;

/// 日志文件最长保留时间。
///
/// 超过该时间的日志文件会在启动时被清理，防止长期累积占用磁盘空间。
/// 当前设置为 30 天，兼顾问题追溯与磁盘占用控制。
pub const LOG_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// 清理超过保留期限的日志文件。
///
/// 遍历 `logs` 目录下的所有 `.log` 文件，删除修改时间超过 `retention` 的文件。
/// 该函数在 `init_logging` 中同步调用，此时 tokio 异步运行时尚未启动，
/// 因此使用标准库同步 IO 是安全的。
///
/// 使用 DirWalker BFS 迭代遍历避免栈溢出，内部深度限制 DEFAULT_MAX_TRAVERSAL_DEPTH=64，
/// 不跟随符号链接，通过 VisitedPathPool 防止循环引用。
///
/// 参数：
/// - `log_dir`: 日志根目录（如 `%LOCALAPPDATA%\xxmi-nrmm\logs`）。
/// - `retention`: 保留期限，超过该时间的日志文件会被删除。
///
/// 返回：
/// 被删除的文件数量。清理失败时返回 0，不中断应用启动。
pub fn cleanup_old_logs(log_dir: &Path, retention: Duration) -> usize {
    let now = SystemTime::now();
    let mut removed = 0usize;

    if !log_dir.exists() || !log_dir.is_dir() {
        return removed;
    }

    let entries = DirWalker::new()
        .follow_symlinks(false)
        .file_ext("log")
        .include_dirs(false)
        .skip_hidden(false)
        .walk_bfs(log_dir);

    for entry in entries {
        let metadata = match fs::metadata(&entry.real_path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };

        let modified = metadata.modified().unwrap_or(now);
        if now.duration_since(modified).unwrap_or(Duration::ZERO) > retention {
            if fs::remove_file(&entry.path).is_ok() {
                removed += 1;
            }
        }
    }

    removed
}

/// 获取日志目录统计信息。
///
/// 用于启动时输出一次日志目录状态，便于后续复查日志增长趋势。
///
/// 使用 DirWalker BFS 迭代遍历避免栈溢出，内部深度限制 DEFAULT_MAX_TRAVERSAL_DEPTH=64，
/// 不跟随符号链接，通过 VisitedPathPool 防止循环引用。
///
/// 参数：
/// - `log_dir`: 日志根目录。
///
/// 返回：
/// 元组 `(文件数量, 总字节数)`。遍历失败时返回 `(0, 0)`。
pub fn get_log_dir_stats(log_dir: &Path) -> (usize, u64) {
    let mut count = 0usize;
    let mut total_bytes = 0u64;

    if !log_dir.exists() || !log_dir.is_dir() {
        return (count, total_bytes);
    }

    let entries = DirWalker::new()
        .follow_symlinks(false)
        .file_ext("log")
        .include_dirs(false)
        .skip_hidden(false)
        .walk_bfs(log_dir);

    for entry in entries {
        if let Ok(metadata) = fs::metadata(&entry.real_path) {
            count += 1;
            total_bytes += metadata.len();
        }
    }

    (count, total_bytes)
}

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
    let format = format_description::parse_borrowed::<2>("[year]-[month]-[day] [hour]:[minute]:[second]")
        .expect("Static log time format should be valid");
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_log_dir() -> (std::path::PathBuf, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (dir.path().join("logs"), dir)
    }

    #[test]
    fn test_cleanup_old_logs_removes_only_old_log_files() {
        // 使用 retention = 0 秒来模拟"所有文件都过期"；
        // 该测试主要验证递归遍历、按扩展名过滤、返回计数等逻辑正确。
        let (log_dir, _temp) = temp_log_dir();
        fs::create_dir_all(&log_dir).unwrap();

        let sub_dir = log_dir.join("2026").join("07");
        fs::create_dir_all(&sub_dir).unwrap();
        fs::write(sub_dir.join("05.log"), "old log").unwrap();
        fs::write(sub_dir.join("04.log"), "old log 2").unwrap();
        fs::write(sub_dir.join("not-a-log.txt"), "keep me").unwrap();

        let removed = cleanup_old_logs(&log_dir, Duration::ZERO);
        assert_eq!(removed, 2);

        // 非 .log 文件应保留
        assert!(sub_dir.join("not-a-log.txt").exists());
        assert!(!sub_dir.join("05.log").exists());
        assert!(!sub_dir.join("04.log").exists());
    }

    #[test]
    fn test_get_log_dir_stats_counts_log_files_and_bytes() {
        let (log_dir, _temp) = temp_log_dir();
        fs::create_dir_all(&log_dir).unwrap();

        let sub_dir = log_dir.join("2026").join("07");
        fs::create_dir_all(&sub_dir).unwrap();
        fs::write(sub_dir.join("05.log"), "hello").unwrap();
        fs::write(sub_dir.join("04.log"), "world").unwrap();
        fs::write(sub_dir.join("ignore.txt"), "ignored").unwrap();

        let (count, bytes) = get_log_dir_stats(&log_dir);
        assert_eq!(count, 2);
        assert_eq!(bytes, 10); // "hello" + "world" = 5 + 5
    }
}
