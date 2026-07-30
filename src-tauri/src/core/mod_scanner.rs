//! 模组扫描模块
//!
//! 提供两种扫描模式：
//! - **轻量扫描 (scan_mods_light)**: 不解析 INI 内容，仅检查目录结构和标记文件，速度快，用于 UI 列表展示
//! - **深度扫描 (scan_mods_deep)**: 完整解析所有 INI 文件，统计段数量、检测错误、提取 namespace，用于 apply 时的 INI 注入
//!
//! # 分组类型
//! - **NormalGroup (group_xx)**: 普通分组，一级子目录为模组，不递归。每组同一时间只能启用一个模组（互斥槽位）
//! - **MutexGroup (非 group_xx 目录)**: 互斥组，支持任意深度嵌套（DFS 遍历），同级目录下的模组互斥
//!
//! # 关键设计
//! - 轻量扫描不递归 NormalGroup，避免遍历 vendor 等大目录
//! - MutexGroup 使用栈式 DFS 非递归遍历，避免栈溢出
//! - 排序规则：启用优先 > 收藏优先 > 最新收藏 > 自然排序
//! - 每个 NormalGroup 自动添加 "None" 空槽位（索引 0），表示不选任何模组

use anyhow::Result;
use std::cmp::Ordering as CmpOrdering;
use std::path::{Path, PathBuf};
use std::collections::HashSet;
use std::fs;
use std::sync::atomic::Ordering;
use std::time::Instant;
use regex::Regex;
use once_cell::sync::Lazy;
use crate::core::constants;
use crate::core::file_watcher::WATCHER_PAUSED;
use crate::core::ini_handler::IniFile;
use crate::core::namespace_handler;
use crate::models::enums::{GroupType, TargetGame, ModsPathStatus};
use crate::models::mod_data::{ModData, ModGroupData, ModIniData, ErroredLines};

/// 图标文件扩展名列表
static ICON_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico"];

/// group_xx 目录正则：严格匹配 group_1, group_12 等
/// 规则：^group_ 开头，后面跟 [1-9] 开头的数字（禁止前导零，禁止 group_0）
/// 设计原因：3Dmigoto 的槽位从 1 开始，group_0 无效，前导零会导致排序和解析混乱
static GROUP_N_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^group_([1-9][0-9]*)$").unwrap());

/// DISABLED 前缀正则（不区分大小写）
/// 匹配 DISABLED, disabled, Disabled 等，后面可以跟 _ - 空格或直接连接
/// 用于检测和移除目录名的禁用前缀
static DISABLED_PREFIX_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?i:disabled)[_\- ]*").unwrap());

/// 扫描结果结构体
///
/// 包含扫描得到的所有分组、模组列表和统计信息
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    /// 分组列表（NormalGroup + MutexGroup 根节点）
    pub groups: Vec<ModGroupData>,
    /// 所有模组的扁平列表
    pub mods: Vec<ModData>,
    /// 总模组数（不含 None 空槽位）
    pub total_mods_count: usize,
    /// 启用的模组数
    pub enabled_mods_count: usize,
    /// 禁用的模组数
    pub disabled_mods_count: usize,
}

/// 获取 _MANAGED_ 文件夹路径
pub fn get_managed_folder(game_mods_path: &Path) -> PathBuf {
    game_mods_path.join(constants::MANAGED_FOLDER)
}

/// 默认扫描函数：使用深度扫描（保持向后兼容，完整解析INI）
/// 注意：UI 初始化和需要完整 INI 数据的场景使用此函数
pub fn scan_mods(game_mods_path: &Path) -> Result<ScanResult> {
    scan_mods_deep(game_mods_path)
}

/// 检查模组路径状态
///
/// 依次检查：
/// 1. 路径是否存在
/// 2. _MANAGED_ 目录是否存在
/// 3. 主 INI 文件（d3dx.ini/RatioShot.ini）是否存在
///
/// # 返回
/// - Valid: 路径有效
/// - NotFound: mods 路径不存在
/// - ManagedFolderNotFound: _MANAGED_ 目录不存在
/// - D3dxIniNotFound: 主 INI 不存在
pub fn check_mods_path(game: TargetGame, mods_path: &Path) -> ModsPathStatus {
    if !mods_path.exists() {
        return ModsPathStatus::NotFound;
    }
    let managed = mods_path.join(constants::MANAGED_FOLDER);
    if !managed.exists() {
        return ModsPathStatus::ManagedFolderNotFound;
    }
    let d3dx_name = game.d3dx_ini_name();
    let d3dx_path = mods_path.join(d3dx_name);
    if !d3dx_path.exists() {
        return ModsPathStatus::D3dxIniNotFound;
    }
    ModsPathStatus::Valid
}

/// 判断目录名是否为普通分组目录（group_xx），返回解析后的 group_index
///
/// 使用 GROUP_N_RE 严格匹配，禁止 group_0 和前导零（如 group_01）
pub fn is_normal_group_dir(dir_name: &str) -> Option<u32> {
    let captures = GROUP_N_RE.captures(dir_name)?;
    let index_str = captures.get(1).unwrap().as_str();
    index_str.parse::<u32>().ok()
}

/// 检查目录是否包含任何 .ini 文件（仅检查扩展名，不读取内容）
pub fn dir_has_ini_file(dir_path: &Path) -> Result<bool> {
    let entries = match fs::read_dir(dir_path) {
        Ok(e) => e,
        Err(_) => return Ok(false),
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext.to_string_lossy().to_lowercase() == "ini" {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// 检查目录名是否有 DISABLED 前缀（不区分大小写）
pub fn is_disabled_dir(dir_name: &str) -> bool {
    DISABLED_PREFIX_RE.is_match(dir_name)
}

/// 读取或创建标记文件，不存在则用默认内容创建
/// 写文件前暂停文件监控，避免触发循环事件
pub fn read_or_create_marker_file(path: &Path, default_content: &str) -> Result<String> {
    if path.exists() {
        let content = fs::read_to_string(path)?;
        Ok(content.trim().to_string())
    } else {
        WATCHER_PAUSED.store(true, Ordering::SeqCst);
        let result = fs::write(path, default_content);
        WATCHER_PAUSED.store(false, Ordering::SeqCst);
        result?;
        Ok(default_content.to_string())
    }
}

/// 在目录中查找图标路径：优先 ICON_NAME_PRIORITY，否则取第一张非 DISABLED 前缀图片
pub fn find_icon_path(dir_path: &Path) -> Result<Option<PathBuf>> {
    let entries = match fs::read_dir(dir_path) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    let mut files: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                let ext_lower = ext.to_string_lossy().to_lowercase();
                if ICON_EXTENSIONS.contains(&ext_lower.as_str()) {
                    files.push(path);
                }
            }
        }
    }

    // 优先查找 ICON_NAME_PRIORITY 中的文件名
    for priority_name in constants::ICON_NAME_PRIORITY {
        for file in &files {
            if let Some(fname) = file.file_name() {
                if fname.to_string_lossy().to_lowercase() == *priority_name {
                    return Ok(Some(file.clone()));
                }
            }
        }
    }

    // 否则取第一张非 DISABLED 前缀的图片
    for file in files {
        if let Some(stem) = file.file_stem() {
            let stem_lower = stem.to_string_lossy().to_lowercase();
            if !stem_lower.starts_with("disable") {
                return Ok(Some(file));
            }
        }
    }

    Ok(None)
}

