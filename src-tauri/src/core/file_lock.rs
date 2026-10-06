//! OS 级建议锁（advisory lock）：跨进程文件操作互斥与一致性保护
//!
//! 设计要点：
//! - 以「原子创建锁文件」实现进程级互斥：在目标 Mods 根目录创建隐藏锁文件
//!   `.nrmm_process.lock`（`OpenOptions::create_new`，原子且失败即已存在），文件内
//!   记录持有者 PID 与获取时间戳；释放时删除文件。
//! - `create_new` 在文件系统层面原子，因此天然提供【进程级互斥】——同一游戏 Mods
//!   目录任意时刻仅一个进程可持有，确保文件操作的一致性（更新模组数据 / 导入 /
//!   备份互不交错）。
//! - 崩溃恢复：若持有进程崩溃，锁文件残留；获取方检测到锁文件内 PID 已不存在
//!   （Windows 经 `GetExitCodeProcess` 探测；非 Windows 经时间戳阈值）即视为过期并接管，
//!   避免永久死锁。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

/// 锁文件被视为「过期（stale）」的最大存活时长：超过此时间的锁一律视为孤儿并强制接管。
/// 用于进程崩溃后残留锁文件的自动恢复，避免永久死锁。
const LOCK_STALE_TIMEOUT: Duration = Duration::from_secs(6 * 3600);

/// Windows `GetExitCodeProcess` 在进程仍运行时返回的退出码（STILL_ACTIVE = 259）。
/// 原始 `windows::Win32::Foundation::STILL_ACTIVE` 为 `NTSTATUS` 类型，此处按 `u32` 字面量定义
/// 以便与 `GetExitCodeProcess` 写入的 `u32` 退出码直接比较。
#[cfg(windows)]
const PROCESS_STILL_ACTIVE: u32 = 259;

/// 进程级建议锁。
///
/// 持有期间对应游戏 Mods 目录被本进程独占；`Drop` 时自动释放（关闭句柄 + 删除锁文件）。
pub struct AdvisoryLock {
    lock_path: PathBuf,
    _handle: Option<File>,
}

impl AdvisoryLock {
    /// 锁文件路径：置于 Mods 根目录下的隐藏 dotfile，覆盖该游戏全部文件操作。
    pub fn lock_path_for(mods_root: &Path) -> PathBuf {
        mods_root.join(".nrmm_process.lock")
    }

