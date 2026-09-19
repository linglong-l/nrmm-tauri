//! 轻量扫描域：ScanResult、scan_mods_light / scan_normal_group_light / scan_single_mod
//!
//! 从 mod_scanner.rs 拆出（2026-09-19 Phase 5-3）。
//! 依赖 util（目录判定/排序/签名）与 marker（标记文件读写）；被 commands/mod_cache/
//! file_watcher/各 mod_* 域消费。单模组扫描复用 mutex（build_mutex_mod_light）与
//! partial（normalize_subpath）。

use anyhow::Result;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::core::constants;
use crate::core::fs_walk::{self, SkippedEntry, TraversalStats};
use crate::core::mod_scan_marker::{
    read_groupname_marker, read_marker_readonly, read_or_create_marker_file, restore_safe_active,
};
use crate::core::mod_scan_util::{
    compute_disk_signature, create_empty_slot_mod, derive_group_active_state, dir_has_ini_file,
    find_icon_path, get_managed_folder, group_order_cmp, is_disabled_dir, is_normal_group_dir,
    metadata_mtime_nanos, mod_order_cmp, recount_mods, DISABLED_PREFIX_RE,
};
use crate::core::mod_scanner::{
    build_mutex_mod_light, normalize_subpath, scan_mods_deep, scan_mutex_group_dfs,
};
use crate::models::enums::{GroupType, ModsPathStatus, TargetGame};
use crate::models::mod_data::{ModData, ModGroupData};

/// 扫描结果结构体
///
/// 包含扫描得到的所有分组、模组列表和统计信息，是 `scan_mods_light()` 和 `scan_mods_deep()` 的返回值。
///
/// # 字段说明
/// - `groups`: 分组列表，包含 NormalGroup 和 MutexGroup 根节点，按 `group_index` 排序
/// - `mods`: 所有模组的扁平列表（不含 None 空槽位），便于前端直接渲染；
///   元素为 `Arc<ModData>`（R3/C-3b），与各分组 `mods` 内条目共享同一底层实例，
///   serde 序列化透明（前端 JSON 零变化）
/// - `total_mods_count`: 总模组数（不含 None 空槽位），用于统计展示
/// - `enabled_mods_count`: 启用的模组数，即 `!disabled && !mod_disabled` 的模组数量
/// - `disabled_mods_count`: 禁用的模组数，即 `disabled || mod_disabled` 的模组数量
/// - `invalid_utf8_count`: 因文件名非 UTF-8 而被跳过的条目总数（目录/文件/图片候选）
/// - `invalid_utf8_entries`: 被跳过的非法条目明细（受 [`TraversalStats::CAP_ENTRIES`]
///   上限约束，空列表时因 `skip_serializing_if` 不输出，前端 JSON 零变化）
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    /// 分组列表（NormalGroup + MutexGroup 根节点），按 group_index 排序
    pub groups: Vec<ModGroupData>,
    /// 所有模组的扁平列表（不含 None 空槽位）
    pub mods: Vec<Arc<ModData>>,
    /// 总模组数（不含 None 空槽位）
    pub total_mods_count: usize,
    /// 启用的模组数（!disabled && !mod_disabled）
    pub enabled_mods_count: usize,
    /// 禁用的模组数（disabled || mod_disabled）
    pub disabled_mods_count: usize,
    /// 因文件名非 UTF-8 被跳过的条目总数
    #[serde(default, skip_serializing_if = "is_zero")]
    pub invalid_utf8_count: usize,
    /// 被跳过的非法条目明细（空时省略）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalid_utf8_entries: Vec<SkippedEntry>,
}

/// serde 辅助：跳过 0 值字段
fn is_zero(v: &usize) -> bool {
    *v == 0
}

/// 默认扫描函数：`scan_mods_deep()` 的别名，保持向后兼容
///
/// 委托给 `scan_mods_deep()` 执行完整深度扫描（解析所有 INI 文件）。
///
/// # 使用场景
/// - UI 初始化时需要完整 INI 数据的场景
/// - 旧代码调用 `scan_mods()` 时无需修改
///
/// # Errors
/// 当 `_MANAGED_` 目录无法读取或创建时返回 `Err`
pub fn scan_mods(game_mods_path: &Path) -> Result<ScanResult> {
    scan_mods_deep(game_mods_path)
}

