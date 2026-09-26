//! 深度扫描域：完整解析 INI，递归扫描所有子目录（scan_mods_deep / build_mod_data_deep 等）
//!
//! 从 mod_scanner.rs 拆出（2026-09-19 Phase 5-5）。
//! 依赖 util（目录判定/正则）、light（ScanResult 返回类型）、mod_ini_cache 与 ini 模型；
//! 仅供 update_mod_data 使用，MutexGroup 的深度扫描不由此处理。

use anyhow::Result;
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::core::constants;
use crate::core::fs_walk::{self, TraversalStats};
use crate::core::ini_model::IniFile;
use crate::core::mod_ini_cache::get_or_parse_ini;
use crate::core::mod_scan_light::ScanResult;
use crate::core::mod_scan_util::{
    get_managed_folder, is_disabled_dir, is_normal_group_dir, DISABLED_PREFIX_RE, ICON_EXTENSIONS,
};
use crate::core::namespace_handler;
use crate::models::enums::GroupType;
use crate::models::mod_data::{ErroredLines, ModData, ModGroupData, ModIniData};

/// 深度扫描：完整解析 INI，递归扫描所有子目录（原 `scan_mods` 重命名）
///
/// 与轻量扫描的区别：
/// - 完整解析所有 INI 文件内容，统计各类型段数量（[TextureOverride], [ShaderOverride] 等）
/// - 提取错误行（`crash_causing_lines`）、未定义引用、namespace 变量
/// - 递归扫描所有子目录（包括 NormalGroup 内部，使用 BFS + visited 去重）
/// - 收集已知库（`defined_libraries`）
/// - 仅处理 NormalGroup（group_xx 目录），MutexGroup 的深度扫描不由此函数处理
///
/// # 使用场景
/// 仅在 `update_mod_data`（点击"应用"按钮）时调用，用于：
/// - 注入槽位条件到 INI
/// - 检测语法错误和未定义引用
/// - 展开 namespace 变量
///
/// # 扫描流程
/// 1. 读取 `_MANAGED_` 目录下的所有一级子目录
/// 2. 仅保留 `is_normal_group_dir()` 匹配的 NormalGroup 目录
/// 3. 对每个 NormalGroup 调用 `scan_group_directory_deep()`（BFS 递归遍历）
/// 4. 收集所有模组的 `defined_libraries` 到 `known_libraries`
/// 5. 按 `group_index` 排序后返回
///
/// 耗时：几百毫秒到几秒（取决于 INI 数量和大小）
///
/// # Panics
/// 不会主动 panic，但 `_MANAGED_` 目录无法创建时通过 `?` 传播错误。
///
/// # Errors
/// 当 `_MANAGED_` 目录不存在且无法创建，或 `fs::read_dir()` 读取失败时返回 `Err`。
pub fn scan_mods_deep(game_mods_path: &Path) -> Result<ScanResult> {
    let managed_folder = get_managed_folder(game_mods_path);
    let stats = TraversalStats::default();
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

    // 收集严格 group_<int> 一级目录（与串行版同过滤条件），非 group 目录（# 互斥分组等）
    // 不读、不写、不遍历——update_mod_data 对其零接触（2026-08-18 用户决策，对齐 NRMM 原版）
    let managed_display = managed_folder.to_string_lossy().to_string();
    let group_dirs: Vec<(PathBuf, String)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let ft = entry.file_type().ok()?;
            if !ft.is_dir() && !ft.is_symlink() {
                return None;
            }
            // UTF-8 合法化：分组目录非法名称跳过（扫不出该组，告知用户）
            match fs_walk::valid_entry_name(&entry, &managed_display, fs_walk::kind::DIR, &stats) {
                Some(n) => {
                    is_normal_group_dir(&n)?;
                    Some((entry.path(), n))
                }
                None => None,
            }
        })
        .collect();

    // 分组并行扫描（对齐 scan_mods_light 的 par_iter 模式）：
    // 每组任务独立（组内 BFS 自带 visited 路径池），结果收集后统一排序合并
    let scan_results: Vec<Result<(ModGroupData, Vec<Arc<ModData>>)>> = group_dirs
        .par_iter()
        .map(|(path, dir_name)| {
            scan_group_directory_deep(path, dir_name, GroupType::NormalGroup, &stats)
        })
        .collect();

    // 按分组 id 升序合并（BTreeMap 迭代序确定，组内保持扫描顺序），与串行版结果一致
    let mut grouped: std::collections::BTreeMap<u32, (ModGroupData, Vec<Arc<ModData>>)> =
        std::collections::BTreeMap::new();
    for result in scan_results {
        let (group, mods) = result?;
        grouped.insert(group.group_index(), (group, mods));
    }
    for (_, (group, mods)) in grouped {
        groups.push(group);
        all_mods.extend(mods);
    }

    let enabled = all_mods
        .iter()
        .filter(|m| !m.disabled && !m.mod_disabled)
        .count();
    let disabled = all_mods
        .iter()
        .filter(|m| m.disabled || m.mod_disabled)
        .count();

    let (invalid_utf8_count, invalid_utf8_entries) = stats.into_parts();
    if invalid_utf8_count > 0 {
        log::warn!(
            "[scan_mods_deep] 跳过 {} 个非法 UTF-8 名称条目（明细 {} 条）",
            invalid_utf8_count,
            invalid_utf8_entries.len()
        );
    }

    Ok(ScanResult {
        groups,
        total_mods_count: all_mods.len(),
        enabled_mods_count: enabled,
        disabled_mods_count: disabled,
        mods: all_mods,
        invalid_utf8_count,
        invalid_utf8_entries,
    })
}

