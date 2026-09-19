//! update_mod_data 底层：分组/模组级并行处理与 panic 兜底
//!
//! 从 mod_manager.rs 拆出（2026-09-19 Phase 4-9）。
//! 仅供 mod_update_pipeline 调用；依赖 ini 缓存/namespace_handler/constants。

use rayon::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::core::constants;
use crate::core::mod_ini_admin::ensure_ini_utf8;
use crate::core::mod_ini_cache::get_or_parse_ini;
use crate::core::namespace_handler;
use crate::models::mod_data::{ErroredLines, ModData};

/// 单个分组（group）并行任务的处理产出。
///
/// `update_mod_data` 步骤 5 按 `group_index` 分桶后，每个分组任务在单一 worker 线程内
/// 串行处理本组全部模组并产出本结构；全部完成后按 `group_id` 升序确定性合并，
/// 保证同一输入多次运行的 `errors` 顺序与 `mod_inis` 内容确定。
#[derive(Debug, Default)]
pub(crate) struct GroupOutcome {
    /// 分组 id（group_index）
    pub(crate) group_id: u32,
    /// 本组成功处理（已原子写入）的模组 INI 路径（组内保持扫描顺序）
    pub(crate) mod_inis: Vec<PathBuf>,
    /// 本组收集到的错误条目（解析/检测/写入失败，组内序保持）
    pub(crate) errors: Vec<ErroredLines>,
    /// 本组实际处理（INI 注入并写入成功）的计数
    pub(crate) processed: u32,
    /// 本组实际写盘（内容有变化）的 INI 计数
    pub(crate) written: u32,
    /// 本组因内容未变化跳过写盘的 INI 计数
    pub(crate) skipped: u32,
    /// 本组任务耗时（毫秒），由 panic 兜底包装器统一记录
    pub(crate) elapsed_ms: u128,
}

/// 单个模组（组内并行任务粒度）的处理产出。
///
/// `process_group_task` 按「每模组一任务」并行处理组内模组，各任务产出本结构，
/// 全部完成后按模组扫描顺序确定性合并进 [`GroupOutcome`]，
/// 保证与串行版的 `mod_inis`/`errors` 顺序完全一致。
#[derive(Debug, Default)]
pub(crate) struct ModPerOutcome {
    /// 本模组成功处理（已原子写入）的 INI 路径（保持 mod_ini_data 顺序）
    mod_inis: Vec<PathBuf>,
    /// 本模组收集到的错误条目
    errors: Vec<ErroredLines>,
    /// 本模组实际处理（注入并写入成功）的 INI 计数
    processed: u32,
    /// 本模组实际写盘（内容有变化）的 INI 计数
    written: u32,
    /// 本模组因内容未变化跳过写盘（幂等快速路径）的 INI 计数
    skipped: u32,
}