/// 检查模组路径状态
///
/// 依次检查以下条件，返回第一个失败的状态：
/// 1. `mods_path` 路径是否存在
/// 2. `_MANAGED_` 目录是否存在
/// 3. 主 INI 文件（`d3dx.ini`，全游戏统一）是否存在
///
/// # 返回
/// - `ModsPathStatus::Valid`: 路径有效
/// - `ModsPathStatus::NotFound`: `mods_path` 不存在
/// - `ModsPathStatus::ManagedFolderNotFound`: `_MANAGED_` 目录不存在
/// - `ModsPathStatus::D3dxIniNotFound`: 主 INI 文件不存在
pub fn check_mods_path(game: TargetGame, mods_path: &Path) -> ModsPathStatus {
    if !mods_path.exists() {
        return ModsPathStatus::NotFound;
    }
    let managed = mods_path.join(constants::MANAGED_FOLDER);
    if !managed.exists() {
        return ModsPathStatus::ManagedFolderNotFound;
    }
    let d3dx_name = game.d3dx_ini_name();
    // d3dx.ini 位于游戏根目录（Mods 的父目录），不在 Mods/ 内
    let d3dx_path = mods_path
        .parent()
        .map(|p| p.join(d3dx_name))
        .unwrap_or_else(|| mods_path.join(d3dx_name));
    if !d3dx_path.exists() {
        return ModsPathStatus::D3dxIniNotFound;
    }
    ModsPathStatus::Valid
}

