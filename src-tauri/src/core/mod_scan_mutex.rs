//! 互斥组扫描域：栈式非递归 DFS 遍历 MutexGroup 目录树、互斥模组轻量构建
//!
//! 从 mod_scanner.rs 拆出（2026-09-19 Phase 5-4）。
//! 依赖 util（目录判定/图标/签名）与 marker（标记文件读写、还原窗口）；
//! 被 light（scan_mods_light/scan_single_mod）与 partial（scan_partial_path）消费。

use anyhow::Result;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::core::constants;
use crate::core::fs_walk::{self, TraversalStats};
use crate::core::mod_scan_marker::{
    read_groupname_marker, read_or_create_marker_file, restore_safe_active,
};
use crate::core::mod_scan_util::{
    compute_disk_signature, create_empty_slot_mod, dir_has_ini_file, find_icon_path,
    is_disabled_dir, is_group_xx_dir, metadata_mtime_nanos, sort_mods_light, DISABLED_PREFIX_RE,
};
use crate::models::enums::GroupType;
use crate::models::mod_data::{ModData, ModGroupData};

/// DFS 栈元素：待遍历目录及其父分组索引
///
/// 用于 `scan_mutex_group_dfs()` 中的栈式非递归 DFS 遍历。
///
/// # 字段
/// - `path`: 待遍历的目录路径
/// - `parent_group_idx`: 父分组在 `groups` 数组中的索引。`Some(0)` 表示根分组，`None` 表示无父分组（极少情况）
struct DfsStackItem {
    path: PathBuf,
    parent_group_idx: Option<usize>,
}

