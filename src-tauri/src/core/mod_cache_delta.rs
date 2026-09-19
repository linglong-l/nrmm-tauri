//! 模组缓存旧分组状态收集域
//!
//! 命令级 delta（R1）与 watcher 增量 delta（R2）共用：从缓存收集旧分组路径与
//! 组内全部真实模组的 mod_path，供组装 `GroupDelta` 的 old_group_path / old_mod_paths。

use crate::core::mod_cache_core::MOD_CACHE;
use crate::core::mod_cache_path::find_group_node;
use crate::models::enums::TargetGame;
use crate::models::mod_data::ModGroupData;
use std::path::Path;

/// 从缓存收集旧分组状态（old_group_path + 组内全部真实模组的 mod_path）。
///
/// 读锁内定位分组节点（路径等价比较，容忍 verbatim 前缀 / 大小写形态差异）：
/// 命中时返回缓存节点的 `group_path`（前端树定位键原值）与组子树内全部
/// 真实模组的 mod_path（递归 mutex children、排除 None 空槽位）；
/// 缓存未命中或节点不存在时 mod_path 列表为空、分组键回退为目录路径串。
///
/// 命令级 delta（R1）与 watcher 增量 delta（R2）共用的旧状态收集入口，
/// 供命令层（mod_commands）与监听层（file_watcher）组装
/// [`crate::models::mod_data::GroupDelta`] 的 old_group_path / old_mod_paths。
///
/// # 参数
/// - `game`: 目标游戏（缓存键）。
/// - `mods_path`: Mods 根目录。
/// - `old_group_dir`: 旧分组目录路径。
///
/// # 返回
///
/// `(old_group_path, old_mod_paths)` 二元组。
pub fn collect_old_group_state(
    game: TargetGame,
    mods_path: &Path,
    old_group_dir: &Path,
) -> (String, Vec<String>) {
    let cache = MOD_CACHE.read();
    match cache
        .get(game, mods_path)
        .as_ref()
        .and_then(|scan| find_group_node(scan, old_group_dir))
    {
        Some(node) => (node.group_path.clone(), collect_group_mod_paths(node)),
        None => (old_group_dir.to_string_lossy().to_string(), Vec::new()),
    }
}

/// 收集分组子树内全部真实模组的 mod_path 列表。
///
/// 递归遍历 `mods` 与 `children`（嵌套 mutex 子分组），排除 `None` 空槽位
/// （`name == "None"` 或 `mod_path == "None"`）。用于命令级 delta 的
/// `old_mod_paths`（前端从扁平 mods 列表剔除旧条目）与 `affected_mods` 埋点统计。
///
/// # 参数
/// - `group`: 分组节点（含子树）。
///
/// # 返回
///
/// 该子树内全部真实模组的 `mod_path`，按树序返回。
pub fn collect_group_mod_paths(group: &ModGroupData) -> Vec<String> {
    let mut out = Vec::new();
    collect_mod_paths_inner(group, &mut out);
    out
}

/// [`collect_group_mod_paths`] 的递归实现。
fn collect_mod_paths_inner(group: &ModGroupData, out: &mut Vec<String>) {
    for m in &group.mods {
        if m.name != "None" && m.mod_path != "None" {
            out.push(m.mod_path.clone());
        }
    }
    for child in &group.children {
        collect_mod_paths_inner(child, out);
    }
}

#[cfg(test)]
#[path = "mod_cache_delta_tests.rs"]
mod tests;
