//! 模组扫描模块
//!
//! 提供两种扫描模式：
//! - **轻量扫描 (scan_mods_light)**: 不解析 INI 内容，仅检查目录结构和标记文件，速度快，用于 UI 列表展示
//! - **深度扫描 (scan_mods_deep)**: 完整解析所有 INI 文件，统计段数量、检测错误、提取 namespace，用于 apply 时的 INI 注入
//!
//! # 分组类型
//! - **NormalGroup (group_xx)**: 普通分组，一级子目录为模组，不递归。每组同一时间只能启用一个模组（互斥槽位）
//! - **MutexGroup (非 group_xx 目录)**: 互斥组，支持任意深度嵌套，同级目录下的模组互斥
//!
//! # 关键设计
//! - **栈式非递归 DFS**: MutexGroup 使用 `Vec<DfsStackItem>` 模拟栈进行非递归深度优先遍历，避免递归调用栈溢出
//! - **rayon 并行遍历**: NormalGroup 和 MutexGroup 根目录使用 `par_iter()` 并行扫描，充分利用多核 CPU
//! - **HashSet 规范化路径去重**: 使用 `Path::canonicalize()` 获取绝对路径后存入 `HashSet`，防止符号链接/硬链接导致无限循环或重复扫描
//! - **`entry.file_type()` 替代 `metadata()`**: 遍历目录时使用 `DirEntry::file_type()` 而非 `fs::metadata()`，减少系统调用次数，提高性能
//! - 轻量扫描不递归 NormalGroup，避免遍历 vendor 等大目录
//! - 排序规则：启用优先 > 收藏优先 > 最新收藏 > 自然排序
//! - 每个 NormalGroup 自动添加 "None" 空槽位（索引 0），表示不选任何模组

use anyhow::Result;
use rayon::prelude::*;
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

/// 支持的图标文件扩展名列表
///
/// 用于 `find_icon_path()` 和 `check_directory_for_mod_deep()` 中过滤图片文件。
/// 涵盖常见 Web 和桌面图片格式：PNG、JPEG、GIF、BMP、WebP、ICO。
/// 匹配时不区分大小写（统一转小写后比较）。
static ICON_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico"];

/// group_xx 目录正则：严格匹配 `group_1`, `group_12` 等
///
/// # 匹配规则
/// - `^group_` 开头，后面跟 `[1-9][0-9]*`（首位非零的数字）
/// - 禁止前导零（如 `group_01`）
/// - 禁止 `group_0`（零值）
///
/// # 设计原因
/// 3Dmigoto 的槽位索引从 1 开始，`group_0` 无效。
/// 前导零（如 `group_01` vs `group_1`）会导致字符串排序与数值排序不一致，引发解析混乱。
/// 此正则确保了 `group_index` 的解析既严格又安全。
// SAFETY: Hardcoded valid regex literal; compilation cannot fail at runtime.
static GROUP_N_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^group_([1-9][0-9]*)$").unwrap());

/// DISABLED 前缀正则（不区分大小写）
///
/// # 匹配规则
/// - `^(?i:disabled)` 不区分大小写匹配 `disabled` 前缀
/// - `[_\- ]*` 后面可跟下划线、连字符、空格或直接连接
///
/// 匹配示例：`DISABLED_Mod`、`disabled_mod`、`Disabled-Mod`、`DISABLED Mod`、`DISABLEDMod`
///
/// # 用途
/// 用于 `is_disabled_dir()` 检测目录是否被禁用，以及 `DISABLED_PREFIX_RE.replace()` 移除前缀获取显示名称。
/// 设计为不区分大小写以兼容不同用户命名习惯，同时允许灵活的分隔符以保持可读性。
// SAFETY: Hardcoded valid regex literal; compilation cannot fail at runtime.
static DISABLED_PREFIX_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?i:disabled)[_\- ]*").unwrap());

/// 扫描结果结构体
///
/// 包含扫描得到的所有分组、模组列表和统计信息，是 `scan_mods_light()` 和 `scan_mods_deep()` 的返回值。
///
/// # 字段说明
/// - `groups`: 分组列表，包含 NormalGroup 和 MutexGroup 根节点，按 `group_index` 排序
/// - `mods`: 所有模组的扁平列表（不含 None 空槽位），便于前端直接渲染
/// - `total_mods_count`: 总模组数（不含 None 空槽位），用于统计展示
/// - `enabled_mods_count`: 启用的模组数，即 `!disabled && !mod_disabled` 的模组数量
/// - `disabled_mods_count`: 禁用的模组数，即 `disabled || mod_disabled` 的模组数量
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    /// 分组列表（NormalGroup + MutexGroup 根节点），按 group_index 排序
    pub groups: Vec<ModGroupData>,
    /// 所有模组的扁平列表（不含 None 空槽位）
    pub mods: Vec<ModData>,
    /// 总模组数（不含 None 空槽位）
    pub total_mods_count: usize,
    /// 启用的模组数（!disabled && !mod_disabled）
    pub enabled_mods_count: usize,
    /// 禁用的模组数（disabled || mod_disabled）
    pub disabled_mods_count: usize,
}

/// 获取 `_MANAGED_` 文件夹路径
///
/// 将 `game_mods_path` 与 `constants::MANAGED_FOLDER` 拼接，返回完整的 Managed 目录路径。
/// 所有模组文件都存放在此目录下。
pub fn get_managed_folder(game_mods_path: &Path) -> PathBuf {
    game_mods_path.join(constants::MANAGED_FOLDER)
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
/// 3. 主 INI 文件（`d3dx.ini` / `RatioShot.ini`，取决于 `TargetGame`）是否存在
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
    // SAFETY: The regex GROUP_N_RE defines exactly one capture group; if captures() succeeded, group 1 is guaranteed.
    let index_str = captures.get(1).unwrap().as_str();
    index_str.parse::<u32>().ok()
}

