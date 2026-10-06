//! 压缩包处理 —— 扁平化 / 目录与压缩包导入域
//!
//! 从 archive_handler.rs 拆出（2026-09-19 剩余巨石拆分 PA-3）。
//! 承载单层包裹目录展平（`unwrap_single_folder_nesting`）、已解压目录导入
//! （`import_directory`）、压缩包导入统一入口（`import_item`/`import_mod`/
//! `import_mod_auto`）及深层复制/文件遍历等辅助。类型契约见
//! [`crate::core::archive_detect`]，解压实现见 [`crate::core::archive_extract`]，
//! 导出见 [`crate::core::archive_export`]。

use crate::core::archive_detect::ExtractResult;
use crate::core::archive_extract::{extract_archive, mods_root_of};
use crate::core::operation_registry::OperationGuard;
use crate::core::resolution::compute_limits;
use crate::core::watcher_ring::WatcherGuard;
use anyhow::{anyhow, Result};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use trash::delete;
use walkdir::WalkDir;

/// 展平单层包裹目录（对齐 Dart `unwrapSingleFolderNesting`）
///
/// 反复检查 `dir`：若其「相关条目」（排除特定元数据/图标/缓存文件后）恰好只有一个，
/// 且该条目为目录，则将该子目录的内容上移到 `dir` 自身，并删除已清空的包裹目录。
/// 循环执行直到无法继续展平。
///
/// 排除的文件名（与 Dart 一致）：`modname` / `modforced` / `modsyntaxerrorremoved` /
/// `modunoptimized` / `modnamespaced` / `modlink` / `fav` / `.nahidamd` / 图标文件 /
/// `jasm_*` / `.jasm*` / `.imm*` / `*.txt` / `*.json`。
///
/// 子目录上移时使用 `unique_path` 处理命名冲突（对齐 Dart `getSafeTarget`），
/// 同盘优先 rename，跨盘回退 copy + 删除。
///
/// # 参数
/// - `dir`: 待展平的目录路径（通常是解压/导入后的最终模组目录）
///
/// # Errors
/// - `read_dir` 或条目移动失败时返回 IO 错误
pub fn unwrap_single_folder_nesting(dir: &Path) -> Result<()> {
    loop {
        let entries: Vec<_> = match fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(Result::ok).collect(),
            Err(_) => break,
        };

        // 过滤掉 Dart 约定的排除项
        let relevant: Vec<_> = entries
            .iter()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                !is_excluded_for_unwrap(&name)
            })
            .collect();

        // 多于 1 个相关条目，或没有 → 停止展平
        if relevant.len() != 1 {
            break;
        }

        let single = &relevant[0];
        let single_path = single.path();
        // 单个相关条目是文件（非目录）→ 停止
        if !single_path.is_dir() {
            break;
        }

        // 将子目录内容逐一上移到 dir 自身
        let children: Vec<_> = match fs::read_dir(&single_path) {
            Ok(rd) => rd.filter_map(Result::ok).collect(),
            Err(_) => break,
        };
        for child in children {
            let child_path = child.path();
            let target = dir.join(child.file_name());
            let target = unique_path(&target);
            move_entry_up(&child_path, &target)?;
        }

        // 删除已清空的包裹目录
        let _ = fs::remove_dir(&single_path);
    }
    Ok(())
}

/// 将条目移动到目标：优先 `rename`（同盘原子、高效），跨盘（EXDEV）等失败
/// 回退 `copy` + 删除源（M4：try-rename-first 跨平台统一，替代已删除的
/// `paths_on_same_disk` 非 Windows 恒 false 预判定）。
/// 目标由调用方保证不冲突（`unique_path`），此处仅做防御性清理。
fn move_entry_up(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        if dst.is_dir() {
            fs::remove_dir_all(dst)?;
        } else {
            fs::remove_file(dst)?;
        }
    }
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    // 跨盘回退：复制后删除源
    if src.is_dir() {
        copy_dir_deep(src, dst)?;
        let _ = fs::remove_dir_all(src);
    } else {
        fs::copy(src, dst)?;
        let _ = fs::remove_file(src);
    }
    Ok(())
}

