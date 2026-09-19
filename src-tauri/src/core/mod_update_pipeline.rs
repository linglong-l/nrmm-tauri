//! update_mod_data 编排域：全量更新管线（备份→解析→检测→namespace→注入→写入）
//!
//! 从 mod_manager.rs 拆出（2026-09-19 Phase 4-9，最后拆分域）。
//! 依赖前序域：mod_results / mod_ini_admin / mod_detect / mod_toggle / mod_scanner / ini 六域。

use anyhow::{anyhow, Context, Result};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::core::constants;
use crate::core::d3dxini_cache::D3DX_INI_CACHE;
use crate::core::file_lock::AdvisoryLock;
use crate::core::ini_model::IniFile;
use crate::core::mod_detect::{
    collect_libraries_parallel, detect_orfix_texfx, detect_standard_xxmi,
};
use crate::core::mod_ini_admin::{
    auto_modify_duplicate_namespace, create_group_ini, create_nrmm_include_ini,
    delete_group_ini_files, detect_include_recursive, fix_non_managed_crash_lines,
    migrate_old_managed_folder, prepare_managed_folder, strip_nrmm_injected_content,
    validate_group_ini_consistency,
};
use crate::core::mod_results::{MainIniStatus, UpdateResult};
use crate::core::watcher_ring::WatcherGuard;

use crate::core::mod_update_lowlevel::{
    process_group_task, run_group_with_panic_guard, GroupOutcome,
};
use crate::core::operation_registry::OperationGuard;
use crate::models::enums::TargetGame;
use crate::models::mod_data::{ErroredLines, ModData};
use crate::models::settings::AppSettings;

/// 备份调度器暂停守卫：构造时暂停 [`crate::core::ini_backup_scheduler`]，
/// Drop 时恢复（RAII）。
///
/// 用途：`update_mod_data` 会对大批量 INI 执行写回，若此时 `get_mods` 触发的
/// 后台全量备份正在运行，两者会产生并发 I/O 竞争（读半成品文件、抢占磁盘带宽）。
/// 在 update 全程暂停备份线程池取新任务；已入队任务保留在队列中不丢失，
/// Drop 时唤醒 worker 继续消费。RAII 保证所有出口（含 `?` 提前返回与 panic
/// unwind）都会恢复调度器，不会令备份永久停摆。
struct BackupPauseGuard;

impl BackupPauseGuard {
    /// 暂停备份调度器并返回守卫
    fn new() -> Self {
        crate::core::ini_backup_scheduler::pause();
        BackupPauseGuard
    }
}

impl Drop for BackupPauseGuard {
    fn drop(&mut self) {
        crate::core::ini_backup_scheduler::resume();
    }
}