/// 判断目录名是否表示 group_xx（普通分组）目录（如 group_1, group_23）
///
/// 匹配规则：以 "group_" 前缀开头，后续为一个或多个数字字符
pub fn is_group_xx_dir(dir_name: &str) -> bool {
    if let Some(rest) = dir_name.strip_prefix(constants::MOD_GROUP_FILE_PREFIX) {
        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}

/// 检查目录是否包含任何 .ini 文件（仅检查扩展名，不读取内容）
/// 使用 entry.file_type() 免 metadata() 系统调用
///
/// NRMM 对齐：显式跳过 desktop.ini，避免将系统配置文件被视为模组 INI
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
        let ft = match entry.file_type() {
            Ok(f) => f,
            Err(_) => continue,
        };
        if ft.is_file() || ft.is_symlink() {
            let path = entry.path();
            // 显式排除系统 desktop.ini（NRMM 对齐）
            if constants::is_desktop_ini(&path) {
                continue;
            }
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
///
/// 写文件前暂停文件监控（`WATCHER_PAUSED`），避免触发循环事件。
///
/// # Panics
/// 如果写入文件失败（如权限不足、磁盘满），`write()` 操作会 panic 前先由 `?` 传播错误。
/// 但若 `WATCHER_PAUSED` 的 `store` 操作在 panic 后未恢复，后续文件监控可能异常。
///
/// # Errors
/// 当 `path` 存在但无法读取，或 `path` 不存在且无法写入时返回 `Err`
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

/// 在目录中查找图标路径
///
/// 查找策略：
/// 1. 优先匹配 `constants::ICON_NAME_PRIORITY` 中的优先级文件名（如 `icon.png`）
/// 2. 否则取第一张非 `DISABLED` 前缀的图片（按字母序）
///
/// 使用 `entry.file_type()` 而非 `fs::metadata()` 判断文件类型，减少系统调用次数。
pub fn find_icon_path(dir_path: &Path) -> Result<Option<PathBuf>> {
    let entries = match fs::read_dir(dir_path) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    let mut files: Vec<PathBuf> = Vec::with_capacity(4);
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
    log::debug!("[core::mod_scanner] [scan_mods_light] Starting light scan | path={:?}", game_mods_path);
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
        });
    }

    let mut groups: Vec<ModGroupData> = Vec::with_capacity(32);   // 预分配 32 个分组，覆盖绝大多数场景
    let mut all_mods: Vec<ModData> = Vec::with_capacity(256);     // 预分配 256 个模组，减少扩容次数

    let entries = fs::read_dir(&managed_folder)?;
    let mut root_dirs: Vec<PathBuf> = Vec::with_capacity(32);     // 预分配存储根目录列表

    for entry in entries {
        let entry = entry?;
        let ft = entry.file_type()?;
        if !ft.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
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
            let dir_name = dir_path.file_name().unwrap_or_default().to_string_lossy().to_string();
            is_normal_group_dir(&dir_name).map(|gi| (dir_path.clone(), dir_name, gi))
        })
        .collect();

    let normal_results: Vec<(ModGroupData, Vec<ModData>)> = normal_tasks
        .par_iter()
        .filter_map(|(dir_path, dir_name, group_index)| {
            scan_normal_group_light(dir_path, dir_name, *group_index).ok()
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
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            is_normal_group_dir(&name).is_none()
        })
        .cloned()
        .collect();

    let mutex_results: Vec<(Option<ModGroupData>, Vec<ModData>)> = mutex_roots
        .par_iter()
        .filter_map(|root_path| scan_mutex_group_dfs(root_path).ok())
        .collect();

    for (g_opt, ms) in mutex_results {
        if let Some(g) = g_opt {
            groups.push(g);
        }
        all_mods.extend(ms);
    }

    // 按 group_index 排序
    groups.sort_by_key(|g| g.group_index());

    let enabled = all_mods.iter().filter(|m| !m.disabled && !m.mod_disabled && m.name != "None").count();
    let disabled = all_mods.iter().filter(|m| (m.disabled || m.mod_disabled) && m.name != "None").count();
    let total = all_mods.iter().filter(|m| m.name != "None").count();

    let elapsed = start.elapsed().as_millis();
    log::info!("Light scan completed in {}ms, {} mods, {} groups", elapsed, total, groups.len());
    log::debug!("[core::mod_scanner] [scan_mods_light] done | elapsed={:?}ms | mods={} groups={}", _s.elapsed().as_millis(), total, groups.len());

    Ok(ScanResult {
        groups,
        total_mods_count: total,
        enabled_mods_count: enabled,
        disabled_mods_count: disabled,
        mods: all_mods,
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
fn scan_normal_group_light(dir_path: &Path, group_name: &str, group_index: u32) -> Result<(ModGroupData, Vec<ModData>)> {
    let mut mods: Vec<ModData> = Vec::with_capacity(32);   // 预分配 32 个模组槽位，覆盖绝大多数 NormalGroup

    // 读取/创建标记文件
    let groupname_path = dir_path.join("groupname");
    let group_display_name = read_or_create_marker_file(&groupname_path, group_name)?;

    let selectedindex_path = dir_path.join(constants::SELECTED_INDEX_FILE);
    let selected_index_str = read_or_create_marker_file(&selectedindex_path, "0")?;
    let selected_index: i32 = selected_index_str.parse().unwrap_or(0);

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

        // 读取/创建 modname 标记文件（NRMM 逻辑：优先使用 modname 文件中的名称作为展示名，不存在则创建并写入文件夹名）
        let modname_path = path.join("modname");
        let mod_name = {
            let n = read_or_create_marker_file(&modname_path, &display_name)?;
            if n.trim().is_empty() { display_name.clone() } else { n }
        };

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
            mod_name: mod_name.clone(),
            name: mod_name,
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
    let mut other_mods: Vec<ModData> = Vec::with_capacity(mods.len().saturating_sub(1));  // 预分配（总数-1），排除 None 槽位
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

    // 重新分配 mod_index（None 始终在 0，真实模组从 1 开始）
    for (idx, m) in mods.iter_mut().enumerate() {
        m.mod_index = idx as u32;
    }

    // 基于磁盘状态判定 is_active：
    // - 统计非 None 且未被禁用（!disabled）的模组数量
    // - 恰好一个启用 → 该模组为 active
    // - 零个启用 → None 为 active
    // - 多个启用（异常状态，如手动操作文件）→ 回退到 selectedindex 文件
    let enabled_non_none: Vec<usize> = mods.iter()
        .enumerate()
        .filter(|(_, m)| m.name != "None" && !m.disabled)
        .map(|(i, _)| i)
        .collect();

    let active_mod_index: i32 = if enabled_non_none.len() == 1 {
        let idx = enabled_non_none[0];
        mods[idx].is_active = true;
        // None 槽位设置为不激活
        if let Some(none) = mods.iter_mut().find(|m| m.name == "None") {
            none.is_active = false;
        }
        idx as i32
    } else if enabled_non_none.is_empty() {
        // 没有启用的模组 → None 激活
        for m in mods.iter_mut() {
            m.is_active = m.name == "None";
        }
        0
    } else {
        // 多个模组启用（异常），回退到 selectedindex 文件
        let mut active_idx: i32 = -1;
        for (idx, m) in mods.iter_mut().enumerate() {
            if m.name == "None" {
                m.is_active = selected_index == 0;
                if selected_index == 0 {
                    active_idx = 0;
                }
            } else {
                m.is_active = idx as i32 == selected_index;
                if idx as i32 == selected_index {
                    active_idx = idx as i32;
                }
            }
        }
        // 如果 selectedindex 指向的模组是禁用的，则 fallback 到第一个启用的
        if active_idx < 0 || mods.get(active_idx as usize).map(|m| m.disabled).unwrap_or(true) {
            if let Some(&first_enabled) = enabled_non_none.first() {
                for m in mods.iter_mut() {
                    m.is_active = false;
                }
                mods[first_enabled].is_active = true;
                active_idx = first_enabled as i32;
            }
        }
        active_idx
    };

    let mod_paths: Vec<PathBuf> = mods.iter()
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
        preview_image_path: group_icon,
        ..Default::default()
    };

    Ok((group, mods))
}

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
fn scan_mutex_group_dfs(root_path: &Path) -> Result<(Option<ModGroupData>, Vec<ModData>)> {
    let root_name = root_path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let root_disabled = is_disabled_dir(&root_name);
    let base_root_name = if root_disabled {
        DISABLED_PREFIX_RE.replace(&root_name, "").to_string()
    } else {
        root_name.clone()
    };
    // 仅 group_xx 目录才读/写 groupname 标记文件，非group目录直接使用目录名
    let root_display_name = if is_group_xx_dir(&root_name) {
        let groupname_path = root_path.join("groupname");
        read_or_create_marker_file(&groupname_path, &base_root_name)?
    } else {
        base_root_name.clone()
    };

    // 规范化路径 visited 集合：防止 symlink/hardlink 导致循环或重复
    let mut visited: HashSet<PathBuf> = HashSet::with_capacity(64);   // 预分配 64 个容量，覆盖大多数嵌套深度
    let root_canonical = root_path.canonicalize().unwrap_or_else(|_| root_path.to_path_buf());
    visited.insert(root_canonical);

    // 检查根目录是否本身就是模组（含 ini）
    if dir_has_ini_file(root_path)? {
        // 根目录是模组叶子节点，不创建分组
        let mod_data = build_mutex_mod_light(root_path, 0, 0)?;
        return Ok((None, vec![mod_data]));
    }

    let mut all_mods: Vec<ModData> = Vec::with_capacity(256);   // 预分配 256 个模组，覆盖大多数 MutexGroup
    // groups[0] 是根分组，后续是子分组
    let mut groups: Vec<ModGroupData> = Vec::with_capacity(32);   // 预分配 32 个分组，覆盖大多数嵌套深度

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

    // DFS 栈初始化（LIFO 后进先出）
    let mut stack: Vec<DfsStackItem> = Vec::with_capacity(64);   // 预分配 64 个栈元素，覆盖大多数嵌套深度

    // 列出根目录的一级子目录，push 到栈（用 entry.file_type() 避免 metadata 系统调用）
    let root_entries = fs::read_dir(root_path)?;
    let mut root_subdirs: Vec<PathBuf> = Vec::with_capacity(8);    // 预分配 8 个，大多数 MutexGroup 根目录子目录不多
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
            let name = entry.file_name().to_string_lossy().to_string();
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
            all_mods.push(mod_data);

            // 添加到父分组
            if let Some(pidx) = parent_idx {
                // SAFETY: all_mods.push() was called immediately above; all_mods is guaranteed non-empty.
                groups[pidx].mods.push(all_mods.last().unwrap().clone());
                groups[pidx].mod_paths.push(current_path.clone());
                groups[pidx].mod_count += 1;
            }
            global_mod_index += 1;
            // 叶子节点，停止向下遍历
            continue;
        }

        // 没有 ini，视为分组节点
        let dir_name = current_path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let dir_disabled = is_disabled_dir(&dir_name);
        let base_dir_name = if dir_disabled {
            DISABLED_PREFIX_RE.replace(&dir_name, "").to_string()
        } else {
            dir_name.clone()
        };
        // 仅 group_xx 目录才读/写 groupname 标记文件，非group目录直接使用目录名
        let dir_display_name = if is_group_xx_dir(&dir_name) {
            let sub_groupname_path = current_path.join("groupname");
            read_or_create_marker_file(&sub_groupname_path, &base_dir_name)?
        } else {
            base_dir_name.clone()
        };

        // 查找图标
        let icon_path = find_icon_path(&current_path)?;

        // 检查是否有子目录（用 entry.file_type() + canonicalize 去重）
        let sub_entries = fs::read_dir(&current_path)?;
        let mut subdirs: Vec<PathBuf> = Vec::with_capacity(8);    // 预分配 8 个，大多数分组子目录数量有限
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
                let name = entry.file_name().to_string_lossy().to_string();
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
fn build_mutex_mod_light(mod_path: &Path, group_index: u32, mod_index: u32) -> Result<ModData> {
    let dir_name = mod_path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let disabled = is_disabled_dir(&dir_name);
    let display_name = if disabled {
        DISABLED_PREFIX_RE.replace(&dir_name, "").to_string()
    } else {
        dir_name.clone()
    };

    // NRMM 逻辑：互斥组下不创建 modname 文件，但若已存在则优先使用其内容作为展示名
    let modname_path = mod_path.join("modname");
    let mod_name = if modname_path.exists() {
        fs::read_to_string(&modname_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(display_name.clone())
    } else {
        display_name.clone()
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
        mod_name: mod_name.clone(),
        name: mod_name,
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

    let mut groups: Vec<ModGroupData> = Vec::with_capacity(32);   // 预分配 32 个分组，覆盖绝大多数场景
    let mut all_mods: Vec<ModData> = Vec::with_capacity(256);     // 预分配 256 个模组，减少扩容次数
    let mut known_libraries = HashSet::new();                       // 收集所有模组中定义的库

    let entries = fs::read_dir(&managed_folder)?;

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
fn scan_group_directory_deep(dir_path: &Path, group_name: &str, group_type: GroupType) -> Result<(ModGroupData, Vec<ModData>)> {
    use std::collections::VecDeque;

    let mut mods = Vec::with_capacity(256);                 // 预分配 256 个模组
    let mut subgroups: Vec<ModGroupData> = Vec::with_capacity(32); // 预分配 32 个子分组
    let mut subgroup_paths: HashSet<PathBuf> = HashSet::with_capacity(32);

    let mut queue = VecDeque::new();
    queue.push_back(dir_path.to_path_buf());

    let mut visited_dirs = HashSet::with_capacity(64);      // 规范化路径 visited 集合，防止 symlink 循环
    visited_dirs.insert(dir_path.to_path_buf());

    while let Some(current_path) = queue.pop_front() {
        let (has_ini, has_icon, icon_path, ini_files) = check_directory_for_mod_deep(&current_path)?;

        if has_ini || has_icon {
            let parent_groups: Vec<String> = Vec::with_capacity(4);
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
                let ft = match sub_entry.file_type() {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                if ft.is_dir() || ft.is_symlink() {
                    let sub_path = sub_entry.path();
                    let sub_name = sub_entry.file_name().to_string_lossy().to_string();
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

/// 深度扫描：检查目录是否包含 INI 文件和图标
///
/// 使用 `entry.file_type()` 判断文件类型，避免 `metadata()` 系统调用。
/// 返回四元组：`(has_ini, has_icon, icon_path, ini_files)`。
///
/// - `has_ini`: 是否至少有一个 `.ini` 文件
/// - `has_icon`: 是否至少有一个图片文件
/// - `icon_path`: 优先返回 `icon.*` 命名的图片，否则返回第一张非 `DISABLED` 前缀的图片
/// - `ini_files`: 所有 `.ini` 文件的路径列表（已排序）
fn check_directory_for_mod_deep(dir: &Path) -> Result<(bool, bool, Option<PathBuf>, Vec<PathBuf>)> {
    let mut has_ini = false;
    let mut has_icon = false;
    let mut icon_path: Option<PathBuf> = None;
    let mut ini_files: Vec<PathBuf> = Vec::with_capacity(8);
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
        let ft = match entry.file_type() {
            Ok(f) => f,
            Err(_) => continue,
        };
        if ft.is_file() || ft.is_symlink() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                let ext_lower = ext.to_string_lossy().to_lowercase();
                if ext_lower == "ini" {
                    // NRMM 对齐：显式跳过桌面配置文件（系统 INI，不参与模组 INI 注入/统计）
                    if constants::is_desktop_ini(&path) {
                        continue;
                    }
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

    // NRMM 逻辑：优先使用 modname 文件内容作为展示名；不存在则回退文件夹名（深度扫描不创建，由轻量扫描负责创建）
    let display_name = if disabled {
        DISABLED_PREFIX_RE.replace(&dir_name, "").to_string()
    } else {
        dir_name.clone()
    };
    let modname_path = dir.join("modname");
    let mod_name = if modname_path.exists() {
        fs::read_to_string(&modname_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(display_name.clone())
    } else {
        display_name.clone()
    };

    let mut mod_ini_data: Vec<ModIniData> = Vec::with_capacity(8);           // 预分配 8 个 INI 数据，大多数模组只有少量 INI
    let mut all_errored_lines: Vec<ErroredLines> = Vec::with_capacity(8);  // 预分配 8 个错误行
    let mut total_sections = 0;
    let mut total_key_sections = 0;
    let mut total_texture_sections = 0;
    let mut total_shader_sections = 0;
    let mut total_command_lists = 0;
    let mut total_resources = 0;
    let mut mod_namespace: Option<String> = None;
    let mut defined_libraries = HashSet::with_capacity(32);                 // 预分配 32 个库，覆盖大多数模组

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

/// 对单个收敛后路径执行局部扫描，返回该路径下的 `ScanResult` 子树
///
/// 增量更新专用：仅扫描 `target_subpath` 指向的分组子树，避免全量扫描。
///
/// # 局部扫描策略
/// - `target_subpath` 为空或指向 `_MANAGED_` 根目录 → 退化为全量轻量扫描
/// - `target_subpath` 指向 `group_xx` 目录 → 仅扫描该 NormalGroup
/// - `target_subpath` 指向 `group_xx/mod_yyy` → 扫描其所在的 `group_xx` 分组
/// - `target_subpath` 指向 MutexGroup 根目录 → 仅扫描该 MutexGroup 根
/// - 目标路径不存在 → 退化为全量扫描（兜底）
///
/// 粒度到分组级，覆盖 file_watcher 的常见场景。保持返回类型 `ScanResult` 不变，
/// `subtree_replace` 调用方无需修改。
///
/// # Errors
/// 当底层扫描函数失败时传播错误。
pub fn scan_partial_path(mods_path: &Path, target_subpath: &Path) -> Result<ScanResult> {
    let managed_folder = get_managed_folder(mods_path);
    let target_norm = normalize_subpath(target_subpath, mods_path);

    // 若 target_subpath 为空或指向 _MANAGED_ 根目录，退化为全量轻量扫描
    if target_norm.is_empty() || target_norm == "_MANAGED_" {
        return scan_mods_light(mods_path);
    }

    // 解析 target_subpath 相对于 _MANAGED_ 的路径
    let target_full = managed_folder.join(&target_norm);
    if !target_full.exists() {
        // 目标路径不存在，退化为全量扫描
        return scan_mods_light(mods_path);
    }

    // 提取 target_subpath 的第一段（_MANAGED_ 下的直接子目录名）
    // 粒度到分组级：无论 target_subpath 指向分组目录还是其下的模组目录，都扫描整个分组
    let first_segment = target_norm.split('/').next().filter(|s| !s.is_empty());

    if let Some(first_seg) = first_segment {
        let first_dir = managed_folder.join(first_seg);
        let dir_name = first_seg.to_string();

        // 判断是否是 NormalGroup（group_xx）
        if let Some(group_index) = is_normal_group_dir(&dir_name) {
            // 仅扫描该 NormalGroup
            let (g, ms) = scan_normal_group_light(&first_dir, &dir_name, group_index)?;
            let total = ms.iter().filter(|m| m.name != "None").count();
            let enabled = ms.iter().filter(|m| !m.disabled && !m.mod_disabled && m.name != "None").count();
            let disabled = ms.iter().filter(|m| (m.disabled || m.mod_disabled) && m.name != "None").count();
            return Ok(ScanResult {
                groups: vec![g],
                mods: ms,
                total_mods_count: total,
                enabled_mods_count: enabled,
                disabled_mods_count: disabled,
            });
        }

        // 否则视为 MutexGroup 根目录
        let (g_opt, ms) = scan_mutex_group_dfs(&first_dir)?;
        let total = ms.iter().filter(|m| m.name != "None").count();
        let enabled = ms.iter().filter(|m| !m.disabled && !m.mod_disabled && m.name != "None").count();
        let disabled = ms.iter().filter(|m| (m.disabled || m.mod_disabled) && m.name != "None").count();
        return Ok(ScanResult {
            groups: g_opt.into_iter().collect(),
            mods: ms,
            total_mods_count: total,
            enabled_mods_count: enabled,
            disabled_mods_count: disabled,
        });
    }

    // 兜底：全量扫描
    scan_mods_light(mods_path)
}

/// 规范化子路径：将任意形式的路径转换为相对于 `_MANAGED_` 的相对路径。
///
/// 用于 `scan_partial_path()` 中提取目标分组名，支持以下输入形式：
/// - 绝对路径（来自文件监听器的 `consolidate` 结果）：`D:\Games\Mods\_MANAGED_\group_1`
/// - 带 `_MANAGED_` 前缀的相对路径：`_MANAGED_/group_1`
/// - 不带前缀的相对路径：`group_1`
///
/// 统一输出为 `/` 分隔符、无末尾斜杠的相对路径（如 `group_1`）。
/// 若输入为空或仅指向 `_MANAGED_` 根目录，返回空字符串（触发全量扫描降级）。
///
/// # 参数
/// - `p`: 待规范化的路径，可为绝对或相对路径
/// - `mods_path`: 模组根目录路径，用于计算 `_MANAGED_` 文件夹位置
///
/// # 返回值
/// 相对于 `_MANAGED_` 的规范化子路径字符串
fn normalize_subpath(p: &Path, mods_path: &Path) -> String {
    let managed_folder = get_managed_folder(mods_path);

    // 策略 1：优先使用 canonicalize 进行可靠的路径前缀剥离
    // 适用于绝对路径（文件监听器 consolidate 的返回值）
    let p_can = p.canonicalize();
    let managed_can = managed_folder.canonicalize();
    if let (Ok(p_abs), Ok(mgr_abs)) = (p_can, managed_can) {
        if let Ok(rel) = p_abs.strip_prefix(&mgr_abs) {
            return rel
                .to_string_lossy()
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_string();
        }
        // 路径不在 _MANAGED_ 下，返回空字符串触发全量扫描降级
        return String::new();
    }

    // 策略 2：canonicalize 失败（如测试中的相对路径），使用字符串匹配剥离前缀
    let normalized = p
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();

    // 剥离 `_MANAGED_/` 前缀
    if let Some(stripped) = normalized.strip_prefix("_MANAGED_/") {
        return stripped.to_string();
    }
    // 整个路径就是 `_MANAGED_`，返回空字符串触发全量扫描
    if normalized == "_MANAGED_" {
        return String::new();
    }

    // 已是相对路径（如 `group_1`），直接返回
    normalized
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// 测试辅助：创建临时目录，在根目录下创建 `_MANAGED_` 子目录
    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        dir
    }

    /// 测试辅助：在 `_MANAGED_` 下创建分组目录
    fn create_group_dir(base: &Path, group_name: &str) -> PathBuf {
        let group_path = base.join("_MANAGED_").join(group_name);
        fs::create_dir_all(&group_path).unwrap();
        group_path
    }

    /// 测试辅助：在分组目录下创建带 INI 文件的模组目录
    fn create_mod_with_ini(group_path: &Path, mod_name: &str, ini_content: &str) -> PathBuf {
        let mod_path = group_path.join(mod_name);
        fs::create_dir_all(&mod_path).unwrap();
        let ini_path = mod_path.join("mod.ini");
        fs::write(&ini_path, ini_content).unwrap();
        mod_path
    }

    /// 测试辅助：在根目录下创建主 INI 文件（d3dx.ini）
    fn create_d3dx_ini(base: &Path) {
        fs::write(base.join("d3dx.ini"), "; test").unwrap();
    }

    // ========== 深度扫描测试（原 scan_mods 改名为 scan_mods_deep） ==========

    /// 测试：空 `_MANAGED_` 目录的深度扫描应返回空结果
    #[test]
    fn test_scan_empty_managed_folder() {
        let dir = setup_test_dir();
        let result = scan_mods_deep(dir.path()).unwrap();
        assert!(result.groups.is_empty());
        assert!(result.mods.is_empty());
        assert_eq!(result.total_mods_count, 0);
    }

    /// 测试：深度扫描单个 NormalGroup 中的单个模组
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

    /// 测试：深度扫描 `DISABLED` 前缀模组应被正确标记为禁用状态
    #[test]
    fn test_scan_disabled_mod() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&group_path, "DISABLEDMyMod", "[TextureOverrideTest]\nhash = 0x456\n");

        let result = scan_mods_deep(dir.path()).unwrap();
        assert_eq!(result.mods.len(), 1);
        assert!(result.mods[0].disabled);
        assert!(result.mods[0].mod_disabled);
        assert!(result.mods[0].name.contains("MyMod"));
        assert_eq!(result.disabled_mods_count, 1);
        assert_eq!(result.enabled_mods_count, 0);
    }

    /// 测试：深度扫描忽略非 `group_xx` 目录（MutexGroup 和无关目录）
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

    /// 测试：深度扫描能递归找到嵌套子目录中的模组（BFS 遍历）
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

    /// 测试：深度扫描模组包含图标文件时可正确识别
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

    /// 测试：`check_mods_path` 对有效路径返回 `Valid`
    #[test]
    fn test_check_mods_path_valid() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let status = check_mods_path(TargetGame::GenshinImpact, dir.path());
        assert_eq!(status, ModsPathStatus::Valid);
    }

    /// 测试：`check_mods_path` 对缺少 `_MANAGED_` 目录的路径返回 `ManagedFolderNotFound`
    #[test]
    fn test_check_mods_path_missing_managed() {
        let dir = TempDir::new().unwrap();
        create_d3dx_ini(dir.path());
        let status = check_mods_path(TargetGame::GenshinImpact, dir.path());
        assert_eq!(status, ModsPathStatus::ManagedFolderNotFound);
    }

    /// 测试：`check_mods_path` 对不存在的路径返回 `NotFound`
    #[test]
    fn test_check_mods_path_not_found() {
        let dir = TempDir::new().unwrap();
        let nonexistent = dir.path().join("nonexistent");
        let status = check_mods_path(TargetGame::GenshinImpact, &nonexistent);
        assert_eq!(status, ModsPathStatus::NotFound);
    }

    /// 测试：`check_mods_path` 对缺少主 INI 文件的路径返回 `D3dxIniNotFound`
    #[test]
    fn test_check_mods_path_missing_d3dx() {
        let dir = setup_test_dir();
        let status = check_mods_path(TargetGame::GenshinImpact, dir.path());
        assert_eq!(status, ModsPathStatus::D3dxIniNotFound);
    }

    /// 测试：BFS 遇到模组目录（含 INI）后停止向下遍历，不扫描子目录
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

    /// 测试：深度扫描多个 NormalGroup 并按 `group_index` 排序
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

    /// 测试：深度扫描正确统计 INI 中各类型段的数量
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

    /// 测试：深度扫描正确检测 INI 中的 `include` 指令
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

    /// 测试：深度扫描正确提取 INI 中的 `namespace` 变量
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

    /// 测试：`is_normal_group_dir` 对 group_xx 的匹配和拒绝规则
    ///
    /// 验证：正确匹配 `group_1`、`group_12` 等；拒绝 `group_0`、`group_01`（前导零）、非数字后缀
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

    /// 测试：`natural_compare` 的自然排序功能
    ///
    /// 验证：`mod2` < `mod10`（数字段按数值比较而非字典序）；不区分大小写；相同字符串返回 Equal
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

    /// 测试：`is_disabled_dir` 对 DISABLED 前缀的检测
    ///
    /// 验证：`DISABLED_`、`disabled_`、`Disabled-`、`DISABLED `、`DISABLEDMod` 均被识别；`NormalMod` 和 `mod_disabled`（后缀）不被识别
    #[test]
    fn test_is_disabled_dir() {
        assert!(is_disabled_dir("DISABLED_Mod"));
        assert!(is_disabled_dir("disabled_mod"));
        assert!(is_disabled_dir("Disabled-Mod"));
        assert!(is_disabled_dir("DISABLED Mod"));
        assert!(is_disabled_dir("DISABLEDMod"));
        assert!(!is_disabled_dir("NormalMod"));
        assert!(!is_disabled_dir("mod_disabled"));
    }

    /// 测试：`dir_has_ini_file` 检测目录中是否存在 .ini 文件
    ///
    /// 验证：空目录返回 false；写入 .ini 后返回 true；其他扩展名文件不影响结果
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

    /// 测试：轻量扫描 NormalGroup 会自动创建 None 空槽位
    ///
    /// 验证：每个 NormalGroup 的第一个模组始终是索引为 0 的 None 槽位
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

    /// 测试：轻量扫描 NormalGroup 不递归子目录
    ///
    /// 验证：`SubDir` 是一级子目录被扫描为模组，但 `SubDir/NestedMod` 是二级子目录不被扫描
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

    /// 测试：轻量扫描会自动创建缺失的标记文件（groupname、selectedindex、modname）
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

    /// 测试：轻量扫描 MutexGroup 不会创建标记文件（与 NormalGroup 不同）
    ///
    /// 验证：MutexGroup 目录下不会出现 groupname、selectedindex、modname 等标记文件
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

    /// 测试：轻量扫描 MutexGroup 的栈式 DFS 能正确遍历嵌套目录
    ///
    /// 验证：嵌套结构 `#MutexRoot/Category1/ModA`、`#MutexRoot/Category1/SubCategory/ModB`、`#MutexRoot/ModC` 全部被扫描到
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

    /// 测试：轻量扫描 DFS 遇到含 INI 的目录后停止向下遍历
    ///
    /// 验证：`ModWithIni` 目录下有 INI，其子目录 `SubModUnder` 不被扫描
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

    /// 测试：轻量扫描正确识别 NormalGroup 和 MutexGroup 中的禁用模组
    #[test]
    fn test_light_scan_disabled_both_types() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // NormalGroup 禁用模组
        let group_path = create_group_dir(dir.path(), "group_1");
        let mod_enabled = group_path.join("EnabledMod");
        fs::create_dir_all(&mod_enabled).unwrap();
        let mod_disabled = group_path.join("DISABLEDDisabledMod");
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

    /// 测试：轻量扫描正确识别 NormalGroup 和 MutexGroup 中的收藏模组
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

    /// 测试：轻量扫描图标查找的优先级顺序
    ///
    /// 验证：`icon.png` 优先于 `preview.png` 和 `other.png`
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

    /// 测试：轻量扫描混合 NormalGroup 和 MutexGroup 并存
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

    /// 测试：轻量扫描严格拒绝 `group_0` 和 `group_01` 作为 NormalGroup
    ///
    /// 验证：`group_0`（零值）和 `group_01`（前导零）被作为 MutexGroup 处理，`group_1` 作为 NormalGroup
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

    /// 测试：轻量扫描排序规则（启用优先 > 收藏优先 > 自然排序）
    ///
    /// 验证：None 固定第一，然后是 fav_mod（收藏），mod2 < mod10（自然排序），禁用模组最后
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
        let m_disabled = group_path.join("DISABLEDbad");
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
        // 禁用的在最后（bad 是 DISABLEDbad 去掉前缀后的名字）
        assert_eq!(group.mods[4].name, "bad");
        assert_eq!(group.mods.len(), 5);
    }
}

#[cfg(test)]
mod tests_non_group_no_markers {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// is_group_xx_dir 正确性测试
    #[test]
    fn test_is_group_xx_dir() {
        assert!(is_group_xx_dir("group_1"));
        assert!(is_group_xx_dir("group_123"));
        assert!(!is_group_xx_dir("group_"));        // 无数字
        assert!(!is_group_xx_dir("group_abc"));     // 非数字
        assert!(!is_group_xx_dir("Group_1"));       // 大小写敏感
        assert!(!is_group_xx_dir("#MyGroup"));      // mutexGroup
        assert!(!is_group_xx_dir("SubGroup"));      // 自定义子分组
        assert!(!is_group_xx_dir("DISABLEDgroup_1")); // 禁用前缀
    }

    /// 扫描 mutexGroup（非group）不产生 groupname/modname 文件
    #[test]
    fn test_scan_mutex_no_markers_created() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        // 非group目录
        let non_group = mods_root.join("_MANAGED_").join("#CustomGroup");
        fs::create_dir_all(&non_group).unwrap();
        let mod_dir = non_group.join("ModA");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("ModA.ini"), "[ShaderOverride]\n").unwrap();

        // 执行扫描（使用轻量扫描，只有它会扫描 MutexGroup 并创建标记文件）
        let result = scan_mods_light(mods_root).expect("scan should succeed");

        // 检查不应存在的标记文件
        assert!(!non_group.join("groupname").exists(), "非group不应创建 groupname 文件");
        assert!(!mod_dir.join("modname").exists(), "非group下模组不应创建 modname 文件");

        // 但模组仍应被正确识别
        let mod_names: Vec<&str> = result.mods.iter().map(|m| m.name.as_str()).collect();
        assert!(mod_names.contains(&"ModA"), "ModA 应被扫描到，名称为 ModA");
    }

    /// 扫描 group_1 仍正常创建标记文件
    #[test]
    fn test_scan_group_xx_still_creates_markers() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join(constants::MANAGED_FOLDER);
        let group1 = managed.join("group_1");
        fs::create_dir_all(&group1).unwrap();
        let mod_dir = group1.join("ModA");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("ModA.ini"), "[ShaderOverride]\n").unwrap();

        // 使用轻量扫描，只有它会创建 groupname / modname 标记文件
        let _result = scan_mods_light(mods_root).expect("scan should succeed");

        // group_xx 目录应创建标记文件
        assert!(group1.join("groupname").exists(), "group_xx 应创建 groupname 文件");
        assert!(mod_dir.join("modname").exists(), "group_xx 下模组应创建 modname 文件");
    }
}

// ============================================================================
// scan_partial_path 单元测试
//
// 验证局部扫描的分组级粒度与降级策略：
// - 空/根路径降级为全量扫描
// - 不存在的目标路径降级为全量扫描
// - NormalGroup（group_xx）目标仅扫描该分组
// - MutexGroup（#xxx）目标仅扫描该分组子树
// - 目标为分组内模组路径时，粒度仍为分组级（扫描整个分组）
// ============================================================================
#[cfg(test)]
mod tests_scan_partial_path {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// 测试辅助：创建带 _MANAGED_ 子目录的临时根目录
    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        dir
    }

    /// 测试辅助：在 _MANAGED_ 下创建分组目录
    fn create_group_dir(base: &Path, group_name: &str) -> PathBuf {
        let group_path = base.join("_MANAGED_").join(group_name);
        fs::create_dir_all(&group_path).unwrap();
        group_path
    }

    /// 测试辅助：在分组目录下创建带 INI 文件的模组目录
    fn create_mod_with_ini(group_path: &Path, mod_name: &str, ini_content: &str) -> PathBuf {
        let mod_path = group_path.join(mod_name);
        fs::create_dir_all(&mod_path).unwrap();
        let ini_path = mod_path.join("mod.ini");
        fs::write(&ini_path, ini_content).unwrap();
        mod_path
    }

    /// 测试辅助：在根目录创建 d3dx.ini
    fn create_d3dx_ini(base: &Path) {
        fs::write(base.join("d3dx.ini"), "; test").unwrap();
    }

    /// 空 target_subpath 应降级为全量扫描，返回所有分组
    #[test]
    fn test_partial_empty_subpath_falls_back_to_full_scan() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");
        let g2 = create_group_dir(dir.path(), "group_2");
        create_mod_with_ini(&g2, "Mod2", "[TextureOverride2]\nhash=2\n");

        // 空路径降级
        let result = scan_partial_path(dir.path(), Path::new("")).unwrap();
        assert_eq!(
            result.groups.len(),
            2,
            "空 target_subpath 应降级为全量扫描，返回 2 个分组"
        );
    }

    /// target_subpath 指向 _MANAGED_ 根目录应降级为全量扫描
    #[test]
    fn test_partial_managed_root_falls_back_to_full_scan() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");
        let g2 = create_group_dir(dir.path(), "group_2");
        create_mod_with_ini(&g2, "Mod2", "[TextureOverride2]\nhash=2\n");

        let result = scan_partial_path(dir.path(), Path::new("_MANAGED_")).unwrap();
        assert_eq!(
            result.groups.len(),
            2,
            "target_subpath=_MANAGED_ 应降级为全量扫描"
        );
    }

    /// target_subpath 不存在时应降级为全量扫描
    #[test]
    fn test_partial_nonexistent_path_falls_back_to_full_scan() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");

        // 指向不存在的分组
        let result =
            scan_partial_path(dir.path(), Path::new("_MANAGED_/group_999")).unwrap();
        assert_eq!(
            result.groups.len(),
            1,
            "目标路径不存在时应降级为全量扫描，返回已存在的 1 个分组"
        );
    }

    /// target_subpath 指向 NormalGroup（group_xx）时仅扫描该分组
    #[test]
    fn test_partial_normal_group_scans_only_target_group() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");
        let g2 = create_group_dir(dir.path(), "group_2");
        create_mod_with_ini(&g2, "Mod2", "[TextureOverride2]\nhash=2\n");
        let g10 = create_group_dir(dir.path(), "group_10");
        create_mod_with_ini(&g10, "Mod10", "[TextureOverride10]\nhash=10\n");

        // 仅扫描 group_2
        let result =
            scan_partial_path(dir.path(), Path::new("_MANAGED_/group_2")).unwrap();

        // 仅返回 group_2，不包含 group_1 和 group_10
        assert_eq!(result.groups.len(), 1, "应仅返回目标分组");
        assert_eq!(result.groups[0].group_name, "group_2");
        // 模组仅包含 Mod2（轻量扫描会插入 None 槽位，故长度为 2）
        assert_eq!(result.mods.len(), 2, "应包含 None 槽位 + Mod2");
        assert_eq!(result.mods.iter().filter(|m| m.name == "Mod2").count(), 1);
        // total_mods_count 不计入 None
        assert_eq!(result.total_mods_count, 1);
    }

    /// target_subpath 指向 MutexGroup（#xxx）时仅扫描该分组子树
    #[test]
    fn test_partial_mutex_group_scans_only_target_group() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // 创建一个 MutexGroup：#CustomGroup 下含 ModA
        let mutex_group = dir.path().join("_MANAGED_").join("#CustomGroup");
        fs::create_dir_all(&mutex_group).unwrap();
        let mod_a = mutex_group.join("ModA");
        fs::create_dir_all(&mod_a).unwrap();
        fs::write(mod_a.join("ModA.ini"), "[ShaderOverride]\n").unwrap();

        // 另一个分组不应被扫描
        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");

        // 仅扫描 #CustomGroup
        let result =
            scan_partial_path(dir.path(), Path::new("_MANAGED_/#CustomGroup")).unwrap();

        // 仅包含 ModA，不包含 Mod1
        assert_eq!(
            result.mods.iter().filter(|m| m.name == "ModA").count(),
            1,
            "应扫描到 ModA"
        );
        assert_eq!(
            result.mods.iter().filter(|m| m.name == "Mod1").count(),
            0,
            "不应扫描到 group_1 下的 Mod1"
        );
    }

    /// target_subpath 指向分组内的模组目录时，粒度仍为分组级（扫描整个分组）
    #[test]
    fn test_partial_mod_path_scans_whole_group() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");
        create_mod_with_ini(&g1, "Mod2", "[TextureOverride2]\nhash=2\n");

        let g2 = create_group_dir(dir.path(), "group_2");
        create_mod_with_ini(&g2, "Mod3", "[TextureOverride3]\nhash=3\n");

        // 指向 group_1 下的 Mod1（粒度仍为分组级）
        let result =
            scan_partial_path(dir.path(), Path::new("_MANAGED_/group_1/Mod1")).unwrap();

        // 应扫描整个 group_1（None + Mod1 + Mod2），不包含 group_2 的 Mod3
        assert_eq!(result.groups.len(), 1, "应仅返回 group_1");
        assert_eq!(result.groups[0].group_name, "group_1");
        assert_eq!(
            result.mods.iter().filter(|m| m.name == "Mod3").count(),
            0,
            "不应扫描到 group_2 的 Mod3"
        );
        // group_1 内的 Mod1 和 Mod2 都应被扫描到
        assert_eq!(
            result.mods.iter().filter(|m| m.name == "Mod1").count(),
            1,
            "应扫描到 Mod1"
        );
        assert_eq!(
            result.mods.iter().filter(|m| m.name == "Mod2").count(),
            1,
            "应扫描到 Mod2（粒度为分组级）"
        );
    }

    /// normalize_subpath 应统一分隔符、去除末尾斜杠，并剥离 `_MANAGED_` 前缀
    #[test]
    fn test_normalize_subpath_handles_separators() {
        // canonicalize 在空 mods_path 下会失败，走策略 2（字符串匹配）
        // Windows 风格反斜杠应被转换为正斜杠，并剥离 _MANAGED_ 前缀
        let n1 = normalize_subpath(Path::new("_MANAGED_\\group_1"), Path::new(""));
        assert_eq!(n1, "group_1");

        // 末尾斜杠应被去除
        let n2 = normalize_subpath(Path::new("_MANAGED_/group_1/"), Path::new(""));
        assert_eq!(n2, "group_1");

        // 混合分隔符
        let n3 = normalize_subpath(Path::new("_MANAGED_\\group_1/Mod1\\"), Path::new(""));
        assert_eq!(n3, "group_1/Mod1");

        // 不带 _MANAGED_ 前缀的相对路径应原样返回
        let n4 = normalize_subpath(Path::new("group_1"), Path::new(""));
        assert_eq!(n4, "group_1");

        // 仅 _MANAGED_ 应返回空字符串（触发全量扫描降级）
        let n5 = normalize_subpath(Path::new("_MANAGED_"), Path::new(""));
        assert_eq!(n5, "");
    }

    /// normalize_subpath 应正确处理绝对路径（文件监听器 consolidate 的返回格式）
    #[test]
    fn test_normalize_subpath_absolute_path() {
        let dir = setup_test_dir();
        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "Mod1", "[Section]\n");

        // 模拟 consolidate 返回的绝对路径：mods_path/_MANAGED_/group_1
        let abs_path = dir.path().join("_MANAGED_").join("group_1");
        let result = normalize_subpath(&abs_path, dir.path());
        assert_eq!(result, "group_1", "绝对路径应被剥离为相对于 _MANAGED_ 的子路径");

        // 模组级绝对路径：mods_path/_MANAGED_/group_1/Mod1
        let abs_mod_path = g1.join("Mod1");
        let result2 = normalize_subpath(&abs_mod_path, dir.path());
        assert_eq!(result2, "group_1/Mod1", "模组级绝对路径应保留分组/模组结构");
    }
}