/// 自然排序比较函数（参考 Dart compareNatural），数字段智能比较
pub fn natural_compare(a: &str, b: &str) -> CmpOrdering {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    let a_chars: Vec<char> = a_lower.chars().collect();
    let b_chars: Vec<char> = b_lower.chars().collect();
    let mut i = 0;
    let mut j = 0;

    while i < a_chars.len() && j < b_chars.len() {
        let a_is_digit = a_chars[i].is_ascii_digit();
        let b_is_digit = b_chars[j].is_ascii_digit();

        if a_is_digit && b_is_digit {
            let mut a_num = 0u64;
            while i < a_chars.len() && a_chars[i].is_ascii_digit() {
                a_num = a_num * 10 + (a_chars[i] as u64 - '0' as u64);
                i += 1;
            }
            let mut b_num = 0u64;
            while j < b_chars.len() && b_chars[j].is_ascii_digit() {
                b_num = b_num * 10 + (b_chars[j] as u64 - '0' as u64);
                j += 1;
            }
            match a_num.cmp(&b_num) {
                CmpOrdering::Equal => continue,
                other => return other,
            }
        } else {
            match a_chars[i].cmp(&b_chars[j]) {
                CmpOrdering::Equal => {
                    i += 1;
                    j += 1;
                }
                other => return other,
            }
        }
    }

    a_chars.len().cmp(&b_chars.len())
}

/// 创建 None 空槽位模组
pub fn create_empty_slot_mod(group_index: u32) -> ModData {
    ModData {
        mod_path: "None".to_string(),
        mod_name: "None".to_string(),
        name: "None".to_string(),
        is_active: false,
        is_favorite: false,
        disabled: false,
        mod_disabled: false,
        group_index,
        mod_index: 0,
        is_mutex: false,
        ..Default::default()
    }
}

/// 统一排序模组列表：禁用在后→收藏在前→最新收藏→自然排序
pub fn sort_mods_light(mods: &mut [ModData]) {
    mods.sort_by(|a, b| {
        let a_enabled = !a.disabled && !a.mod_disabled;
        let b_enabled = !b.disabled && !b.mod_disabled;
        match b_enabled.cmp(&a_enabled) {
            CmpOrdering::Equal => {}
            other => return other,
        }

        match b.is_favorite.cmp(&a.is_favorite) {
            CmpOrdering::Equal => {}
            other => return other,
        }

        let a_fav_path = a.full_path.join(constants::FAV_MARKER);
        let b_fav_path = b.full_path.join(constants::FAV_MARKER);
        let a_time = a_fav_path.metadata().and_then(|m| m.modified()).ok();
        let b_time = b_fav_path.metadata().and_then(|m| m.modified()).ok();
        match (b_time, a_time) {
            (Some(bt), Some(at)) => match bt.cmp(&at) {
                CmpOrdering::Equal => {}
                other => return other,
            },
            (Some(_), None) => return CmpOrdering::Less,
            (None, Some(_)) => return CmpOrdering::Greater,
            (None, None) => {}
        }

        natural_compare(&a.name, &b.name)
    });
}

/// 轻量扫描：混合目录扫描，不解析 INI，不递归 NormalGroup
///
/// 扫描策略：
/// - NormalGroup（group_xx 一级子目录）：仅扫描一层，不递归，避免进入 vendor 等大目录
/// - MutexGroup（非 group_xx 子目录）：DFS 递归扫描，收集所有包含 INI 的目录
///
/// # 收集的信息
/// - 目录路径、名称、显示名称（去掉 DISABLED_ 前缀）
/// - enabled 状态、fav 收藏状态、fav_timestamp
/// - 图标路径（自动查找目录下的图片文件）
/// - ini_file_paths（仅路径，不解析内容）
///
/// 耗时：<100ms（取决于目录数量）
pub fn scan_mods_light(game_mods_path: &Path) -> Result<ScanResult> {
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
        });
    }

    let mut groups: Vec<ModGroupData> = Vec::new();
    let mut all_mods: Vec<ModData> = Vec::new();

    let entries = fs::read_dir(&managed_folder)?;
    let mut root_dirs: Vec<PathBuf> = Vec::new();

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
        root_dirs.push(path);
    }

    // 先处理 NormalGroup（group_xx）
    for dir_path in &root_dirs {
        let dir_name = dir_path.file_name().unwrap().to_string_lossy().to_string();
        if let Some(group_index) = is_normal_group_dir(&dir_name) {
            let (group, mods) = scan_normal_group_light(dir_path, &dir_name, group_index)?;
            groups.push(group);
            all_mods.extend(mods);
        }
    }

    // 再处理 MutexGroup（非 group_xx 目录），使用 DFS 栈非递归遍历
    let mutex_roots: Vec<PathBuf> = root_dirs.iter()
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            is_normal_group_dir(&name).is_none()
        })
        .cloned()
        .collect();

    for root_path in mutex_roots {
        let (group, mods) = scan_mutex_group_dfs(&root_path)?;
        if let Some(g) = group {
            groups.push(g);
            all_mods.extend(mods);
        }
    }

    // 按 group_index 排序
    groups.sort_by_key(|g| g.group_index());

    let enabled = all_mods.iter().filter(|m| !m.disabled && !m.mod_disabled && m.name != "None").count();
    let disabled = all_mods.iter().filter(|m| (m.disabled || m.mod_disabled) && m.name != "None").count();
    let total = all_mods.iter().filter(|m| m.name != "None").count();

    let elapsed = start.elapsed().as_millis();
    log::info!("Light scan completed in {}ms, {} mods, {} groups", elapsed, total, groups.len());

    Ok(ScanResult {
        groups,
        total_mods_count: total,
        enabled_mods_count: enabled,
        disabled_mods_count: disabled,
        mods: all_mods,
    })
}

