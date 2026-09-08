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

use crate::core::constants;
use crate::core::file_watcher::WatcherGuard;
use crate::core::fs_walk::{self, SkippedEntry, TraversalStats};
use crate::core::ini_handler::IniFile;
use crate::core::mod_ini_cache::get_or_parse_ini;
use crate::core::namespace_handler;
use crate::models::enums::{GroupType, ModsPathStatus, TargetGame};
use crate::models::mod_data::{ErroredLines, ModData, ModGroupData, ModIniData};
use anyhow::Result;
use once_cell::sync::Lazy;
use rayon::prelude::*;
use regex::Regex;
use std::cmp::Ordering as CmpOrdering;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

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
//
// 约束（对齐用户要求 + 内部 u32 分组索引）：
// - 禁止前导零（`group_01` 无效）
// - 禁止 `group_0`（零值）
// - 数值位数 ≤ 9（`[1-9][0-9]{0,8}`）：`group_1`..`group_999999999` 合法；
//   `group_1000000000`（10 位）被拒，避免超出安全范围的超大索引。
// - 不重新引入原版 1–500 上限，数值大小不受限。
static GROUP_N_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^group_([1-9][0-9]{0,8})$").unwrap());

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
static DISABLED_PREFIX_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(?i:disabled)[_\- ]*").unwrap());

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

/// [`derive_group_active_state`] 的组内条目访问抽象。
///
/// 值语义 `ModData`（扫描构建期，Arc 包装前）与 `Arc<ModData>`（缓存定向
/// patch 期，经 `Arc::make_mut` 写时拷贝）经此 trait 共用同一派生真源。
pub trait GroupModSlot {
    /// 只读访问底层 `ModData`。
    fn mod_slot(&self) -> &ModData;
    /// 可变访问底层 `ModData`。
    fn mod_slot_mut(&mut self) -> &mut ModData;
}

impl GroupModSlot for ModData {
    fn mod_slot(&self) -> &ModData {
        self
    }

    fn mod_slot_mut(&mut self) -> &mut ModData {
        self
    }
}

impl GroupModSlot for Arc<ModData> {
    fn mod_slot(&self) -> &ModData {
        self
    }

    fn mod_slot_mut(&mut self) -> &mut ModData {
        Arc::make_mut(self)
    }
}

