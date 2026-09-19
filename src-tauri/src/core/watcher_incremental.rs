//! 文件监控器 —— 增量根分类 / 合并 / 批次路由域
//!
//! 从 file_watcher.rs 拆出（2026-09-19 剩余巨石拆分 PA-2）。
//! 承载：溢出分层分类（`classify_overflow_roots`）、批次世代校验
//! （`is_generation_stale`）、增量根合并（`consolidate_incremental_roots`）、
//! 双粒度路由（`route_incremental_batch`）及其辅助（模组级判定 / 顶层组推导 /
//! 批次聚合 / partial 合并组装 delta）。
//! 事件环形缓冲/暂停守卫见 [`crate::core::watcher_ring`]，事件发射见
//! [`crate::core::watcher_emit`]，FileWatcher 主体见 [`crate::core::watcher_core`]。

use crate::core::constants;
use crate::models::mod_data::{GroupDelta, ModData};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 溢出分层分类（2026-08-19 用户指定的降低全量更新时间策略）。
///
/// 将脏目录合并到 `_MANAGED_` 一级子目录粒度，按类型分层返回需全量扫描的根目录：
/// 1. 仅涉及 group 目录（单个或多个）→ 涉及的各 `group_<int>` 分组全量；
/// 2. 仅涉及 mutex 目录 → 涉及的各 mutex 目录全量；
/// 3. group/mutex 混合、脏目录位于 `_MANAGED_` 之外或即 `_MANAGED_` 根自身
///    → 返回空 Vec（调用方走 `invalidate_game` + 前端 loadMods 全量兜底）。
///
/// # 参数
/// - `dirty`: drain 出的脏目录列表
/// - `managed_path`: `_MANAGED_` 目录路径
///
/// # 返回
/// 需全量扫描的根目录列表；空列表表示应走全量兜底
pub(crate) fn classify_overflow_roots(dirty: &[PathBuf], managed_path: &Path) -> Vec<PathBuf> {
    let mut group_roots: Vec<PathBuf> = Vec::new();
    let mut mutex_roots: Vec<PathBuf> = Vec::new();
    // 是否存在无法归类到 _MANAGED_ 一级子目录的脏路径（外部门录 / _MANAGED_ 根自身）
    let mut unclassified = false;

    for dir in dirty {
        match dir.strip_prefix(managed_path) {
            Ok(rel) => match rel.components().next() {
                Some(first) => {
                    let name = first.as_os_str().to_string_lossy().to_string();
                    let root = managed_path.join(first);
                    if crate::core::mod_scan_util::is_normal_group_dir(&name).is_some() {
                        if !group_roots.contains(&root) {
                            group_roots.push(root);
                        }
                    } else if !mutex_roots.contains(&root) {
                        mutex_roots.push(root);
                    }
                }
                None => unclassified = true, // rel 为空：dirty 即 _MANAGED_ 根自身
            },
            Err(_) => unclassified = true, // _MANAGED_ 之外的变更
        }
    }

    if unclassified || (!group_roots.is_empty() && !mutex_roots.is_empty()) {
        Vec::new()
    } else if !group_roots.is_empty() {
        group_roots
    } else {
        mutex_roots
    }
}

/// 判断批次世代是否已过期（C4 世代校验）
///
/// `stored_gen` 为防抖批次捕获创建时的世代快照，`current_gen` 为当前世代计数。
/// 当两者不一致时说明批次属于旧监听（期间 `start_watching` 再次被调用、世代已递增，
/// 例如切换游戏），静默丢弃整批，防止 `subtree_replace` 把旧路径数据合入新游戏、
/// 或 emit 跨游戏事件。
pub(crate) fn is_generation_stale(stored_gen: u64, current_gen: u64) -> bool {
    stored_gen != current_gen
}

/// 未溢出路径的精确增量合并：脏目录经 [`IncrementalUpdater::consolidate`] 合并根路径
/// （复用既有算法：规范化、共同前缀、向上收敛到 group/mod 目录粒度）。
///
/// # 参数
/// - `dirty`: drain 出的脏目录列表
/// - `managed_path`: `_MANAGED_` 目录路径
///
/// # 返回
/// 增量扫描根路径列表（可能为空，如全部路径被合并规则归约）
pub(crate) fn consolidate_incremental_roots(
    dirty: &[PathBuf],
    managed_path: &Path,
) -> Vec<PathBuf> {
    let mut updater = crate::core::incremental_updater::IncrementalUpdater::new(
        constants::FILE_WATCHER_DEBOUNCE_MS,
    );
    for dir in dirty {
        updater.collect(dir.clone());
    }
    updater.consolidate(managed_path)
}