/// 轻量扫描普通分组（group_xx）：仅一级子目录，不递归
fn scan_normal_group_light(dir_path: &Path, group_name: &str, group_index: u32) -> Result<(ModGroupData, Vec<ModData>)> {
    let mut mods: Vec<ModData> = Vec::new();

    // 读取/创建标记文件
    let groupname_path = dir_path.join("groupname");
    let _group_name = read_or_create_marker_file(&groupname_path, group_name)?;

    let selectedindex_path = dir_path.join(constants::SELECTED_INDEX_FILE);
    let selected_index_str = read_or_create_marker_file(&selectedindex_path, "0")?;
    let selected_index: i32 = selected_index_str.parse().unwrap_or(0);

    // 插入 None 空槽位（realIndex=0）
    mods.push(create_empty_slot_mod(group_index));

    // 仅读取一级子目录，绝对不递归
    let entries = fs::read_dir(dir_path)?;
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
        // 跳过标记文件目录（不会是目录，这里仅作保险）
        if dir_name == "groupname" || dir_name == constants::SELECTED_INDEX_FILE || dir_name == "modname" {
            continue;
        }

        let disabled = is_disabled_dir(&dir_name);
        let display_name = if disabled {
            DISABLED_PREFIX_RE.replace(&dir_name, "").to_string()
        } else {
            dir_name.clone()
        };

        // 读取/创建 modname 标记文件
        let modname_path = path.join("modname");
        let _mod_name = read_or_create_marker_file(&modname_path, &display_name)?;

        // 检查 fav 文件
        let fav_path = path.join(constants::FAV_MARKER);
        let is_favorite = fav_path.exists();

        // 检查各种标记文件
        let mod_forced = path.join(constants::MODFORCED_MARKER).exists();
        let _mod_syntax_error_removed = path.join("modsyntaxerrorremoved").exists();
        let _mod_unoptimized = path.join("modunoptimized").exists();
        let is_namespaced = path.join(constants::NAMESPACED_MARKER).exists();

        // 查找图标
        let icon_path = find_icon_path(&path)?;

        let real_index = mods.len() as u32;
        let is_active = real_index as i32 == selected_index;

        let mod_data = ModData {
            mod_path: path.to_string_lossy().to_string(),
            mod_name: display_name.clone(),
            name: display_name,
            full_path: path.clone(),
            parent_folder: dir_path.to_path_buf(),
            preview_image_path: icon_path,
            is_active,
            is_favorite,
            is_namespaced,
            disabled,
            mod_disabled: disabled,
            group_index,
            mod_index: real_index,
            is_mutex: false,
            has_nonmanaged_mods_crashline_fix: mod_forced,
            ..Default::default()
        };
        mods.push(mod_data);
    }

    // 分离 None 槽位和其他模组
    let mut none_slot: Option<ModData> = None;
    let mut other_mods: Vec<ModData> = Vec::new();
    for m in mods.drain(..) {
        if m.name == "None" {
            none_slot = Some(m);
        } else {
            other_mods.push(m);
        }
    }

    // 只对非 None 模组排序
    sort_mods_light(&mut other_mods);

    // 重新组装：None 始终在第一位
    mods.push(none_slot.unwrap_or_else(|| create_empty_slot_mod(group_index)));
    mods.extend(other_mods);

    // 重新分配 mod_index 并更新 is_active
    let mut active_mod_index: i32 = -1;
    for (idx, m) in mods.iter_mut().enumerate() {
        m.mod_index = idx as u32;
        if m.name == "None" {
            m.is_active = selected_index == 0;
            if selected_index == 0 {
                active_mod_index = 0;
            }
        } else {
            let real_idx_in_list = idx as i32;
            m.is_active = real_idx_in_list == selected_index;
            if real_idx_in_list == selected_index {
                active_mod_index = real_idx_in_list;
            }
        }
    }

    let mod_paths: Vec<PathBuf> = mods.iter()
        .filter(|m| m.name != "None")
        .map(|m| m.full_path.clone())
        .collect();

    let group = ModGroupData {
        name: group_name.to_string(),
        group_name: group_name.to_string(),
        group_type: GroupType::NormalGroup,
        full_path: dir_path.to_path_buf(),
        group_path: dir_path.to_string_lossy().to_string(),
        group_index,
        mods: mods.clone(),
        mod_count: (mods.len() - 1) as u32, // 减去 None 槽位
        mod_paths,
        has_child: false,
        children: vec![],
        child_groups: vec![],
        active_mod_index,
        is_favorite: false,
        group_disabled: false,
        is_active: active_mod_index >= 0,
        ..Default::default()
    };

    Ok((group, mods))
}

/// DFS 栈元素：待遍历目录 + 父分组引用（通过索引构建后关联）
struct DfsStackItem {
    path: PathBuf,
    parent_group_idx: Option<usize>,
}

/// 使用 Vec 栈非递归 DFS 遍历 MutexGroup
fn scan_mutex_group_dfs(root_path: &Path) -> Result<(Option<ModGroupData>, Vec<ModData>)> {
    let root_name = root_path.file_name().unwrap().to_string_lossy().to_string();
    let root_disabled = is_disabled_dir(&root_name);
    let root_display_name = if root_disabled {
        DISABLED_PREFIX_RE.replace(&root_name, "").to_string()
    } else {
        root_name.clone()
    };

    // 检查根目录是否本身就是模组（含 ini）
    if dir_has_ini_file(root_path)? {
        // 根目录是模组叶子节点，不创建分组
        let mod_data = build_mutex_mod_light(root_path, 0, 0)?;
        return Ok((None, vec![mod_data]));
    }

    let mut all_mods: Vec<ModData> = Vec::new();
    // groups[0] 是根分组，后续是子分组
    let mut groups: Vec<ModGroupData> = Vec::new();

    // 创建根分组
    let root_icon = find_icon_path(root_path)?;
    let root_group = ModGroupData {
        name: root_display_name.clone(),
        group_name: root_name.clone(),
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

    // DFS 栈初始化
    let mut stack: Vec<DfsStackItem> = Vec::new();

    // 列出根目录的一级子目录，push 到栈
    let root_entries = fs::read_dir(root_path)?;
    let mut root_subdirs: Vec<PathBuf> = Vec::new();
    for entry in root_entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let p = entry.path();
        if p.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') {
                root_subdirs.push(p);
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
            all_mods.push(mod_data);

            // 添加到父分组
            if let Some(pidx) = parent_idx {
                groups[pidx].mods.push(all_mods.last().unwrap().clone());
                groups[pidx].mod_paths.push(current_path.clone());
                groups[pidx].mod_count += 1;
            }
            global_mod_index += 1;
            // 叶子节点，停止向下遍历
            continue;
        }

        // 没有 ini，视为分组节点
        let dir_name = current_path.file_name().unwrap().to_string_lossy().to_string();
        let dir_disabled = is_disabled_dir(&dir_name);
        let dir_display_name = if dir_disabled {
            DISABLED_PREFIX_RE.replace(&dir_name, "").to_string()
        } else {
            dir_name.clone()
        };

        // 查找图标
        let icon_path = find_icon_path(&current_path)?;

        // 检查是否有子目录
        let sub_entries = fs::read_dir(&current_path)?;
        let mut subdirs: Vec<PathBuf> = Vec::new();
        for entry in sub_entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let p = entry.path();
            if p.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with('.') {
                    subdirs.push(p);
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

        // 注意：不在此关联 child_groups，后面通过 rebuild_tree 根据路径重建

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
        sort_mods_light(&mut group.mods);
        // 重新分配 mod_index
        for (idx, m) in group.mods.iter_mut().enumerate() {
            m.mod_index = idx as u32;
        }
    }

    // 更新根分组的 child_groups（因为后续分组修改了，需要重建）
    // 重新构建树形结构
    fn rebuild_tree(groups: &mut [ModGroupData], parent_idx: usize) {
        let child_indices: Vec<usize> = groups.iter().enumerate()
            .filter(|(i, g)| {
                *i != parent_idx && g.full_path.parent() == Some(&groups[parent_idx].full_path)
            })
            .map(|(i, _)| i)
            .collect();

        groups[parent_idx].child_groups.clear();
        for ci in child_indices {
            rebuild_tree(groups, ci);
            groups[parent_idx].child_groups.push(groups[ci].clone());
        }
        // 同步 children 字段（前端使用 camelCase 后的 children）
        groups[parent_idx].children = groups[parent_idx].child_groups.clone();
        groups[parent_idx].has_child = !groups[parent_idx].child_groups.is_empty() || !groups[parent_idx].mods.is_empty();
    }

    rebuild_tree(&mut groups, 0);

    // 收集根分组的所有 mods（包括子分组的）到 all_mods
    fn collect_mods(group: &ModGroupData, all: &mut Vec<ModData>) {
        all.extend(group.mods.iter().cloned());
        for child in &group.child_groups {
            collect_mods(child, all);
        }
    }
    all_mods.clear();
    collect_mods(&groups[0], &mut all_mods);

    Ok((Some(groups.remove(0)), all_mods))
}

