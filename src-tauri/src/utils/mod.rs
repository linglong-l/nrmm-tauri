//! 通用工具模块
//!
//! 提供 Mutex 安全锁获取和线程 panic 安全包装工具函数。

use std::sync::Mutex;
use std::panic::UnwindSafe;

/// 安全获取 Mutex 锁，中毒时恢复数据并记录日志
///
/// 当 Mutex 因持有锁的线程 panic 而中毒时，正常 `.lock().unwrap()` 会导致 panic 传播。
/// 此函数使用 `unwrap_or_else` 在中毒时恢复内部数据，并记录警告日志，
/// 避免因单个线程 panic 导致整个应用不可用。
///
/// # 参数
/// - `mutex`: 待获取锁的 Mutex 引用
///
/// # 返回
/// MutexGuard，即使 Mutex 中毒也能成功获取
pub fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        log::warn!("Mutex was poisoned, recovering with inner data");
        poisoned.into_inner()
    })
}

/// 安全线程包装：catch_unwind 捕获 panic 并记录日志
///
/// 在窗口事件回调和托盘菜单事件中使用的 `std::thread::spawn` 不支持异步运行时，
/// 但线程内的 panic 会导致静默失败。此函数使用 `catch_unwind` 包装线程闭包，
/// 捕获 panic 并记录错误日志，避免静默失败。
///
/// # 参数
/// - `name`: 线程名称（用于日志标识）
/// - `f`: 线程闭包函数
pub fn spawn_safe<F>(name: &str, f: F)
where
    F: FnOnce() + UnwindSafe + Send + 'static,
{
    let name = name.to_string();
    std::thread::spawn(move || {
        if let Err(e) = std::panic::catch_unwind(f) {
            log::error!("Thread '{}' panicked: {:?}", name, e);
        }
    });
}