/// 重量级更新模组数据（完整 INI 解析 + 注入）
///
/// 这是模组管理的核心函数，严格复刻 NRMM 的 updateModData 流程：
/// 1. 准备 _MANAGED_ 目录（创建必要的模板 INI 文件）
/// 2. 深度扫描所有模组（完整解析 INI）
/// 3. 备份主 INI（d3dx.ini，全游戏统一）
/// 4. 收集所有启用模组的 INI 路径和已知库
/// 5. 清理旧的 group INI 文件
/// 6. 为每个 group 创建 ModFolder.ini（include 该组启用的模组）
/// 7. 对每个启用模组的 INI：备份 → 展开 namespace 变量 → 注入槽位条件 → 注释崩溃行 → 原子写入
/// 8. 生成 nrmm_include.ini（include 所有 group INI + 管理 INI）
/// 9. 原子写入主 INI（include nrmm_include.ini）
///
/// # 注意
/// - 这是重度 IO + CPU 操作，应在 spawn_blocking 中调用
/// - 仅在用户主动点击"应用"或切换模组时调用
///
/// # 参数
/// - `game`: 目标游戏
/// - `game_mods_path`: 游戏 Mods 目录
/// - `_settings`: 应用设置（预留参数）
pub fn update_mod_data(
    game: TargetGame,
    game_mods_path: &Path,
    _settings: &AppSettings,
) -> Result<UpdateResult> {
    log::debug!(
        "[core::mod_manager] [update_mod_data] Starting heavy update | game={:?} path={:?}",
        game,
        game_mods_path
    );
    // 进程级 advisory lock：确保同一游戏 Mods 目录的「更新」与其他 NRMM 实例的文件操作
    // 互斥（进程级一致性）。阻塞等待至多 30s；若另有实例持有则取消本次更新并返回中文报错。
    let _proc_lock = match AdvisoryLock::acquire_blocking(game_mods_path, Duration::from_secs(30)) {
        Ok(Some(l)) => l,
        Ok(None) => return Err(anyhow!("另一个 NRMM 实例正在操作该游戏目录，请稍后再试")),
        Err(e) => return Err(e),
    };
    // 重量级后台操作登记：供用户主动退出时检查「是否仍在更新模组数据」，若有则取消退出。
    let _op_guard = OperationGuard::new("update_mod_data");
    let _s = std::time::Instant::now();
    // 暂停文件监听器：update_mod_data 会对大量 INI / 标记文件执行写操作，
    // 若监听器保持活跃，防抖线程会在写入过程中并发触发增量扫描与缓存合并
    // （可能读到半成品文件、把中间状态并入缓存）。WatcherGuard 在函数退出时
    // （含 `?` 提前返回）自动恢复监听器（对齐 NRMM 的 watcher.stop()/watch() 模式）。
    let _watcher_guard = WatcherGuard::new();
    // 暂停备份调度器：避免 get_mods 触发的后台全量备份与本次 update 的
    // 大批量 INI 写回产生并发 I/O 竞争（RAII，所有出口自动恢复）
    let _backup_pause_guard = BackupPauseGuard::new();
    let managed_folder = game_mods_path.join(constants::MANAGED_FOLDER);

    // 游戏根目录（d3dx.ini 所在位置）。
    // 所有 include 路径必须相对于此基准计算，因为 3Dmigoto 将 include 路径相对于 d3dx.ini 目录解析。
    let game_root = game_mods_path.parent().unwrap_or(game_mods_path);

    // 阶段计时：migrate/prepare（目录迁移 + 创建 + 模板 INI 准备）
    let t_migrate_prepare = std::time::Instant::now();
    // 对齐 Dart `_prepareManagedFolder`：首次运行（`_MANAGED_` 不存在）时，
    // 先尝试将旧版托管目录名迁移为 `_MANAGED_`；仍不存在才创建。
    migrate_old_managed_folder(game_mods_path);
    if !managed_folder.exists() {
        fs::create_dir_all(&managed_folder)?;
    }

    // 步骤1: 准备 _MANAGED_ 目录，创建模板 INI 文件
    let mut need_reload_manual = prepare_managed_folder(&managed_folder, game)?;
    log::debug!(
        "[mod_manager] [update_mod_data] step=prepare_managed_folder done need_reload_manual={}",
        need_reload_manual
    );
    log::info!(
        "[UPDATE] phase=migrate_prepare took={}ms",
        t_migrate_prepare.elapsed().as_millis()
    );

    // 步骤2: 扫描模组
    let t_scan = std::time::Instant::now();
    let scan_result = crate::core::mod_scan_light::scan_mods(game_mods_path)?;
    log::debug!(
        "[mod_manager] [update_mod_data] step=scan_mods done total_mods={} total_groups={}",
        scan_result.total_mods_count,
        scan_result.groups.len()
    );
    log::info!(
        "[UPDATE] phase=scan_mods took={}ms groups={} mods={}",
        t_scan.elapsed().as_millis(),
        scan_result.groups.len(),
        scan_result.total_mods_count
    );

    let main_ini_name = game.d3dx_ini_name();
    // d3dx.ini 位于游戏根目录（Mods 的父目录），而非 Mods/ 内。
    // 原版 NRMM 不会在 Mods/ 下创建 d3dx.ini；d3dx.ini 是 XXMI/3Dmigoto 安装时就存在的。
    let main_ini_path = game_mods_path
        .parent()
        .map(|p| p.join(main_ini_name))
        .unwrap_or_else(|| game_mods_path.join(main_ini_name));

    // 阶段计时：主 INI 备份 + 读取 + strip
    let t_main_ini_read = std::time::Instant::now();
    // 若 d3dx.ini 不存在于游戏根（非标准 XXMI 环境），跳过主 INI 修改，
    // 不在 Mods/ 内创建默认 d3dx.ini（对齐原版 NRMM 行为）。
    let main_ini_existed = main_ini_path.exists();
    if !main_ini_existed {
        log::warn!(
            "[mod_manager] [update_mod_data] d3dx.ini 不存在于 {:?}（非标准 XXMI 环境），跳过主 INI 处理",
            main_ini_path
        );
    } else {
        // 备份主 INI（原子复制：进程任一时刻退出不产生半成品备份）
        let backup_path = main_ini_path.with_extension(constants::BACKUP_EXTENSION);
        if !backup_path.exists() {
            crate::utils::atomic_copy(&main_ini_path, &backup_path)
                .with_context(|| format!("Failed to backup main INI: {:?}", main_ini_path))?;
        }
    }

    // 读取主 INI 内容（d3dx.ini 不存在时为空字符串，跳过后续主 INI 修改）
    let main_ini_content = if main_ini_existed {
        let c = IniFile::force_read_as_utf8(&main_ini_path)?;
        strip_nrmm_injected_content(&c)
    } else {
        String::new()
    };
    log::info!(
        "[UPDATE] phase=main_ini_read took={}ms existed={}",
        t_main_ini_read.elapsed().as_millis(),
        main_ini_existed
    );

    // 步骤3: 收集启用模组和已知库（R3/C-3b：扁平列表为 Vec<Arc<ModData>>，
    // 此处解引用收集，下游 &[&ModData] 处理链签名不变）
    let enabled_mods: Vec<&ModData> = scan_result
        .mods
        .iter()
        .filter(|m| !m.disabled && !m.mod_disabled)
        .map(|m| m.as_ref())
        .collect();
    log::debug!(
        "[mod_manager] [update_mod_data] step=collect_enabled done enabled_mods_count={}",
        enabled_mods.len()
    );

    // 阶段计时：known_libraries 并行预收集
    let t_collect_libs = std::time::Instant::now();
    // known_libraries 并行预收集：启用模组的全部 INI 路径经 rayon 并行解析（get_or_parse_ini，
    // 读锁快路径 + 锁外解析），defined_libraries 归并进同一 HashSet，集合与串行版相等。
    let mut lib_ini_paths: Vec<PathBuf> = Vec::new();
    for mod_data in &enabled_mods {
        for ini_data in &mod_data.mod_ini_data {
            lib_ini_paths.push(PathBuf::from(&ini_data.ini_path));
        }
    }
    let known_libraries = collect_libraries_parallel(&lib_ini_paths);
    log::info!(
        "[UPDATE] phase=collect_libraries took={}ms ini_files={} libs={}",
        t_collect_libs.elapsed().as_millis(),
        lib_ini_paths.len(),
        known_libraries.len()
    );

    // 步骤3.1（已移除，2026-08-18 用户决策）：不再收集 # / MutexGroup 目录中的库定义。
    // update_mod_data 对 _MANAGED_ 下非 group_<int> 一级目录不读、不写、不遍历
    // （NRMM 原版亦不解析该类一级目录）。代价：引用 # 目录库定义的启用模组会产生
    // error_type=5（缺失库）假阳性错误条目——仅影响错误报告，不改变任何文件写入。

    // 步骤3.6: 跨模组 namespace 去重（引入原版语义：_autoModifyDuplicateNamespaceInManagedMod）
    // 必须在每-ini 注入循环之前调用，使改名后的 namespace= 声明能被 expand_ini_variables 展开。
    let t_ns_fix = std::time::Instant::now();
    auto_modify_duplicate_namespace(&enabled_mods);
    log::debug!("[mod_manager] [update_mod_data] step=auto_modify_duplicate_namespace done");
    log::info!(
        "[UPDATE] phase=namespace_fix took={}ms",
        t_ns_fix.elapsed().as_millis()
    );

    // 步骤3.7: 校验现有 group_X.ini 的一致性（group_id 与目录名是否匹配）
    // 对齐原版 NRMM：若 INI 中 `global $group_id = X` 与目录 `group_X` 不一致，则跳过处理。
    let t_ns_validate = std::time::Instant::now();
    let mismatched_groups = validate_group_ini_consistency(&managed_folder, &enabled_mods);
    if !mismatched_groups.is_empty() {
        log::warn!(
            "[mod_manager] [update_mod_data] group_X.ini 不一致，跳过组: {:?}",
            mismatched_groups
        );
    }
    log::info!(
        "[UPDATE] phase=group_validate took={}ms mismatched={}",
        t_ns_validate.elapsed().as_millis(),
        mismatched_groups.len()
    );

    // 步骤3.5: ORFix/TexFx 检测（在 INI 修改之前，使用原始 INI 内容）
    let t_orfix = std::time::Instant::now();
    let orfix_detection = detect_orfix_texfx(&enabled_mods);
    log::debug!(
        "[mod_manager] [update_mod_data] step=detect_orfix_texfx done has_detection={}",
        orfix_detection.has_detection
    );
    log::info!(
        "[UPDATE] phase=orfix_detect took={}ms has_detection={}",
        t_orfix.elapsed().as_millis(),
        orfix_detection.has_detection
    );

    // 步骤4: 清理旧的 group INI 文件
    let t_group_cleanup = std::time::Instant::now();
    delete_group_ini_files(&managed_folder)?;
    log::debug!("[mod_manager] [update_mod_data] step=delete_group_ini_files done");
    log::info!(
        "[UPDATE] phase=group_ini_cleanup took={}ms",
        t_group_cleanup.elapsed().as_millis()
    );

    // 步骤5: 按 group 组织启用的模组 INI（rayon 线程池按分组并行，组内保持扫描顺序串行）
    let parallel_start = std::time::Instant::now();

    // 有效模组按 group_index 分桶（BTreeMap 保持组 id 升序，组内保持扫描顺序）。
    // 分桶前守卫与串行版完全一致（行为与日志不变）。
    let mut buckets: std::collections::BTreeMap<u32, Vec<&ModData>> =
        std::collections::BTreeMap::new();
    for mod_data in enabled_mods.iter() {
        let group_id = mod_data.group_index;

        // 对齐 NRMM Dart：跳过 mod_index == 0 的模组（对应 Dart 的 "None" 空槽位，realIndex: 0）。
        // Dart getModsOnGroup 在 modDatas[0] 插入 realIndex=0 的占位符，_manageMod 循环
        // 通过 j != 0 跳过该条目。Rust 扫描不产生此占位符，但保留守卫以对齐语义。
        if mod_data.mod_index == 0 {
            log::debug!(
                "[mod_manager] [update_mod_data] 跳过 mod_index=0 的空槽位（对齐 Dart None 占位符）"
            );
            continue;
        }

        // 防御性守卫：仅处理严格 group_<int> 分组，跳过任何非标准标识
        // （scan_mods 已天然排除 MutexGroup / # 目录，此处为双保险，
        //  确保 update_mod_data 绝不注入或改写非 group_int 目录下的文件）
        // 与 mod_scanner::is_normal_group_dir 的正则 ^group_([1-9][0-9]{0,8})$ 等价：
        // group_id 来自已按同一正则解析的 group_index（u32），成功路径下恒为有效值；
        // 仅当 group_id == 0 或 > 999_999_999 时判为非标准（防御性守卫）。
        const MAX_NORMAL_GROUP_ID: u32 = 999_999_999;
        if group_id == 0 || group_id > MAX_NORMAL_GROUP_ID {
            log::warn!(
                "[mod_manager] [update_mod_data] 跳过非标准分组标识 group_{}，不进行注入/改写",
                group_id
            );
            continue;
        }
        // 跳过 group_X.ini 不一致的分组（global $group_id 与目录名不匹配）
        if mismatched_groups.contains(&group_id) {
            log::warn!(
                "[mod_manager] [update_mod_data] 跳过 group_{}（group_id 不一致）",
                group_id
            );
            continue;
        }

        buckets.entry(group_id).or_default().push(mod_data);
    }

    // known_libraries（含 MutexGroup 库补充）以 Arc 只读共享进各分组任务，detect_errors 只读使用
    let known_libs = Arc::new(known_libraries);

    // 桶 par_iter：每组任务在单一 worker 线程内串行处理本组全部模组；
    // catch_unwind 兜底，单组 panic 转组级错误条目，不影响其他组
    let outcomes: Vec<GroupOutcome> = buckets
        .par_iter()
        .map(|(group_id, mods)| {
            run_group_with_panic_guard(*group_id, || {
                process_group_task(*group_id, mods, &known_libs)
            })
        })
        .collect();

    // 确定性合并：outcomes 与 BTreeMap 迭代顺序一致（按 group_id 升序），组内序保持
    let mut group_mod_inis: std::collections::HashMap<u32, Vec<PathBuf>> =
        std::collections::HashMap::new();
    let mut all_errors: Vec<ErroredLines> = Vec::new();
    let mut processed_mods = 0u32;
    let mut slowest: Option<(u32, u128)> = None;
    for outcome in outcomes {
        processed_mods += outcome.processed;
        all_errors.extend(outcome.errors);
        group_mod_inis
            .entry(outcome.group_id)
            .or_default()
            .extend(outcome.mod_inis);
        match slowest {
            Some((_, ms)) if outcome.elapsed_ms <= ms => {}
            _ => slowest = Some((outcome.group_id, outcome.elapsed_ms)),
        }
    }
    // error_type 计数摘要（BTreeMap 保证输出顺序稳定，便于跨次运行对比）
    let error_type_summary: std::collections::BTreeMap<u8, usize> =
        all_errors
            .iter()
            .fold(std::collections::BTreeMap::new(), |mut m, e| {
                *m.entry(e.error_type).or_default() += 1;
                m
            });
    log::info!(
        "[UPDATE] phase=group_process took={}ms groups={} mod_inis={} workers={} slowest_group={} slowest_ms={} errors={} error_types={:?}",
        parallel_start.elapsed().as_millis(),
        buckets.len(),
        processed_mods,
        rayon::current_num_threads(),
        slowest.map(|(g, _)| g).unwrap_or(0),
        slowest.map(|(_, ms)| ms).unwrap_or(0),
        all_errors.len(),
        error_type_summary
    );
    log::debug!("[mod_manager] [update_mod_data] step=process_mod_inis done processed={} groups={} errors={}", processed_mods, group_mod_inis.len(), all_errors.len());

    // 步骤5.1: 修复非托管模组的崩溃行（对齐 NRMM _fixNonManagedModsCrashLine）
    // 扫描 Mods 目录下、_MANAGED_ 之外的 .ini 文件，注释其中会导致 XXMI 崩溃的
    // 绘制/缓冲行（drawindexed / draw / ib= / vb0= 等，位于非 TextureOverride 段且不在 if 块内）。
    fix_non_managed_crash_lines(game_mods_path, &managed_folder, &known_libs);
    log::debug!("[mod_manager] [update_mod_data] step=fix_non_managed_crash_lines done");

    // 步骤6: 为每个 group 创建 ModFolder.ini
    let t_group_ini = std::time::Instant::now();
    let mut group_ini_paths: Vec<PathBuf> = Vec::new();
    for (group_id, ini_paths) in &group_mod_inis {
        log::debug!(
            "[mod_manager] [update_mod_data] step=create_group_ini group_id={} mod_ini_count={}",
            group_id,
            ini_paths.len()
        );
        let group_dir = managed_folder.join(format!("group_{}", group_id));
        let group_ini_path = create_group_ini(&group_dir, *group_id, ini_paths)?;
        if let Some(p) = group_ini_path {
            group_ini_paths.push(p);
        }
    }
    log::debug!(
        "[mod_manager] [update_mod_data] step=create_group_ini done total_groups={}",
        group_ini_paths.len()
    );
    log::info!(
        "[UPDATE] phase=create_group_ini took={}ms groups={}",
        t_group_ini.elapsed().as_millis(),
        group_ini_paths.len()
    );

    // 步骤7: 生成 nrmm_include.ini
    let t_include = std::time::Instant::now();
    let nrmm_include_path = managed_folder.join(constants::INCLUDE_FILENAME);
    create_nrmm_include_ini(
        &nrmm_include_path,
        &managed_folder,
        &group_ini_paths,
        game_mods_path,
    )?;
    log::debug!(
        "[mod_manager] [update_mod_data] step=create_nrmm_include done group_ini_count={}",
        group_ini_paths.len()
    );

    // 步骤8: 检测 include_recursive，缺失时自动注入 include 指令
    // 标准 XXMI 环境通过 [Include] 段的 include_recursive = Mods 自动加载 _MANAGED_ 下的 .ini 文件。
    // 若用户的 d3dx.ini 缺少此配置，则直接在文件末尾追加 include = Mods/_MANAGED_/nrmm_include.ini
    // 确保 manager_group.ini / group_N.ini 等管理文件被 3Dmigoto 加载。
    // d3dx.ini 不存在（非标准环境）时跳过主 INI 修改。
    let mut final_content = main_ini_content;
    let mut d3dx_modified = false;
    if main_ini_existed {
        let has_include_recursive = detect_include_recursive(&main_ini_path, game_mods_path);

        if !has_include_recursive {
            log::warn!(
                "[mod_manager] [update_mod_data] d3dx.ini 缺少 include_recursive 配置，自动注入 include 指令"
            );
            // 在主 INI 末尾追加 NRMM 管理段（include 指令需在段外，3Dmigoto 仅处理顶层的 include）
            // 关键：include 路径必须相对于 d3dx.ini 所在目录（game_root），而非 Mods/。
            // 3Dmigoto 将 include 路径相对于 d3dx.ini 目录解析，因此路径需包含 "Mods/" 前缀。
            if let Ok(rel_path) = nrmm_include_path.strip_prefix(game_root) {
                let rel_str = rel_path.to_string_lossy().replace('\\', "/");
                final_content.push_str("\n;NRMM_INI_START\n");
                final_content.push_str("; No-Reload Mod Manager managed section\n");
                final_content.push_str("; Do not edit this section manually\n");
                final_content.push_str("[Constants]\n");
                final_content.push_str("global $managed_slot_id = 0\n\n");
                final_content.push_str(&format!("include = {}\n", rel_str));
                final_content.push_str(";NRMM_INI_END\n");
                d3dx_modified = true;
            }
            need_reload_manual = true;
        } else {
            log::debug!(
                "[mod_manager] [update_mod_data] d3dx.ini 已有 include_recursive=Mods，跳过注入"
            );
        }
        log::info!(
            "[UPDATE] phase=create_include took={}ms has_include_recursive={} injected={}",
            t_include.elapsed().as_millis(),
            has_include_recursive,
            d3dx_modified
        );

        // 仅当 d3dx.ini 被实际修改时才写回，避免不必要的编码转换或格式变更
        let t_main_write = std::time::Instant::now();
        if d3dx_modified {
            let tmp_path = main_ini_path.with_extension("ini.tmp");
            fs::write(&tmp_path, &final_content)
                .with_context(|| format!("Failed to write temp main INI: {:?}", tmp_path))?;
            fs::rename(&tmp_path, &main_ini_path).with_context(|| {
                format!("Failed to rename temp main INI to: {:?}", main_ini_path)
            })?;
            // 加固（根因 4，对齐 mod_ini_cache::invalidate_ini）：真实写盘后显式失效
            // d3dxini_cache 条目，规避 (mtime, len) 粗分辨率同秒等长替换下的陈旧解析。
            D3DX_INI_CACHE.write().invalidate(&main_ini_path);
            log::debug!(
                "[mod_manager] [update_mod_data] step=write_main_ini done path={:?}",
                main_ini_path
            );
        }
        log::info!(
            "[UPDATE] phase=main_ini_write took={}ms written={}",
            t_main_write.elapsed().as_millis(),
            d3dx_modified
        );
    } else {
        log::debug!(
            "[mod_manager] [update_mod_data] step=write_main_ini skipped (d3dx.ini 不存在)"
        );
        log::info!(
            "[UPDATE] phase=create_include took={}ms skipped_no_main_ini=true",
            t_include.elapsed().as_millis()
        );
    }

    // 步骤9: 检测标准 XXMI/3DMigoto 环境
    let is_standard_xxmi = detect_standard_xxmi(game_mods_path, main_ini_name);

    let result = UpdateResult {
        total_groups: scan_result.groups.len() as u32,
        total_mods: scan_result.total_mods_count as u32,
        enabled_mods: enabled_mods.len() as u32,
        disabled_mods: scan_result.disabled_mods_count as u32,
        processed_mods,
        errors: all_errors,
        need_reload_manual,
        is_standard_xxmi,
        // 主 INI 缺失状态显式上报（此前仅写日志静默跳过，前端无从感知游戏侧不生效）
        main_ini_status: if main_ini_existed {
            MainIniStatus::Resolved
        } else {
            MainIniStatus::MissingMainIni
        },
        orfix_detection,
        invalid_utf8_count: scan_result.invalid_utf8_count,
        invalid_utf8_entries: scan_result.invalid_utf8_entries,
        ..Default::default()
    };
    log::debug!(
        "[core::mod_manager] [update_mod_data] done | elapsed={:?}ms | processed={} errors={}",
        _s.elapsed().as_millis(),
        result.processed_mods,
        result.errors.len()
    );
    log::info!(
        "[UPDATE] total took={}ms groups={} mod_inis={} enabled_mods={} errors={}",
        _s.elapsed().as_millis(),
        scan_result.groups.len(),
        result.processed_mods,
        result.enabled_mods,
        result.errors.len()
    );
    Ok(result)
}

#[cfg(test)]
#[path = "mod_update_pipeline_tests.rs"]
mod tests;