/// 构建 MutexGroup 模组的轻量数据（不解析 INI）
fn build_mutex_mod_light(mod_path: &Path, group_index: u32, mod_index: u32) -> Result<ModData> {
    let dir_name = mod_path.file_name().unwrap().to_string_lossy().to_string();
    let disabled = is_disabled_dir(&dir_name);
    let display_name = if disabled {
        DISABLED_PREFIX_RE.replace(&dir_name, "").to_string()
    } else {
        dir_name.clone()
    };

    // 检查 fav 文件
    let fav_path = mod_path.join(constants::FAV_MARKER);
    let is_favorite = fav_path.exists();

    // 检查标记文件
    let mod_forced = mod_path.join(constants::MODFORCED_MARKER).exists();
    let is_namespaced = mod_path.join(constants::NAMESPACED_MARKER).exists();
    let _syntax_removed = mod_path.join("modsyntaxerrorremoved").exists();
    let _unoptimized = mod_path.join("modunoptimized").exists();

    // 查找图标
    let icon_path = find_icon_path(mod_path)?;

    Ok(ModData {
        mod_path: mod_path.to_string_lossy().to_string(),
        mod_name: display_name.clone(),
        name: display_name,
        full_path: mod_path.to_path_buf(),
        parent_folder: mod_path.parent().unwrap_or(mod_path).to_path_buf(),
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
        ..Default::default()
    })
}

// ============================================================================
// 以下是深度扫描（原 scan_mods，重命名为 scan_mods_deep），完整解析 INI，仅供 update_mod_data 使用
// ============================================================================

/// 深度扫描：完整解析 INI，递归扫描所有子目录（原 scan_mods 重命名）
///
/// 与轻量扫描的区别：
/// - 完整解析所有 INI 文件内容，统计各类型段数量（[TextureOverride], [ShaderOverride] 等）
/// - 提取错误行（crash_causing_lines）、未定义引用、namespace 变量
/// - 递归扫描所有子目录（包括 NormalGroup 内部）
/// - 收集已知库（defined_libraries）
///
/// # 使用场景
/// 仅在 update_mod_data（点击"应用"按钮）时调用，用于：
/// - 注入槽位条件到 INI
/// - 检测语法错误和未定义引用
/// - 展开 namespace 变量
///
/// 耗时：可能需要几百毫秒到几秒（取决于 INI 数量和大小）
pub fn scan_mods_deep(game_mods_path: &Path) -> Result<ScanResult> {
    let managed_folder = get_managed_folder(game_mods_path);
    if !managed_folder.exists() {
        fs::create_dir_all(&managed_folder)?;
        return Ok(ScanResult {
            groups: vec![],
            mods: vec![],
            total_mods_count: 0,
            enabled_mods_count: 0,
            disabled_mods_count: 0,
        });
    }

    let mut groups: Vec<ModGroupData> = Vec::new();
    let mut all_mods: Vec<ModData> = Vec::new();
    let mut known_libraries = HashSet::new();

    let entries = fs::read_dir(&managed_folder)?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();

        // 使用新正则过滤，仅处理 group_xx 目录
        if is_normal_group_dir(&dir_name).is_none() {
            continue;
        }

        let (group, mods) = scan_group_directory_deep(&path, &dir_name, GroupType::NormalGroup)?;

        for mod_data in &mods {
            for ini_data in &mod_data.mod_ini_data {
                if let Ok(ini) = IniFile::parse(&PathBuf::from(&ini_data.ini_path)) {
                    let libs = ini.defined_libraries();
                    known_libraries.extend(libs);
                }
            }
        }

        groups.push(group);
        all_mods.extend(mods);
    }

    groups.sort_by_key(|g| g.group_index());

    let enabled = all_mods.iter().filter(|m| !m.disabled && !m.mod_disabled).count();
    let disabled = all_mods.iter().filter(|m| m.disabled || m.mod_disabled).count();

    Ok(ScanResult {
        groups,
        total_mods_count: all_mods.len(),
        enabled_mods_count: enabled,
        disabled_mods_count: disabled,
        mods: all_mods,
    })
}