/// 深度扫描分组目录：BFS 递归遍历，查找所有包含 INI 的模组目录
///
/// 使用 `VecDeque` 进行广度优先搜索（BFS），配合 `HashSet<PathBuf> visited_dirs` 防止循环。
/// 与轻量扫描不同，此函数会递归扫描所有子目录（包括 NormalGroup 内部的多层嵌套）。
///
/// # 算法细节
/// - 使用队列进行 BFS 遍历
/// - 通过 `canonicalize()` 规范化路径后存入 `visited_dirs` 去重
/// - 遇到包含 INI 文件的目录时，视为模组叶子节点，不再继续向下遍历
/// - 不包含 INI 但有子目录的节点，视为子分组（`ModGroupData`），继续 BFS
/// - 最终重建分组的 `mod_index` 和 `group_index` 等字段
///
/// # Errors
/// 当目录无法读取时返回 `Err`。
fn scan_group_directory_deep(
    dir_path: &Path,
    group_name: &str,
    group_type: GroupType,
    stats: &TraversalStats,
) -> Result<(ModGroupData, Vec<Arc<ModData>>)> {
    use std::collections::VecDeque;

    let mut mods = Vec::with_capacity(256); // 预分配 256 个模组
    let mut subgroups: Vec<ModGroupData> = Vec::with_capacity(32); // 预分配 32 个子分组
    let mut subgroup_paths: HashSet<PathBuf> = HashSet::with_capacity(32);

    let mut queue = VecDeque::new();
    queue.push_back(dir_path.to_path_buf());

    let mut visited_dirs = HashSet::with_capacity(64); // 规范化路径 visited 集合，防止 symlink 循环
    visited_dirs.insert(dir_path.to_path_buf());

    while let Some(current_path) = queue.pop_front() {
        // 跳过分组根目录本身的模组检测：分组根目录可能包含 NRMM 生成的
        // group_XX.ini 管理文件，不应将其视为模组。仅扫描子目录作为模组。
        let is_group_root = current_path == dir_path;

        let (has_ini, has_icon, icon_path, ini_files) = if is_group_root {
            (false, false, None, Vec::with_capacity(0))
        } else {
            check_directory_for_mod_deep(&current_path, stats)?
        };

        if has_ini || has_icon {
            let parent_groups: Vec<String> = Vec::with_capacity(4);
            let mod_data = build_mod_data_deep(
                &current_path,
                dir_path,
                group_name,
                &parent_groups,
                ini_files,
                icon_path,
            )?;
            mods.push(mod_data);
        } else {
            let sub_entries = match fs::read_dir(&current_path) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let mut has_subdirs = false;
            for sub_entry in sub_entries {
                let sub_entry = match sub_entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let ft = match sub_entry.file_type() {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                if ft.is_dir() || ft.is_symlink() {
                    let sub_path = sub_entry.path();
                    // UTF-8 合法化：深扫子目录非法名称跳过（子树剪枝）
                    let sub_name = match fs_walk::valid_entry_name(
                        &sub_entry,
                        &current_path.to_string_lossy(),
                        fs_walk::kind::DIR,
                        stats,
                    ) {
                        Some(n) => n,
                        None => continue,
                    };
                    if sub_name.starts_with('.') {
                        continue;
                    }
                    has_subdirs = true;
                    let canon = sub_path.canonicalize().unwrap_or_else(|_| sub_path.clone());
                    if !visited_dirs.contains(&canon) {
                        visited_dirs.insert(canon);
                        queue.push_back(sub_path);
                    }
                }
            }

            if current_path != dir_path && has_subdirs && !subgroup_paths.contains(&current_path) {
                subgroup_paths.insert(current_path.clone());
                // 深扫子分组名来自 visited 收集（合法），file_name 安全取用
                let subgroup_name = current_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let subgroup = ModGroupData {
                    name: subgroup_name.clone(),
                    group_name: subgroup_name,
                    group_type,
                    full_path: current_path.clone(),
                    group_path: current_path.to_string_lossy().to_string(),
                    ..Default::default()
                };
                subgroups.push(subgroup);
            }
        }
    }

    let group_index: u32 = group_name.trim_start_matches("group_").parse().unwrap_or(0);

    let mod_paths: Vec<Arc<str>> = mods.iter().map(|m| m.full_path.clone()).collect();

    // R3/C-3b：组节点持有「索引修正前」快照（与原值语义 mods.clone() 一致——
    // 原实现组节点先克隆、随后仅修正扁平列表），此处包装为独立 Arc
    let group_mods: Vec<Arc<ModData>> = mods.iter().map(|m| Arc::new(m.clone())).collect();

    let has_children = !mods.is_empty() || !subgroups.is_empty();
    let group = ModGroupData {
        name: group_name.to_string(),
        group_name: group_name.to_string(),
        group_type,
        full_path: dir_path.to_path_buf(),
        group_path: dir_path.to_string_lossy().to_string(),
        group_index,
        child_groups: subgroups.clone(),
        children: subgroups.into_iter().map(Arc::new).collect(),
        has_child: has_children,
        mod_count: mods.len() as u32,
        mod_paths,
        mods: group_mods,
        ..Default::default()
    };

    for (idx, mod_data) in mods.iter_mut().enumerate() {
        mod_data.group_index = group_index;
        mod_data.mod_index = idx as u32 + 1; // 1-based，对齐原版 NRMM
        // 槽位身份（BFS 发现序 = 组内目录枚举序，与轻扫 slot_id / Dart realIndex
        // 同源）——$managed_slot_id 注入取此值。深扫无展示重排，故与 mod_index 同值。
        mod_data.slot_id = idx as u32 + 1;
        mod_data.mod_path = mod_data.full_path.as_ref().to_string();
        mod_data.mod_name = mod_data.name.clone();
        mod_data.mod_disabled = mod_data.disabled;
        if let Some(first_ini) = mod_data.mod_ini_data.first() {
            mod_data.mod_ini = Some(first_ini.clone());
        }
        if mod_data.namespace.is_some() {
            mod_data.is_namespaced = true;
            mod_data.namespaces = mod_data.namespace.clone().into_iter().collect();
        }
    }

    // 深扫产物不填磁盘签名（默认 0 = 无签名）：深扫结果不进入 MOD_CACHE，
    // 增量合并链路仅消费轻量扫描产物，无需签名
    Ok((group, mods.into_iter().map(Arc::new).collect::<Vec<_>>()))
}

/// 深度扫描：检查目录是否包含 INI 文件和图标
///
/// 使用 `entry.file_type()` 判断文件类型，避免 `metadata()` 系统调用。
/// 返回四元组：`(has_ini, has_icon, icon_path, ini_files)`。
///
/// - `has_ini`: 是否至少有一个 `.ini` 文件
/// - `has_icon`: 是否至少有一个图片文件
/// - `icon_path`: 优先返回 `icon.*` 命名的图片，否则返回第一张非 `DISABLED` 前缀的图片
/// - `ini_files`: 所有 `.ini` 文件的路径列表（已排序）
fn check_directory_for_mod_deep(
    dir: &Path,
    stats: &TraversalStats,
) -> Result<(bool, bool, Option<PathBuf>, Vec<PathBuf>)> {
    let mut has_ini = false;
    let mut has_icon = false;
    let mut icon_path: Option<PathBuf> = None;
    let mut ini_files: Vec<PathBuf> = Vec::with_capacity(8);
    let mut first_image: Option<PathBuf> = None;

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok((false, false, None, vec![])),
    };

    let dir_display = dir.to_string_lossy().to_string();

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let ft = match entry.file_type() {
            Ok(f) => f,
            Err(_) => continue,
        };
        if ft.is_file() || ft.is_symlink() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                let ext_cow = ext.to_string_lossy();
                if ext_cow.eq_ignore_ascii_case("ini") {
                    // NRMM 对齐：显式跳过桌面配置文件（系统 INI，不参与模组 INI 注入/统计）
                    if constants::is_desktop_ini(&path) {
                        continue;
                    }
                    // 结构性路径（INI 文件）：非法名称跳过 + 上报
                    match fs_walk::valid_entry_name(
                        &entry,
                        &dir_display,
                        fs_walk::kind::FILE,
                        stats,
                    ) {
                        Some(_) => {
                            has_ini = true;
                            ini_files.push(path);
                        }
                        None => continue,
                    }
                } else if ICON_EXTENSIONS
                    .iter()
                    .any(|e| ext_cow.eq_ignore_ascii_case(e))
                {
                    // 装饰性路径（图片）：非法名称跳过该候选（前端走占位逻辑，无需上报）
                    if entry.file_name().to_str().is_none() {
                        log::warn!(
                            "[check_directory_for_mod_deep] 跳过非法 UTF-8 名称图片候选 | dir={:?}",
                            dir
                        );
                        continue;
                    }
                    has_icon = true;
                    let stem = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase();
                    if stem == "icon" {
                        icon_path = Some(path);
                    } else if first_image.is_none() {
                        let stem_lower = stem.to_lowercase();
                        if !stem_lower.starts_with("disable") {
                            first_image = Some(path);
                        }
                    }
                }
            }
        }
    }

    if icon_path.is_none() {
        icon_path = first_image;
    }

    // 若目录已被识别为模组（有 icon 或直接 INI），则 DFS 遍历整个子树
    // 收集所有 .ini 文件，对齐原版 NRMM 的递归 INI 发现行为。
    // group_int/ 一级子目录 = 模组根，模组内所有 INI 均属该模组。
    if has_ini || has_icon {
        collect_ini_files_dfs(dir, &mut ini_files, stats);
        if !ini_files.is_empty() {
            has_ini = true;
        }
    }

    ini_files.sort();
    Ok((has_ini, has_icon, icon_path, ini_files))
}