/// 轻量扫描：混合目录扫描，不解析 INI 内容，不递归 NormalGroup
///
/// 这是 UI 列表展示的主要入口，速度极快（通常 < 100ms）。
///
/// # 扫描流程
/// 1. 读取 `_MANAGED_` 目录下所有一级子目录
/// 2. 使用 `is_normal_group_dir()` 区分 NormalGroup 和 MutexGroup
/// 3. NormalGroup 使用 `par_iter()` 并行调用 `scan_normal_group_light()`，仅扫描一级子目录，不递归
/// 4. MutexGroup 使用 `par_iter()` 并行调用 `scan_mutex_group_dfs()`，栈式 DFS 递归遍历
/// 5. 合并结果，按 `group_index` 排序后返回
///
/// # 收集的信息
/// - 目录路径、名称、显示名称（去掉 `DISABLED` 前缀）
/// - enabled 状态、fav 收藏状态、fav_timestamp
/// - 图标路径（通过 `find_icon_path()` 自动查找目录下的图片文件）
/// - `ini_file_paths`（仅路径，不解析内容）
///
/// # Panics
/// 不会主动 panic，但如果 `_MANAGED_` 目录无法创建或读取，会通过 `?` 向上传播错误。
///
/// # Errors
/// 当 `_MANAGED_` 目录不存在且无法创建，或 `fs::read_dir()` 读取失败时返回 `Err`。
pub fn scan_mods_light(game_mods_path: &Path) -> Result<ScanResult> {
    log::debug!(
        "[core::mod_scan_light] [scan_mods_light] Starting light scan | path={:?}",
        game_mods_path
    );
    let _s = std::time::Instant::now();
    let start = Instant::now();
    let managed_folder = get_managed_folder(game_mods_path);
    if !managed_folder.exists() {
        fs::create_dir_all(&managed_folder)?;
        return Ok(ScanResult {
            groups: vec![],
            mods: vec![],
            total_mods_count: 0,
            enabled_mods_count: 0,
            disabled_mods_count: 0,
            ..Default::default()
        });
    }

    let mut groups: Vec<ModGroupData> = Vec::with_capacity(32); // 预分配 32 个分组，覆盖绝大多数场景
    let mut all_mods: Vec<Arc<ModData>> = Vec::with_capacity(256); // 预分配 256 个模组，减少扩容次数

    let entries = fs::read_dir(&managed_folder)?;
    let mut root_dirs: Vec<PathBuf> = Vec::with_capacity(32); // 预分配存储根目录列表
    let stats = TraversalStats::default();
    let managed_display = managed_folder.to_string_lossy().to_string();

    for entry in entries {
        let entry = entry?;
        let ft = entry.file_type()?;
        if !ft.is_dir() {
            continue;
        }
        // UTF-8 合法化（方案A）：非法名称跳过 + 树剪枝（其下内容一并不见）
        let dir_name =
            match fs_walk::valid_entry_name(&entry, &managed_display, fs_walk::kind::DIR, &stats) {
                Some(n) => n,
                None => continue,
            };
        if dir_name.starts_with('.') {
            continue;
        }
        root_dirs.push(entry.path());
    }

    let root_dirs_vec: Vec<PathBuf> = root_dirs;

    // 先处理 NormalGroup（group_xx）：par_iter 并行扫描
    let normal_tasks: Vec<(PathBuf, String, u32)> = root_dirs_vec
        .iter()
        .filter_map(|dir_path| {
            let dir_name = dir_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            is_normal_group_dir(&dir_name).map(|gi| (dir_path.clone(), dir_name, gi))
        })
        .collect();

    let normal_results: Vec<(ModGroupData, Vec<Arc<ModData>>)> = normal_tasks
        .par_iter()
        .filter_map(|(dir_path, dir_name, group_index)| {
            scan_normal_group_light(dir_path, dir_name, *group_index, &stats).ok()
        })
        .collect();

    for (g, ms) in normal_results {
        groups.push(g);
        all_mods.extend(ms);
    }

    // 再处理 MutexGroup（非 group_xx 目录）：par_iter 并行扫描
    let mutex_roots: Vec<PathBuf> = root_dirs_vec
        .iter()
        .filter(|p| {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            is_normal_group_dir(&name).is_none()
        })
        .cloned()
        .collect();

    let mutex_results: Vec<(Option<ModGroupData>, Vec<Arc<ModData>>)> = mutex_roots
        .par_iter()
        .filter_map(|root_path| scan_mutex_group_dfs(root_path, &stats).ok())
        .collect();

    for (g_opt, ms) in mutex_results {
        if let Some(g) = g_opt {
            groups.push(g);
        }
        all_mods.extend(ms);
    }

    // 扁平 mods 列表不含 None 空槽位（None 仅存在于 groups[i].mods 内，用于槽位选择展示）
    all_mods.retain(|m| m.name != "None");

    // 排序（对齐 NRMM：group_xx 角色分组永远置顶，按 int 数字序；互斥分组按名称字符串序）
    // 注意：互斥分组根的 group_index 恒为 0，不能作为排序键（会把互斥组顶到最前）
    groups.sort_by(group_order_cmp);

    let (total, enabled, disabled) = recount_mods(&all_mods);

    let elapsed = start.elapsed().as_millis();
    log::info!(
        "Light scan completed in {}ms, {} mods, {} groups",
        elapsed,
        total,
        groups.len()
    );
    log::debug!(
        "[core::mod_scan_light] [scan_mods_light] done | elapsed={:?}ms | mods={} groups={}",
        _s.elapsed().as_millis(),
        total,
        groups.len()
    );

    let (invalid_utf8_count, invalid_utf8_entries) = stats.into_parts();
    if invalid_utf8_count > 0 {
        log::warn!(
            "[scan_mods_light] 跳过 {} 个非法 UTF-8 名称条目（明细 {} 条）",
            invalid_utf8_count,
            invalid_utf8_entries.len()
        );
    }

    Ok(ScanResult {
        groups,
        total_mods_count: total,
        enabled_mods_count: enabled,
        disabled_mods_count: disabled,
        mods: all_mods,
        invalid_utf8_count,
        invalid_utf8_entries,
    })
}