/// 基于磁盘状态派生组内 `is_active`（扫描与缓存定向 patch 的单一真源）。
///
/// 判定口径（与 NRMM 对齐）：
/// - 恰好一个启用（非 None 且未禁用）→ 该模组为 active；
/// - 零个启用 → None 槽位为 active（返回 0）；
/// - 多个启用（异常状态，如手动操作文件）→ 回退到 selectedindex 文件值；
///   selectedindex 指向禁用模组时 fallback 到第一个启用模组。
///
/// # 参数
/// - `mods`: 组内模组列表（含 None 空槽位，None 恒在下标 0；就地写入 `is_active`），
///   元素可为值语义 `ModData` 或 `Arc<ModData>`（写时拷贝，见 [`GroupModSlot`]）
/// - `selected_index`: selectedindex 标记文件当前值（None 槽位 = 0）
///
/// # 返回
/// 派生后的 `active_mod_index`（无激活条目为 -1；零启用且无 None 槽位时返回 0 但不激活任何条目）
pub fn derive_group_active_state<M: GroupModSlot>(mods: &mut [M], selected_index: i32) -> i32 {
    let enabled_non_none: Vec<usize> = mods
        .iter()
        .enumerate()
        .filter(|(_, m)| m.mod_slot().name != "None" && !m.mod_slot().disabled)
        .map(|(i, _)| i)
        .collect();

    if enabled_non_none.len() == 1 {
        let idx = enabled_non_none[0];
        mods[idx].mod_slot_mut().is_active = true;
        // None 槽位设置为不激活
        if let Some(none) = mods.iter_mut().find(|m| m.mod_slot().name == "None") {
            none.mod_slot_mut().is_active = false;
        }
        idx as i32
    } else if enabled_non_none.is_empty() {
        // 没有启用的模组 → None 激活
        for m in mods.iter_mut() {
            let slot = m.mod_slot_mut();
            let is_none = slot.name == "None";
            slot.is_active = is_none;
        }
        0
    } else {
        // 多个模组启用（异常），回退到 selectedindex 文件
        let mut active_idx: i32 = -1;
        for (idx, m) in mods.iter_mut().enumerate() {
            let slot = m.mod_slot_mut();
            if slot.name == "None" {
                slot.is_active = selected_index == 0;
                if selected_index == 0 {
                    active_idx = 0;
                }
            } else {
                slot.is_active = idx as i32 == selected_index;
                if idx as i32 == selected_index {
                    active_idx = idx as i32;
                }
            }
        }
        // 如果 selectedindex 指向的模组是禁用的，则 fallback 到第一个启用的
        if active_idx < 0
            || mods
                .get(active_idx as usize)
                .map(|m| m.mod_slot().disabled)
                .unwrap_or(true)
        {
            if let Some(&first_enabled) = enabled_non_none.first() {
                for m in mods.iter_mut() {
                    m.mod_slot_mut().is_active = false;
                }
                mods[first_enabled].mod_slot_mut().is_active = true;
                active_idx = first_enabled as i32;
            }
        }
        active_idx
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
                if ext.to_string_lossy().eq_ignore_ascii_case("ini") {
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

/// 移除 DISABLED 前缀（不区分大小写，含 `_`/`-`/空格分隔符），一次移除连续的全部前缀
///
/// 复用 [`DISABLED_PREFIX_RE`]（`^(?i:disabled)[_\- ]*`），因 `^` 锚定在串首，
/// 单次 `replace` 仅移除首个前缀，故循环剥离直到无变化，确保 `DISABLEDDISABLEDMod` → `Mod`。
///
/// # 参数
/// - `name`: 原始目录名
///
/// # 返回
/// 移除全部连续 DISABLED 前缀及前导分隔符后的目录名
pub fn strip_disabled_prefix(name: &str) -> String {
    let mut result = name.to_string();
    loop {
        let stripped = DISABLED_PREFIX_RE.replace(&result, "");
        if stripped.as_ref() == result {
            return result; // 无可剥前缀
        }
        // 关键：剩余部分以精确全大写 DISABLED（NRMM 累积前缀）开头才继续，
        // 避免把模组名中自然的大小写混合 "Disabled" 误删。
        if !stripped.starts_with("DISABLED") {
            return stripped.into_owned();
        }
        result = stripped.into_owned();
    }
}

/// 读取或创建标记文件，不存在则用默认内容创建
///
/// 写文件前通过 [`WatcherGuard`] 递增全局暂停计数，避免触发循环事件。
/// 守卫离开作用域自动递减计数（panic 安全），无需手动恢复。
///
/// # Errors
/// 当 `path` 存在但无法读取，或 `path` 不存在且无法写入时返回 `Err`
pub fn read_or_create_marker_file(path: &Path, default_content: &str) -> Result<String> {
    if path.exists() {
        let content = fs::read_to_string(path)?;
        Ok(content.trim().to_string())
    } else {
        let result = {
            let _guard = WatcherGuard::new();
            crate::utils::atomic_write(path, default_content.as_bytes())
        }; // _guard drop：计数递减
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
                if ICON_EXTENSIONS
                    .iter()
                    .any(|e| ext.to_string_lossy().eq_ignore_ascii_case(e))
                {
                    // 装饰性路径（图片）：非法 UTF-8 文件名直接跳过该候选，
                    // 前端走占位图逻辑，无需上报（用户指示：图片路径前端自行处理）
                    if entry.file_name().to_str().is_none() {
                        log::warn!(
                            "[find_icon_path] 跳过非法 UTF-8 名称图片候选 | dir={:?}",
                            dir_path
                        );
                        continue;
                    }
                    files.push(path);
                }
            }
        }
    }

    // 优先查找 ICON_NAME_PRIORITY 中的文件名
    for priority_name in constants::ICON_NAME_PRIORITY {
        for file in &files {
            if let Some(fname) = file.file_name() {
                if fname.to_string_lossy().eq_ignore_ascii_case(priority_name) {
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

/// 读取 metadata 的修改时间（UNIX_EPOCH 起的纳秒数）。
///
/// C-3b 磁盘签名的 mtime 源提取器：metadata 缺失（文件/目录不存在）或
/// 时间不可得时返回 0（fav 不存在、目录不可 stat 等场景的约定源值）。
fn metadata_mtime_nanos(meta: Option<&fs::Metadata>) -> u64 {
    meta.and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// 组合磁盘签名（C-3b 增量复用判定）。
///
/// 三源 mtime 纳秒按位移分散后异或混合：
/// 1. mod 目录 mtime（目录名/标记文件增删、modname 创建等直接子项变化）；
/// 2. fav 标记 mtime（收藏状态，不存在恒 0，创建/删除即失效）；
/// 3. 所属组 selectedindex mtime（`is_active` 派生源，select 改写即失效；
///    互斥组无该文件，源恒 0）。
///
/// 目录源为 0（目录缺失/不可 stat）时整体返回 0 = 无签名 → 增量合并时
/// 永不复用该条目（fail-safe，宁可重建不可误复用）。
fn compute_disk_signature(dir_nanos: u64, fav_nanos: u64, group_selectedindex_nanos: u64) -> u64 {
    if dir_nanos == 0 {
        return 0;
    }
    dir_nanos.rotate_left(32) ^ fav_nanos.rotate_left(16) ^ group_selectedindex_nanos
}

/// 统一重算模组计数，口径与全量轻量扫描一致。
///
/// 排除 `None` 空槽位（`name == "None"`），以 `!disabled && !mod_disabled` 判定启用。
/// 供全量扫描与增量合并（`mod_cache::subtree_replace`）共用，保证两处计数一致。
///
/// # 返回
///
/// `(total, enabled, disabled)`，均为 `name != "None"` 的活跃模组计数。
pub(crate) fn recount_mods(mods: &[Arc<ModData>]) -> (usize, usize, usize) {
    let mut total = 0usize;
    let mut enabled = 0usize;
    let mut disabled = 0usize;
    for m in mods {
        if m.name == "None" {
            continue;
        }
        total += 1;
        if !m.disabled && !m.mod_disabled {
            enabled += 1;
        } else {
            disabled += 1;
        }
    }
    (total, enabled, disabled)
}

/// 分组排序比较（对齐 NRMM）：NormalGroup 角色分组永远置顶、按整数索引序；
/// MutexGroup 互斥分组按名称字符串序。
///
/// 供全量扫描与增量合并（`mod_cache::subtree_replace`）共用分组排序规则。
pub(crate) fn group_order_cmp(a: &ModGroupData, b: &ModGroupData) -> CmpOrdering {
    let a_mutex = a.group_type == GroupType::MutexGroup;
    let b_mutex = b.group_type == GroupType::MutexGroup;
    match (a_mutex, b_mutex) {
        (false, true) => CmpOrdering::Less,
        (true, false) => CmpOrdering::Greater,
        (false, false) => a.group_index.cmp(&b.group_index),
        (true, true) => a.group_name.cmp(&b.group_name),
    }
}

/// 模组排序比较器（单一真源）：禁用在后→收藏在前→最新收藏→自然排序
///
/// 供 `sort_mods_light`（`Arc<ModData>` 切片）与 `scan_normal_group_light`
/// 构建期（值语义 `Vec<ModData>`）共用，R3/C-3b 前者为 Arc 指针排序。
fn mod_order_cmp(a: &ModData, b: &ModData) -> CmpOrdering {
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

    let a_fav_path = Path::new(a.full_path.as_ref()).join(constants::FAV_MARKER);
    let b_fav_path = Path::new(b.full_path.as_ref()).join(constants::FAV_MARKER);
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
}

/// 统一排序模组列表：禁用在后→收藏在前→最新收藏→自然排序
///
/// R3/C-3b：元素为 `Arc<ModData>`（比较经 Deref 透明，排序移动的是指针）。
pub fn sort_mods_light(mods: &mut [Arc<ModData>]) {
    mods.sort_by(|a, b| mod_order_cmp(a, b));
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
        "[core::mod_scanner] [scan_mods_light] Starting light scan | path={:?}",
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
        "[core::mod_scanner] [scan_mods_light] done | elapsed={:?}ms | mods={} groups={}",
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
fn scan_normal_group_light(
    dir_path: &Path,
    group_name: &str,
    group_index: u32,
    stats: &TraversalStats,
) -> Result<(ModGroupData, Vec<Arc<ModData>>)> {
    let mut mods: Vec<ModData> = Vec::with_capacity(32); // 预分配 32 个模组槽位，覆盖绝大多数 NormalGroup

    // 读取/创建标记文件
    let groupname_path = dir_path.join("groupname");
    let group_display_name = read_or_create_marker_file(&groupname_path, group_name)?;

    let selectedindex_path = dir_path.join(constants::SELECTED_INDEX_FILE);
    let selected_index_str = read_or_create_marker_file(&selectedindex_path, "0")?;
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
        let n = read_or_create_marker_file(&modname_path, &display_name)?;
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
fn scan_mutex_group_dfs(
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
    // 仅 group_xx 目录才读/写 groupname 标记文件，非group目录直接使用目录名
    let root_display_name = if is_group_xx_dir(&root_name) {
        let groupname_path = root_path.join("groupname");
        read_or_create_marker_file(&groupname_path, &base_root_name)?
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
            // root 一级仅收集目录入栈（ini 文件已由 check_directory_for_mod_deep 收集）
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
            let stats = TraversalStats::default();
            let (g, ms) = scan_normal_group_light(&first_dir, &dir_name, group_index, &stats)?;
            let (invalid_utf8_count, invalid_utf8_entries) = stats.into_parts();
            let (mut total, mut enabled, mut disabled) = (0usize, 0usize, 0usize);
            for m in &ms {
                if m.name == "None" {
                    continue;
                }
                total += 1;
                if !m.disabled && !m.mod_disabled {
                    enabled += 1;
                } else {
                    disabled += 1;
                }
            }
            return Ok(ScanResult {
                groups: vec![g],
                mods: ms,
                total_mods_count: total,
                enabled_mods_count: enabled,
                disabled_mods_count: disabled,
                invalid_utf8_count,
                invalid_utf8_entries,
            });
        }

        // 否则视为 MutexGroup 根目录
        let stats = TraversalStats::default();
        let (g_opt, ms) = scan_mutex_group_dfs(&first_dir, &stats)?;
        let (invalid_utf8_count, invalid_utf8_entries) = stats.into_parts();
        let total = ms.iter().filter(|m| m.name != "None").count();
        let enabled = ms
            .iter()
            .filter(|m| !m.disabled && !m.mod_disabled && m.name != "None")
            .count();
        let disabled = ms
            .iter()
            .filter(|m| (m.disabled || m.mod_disabled) && m.name != "None")
            .count();
        return Ok(ScanResult {
            groups: g_opt.into_iter().collect(),
            mods: ms,
            total_mods_count: total,
            enabled_mods_count: enabled,
            disabled_mods_count: disabled,
            invalid_utf8_count,
            invalid_utf8_entries,
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

    /// 测试：深度扫描对禁用模组跳过内容解析（2026-08-18 性能优化）
    ///
    /// 构造启用 + 禁用（DISABLED_ 前缀）各一个模组，运行 scan_mods_deep 后断言：
    /// 1. 禁用模组仍被发现（disabled=true，ini_path 保留——update 需要路径做状态展示）；
    /// 2. 禁用模组的 section 计数为 0（未执行内容解析）；
    /// 3. 启用模组的 section 计数正常（解析未受影响）。
    #[test]
    fn test_scan_deep_disabled_mod_skips_parse() {
        let dir = setup_test_dir();
        let group = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(
            &group,
            "EnabledMod",
            "[TextureOverride]\nhash = 0xAAAA\n[CommandListX]\ny = 1\n",
        );
        create_mod_with_ini(
            &group,
            "DISABLED_DisabledMod",
            "[TextureOverride]\nhash = 0xBBBB\n[CommandListY]\nz = 1\n",
        );

        let result = scan_mods_deep(dir.path()).expect("scan_mods_deep 失败");
        assert_eq!(result.mods.len(), 2, "应发现启用与禁用两个模组");

        let enabled = result
            .mods
            .iter()
            .find(|m| !m.disabled)
            .expect("应存在启用模组");
        assert_eq!(
            enabled.mod_ini_data[0].section_count, 2,
            "启用模组应正常解析（2 个段）"
        );

        let disabled = result
            .mods
            .iter()
            .find(|m| m.disabled)
            .expect("应存在禁用模组");
        assert!(
            !disabled.mod_ini_data.is_empty() && !disabled.mod_ini_data[0].ini_path.is_empty(),
            "禁用模组 INI 路径应保留"
        );
        assert_eq!(
            disabled.mod_ini_data[0].section_count, 0,
            "禁用模组应跳过内容解析（section 计数为 0）"
        );
        assert!(
            disabled.errored_lines.is_empty(),
            "禁用模组不应产生解析错误条目"
        );
    }

    /// 测试：深扫两阶段库收集，跨 INI 引用已声明库不误报（error_type=5）
    ///
    /// 构造同一模组目录下的两个 INI：a.ini 通过 `run` 引用 b.ini 声明的
    /// ShaderOverride 库，同时再引用一个完全未声明的库。断言：
    /// 1. 引用已声明库（应合计为全局 known_libraries）不产生 NON EXISTENT LIB（error_type=5）；
    /// 2. 引用未声明库仍产生 error_type=5。
    #[test]
    fn test_deep_cross_ini_library_reference_resolution() {
        let dir = setup_test_dir();
        let group = create_group_dir(dir.path(), "group_1");
        let mod_path = group.join("MyMod");
        fs::create_dir_all(&mod_path).unwrap();
        // a.ini：引用 b.ini 声明的库 + 一个未声明库
        fs::write(
            mod_path.join("a.ini"),
            "[ShaderOverrideShaderA]\nhash = 0xAAA\nrun = ShaderOverrideShaderB\nrun = ShaderOverrideMissingLib\n",
        )
        .unwrap();
        // b.ini：声明 ShaderOverrideShaderB 库
        fs::write(
            mod_path.join("b.ini"),
            "[ShaderOverrideShaderB]\nhash = 0xBBB\n",
        )
        .unwrap();

        let result = scan_mods_deep(dir.path()).expect("scan_mods_deep 失败");
        assert_eq!(result.mods.len(), 1, "应发现一个模组");
        let mod_data = &result.mods[0];

        let type5_errors: Vec<&String> = mod_data
            .errored_lines
            .iter()
            .filter(|e| e.error_type == 5)
            .map(|e| &e.error_message)
            .collect();

        assert_eq!(
            type5_errors.len(),
            1,
            "仅未声明库应产出 NON EXISTENT LIB，实际: {type5_errors:?}"
        );
        assert!(
            type5_errors[0].contains("ShaderOverrideMissingLib"),
            "应仅报未声明库，实际: {type5_errors:?}"
        );
    }

    /// 测试：strip_disabled_prefix 一次移除连续的全部 DISABLED 前缀及分隔符
    #[test]
    fn test_strip_disabled_prefix() {
        // 大小写 + 不同分隔符
        assert_eq!(strip_disabled_prefix("DISABLED_Mod"), "Mod");
        assert_eq!(strip_disabled_prefix("Disabled-Mod"), "Mod");
        assert_eq!(strip_disabled_prefix("disabled_mod"), "mod");
        assert_eq!(strip_disabled_prefix("DISABLED Mod"), "Mod");
        assert_eq!(strip_disabled_prefix("DISABLEDMod"), "Mod");
        // 连续多个前缀：必须一次全部移除
        assert_eq!(strip_disabled_prefix("DISABLEDDISABLEDMod"), "Mod");
        assert_eq!(strip_disabled_prefix("DISABLED_DISABLED_Mod"), "Mod");
        // 前缀 DISABLED + 模组名以 Disabled（大小写混合）开头：只剥前缀，保留模组名
        assert_eq!(strip_disabled_prefix("DISABLEDDisabledMod"), "DisabledMod");
        assert_eq!(
            strip_disabled_prefix("DISABLEDDISABLEDDisabledMod"),
            "DisabledMod"
        );
        // 无前缀时原样返回
        assert_eq!(strip_disabled_prefix("Mod"), "Mod");
        // 整个名称就是前缀 → 空串
        assert_eq!(strip_disabled_prefix("DISABLED"), "");
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
        create_mod_with_ini(
            &group_path,
            "TestMod",
            "[TextureOverrideTest]\nhash = 0x123\n",
        );

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
        create_mod_with_ini(
            &group_path,
            "DISABLEDMyMod",
            "[TextureOverrideTest]\nhash = 0x456\n",
        );

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
        create_mod_with_ini(
            &mutex_path,
            "MutexMod",
            "[TextureOverrideTest]\nhash = 0x789\n",
        );

        let other_path = dir.path().join("_MANAGED_").join("OtherFolder");
        fs::create_dir_all(&other_path).unwrap();
        create_mod_with_ini(
            &other_path,
            "OtherMod",
            "[TextureOverrideTest]\nhash = 0xABC\n",
        );

        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(
            &group_path,
            "ValidMod",
            "[TextureOverrideTest]\nhash = 0xDEF\n",
        );

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
        assert!(result.mods[0]
            .preview_image_path
            .as_ref()
            .unwrap()
            .ends_with("icon.png"));
    }

    /// 测试：`check_mods_path` 对有效路径返回 `Valid`
    #[test]
    fn test_check_mods_path_valid() {
        // d3dx.ini 位于游戏根（Mods 父目录），不在 Mods/ 内
        let dir = setup_test_dir();
        let mods_path = dir.path().join("Mods");
        fs::create_dir_all(mods_path.join("_MANAGED_")).unwrap();
        create_d3dx_ini(dir.path());
        let status = check_mods_path(TargetGame::GenshinImpact, &mods_path);
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
        fs::write(
            subdir_under_mod.join("ignored.ini"),
            "[TextureOverrideB]\nhash=2\n",
        )
        .unwrap();

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
        // 9 位合法上限
        assert_eq!(is_normal_group_dir("group_999999999"), Some(999_999_999));

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

        // 拒绝非严格 group_int 命名（避免误删/误管）
        assert_eq!(is_normal_group_dir("groupxx"), None);
        assert_eq!(is_normal_group_dir("groupsxx"), None);
        assert_eq!(is_normal_group_dir("XX"), None);

        // 拒绝 10 位及以上（超出安全索引范围）
        assert_eq!(is_normal_group_dir("group_1000000000"), None);
        assert_eq!(is_normal_group_dir("group_4294967295"), None);
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
        assert!(
            !mod_names.contains(&"SubModUnder"),
            "Should not recurse into mod dir with ini"
        );
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
                assert!(
                    m.disabled || m.name == "None",
                    "{} should be disabled",
                    m.name
                );
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
        let group_names: Vec<&str> = result
            .groups
            .iter()
            .filter(|g| g.group_type == GroupType::NormalGroup)
            .map(|g| g.group_name.as_str())
            .collect();

        assert!(
            group_names.contains(&"group_1"),
            "group_1 should be NormalGroup"
        );
        // group_0 和 group_01 应该作为 MutexGroup 存在
        let mutex_names: Vec<&str> = result
            .groups
            .iter()
            .filter(|g| g.group_type == GroupType::MutexGroup)
            .map(|g| g.group_name.as_str())
            .collect();
        assert!(
            mutex_names.contains(&"group_0"),
            "group_0 should be MutexGroup"
        );
        assert!(
            mutex_names.contains(&"group_01"),
            "group_01 should be MutexGroup"
        );
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

    /// 测试（R3/C-3b）：轻量扫描为模组填充磁盘签名。
    ///
    /// 签名 = mod 目录 mtime ^ fav mtime ^ 组 selectedindex mtime 三源混合：
    /// - NormalGroup 与 MutexGroup 模组均非零；
    /// - 无磁盘变化时两次扫描签名相等（稳定，复用判定前提）；
    /// - fav 创建（fav 源 0→t）→ 该模组签名变化，未动兄弟不变；
    /// - 组 selectedindex 改写 → 组内全部模组签名变化（覆盖 is_active 派生源）；
    /// - 深度扫描不填充（0 = 无签名，永不参与增量复用）。
    #[test]
    fn test_light_scan_disk_signature_sources() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&group_path, "ModA", "[Section]\n");
        create_mod_with_ini(&group_path, "FavMod", "[Section]\n");
        // 互斥组模组
        let mutex_root = dir.path().join("_MANAGED_").join("#Mutex");
        fs::create_dir_all(mutex_root.join("ModX")).unwrap();
        fs::write(mutex_root.join("ModX").join("m.ini"), "[Section]\n").unwrap();

        let find_sig = |r: &ScanResult, name: &str| {
            r.mods
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("应存在模组 {name}"))
                .disk_signature
        };

        // 首扫会创建 modname/selectedindex 标记文件；二次扫描读已有文件（mtime 稳定）
        let r1 = scan_mods_light(dir.path()).unwrap();
        let r2 = scan_mods_light(dir.path()).unwrap();
        assert_ne!(find_sig(&r2, "ModA"), 0, "NormalGroup 模组签名应非零");
        assert_ne!(find_sig(&r2, "ModX"), 0, "MutexGroup 模组签名应非零");
        assert_eq!(
            find_sig(&r1, "ModA"),
            find_sig(&r2, "ModA"),
            "无磁盘变化时两次扫描签名应相等"
        );
        assert_eq!(find_sig(&r1, "ModX"), find_sig(&r2, "ModX"));

        // fav 源：FavMod 创建 fav → 其签名变化，未动的 ModA 签名不变
        fs::write(group_path.join("FavMod").join("fav"), "").unwrap();
        let r3 = scan_mods_light(dir.path()).unwrap();
        assert_ne!(
            find_sig(&r2, "FavMod"),
            find_sig(&r3, "FavMod"),
            "fav 创建（mtime 源 0→t）应使签名失效"
        );
        assert_eq!(
            find_sig(&r2, "ModA"),
            find_sig(&r3, "ModA"),
            "未动模组签名应保持稳定"
        );

        // 组 selectedindex 源：改写组目录 selectedindex → 组内全部模组签名变化
        fs::write(group_path.join("selectedindex"), "1").unwrap();
        let r4 = scan_mods_light(dir.path()).unwrap();
        assert_ne!(
            find_sig(&r3, "ModA"),
            find_sig(&r4, "ModA"),
            "selectedindex mtime 源应参与签名（覆盖 is_active 派生源）"
        );
        // 互斥组不受该组 selectedindex 影响（各自组源独立）
        assert_eq!(find_sig(&r3, "ModX"), find_sig(&r4, "ModX"));

        // 深度扫描不填充签名（其产物不参与增量复用）
        let deep = scan_mods_deep(dir.path()).unwrap();
        assert!(
            deep.mods.iter().all(|m| m.disk_signature == 0),
            "深扫产物签名应为 0（无签名 → 永不复用）"
        );
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
        assert!(!is_group_xx_dir("group_")); // 无数字
        assert!(!is_group_xx_dir("group_abc")); // 非数字
        assert!(!is_group_xx_dir("Group_1")); // 大小写敏感
        assert!(!is_group_xx_dir("#MyGroup")); // mutexGroup
        assert!(!is_group_xx_dir("SubGroup")); // 自定义子分组
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
        assert!(
            !non_group.join("groupname").exists(),
            "非group不应创建 groupname 文件"
        );
        assert!(
            !mod_dir.join("modname").exists(),
            "非group下模组不应创建 modname 文件"
        );

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
        assert!(
            group1.join("groupname").exists(),
            "group_xx 应创建 groupname 文件"
        );
        assert!(
            mod_dir.join("modname").exists(),
            "group_xx 下模组应创建 modname 文件"
        );
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
        let result = scan_partial_path(dir.path(), Path::new("_MANAGED_/group_999")).unwrap();
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
        let result = scan_partial_path(dir.path(), Path::new("_MANAGED_/group_2")).unwrap();

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
        let result = scan_partial_path(dir.path(), Path::new("_MANAGED_/#CustomGroup")).unwrap();

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
        let result = scan_partial_path(dir.path(), Path::new("_MANAGED_/group_1/Mod1")).unwrap();

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
        assert_eq!(
            result, "group_1",
            "绝对路径应被剥离为相对于 _MANAGED_ 的子路径"
        );

        // 模组级绝对路径：mods_path/_MANAGED_/group_1/Mod1
        let abs_mod_path = g1.join("Mod1");
        let result2 = normalize_subpath(&abs_mod_path, dir.path());
        assert_eq!(result2, "group_1/Mod1", "模组级绝对路径应保留分组/模组结构");
    }
}

// ============================================================================
// scan_single_mod 单元测试（C-3a 单模组扫描）
// ============================================================================

#[cfg(test)]
mod tests_scan_single_mod {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// 测试辅助：创建带 `_MANAGED_` 子目录的临时 Mods 根
    fn setup() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("_MANAGED_")).unwrap();
        dir
    }

    /// 测试辅助：创建带 ini 的模组目录（可选 fav/modname 标记）
    fn make_mod(parent: &Path, name: &str) -> PathBuf {
        let p = parent.join(name);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("mod.ini"), "[Section]\nx=1\n").unwrap();
        p
    }

    /// 单模组扫描产出与组级扫描同口径的磁盘派生字段：
    /// 名称（modname 标记优先）、DISABLED 解析、fav、is_active（唯一启用者）、组定位。
    #[test]
    fn scan_single_mod_normal_group_fields() {
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "ModA");
        let mod_b = make_mod(&g1, "DISABLED_ModB");
        // modname 标记优先于目录名
        fs::write(mod_a.join("modname"), "My Display Name\n").unwrap();
        // fav 标记
        fs::write(mod_b.join(constants::FAV_MARKER), "").unwrap();

        // ModA：唯一启用模组 → is_active = true
        let (arc, group_dir) = scan_single_mod(dir.path(), &mod_a).unwrap();
        assert_eq!(group_dir, g1, "组目录应为 mod_dir.parent()");
        assert_eq!(arc.name, "My Display Name", "modname 标记内容优先");
        assert_eq!(arc.group_index, 1);
        assert!(!arc.is_mutex, "普通组模组 is_mutex=false");
        assert!(arc.is_active, "唯一启用模组应为 active");
        assert!(!arc.disabled && !arc.mod_disabled);
        assert!(!arc.is_favorite);
        assert!(arc.disk_signature != 0, "磁盘签名非 0（可参与复用判定）");
        assert_eq!(
            arc.mod_path,
            mod_a.to_string_lossy().to_string(),
            "full_path/mod_path 为归一（mods_path 基准）形态"
        );

        // ModB：禁用 + 收藏 + 非 active
        let (arc_b, _) = scan_single_mod(dir.path(), &mod_b).unwrap();
        assert!(arc_b.disabled && arc_b.mod_disabled, "DISABLED_ 前缀解析");
        assert_eq!(arc_b.name, "ModB", "禁用模组展示名剥离前缀");
        assert!(arc_b.is_favorite, "fav 标记存在");
        assert!(!arc_b.is_active, "禁用模组不 active");
    }

    /// modname 标记不存在时创建并回写目录名（NRMM 对齐）
    #[test]
    fn scan_single_mod_creates_modname_marker() {
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "FreshMod");
        assert!(!mod_a.join("modname").exists());

        let (arc, _) = scan_single_mod(dir.path(), &mod_a).unwrap();
        assert_eq!(arc.name, "FreshMod", "无 modname 时以目录名为展示名");
        assert!(
            mod_a.join("modname").exists(),
            "scan_single_mod 应创建 modname 标记（对齐组扫描语义）"
        );
    }

    /// is_active 组语义近似：多个启用模组（默认 selectedindex=0 → None 槽位 active）
    /// 时所有真实模组均非 active，与整组重扫派生一致
    #[test]
    fn scan_single_mod_is_active_with_multiple_enabled() {
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "ModA");
        let mod_b = make_mod(&g1, "ModB");

        let (arc_a, _) = scan_single_mod(dir.path(), &mod_a).unwrap();
        let (arc_b, _) = scan_single_mod(dir.path(), &mod_b).unwrap();
        assert!(
            !arc_a.is_active && !arc_b.is_active,
            "多启用（默认 selectedindex=0 → None active）时真实模组均非 active"
        );

        // 对照：整组重扫同口径（多启用回退 selectedindex=0 → None active）
        let group_scan = scan_partial_path(dir.path(), &g1).unwrap();
        let gm = &group_scan.groups[0];
        assert_eq!(gm.active_mod_index, 0, "整组重扫：None 槽位 active");
        assert!(
            gm.mods
                .iter()
                .filter(|m| m.name != "None")
                .all(|m| !m.is_active),
            "整组重扫：真实模组均非 active"
        );
    }

    /// 互斥组模组（含深层嵌套）：is_active 恒 false、is_mutex=true
    #[test]
    fn scan_single_mod_mutex_mod_always_inactive() {
        let dir = setup();
        let mutex_root = dir.path().join("_MANAGED_").join("#Mutex");
        let sub = mutex_root.join("Sub");
        let mod_x = make_mod(&sub, "ModX");
        let mod_root_level = make_mod(&mutex_root, "ModY");

        let (arc_x, group_dir_x) = scan_single_mod(dir.path(), &mod_x).unwrap();
        assert_eq!(group_dir_x, sub, "嵌套模组的所属组目录为其直接父目录");
        assert!(arc_x.is_mutex, "互斥组模组 is_mutex=true");
        assert!(!arc_x.is_active, "互斥组模组 is_active 恒 false");

        let (arc_y, group_dir_y) = scan_single_mod(dir.path(), &mod_root_level).unwrap();
        assert_eq!(group_dir_y, mutex_root);
        assert!(arc_y.is_mutex && !arc_y.is_active);
    }

    /// 非 `_MANAGED_` 子树路径 / 无 .ini 目录 → Err（调用方降级组级重扫）
    #[test]
    fn scan_single_mod_rejects_invalid_targets() {
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "ModA");

        // 不含 .ini 的目录
        let no_ini = g1.join("NoIni");
        fs::create_dir_all(&no_ini).unwrap();
        assert!(scan_single_mod(dir.path(), &no_ini).is_err());

        // _MANAGED_ 根自身
        assert!(scan_single_mod(dir.path(), &dir.path().join("_MANAGED_")).is_err());

        // 组外的任意路径（不在 _MANAGED_ 下）
        let outside = dir.path().join("Outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("x.ini"), "[S]\n").unwrap();
        assert!(scan_single_mod(dir.path(), &outside).is_err());

        // 合法模组目录不应报错
        assert!(scan_single_mod(dir.path(), &mod_a).is_ok());
    }

    /// 单模组扫描的磁盘派生字段与整组扫描产出一致（除 mod_index/is_active 占位外）：
    /// name/disabled/favorite/is_active/签名三源一致（正确性金标准的字段级子集）
    #[test]
    fn scan_single_mod_matches_group_scan_fields() {
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "ModA");
        make_mod(&g1, "DISABLED_ModB");

        let group_scan = scan_partial_path(dir.path(), &g1).unwrap();
        let group_mod_a = group_scan.groups[0]
            .mods
            .iter()
            .find(|m| m.name == "ModA")
            .unwrap()
            .clone();

        let (single, _) = scan_single_mod(dir.path(), &mod_a).unwrap();
        assert_eq!(single.name, group_mod_a.name);
        assert_eq!(single.disabled, group_mod_a.disabled);
        assert_eq!(single.mod_disabled, group_mod_a.mod_disabled);
        assert_eq!(single.is_favorite, group_mod_a.is_favorite);
        assert_eq!(single.is_namespaced, group_mod_a.is_namespaced);
        assert_eq!(single.is_mutex, group_mod_a.is_mutex);
        assert_eq!(single.group_index, group_mod_a.group_index);
        assert_eq!(
            single.full_path.as_ref(),
            group_mod_a.full_path.as_ref(),
            "full_path 与组扫描一致（缓存 patch 位置匹配前提）"
        );
        // 唯一启用者 → 两路径 is_active 均为 true
        assert_eq!(single.is_active, group_mod_a.is_active);
        // 签名一致（三源 mtime 相同）
        assert_eq!(single.disk_signature, group_mod_a.disk_signature);
    }
}
