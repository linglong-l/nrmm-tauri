//! 模组启停域：单模组启停、互斥组启停、批量启停
//!
//! 从 mod_manager.rs 拆出（2026-09-19 Phase 4-3）。
//! 依赖 mod_scanner（DISABLED 前缀识别/剥离、目录扫描）、mod_path_utils（路径比较）、
//! constants、watcher_ring::WatcherGuard；被 commands/toggle_mod_disabled、
//! batch_toggle_mods、disable/enable_all 消费。

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::constants;
use crate::core::mod_path_utils::path_eq_ignore_case;
use crate::core::watcher_ring::WatcherGuard;

use crate::sel_dbg;

/// 错误前缀标识：目标模组路径无法在磁盘上定位，前端捕获后需执行清除缓存+重读模组
///
/// 与 `commands/mod_mod_commands.rs` 的 `ERR_PREFIX_PATH_NOT_FOUND` 是同一约定串。
/// core 不得反向依赖 commands 层（arch_gate F1 分层纪律），故在此重复登记字面量，
/// 由 `mod_toggle_tests` 断言其值，避免两处漂移。
const ERR_PATH_NOT_FOUND: &str = "[PATH_NOT_FOUND]";

/// 切换单个模组的启用/禁用状态（独立开关，不影响同组其他模组）
///
/// 检查磁盘实际状态，支持幂等操作：
/// - 传入路径不存在时，检查父目录下是否有对应的启用/禁用版本
/// - 传入路径存在时，检查是否需要重命名
/// - 已经是目标状态时，直接返回（幂等）
///
/// # 参数
/// - `mod_path`: 模组目录路径
/// - `enable`: true = 启用（移除 DISABLED 前缀），false = 禁用（添加 DISABLED 前缀）
pub fn toggle_mod(mod_path: &Path, enable: bool) -> Result<()> {
    let dir_name = mod_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let parent = mod_path.parent().unwrap_or(mod_path);
    let is_disabled = crate::core::mod_scan_util::is_disabled_dir(&dir_name);

    if enable {
        if mod_path.exists() {
            // 传入路径存在 → 检查是否需要启用
            if is_disabled {
                let new_name = crate::core::mod_scan_util::strip_disabled_prefix(&dir_name);
                let new_path = parent.join(new_name);
                if mod_path != new_path {
                    fs::rename(mod_path, &new_path)
                        .with_context(|| format!("Failed to enable mod: {:?}", mod_path))?;
                }
            }
            // 已启用 → 幂等，直接返回
        } else {
            // 传入路径不存在 → 检查父目录下是否有启用版本
            if is_disabled {
                let enabled_name = crate::core::mod_scan_util::strip_disabled_prefix(&dir_name);
                let enabled_path = parent.join(enabled_name);
                if enabled_path.exists() {
                    // 已启用 → 幂等，直接返回
                    return Ok(());
                }
            }
            // 路径不存在且无启用版本 → 返回错误
            return Err(anyhow::anyhow!("Mod path does not exist: {:?}", mod_path));
        }
    } else {
        // disable
        if mod_path.exists() {
            // 传入路径存在 → 检查是否需要禁用
            if !is_disabled {
                let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                let new_path = parent.join(new_name);
                if mod_path != new_path {
                    if new_path.exists() {
                        log::warn!(
                            "Target path already exists, skipping disable: {:?}",
                            new_path
                        );
                    } else {
                        fs::rename(mod_path, &new_path)
                            .with_context(|| format!("Failed to disable mod: {:?}", mod_path))?;
                    }
                }
            }
            // 已禁用 → 幂等，直接返回
        } else {
            // 传入路径不存在 → 检查父目录下是否有禁用版本
            if !is_disabled {
                let disabled_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                let disabled_path = parent.join(disabled_name);
                if disabled_path.exists() {
                    // 已禁用 → 幂等，直接返回
                    return Ok(());
                }
            }
            // 路径不存在且无禁用版本 → 返回错误
            return Err(anyhow::anyhow!("Mod path does not exist: {:?}", mod_path));
        }
    }

    Ok(())
}