/// 轻量扫描普通分组（group_xx）：仅扫描一级子目录，不递归
///
/// 这是 NormalGroup 的扫描核心逻辑：
/// 1. 读取/创建 `groupname` 标记文件获取显示名称
/// 2. 读取/创建 `selectedindex` 标记文件获取当前选中的模组索引
/// 3. 插入索引为 0 的 None 空槽位（表示不选任何模组）
/// 4. 仅遍历一级子目录（绝对不递归），跳过隐藏目录和标记文件目录
/// 5. 为每个子目录查找 `modname` 标记文件、fav 收藏状态、图标路径
/// 6. 分离 None 槽位，对实际模组排序后重新组装
/// 7. 基于磁盘状态判定 `is_active`：恰好一个启用→该模组 active；零个启用→None active；多个启用→回退到 selectedindex 文件
///
/// 使用 `entry.file_type()` 判断文件类型，避免 `metadata()` 系统调用。
///
/// # Panics
/// 不会主动 panic，但 `read_or_create_marker_file()` 在写入失败时通过 `?` 传播错误。
///
/// # Errors
/// 当目录无法读取、标记文件无法创建时返回 `Err`。
pub(crate) fn scan_normal_group_light(
    dir_path: &Path,
    group_name: &str,
    group_index: u32,
    stats: &TraversalStats,
) -> Result<(ModGroupData, Vec<Arc<ModData>>)> {
    let mut mods: Vec<ModData> = Vec::with_capacity(32); // 预分配 32 个模组槽位，覆盖绝大多数 NormalGroup

    // 读取标记文件：还原安全窗口内只读不建（还原后目录将被删除/导出，不留专属配置），
    // 窗口外缺失照常新建
    let group_display_name = if restore_safe_active() {
        read_groupname_marker(dir_path, group_name)?
    } else {
        read_or_create_marker_file(&dir_path.join("groupname"), group_name)?
    };

    let selectedindex_path = dir_path.join(constants::SELECTED_INDEX_FILE);
    let selected_index_str = if restore_safe_active() {
        read_marker_readonly(&selectedindex_path, "0")?
    } else {
        read_or_create_marker_file(&selectedindex_path, "0")?
    };
    let selected_index: i32 = selected_index_str.parse().unwrap_or(0);
    // 磁盘签名组级源（C-3b）：selectedindex mtime，is_active 派生源——
    // select 改写该文件后组内全部模组签名失效，增量合并不复用旧实例。
    // 每组一次 stat，组内模组共享。
    let selectedindex_nanos = metadata_mtime_nanos(fs::metadata(&selectedindex_path).ok().as_ref());

    // 查找分组图标
    let group_icon = find_icon_path(dir_path)?;

    // 插入 None 空槽位（realIndex=0）
    mods.push(create_empty_slot_mod(group_index));

    // 仅读取一级子目录，绝对不递归
    let entries = fs::read_dir(dir_path)?;
    for entry in entries {
        let entry = entry?;
        let ft = match entry.file_type() {
            Ok(f) => f,
            Err(_) => continue,
        };
        if !ft.is_dir() && !ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        let dir_name = match fs_walk::valid_entry_name(
            &entry,
            &dir_path.to_string_lossy(),
            fs_walk::kind::DIR,
            stats,
        ) {
            Some(n) => n,
            None => continue,
        };
        if dir_name.starts_with('.') {
            continue;
        }
        // 跳过标记文件目录（不会是目录，这里仅作保险）
        if dir_name == "groupname"
            || dir_name == constants::SELECTED_INDEX_FILE
            || dir_name == "modname"
        {
            continue;
        }

        let real_index = mods.len() as u32;
        // 构建期 is_active 为目录枚举序对 selectedindex 的初值（组级派生随后统一覆盖）；
        // 单模组构建逻辑提炼至 build_normal_mod_light（C-3a 与 scan_single_mod 共用）
        mods.push(build_normal_mod_light(
            dir_path,
            group_index,
            &path,
            &dir_name,
            selectedindex_nanos,
            real_index as i32 == selected_index,
            real_index,
        )?);
    }

    // 分离 None 槽位和其他模组
    let mut none_slot: Option<ModData> = None;
    let mut other_mods: Vec<ModData> = Vec::with_capacity(mods.len().saturating_sub(1)); // 预分配（总数-1），排除 None 槽位
    for m in mods.drain(..) {
        if m.name == "None" {
            none_slot = Some(m);
        } else {
            other_mods.push(m);
        }
    }

    // 只对非 None 模组排序（构建期值语义，比较器与 sort_mods_light 共用单一真源）
    other_mods.sort_by(mod_order_cmp);

    // 重新组装：None 始终在第一位
    mods.push(none_slot.unwrap_or_else(|| create_empty_slot_mod(group_index)));
    mods.extend(other_mods);

    // 重新分配 mod_index（None 始终在 0，真实模组从 1 开始）
    for (idx, m) in mods.iter_mut().enumerate() {
        m.mod_index = idx as u32;
    }

    // 基于磁盘状态判定 is_active（单一真源：derive_group_active_state）
    let active_mod_index = derive_group_active_state(&mut mods, selected_index);

    // Arc 包装（R3/C-3b）：构建期保持值语义完成排序/索引/is_active 派生，
    // 此处一次性包装——组节点 mods 与返回的扁平列表共享同一批 Arc 实例
    //（下方 clone 仅克隆指针，消除原值语义下的整组深拷贝）。
    let arc_mods: Vec<Arc<ModData>> = mods.into_iter().map(Arc::new).collect();
    let mod_paths: Vec<Arc<str>> = arc_mods
        .iter()
        .filter(|m| m.name != "None")
        .map(|m| m.full_path.clone())
        .collect();

    let group = ModGroupData {
        name: group_display_name,
        group_name: group_name.to_string(),
        group_type: GroupType::NormalGroup,
        full_path: dir_path.to_path_buf(),
        group_path: dir_path.to_string_lossy().to_string(),
        group_index,
        mods: arc_mods.clone(),
        mod_count: arc_mods.len().saturating_sub(1) as u32, // 减去 None 槽位
        mod_paths,
        has_child: false,
        children: vec![],
        child_groups: vec![],
        active_mod_index,
        is_favorite: false,
        group_disabled: false,
        is_active: active_mod_index >= 0,
        preview_image_path: group_icon,
        ..Default::default()
    };

    Ok((group, arc_mods))
}