/// 深度扫描分组目录：BFS 递归查找模组
fn scan_group_directory_deep(dir_path: &Path, group_name: &str, group_type: GroupType) -> Result<(ModGroupData, Vec<ModData>)> {
    use std::collections::VecDeque;

    let mut mods = Vec::new();
    let mut subgroups: Vec<ModGroupData> = Vec::new();
    let mut subgroup_paths: HashSet<PathBuf> = HashSet::new();

    let mut queue = VecDeque::new();
    queue.push_back(dir_path.to_path_buf());

    let mut visited_dirs = HashSet::new();
    visited_dirs.insert(dir_path.to_path_buf());

    while let Some(current_path) = queue.pop_front() {
        let (has_ini, has_icon, icon_path, ini_files) = check_directory_for_mod_deep(&current_path)?;

        if has_ini || has_icon {
            let parent_groups: Vec<String> = Vec::new();
            let mod_data = build_mod_data_deep(&current_path, group_name, &parent_groups, ini_files, icon_path)?;
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
                let sub_path = sub_entry.path();
                if sub_path.is_dir() {
                    let sub_name = sub_entry.file_name().to_string_lossy().to_string();
                    if sub_name.starts_with('.') {
                        continue;
                    }
                    has_subdirs = true;
                    if !visited_dirs.contains(&sub_path) {
                        visited_dirs.insert(sub_path.clone());
                        queue.push_back(sub_path);
                    }
                }
            }

            if current_path != dir_path && has_subdirs {
                if !subgroup_paths.contains(&current_path) {
                    subgroup_paths.insert(current_path.clone());
                    let subgroup_name = current_path.file_name()
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
    }

    let group_index: u32 = group_name
        .trim_start_matches("group_")
        .parse()
        .unwrap_or(0);

    let mod_paths: Vec<PathBuf> = mods.iter().map(|m| m.full_path.clone()).collect();

    let has_children = !mods.is_empty() || !subgroups.is_empty();
    let group = ModGroupData {
        name: group_name.to_string(),
        group_name: group_name.to_string(),
        group_type,
        full_path: dir_path.to_path_buf(),
        group_path: dir_path.to_string_lossy().to_string(),
        group_index,
        child_groups: subgroups.clone(),
        children: subgroups,
        has_child: has_children,
        mod_count: mods.len() as u32,
        mod_paths: mod_paths.clone(),
        mods: mods.clone(),
        ..Default::default()
    };

    for (idx, mod_data) in mods.iter_mut().enumerate() {
        mod_data.group_index = group_index;
        mod_data.mod_index = idx as u32;
        mod_data.mod_path = mod_data.full_path.to_string_lossy().to_string();
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

    Ok((group, mods))
}

/// 深度扫描：检查目录是否为模组目录
fn check_directory_for_mod_deep(dir: &Path) -> Result<(bool, bool, Option<PathBuf>, Vec<PathBuf>)> {
    let mut has_ini = false;
    let mut has_icon = false;
    let mut icon_path: Option<PathBuf> = None;
    let mut ini_files: Vec<PathBuf> = Vec::new();
    let mut first_image: Option<PathBuf> = None;

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok((false, false, None, vec![])),
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                let ext_lower = ext.to_string_lossy().to_lowercase();
                if ext_lower == "ini" {
                    has_ini = true;
                    ini_files.push(path);
                } else if ICON_EXTENSIONS.contains(&ext_lower.as_str()) {
                    has_icon = true;
                    let stem = path.file_stem()
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

    ini_files.sort();
    Ok((has_ini, has_icon, icon_path, ini_files))
}

/// 深度扫描：构建完整 ModData（解析 INI）
fn build_mod_data_deep(
    dir: &Path,
    _group_name: &str,
    _parent_groups: &[String],
    ini_files: Vec<PathBuf>,
    icon_path: Option<PathBuf>,
) -> Result<ModData> {
    let dir_name = dir.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let disabled = dir_name.to_uppercase().starts_with("DISABLED");

    let mod_name = if disabled {
        DISABLED_PREFIX_RE.replace(&dir_name, "").to_string()
    } else {
        dir_name.clone()
    };

    let mut mod_ini_data: Vec<ModIniData> = Vec::new();
    let mut all_errored_lines: Vec<ErroredLines> = Vec::new();
    let mut total_sections = 0;
    let mut total_key_sections = 0;
    let mut total_texture_sections = 0;
    let mut total_shader_sections = 0;
    let mut total_command_lists = 0;
    let mut total_resources = 0;
    let mut mod_namespace: Option<String> = None;
    let mut defined_libraries = HashSet::new();

    for ini_path in &ini_files {
        match IniFile::parse(ini_path) {
            Ok(ini) => {
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
                    mod_namespace = namespace_handler::extract_namespace(&ini);
                }

                defined_libraries.extend(ini.defined_libraries());

                let known_libs = HashSet::new();
                let errors = ini.detect_errors(ini_path, &known_libs);
                all_errored_lines.extend(errors);

                let ini_filename = ini_path.file_name()
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

    let is_namespaced = mod_namespace.is_some();
    let namespaces_vec = mod_namespace.as_ref().map(|ns| vec![ns.clone()]).unwrap_or_default();

    Ok(ModData {
        name: mod_name.clone(),
        mod_name,
        full_path: dir.to_path_buf(),
        mod_path: dir.to_string_lossy().to_string(),
        parent_folder: dir.parent().unwrap_or(dir).to_path_buf(),
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

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        dir
    }

    fn create_group_dir(base: &Path, group_name: &str) -> PathBuf {
        let group_path = base.join("_MANAGED_").join(group_name);
        fs::create_dir_all(&group_path).unwrap();
        group_path
    }

    fn create_mod_with_ini(group_path: &Path, mod_name: &str, ini_content: &str) -> PathBuf {
        let mod_path = group_path.join(mod_name);
        fs::create_dir_all(&mod_path).unwrap();
        let ini_path = mod_path.join("mod.ini");
        fs::write(&ini_path, ini_content).unwrap();
        mod_path
    }

    fn create_d3dx_ini(base: &Path) {
        fs::write(base.join("d3dx.ini"), "; test").unwrap();
    }

    // ========== 深度扫描测试（原 scan_mods 改名为 scan_mods_deep） ==========

    #[test]
    fn test_scan_empty_managed_folder() {
        let dir = setup_test_dir();
        let result = scan_mods_deep(dir.path()).unwrap();
        assert!(result.groups.is_empty());
        assert!(result.mods.is_empty());
        assert_eq!(result.total_mods_count, 0);
    }

    #[test]
    fn test_scan_single_mod() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&group_path, "TestMod", "[TextureOverrideTest]\nhash = 0x123\n");

        let result = scan_mods_deep(dir.path()).unwrap();
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.mods.len(), 1);
        assert_eq!(result.total_mods_count, 1);
        assert_eq!(result.mods[0].name, "TestMod");
        assert!(!result.mods[0].disabled);
    }

    #[test]
    fn test_scan_disabled_mod() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&group_path, "DISABLED_MyMod", "[TextureOverrideTest]\nhash = 0x456\n");

        let result = scan_mods_deep(dir.path()).unwrap();
        assert_eq!(result.mods.len(), 1);
        assert!(result.mods[0].disabled);
        assert!(result.mods[0].mod_disabled);
        assert!(result.mods[0].name.contains("MyMod"));
        assert_eq!(result.disabled_mods_count, 1);
        assert_eq!(result.enabled_mods_count, 0);
    }

    #[test]
    fn test_scan_ignores_non_group_dirs() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let mutex_path = dir.path().join("_MANAGED_").join("#MutexMods");
        fs::create_dir_all(&mutex_path).unwrap();
        create_mod_with_ini(&mutex_path, "MutexMod", "[TextureOverrideTest]\nhash = 0x789\n");

        let other_path = dir.path().join("_MANAGED_").join("OtherFolder");
        fs::create_dir_all(&other_path).unwrap();
        create_mod_with_ini(&other_path, "OtherMod", "[TextureOverrideTest]\nhash = 0xABC\n");

        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&group_path, "ValidMod", "[TextureOverrideTest]\nhash = 0xDEF\n");

        let result = scan_mods_deep(dir.path()).unwrap();
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.mods.len(), 1);
        assert_eq!(result.mods[0].name, "ValidMod");
    }

    #[test]
    fn test_scan_nested_mods_deep() {
        // 深度扫描下应该能找到 NestedMod
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let subdir = group_path.join("SubCategory");
        fs::create_dir_all(&subdir).unwrap();
        let mod_path = subdir.join("NestedMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[KeyTest]\nkey = VkA\n").unwrap();

        let result = scan_mods_deep(dir.path()).unwrap();
        assert_eq!(result.mods.len(), 1);
        assert_eq!(result.mods[0].name, "NestedMod");
    }

    #[test]
    fn test_scan_mod_with_icon() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let mod_path = group_path.join("IconMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[Constants]\nx=1\n").unwrap();
        fs::write(mod_path.join("icon.png"), b"fake png").unwrap();

        let result = scan_mods_deep(dir.path()).unwrap();
        assert_eq!(result.mods.len(), 1);
        assert!(result.mods[0].preview_image_path.is_some());
        assert!(result.mods[0].preview_image_path.as_ref().unwrap().ends_with("icon.png"));
    }

    #[test]
    fn test_check_mods_path_valid() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let status = check_mods_path(TargetGame::GenshinImpact, dir.path());
        assert_eq!(status, ModsPathStatus::Valid);
    }

    #[test]
    fn test_check_mods_path_missing_managed() {
        let dir = TempDir::new().unwrap();
        create_d3dx_ini(dir.path());
        let status = check_mods_path(TargetGame::GenshinImpact, dir.path());
        assert_eq!(status, ModsPathStatus::ManagedFolderNotFound);
    }

    #[test]
    fn test_check_mods_path_not_found() {
        let dir = TempDir::new().unwrap();
        let nonexistent = dir.path().join("nonexistent");
        let status = check_mods_path(TargetGame::GenshinImpact, &nonexistent);
        assert_eq!(status, ModsPathStatus::NotFound);
    }

    #[test]
    fn test_check_mods_path_missing_d3dx() {
        let dir = setup_test_dir();
        let status = check_mods_path(TargetGame::GenshinImpact, dir.path());
        assert_eq!(status, ModsPathStatus::D3dxIniNotFound);
    }

    #[test]
    fn test_bfs_stops_at_mod_directory() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let mod_path = group_path.join("StoppingMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[TextureOverrideA]\nhash=1\n").unwrap();

        let subdir_under_mod = mod_path.join("ShouldNotBeScanned");
        fs::create_dir_all(&subdir_under_mod).unwrap();
        fs::write(subdir_under_mod.join("ignored.ini"), "[TextureOverrideB]\nhash=2\n").unwrap();

        let result = scan_mods_deep(dir.path()).unwrap();
        assert_eq!(result.mods.len(), 1);
        assert_eq!(result.mods[0].name, "StoppingMod");
    }

    #[test]
    fn test_multiple_groups() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");

        let g2 = create_group_dir(dir.path(), "group_2");
        create_mod_with_ini(&g2, "Mod2", "[TextureOverride2]\nhash=2\n");

        let g10 = create_group_dir(dir.path(), "group_10");
        create_mod_with_ini(&g10, "Mod10", "[TextureOverride10]\nhash=10\n");

        let result = scan_mods_deep(dir.path()).unwrap();
        assert_eq!(result.groups.len(), 3);
        assert_eq!(result.mods.len(), 3);
        assert_eq!(result.groups[0].group_name, "group_1");
        assert_eq!(result.groups[1].group_name, "group_2");
        assert_eq!(result.groups[2].group_name, "group_10");
    }

    #[test]
    fn test_ini_section_counting() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let ini_content = r#"