/// 增量分支 delta 组装 + 缓存合并（R2）：对每个局部重扫产物，
/// 合并前（读锁）收集各组旧状态 → 写锁 `subtree_replace` 就地合并 → 组装
/// [`GroupDelta`]（new_group 为该组重扫后的完整新节点）。
///
/// 逐 partial 顺序执行（而非先全量收集旧状态再统一合并）：多个 partial
/// 收敛到同一分组时（consolidate 的发散分支），后续 partial 的旧状态
/// 反映前一个已合并的缓存，前端连续 apply 不产生扁平重复条目。
///
/// # 参数
/// - `game`: 目标游戏（缓存键）
/// - `mods_path`: Mods 根目录
/// - `scan_results`: 并行局部重扫产物（root, partial）列表
///
/// # 返回
/// 本批次全部受影响分组的 [`GroupDelta`] 列表（partial 内每组一个）
fn merge_partials_and_build_deltas(
    game: crate::models::enums::TargetGame,
    mods_path: &Path,
    scan_results: &[(PathBuf, crate::core::mod_scan_light::ScanResult)],
) -> Vec<GroupDelta> {
    let mut deltas = Vec::new();
    let mut skipped = 0usize;
    for (_root, partial) in scan_results {
        // a. 合并前（读锁）收集该 partial 各组旧状态（复用命令层共享的
        //    mod_cache_delta::collect_old_group_state；缓存 miss 时 old 为空、键回退目录路径串）
        for g in &partial.groups {
            // P1 修复（全量 partial 差异过滤）：与缓存无实质差异的组跳过 delta emit。
            // 全量兜底 partial 携带全部 109 组，无条件 emit 曾导致前端
            // applyGroupDelta ×110/轮（两轮 220 次）的选择后 UI 重排抖动；
            // 过滤后仅实际变化的组到达前端（缓存照常 subtree_replace，签名
            // 复用零拷贝，语义不变）。判定读锁短持有、O(组内) 比较，总 O(全树)。
            if !crate::core::mod_cache_path::group_has_real_change(game, mods_path, g) {
                skipped += 1;
                continue;
            }
            let (old_group_path, old_mod_paths) =
                crate::core::mod_cache_delta::collect_old_group_state(
                    game,
                    mods_path,
                    &g.full_path,
                );
            // c. delta：new_group 为该组重扫后的完整新节点（磁盘为唯一真相源）
            deltas.push(GroupDelta {
                old_group_path,
                new_group: Some(g.clone()),
                old_mod_paths,
            });
        }
        // b. 写锁 subtree_replace 就地合并缓存（现状逻辑；缓存 miss 时内部跳过）
        crate::core::mod_cache_core::MOD_CACHE
            .write()
            .subtree_replace(game, mods_path, partial.clone());
    }
    if skipped > 0 {
        log::info!(
            "[watcher] delta 差异过滤 | 跳过无变化组={} 实际emit={}",
            skipped,
            deltas.len()
        );
    }
    deltas
}

/// 路径段键（C-3a 私有）：Windows 大小写不敏感比较用（其他平台原样）。
fn seg_key(s: &str) -> String {
    #[cfg(windows)]
    {
        s.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        s.to_string()
    }
}

/// 路径段切分（C-3a 私有）：统一分隔符、剥 Windows verbatim 前缀（`\\?\`）、
/// 保留**原大小写**（`group_xx` 判定对大小写敏感，需对齐 snap/扫描口径）。
fn path_segments_original_case(p: &Path) -> Vec<String> {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").map(str::to_string).unwrap_or(s);
    s.split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .map(str::to_string)
        .collect()
}

