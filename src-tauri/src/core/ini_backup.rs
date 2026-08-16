//! INI 启动备份模块（防损坏冗余备份）
//!
//! 应用启动后首次模组扫描完成时，后台单线程对 Mods 目录下的全部 INI 文件
//! 执行一次全量冗余备份（后缀 `.ini_manager_backup`，与 NRMM 注入管线的
//! `.ini_managed_backup` 区分），防止其他工具/本项目意外/用户误改导致 INI
//! 为空或损坏；仅添加模组（导入）时对新导入目录补做备份。
//!
//! # 备份判定（v2：无视顶层分组）
//! - 遍历不做任何分组语义特判（不区分 `_MANAGED_`/`group_xx`/互斥组/
//!   `DISABLED_MANAGED_REMOVED` 等，也不跳过隐藏目录），唯一排除条件是白名单
//! - 白名单（NRMM 每次重新生成的管理文件与系统文件，见
//!   `constants::is_ini_backup_whitelisted`）不参与备份
//! - 已存在的备份文件跳过（幂等），备份仅新增文件，不修改任何现有文件
//!
//! # 遍历算法
//! - 显式栈 DFS（`Vec<PathBuf>` 模拟递归，对齐 `scan_mutex_group_dfs` 先例）
//! - `HashSet<PathBuf>` visited（canonicalize 去重，防 symlink 循环/重复遍历）
//! - `HashMap<PathBuf, PathBuf>` 符号链接池（原始绝对路径 → 解析后绝对路径）
//! - 符号链接 INI：在真实路径旁创建实体备份，并在链接所在目录创建指向该
//!   备份的符号链接（链接名 = 符号链接 stem + 备份后缀）
//!
//! # 错误行为
//! 全部函数不向上传播错误：所有 IO 错误仅 `log::warn!` 后继续处理下一项。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::constants;

/// 启动备份触发标志：每次进程生命周期仅执行一次启动全量备份
static BACKUP_TRIGGERED: AtomicBool = AtomicBool::new(false);

/// 单文件备份结果（仅模块内部计数用，不对外暴露）
enum BackupOutcome {
    /// 新创建了备份文件（或符号链接备份对）
    Backed,
    /// 备份已存在，跳过（幂等）
    Skipped,
    /// 备份失败（IO 错误，已记录日志）
    Failed,
}

/// 确保启动 INI 备份仅执行一次（后台单线程触发入口）
///
/// 首次调用时通过 `swap` 置位 `BACKUP_TRIGGERED` 并在后台线程执行一次
/// `backup_ini_files`（fire-and-forget）；后续调用直接返回，不重复执行。
/// 供 `get_mods` 的缓存命中/扫描完成两个返回点挂载，两处都挂安全。
///
/// # 参数
/// - `mods_path`: Mods 根目录路径（所有权移入后台线程）
///
/// # 返回
/// 无返回值；后台任务的开始/结束与耗时仅记录日志，不上传任何错误
pub fn ensure_startup_ini_backup(mods_path: PathBuf) {
    if BACKUP_TRIGGERED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::utils::spawn_safe(
        "ini_startup_backup",
        std::panic::AssertUnwindSafe(move || {
            let start = std::time::Instant::now();
            log::info!(
                "[ini_backup] Startup INI backup started: {}",
                mods_path.display()
            );
            backup_ini_files(&mods_path);
            log::info!(
                "[ini_backup] Startup INI backup finished in {}ms",
                start.elapsed().as_millis()
            );
        }),
    );
}