/// 判断文件名是否应在展平时忽略（对齐 Dart `unwrapSingleFolderNesting` 的排除集）
fn is_excluded_for_unwrap(name: &str) -> bool {
    let lower = name.to_lowercase();
    const EXACT: &[&str] = &[
        "modname",
        "modforced",
        "modsyntaxerrorremoved",
        "modunoptimized",
        "modnamespaced",
        "modlink",
        "fav",
        ".nahidamd",
    ];
    if EXACT.contains(&lower.as_str()) {
        return true;
    }
    if lower.starts_with("jasm_") || lower.starts_with(".jasm") || lower.starts_with(".imm") {
        return true;
    }
    if lower.ends_with(".txt") || lower.ends_with(".json") {
        return true;
    }
    // 图标文件（对应 Dart ConstantVar.modIconFilenames）
    if crate::core::constants::ICON_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(*ext))
    {
        return true;
    }
    false
}

/// 遍历目录下所有文件（BFS + 路径池防 symlink 循环），对每个文件调用回调
///
/// 用于统计文件数量和检查是否包含 INI 文件。
/// 注意：仅遍历文件（`is_file`），不包含目录本身。
///
/// # 参数
/// - `dir`: 根目录路径
/// - `callback`: 每个文件路径的处理回调
///
/// # Errors
/// - `read_dir` 失败或权限不足时返回 IO 错误
pub(crate) fn visit_files(dir: &Path, callback: &mut dyn FnMut(&Path)) -> Result<()> {
    use std::collections::VecDeque;
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    // 路径池（visited + symlink 登记）：防 symlink 目录循环/重复遍历
    let mut pool = crate::core::fs_walk::PathPool::new();
    queue.push_back(dir.to_path_buf());
    pool.try_visit(dir);
    // 非法 UTF-8 名称计数（仅日志汇总，不上报前端）
    let stats = crate::core::fs_walk::TraversalStats::default();

    while let Some(current) = queue.pop_front() {
        let entries = fs::read_dir(&current)?;
        let current_display = current.to_string_lossy().to_string();
        for entry in entries {
            let entry = entry?;
            let ft = entry.file_type()?;
            let path = entry.path();
            // UTF-8 合法化：非法名称条目跳过（结构性路径）
            if crate::core::fs_walk::valid_entry_name(
                &entry,
                &current_display,
                crate::core::fs_walk::kind::FILE,
                &stats,
            )
            .is_none()
            {
                continue;
            }
            if is_dir_like(&entry, &ft) {
                // 目录（含 symlink 指向目录）：路径池判重后入队
                if pool.try_visit(&path) {
                    queue.push_back(path);
                }
            } else if ft.is_file() {
                callback(&path);
            }
        }
    }
    let (skipped, _entries) = stats.into_parts();
    if skipped > 0 {
        log::warn!("[visit_files] 跳过 {} 个非法 UTF-8 名称条目", skipped);
    }
    Ok(())
}

/// 判断目录项是否为「真实目录或指向目录的 symlink」（symlink 目录跟随，路径池防环）
fn is_dir_like(entry: &std::fs::DirEntry, ft: &std::fs::FileType) -> bool {
    if ft.is_dir() {
        return true;
    }
    ft.is_symlink()
        && entry
            .path()
            .metadata()
            .map(|md| md.is_dir())
            .unwrap_or(false)
}