/// 计算收敛根相对 `_MANAGED_` 的路径段列表（C-3a 粒度路由依据）。
///
/// 收敛根来自 `IncrementalUpdater::consolidate` 的 canonicalize 产物（Windows 为
/// `\\?\` verbatim 形态），与 watcher 内部的普通形态 managed_path 经段键比较统一。
///
/// # 返回
/// `Some(segs)`：根在 `_MANAGED_` 子树内（根即 `_MANAGED_` 自身时为空段列表）；
/// `None`：根不在 `_MANAGED_` 下。
fn managed_relative_segments(root: &Path, managed_path: &Path) -> Option<Vec<String>> {
    let root_segs = path_segments_original_case(root);
    let managed_segs = path_segments_original_case(managed_path);
    if root_segs.len() < managed_segs.len() {
        return None;
    }
    for (a, b) in root_segs[..managed_segs.len()].iter().zip(&managed_segs) {
        if seg_key(a) != seg_key(b) {
            return None;
        }
    }
    Some(root_segs[managed_segs.len()..].to_vec())
}

/// C-3a 粒度路由判定：收敛根是否为模组级。
///
/// 与 `snap_to_group_or_mod_boundary` 语义一致：**组级根** = `group_xx` 目录或
/// `_MANAGED_` 一级子目录（mutex 顶层根，含根即模组的形态）；**模组级根** = 其下
/// 含 `.ini` 的模组目录（snap 保证深层非 `.ini` 目录不会成为根——含 `.ini` 判定在
/// 收敛时完成，此处的结构判定：相对深度 ≥ 2 且末段非 `group_xx` 目录名）。
///
/// # 参数
/// - `root`: 收敛根路径（任意形态，容忍 verbatim/大小写差异）。
/// - `managed_path`: `_MANAGED_` 目录路径。
fn is_mod_level_root(root: &Path, managed_path: &Path) -> bool {
    managed_relative_segments(root, managed_path).is_some_and(|segs| {
        segs.len() >= 2
            && segs
                .last()
                .is_some_and(|last| !crate::core::mod_scan_util::is_group_xx_dir(last))
    })
}

/// 取模组级根所属的顶层组目录（C-3a）：delta 旧状态收集与批次归并键
/// （`_MANAGED_` 一级子目录形态；路径等价比较容忍大小写形态差异）。
fn top_level_group_dir(root: &Path, managed_path: &Path) -> Option<PathBuf> {
    let segs = managed_relative_segments(root, managed_path)?;
    let first = segs.first()?;
    Some(managed_path.join(first))
}

/// 组级根并行局部重扫（现状路径提炼，C-3a 后溢出与组级路由共用）。
///
/// rayon 并行执行 `scan_partial_path`（纯读磁盘+解析，锁外执行）；单个根失败仅
/// 跳过该根（记日志），不中断其余。根不存在（整组目录被删除）→ 失效该游戏缓存
/// （全量兜底——`scan_partial_path` 会退化为全量扫描，且缓存会残留已删分组）。
pub(crate) fn scan_group_roots(
    game: crate::models::enums::TargetGame,
    mods_path: &Path,
    roots: &[PathBuf],
) -> Vec<(PathBuf, crate::core::mod_scan_light::ScanResult)> {
    roots
        .par_iter()
        .filter_map(|sub| {
            if !sub.exists() {
                log::info!(
                    "[Incremental] scan root no longer exists, invalidating game cache: {:?}",
                    sub
                );
                let mut cache_guard = crate::core::mod_cache_core::MOD_CACHE.write();
                cache_guard.invalidate_game(game);
                return None;
            }
            match crate::core::mod_scan_partial::scan_partial_path(mods_path, sub) {
                Ok(partial) => Some((sub.clone(), partial)),
                Err(e) => {
                    log::error!(
                        "[Incremental] scan_partial_path failed for {}: {}",
                        sub.display(),
                        e
                    );
                    None
                }
            }
        })
        .collect()
}

/// 模组级批次条目：`(模组根路径, 新模组数据)`（`None` = 删除 patch）。
type ModPatchItem = (PathBuf, Option<Arc<ModData>>);