/// 互斥启用模组：启用指定模组，禁用其父目录下同级的其他模组叶子节点（含.ini的目录）
///
/// - 只处理父目录下的一级子目录，不递归子分组
/// - 没有.ini的目录（分组节点/子分组）不处理
/// - 重命名保持幂等，检查目标路径是否存在避免覆盖
/// - 实现为「解析 → 计划 → 预检 → 提交（失败逆序回滚）」：改名要么全部完成，
///   要么回到调用前状态，不会因中途失败留下半禁用的一族兄弟目录
///
/// # 参数
/// - `mod_path`: 目标模组目录路径，必须能在父目录的叶子模组条目中唯一命中
///
/// # Errors
/// - 带 `[PATH_NOT_FOUND]` 前缀：`mod_path` 在父目录的含 .ini 条目中命中数为 0（目录已消失、
///   拼错，或非 Windows 宿主上的大小写不匹配）或 >1。此时**不生成也不执行任何改名**，
///   兄弟目录状态原样保留（修复「目标未命中即成批禁用且沉默返回 Ok」缺陷）。
/// - 读取/枚举父目录失败、探测 `.ini` 失败
/// - 计划预检失败：两个条目改名后目标名相同（或占用另一条目的现名）
/// - 任一 `fs::rename` 失败：已成功的改名会逆序回滚，随后返回原始错误
pub fn enable_mutex_mod(mod_path: &Path) -> Result<()> {
    // 暂停文件监听器，防止目录重命名触发增量更新竞态
    // WatcherGuard 在函数退出时（无论成功或出错）自动恢复监听器
    let _guard = WatcherGuard::new();
    sel_dbg!(
        "mod_manager",
        "enable_mutex_mod",
        "调用链: select_mod(互斥) → enable_mutex_mod | 入口 | 模组路径={:?} 模组名称={}",
        mod_path,
        mod_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    );

    let parent_dir = mod_path
        .parent()
        .with_context(|| format!("Failed to get parent directory of: {:?}", mod_path))?;

    // 解析：一次 read_dir 拿到父目录快照（不重复扫描磁盘）
    let snapshot = read_parent_snapshot(parent_dir)?;
    // 计划：目标命中校验 + 改名清单 + 目标名冲突预检
    let plan = build_mutex_enable_plan(parent_dir, &snapshot, mod_path)?;
    sel_dbg!(
        "mod_manager",
        "enable_mutex_mod",
        "步骤=改名计划生成 | 待执行改名数={} | 父目录={:?}",
        plan.len(),
        parent_dir
    );
    // 提交：按序执行，失败则逆序回滚
    commit_mutex_plan(&plan)
}

/// 父目录内的一个条目（名称 + 完整路径），由一次 `read_dir` 解析得到
struct SnapshotEntry {
    /// 条目名（`file_name`，用于前缀判定与隐藏判定）
    name: String,
    /// 条目完整路径（`parent_dir/名称`）
    path: PathBuf,
}

/// 父目录快照：全部条目 + 参与互斥切换的叶子模组条目
///
/// `all_paths` 用于「目标名已存在」判定，口径等价于逐条 `new_path.exists()`，
/// 但只在解析阶段查一次磁盘，使计划阶段完全无副作用。
struct ParentSnapshot {
    /// 父目录全部条目路径（含普通文件、隐藏项、不含 .ini 的目录）
    all_paths: Vec<PathBuf>,
    /// 是目录 + 名称不以 `.` 开头 + 含 .ini 的一级条目（互斥切换的作用域）
    leaves: Vec<SnapshotEntry>,
}

/// 一条待执行的改名动作（计划阶段产出，提交阶段按序执行、失败逆序回滚）
struct RenameOp {
    /// 改名前路径
    from: PathBuf,
    /// 改名后路径
    to: PathBuf,
    /// 动作描述，用于日志与错误上下文
    desc: &'static str,
}

