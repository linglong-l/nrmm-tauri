//! 模组扫描公共基础域：正则/常量、目录判定、标记/图标工具、排序与计数真源
//!
//! 从 mod_scanner.rs 拆出（2026-09-19 Phase 5-1）。
//! 零扫描域依赖（仅 constants/模型）；被 light/mutex/deep/partial 四域与
//! mod_cache（derive/计数/分组排序）消费。

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use std::cmp::Ordering as CmpOrdering;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::core::constants;
use crate::models::enums::GroupType;
use crate::models::mod_data::{ModData, ModGroupData};

/// 支持的图标文件扩展名列表
///
/// 用于 `find_icon_path()` 和 `check_directory_for_mod_deep()` 中过滤图片文件。
/// 涵盖常见 Web 和桌面图片格式：PNG、JPEG、GIF、BMP、WebP、ICO。
/// 匹配时不区分大小写（统一转小写后比较）。
pub(crate) static ICON_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico"];

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
pub(crate) static DISABLED_PREFIX_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(?i:disabled)[_\- ]*").unwrap());

/// 获取 `_MANAGED_` 文件夹路径
///
/// 将 `game_mods_path` 与 `constants::MANAGED_FOLDER` 拼接，返回完整的 Managed 目录路径。
/// 所有模组文件都存放在此目录下。
pub fn get_managed_folder(game_mods_path: &Path) -> PathBuf {
    game_mods_path.join(constants::MANAGED_FOLDER)
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
pub(crate) fn create_empty_slot_mod(group_index: u32) -> ModData {
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
pub(crate) fn metadata_mtime_nanos(meta: Option<&fs::Metadata>) -> u64 {
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
pub(crate) fn compute_disk_signature(
    dir_nanos: u64,
    fav_nanos: u64,
    group_selectedindex_nanos: u64,
) -> u64 {
    if dir_nanos == 0 {
        return 0;
    }
    dir_nanos.rotate_left(32) ^ fav_nanos.rotate_left(16) ^ group_selectedindex_nanos
}

/// 统一重算模组计数，口径与全量轻量扫描一致。
///
/// 排除 `None` 空槽位（`name == "None"`），以 `!disabled && !mod_disabled` 判定启用。
/// 供全量扫描与增量合并（`mod_cache_patch::subtree_replace`）共用，保证两处计数一致。
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
/// 供全量扫描与增量合并（`mod_cache_patch::subtree_replace`）共用分组排序规则。
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
pub(crate) fn mod_order_cmp(a: &ModData, b: &ModData) -> CmpOrdering {
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

#[cfg(test)]
#[path = "mod_scan_util_tests.rs"]
mod tests;