/// 使用栈式非递归 DFS 遍历 MutexGroup 目录树
///
/// 这是 MutexGroup 的核心扫描逻辑，使用 `Vec<DfsStackItem>` 模拟栈进行深度优先遍历。
///
/// # 算法流程
/// 1. 检查根目录是否本身就是模组（含 INI 文件）→ 直接返回叶子模组，不创建分组
/// 2. 创建根分组（`groups[0]`），将根目录的一级子目录逆序压入 DFS 栈
/// 3. 循环弹出栈顶元素：
///    - 如果当前目录包含 INI 文件 → 视为叶子模组，添加到父分组和 `all_mods` 列表，不再向下遍历
///    - 如果当前目录不包含 INI 文件 → 视为子分组节点，创建新的 `ModGroupData`，将其子目录逆序压入栈
/// 4. 对每个分组内的模组执行排序
/// 5. 调用 `rebuild_tree()` 递归重建树形结构（`child_groups` 层级关系）
/// 6. 收集所有模组到扁平列表
///
/// # 防循环机制
/// 使用 `HashSet<PathBuf> visited` 存储规范化（`canonicalize()`）后的绝对路径，
/// 在插入栈前检查是否已访问，防止符号链接/硬链接导致无限循环或重复扫描。
///
/// # 性能优化
/// - 使用 `entry.file_type()` 替代 `metadata()` 减少系统调用
/// - 预分配 `visited`、`stack`、`groups`、`all_mods` 的容量
/// - 叶子节点（含 INI）立即返回，不继续向下遍历
///
/// # Panics
/// 不会主动 panic，但 `read_or_create_marker_file()` 的 `?` 操作符可能在写入失败时传播 `Err`。
///
/// # Errors
/// 当目录无法读取、标记文件无法创建时返回 `Err`。
pub(crate) fn scan_mutex_group_dfs(
    root_path: &Path,
    stats: &TraversalStats,
) -> Result<(Option<ModGroupData>, Vec<Arc<ModData>>)> {
    let root_name = root_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let root_disabled = is_disabled_dir(&root_name);
    let base_root_name = if root_disabled {
        DISABLED_PREFIX_RE.replace(&root_name, "").to_string()
    } else {
        root_name.clone()
    };
    // 仅 group_xx 目录才读取 groupname 显示名（还原安全窗口内只读不建，窗口外缺失照常新建），
    // 非group目录直接使用目录名
    let root_display_name = if is_group_xx_dir(&root_name) {
        if restore_safe_active() {
            read_groupname_marker(root_path, &base_root_name)?
        } else {
            read_or_create_marker_file(&root_path.join("groupname"), &base_root_name)?
        }
    } else {
        base_root_name
    };

    // 规范化路径 visited 集合：防止 symlink/hardlink 导致循环或重复
    let mut visited: HashSet<PathBuf> = HashSet::with_capacity(64); // 预分配 64 个容量，覆盖大多数嵌套深度
    let root_canonical = root_path
        .canonicalize()
        .unwrap_or_else(|_| root_path.to_path_buf());
    visited.insert(root_canonical);

    // 检查根目录是否本身就是模组（含 ini）
    if dir_has_ini_file(root_path)? {
        // 根目录是模组叶子节点，不创建分组
        let mod_data = build_mutex_mod_light(root_path, 0, 0)?;
        return Ok((None, vec![Arc::new(mod_data)]));
    }

    // groups[0] 是根分组，后续是子分组
    let mut groups: Vec<ModGroupData> = Vec::with_capacity(32); // 预分配 32 个分组，覆盖大多数嵌套深度

    // 创建根分组
    let root_icon = find_icon_path(root_path)?;
    let root_group = ModGroupData {
        name: root_display_name,
        group_name: root_name,
        group_type: GroupType::MutexGroup,
        full_path: root_path.to_path_buf(),
        group_path: root_path.to_string_lossy().to_string(),
        group_index: 0,
        mods: vec![],
        mod_paths: vec![],
        mod_count: 0,
        has_child: true,
        children: vec![],
        child_groups: vec![],
        active_mod_index: -1,
        is_favorite: false,
        group_disabled: root_disabled,
        is_active: false,
        preview_image_path: root_icon,
        ..Default::default()
    };
    groups.push(root_group);

    // DFS 栈初始化（LIFO 后进先出）
    let mut stack: Vec<DfsStackItem> = Vec::with_capacity(64); // 预分配 64 个栈元素，覆盖大多数嵌套深度

    // 列出根目录的一级子目录，push 到栈（用 entry.file_type() 避免 metadata 系统调用）
    let root_entries = fs::read_dir(root_path)?;
    let mut root_subdirs: Vec<PathBuf> = Vec::with_capacity(8); // 预分配 8 个，大多数 MutexGroup 根目录子目录不多
    for entry in root_entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let ft = match entry.file_type() {
            Ok(f) => f,
            Err(_) => continue,
        };
        if ft.is_dir() || ft.is_symlink() {
            let p = entry.path();
            // UTF-8 合法化：根一级子目录非法名称跳过（子树剪枝）
            let name = match fs_walk::valid_entry_name(
                &entry,
                &root_path.to_string_lossy(),
                fs_walk::kind::DIR,
                stats,
            ) {
                Some(n) => n,
                None => continue,
            };
            if !name.starts_with('.') {
                let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
                if visited.insert(canon) {
                    root_subdirs.push(p);
                }
            }
        }
    }
    // 逆序 push 保持顺序（栈是 LIFO）
    for subdir in root_subdirs.into_iter().rev() {
        stack.push(DfsStackItem {
            path: subdir,
            parent_group_idx: Some(0),
        });
    }

    let mut global_mod_index: u32 = 0;

    while let Some(item) = stack.pop() {
        let current_path = item.path;
        let parent_idx = item.parent_group_idx;

        // 检查是否有 .ini 文件（叶子节点）
        if dir_has_ini_file(&current_path)? {
            let mod_data = build_mutex_mod_light(&current_path, 0, global_mod_index)?;

            // 添加到父分组（R3/C-3b：Arc 直接入组，不再经 all_mods 中转深拷贝；
            // 扁平列表由函数末尾 collect_mods 从树统一收集，与组内共享同一 Arc）
            if let Some(pidx) = parent_idx {
                let arc = Arc::new(mod_data);
                groups[pidx].mod_paths.push(arc.full_path.clone());
                groups[pidx].mods.push(arc);
                groups[pidx].mod_count += 1;
            }
            global_mod_index += 1;
            // 叶子节点，停止向下遍历
            continue;
        }

        // 没有 ini，视为分组节点
        let dir_name = current_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let dir_disabled = is_disabled_dir(&dir_name);
        let base_dir_name = if dir_disabled {
            DISABLED_PREFIX_RE.replace(&dir_name, "").to_string()
        } else {
            dir_name.clone()
        };
        // 仅 group_xx 目录才读取 groupname 显示名（还原安全窗口内只读不建，窗口外缺失照常新建），
        // 非group目录直接使用目录名
        let dir_display_name = if is_group_xx_dir(&dir_name) {
            if restore_safe_active() {
                read_groupname_marker(&current_path, &base_dir_name)?
            } else {
                read_or_create_marker_file(&current_path.join("groupname"), &base_dir_name)?
            }
        } else {
            base_dir_name.clone()
        };

        // 查找图标
        let icon_path = find_icon_path(&current_path)?;

        // 检查是否有子目录（用 entry.file_type() + canonicalize 去重）
        let sub_entries = fs::read_dir(&current_path)?;
        let mut subdirs: Vec<PathBuf> = Vec::with_capacity(8); // 预分配 8 个，大多数分组子目录数量有限
        for entry in sub_entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let ft = match entry.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            if ft.is_dir() || ft.is_symlink() {
                let p = entry.path();
                // UTF-8 合法化：深层子目录非法名称跳过（子树剪枝）
                let name = match fs_walk::valid_entry_name(
                    &entry,
                    &current_path.to_string_lossy(),
                    fs_walk::kind::DIR,
                    stats,
                ) {
                    Some(n) => n,
                    None => continue,
                };
                if !name.starts_with('.') {
                    let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
                    if visited.insert(canon) {
                        subdirs.push(p);
                    }
                }
            }
        }

        if subdirs.is_empty() {
            // 空子目录，忽略
            continue;
        }

        // 创建子分组
        let new_group_idx = groups.len();
        let child_group = ModGroupData {
            name: dir_display_name.clone(),
            group_name: dir_name.clone(),
            group_type: GroupType::MutexGroup,
            full_path: current_path.clone(),
            group_path: current_path.to_string_lossy().to_string(),
            group_index: 0,
            mods: vec![],
            mod_paths: vec![],
            mod_count: 0,
            has_child: true,
            children: vec![],
            child_groups: vec![],
            active_mod_index: -1,
            is_favorite: false,
            group_disabled: dir_disabled,
            is_active: false,
            preview_image_path: icon_path,
            ..Default::default()
        };
        groups.push(child_group);

        // 逆序 push 子目录到栈
        for subdir in subdirs.into_iter().rev() {
            stack.push(DfsStackItem {
                path: subdir,
                parent_group_idx: Some(new_group_idx),
            });
        }
    }

    // 对每个分组内的 mods 排序
    for group in &mut groups {
        // 空 mutex 分组注入 None 空槽位（对齐 scan_normal_group_light:395）：
        // 即使无模组也提供空槽位，供前端网格渲染可交互占位卡（双击添加模组）。
        if group.mods.is_empty() {
            group.mods.push(Arc::new(create_empty_slot_mod(0)));
        }
        sort_mods_light(&mut group.mods);
        // 重新分配 mod_index（make_mut：构建期各 Arc 唯一持有，原地改零拷贝）
        for (idx, m) in group.mods.iter_mut().enumerate() {
            Arc::make_mut(m).mod_index = idx as u32;
        }
    }

    // 更新根分组的 child_groups（因为后续分组修改了，需要重建）
    // 重新构建树形结构
    fn rebuild_tree(groups: &mut [ModGroupData], parent_idx: usize) {
        let child_indices: Vec<usize> = groups
            .iter()
            .enumerate()
            .filter(|(i, g)| {
                *i != parent_idx && g.full_path.parent() == Some(&groups[parent_idx].full_path)
            })
            .map(|(i, _)| i)
            .collect();

        groups[parent_idx].child_groups.clear();
        groups[parent_idx].children.clear();
        for ci in child_indices {
            rebuild_tree(groups, ci);
            // child_groups 维持值语义（skip_serializing 内部字段）；
            // children Arc 化——两者内容一致，children 的 mods 与原组共享 Arc
            groups[parent_idx].child_groups.push(groups[ci].clone());
            groups[parent_idx]
                .children
                .push(Arc::new(groups[ci].clone()));
        }
        groups[parent_idx].has_child =
            !groups[parent_idx].child_groups.is_empty() || !groups[parent_idx].mods.is_empty();
    }

    rebuild_tree(&mut groups, 0);

    // 收集根分组的所有 mods（包括子分组的）到扁平列表：
    // R3/C-3b 经 child_groups 递归收集 Arc 指针（与组内条目共享同一实例，
    // 增量合并的签名复用依赖该共享性）
    fn collect_mods(group: &ModGroupData, all: &mut Vec<Arc<ModData>>) {
        all.extend(group.mods.iter().cloned());
        for child in &group.child_groups {
            collect_mods(child, all);
        }
    }
    let mut all_mods: Vec<Arc<ModData>> = Vec::with_capacity(256);
    collect_mods(&groups[0], &mut all_mods);
    // 对齐 scan_mods_light:240——扁平列表不含 None 空槽位（仅组内 mods 保留以供网格渲染）
    all_mods.retain(|m| m.name != "None");

    Ok((Some(groups.remove(0)), all_mods))
}

