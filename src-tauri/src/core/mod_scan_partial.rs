//! 局部扫描域：scan_partial_path / normalize_subpath——增量更新的分组级局部重扫
//!
//! 从 mod_scanner.rs 拆出（2026-09-19 Phase 5-6）。
//! 依赖 light（scan_mods_light/scan_normal_group_light/ScanResult）与 mutex
//! （scan_mutex_group_dfs）；被 mod_cache / file_watcher / mod_commands 消费。

use anyhow::Result;
use std::path::Path;

use crate::core::fs_walk::TraversalStats;
use crate::core::mod_scan_light::{scan_mods_light, scan_normal_group_light, ScanResult};
use crate::core::mod_scan_mutex::scan_mutex_group_dfs;
use crate::core::mod_scan_util::{get_managed_folder, is_normal_group_dir};

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
        // 目标路径不存在 → 全量兜底（防御路由层 exists 检查后的竞态删除）。
        // P1 修复后正常批次不会走到这里：模组级消失根在 snap 保留身份后由
        // 路由层删除 patch 处理；组级消失根由 route_incremental_batch 的
        // subtree_remove 删除语义处理。命中此日志即说明出现未覆盖的消失路径
        // 形态，需按日志排查。
        log::warn!(
            "[scan_partial_path] 目标路径不存在 → 全量兜底 {}（P1 后不应常态命中，请排查）",
            target_subpath.display()
        );
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
pub(crate) fn normalize_subpath(p: &Path, mods_path: &Path) -> String {
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

#[cfg(test)]
#[path = "mod_scan_partial_tests.rs"]
mod tests;