/// 生成唯一路径：如果路径已存在，追加 `_1`, `_2` 等数字后缀
///
/// 避免模组导入时因重名覆盖已有文件。
/// 处理方式：`path` → 检查存在 → 存在则 `path_stem_1.ext` → 存在则 `path_stem_2.ext` → ...
///
/// # 参数
/// - `path`: 期望的原始路径
///
/// # 返回
/// 不存在的唯一路径
fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    let mut counter = 1;
    loop {
        let candidate = if ext.is_empty() {
            parent.join(format!("{}_{}", stem, counter))
        } else {
            parent.join(format!("{}_{}.{}", stem, counter, ext))
        };
        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

/// 导入目录（已解压的模组目录）到分组——深层遍历复制（对齐 NRMM 多层文件夹导入）
///
/// # 行为
/// - `dir_name` = 源目录名，使用 `unique_path` 避免重名（对齐 Dart `getSafeTarget` 的 `_N` 后缀）
/// - 获取源目录相对路径，在 `target_group_dir/dir_name` 下重建完整目录结构
/// - 逐文件流式复制：同盘优先 `fs::rename`（快速路径），失败回退 `fs::copy`
///   （注：为保证「任意文件复制失败 → 仅删除复制目录、保留原目录」的回滚不变式，
///   跨盘场景始终为复制；同盘场景优先 rename 以获得最佳性能，失败时仅删除目标目录）
/// - 符号链接：直接复制链接本体（文件链接拷贝目标内容、目录链接重建链接），
///   不解析、不递归，避免死循环遍历
/// - 任意文件复制失败 → 返回错误并删除已复制的目标目录（取消本次导入）
/// - 全部成功 → 通过回收站回收（trash）原目录
///
/// # 参数
/// - `src`: 源目录路径
/// - `target_group_dir`: 目标分组目录路径
///
/// # 返回
/// 最终安装到的 `PathBuf`（已处理重名）
///
/// # Errors
/// - 源不存在或不是目录
/// - 目录名为空
/// - 目标目录创建失败
/// - 复制操作失败（权限不足、磁盘空间不足等）
pub fn import_directory(src: &Path, target_group_dir: &Path) -> Result<PathBuf> {
    if !src.is_dir() {
        return Err(anyhow!(
            "import_directory: src is not a directory: {:?}",
            src
        ));
    }

    let dir_name = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    if dir_name.is_empty() {
        return Err(anyhow!(
            "import_directory: src directory name is empty: {:?}",
            src
        ));
    }

    // 暂停文件监视器：深层复制期间高密度写入目标会触发监视器扫描并生成 `modname` 标记，
    // 导致后续单层展平判定失败（B 修复）。守卫引用计数，函数退出自动恢复。
    let _watch_guard = WatcherGuard::new();

    fs::create_dir_all(target_group_dir)?;

    let target = target_group_dir.join(&dir_name);
    let target = unique_path(&target);

    // 深层遍历复制；失败则回滚（删除已复制目录），原目录保持不变
    if let Err(e) = copy_dir_deep(src, &target) {
        let _ = fs::remove_dir_all(&target);
        return Err(anyhow!("import_directory: 复制失败已回滚: {}", e));
    }

    // 展平单层包裹目录（对齐 Dart unwrapSingleFolderNesting）
    if let Err(e) = unwrap_single_folder_nesting(&target) {
        log::warn!("import_directory: 展平单层目录失败 {:?}: {}", target, e);
    }

    // 成功：回收站回收原目录（不阻断流程，失败仅告警）
    match delete(src) {
        Ok(_) => {}
        Err(e) => log::warn!(
            "import_directory: 回收原目录失败 {:?}: {}，原目录保留",
            src,
            e
        ),
    }

    Ok(target)
}

/// 深层遍历复制目录树：在 `dst` 下重建与 `src` 相同的目录结构并流式复制每个文件。
///
/// - 同盘文件优先 `fs::rename`（快速路径），否则 `fs::copy`
/// - 符号链接直接复制（不解析、不递归）
/// - 任一股文件复制失败立即返回 `Err`（调用方负责删除 `dst`）
pub(crate) fn copy_dir_deep(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;

    let mut jobs: Vec<(PathBuf, PathBuf)> = Vec::new();
    for entry in WalkDir::new(src).follow_links(false).min_depth(1) {
        let entry = entry?;
        let path = entry.path();
        let rel = match path.strip_prefix(src) {
            Ok(r) => r,
            Err(_) => continue,
        };
        jobs.push((path.to_path_buf(), dst.join(rel)));
    }

    // 并行复制（std::fs + rayon 线程池回退）
    let failures: Vec<String> = jobs
        .par_iter()
        .filter_map(|(s, d)| match copy_one(s, d) {
            Ok(_) => None,
            Err(e) => Some(format!("{}: {}", s.display(), e)),
        })
        .collect();

    if !failures.is_empty() {
        return Err(anyhow!(
            "copy_dir_deep: {} 个文件复制失败\n{}",
            failures.len(),
            failures.join("\n")
        ));
    }
    Ok(())
}

/// 复制单个条目（文件/目录/符号链接）到目标。
fn copy_one(src: &Path, dst: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(src)?;

    // 目录：仅创建（内容由其子项 jobs 处理）
    if meta.is_dir() {
        fs::create_dir_all(dst)?;
        return Ok(());
    }

    // 确保父目录存在
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    // 符号链接 / junction：跳过，不重建、不跟随（阻断导入逃逸到 Mods 根外，R3）
    // 正常 mod 不含符号链接；保留它会把指向 Mods 外的 junction 带进导入目录，
    // 后续扫描 / 还原遍历该 junction 会在外部创建 / 改写 INI。
    if meta.is_symlink() {
        log::warn!(
            "copy_one: skip symlink/junction (not imported to prevent escape): {:?}",
            src
        );
        return Ok(());
    }

    // 普通文件：优先 rename（M4：try-rename-first，跨盘失败再 copy）
    let _ = fs::remove_file(dst);
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    fs::copy(src, dst).map(|_| ())?;
    Ok(())
}

/// 导入模组到指定分组：统一入口，支持目录和压缩包
///
/// 根据 `fs::metadata` 判断：
/// - `is_dir` → 调用 `import_directory`（成功后包装成 `ExtractResult::Success`）
/// - `is_file` → 调用 `import_mod`（即 `extract_archive`）
/// - 其他（如符号链接、socket 等）→ `Err`
///
/// # 参数
/// - `src_path`: 源路径（文件或目录）
/// - `target_group_dir`: 目标分组目录路径
/// - `password`: 可选压缩包密码
///
/// # 返回
/// `ExtractResult` 枚举（成功或各类失败详情）
///
/// # Panics
/// 不会 panic
///
/// # Errors
/// - `src_path` 既不是文件也不是目录
/// - `metadata` 获取失败（路径不存在、权限不足等）
pub fn import_item(
    src_path: &Path,
    target_group_dir: &Path,
    password: Option<&str>,
) -> Result<ExtractResult> {
    // 跨进程建议锁：确保同一游戏 Mods 目录仅一个 NRMM 进程执行导入，避免文件竞争
    // （与 update_mod_data / 备份共用同一锁文件，互斥覆盖整个游戏 Mods 根）。
    let mods_root = mods_root_of(target_group_dir);
    let _proc_lock = match crate::core::file_lock::AdvisoryLock::acquire_blocking(
        &mods_root,
        Duration::from_secs(30),
    ) {
        Ok(Some(l)) => l,
        Ok(None) => return Err(anyhow!("另一个 NRMM 实例正在操作该游戏目录，请稍后再试")),
        Err(e) => return Err(e),
    };

    // 重量级后台操作登记：供用户主动退出时检查「是否仍在导入」，若有则取消退出。
    let _op_guard = OperationGuard::new("import_mod");

    let meta = fs::metadata(src_path)
        .map_err(|e| anyhow!("import_item: failed to stat {:?}: {}", src_path, e))?;

    if meta.is_dir() {
        let final_path = import_directory(src_path, target_group_dir)?;
        let mod_name = final_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let mut has_ini = false;
        let mut file_count = 0usize;
        let mut counter = |path: &Path| {
            file_count += 1;
            if let Some(ext) = path.extension() {
                if ext.eq_ignore_ascii_case("ini") {
                    has_ini = true;
                }
            }
        };
        let _ = visit_files(&final_path, &mut counter);
        Ok(ExtractResult::Success {
            mod_path: final_path,
            mod_name,
            has_ini,
            file_count,
        })
    } else if meta.is_file() {
        import_mod(src_path, target_group_dir, password)
    } else {
        Err(anyhow!(
            "import_item: {:?} is neither file nor directory",
            src_path
        ))
    }
}

/// 导入模组压缩包到指定分组
///
/// 解压 `archive_path` 压缩包到 `group_dir` 目录下。
/// 实际委托给 `extract_archive` 处理完整流程。
///
/// # 参数
/// - `archive_path`: 压缩包路径
/// - `group_dir`: 目标分组目录路径
/// - `password`: 可选压缩包密码
///
/// # 返回
/// `ExtractResult` 枚举
///
/// # Panics
/// 不会 panic
///
/// # Errors
/// 同 `extract_archive`：7z 未找到、IO 错误、解压失败等
pub fn import_mod(
    archive_path: &Path,
    group_dir: &Path,
    password: Option<&str>,
) -> Result<ExtractResult> {
    // 暂停文件监视器：解压会把大量文件写入分组目录，监视器若实时扫描会写入 `modname` 等标记、
    // 触发增量扫描管线，既浪费 CPU 也是在「先于移动执行展平」之前导致智能解压退化的诱因
    // （B 修复）。守卫为引用计数，函数退出（含错误路径）自动恢复监听。
    let _watch_guard = WatcherGuard::new();
    extract_archive(archive_path, group_dir, password)
}

/// 自动导入模组到第一个存在的分组
///
/// 从 `group_1` 开始扫描，找到第一个已存在的分组目录作为目标。
/// 如果所有分组目录（1-100）都不存在，创建 `group_1` 并导入。
/// 上限 100 个分组，超过则默认使用 `group_1`。
///
/// # 参数
/// - `archive_path`: 压缩包路径
/// - `mods_path`: 游戏 Mods 目录路径（会自动拼接 `_MANAGED_`）
/// - `password`: 可选压缩包密码
///
/// # 返回
/// `ExtractResult` 枚举
///
/// # Panics
/// 不会 panic
///
/// # Errors
/// 同 `import_mod`：7z 未找到、IO 错误、解压失败等
pub fn import_mod_auto(
    archive_path: &Path,
    mods_path: &Path,
    password: Option<&str>,
) -> Result<ExtractResult> {
    use crate::core::constants;
    let managed_dir = mods_path.join(constants::MANAGED_FOLDER);
    if !managed_dir.exists() {
        fs::create_dir_all(&managed_dir)?;
    }

    let limits = compute_limits();
    let mut group_num = 1u32;
    let group_dir = loop {
        let name = format!("group_{}", group_num);
        let path = managed_dir.join(&name);
        if path.exists() {
            break path;
        }
        group_num += 1;
        if group_num > limits.max_groups {
            let path = managed_dir.join("group_1");
            fs::create_dir_all(&path)?;
            break path;
        }
    };

    // 跨进程建议锁：与 import_item / update_mod_data / 备份共用同一锁文件
    let mods_root = mods_root_of(&group_dir);
    let _proc_lock = match crate::core::file_lock::AdvisoryLock::acquire_blocking(
        &mods_root,
        Duration::from_secs(30),
    ) {
        Ok(Some(l)) => l,
        Ok(None) => return Err(anyhow!("另一个 NRMM 实例正在操作该游戏目录，请稍后再试")),
        Err(e) => return Err(e),
    };

    // 重量级后台操作登记：供用户主动退出时检查「是否仍在导入」，若有则取消退出。
    let _op_guard = OperationGuard::new("import_mod");

    import_mod(archive_path, &group_dir, password)
}

#[cfg(test)]
#[path = "archive_import_tests.rs"]
mod tests;
