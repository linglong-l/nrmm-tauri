//! 还原安全窗口 / 标记文件域：groupname/modname/selectedindex 只读不建、marker 读写
//!
//! 从 mod_scanner.rs 拆出（2026-09-19 Phase 5-2）。
//! 被 light/mutex 两域与 mod_restore（mark_restore_safe_window）消费；
//! 还原流程改写 INI 后 watcher 防抖触发重扫，窗口期内的扫描对管理标记只读不建。

use anyhow::Result;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use crate::core::file_watcher::WatcherGuard;
#[cfg(test)]
use once_cell::sync::Lazy;

/// 读取组目录 `groupname` 显示名（只读不建）。
///
/// 仅在还原安全窗口内使用（见 [`restore_safe_active`]）：还原后的目录通常会被
/// 用户删除/导出，扫描不得回写 groupname 等专属配置文件。存在则读取显示名，
/// 缺失时回退目录名（`group_X`）。
///
/// # Arguments
/// - `dir_path`: 组目录路径。
/// - `fallback`: 文件缺失时的回退显示名（通常为目录名 / 去 DISABLED 前缀目录名）。
///
/// # Errors
/// 文件存在但读取失败（如非 UTF-8）时返回 `Err`（与原 [`read_or_create_marker_file`]
/// 存在分支语义一致）。
pub(crate) fn read_groupname_marker(dir_path: &Path, fallback: &str) -> Result<String> {
    let groupname_path = dir_path.join("groupname");
    if groupname_path.exists() {
        Ok(fs::read_to_string(&groupname_path)?.trim().to_string())
    } else {
        Ok(fallback.to_string())
    }
}

// ---- 还原安全窗口 ----
// 还原（restore_all_inis / restore_managed_mod）改写 INI 后，watcher 防抖触发重扫；
// 窗口期内的扫描对 groupname/modname/selectedindex 等管理标记**只读不建**——
// 还原后的目录通常会被用户删除/导出，项目专属配置文件会干扰其他管理器。
// 窗口过期后恢复正常扫描语义（缺失标记照常新建）。
static RESTORE_SAFE_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
/// 窗口时长：覆盖 watcher 防抖（1s）+ 大目录重扫耗时
const RESTORE_SAFE_WINDOW_MS: u64 = 60_000;

/// 读取标记文件（只读不建）：存在则读取（trim），缺失返回默认内容，不写盘。
///
/// 用于还原安全窗口内的扫描（见 [`restore_safe_active`]）。
///
/// # Errors
/// 文件存在但读取失败（如非 UTF-8）时返回 `Err`。
pub(crate) fn read_marker_readonly(path: &Path, default_content: &str) -> Result<String> {
    if path.exists() {
        Ok(fs::read_to_string(path)?.trim().to_string())
    } else {
        Ok(default_content.to_string())
    }
}

/// 标记进入还原安全窗口（还原流程入口调用）
pub fn mark_restore_safe_window() {
    set_restore_safe_window(RESTORE_SAFE_WINDOW_MS);
}

/// 设置还原安全窗口剩余时长（毫秒；0 = 立即关闭），测试可注入
pub(crate) fn set_restore_safe_window(remaining_ms: u64) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    RESTORE_SAFE_UNTIL_MS.store(now_ms.saturating_add(remaining_ms), AtomicOrdering::SeqCst);
}

/// 还原安全窗口是否生效；生效时扫描对管理标记文件只读不建
pub(crate) fn restore_safe_active() -> bool {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    RESTORE_SAFE_UNTIL_MS.load(AtomicOrdering::SeqCst) > now_ms
}

/// 测试专用：串行化「还原安全窗口」全局状态的使用。
/// 窗口测试（设置全局窗口）与依赖标记创建的测试（隐式要求窗口未生效）
/// 必须互斥执行，避免同进程并发测试互扰。
#[cfg(test)]
pub(crate) static TEST_WINDOW_LOCK: Lazy<std::sync::Mutex<()>> =
    Lazy::new(|| std::sync::Mutex::new(()));

/// 测试辅助：立即清除还原安全窗口。
///
/// 供跨模块测试（如 mod_commands 的还原集成测试）在持有 [`TEST_WINDOW_LOCK`]
/// 的前提下结束时恢复全局状态——`restore_all_inis` 内部会开启 60s 窗口，
/// 不清除会泄漏到后续并发执行的标记创建断言测试。
#[cfg(test)]
pub(crate) fn test_clear_restore_safe_window() {
    RESTORE_SAFE_UNTIL_MS.store(0, AtomicOrdering::SeqCst);
}

/// 读取或创建标记文件，不存在则用默认内容创建
///
/// 写文件前通过 [`WatcherGuard`] 递增全局暂停计数，避免触发循环事件。
/// 守卫离开作用域自动递减计数（panic 安全），无需手动恢复。
///
/// # Errors
/// 当 `path` 存在但无法读取，或 `path` 不存在且无法写入时返回 `Err`
pub fn read_or_create_marker_file(path: &Path, default_content: &str) -> Result<String> {
    if path.exists() {
        let content = fs::read_to_string(path)?;
        Ok(content.trim().to_string())
    } else {
        let result = {
            let _guard = WatcherGuard::new();
            crate::utils::atomic_write(path, default_content.as_bytes())
        }; // _guard drop：计数递减
        result?;
        Ok(default_content.to_string())
    }
}

#[cfg(test)]
#[path = "mod_scan_marker_tests.rs"]
mod tests;