[Constants]
x = 1

[KeyTest]
key = VkA

[KeyPressB]
key = VkB

[TextureOverrideTex1]
hash = 0x1

[TextureOverrideTex2]
hash = 0x2

[ShaderOverrideS1]
hash = 0x3

[CommandListCL1]
x = 1

[ResourceR1]
filename = test.dds
"#;
        create_mod_with_ini(&group_path, "CountingMod", ini_content);

        let result = scan_mods_deep(dir.path()).unwrap();
        let m = &result.mods[0];
        assert_eq!(m.key_sections, 2);
        assert_eq!(m.texture_override_sections, 2);
        assert_eq!(m.shader_override_sections, 1);
        assert_eq!(m.command_list_sections, 1);
        assert_eq!(m.resource_sections, 1);
        assert_eq!(m.total_section_count, 8);
    }

    #[test]
    fn test_has_include_detection() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let ini_content = "include = common.ini\n[Section]\nx=1\n";
        create_mod_with_ini(&group_path, "IncludeMod", ini_content);

        let result = scan_mods_deep(dir.path()).unwrap();
        let ini_data = &result.mods[0].mod_ini_data[0];
        assert!(ini_data.has_include);
    }

    #[test]
    fn test_namespace_extraction() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let ini_content = "namespace = MyTestMod\n[TextureOverrideT]\nhash=1\n";
        create_mod_with_ini(&group_path, "NsMod", ini_content);

        let result = scan_mods_deep(dir.path()).unwrap();
        assert!(result.mods[0].is_namespaced);
        assert_eq!(result.mods[0].namespace, Some("MyTestMod".to_string()));
    }

    // ========== 轻量扫描测试（scan_mods_light） ==========

    #[test]
    fn test_is_normal_group_dir() {
        // 正确匹配
        assert_eq!(is_normal_group_dir("group_1"), Some(1));
        assert_eq!(is_normal_group_dir("group_12"), Some(12));
        assert_eq!(is_normal_group_dir("group_123"), Some(123));
        assert_eq!(is_normal_group_dir("group_999"), Some(999));

        // 拒绝前导零
        assert_eq!(is_normal_group_dir("group_0"), None);
        assert_eq!(is_normal_group_dir("group_01"), None);
        assert_eq!(is_normal_group_dir("group_012"), None);
        assert_eq!(is_normal_group_dir("group_00"), None);

        // 拒绝非数字
        assert_eq!(is_normal_group_dir("group_abc"), None);
        assert_eq!(is_normal_group_dir("group_"), None);
        assert_eq!(is_normal_group_dir("group"), None);
        assert_eq!(is_normal_group_dir("#MutexMods"), None);
        assert_eq!(is_normal_group_dir("OtherFolder"), None);
    }

    #[test]
    fn test_natural_compare() {
        assert_eq!(natural_compare("mod2", "mod10"), CmpOrdering::Less);
        assert_eq!(natural_compare("mod10", "mod2"), CmpOrdering::Greater);
        assert_eq!(natural_compare("mod2", "mod2"), CmpOrdering::Equal);
        assert_eq!(natural_compare("a1b2", "a1b10"), CmpOrdering::Less);
        assert_eq!(natural_compare("ModA", "moda"), CmpOrdering::Equal);
        assert_eq!(natural_compare("mod1", "mod2"), CmpOrdering::Less);
        assert_eq!(natural_compare("mod10", "mod11"), CmpOrdering::Less);
        assert_eq!(natural_compare("abc", "def"), CmpOrdering::Less);
    }

    #[test]
    fn test_is_disabled_dir() {
        assert!(is_disabled_dir("DISABLED_Mod"));
        assert!(is_disabled_dir("disabled_mod"));
        assert!(is_disabled_dir("Disabled-Mod"));
        assert!(is_disabled_dir("DISABLED Mod"));
        assert!(!is_disabled_dir("NormalMod"));
        assert!(!is_disabled_dir("mod_disabled"));
    }

    #[test]
    fn test_dir_has_ini_file() {
        let dir = TempDir::new().unwrap();
        assert!(!dir_has_ini_file(dir.path()).unwrap());

        let ini_path = dir.path().join("test.ini");
        fs::write(&ini_path, "").unwrap();
        assert!(dir_has_ini_file(dir.path()).unwrap());

        let txt_path = dir.path().join("test.txt");
        fs::write(&txt_path, "").unwrap();
        assert!(dir_has_ini_file(dir.path()).unwrap());
    }

    #[test]
    fn test_light_scan_none_slot() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&group_path, "TestMod", "[Section]\n");

        let result = scan_mods_light(dir.path()).unwrap();
        assert_eq!(result.groups.len(), 1);
        // None + TestMod
        let group = &result.groups[0];
        assert!(group.mods.iter().any(|m| m.name == "None"));
        assert!(group.mods.iter().any(|m| m.name == "TestMod"));
    }

    #[test]
    fn test_light_scan_no_recursion_normal_group() {
        // 轻量扫描下 NormalGroup 绝对不递归：SubDir 是一级子目录会被扫描为模组，
        // 但 SubDir/NestedMod 是二级子目录，不会被扫描到
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let subdir = group_path.join("SubDir");
        fs::create_dir_all(&subdir).unwrap();
        let nested_mod = subdir.join("NestedMod");
        fs::create_dir_all(&nested_mod).unwrap();
        fs::write(nested_mod.join("mod.ini"), "[Section]\n").unwrap();

        let result = scan_mods_light(dir.path()).unwrap();
        let group = &result.groups[0];
        // NestedMod 在子目录中，不应该被扫描到
        assert!(!group.mods.iter().any(|m| m.name == "NestedMod"));
        // SubDir 是一级子目录，会被扫描为模组；加上 None 槽位，共 2 个
        assert!(group.mods.iter().any(|m| m.name == "SubDir"));
        assert_eq!(group.mods.len(), 2); // None + SubDir
    }

    #[test]
    fn test_light_scan_creates_marker_files() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");
        let mod_path = group_path.join("TestMod");
        fs::create_dir_all(&mod_path).unwrap();

        // 标记文件不存在
        assert!(!group_path.join("groupname").exists());
        assert!(!group_path.join("selectedindex").exists());
        assert!(!mod_path.join("modname").exists());

        let _result = scan_mods_light(dir.path()).unwrap();

        // 标记文件应该被创建
        assert!(group_path.join("groupname").exists());
        assert!(group_path.join("selectedindex").exists());
        assert!(mod_path.join("modname").exists());
    }

    #[test]
    fn test_light_scan_mutex_group_no_markers() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let mutex_path = dir.path().join("_MANAGED_").join("#MutexMods");
        fs::create_dir_all(&mutex_path).unwrap();
        let mod_path = mutex_path.join("MutexMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[Section]\n").unwrap();

        let _result = scan_mods_light(dir.path()).unwrap();

        // MutexGroup 不应该创建 groupname/selectedindex/modname
        assert!(!mutex_path.join("groupname").exists());
        assert!(!mutex_path.join("selectedindex").exists());
        assert!(!mod_path.join("modname").exists());
    }

    #[test]
    fn test_light_scan_mutex_group_dfs() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // 创建嵌套 MutexGroup 结构：
        // #MutexRoot/
        //   Category1/
        //     ModA/ (有 ini)
        //     SubCategory/
        //       ModB/ (有 ini)
        //   ModC/ (有 ini)
        let root = dir.path().join("_MANAGED_").join("#MutexRoot");
        fs::create_dir_all(&root).unwrap();

        let cat1 = root.join("Category1");
        fs::create_dir_all(&cat1).unwrap();
        let mod_a = cat1.join("ModA");
        fs::create_dir_all(&mod_a).unwrap();
        fs::write(mod_a.join("mod.ini"), "[A]\n").unwrap();

        let sub_cat = cat1.join("SubCategory");
        fs::create_dir_all(&sub_cat).unwrap();
        let mod_b = sub_cat.join("ModB");
        fs::create_dir_all(&mod_b).unwrap();
        fs::write(mod_b.join("mod.ini"), "[B]\n").unwrap();

        let mod_c = root.join("ModC");
        fs::create_dir_all(&mod_c).unwrap();
        fs::write(mod_c.join("mod.ini"), "[C]\n").unwrap();

        let result = scan_mods_light(dir.path()).unwrap();

        // 应该有一个分组（#MutexRoot）
        assert_eq!(result.groups.len(), 1);
        let root_group = &result.groups[0];
        assert_eq!(root_group.group_type, GroupType::MutexGroup);

        // 检查 mods 是否包含所有模组（通过 all_mods 收集）
        let mod_names: Vec<&str> = result.mods.iter().map(|m| m.name.as_str()).collect();
        assert!(mod_names.contains(&"ModA"), "Should find ModA");
        assert!(mod_names.contains(&"ModB"), "Should find ModB");
        assert!(mod_names.contains(&"ModC"), "Should find ModC");

        // 检查 is_mutex 标记
        for m in &result.mods {
            assert!(m.is_mutex, "Mutex mod should have is_mutex=true");
        }
    }

    #[test]
    fn test_light_scan_stops_at_ini_dir() {
        // 有 .ini 的目录不遍历其子目录
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let root = dir.path().join("_MANAGED_").join("#MutexRoot");
        fs::create_dir_all(&root).unwrap();

        let mod_dir = root.join("ModWithIni");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("mod.ini"), "[Section]\n").unwrap();

        // 模组目录下有子目录，应该不被扫描
        let sub_under_mod = mod_dir.join("SubModUnder");
        fs::create_dir_all(&sub_under_mod).unwrap();
        fs::write(sub_under_mod.join("another.ini"), "[Another]\n").unwrap();

        let result = scan_mods_light(dir.path()).unwrap();
        let mod_names: Vec<&str> = result.mods.iter().map(|m| m.name.as_str()).collect();
        assert!(mod_names.contains(&"ModWithIni"));
        assert!(!mod_names.contains(&"SubModUnder"), "Should not recurse into mod dir with ini");
    }

    #[test]
    fn test_light_scan_disabled_both_types() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // NormalGroup 禁用模组
        let group_path = create_group_dir(dir.path(), "group_1");
        let mod_enabled = group_path.join("EnabledMod");
        fs::create_dir_all(&mod_enabled).unwrap();
        let mod_disabled = group_path.join("DISABLED_DisabledMod");
        fs::create_dir_all(&mod_disabled).unwrap();

        // MutexGroup 禁用模组
        let mutex_path = dir.path().join("_MANAGED_").join("#Mutex");
        fs::create_dir_all(&mutex_path).unwrap();
        let m_enabled = mutex_path.join("MutexEnabled");
        fs::create_dir_all(&m_enabled).unwrap();
        fs::write(m_enabled.join("m.ini"), "").unwrap();
        let m_disabled = mutex_path.join("disabled_MutexDisabled");
        fs::create_dir_all(&m_disabled).unwrap();
        fs::write(m_disabled.join("m.ini"), "").unwrap();

        let result = scan_mods_light(dir.path()).unwrap();

        for m in &result.mods {
            if m.name.contains("Disabled") {
                assert!(m.disabled || m.name == "None", "{} should be disabled", m.name);
            } else if m.name != "None" {
                assert!(!m.disabled, "{} should be enabled", m.name);
            }
        }
    }

    #[test]
    fn test_light_scan_fav_both_types() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // NormalGroup fav
        let group_path = create_group_dir(dir.path(), "group_1");
        let mod_fav = group_path.join("FavMod");
        fs::create_dir_all(&mod_fav).unwrap();
        fs::write(mod_fav.join("fav"), "").unwrap();
        let mod_normal = group_path.join("NormalMod");
        fs::create_dir_all(&mod_normal).unwrap();

        // MutexGroup fav
        let mutex_path = dir.path().join("_MANAGED_").join("#Mutex");
        fs::create_dir_all(&mutex_path).unwrap();
        let m_fav = mutex_path.join("MutexFav");
        fs::create_dir_all(&m_fav).unwrap();
        fs::write(m_fav.join("m.ini"), "").unwrap();
        fs::write(m_fav.join("fav"), "").unwrap();
        let m_normal = mutex_path.join("MutexNormal");
        fs::create_dir_all(&m_normal).unwrap();
        fs::write(m_normal.join("m.ini"), "").unwrap();

        let result = scan_mods_light(dir.path()).unwrap();

        for m in &result.mods {
            if m.name.contains("Fav") {
                assert!(m.is_favorite, "{} should be favorite", m.name);
            } else if m.name != "None" {
                assert!(!m.is_favorite, "{} should not be favorite", m.name);
            }
        }
    }

    #[test]
    fn test_light_scan_icon_priority() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let mod_path = group_path.join("IconTest");
        fs::create_dir_all(&mod_path).unwrap();
        // 有 preview.png 和 other.png，但 icon.png 优先
        fs::write(mod_path.join("preview.png"), b"p").unwrap();
        fs::write(mod_path.join("other.png"), b"o").unwrap();
        fs::write(mod_path.join("icon.png"), b"i").unwrap();

        let result = scan_mods_light(dir.path()).unwrap();
        let m = result.mods.iter().find(|m| m.name == "IconTest").unwrap();
        assert!(m.preview_image_path.is_some());
        assert!(m.preview_image_path.as_ref().unwrap().ends_with("icon.png"));
    }

    #[test]
    fn test_light_scan_mixed_groups() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // NormalGroup
        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "NormalMod", "[S]\n");

        // MutexGroup
        let mutex_path = dir.path().join("_MANAGED_").join("#MutexMods");
        fs::create_dir_all(&mutex_path).unwrap();
        create_mod_with_ini(&mutex_path, "MutexMod", "[S]\n");

        let result = scan_mods_light(dir.path()).unwrap();

        let group_types: Vec<GroupType> = result.groups.iter().map(|g| g.group_type).collect();
        assert!(group_types.contains(&GroupType::NormalGroup));
        assert!(group_types.contains(&GroupType::MutexGroup));
        assert_eq!(result.groups.len(), 2);
    }

    #[test]
    fn test_light_scan_group_regex_strict() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // group_0 应该被拒绝（零值）
        let g0 = dir.path().join("_MANAGED_").join("group_0");
        fs::create_dir_all(&g0).unwrap();
        create_mod_with_ini(&g0, "Mod0", "[S]\n");

        // group_01 应该被拒绝（前导零）
        let g01 = dir.path().join("_MANAGED_").join("group_01");
        fs::create_dir_all(&g01).unwrap();
        create_mod_with_ini(&g01, "Mod01", "[S]\n");

        // group_1 应该被接受
        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "Mod1", "[S]\n");

        let result = scan_mods_light(dir.path()).unwrap();
        let group_names: Vec<&str> = result.groups.iter()
            .filter(|g| g.group_type == GroupType::NormalGroup)
            .map(|g| g.group_name.as_str())
            .collect();

        assert!(group_names.contains(&"group_1"), "group_1 should be NormalGroup");
        // group_0 和 group_01 应该作为 MutexGroup 存在
        let mutex_names: Vec<&str> = result.groups.iter()
            .filter(|g| g.group_type == GroupType::MutexGroup)
            .map(|g| g.group_name.as_str())
            .collect();
        assert!(mutex_names.contains(&"group_0"), "group_0 should be MutexGroup");
        assert!(mutex_names.contains(&"group_01"), "group_01 should be MutexGroup");
    }

    #[test]
    fn test_light_scan_sorting() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        // 创建 mod10, mod2, disabled_mod, fav_mod
        let m10 = group_path.join("mod10");
        fs::create_dir_all(&m10).unwrap();
        let m2 = group_path.join("mod2");
        fs::create_dir_all(&m2).unwrap();
        let m_disabled = group_path.join("DISABLED_bad");
        fs::create_dir_all(&m_disabled).unwrap();
        let m_fav = group_path.join("fav_mod");
        fs::create_dir_all(&m_fav).unwrap();
        fs::write(m_fav.join("fav"), "").unwrap();

        let result = scan_mods_light(dir.path()).unwrap();
        let group = &result.groups[0];

        // 第一个应该是 None（固定位置，不参与排序）
        assert_eq!(group.mods[0].name, "None");
        // 然后是 fav_mod（收藏优先）
        assert_eq!(group.mods[1].name, "fav_mod");
        // 然后是 mod2（自然排序 mod2 < mod10）
        assert_eq!(group.mods[2].name, "mod2");
        assert_eq!(group.mods[3].name, "mod10");
        // 禁用的在最后（bad 是 DISABLED_bad 去掉前缀后的名字）
        assert_eq!(group.mods[4].name, "bad");
        assert_eq!(group.mods.len(), 5);
    }
}