/// 模组级批次按顶层组归并后统一 patch + 组装 delta（C-3a）。
///
/// 同批次多个模组级根落同一顶层组 → 合并为**一次**组 delta（一次旧状态收集 +
/// 一次写锁批次 + 单个 new_group），避免重复 make_mut/重复 delta。批次内逐模组
/// `patch_single_mod`（单模组扫描产物或删除 patch）；全部未命中（缓存 miss/组节点
/// 无定位）时该组不产出 delta，未命中根交还调用方降级组级重扫。
///
/// # 参数
/// - `game`: 目标游戏（缓存键）。
/// - `mods_path`: Mods 根目录。
/// - `managed_path`: `_MANAGED_` 目录路径（顶层组推导）。
/// - `batch`: `(模组根路径, 新模组数据)` 列表（`None` = 删除 patch）。
///
/// # 返回
/// `(该批次产出的 GroupDelta 列表, 归并后的顶层组数, 未命中的根列表)`
fn patch_mod_batch_and_build_deltas(
    game: crate::models::enums::TargetGame,
    mods_path: &Path,
    managed_path: &Path,
    batch: Vec<ModPatchItem>,
) -> (Vec<GroupDelta>, usize, Vec<PathBuf>) {
    if batch.is_empty() {
        return (Vec::new(), 0, Vec::new());
    }
    // 按顶层组归并（保持批次内出现顺序，组内逐模组顺序稳定）
    let mut order: Vec<PathBuf> = Vec::new();
    let mut buckets: HashMap<PathBuf, Vec<ModPatchItem>> = HashMap::new();
    for (root, new_mod) in batch {
        let Some(top) = top_level_group_dir(&root, managed_path) else {
            log::warn!("[Incremental] mod 级根不在 _MANAGED_ 下，跳过: {:?}", root);
            continue;
        };
        if !buckets.contains_key(&top) {
            order.push(top.clone());
        }
        buckets.entry(top).or_default().push((root, new_mod));
    }

    let mut deltas = Vec::new();
    let mut missed: Vec<PathBuf> = Vec::new();
    let merged_groups = order.len();
    for top in &order {
        let items = buckets.remove(top).unwrap_or_default();
        // 读锁收集旧状态（缓存 miss 时 old 为空、键回退目录路径串）
        let (old_group_path, old_mod_paths) =
            crate::core::mod_cache_delta::collect_old_group_state(game, mods_path, top);
        // 写锁逐模组 patch（同组批次一次锁定；返回顶层组节点含定位链重建结果）
        let mut final_node: Option<crate::models::mod_data::ModGroupData> = None;
        {
            let mut cache = crate::core::mod_cache_core::MOD_CACHE.write();
            for (root, new_mod) in items {
                match cache.patch_single_mod(game, mods_path, &root, new_mod) {
                    Some(node) => final_node = Some(node),
                    None => {
                        log::warn!(
                            "[Incremental] mod 级 patch 未命中（缓存 miss/组节点无定位），降级组级重扫: {:?}",
                            root
                        );
                        missed.push(root);
                    }
                }
            }
        }
        // delta：new_group 为该组 patch 后的完整顶层节点（与组级 partial 同形状，
        // 前端 applyGroupDelta 无感）；全部未命中时缓存未变更，不产出
        if let Some(node) = final_node {
            deltas.push(GroupDelta {
                old_group_path,
                new_group: Some(node),
                old_mod_paths,
            });
        }
    }
    (deltas, merged_groups, missed)
}