/// 遍历目录树，备份全部非白名单 INI 文件（无视顶层分组）
///
/// 显式栈 DFS + canonicalize 去重（防 symlink 循环）+ 符号链接池。
/// 遍历规则：不做任何目录名/分组特判，只要 ini 文件不在白名单内即备份。
/// 启动全量备份与新导入目录补备份共用此入口。
///
/// # 参数
/// - `mods_path`: 遍历根目录（Mods 根或新导入模组目录）
///
/// # 返回
/// 无返回值；`read_dir`/entry 错误仅 `log::warn!` 后继续，结束记录汇总日志
/// （backed/skipped/failed 计数），不向上传播错误
pub fn backup_ini_files(mods_path: &Path) {
    let mut stack: Vec<PathBuf> = vec![mods_path.to_path_buf()];
    // 规范化路径 visited 集合：防止 symlink/hardlink 导致循环或重复遍历
    let mut visited: HashSet<PathBuf> = HashSet::new();
    // 符号链接池：原始绝对路径 → canonicalize 解析后的绝对路径
    let mut symlink_pool: HashMap<PathBuf, PathBuf> = HashMap::new();
    let (mut backed, mut skipped, mut failed) = (0usize, 0usize, 0usize);

    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("[ini_backup] read_dir failed for {}: {}", dir.display(), e);
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    log::warn!(
                        "[ini_backup] dir entry error under {}: {}",
                        dir.display(),
                        e
                    );
                    continue;
                }
            };
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(f) => f,
                Err(e) => {
                    log::warn!(
                        "[ini_backup] file_type failed for {}: {}",
                        path.display(),
                        e
                    );
                    continue;
                }
            };
            if ft.is_symlink() {
                // 符号链接：解析后登记链接池，按 follow 目标类型分派
                let real = path.canonicalize().unwrap_or_else(|_| path.clone());
                symlink_pool.entry(path.clone()).or_insert_with(|| real.clone());
                match fs::metadata(&path) {
                    Ok(md) if md.is_dir() => {
                        if visited.insert(real.clone()) {
                            stack.push(real);
                        }
                    }
                    Ok(md) if md.is_file() => {
                        if let Some(outcome) = try_backup_ini_entry(&path, &mut symlink_pool) {
                            match outcome {
                                BackupOutcome::Backed => backed += 1,
                                BackupOutcome::Skipped => skipped += 1,
                                BackupOutcome::Failed => failed += 1,
                            }
                        }
                    }
                    Ok(_) => {} // 其他类型（管道/套接字等），忽略
                    Err(e) => {
                        log::warn!(
                            "[ini_backup] symlink target metadata failed for {}: {}",
                            path.display(),
                            e
                        );
                    }
                }
            } else if ft.is_dir() {
                // 普通目录：canonicalize 去重后入栈（不跳过任何分组/隐藏目录）
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                if visited.insert(canonical.clone()) {
                    stack.push(canonical);
                }
            } else if ft.is_file() {
                if let Some(outcome) = try_backup_ini_entry(&path, &mut symlink_pool) {
                    match outcome {
                        BackupOutcome::Backed => backed += 1,
                        BackupOutcome::Skipped => skipped += 1,
                        BackupOutcome::Failed => failed += 1,
                    }
                }
            }
        }
    }
    log::info!(
        "[ini_backup] backup summary: backed={}, skipped={}, failed={}",
        backed,
        skipped,
        failed
    );
}

/// ini 判定统一入口：扩展名为 ini（大小写不敏感）且不在白名单内才执行备份
///
/// # 参数
/// - `path`: 待判定文件路径
/// - `symlink_pool`: 符号链接池（透传给 `backup_one_ini`）
///
/// # 返回
/// 非备份目标返回 `None`；否则返回 `backup_one_ini` 的结果
fn try_backup_ini_entry(
    path: &Path,
    symlink_pool: &mut HashMap<PathBuf, PathBuf>,
) -> Option<BackupOutcome> {
    let is_ini = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("ini"))
        .unwrap_or(false);
    if !is_ini {
        return None;
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if constants::is_ini_backup_whitelisted(&file_name) {
        return None;
    }
    Some(backup_one_ini(path, symlink_pool))
}