/// 处理单个模组的全部 INI（组内并行任务体，语义与原串行版逐模组循环完全一致）。
///
/// 管线：备份 → 解析（mod_ini_cache）→ 错误检测 → namespace 展开 → 清理旧托管内容 →
/// 补齐段属性键 → 重排 → 注入槽位条件 → 注释崩溃行 → 清理空 if 块 → 缩进 → 头注释 → 原子写入 → 标记文件。
/// `write_atomic` 失败不中止整体，收集为错误条目（`error_type=3`），该 INI 不计入 `mod_inis`。
///
/// # 参数
/// - `group_id`: 分组 id（用于槽位条件注入）
/// - `mod_data`: 目标模组（已确认启用）
/// - `known_libraries`: 预收集的已知库集合（只读）
///
/// # 返回
/// 该模组的 [`ModPerOutcome`]
pub(crate) fn process_single_mod(
    group_id: u32,
    mod_data: &ModData,
    known_libraries: &HashSet<String>,
) -> ModPerOutcome {
    let mut per = ModPerOutcome::default();
    let mut mod_inis: Vec<PathBuf> = Vec::new();
    // 跟踪本模组是否需要写标记文件（对齐 Dart _markAsOldAutoFix / _markAsRemovedSyntaxError）
    let mut mod_has_crash_lines = false;
    let mut mod_has_syntax_errors = false;

    for ini_data in &mod_data.mod_ini_data {
        let ini_path = PathBuf::from(&ini_data.ini_path);

        // 备份模组 INI（原子复制：进程任一时刻退出不产生半成品备份）
        let mod_backup = ini_path.with_extension(constants::BACKUP_EXTENSION);
        if !mod_backup.exists() {
            if let Err(e) = crate::utils::atomic_copy(&ini_path, &mod_backup) {
                log::warn!("Failed to backup mod INI {}: {}", ini_path.display(), e);
            }
        }

        // C6：INI 读取入口预检转换——非 UTF-8（GBK/GB18030）先无损转 UTF-8 原子写回，
        // 使 parse 读到一致 UTF-8，`write_atomic_skip_unchanged` 的字节比较不再误判。
        ensure_ini_utf8(&ini_path);

        match get_or_parse_ini(&ini_path) {
            Ok(mut ini) => {
                // 错误检测
                let errors = ini.detect_errors(&ini_path, known_libraries);
                if !errors.is_empty() {
                    // 对齐 Dart _modifyLinesBasedOnError：modsyntaxerrorremoved 标记仅由
                    // 「非 endif 的崩溃行错误」（crash line=1）触发。
                    // - 3Dmigoto 容错解析悬空/孤立 endif（flow control=3）、重复库(0)、
                    //   缺失 endif(2)、缺失库(5)、路径过长(6)，这些错误由 NRMM 自动修复
                    //   （remove_empty_if_blocks / fix_manager_endif）或仅为 UI 报告项，
                    //   不会令模组被移除，故不触发 modsyntaxerrorremoved 标记。
                    // - 仅当模组存在会致 XXMI 崩溃的绘制/缓冲行（crash line=1，如
                    //   drawindexed/draw/ib 取到非法值且非 auto / 数值 / 资源引用）时，
                    //   才标记该模组曾被移除（与 Dart 基线行为一致：基线零标记）。
                    let has_syntax_marker_error = errors.iter().any(|e| e.error_type == 1);
                    per.errors.extend(errors);
                    if has_syntax_marker_error {
                        mod_has_syntax_errors = true;
                    }
                }

                // 对齐 NRMM：错误条件行（`else if $X = = N` 等畸形条件 / legacy 旧槽位守卫）
                // 就地 `;-;` 注释，并视为该模组存在语法错误（写 modsyntaxerrorremoved 标记），
                // 与基线 NRMM 对 15 个模组的标记行为一致。幂等：重复 update 时已被注释的
                // `;-;` 行是 Comment，不会再命中。
                let errored_cond_lines = ini.comment_errored_conditional_lines();
                if !errored_cond_lines.is_empty() {
                    mod_has_syntax_errors = true;
                    per.errors.push(ErroredLines {
                        line_number: errored_cond_lines[0],
                        line: format!(
                            ";-; errored conditional lines: {}",
                            errored_cond_lines.len()
                        ),
                        error_type: 3, // FLOW CONTROL 系列：流程控制错误（与 detect_errors 对齐）
                        error_message: format!(
                            "FLOW CONTROL: commented {} errored conditional line(s)",
                            errored_cond_lines.len()
                        ),
                        ..Default::default()
                    });
                }

                // 展开 namespace 变量
                if let Some(ns) = namespace_handler::extract_namespace(&ini) {
                    namespace_handler::expand_ini_variables(&mut ini, &ns);
                }

                // 清理旧的 NRMM managed 内容（对齐 Dart _parseIniSections），
                // 防止重复更新时旧注入内容累积（if 守卫/$managed_slot_id/旧注释标记等）
                ini.remove_old_managed_content();

                // 对齐 NRMM _parseIniSections：补齐段级属性键（match_priority/allow_duplicate_hash），插入段末
                ini.ensure_section_attribute_keys();

                // 对齐 NRMM _reorderByIniKeyPriority：按段类型重排已知优先键到段首
                ini.reorder_by_ini_key_priority();

                // 注入槽位条件（含 $managed_slot_id 赋值）
                ini.inject_slot_conditions(group_id, mod_data.mod_index);

                // 注释崩溃行
                let crash_lines = ini.comment_crash_lines();
                if !crash_lines.is_empty() {
                    log::info!(
                        "Commented {} crash lines in {}",
                        crash_lines.len(),
                        ini_path.display()
                    );
                    mod_has_crash_lines = true;
                }

                // 移除空 if 块，应用缩进
                ini.remove_empty_if_blocks();
                ini.apply_indentation();
                ini.prepend_header_comment();

                // 原子写入（失败不中止整体：收集为错误条目，该 INI 不计入 mod_inis）。
                // 内容未变化时跳过写盘（幂等快速路径）：重复运行 update 时绝大多数
                // INI 输出与磁盘一致，跳过可省 create+rename 系统调用与杀毒扫描。
                if let Err(e) = ini.write_atomic_skip_unchanged(&ini_path) {
                    log::error!("Failed to write mod INI {}: {}", ini_path.display(), e);
                    per.errors.push(ErroredLines {
                        error_type: 3,
                        error_message: format!("Failed to write {}: {}", ini_path.display(), e),
                        ..Default::default()
                    });
                    continue;
                }

                mod_inis.push(ini_path.clone());
                per.processed += 1;
            }
            Err(e) => {
                log::error!("Failed to process mod INI {}: {}", ini_path.display(), e);
                per.errors.push(ErroredLines {
                    error_type: 3,
                    error_message: format!("Processing error: {}", e),
                    ..Default::default()
                });
            }
        }
    }

    per.mod_inis = mod_inis;

    // 写入标记文件（对齐 Dart _markAsOldAutoFix / _markAsRemovedSyntaxError）
    let mod_dir = std::path::Path::new(mod_data.mod_path.as_str());
    if mod_has_crash_lines {
        let marker = mod_dir.join(constants::MODFORCED_MARKER);
        if let Err(e) = crate::utils::atomic_write(&marker, b"") {
            log::warn!("写入 modforced 标记失败 {:?}: {}", marker, e);
        }
    }
    if mod_has_syntax_errors {
        let marker = mod_dir.join("modsyntaxerrorremoved");
        if let Err(e) = crate::utils::atomic_write(&marker, b"") {
            log::warn!("写入 modsyntaxerrorremoved 标记失败 {:?}: {}", marker, e);
        }
    }

    per
}