/// 增量批次双粒度路由（R3/C-3a）：深部单文件变更（如 `group_2/ModA/textures/x.png`）
/// 的收敛根从「整组重扫」降为「单模组重扫 + 组内位置 patch + 兄弟 Arc 复用」。
///
/// - **模组级根**（`is_mod_level_root`）：存在 → `scan_single_mod` 单模组扫描（扫描
///   失败降级组级）；不存在（consolidate 后被删的竞态窗口）→ 删除 patch。
/// - **组级根**（group_xx / mutex 顶层根）：维持现状——并行 `scan_partial_path`
///   整组重扫 + `subtree_replace`（含 R2 delta 组装）。
/// - **批次聚合**：模组级根按顶层组归并，同组一次 delta（`patch_mod_batch_and_build_deltas`）。
/// - **降级兜底**：单模组扫描失败或 patch 未命中（如普通组内嵌套目录——轻扫不递归
///   NormalGroup，深层路径在缓存树中无模组定位）的根回退组级重扫（现状路径）。
///
/// delta 顺序与缓存合并顺序一致（组级 → 模组级 → 降级组级），前端按序 apply 的
/// 终态 = 最后一次合并的缓存状态。
///
/// # 返回
/// 本批次全部受影响分组的 [`GroupDelta`] 列表。
pub(crate) fn route_incremental_batch(
    game: crate::models::enums::TargetGame,
    mods_path: &Path,
    managed_path: &Path,
    scan_roots: &[PathBuf],
) -> Vec<GroupDelta> {
    let mut group_roots: Vec<PathBuf> = Vec::new();
    let mut mod_batch: Vec<ModPatchItem> = Vec::new();
    let mut mod_roots_total = 0usize;

    for root in scan_roots {
        if is_mod_level_root(root, managed_path) {
            mod_roots_total += 1;
            if root.exists() {
                match crate::core::mod_scan_light::scan_single_mod(mods_path, root) {
                    Ok((arc, _group_dir)) => mod_batch.push((root.clone(), Some(arc))),
                    Err(e) => {
                        log::warn!(
                            "[Incremental] scan_single_mod failed for {} (降级组级重扫): {}",
                            root.display(),
                            e
                        );
                        group_roots.push(root.clone());
                    }
                }
            } else {
                // 模组级目录被删除（consolidate 后竞态窗口）→ 删除 patch；
                // patch 未命中时降级组级（组级根不存在 → invalidate_game 兜底）
                mod_batch.push((root.clone(), None));
            }
        } else {
            group_roots.push(root.clone());
        }
    }

    // 组级根：已消失 → 删除语义（P1 修复）；存在 → 现状并行整组重扫 + subtree_replace
    //
    // P1 修复：整组被删/整组重命名的收敛根（组级）已不存在时，`scan_partial_path`
    // 的兜底是全量扫描（6542 模组 × 1.1s）——改为读旧状态 + `subtree_remove`
    // 裁剪缓存 + 组装 `new_group: None` 删除 delta（remove_group 命令同契约，
    // 前端 applyGroupDelta 已支持删除语义）。
    let mut vanished_deltas: Vec<GroupDelta> = Vec::new();
    let mut live_group_roots: Vec<PathBuf> = Vec::new();
    for r in &group_roots {
        if r.exists() {
            live_group_roots.push(r.clone());
        } else {
            let (old_group_path, old_mod_paths) =
                crate::core::mod_cache_delta::collect_old_group_state(game, mods_path, r);
            let removed = crate::core::mod_cache_core::MOD_CACHE
                .write()
                .subtree_remove(game, mods_path, r);
            log::info!(
                "[Incremental] 组级根已消失 → 删除语义 {} (裁剪 {} 条)",
                r.display(),
                removed
            );
            vanished_deltas.push(GroupDelta {
                old_group_path,
                new_group: None,
                old_mod_paths,
            });
        }
    }
    let scan_results = scan_group_roots(game, mods_path, &live_group_roots);
    let mut deltas = merge_partials_and_build_deltas(game, mods_path, &scan_results);
    deltas.extend(vanished_deltas);

    // 模组级批次：按顶层组归并后统一 patch + delta（未命中根收集降级）
    let (mod_deltas, merged_groups, missed_roots) = patch_mod_batch_and_build_deltas(
        game,
        mods_path,
        managed_path,
        std::mem::take(&mut mod_batch),
    );
    deltas.extend(mod_deltas);

    // 降级兜底：patch 未命中的根走组级重扫（现状路径；scan_partial_path 自动扩展
    // 到所属整组），保证这些根与变更前的行为一致（不漏更新）
    if !missed_roots.is_empty() {
        let fallback_results = scan_group_roots(game, mods_path, &missed_roots);
        let fallback_deltas = merge_partials_and_build_deltas(game, mods_path, &fallback_results);
        deltas.extend(fallback_deltas);
    }

    // 埋点（批次级，与 R2 静默期埋点并列）
    log::debug!(
        "[watcher] mod 级 patch | mod_roots={} group_roots={} (live={} vanished={}) 合并组={}",
        mod_roots_total,
        group_roots.len(),
        live_group_roots.len(),
        group_roots.len() - live_group_roots.len(),
        merged_groups
    );
    deltas
}

#[cfg(test)]
#[path = "watcher_incremental_tests.rs"]
mod tests;