/// DFS 遍历模组目录子树，收集所有 .ini 文件。
///
/// 从 `root` 的**子目录**开始遍历（跳过 root 本身——其文件已由调用方收集），
/// 跳过 `.` 前缀和 `DISABLED` 前缀的目录，将找到的 .ini 追加到 `ini_files`。
fn collect_ini_files_dfs(root: &Path, ini_files: &mut Vec<PathBuf>, stats: &TraversalStats) {
    let mut stack: Vec<PathBuf> = Vec::new();
    // 路径池（visited + symlink 登记）：防 symlink 目录循环/重复遍历
    let mut pool = fs_walk::PathPool::new();
    let root_display = root.to_string_lossy().to_string();
    // symlink 目录跟随遍历（路径池判重防环），symlink 文件按 ini 收集
    let push_dir = |stack: &mut Vec<PathBuf>, pool: &mut fs_walk::PathPool, path: PathBuf| {
        if pool.try_visit(&path) {
            stack.push(path);
        }
    };
    // 从 root 的直接子目录开始遍历（root 直属 ini 已由调用方收集，本函数不重复收集）
    if let Ok(entries) = fs::read_dir(root) {
        for e in entries.flatten() {
            let ft = match e.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            if !is_dir_entry(&e, &ft) {
                continue;
            }
            let name = match fs_walk::valid_entry_name(&e, &root_display, fs_walk::kind::DIR, stats)
            {
                Some(n) => n,
                None => continue,
            };
            // 跳过隐藏目录与被禁用目录（大小写不敏感，与 is_disabled_dir/contains_disabled_segment 一致）
            if name.starts_with('.') || is_disabled_dir(&name) {
                continue;
            }
            let path = e.path();
            // 目录（含 symlink 指向目录）：入栈继续遍历
            push_dir(&mut stack, &mut pool, path);
        }
    }
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let dir_display = dir.to_string_lossy().to_string();
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            let path = entry.path();
            let name =
                match fs_walk::valid_entry_name(&entry, &dir_display, fs_walk::kind::DIR, stats) {
                    Some(n) => n,
                    None => continue,
                };
            if name.starts_with('.') || is_disabled_dir(&name) {
                continue;
            }
            // 目录（含 symlink 指向目录）：路径池判重后入栈
            if is_dir_entry(&entry, &ft) {
                push_dir(&mut stack, &mut pool, path);
            } else if is_file_entry(&entry, &ft)
                && is_ini_path(&path)
                && !constants::is_desktop_ini(&path)
            {
                // 结构性路径（模组 INI 文件）：非法名称跳过 + 上报
                if fs_walk::valid_entry_name(&entry, &dir_display, fs_walk::kind::FILE, stats)
                    .is_some()
                {
                    ini_files.push(path);
                }
            }
        }
    }
}