    /// 非阻塞获取锁：成功返回 `Some`，被占用（且未过期）返回 `None`，错误返回 `Err`。
    pub fn try_acquire(mods_root: &Path) -> Result<Option<AdvisoryLock>> {
        let lock_path = Self::lock_path_for(mods_root);
        if let Some(parent) = lock_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(f) => {
                let pid = process::id();
                let ts = now_secs();
                // 一次 write_all 完整内容；失败即清理锁文件并返回 Err
                //（不产生空锁孤儿，避免其他进程误判为孤儿而接管，破坏互斥）
                let f = Self::write_lock_content(f, &lock_path, pid, ts)?;
                Ok(Some(AdvisoryLock {
                    lock_path,
                    _handle: Some(f),
                }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if Self::is_stale(&lock_path) {
                    // 孤儿锁：接管（删除后重试一次）
                    let _ = fs::remove_file(&lock_path);
                    return Self::try_acquire(mods_root);
                }
                Ok(None)
            }
            Err(e) => Err(e).with_context(|| format!("Failed to create lock file {:?}", lock_path)),
        }
    }

    /// 写入锁文件内容（PID 与时间戳）；失败时先关闭句柄、清理锁文件并返回 `Err`。
    ///
    /// 原实现用多次 `writeln!` 并忽略失败——磁盘满时会残留内容残缺的「空锁孤儿」，
    /// 持锁者自认持锁，其他进程却读不到 PID/时间戳而判其过期接管，导致互斥失效。
    /// 此处以一次 `write_all` 写入完整内容，任一步失败即回滚删除锁文件，保证不遗留空锁。
    /// 成功时归还所属句柄（供 `AdvisoryLock` 持有至释放）。
    fn write_lock_content<W: Write>(mut f: W, path: &Path, pid: u32, ts: u64) -> Result<W> {
        let content = format!("pid={}\nts={}\n", pid, ts);
        let result = f.write_all(content.as_bytes()).and_then(|_| f.flush());
        if let Err(e) = result {
            drop(f); // Windows 上需先关闭占用句柄才能删除锁文件
            let _ = fs::remove_file(path);
            return Err(anyhow::anyhow!("写入锁文件失败并已清理: {}", e));
        }
        Ok(f)
    }

    /// 带重试的阻塞获取：在超时内周期性重试；超时仍未获取返回 `None`。
    ///
    /// 用于 `update_mod_data` / 导入等期望「稍候即可获得锁」的场景，避免瞬时被占用即失败。
    pub fn acquire_blocking(mods_root: &Path, timeout: Duration) -> Result<Option<AdvisoryLock>> {
        let start = std::time::Instant::now();
        loop {
            match Self::try_acquire(mods_root)? {
                Some(l) => return Ok(Some(l)),
                None => {
                    if start.elapsed() >= timeout {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }

    /// 判断锁文件是否过期（孤儿锁）：时间戳超过阈值，或 PID 已不存在。
    fn is_stale(lock_path: &Path) -> bool {
        let content = match fs::read_to_string(lock_path) {
            Ok(c) => c,
            Err(_) => return true,
        };
        let pid = content
            .lines()
            .find_map(|l| l.strip_prefix("pid="))
            .and_then(|v| v.trim().parse::<u32>().ok());
        let ts = content
            .lines()
            .find_map(|l| l.strip_prefix("ts="))
            .and_then(|v| v.trim().parse::<u64>().ok());
        // 时间戳阈值检查优先：超过 6h 一律视为过期（覆盖非 Windows 无法精确探测 PID 的情况）
        if let Some(ts) = ts {
            let elapsed = now_secs().saturating_sub(ts);
            if elapsed > LOCK_STALE_TIMEOUT.as_secs() {
                return true;
            }
        }
        match pid {
            Some(pid) => !process_alive(pid),
            None => true,
        }
    }
}

impl Drop for AdvisoryLock {
    fn drop(&mut self) {
        // 先关闭句柄，再删除锁文件（Windows 下打开中的文件无法直接删除）
        self._handle = None;
        let _ = fs::remove_file(&self.lock_path);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 探测指定 PID 的进程是否存活（跨平台尽力而为）。
#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_INFORMATION,
    };
    // SAFETY: 仅以查询权限打开，不修改目标进程；句柄用完即关闭。
    let handle = match unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, false, pid) } {
        Ok(h) => h,
        Err(_) => return false, // 打开失败（进程不存在/无权限）→ 视为不存活
    };
    let mut exit_code: u32 = 0;
    let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code).is_ok() };
    let _ = unsafe { CloseHandle(handle) };
    ok && exit_code == PROCESS_STILL_ACTIVE
}

#[cfg(not(windows))]
fn process_alive(_pid: u32) -> bool {
    // 非 Windows 平台无 libc 依赖，无法精确探测 PID；交由 `is_stale` 的时间戳阈值判定。
    // 返回 true 表示「默认视为存活」，从而仅当超过时间戳阈值才接管，避免误抢活跃锁。
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};

    /// 恒失败写入器：无需真实磁盘故障即可稳定触发写入错误路径（对应磁盘满等写失败场景）。
    struct ErrorWriter;

    impl Write for ErrorWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("注入写入失败"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("注入写入失败"))
        }
    }

    /// 写入失败时必须返回 `Err`，且已创建的锁文件被清理（不残留「空锁孤儿」）。
    #[test]
    fn write_failure_cleans_up_lock_file_and_returns_err() {
        let tmp = tempfile::TempDir::new().expect("创建临时测试目录失败");
        let lock_path = tmp.path().join(".nrmm_process.lock");
        // 预置真实锁文件，验证失败路径确会删除磁盘上的锁文件。
        std::fs::write(&lock_path, "bogus").expect("预置锁文件失败");

        let res = AdvisoryLock::write_lock_content(ErrorWriter, &lock_path, 1234, 99);
        assert!(res.is_err(), "写入失败应返回 Err 而非静默忽略");
        assert!(
            !lock_path.exists(),
            "写入失败后锁文件应被清理，不得残留空锁孤儿"
        );
    }
}