/// 备份单个 INI 文件（普通文件复制 / 符号链接双备份）
///
/// # 参数
/// - `ini_path`: INI 文件路径（可能是普通文件或符号链接）
/// - `symlink_pool`: 符号链接池（登记 原始绝对路径 → 解析绝对路径）
///
/// # 行为
/// - 普通文件：同目录创建 `<stem>.ini_manager_backup`；已存在则跳过（幂等）
/// - 符号链接：真实路径旁创建 `<real stem>.ini_manager_backup` 实体备份；
///   链接所在目录创建 `<链接 stem>.ini_manager_backup` 符号链接指向该备份
///   （每步独立 exists 检查，两文件各自幂等；链接创建失败仅告警，如
///   Windows 无特权时实体备份仍然保留）
///
/// # 返回
/// 备份结果（Backed/Skipped/Failed）；所有 IO 错误仅 `log::warn!`，不上传
fn backup_one_ini(ini_path: &Path, symlink_pool: &mut HashMap<PathBuf, PathBuf>) -> BackupOutcome {
    let ft = match ini_path.symlink_metadata() {
        Ok(md) => md.file_type(),
        Err(e) => {
            log::warn!(
                "[ini_backup] symlink_metadata failed for {}: {}",
                ini_path.display(),
                e
            );
            return BackupOutcome::Failed;
        }
    };

    if !ft.is_symlink() {
        // 普通文件：同目录 <stem>.ini_manager_backup（拼接构造，避免 with_extension
        // 对多 dot stem（如 "my.mod.ini"）的破坏）
        let stem = ini_path.file_stem().unwrap_or_default().to_string_lossy();
        let backup = ini_path.with_file_name(format!(
            "{}.{}",
            stem,
            constants::INI_MANAGER_BACKUP_SUFFIX
        ));
        if backup.exists() {
            return BackupOutcome::Skipped;
        }
        return match crate::utils::atomic_copy(ini_path, &backup) {
            Ok(_) => BackupOutcome::Backed,
            Err(e) => {
                log::warn!(
                    "[ini_backup] copy failed {} -> {}: {}",
                    ini_path.display(),
                    backup.display(),
                    e
                );
                BackupOutcome::Failed
            }
        };
    }

    // 符号链接：解析真实路径并登记链接池
    let real = match ini_path.canonicalize() {
        Ok(r) => r,
        Err(e) => {
            log::warn!(
                "[ini_backup] canonicalize failed for {}: {}",
                ini_path.display(),
                e
            );
            return BackupOutcome::Failed;
        }
    };
    symlink_pool.insert(ini_path.to_path_buf(), real.clone());

    // 1) 真实路径旁的实体备份
    let real_stem = real.file_stem().unwrap_or_default().to_string_lossy();
    let real_backup = real.with_file_name(format!(
        "{}.{}",
        real_stem,
        constants::INI_MANAGER_BACKUP_SUFFIX
    ));
    let mut outcome = BackupOutcome::Skipped;
    if !real_backup.exists() {
        match crate::utils::atomic_copy(&real, &real_backup) {
            Ok(_) => outcome = BackupOutcome::Backed,
            Err(e) => {
                log::warn!(
                    "[ini_backup] copy real failed {} -> {}: {}",
                    real.display(),
                    real_backup.display(),
                    e
                );
                return BackupOutcome::Failed;
            }
        }
    }

    // 2) 链接所在目录的符号链接（指向实体备份）
    let link_stem = ini_path.file_stem().unwrap_or_default().to_string_lossy();
    let link = ini_path.with_file_name(format!(
        "{}.{}",
        link_stem,
        constants::INI_MANAGER_BACKUP_SUFFIX
    ));
    if !link.exists() {
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&real_backup, &link);
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_file(&real_backup, &link);
        #[cfg(not(any(unix, windows)))]
        let link_result = Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symlink not supported on this platform",
        ));
        if let Err(e) = link_result {
            log::warn!(
                "[ini_backup] create symlink failed {} -> {}: {}",
                real_backup.display(),
                link.display(),
                e
            );
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    /// 便捷构造备份文件名：`<stem>.ini_manager_backup`
    fn backup_name(stem: &str) -> String {
        format!("{}.{}", stem, constants::INI_MANAGER_BACKUP_SUFFIX)
    }

    /// 等待谓词成立（50ms 轮询，默认最多 5s），用于异步 spawn_safe 线程的测试同步
    fn wait_until<F: Fn() -> bool>(pred: F) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if pred() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        pred()
    }

    #[test]
    fn test_backup_plain_ini() {
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("my.mod.ini");
        fs::write(&ini, "[Test]\nkey=1\n").unwrap();

        backup_ini_files(dir.path());

        let backup = dir.path().join(backup_name("my.mod"));
        assert!(backup.exists(), "备份文件应存在");
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "[Test]\nkey=1\n",
            "备份内容应与原文件一致"
        );

        // 修改原文件后再次运行：备份内容不变（跳过不覆盖）
        fs::write(&ini, "[Test]\nkey=CHANGED\n").unwrap();
        backup_ini_files(dir.path());
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "[Test]\nkey=1\n",
            "已存在的备份不应被覆盖"
        );
    }

    #[test]
    fn test_backup_nested_dirs_dfs() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = a.join("b");
        let c = b.join("c");
        fs::create_dir_all(&c).unwrap();
        fs::write(a.join("a.ini"), "a").unwrap();
        fs::write(b.join("b.ini"), "b").unwrap();
        fs::write(c.join("c.ini"), "c").unwrap();
        fs::write(c.join("deep.ini"), "deep").unwrap();

        backup_ini_files(dir.path());

        assert!(a.join(backup_name("a")).exists());
        assert!(b.join(backup_name("b")).exists());
        assert!(c.join(backup_name("c")).exists());
        assert!(c.join(backup_name("deep")).exists());
    }

    #[test]
    fn test_backup_ignores_top_level_grouping() {
        // v2 核心：无视顶层分组——_MANAGED_/group_1、#Mutex、DISABLED_MANAGED_REMOVED、
        // Mods 根直属的非白名单 ini 全部被备份
        let dir = tempfile::tempdir().unwrap();
        let managed = dir.path().join("_MANAGED_").join("group_1").join("ModA");
        let mutex = dir.path().join("#Mutex").join("ModB");
        let removed = dir.path().join("DISABLED_MANAGED_REMOVED").join("ModC");
        fs::create_dir_all(&managed).unwrap();
        fs::create_dir_all(&mutex).unwrap();
        fs::create_dir_all(&removed).unwrap();
        fs::write(managed.join("a.ini"), "a").unwrap();
        fs::write(mutex.join("b.ini"), "b").unwrap();
        fs::write(removed.join("c.ini"), "c").unwrap();
        fs::write(dir.path().join("root.ini"), "root").unwrap();

        backup_ini_files(dir.path());

        assert!(managed.join(backup_name("a")).exists(), "_MANAGED_/group_1 内 ini 应备份");
        assert!(mutex.join(backup_name("b")).exists(), "#Mutex 内 ini 应备份");
        assert!(
            removed.join(backup_name("c")).exists(),
            "DISABLED_MANAGED_REMOVED 内 ini 应备份"
        );
        assert!(dir.path().join(backup_name("root")).exists(), "根直属 ini 应备份");
    }

    #[test]
    fn test_whitelist_not_backed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("desktop.ini"), "d").unwrap();
        fs::write(dir.path().join("manager_group.ini"), "m").unwrap();
        fs::write(dir.path().join("nrmm_include.ini"), "n").unwrap();
        fs::write(dir.path().join("group_1.ini"), "g").unwrap();
        fs::write(dir.path().join("GROUP_12.ini"), "G").unwrap(); // 大写也命中

        backup_ini_files(dir.path());

        for name in ["desktop", "manager_group", "nrmm_include", "group_1", "GROUP_12"] {
            assert!(
                !dir.path().join(backup_name(name)).exists(),
                "白名单文件 {} 不应产生备份",
                name
            );
        }
    }

    #[test]
    fn test_skip_existing_backup() {
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("mod.ini");
        fs::write(&ini, "NEW").unwrap();
        let backup = dir.path().join(backup_name("mod"));
        fs::write(&backup, "OLD").unwrap();

        backup_ini_files(dir.path());

        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "OLD",
            "预置备份内容不应被覆盖"
        );
    }

    #[test]
    fn test_backup_file_not_rebacked() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("mod.ini"), "x").unwrap();

        backup_ini_files(dir.path());
        backup_ini_files(dir.path());

        // 不存在 <stem>.ini_manager_backup.ini_manager_backup（备份自身不被再备份）
        assert!(!dir.path().join("mod.ini_manager_backup.ini_manager_backup").exists());
        // 目录内备份文件数量稳定（恰为 1）
        let backup_count = fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .map(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .ends_with(&format!(".{}", constants::INI_MANAGER_BACKUP_SUFFIX))
                    })
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(backup_count, 1, "备份文件数量应稳定为 1");
    }

    #[cfg(unix)]
    #[test]
    fn test_backup_symlink_ini() {
        let dir = tempfile::tempdir().unwrap();
        // 真实文件在 mods 外的 real_dir
        let real_dir = tempfile::tempdir().unwrap();
        let real = real_dir.path().join("real.ini");
        fs::write(&real, "REAL").unwrap();
        // symlink 在 mods 内指向它
        let link = dir.path().join("link.ini");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        backup_ini_files(dir.path());

        let real_backup = real_dir.path().join(backup_name("real"));
        assert!(real_backup.exists(), "真实目录应有实体备份");
        let link_backup = dir.path().join(backup_name("link"));
        assert!(link_backup.exists(), "链接所在目录应有备份链接");
        // 链接指向有效目标
        let target = fs::read_link(&link_backup).unwrap();
        assert!(target.try_exists().unwrap_or(false), "备份链接应指向有效目标");

        // 再次运行幂等：链接数量不翻倍（目录内 .ini_manager_backup 后缀项仍为 1）
        backup_ini_files(dir.path());
        let count = fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .map(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .ends_with(&format!(".{}", constants::INI_MANAGER_BACKUP_SUFFIX))
                    })
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(count, 1, "幂等重跑后备份链接数量应稳定");
    }

    #[test]
    fn test_ensure_startup_backup_once() {
        // 复位全局 flag，保证测试独立（模块内可访问 private static）
        BACKUP_TRIGGERED.store(false, Ordering::SeqCst);

        // 目录 A：首次调用应产生备份
        let dir_a = tempfile::tempdir().unwrap();
        fs::write(dir_a.path().join("a.ini"), "a").unwrap();
        ensure_startup_ini_backup(dir_a.path().to_path_buf());
        let backup_a = dir_a.path().join(backup_name("a"));
        assert!(
            wait_until(|| backup_a.exists()),
            "首次调用后（异步线程）目录 A 备份应出现"
        );

        // 目录 B：flag 已置位，再次调用不应产生新备份
        let dir_b = tempfile::tempdir().unwrap();
        fs::write(dir_b.path().join("b.ini"), "b").unwrap();
        ensure_startup_ini_backup(dir_b.path().to_path_buf());
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !dir_b.path().join(backup_name("b")).exists(),
            "flag 已置位后再次调用不应执行备份"
        );

        // 结束复位，避免污染其他测试
        BACKUP_TRIGGERED.store(false, Ordering::SeqCst);
    }
}