/// 构建 NormalGroup 单模组的轻量数据（C-3a 从 `scan_normal_group_light` 提炼）。
///
/// 涵盖组内模组构建的全部磁盘派生逻辑：`DISABLED` 前缀解析、`modname` 标记读取/创建、
/// fav 收藏状态（一次 stat 同时取存在性与 mtime）、标记文件检查、图标查找、磁盘签名
/// （mod 目录 mtime ^ fav mtime ^ 组 selectedindex mtime 三源混合）。供组级扫描
/// （逐子目录）与单模组扫描（[`scan_single_mod`]）共用，保证两路径产出字段口径一致。
///
/// # 参数
/// - `group_dir`: 所属组目录（`parent_folder` 字段源）。
/// - `group_index`: 组索引（`group_index` 字段源）。
/// - `mod_dir`: 模组目录。
/// - `dir_name`: 模组目录名（含可能的 `DISABLED` 前缀）。
/// - `selectedindex_nanos`: 组 selectedindex 文件 mtime（签名源，组内模组共享）。
/// - `is_active`: 构建期激活位（组扫描为目录枚举序对 selectedindex 的初值，组级派生
///   随后统一覆盖；单模组扫描为组语义近似派生值）。
/// - `mod_index`: 组内索引（组扫描为枚举序，重排后统一重分配；单模组扫描为占位 0，
///   由 `mod_cache::patch_single_mod` 保留旧值/置末位）。
///
/// # Errors
/// 当 `modname` 标记文件无法读取/创建时传播错误。
fn build_normal_mod_light(
    group_dir: &Path,
    group_index: u32,
    mod_dir: &Path,
    dir_name: &str,
    selectedindex_nanos: u64,
    is_active: bool,
    mod_index: u32,
) -> Result<ModData> {
    let disabled = is_disabled_dir(dir_name);
    let display_name = if disabled {
        DISABLED_PREFIX_RE.replace(dir_name, "").to_string()
    } else {
        dir_name.to_string()
    };

    // 读取/创建 modname 标记文件（NRMM 逻辑：优先使用 modname 文件中的名称作为展示名，不存在则创建并写入文件夹名）
    let modname_path = mod_dir.join("modname");
    let mod_name = {
        let n = if restore_safe_active() {
            read_marker_readonly(&modname_path, &display_name)?
        } else {
            read_or_create_marker_file(&modname_path, &display_name)?
        };
        if n.trim().is_empty() {
            display_name.clone()
        } else {
            n
        }
    };

    // 检查 fav 文件（一次 stat 同时取存在性与 mtime，替代原 exists() 调用）
    let fav_path = mod_dir.join(constants::FAV_MARKER);
    let fav_meta = fs::metadata(&fav_path).ok();
    let is_favorite = fav_meta.is_some();
    let fav_nanos = metadata_mtime_nanos(fav_meta.as_ref());

    // 检查各种标记文件
    let mod_forced = mod_dir.join(constants::MODFORCED_MARKER).exists();
    let _mod_syntax_error_removed = mod_dir.join("modsyntaxerrorremoved").exists();
    let _mod_unoptimized = mod_dir.join("modunoptimized").exists();
    let is_namespaced = mod_dir.join(constants::NAMESPACED_MARKER).exists();

    // 查找图标
    let icon_path = find_icon_path(mod_dir)?;

    // 磁盘签名（C-3b）：mod 目录 mtime ^ fav mtime ^ 组 selectedindex mtime
    // 三源混合。目录源必须用新鲜 stat（不能用 DirEntry::metadata 的枚举
    // 快照——上方 read_or_create_marker_file 首扫创建 modname 会改写目录
    // mtime，快照仍是旧值，导致首扫/复扫签名不一致）
    let dir_nanos = metadata_mtime_nanos(fs::metadata(mod_dir).ok().as_ref());
    let disk_signature = compute_disk_signature(dir_nanos, fav_nanos, selectedindex_nanos);

    Ok(ModData {
        mod_path: mod_dir.to_string_lossy().to_string(),
        mod_name: mod_name.clone(),
        name: mod_name,
        full_path: Arc::from(mod_dir.to_string_lossy().as_ref()),
        parent_folder: Arc::from(group_dir.to_string_lossy().as_ref()),
        preview_image_path: icon_path,
        is_active,
        is_favorite,
        is_namespaced,
        disabled,
        mod_disabled: disabled,
        group_index,
        mod_index,
        is_mutex: false,
        has_nonmanaged_mods_crashline_fix: mod_forced,
        disk_signature,
        ..Default::default()
    })
}