/// 解析父目录：一次 `read_dir` 同时产出全部条目名与可参与互斥切换的叶子模组条目
///
/// # Errors
/// - 父目录不可读（不存在/权限不足）
/// - 目录项枚举中途失败
/// - 某条目探测 `.ini` 失败
fn read_parent_snapshot(parent_dir: &Path) -> Result<ParentSnapshot> {
    let entries = fs::read_dir(parent_dir)
        .with_context(|| format!("Failed to read parent directory: {:?}", parent_dir))?;

    let mut all_paths: Vec<PathBuf> = Vec::new();
    let mut leaves: Vec<SnapshotEntry> = Vec::new();

    for entry in entries {
        let entry = entry.with_context(|| {
            format!(
                "Failed to iterate parent directory entries: {:?}",
                parent_dir
            )
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        all_paths.push(path.clone());

        // 既有过滤口径原样保留：非目录、隐藏名（`.` 开头）、不含 .ini 的目录均不参与
        if !path.is_dir() || name.starts_with('.') {
            continue;
        }
        if !crate::core::mod_scan_util::dir_has_ini_file(&path)? {
            continue;
        }
        leaves.push(SnapshotEntry { name, path });
    }

    Ok(ParentSnapshot { all_paths, leaves })
}

/// 判定 `parent_dir` 下的名字 `name` 在快照中是否已被占用（等价原 `new_path.exists()`）
fn name_taken(snapshot: &ParentSnapshot, parent_dir: &Path, name: &str) -> bool {
    let candidate = parent_dir.join(name);
    snapshot
        .all_paths
        .iter()
        .any(|p| path_eq_ignore_case(p, &candidate))
}

/// 生成互斥启用的改名清单：目标去前缀启用（置于首位）+ 兄弟加前缀禁用
///
/// 纯计算：只读快照、只写日志（幂等/重名跳过），不触碰磁盘。
///
/// # Errors
/// - 目标命中数为 0 或 >1：带 `[PATH_NOT_FOUND]` 前缀返回，不产出任何改名
/// - 计划预检发现目标名冲突
fn build_mutex_enable_plan(
    parent_dir: &Path,
    snapshot: &ParentSnapshot,
    mod_path: &Path,
) -> Result<Vec<RenameOp>> {
    // 目标命中校验：宁可在找不到目标时报错，也绝不退化成「父目录全体禁用」
    let mut hits = 0usize;
    let mut target: Option<&SnapshotEntry> = None;
    for entry in &snapshot.leaves {
        if path_eq_ignore_case(&entry.path, mod_path) {
            hits += 1;
            target = Some(entry);
        }
    }
    if hits > 1 {
        bail!(
            "{}Target mod path is ambiguous ({} matching entries under parent {:?}): {:?}",
            ERR_PATH_NOT_FOUND,
            hits,
            parent_dir,
            mod_path
        );
    }
    let target = target.ok_or_else(|| {
        anyhow::anyhow!(
            "{}Target mod directory does not exist (no matching .ini mod dir under parent {:?}): {:?}",
            ERR_PATH_NOT_FOUND,
            parent_dir,
            mod_path
        )
    })?;

    let mut plan: Vec<RenameOp> = Vec::new();

    // 目标：处于禁用态则计划「去前缀启用」；去前缀后名字已被占用则沿用既有行为——warn 跳过
    if crate::core::mod_scan_util::is_disabled_dir(&target.name) {
        let new_name = crate::core::mod_scan_util::strip_disabled_prefix(&target.name);
        if new_name.is_empty() {
            // 极端情况：整个目录名只剩前缀，去前缀会得到空名（等价父目录自身），不改名
            log::warn!(
                "Target name carries only the DISABLED prefix, skipping enable: {:?}",
                target.path
            );
        } else if name_taken(snapshot, parent_dir, &new_name) {
            log::warn!(
                "Target path already exists, skipping enable: {:?}",
                parent_dir.join(&new_name)
            );
        } else {
            sel_dbg!(
                "mod_manager",
                "enable_mutex_mod",
                "步骤=启用目标模组(移除 DISABLED 前缀) | {:?} → {:?}",
                target.path,
                parent_dir.join(&new_name)
            );
            plan.push(RenameOp {
                from: target.path.clone(),
                to: parent_dir.join(new_name),
                desc: "Failed to enable mod",
            });
        }
    }

    // 兄弟：含 .ini 且处于启用态则计划「加 DISABLED 前缀」；前缀名已占用则 warn 跳过
    for entry in &snapshot.leaves {
        if path_eq_ignore_case(&entry.path, mod_path) {
            continue;
        }
        if crate::core::mod_scan_util::is_disabled_dir(&entry.name) {
            continue;
        }
        let new_name = format!("{}{}", constants::DISABLED_PREFIX, entry.name);
        if name_taken(snapshot, parent_dir, &new_name) {
            log::warn!(
                "Target path already exists, skipping disable: {:?}",
                parent_dir.join(&new_name)
            );
            continue;
        }
        plan.push(RenameOp {
            from: entry.path.clone(),
            to: parent_dir.join(new_name),
            desc: "Failed to disable sibling mod",
        });
    }

    precheck_mutex_plan(&plan)?;
    Ok(plan)
}

/// 执行前预检：计划内不得有两个动作争抢同一目标名，也不得改写另一动作的现名
///
/// 二者都会让结果依赖执行顺序（甚至互相覆盖），宁可拒绝执行也不做半吊子改名。
///
/// # Errors
/// - 计划内目标名重复，或目标名命中另一动作的源名
fn precheck_mutex_plan(plan: &[RenameOp]) -> Result<()> {
    for (i, op) in plan.iter().enumerate() {
        for (j, other) in plan.iter().enumerate() {
            if i == j {
                continue;
            }
            if path_eq_ignore_case(&op.to, &other.to) {
                bail!(
                    "Conflicting rename plan: both {:?} and {:?} target {:?}",
                    op.from,
                    other.from,
                    op.to
                );
            }
            if path_eq_ignore_case(&op.to, &other.from) {
                bail!(
                    "Conflicting rename plan: {:?} would overwrite the current name of {:?}",
                    op.to,
                    other.from
                );
            }
        }
    }
    Ok(())
}

/// 提交改名计划：按序执行；任一动作失败则逆序回滚已成功项，再返回原始错误
///
/// # Errors
/// - 任一 `fs::rename` 失败（权限/磁盘满/名字冲突等），回滚后仍返回该原始错误
fn commit_mutex_plan(plan: &[RenameOp]) -> Result<()> {
    let mut done: Vec<usize> = Vec::new();

    for (i, op) in plan.iter().enumerate() {
        let renamed =
            fs::rename(&op.from, &op.to).with_context(|| format!("{}: {:?}", op.desc, op.from));
        match renamed {
            Ok(()) => done.push(i),
            Err(err) => {
                rollback_mutex_plan(plan, &done);
                return Err(err);
            }
        }
    }

    Ok(())
}

/// 逆序回滚已执行的改名，使磁盘回到调用前状态
///
/// 回滚自身的失败只 `log::error!`（不 `Err` 上抛），避免覆盖调用方的原始错误。
fn rollback_mutex_plan(plan: &[RenameOp], done: &[usize]) {
    for &i in done.iter().rev() {
        let op = match plan.get(i) {
            Some(op) => op,
            None => continue,
        };
        match fs::rename(&op.to, &op.from) {
            Ok(()) => log::warn!(
                "enable_mutex_mod rolled back rename to restore previous state: {:?} → {:?}",
                op.to,
                op.from
            ),
            Err(e) => log::error!(
                "enable_mutex_mod ROLLBACK FAILED, disk needs manual check: {:?} → {:?}: {}",
                op.to,
                op.from,
                e
            ),
        }
    }
}

/// 禁用互斥模组：给模组目录添加DISABLED前缀
///
/// - 检查磁盘实际状态，支持幂等操作
/// - 传入路径不存在时，检查父目录下是否已有禁用版本
/// - 如果已经以DISABLED开头（大小写不敏感）则不做任何操作（幂等）
/// - 检查目标路径不存在才重命名
pub fn disable_mutex_mod(mod_path: &Path) -> Result<()> {
    let dir_name = mod_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let parent = mod_path.parent().unwrap_or(mod_path);
    let is_disabled = crate::core::mod_scan_util::is_disabled_dir(&dir_name);

    // 传入路径不存在 → 检查磁盘上是否已禁用
    if !mod_path.exists() {
        if !is_disabled {
            let disabled_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
            let disabled_path = parent.join(disabled_name);
            if disabled_path.exists() {
                // 已禁用 → 幂等，直接返回
                return Ok(());
            }
        }
        return Err(anyhow::anyhow!("Mod path does not exist: {:?}", mod_path));
    }

    // 传入路径存在 → 检查是否需要禁用
    if !is_disabled {
        let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
        let new_path = parent.join(new_name);
        if mod_path != new_path {
            if new_path.exists() {
                log::warn!(
                    "Target path already exists, skipping disable: {:?}",
                    new_path
                );
            } else {
                fs::rename(mod_path, &new_path)
                    .with_context(|| format!("Failed to disable mod: {:?}", mod_path))?;
            }
        }
    }

    Ok(())
}

/// 禁用指定分组下的所有一级模组（叶子节点，含 .ini 文件的目录）
///
/// 遍历分组目录下的所有直接子目录，对每个未禁用的模组添加 DISABLED 前缀。
/// 跳过：
/// - 非目录
/// - 已禁用的模组目录（名称以 DISABLED 开头）
/// - 不含 .ini 文件的目录（分组节点/子分组目录）
///
/// 不递归处理子分组目录，仅处理一级子目录。
///
/// # 参数
/// - `group_path`: 分组目录路径
///
/// # 返回
/// 成功禁用的模组数量（已禁用的跳过不计入）
///
/// # 错误
/// - 读取目录失败时返回错误
/// - 重命名失败时返回错误
pub fn disable_all_mods_in_group(group_path: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    if !group_path.exists() {
        return Ok(Vec::new());
    }
    if !group_path.is_dir() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(group_path)
        .with_context(|| format!("Failed to read group directory: {:?}", group_path))?;

    let mut renamed: Vec<(PathBuf, PathBuf)> = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }

        // 仅处理含 .ini 的叶子模组节点（不含 .ini 的是分组节点/子分组，跳过）
        let has_ini = match crate::core::mod_scan_util::dir_has_ini_file(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !has_ini {
            continue;
        }

        // 检查是否已禁用（名称以 DISABLED 开头，大小写不敏感）
        let is_disabled = crate::core::mod_scan_util::is_disabled_dir(&dir_name);
        if is_disabled {
            continue;
        }

        // 构建新目录名：DISABLED + 原名称
        let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
        let new_path = group_path.join(&new_name);

        // 检查目标路径是否已存在（避免覆盖）
        if new_path.exists() {
            log::warn!(
                "Target path already exists, skipping disable: {:?}",
                new_path
            );
            continue;
        }

        // 重命名添加前缀，并记录 (old, new) 供调用方零扫描更新缓存
        fs::rename(&path, &new_path)
            .with_context(|| format!("Failed to disable mod: {:?}", path))?;
        renamed.push((path, new_path));
    }

    Ok(renamed)
}

/// 判断模组是否属于MutexGroup（非group_xx目录下的模组）
///
/// 逻辑：获取mod_path相对于managed_path的路径，检查第一个路径段是否匹配group_xx正则；
/// 如果第一个路径段不匹配group_xx（即不是NormalGroup），则是mutex mod。
pub fn is_mutex_mod(mod_path: &Path, managed_path: &Path) -> bool {
    let rel_path = match mod_path.strip_prefix(managed_path) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let first_component = rel_path.components().next();
    match first_component {
        Some(std::path::Component::Normal(name)) => {
            let name_str = name.to_string_lossy();
            crate::core::mod_scan_util::is_normal_group_dir(&name_str).is_none()
        }
        _ => false,
    }
}

/// 批量切换模组启用/禁用状态
///
/// 只计数实际发生变更的模组数量。
///
/// # 参数
/// - `mod_paths`: 模组路径列表
/// - `enable`: true=启用, false=禁用
/// - `is_mutex`: 是否为互斥组模组
pub fn batch_toggle_mods(mod_paths: &[String], enable: bool, is_mutex: bool) -> Result<u32> {
    let mut count = 0u32;
    for path_str in mod_paths {
        let path = PathBuf::from(path_str);

        // 预检查是否需要操作
        let dir_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let parent = path.parent().unwrap_or(&path);
        let is_disabled = crate::core::mod_scan_util::is_disabled_dir(&dir_name);

        // 判断是否需要操作
        let needs_change = if enable {
            // 启用：路径存在且已禁用，或路径不存在但父目录有禁用版本
            if path.exists() {
                is_disabled
            } else {
                !is_disabled || {
                    let enabled_name = crate::core::mod_scan_util::strip_disabled_prefix(&dir_name);
                    !parent.join(enabled_name).exists()
                }
            }
        } else {
            // 禁用：路径存在且未禁用，或路径不存在但父目录有启用版本
            if path.exists() {
                !is_disabled
            } else {
                is_disabled || {
                    let disabled_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                    !parent.join(disabled_name).exists()
                }
            }
        };

        if !needs_change {
            continue;
        }

        if is_mutex {
            if enable {
                enable_mutex_mod(&path)?;
            } else {
                disable_mutex_mod(&path)?;
            }
        } else {
            toggle_mod(&path, enable)?;
        }
        count += 1;
    }
    Ok(count)
}

/// 启用指定分组下的所有一级模组（叶子节点，含 .ini 文件的目录）
///
/// 遍历分组目录下的所有直接子目录，对每个已禁用的模组移除 DISABLED 前缀。
/// 跳过：
/// - 非目录
/// - 已启用的模组目录（名称不以 DISABLED 开头）
/// - 不含 .ini 文件的目录（分组节点/子分组目录）
///
/// 不递归处理子分组目录，仅处理一级子目录。
///
/// # 参数
/// - `group_path`: 分组目录路径
///
/// # 返回
/// 成功启用的模组数量（已启用的跳过不计入）
///
/// # 错误
/// - 读取目录失败时返回错误
/// - 重命名失败时返回错误
pub fn enable_all_mods_in_group(group_path: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    if !group_path.exists() {
        return Ok(Vec::new());
    }
    if !group_path.is_dir() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(group_path)
        .with_context(|| format!("Failed to read group directory: {:?}", group_path))?;

    let mut renamed: Vec<(PathBuf, PathBuf)> = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }

        // 仅处理含 .ini 的叶子模组节点
        let has_ini = match crate::core::mod_scan_util::dir_has_ini_file(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !has_ini {
            continue;
        }

        // 检查是否已禁用（名称以 DISABLED 开头，大小写不敏感）
        let is_disabled = crate::core::mod_scan_util::is_disabled_dir(&dir_name);
        if !is_disabled {
            continue;
        }

        // 去除 DISABLED 前缀及前导分隔符 _ 空格 - 等
        let new_name = crate::core::mod_scan_util::strip_disabled_prefix(&dir_name);
        let new_name = if new_name.is_empty() {
            // 极端情况：整个目录名就是前缀，跳过
            continue;
        } else {
            new_name
        };

        let new_path = group_path.join(&new_name);

        if new_path.exists() {
            log::warn!(
                "Target path already exists, skipping enable: {:?}",
                new_path
            );
            continue;
        }

        // 源路径和目标路径相同则跳过
        if path == new_path {
            continue;
        }

        fs::rename(&path, &new_path)
            .with_context(|| format!("Failed to enable mod: {:?}", path))?;

        renamed.push((path, new_path));
    }

    Ok(renamed)
}

#[cfg(test)]
#[path = "mod_toggle_tests_disable_all.rs"]
mod tests_disable_all;

#[cfg(test)]
#[path = "mod_toggle_tests_enable_all.rs"]
mod tests_enable_all;

#[cfg(test)]
#[path = "mod_toggle_tests.rs"]
mod tests;