/// 构建 MutexGroup 叶子模组的轻量数据（不解析 INI 内容）
///
/// 从目录名推断显示名称（移除 `DISABLED` 前缀），检查 fav 收藏标记文件、
/// 各种标记文件（`modforced`、`modsyntaxerrorremoved`、`modunoptimized`、`namespaced`），
/// 并查找图标路径。
///
/// 与 `scan_normal_group_light` 中的模组构建不同，此函数不处理 `selectedindex` 或 `is_active`，
/// 因为这些状态由 MutexGroup 的运行时逻辑管理。
///
/// 使用 `entry.file_type()` 避免 `metadata()` 系统调用。
pub(crate) fn build_mutex_mod_light(
    mod_path: &Path,
    group_index: u32,
    mod_index: u32,
) -> Result<ModData> {
    let dir_name = mod_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let disabled = is_disabled_dir(&dir_name);
    let display_name = if disabled {
        DISABLED_PREFIX_RE.replace(&dir_name, "").to_string()
    } else {
        dir_name
    };

    // NRMM 逻辑：互斥组下不创建 modname 文件，但若已存在则优先使用其内容作为展示名
    let modname_path = mod_path.join("modname");
    let mod_name = if modname_path.exists() {
        fs::read_to_string(&modname_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(display_name)
    } else {
        display_name
    };

    // 检查 fav 文件（一次 stat 同时取存在性与 mtime，替代原 exists() 调用）
    let fav_path = mod_path.join(constants::FAV_MARKER);
    let fav_meta = fs::metadata(&fav_path).ok();
    let is_favorite = fav_meta.is_some();
    let fav_nanos = metadata_mtime_nanos(fav_meta.as_ref());

    // 检查标记文件
    let mod_forced = mod_path.join(constants::MODFORCED_MARKER).exists();
    let is_namespaced = mod_path.join(constants::NAMESPACED_MARKER).exists();
    let _syntax_removed = mod_path.join("modsyntaxerrorremoved").exists();
    let _unoptimized = mod_path.join("modunoptimized").exists();

    // 查找图标
    let icon_path = find_icon_path(mod_path)?;

    // 磁盘签名（C-3b）：mod 目录 mtime ^ fav mtime 两源混合；
    // 互斥组无 selectedindex 标记文件且 is_active 由运行时管理（扫描恒 false），
    // 无组级派生源，组源恒 0
    let dir_nanos = metadata_mtime_nanos(fs::metadata(mod_path).ok().as_ref());
    let disk_signature = compute_disk_signature(dir_nanos, fav_nanos, 0);

    Ok(ModData {
        mod_path: mod_path.to_string_lossy().to_string(),
        mod_name: mod_name.clone(),
        name: mod_name,
        full_path: Arc::from(mod_path.to_string_lossy().as_ref()),
        parent_folder: Arc::from(
            mod_path
                .parent()
                .unwrap_or(mod_path)
                .to_string_lossy()
                .as_ref(),
        ),
        preview_image_path: icon_path,
        is_active: false,
        is_favorite,
        is_namespaced,
        disabled,
        mod_disabled: disabled,
        group_index,
        mod_index,
        is_mutex: true,
        has_nonmanaged_mods_crashline_fix: mod_forced,
        disk_signature,
        ..Default::default()
    })
}