/// 处理单个分组的全部模组 INI（组内模组级并行，2026-08-18 用户需求：每个线程对应一个模组）。
///
/// 组内每个模组一个并行任务（rayon work-stealing，与外层分组 `par_iter` 嵌套安全），
/// 每模组任务以 `catch_unwind` 隔离：单模组 panic 转该模组 `error_type=3` 错误条目，
/// 不影响组内其他模组。rayon `collect` 保持输入顺序，合并结果（`mod_inis`/`errors`/
/// `processed`）与串行版完全一致，group INI include 顺序与日志确定性不变。
///
/// # 参数
/// - `group_id`: 分组 id（用于槽位条件注入）
/// - `mods`: 本组启用模组列表（扫描顺序）
/// - `known_libraries`: 预收集的已知库集合（只读）
///
/// # 返回
/// 该分组的 [`GroupOutcome`]（局部产出与错误，稍后按 group_id 升序合并）
pub(crate) fn process_group_task(
    group_id: u32,
    mods: &[&ModData],
    known_libraries: &HashSet<String>,
) -> GroupOutcome {
    let mut outcome = GroupOutcome {
        group_id,
        ..Default::default()
    };

    // 组内模组级并行：rayon collect 按输入顺序返回，合并结果与串行版一致
    let per_mod_results: Vec<ModPerOutcome> = mods
        .par_iter()
        .map(|mod_data| {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                process_single_mod(group_id, mod_data, known_libraries)
            })) {
                Ok(per) => per,
                Err(payload) => {
                    let reason = if let Some(s) = payload.downcast_ref::<&str>() {
                        (*s).to_string()
                    } else if let Some(s) = payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic payload".to_string()
                    };
                    log::error!(
                        "[mod_manager] [process_group_task] group {} 模组处理 panic，已跳过: {} | mod={:?}",
                        group_id,
                        reason,
                        mod_data.mod_path
                    );
                    ModPerOutcome {
                        errors: vec![ErroredLines {
                            error_type: 3,
                            error_message: format!(
                                "Mod panicked (skipped): {} | mod={:?}",
                                reason, mod_data.mod_path
                            ),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }
                }
            }
        })
        .collect();

    for per in per_mod_results {
        outcome.mod_inis.extend(per.mod_inis);
        outcome.errors.extend(per.errors);
        outcome.processed += per.processed;
        outcome.written += per.written;
        outcome.skipped += per.skipped;
    }

    // H3：组内全部可见模组写入失败 → 显式告警，避免「组内全失败→create_group_ini 跳过空组→模组被静默移出管理」不可察。
    // 前端应据此 errors 中 error_type=3 汇总条目提示用户手动介入，而非误以为更新成功。
    if !mods.is_empty() && outcome.mod_inis.is_empty() {
        let write_failures = outcome.errors.iter().filter(|e| e.error_type == 3).count();
        if write_failures > 0 {
            log::error!(
                "[mod_manager] [process_group_task] group {} 全部 {} 个可见模组写入失败，将无 group_X.ini 生成（模组可能被移出管理）",
                group_id,
                mods.len()
            );
            outcome.errors.push(ErroredLines {
                error_type: 3,
                error_message: format!(
                    "Group {}: all {} enabled mods failed to write ({} write errors); group INI will be skipped, mods may be dropped from management",
                    group_id,
                    mods.len(),
                    write_failures
                ),
                ..Default::default()
            });
        }
    }

    outcome
}