/// 统计组内启用（非 `DISABLED` 前缀、非隐藏）一级子目录，并判定目标模组是否为唯一启用者
/// （C-3a 单模组扫描的 `is_active` 组语义近似）。
///
/// 对齐 `scan_normal_group_light` 的组级 `is_active` 派生分支：
/// - 恰好一个启用 → 该模组 active（本函数返回 `true` 的唯一情形，与整组重扫一致）；
/// - 零个启用 → None 槽位 active，真实模组均 false（返回 `false`，一致）；
/// - 多个启用（异常态，如手动改目录名）→ 整组重扫回退到 selectedindex 指向的
///   排序位次；单模组扫描无法独立得出组内排序位，近似返回 `false`——仅当
///   selectedindex 非 0 且组内多启用（需外部手动破坏组状态）才会与整组重扫分歧，
///   下次组级事件（启用集/selectedindex 变化必经目录重命名，收敛根为组级）整组重扫校正。
///
/// 读取失败（组目录不可读）时返回 `false`（fail-safe：宁可不 active）。
///
/// # 参数
/// - `group_dir`: 组目录。
/// - `target_file_name`: 目标模组目录名（与组直接子项名比较）。
fn group_has_single_enabled_mod(group_dir: &Path, target_file_name: &str) -> bool {
    let entries = match fs::read_dir(group_dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    let mut enabled = 0usize;
    let mut target_enabled = false;
    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(f) => f,
            Err(_) => continue,
        };
        if !ft.is_dir() && !ft.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        // 跳过标记文件目录（不会是目录，与组扫描同样仅作保险）
        if name == "groupname" || name == constants::SELECTED_INDEX_FILE || name == "modname" {
            continue;
        }
        if is_disabled_dir(&name) {
            continue;
        }
        enabled += 1;
        if name == target_file_name {
            target_enabled = true;
        }
    }
    enabled == 1 && target_enabled
}