/// 判断目录项是否为「真实目录或指向目录的 symlink」（不跟随解析类型）
fn is_dir_entry(entry: &fs::DirEntry, ft: &fs::FileType) -> bool {
    if ft.is_dir() {
        return true;
    }
    // symlink：解析 metadata 判断指向类型（目录则跟随，文件则忽略此处）
    ft.is_symlink()
        && entry
            .path()
            .metadata()
            .map(|md| md.is_dir())
            .unwrap_or(false)
}

/// 判断目录项是否为「真实文件或指向文件的 symlink」（symlink 文件按文件收集）
fn is_file_entry(entry: &fs::DirEntry, ft: &fs::FileType) -> bool {
    if ft.is_file() {
        return true;
    }
    ft.is_symlink()
        && entry
            .path()
            .metadata()
            .map(|md| md.is_file())
            .unwrap_or(false)
}

/// 判断路径是否以 .ini 为扩展名（大小写不敏感）
fn is_ini_path(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case("ini"))
        .unwrap_or(false)
}

/// 深度扫描：构建完整 ModData（解析所有 INI 文件）
///
/// 此函数是深度扫描的核心，对每个 `.ini` 文件执行：
/// - 调用 `IniFile::parse()` 解析 INI 内容
/// - 统计各类型段数量（KeyPress、TextureOverride、ShaderOverride、CommandList、Resource）
/// - 提取 namespace 变量
/// - 收集 `defined_libraries`
/// - 检测错误行（`detect_errors()`）
///
/// 所有统计信息累加到最终 `ModData` 中，供前端展示和 apply 注入使用。
fn build_mod_data_deep(
    dir: &Path,
    group_root: &Path,
    _group_name: &str,
    _parent_groups: &[String],
    ini_files: Vec<PathBuf>,
    icon_path: Option<PathBuf>,
) -> Result<ModData> {
    let dir_name = dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    // 检查从分组根目录到当前目录的完整路径中是否有任意段以 DISABLED 开头。
    // 修复此前仅检查叶子目录名导致的 DISABLED_xxx/sub/mod.ini 误判为启用的问题。
    let disabled = constants::contains_disabled_segment(dir, group_root);

    // NRMM 逻辑：优先使用 modname 文件内容作为展示名；不存在则回退文件夹名（深度扫描不创建，由轻量扫描负责创建）
    let display_name = if disabled {
        DISABLED_PREFIX_RE.replace(&dir_name, "").to_string()
    } else {
        dir_name
    };
    let modname_path = dir.join("modname");
    let mod_name = if modname_path.exists() {
        fs::read_to_string(&modname_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(display_name)
    } else {
        display_name
    };

    let mut mod_ini_data: Vec<ModIniData> = Vec::with_capacity(8); // 预分配 8 个 INI 数据，大多数模组只有少量 INI
    let mut all_errored_lines: Vec<ErroredLines> = Vec::with_capacity(8); // 预分配 8 个错误行
    let mut total_sections = 0;
    let mut total_key_sections = 0;
    let mut total_texture_sections = 0;
    let mut total_shader_sections = 0;
    let mut total_command_lists = 0;
    let mut total_resources = 0;
    let mut mod_namespace: Option<String> = None;
    let mut defined_libraries = HashSet::with_capacity(32); // 预分配 32 个库，覆盖大多数模组

    // 禁用模组跳过内容解析（2026-08-18 性能优化）：update_mod_data 仅修改启用模组 INI，
    // 禁用模组的统计字段/错误行无消费方（UI 走 scan_mods_light 路径），
    // 仅保留路径信息（section 计数等置 0），避免首次运行的无效全量解析。
    if disabled {
        for ini_path in &ini_files {
            let ini_filename = ini_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            mod_ini_data.push(ModIniData {
                ini_path: ini_path.to_string_lossy().to_string(),
                ini_filename,
                file_relative_path: ini_path.to_string_lossy().to_string(),
                ..Default::default()
            });
        }
    } else {
        // 两阶段深扫（任务 C2）：跨 INI 库引用（A INI 的 `run` 等引用 B INI 声明的
        // ORFix/TexFx 库名）必须在检测前先汇总全部 `defined_libraries`，否则
        // `detect_errors` 恒以 `HashSet::new()` 判定，把所有跨库引用误报为
        // NON EXISTENT LIB（error_type=5）。解析仍走 `mod_ini_cache`，缓存命中零重解析。

        // 阶段一（收集）：解析并暂存所有 INI，汇总全局 known_libraries。
        let mut parsed_inis: Vec<(PathBuf, IniFile)> = Vec::with_capacity(ini_files.len());
        for ini_path in &ini_files {
            match get_or_parse_ini(ini_path) {
                Ok(ini) => {
                    defined_libraries.extend(ini.defined_libraries());
                    parsed_inis.push((ini_path.clone(), ini));
                }
                Err(e) => {
                    log::warn!("Failed to parse INI {}: {}", ini_path.display(), e);
                    all_errored_lines.push(ErroredLines {
                        error_type: 3,
                        error_message: format!("Parse error: {}", e),
                        ..Default::default()
                    });
                }
            }
        }

        // 阶段二（检测）：带汇总后的全局 known_libraries 逐个 INI 物化错误并构建统计/ModIniData。
        for (ini_path, ini) in &parsed_inis {
            let mut keys = 0usize;
            let mut textures = 0usize;
            let mut shaders = 0usize;
            let mut commands = 0usize;
            let mut resources = 0usize;

            for section in &ini.sections {
                let sname = section.name.to_lowercase();
                if sname.starts_with("keypress")
                    || (sname.starts_with("key")
                        && !sname.starts_with("keyboard")
                        && !sname.starts_with("keybind"))
                {
                    keys += 1;
                } else if sname.starts_with("textureoverride") {
                    textures += 1;
                } else if sname.starts_with("shaderoverride") {
                    shaders += 1;
                } else if sname.starts_with("commandlist") {
                    commands += 1;
                } else if sname.starts_with("resource") {
                    resources += 1;
                }
            }

            total_sections += ini.sections.len();
            total_key_sections += keys;
            total_texture_sections += textures;
            total_shader_sections += shaders;
            total_command_lists += commands;
            total_resources += resources;

            if mod_namespace.is_none() {
                mod_namespace = namespace_handler::extract_namespace(ini);
            }

            let errors = ini.detect_errors(ini_path, &defined_libraries);
            all_errored_lines.extend(errors);

            let ini_filename = ini_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            mod_ini_data.push(ModIniData {
                ini_path: ini_path.to_string_lossy().to_string(),
                ini_filename,
                file_relative_path: ini_path.to_string_lossy().to_string(),
                section_count: ini.sections.len() as u32,
                key_sections: keys as u32,
                texture_override_sections: textures as u32,
                shader_override_sections: shaders as u32,
                command_list_sections: commands as u32,
                resource_sections: resources as u32,
                has_include: ini.has_include(),
                ..Default::default()
            });
        }
    }

    let is_namespaced = mod_namespace.is_some();
    let namespaces_vec = mod_namespace
        .as_ref()
        .map(|ns| vec![ns.clone()])
        .unwrap_or_default();

    Ok(ModData {
        name: mod_name.clone(),
        mod_name,
        full_path: Arc::from(dir.to_string_lossy().as_ref()),
        mod_path: dir.to_string_lossy().to_string(),
        parent_folder: Arc::from(dir.parent().unwrap_or(dir).to_string_lossy().as_ref()),
        preview_image_path: icon_path,
        disabled,
        mod_disabled: disabled,
        mod_ini_data,
        errored_lines: all_errored_lines,
        namespace: mod_namespace,
        namespaces: namespaces_vec,
        is_namespaced,
        known_libraries: defined_libraries.into_iter().collect(),
        key_sections: total_key_sections as u32,
        texture_override_sections: total_texture_sections as u32,
        shader_override_sections: total_shader_sections as u32,
        command_list_sections: total_command_lists as u32,
        resource_sections: total_resources as u32,
        total_section_count: total_sections as u32,
        is_mutex: false,
        ..Default::default()
    })
}

#[cfg(test)]
#[path = "mod_scan_deep_tests.rs"]
mod tests;