/// 以 `catch_unwind`（`AssertUnwindSafe`）包裹执行单个分组任务，panic 时不令其逃逸。
///
/// # 参数
/// - `group_id`: 分组 id（用于错误条目与日志定位）
/// - `task`: 分组任务闭包（通常为 [`process_group_task`] 的调用）
///
/// # 返回
/// 任务正常完成时返回其产出（补充耗时）；任务 panic 时返回仅含一条 `error_type=3`
/// 错误条目的 [`GroupOutcome`]（组内已收集的局部结果被丢弃，避免半成品路径进入 group INI）
pub(crate) fn run_group_with_panic_guard(
    group_id: u32,
    task: impl FnOnce() -> GroupOutcome,
) -> GroupOutcome {
    let start = std::time::Instant::now();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(task)) {
        Ok(mut outcome) => {
            outcome.elapsed_ms = start.elapsed().as_millis();
            outcome
        }
        Err(payload) => {
            let reason = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            log::error!(
                "[mod_manager] [update_mod_data] Group {} processing panicked: {}",
                group_id,
                reason
            );
            GroupOutcome {
                group_id,
                errors: vec![ErroredLines {
                    error_type: 3,
                    error_message: format!("Group {} processing panicked: {}", group_id, reason),
                    ..Default::default()
                }],
                elapsed_ms: start.elapsed().as_millis(),
                ..Default::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// 测试：组内模组级并行的结果保序（2026-08-18 并行化改造）
    ///
    /// 构造同组 5 个模组（mod_index 递增），直接调用 process_group_task，
    /// 断言 outcome.mod_inis 顺序与输入 mods 顺序完全一致（rayon collect 保序，
    /// group INI 生成与错误报告的确定性不因并行改变）。
    #[test]
    fn test_process_group_task_per_mod_parallel_preserves_order() {
        let dir = TempDir::new().unwrap();
        let group_dir = dir.path().join("group_1");
        fs::create_dir_all(&group_dir).unwrap();
        fs::write(
            group_dir.join("group_1.ini"),
            ";template\n[Constants]\nglobal $group_id = 1\n",
        )
        .unwrap();

        // 构造 5 个模组（每个一个 INI，mod_index 1..=5，目录名带序号便于比对）
        let mods: Vec<ModData> = (1..=5u32)
            .map(|i| {
                let mod_dir = group_dir.join(format!("Mod_{}", i));
                fs::create_dir_all(&mod_dir).unwrap();
                let ini_path = mod_dir.join("mod.ini");
                fs::write(
                    &ini_path,
                    format!("[TextureOverride]\nhash = 0x{:08X}\n", i),
                )
                .unwrap();
                ModData {
                    mod_index: i,
                    group_index: 1,
                    mod_path: mod_dir.to_string_lossy().to_string(),
                    mod_ini_data: vec![crate::models::mod_data::ModIniData {
                        ini_path: ini_path.to_string_lossy().to_string(),
                        ini_filename: "mod.ini".to_string(),
                        file_relative_path: ini_path.to_string_lossy().to_string(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }
            })
            .collect();
        let mods_ref: Vec<&ModData> = mods.iter().collect();

        let known = HashSet::new();
        let outcome = process_group_task(1, &mods_ref, &known);

        assert_eq!(outcome.processed, 5, "5 个模组应全部处理成功");
        assert_eq!(
            outcome.mod_inis.len(),
            5,
            "应收集 5 个 INI 路径（顺序保序断言见下）"
        );
        // 保序断言：mod_inis 顺序 == 输入 mods 顺序（Mod_1..Mod_5）
        for (i, path) in outcome.mod_inis.iter().enumerate() {
            let expected = group_dir.join(format!("Mod_{}", i + 1)).join("mod.ini");
            assert_eq!(
                path, &expected,
                "mod_inis[{}] 应与输入顺序一致（并行合并不得乱序）",
                i
            );
        }
    }

    /// 测试：组任务 panic 被 catch_unwind 捕获并转为错误条目，而非 panic 逃逸
    #[test]
    fn test_group_panic_isolation() {
        // &str panic payload
        let outcome = run_group_with_panic_guard(3, || panic!("boom"));
        assert_eq!(outcome.group_id, 3);
        assert!(outcome.mod_inis.is_empty());
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.errors[0].error_type, 3);
        assert!(outcome.errors[0]
            .error_message
            .contains("Group 3 processing panicked"));
        assert!(outcome.errors[0].error_message.contains("boom"));

        // String panic payload（格式化 panic）
        let outcome2 = run_group_with_panic_guard(5, || panic!("bad input: {}", 42));
        assert_eq!(outcome2.group_id, 5);
        assert_eq!(outcome2.errors.len(), 1);
        assert!(outcome2.errors[0].error_message.contains("bad input: 42"));

        // 非 panic 路径：正常产出透传
        let outcome3 = run_group_with_panic_guard(7, || GroupOutcome {
            group_id: 7,
            processed: 2,
            ..Default::default()
        });
        assert_eq!(outcome3.group_id, 7);
        assert_eq!(outcome3.processed, 2);
        assert!(outcome3.errors.is_empty());
    }
}
