//! 模组缓存路径比较/组判定域
//!
//! 提供路径等价（`\\?\` 前缀剥离 / Windows 大小写不敏感）、组节点定位与
//! 「缓存旧节点 vs 局部重扫产物」实质差异判定原语。由缓存 core 域的
//! subtree 合并、patch 域与命令级 delta 组装共用。

use crate::core::mod_cache_core::MOD_CACHE;
use crate::core::mod_scan_light::ScanResult;
use crate::models::enums::TargetGame;
use crate::models::mod_data::ModGroupData;
use std::path::Path;

/// 路径字符串等价比较（Windows 下大小写不敏感，其他平台精确比较）。
///
/// 用于增量合并时按路径原位识别分组/模组，避免大小写路径差异产生重复条目。
/// 语义对齐 `mod_manager::path_eq_ignore_case`。
pub(crate) fn path_string_eq(a: &str, b: &str) -> bool {
    #[cfg(windows)]
    {
        a.to_lowercase() == b.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// 路径比较键：统一分隔符为 `/`、剥离 Windows verbatim 前缀（`\\?\`），
/// Windows 下再转小写实现大小写不敏感比较。
///
/// 命令层路径经 `canonicalize` 后携带 verbatim 前缀，而缓存中的
/// `full_path` / `group_path` 保持 `get_mods` 入参的普通形态；
/// 统一归一后再比较，避免形态差异导致匹配失败。
pub(crate) fn path_cmp_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").map(str::to_string).unwrap_or(s);
    #[cfg(windows)]
    {
        s.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        s
    }
}

/// 路径等价判断（语义同 [`path_string_eq`]，额外容忍 verbatim 前缀差异）。
///
/// # 参数
/// - `a`/`b`: 待比较的两个路径。
///
/// # 返回
///
/// 两路径指向同一位置时返回 `true`。
pub(crate) fn path_equivalent(a: &Path, b: &Path) -> bool {
    path_cmp_key(a) == path_cmp_key(b)
}

/// 判断 `child` 是否位于 `ancestor` 目录之下（含相等；比较口径同 [`path_equivalent`]）。
///
/// 命令级 delta 组装用它在新分组节点中按 `full_path` 前缀匹配局部重扫入口目录。
///
/// # 参数
/// - `child`: 子路径。
/// - `ancestor`: 祖先目录路径。
///
/// # 返回
///
/// `child` 等于 `ancestor` 或位于其下（以路径分隔符为界，避免 `group_1` 误匹配 `group_10`）时返回 `true`。
pub fn path_within(child: &Path, ancestor: &Path) -> bool {
    let (c, a) = (path_cmp_key(child), path_cmp_key(ancestor));
    c == a || c.starts_with(&format!("{a}/"))
}

/// 在扫描结果顶层分组中按完整路径定位分组节点。
///
/// 仅匹配顶层（`_MANAGED_` 一级子目录）：命令级 delta 的粒度恒为顶层分组
/// （`scan_partial_path` 亦收敛到该粒度），限制在顶层可避免「旧收集范围与
/// 新分组范围不一致」的偏差。
///
/// # 参数
/// - `result`: 扫描结果（缓存或局部重扫产物）。
/// - `group_full_path`: 分组完整目录路径（容忍 verbatim 前缀 / 大小写形态差异）。
///
/// # 返回
///
/// 匹配的分组节点引用；顶层无匹配时返回 `None`。
pub fn find_group_node<'a>(
    result: &'a ScanResult,
    group_full_path: &Path,
) -> Option<&'a ModGroupData> {
    result
        .groups
        .iter()
        .find(|g| path_equivalent(&g.full_path, group_full_path))
}

/// 判断局部重扫产物中的分组与缓存旧节点是否存在**实质差异**（P1 修复：
/// 全量 partial 的 delta 过滤依据）。
///
/// 读锁内定位缓存旧组节点后逐层比较：
/// - 组头派生字段：`mod_count` / `is_active` / `has_child` /
///   `active_mod_index` / `group_disabled` / `is_favorite`；
/// - 组内条目：数量 + 逐条（`full_path` 等价、`disk_signature` 三源 mtime
///   签名、`is_active` / `disabled` / `mod_disabled` 状态）；
/// - `children` 递归同款（数量 + 按 `full_path` 匹配 + 递归比较）。
///
/// 签名相等 ⇔ 磁盘派生状态一致（C-3b 语义）；缓存无该组节点视为新组（有变化）。
/// 全量 partial 中与缓存一致的组由此跳过 delta emit，前端不再收到 109 组全量
/// applyGroupDelta（实测 2 轮 × 110 delta 的 UI 重排抖动来源）。
///
/// # 参数
/// - `game`: 目标游戏（缓存键）。
/// - `mods_path`: Mods 根目录（缓存键）。
/// - `new_group`: 局部重扫产物中的组节点。
///
/// # 返回
/// `true` = 有实质差异（应 emit delta）；`false` = 与缓存一致（可跳过）。
pub fn group_has_real_change(game: TargetGame, mods_path: &Path, new_group: &ModGroupData) -> bool {
    let cache = MOD_CACHE.read();
    match cache
        .get(game, mods_path)
        .as_ref()
        .and_then(|scan| find_group_node(scan, &new_group.full_path))
    {
        None => true,
        Some(old) => !group_node_equivalent(new_group, old),
    }
}

/// [`group_has_real_change`] 的递归比较实现。
fn group_node_equivalent(n: &ModGroupData, o: &ModGroupData) -> bool {
    if n.mod_count != o.mod_count
        || n.is_active != o.is_active
        || n.has_child != o.has_child
        || n.active_mod_index != o.active_mod_index
        || n.group_disabled != o.group_disabled
        || n.is_favorite != o.is_favorite
    {
        return false;
    }
    if n.mods.len() != o.mods.len() {
        return false;
    }
    for (nm, om) in n.mods.iter().zip(o.mods.iter()) {
        if !path_string_eq(nm.full_path.as_ref(), om.full_path.as_ref())
            || nm.disk_signature != om.disk_signature
            || nm.is_active != om.is_active
            || nm.disabled != om.disabled
            || nm.mod_disabled != om.mod_disabled
        {
            return false;
        }
    }
    if n.children.len() != o.children.len() {
        return false;
    }
    for nc in &n.children {
        match o
            .children
            .iter()
            .find(|oc| path_equivalent(&oc.full_path, &nc.full_path))
        {
            Some(oc) => {
                if !group_node_equivalent(nc, oc) {
                    return false;
                }
            }
            None => return false,
        }
    }
    true
}

#[cfg(test)]
#[path = "mod_cache_path_tests.rs"]
mod tests;