/// 单模组轻量扫描（C-3a）：对含 `.ini` 的模组目录产出单个 [`ModData`]。
///
/// 复用组级扫描的单模组构建逻辑（NormalGroup 走 [`build_normal_mod_light`]，MutexGroup
/// 走 [`build_mutex_mod_light`]），marker/modname/fav/签名等磁盘派生字段与整组扫描口径
/// 一致。组类型按父目录判定：
/// - 父目录为 `group_xx`（NormalGroup）→ 普通模组；`is_active` 按组语义近似派生
///   （唯一启用者 active，见 [`group_has_single_enabled_mod`]），签名含组
///   selectedindex mtime 源（文件不存在时源为 0）。
/// - 其他（MutexGroup 树节点）→ 互斥模组；`is_active` 恒 false（运行时管理），签名无组源。
///
/// `mod_index` 无法独立得出（依赖组内排序）：替换场景由 `mod_cache::patch_single_mod`
/// 保留旧实例值，新增场景置组内末位，均由下次组级扫描校正。
///
/// # 参数
/// - `mods_path`: 游戏 Mods 根目录（路径归一基准；`mod_dir` 可为 watcher 收敛产物的
///   canonicalize/verbatim 形态，内部统一重挂到 `mods_path/_MANAGED_` 之下，与缓存内
///   `full_path` 形态一致）。
/// - `mod_dir`: 模组目录（须存在且含 `.ini`）。
///
/// # 返回
/// `(模组数据, 所属组目录)`——组目录为 `mod_dir` 的父目录（归一形态），供缓存定位。
///
/// # Errors
/// - `mod_dir` 不在 `_MANAGED_` 子树内，或目标目录不含 `.ini`（非模组目录）；
/// - `modname` 标记文件读取/创建失败。
pub fn scan_single_mod(mods_path: &Path, mod_dir: &Path) -> Result<(Arc<ModData>, PathBuf)> {
    let managed_folder = get_managed_folder(mods_path);
    let rel = normalize_subpath(mod_dir, mods_path);
    if rel.is_empty() || rel == "_MANAGED_" {
        anyhow::bail!("scan_single_mod: 目标不在 _MANAGED_ 子树内: {:?}", mod_dir);
    }
    // 分段 join 保持平台原生分隔符：`rel` 为 '/' 分隔的归一串，直接整体 `join`
    // 会混入 '/'（如 `...\group_1/ModA`），与缓存内 full_path 的字符串匹配失效
    let target = rel
        .split('/')
        .fold(managed_folder, |acc, seg| acc.join(seg));
    if !dir_has_ini_file(&target)? {
        anyhow::bail!(
            "scan_single_mod: 目标目录不含 .ini（非模组目录）: {:?}",
            target
        );
    }
    let group_dir = target
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("scan_single_mod: 模组目录无父级: {:?}", target))?;
    let group_name = group_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let dir_name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let mod_data = if let Some(group_index) = is_normal_group_dir(&group_name) {
        // NormalGroup 模组：签名含组 selectedindex mtime 源（stat 即可，不读内容不创建——
        // 组扫描首扫会创建该文件，此处仅取 mtime，缺失时源为 0）
        let selectedindex_path = group_dir.join(constants::SELECTED_INDEX_FILE);
        let selectedindex_nanos =
            metadata_mtime_nanos(fs::metadata(&selectedindex_path).ok().as_ref());
        let is_active = group_has_single_enabled_mod(&group_dir, &dir_name);
        build_normal_mod_light(
            &group_dir,
            group_index,
            &target,
            &dir_name,
            selectedindex_nanos,
            is_active,
            0,
        )?
    } else {
        // MutexGroup 模组：is_active 恒 false，签名无组源（对齐 build_mutex_mod_light）
        build_mutex_mod_light(&target, 0, 0)?
    };
    Ok((Arc::new(mod_data), group_dir))
}
