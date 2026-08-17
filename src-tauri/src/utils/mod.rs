//! 通用工具模块
//!
//! 提供 Mutex 安全锁获取和线程 panic 安全包装工具函数。

use std::fs;
use std::panic::UnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

/// 构造同目录临时文件路径 `<name>.tmp_write`
///
/// 临时文件与目标同目录，保证处于同一文件系统，rename 才是原子操作。
fn tmp_write_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp_write");
    path.with_file_name(name)
}

/// 原子写入文件：同目录临时文件 + rename 覆盖
///
/// 先将内容写入 `path` 同目录的 `<name>.tmp_write` 临时文件，成功后
/// `fs::rename` 原子覆盖目标（Windows 下 std rename 内置
/// `MOVEFILE_REPLACE_EXISTING`，可覆盖已存在目标）。任一步失败时清理
/// 临时文件并返回错误，目标文件不受影响。
///
/// # 崩溃安全语义
/// 进程在任一时刻被终止，目标文件要么保持旧内容，要么为新完整内容；
/// 至多残留一个 `<name>.tmp_write` 临时文件（不影响目标正确性）。
///
/// # 参数
/// - `path`: 目标文件路径
/// - `content`: 写入的字节内容
///
/// # 返回
/// 成功返回 `Ok(())`；临时文件写入或 rename 失败返回 `Err`（临时文件已清理）
pub fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let tmp = tmp_write_path(path);
    if let Err(e) = fs::write(&tmp, content) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// 原子复制文件：同目录临时文件 + rename 覆盖
///
/// 先将 `src` 复制到 `dst` 同目录的 `<name>.tmp_write` 临时文件，成功后
/// `fs::rename` 原子覆盖 `dst`。任一步失败时清理临时文件并返回错误，
/// 目标文件不受影响。
///
/// # 崩溃安全语义
/// 进程在任一时刻被终止，目标文件要么保持旧内容，要么为新完整内容；
/// 至多残留一个 `<name>.tmp_write` 临时文件（不影响目标正确性）。
///
/// # 参数
/// - `src`: 源文件路径（只读，不受影响）
/// - `dst`: 目标文件路径
///
/// # 返回
/// 成功返回 `Ok(())`；复制或 rename 失败返回 `Err`（临时文件已清理）
pub fn atomic_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    let tmp = tmp_write_path(dst);
    if let Err(e) = fs::copy(src, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, dst) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_write_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        atomic_write(&path, b"hello").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hello");
        assert!(!tmp_write_path(&path).exists());
    }

    #[test]
    fn test_atomic_write_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");
        fs::write(&path, "old content").unwrap();
        atomic_write(&path, b"new content").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new content");
        assert!(!tmp_write_path(&path).exists());
    }

    #[test]
    fn test_atomic_copy_basic() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.ini");
        let dst = dir.path().join("dst.ini");
        fs::write(&src, "ini content").unwrap();
        atomic_copy(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "ini content");
        assert!(!tmp_write_path(&dst).exists());
    }

    #[test]
    fn test_atomic_copy_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.ini");
        let dst = dir.path().join("dst.ini");
        fs::write(&src, "new full content").unwrap();
        fs::write(&dst, "stale half content").unwrap();
        atomic_copy(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "new full content");
        assert!(!tmp_write_path(&dst).exists());
    }

    #[test]
    fn test_atomic_copy_missing_src() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("missing.ini");
        let dst = dir.path().join("dst.ini");
        assert!(atomic_copy(&src, &dst).is_err());
        assert!(!dst.exists());
        assert!(!tmp_write_path(&dst).exists());
    }

    #[test]
    fn test_atomic_write_failure_cleans_tmp() {
        // 目标父目录不存在 → 临时文件写入失败
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent_dir").join("file.json");
        assert!(atomic_write(&path, b"data").is_err());
        assert!(!path.exists());
        assert!(!tmp_write_path(&path).exists());
    }
}
