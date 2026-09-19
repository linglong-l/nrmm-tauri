//! 模组管理核心模块
//!
//! 负责模组的启用/禁用切换、互斥选择、INI 注入和备份恢复。
//! 核心功能：
//! - update_mod_data: 重量级更新，完整复刻 NRMM 的 update_mod_data 流程
//! - switch_mod: 普通分组模组选择（互斥：同一分组只能启用一个）
//! - enable_mutex_mod/disable_mutex_mod: 互斥组（MutexGroup）模组启用/禁用
//! - toggle_mod: 独立模组启用/禁用切换
//! - restore_all_inis: 从备份恢复所有 INI 文件
//! - save_customizations: Save Mod Customizations 功能，写入 d3dx_user.ini
//!
//! # NRMM 原版流程对齐
//! update_mod_data 严格复刻 NRMM 的以下步骤：
//! 1. _prepareManagedFolder: 准备 _MANAGED_ 目录，创建 nrmm_keypress.txt、nrmm_include.ini、manager_group.ini
//! 2. _deleteGroupIniFiles: 清理旧的 group_*.ini 文件
//! 3. 扫描并收集所有启用模组
//! 4. 命名空间去重处理（_autoModifyDuplicateNamespaceInManagedMod）
//! 5. 错误检测（重复 section、缺失 endif、库冲突等）
//! 6. _createGroupIni: 为每个 group_X 生成 ModFolder.ini
//! 7. _manageMod: 修改每个启用模组的 INI 文件，注入条件包裹
//! 8. 生成主 INI 注入段，include nrmm_include.ini

use crate::core::constants;
use crate::core::d3dxini_cache::D3DX_INI_CACHE;
use crate::core::file_lock::AdvisoryLock;
use crate::core::file_watcher::WatcherGuard;
use crate::core::ini_model::IniFile;
use crate::core::mod_ini_cache::get_or_parse_ini;
use crate::core::mod_scanner;
use crate::core::namespace_handler;
use crate::core::operation_registry::OperationGuard;
use crate::models::enums::TargetGame;
use crate::models::mod_data::{ErroredLines, GroupDelta, ModData};
use crate::models::settings::AppSettings;
use anyhow::{anyhow, Context, Result};
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
use walkdir::WalkDir;

// 文件监听器暂停守卫 `WatcherGuard` 已移至 `core::file_watcher` 模块
// （引用计数实现，支持嵌套暂停，修复 B2 竞态）

// 重导出供 commands 层使用（避免 commands 直接依赖 models 模块）
// 同时供本模块内部使用（pub use 也会将名称引入当前作用域）
pub use crate::models::mod_data::{HashConflictResult, OrfixDetection};

// 引用路径工具域，供本模块残留调用方使用
use crate::core::mod_path_utils::{get_available_folder_name, locate_mods_root};

// 重导出结果数据域，供 commands 层与本模块残留调用方使用（pub 类型，可 pub use）
pub use crate::core::mod_results::{
    MainIniStatus, RemoveModResult, RestoreManagedResult, RestoredCount,
    SaveCustomizationsResult, UpdateResult,
};

// 重导出启停域，供 commands 层与本模块残留调用方使用（pub 类型，可 pub use）
pub use crate::core::mod_toggle::{
    batch_toggle_mods, disable_all_mods_in_group, disable_mutex_mod, enable_all_mods_in_group,
    enable_mutex_mod, is_mutex_mod, toggle_mod,
};

// 重导出选择域，供 commands 层与本模块残留调用方使用（pub 类型，可 pub use）
pub use crate::core::mod_select::{
    deselect_group_mods, resolve_mod_index_by_path_from_cache, switch_mod,
    write_selectedindex_direct,
};

// 重导出检测域，供 commands 层与本模块残留调用方使用（pub 类型，可 pub use）
pub use crate::core::mod_detect::{detect_hash_conflicts, detect_orfix_texfx, detect_standard_xxmi};

// 引用检测域私有辅助（update 域残留调用）
use crate::core::mod_detect::collect_libraries_parallel;

// 重导出 INI 管理域，供 commands 层与本模块残留调用方使用（pub 类型，可 pub use）
pub use crate::core::mod_ini_admin::{auto_modify_duplicate_namespace, save_customizations};

// 引用 INI 管理域 pub(crate) 函数，供 update 编排域与本模块残留测试使用
use crate::core::mod_ini_admin::{
    create_group_ini, create_nrmm_include_ini, delete_group_ini_files, detect_include_recursive,
    ensure_ini_utf8, fix_non_managed_crash_lines, migrate_old_managed_folder,
    prepare_managed_folder, strip_nrmm_injected_content, validate_group_ini_consistency,
};
#[cfg(test)]
use crate::core::mod_ini_admin::{
    atomic_write_file, comment_crash_lines_in_file, generate_nrmm_injected_content,
};

// 重导出恢复域，供 commands 层与本模块残留调用方使用（pub 类型，可 pub use）
pub use crate::core::mod_restore::{restore_all_inis, restore_managed_mod};
#[cfg(test)]
use crate::core::mod_restore::collect_mod_ini_files;


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
    let scan_result = mod_scanner::scan_mods(game_mods_path)?;
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

/// 单个分组（group）并行任务的处理产出。
///
/// `update_mod_data` 步骤 5 按 `group_index` 分桶后，每个分组任务在单一 worker 线程内
/// 串行处理本组全部模组并产出本结构；全部完成后按 `group_id` 升序确定性合并，
/// 保证同一输入多次运行的 `errors` 顺序与 `mod_inis` 内容确定。
#[derive(Debug, Default)]
struct GroupOutcome {
    /// 分组 id（group_index）
    group_id: u32,
    /// 本组成功处理（已原子写入）的模组 INI 路径（组内保持扫描顺序）
    mod_inis: Vec<PathBuf>,
    /// 本组收集到的错误条目（解析/检测/写入失败，组内序保持）
    errors: Vec<ErroredLines>,
    /// 本组实际处理（INI 注入并写入成功）的计数
    processed: u32,
    /// 本组实际写盘（内容有变化）的 INI 计数
    written: u32,
    /// 本组因内容未变化跳过写盘的 INI 计数
    skipped: u32,
    /// 本组任务耗时（毫秒），由 panic 兜底包装器统一记录
    elapsed_ms: u128,
}

/// 单个模组（组内并行任务粒度）的处理产出。
///
/// `process_group_task` 按「每模组一任务」并行处理组内模组，各任务产出本结构，
/// 全部完成后按模组扫描顺序确定性合并进 [`GroupOutcome`]，
/// 保证与串行版的 `mod_inis`/`errors` 顺序完全一致。
#[derive(Debug, Default)]
struct ModPerOutcome {
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
fn process_single_mod(
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
fn process_group_task(
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
fn run_group_with_panic_guard(group_id: u32, task: impl FnOnce() -> GroupOutcome) -> GroupOutcome {
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




/// 移除模组（NRMM 对齐：移至 DISABLED_MANAGED_REMOVED + 还原 INI）
///
/// 严格复刻 NRMM 的 `renameOrMoveFolder` + `restoreManagedMod` 流程：
/// 1. 定位 `Mods/DISABLED_MANAGED_REMOVED` 目录（与 `_MANAGED_` 同级）
/// 2. 若不存在则创建
/// 3. 构造目标路径，名称冲突时追加 `_1`、`_2`…（对齐 NRMM `_getAvailableFolderName`）
/// 4. `fs::rename` 移动模组文件夹到目标路径
/// 5. 对移动后的模组调用 `restore_managed_mod` 清理所有 NRMM 注入内容
///
/// # 参数
/// - `mod_path`: 要移除的模组文件夹路径（如 `Mods/_MANAGED_/group_1/MyMod`）
///
/// # 返回值
/// 返回 `RemoveModResult`，包含移动后路径、INI 还原状态等
///
/// # 错误
/// - 模组路径不存在
/// - 无法定位 Mods 根目录
/// - 创建 DISABLED_MANAGED_REMOVED 目录失败
/// - 移动文件夹失败（如跨卷）
pub fn remove_mod(mod_path: &Path) -> Result<RemoveModResult> {
    if !mod_path.exists() {
        return Err(anyhow::anyhow!("Mod path does not exist: {:?}", mod_path));
    }
    if !mod_path.is_dir() {
        return Err(anyhow::anyhow!(
            "Mod path is not a directory: {:?}",
            mod_path
        ));
    }

    let mod_name = mod_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // ========== 定位 Mods 根目录 ==========
    // mod_path 结构：Mods/_MANAGED_/group_X/MyMod 或 Mods/_MANAGED_/#MutexGroup/MyMod
    // 需向上查找直到找到 _MANAGED_ 的父目录（即 Mods 根目录）
    let mods_root = locate_mods_root(mod_path)?;

    // ========== 定位 DISABLED_MANAGED_REMOVED 目录 ==========
    let removed_folder = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
    if !removed_folder.exists() {
        fs::create_dir_all(&removed_folder).with_context(|| {
            format!(
                "Failed to create {}: {:?}",
                constants::MANAGED_REMOVED_FOLDER,
                removed_folder
            )
        })?;
    }

    // ========== 构造目标路径（冲突追加 _1、_2…） ==========
    let target = get_available_folder_name(&mod_name, &removed_folder);

    // ========== 移动模组文件夹 ==========
    fs::rename(mod_path, &target)
        .with_context(|| format!("Failed to move mod {:?} to {:?}", mod_path, target))?;

    // ========== 还原 INI ==========
    let (ini_count, failed_count) = restore_managed_mod(&target);

    log::info!(
        "[remove_mod] Moved '{}' to {:?}, INI restored: {} total, {} failed",
        mod_name,
        target,
        ini_count,
        failed_count
    );

    Ok(RemoveModResult {
        mod_name,
        moved_to: target,
        restored: failed_count == 0,
        ini_count,
        failed_count,
        // 占位：命令层局部重扫受影响分组后覆盖为真实 delta
        group_delta: GroupDelta::default(),
        degraded: false,
    })
}

/// 底层移动：将目录移动到目标删除目录（仅负责移动，不做任何业务判断）
///
/// 单一职责：仅接受「源目录」与「目标删除目录」，执行目录移动。
/// - 若目标删除目录不存在，先创建
/// - 目标名冲突时追加 `_1`、`_2`… 后缀
/// - 不关心源目录从何而来、为何移动，也不负责定位 Mods 根目录
///
/// # 参数
/// - `src`: 待移动的源目录路径
/// - `removed_dir`: 目标删除目录（模组/分组将被移动到此目录下）
///
/// # 返回值
/// 返回移动后的实际路径（冲突时含后缀）
///
/// # 错误
/// - 创建目标删除目录失败
/// - 目录移动失败（如跨卷、权限不足）
pub fn move_dir_to_removed(src: &Path, removed_dir: &Path) -> Result<PathBuf> {
    if !removed_dir.exists() {
        fs::create_dir_all(removed_dir)
            .with_context(|| format!("Failed to create removed dir: {:?}", removed_dir))?;
    }

    let name = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let target = get_available_folder_name(&name, removed_dir);

    fs::rename(src, &target)
        .with_context(|| format!("Failed to move {:?} to {:?}", src, target))?;

    Ok(target)
}

/// 移除分组（NRMM 对齐：移至 DISABLED_MANAGED_REMOVED 目录，非group先移子分组再移除）
///
/// 本函数为「编排层」，仅决定哪些目录移动到哪里，具体移动操作委托给底层 `move_dir_to_removed`。
/// Mods 根目录由调用方（顶层命令）从设置数据传入，本函数不做路径推导。
///
/// # 参数
/// - `group_path`: 要移除的分组目录路径
/// - `is_group_xx`: true = group_1/group_2 等普通分组，false = mutexGroup 非group目录
/// - `mods_root`: Mods 根目录（与 `_MANAGED_` 同级），由调用方从设置数据获取
///
/// # 对于 group_xx：
/// 1. 将整个分组目录移至 `mods_root/DISABLED_MANAGED_REMOVED/原名`
///    - 名称冲突追加 `_1`、`_2`…
///
/// # 对于非group：
/// 1. 先将分组下的**一级子目录且不含 .ini 的目录**（即子分组目录）移至 `mods_root`
///    - 子分组目录重名时追加 `_1`、`_2`… 后缀
/// 2. 然后将被清空的分组目录移至 `mods_root/DISABLED_MANAGED_REMOVED/`
///
/// # 错误
/// 路径不存在或目录移动失败时返回错误
pub fn remove_group_ex(group_path: &Path, is_group_xx: bool, mods_root: &Path) -> Result<()> {
    if !group_path.exists() {
        return Err(anyhow::anyhow!(
            "Group path does not exist: {:?}",
            group_path
        ));
    }
    if !group_path.is_dir() {
        return Err(anyhow::anyhow!(
            "Group path is not a directory: {:?}",
            group_path
        ));
    }

    // ========== 非group：先将一级子分组（无 .ini 的子目录）移至 mods_root ==========
    if !is_group_xx {
        // 收集所有一级子分组（子目录且不含 .ini 的目录）
        let entries = match fs::read_dir(group_path) {
            Ok(e) => e,
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Failed to read group directory: {:?}",
                    group_path
                ))
            }
        };

        // 先收集再处理（避免在迭代期间修改目录）
        let mut to_move: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let child_path = entry.path();
            if !child_path.is_dir() {
                continue;
            }
            let child_name = entry.file_name().to_string_lossy().to_string();
            if child_name.starts_with('.') {
                continue;
            }
            // 含 .ini → 叶子模组节点，不是子分组，跳过（会随父目录一起被移除）
            let has_ini = match mod_scanner::dir_has_ini_file(&child_path) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if has_ini {
                continue;
            }
            // 不含 .ini 的目录 → 子分组目录，需移至 mods_root
            to_move.push(child_path);
        }

        for src in to_move {
            move_dir_to_removed(&src, mods_root)?;
        }
    }

    // ========== 将分组目录移至 DISABLED_MANAGED_REMOVED ==========
    let removed_folder = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
    move_dir_to_removed(group_path, &removed_folder)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::SystemTime;
    use tempfile::TempDir;

    /// 还原安全窗口全局状态互斥 + RAII 清除。
    ///
    /// 调用 `restore_all_inis` / `restore_managed_mod`（内部开启 60s 全局还原安全窗口，
    /// `mod_scanner::mark_restore_safe_window`）的测试必须持有 `mod_scanner::TEST_WINDOW_LOCK`，
    /// 并在退出 / panic 时通过本 guard 清除窗口——否则窗口会泄漏到并发执行的
    /// mod_scanner 标记创建断言测试（窗口期内标记不被创建而误报失败）。
    struct RestoreWindowGuard;
    impl Drop for RestoreWindowGuard {
        fn drop(&mut self) {
            crate::core::mod_scanner::test_clear_restore_safe_window();
        }
    }

    fn setup_test_env() -> TempDir {
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

    fn create_main_ini(base: &Path, game: TargetGame) {
        let ini_name = game.d3dx_ini_name();
        // 包含 [Include] 段的 include_recursive，对齐标准 XXMI 环境
        // 使用实际目录名确保 detect_include_recursive 返回 true，不注入 ;NRMM_INI_START 块
        let dir_name = base.file_name().and_then(|n| n.to_str()).unwrap_or("Mods");
        let content = format!(
            "; original main ini\n[Include]\ninclude_recursive = {}\n",
            dir_name
        );
        fs::write(base.join(ini_name), content).unwrap();
    }

    #[test]
    fn test_strip_nrmm_content() {
        let content = r#"; original header
[Constants]
x = 1
;NRMM_INI_START
managed stuff here
include = mod1.ini
;NRMM_INI_END
; trailing content
y = 2
"#;
        let result = strip_nrmm_injected_content(content);
        assert!(!result.contains("managed stuff"));
        assert!(!result.contains("include = mod1.ini"));
        assert!(result.contains("original header"));
        assert!(result.contains("x = 1"));
        assert!(result.contains("y = 2"));
        assert!(!result.contains("NRMM_INI_START"));
        assert!(!result.contains("NRMM_INI_END"));
    }

    #[test]
    fn test_strip_nrmm_content_no_injection() {
        let content = "; no injection here\n[Section]\nkey=val\n";
        let result = strip_nrmm_injected_content(content);
        assert_eq!(result.trim(), content.trim());
    }

    #[test]
    fn test_detect_include_recursive_present() {
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().join("Mods");
        fs::create_dir_all(&mods_path).unwrap();
        let ini_path = dir.path().join("d3dx.ini");
        fs::write(
            &ini_path,
            "[Include]\ninclude_recursive = Mods\n\n[Constants]\nglobal $test = 0\n",
        )
        .unwrap();
        assert!(detect_include_recursive(&ini_path, &mods_path));
    }

    #[test]
    fn test_detect_include_recursive_missing() {
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().join("Mods");
        fs::create_dir_all(&mods_path).unwrap();
        let ini_path = dir.path().join("d3dx.ini");
        fs::write(&ini_path, "[Constants]\nglobal $test = 0\n").unwrap();
        assert!(!detect_include_recursive(&ini_path, &mods_path));
    }

    #[test]
    fn test_detect_include_recursive_wrong_section() {
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().join("Mods");
        fs::create_dir_all(&mods_path).unwrap();
        let ini_path = dir.path().join("d3dx.ini");
        // include_recursive 放在 [Constants] 段内，3Dmigoto 不处理此位置
        fs::write(&ini_path, "[Constants]\ninclude_recursive = Mods\n").unwrap();
        assert!(!detect_include_recursive(&ini_path, &mods_path));
    }

    #[test]
    fn test_detect_include_recursive_case_insensitive() {
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().join("Mods");
        fs::create_dir_all(&mods_path).unwrap();
        let ini_path = dir.path().join("d3dx.ini");
        fs::write(&ini_path, "[INCLUDE]\ninclude_recursive = Mods\n").unwrap();
        assert!(detect_include_recursive(&ini_path, &mods_path));
    }

    #[test]
    fn test_generate_injected_content() {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        let include_path = managed.join("nrmm_include.ini");
        fs::write(&include_path, "").unwrap();

        let result = generate_nrmm_injected_content(&include_path, dir.path()).unwrap();

        assert!(result.contains(";NRMM_INI_START"));
        assert!(result.contains(";NRMM_INI_END"));
        assert!(result.contains("[Constants]"));
        assert!(result.contains("$managed_slot_id"));
        assert!(result.contains("include = _MANAGED_/nrmm_include.ini"));
        assert!(!result.contains("\\"));
    }

    #[test]
    fn test_toggle_mod_enable_disable() {
        let dir = TempDir::new().unwrap();
        let enabled_path = dir.path().join("MyMod");
        fs::create_dir_all(&enabled_path).unwrap();

        toggle_mod(&enabled_path, false).unwrap();
        let disabled_name = enabled_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let new_path = dir.path().join(format!("DISABLED{}", disabled_name));
        assert!(new_path.exists());
        assert!(!enabled_path.exists());

        toggle_mod(&new_path, true).unwrap();
        assert!(enabled_path.exists());
        assert!(!new_path.exists());
    }

    #[test]
    fn test_toggle_mod_already_in_state() {
        let dir = TempDir::new().unwrap();

        let enabled_path = dir.path().join("EnabledMod");
        fs::create_dir_all(&enabled_path).unwrap();
        toggle_mod(&enabled_path, true).unwrap();
        assert!(enabled_path.exists());

        let disabled_path = dir.path().join("DISABLEDDisabledMod");
        fs::create_dir_all(&disabled_path).unwrap();
        toggle_mod(&disabled_path, false).unwrap();
        assert!(disabled_path.exists());
    }

    #[test]
    fn test_prepare_managed_folder() {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();

        let need_reload = prepare_managed_folder(&managed, TargetGame::GenshinImpact).unwrap();
        assert!(need_reload); // 首次创建需要手动重载

        assert!(managed.join(constants::KEYPRESS_FILENAME).exists());
        assert!(managed.join(constants::INCLUDE_FILENAME).exists());
        assert!(managed.join("manager_group.ini").exists());

        // 第二次调用应该不需要手动重载
        let need_reload2 = prepare_managed_folder(&managed, TargetGame::GenshinImpact).unwrap();
        assert!(!need_reload2);
    }

    #[test]
    fn test_create_group_ini() {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        let group_dir = managed.join("group_1");
        fs::create_dir_all(&group_dir).unwrap();

        let mod1 = group_dir.join("Mod1/mod.ini");
        fs::create_dir_all(mod1.parent().unwrap()).unwrap();
        fs::write(&mod1, "").unwrap();

        let mod2 = group_dir.join("Mod2/config.ini");
        fs::create_dir_all(mod2.parent().unwrap()).unwrap();
        fs::write(&mod2, "").unwrap();

        let ini_paths = vec![mod1, mod2];
        let result = create_group_ini(&group_dir, 1, &ini_paths).unwrap();
        assert!(result.is_some());

        let group_ini = result.unwrap();
        assert!(group_ini.exists());
        let content = fs::read_to_string(&group_ini).unwrap();
        assert!(content.contains("group_1"));
        assert!(content.contains("$group_id = 1"));
        // 对齐 NRMM：group_X.ini 仅包含组模板，不包含显式 include 指令
        assert!(!content.contains("include = "));
    }

    /// 测试：create_group_ini 对空分组返回 None，不生成 group_X.ini
    #[test]
    fn test_create_group_ini_skips_empty() {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        let group_dir = managed.join("group_84");
        fs::create_dir_all(&group_dir).unwrap();

        let ini_paths: Vec<PathBuf> = vec![];
        let result = create_group_ini(&group_dir, 84, &ini_paths).unwrap();
        assert!(result.is_none(), "空分组应返回 None");

        // 验证未生成 group_84.ini 文件
        let group_ini = group_dir.join("group_84.ini");
        assert!(!group_ini.exists(), "空分组不应生成 group_X.ini 文件");
    }

    /// 测试：update_mod_data 对 # 目录（MutexGroup）零触碰（不读、不写、不遍历）
    ///
    /// 构造 _MANAGED_/#MyMutex/MyMod/lib.ini（含库定义与崩溃行混合内容，记录
    /// 写入时间戳），同时在 # 目录放置哨兵文件；运行 update_mod_data 后断言：
    /// 1. # 目录内所有文件内容与 mtime 完全不变；
    /// 2. # 目录内未新增任何文件（无标记文件、无备份、无 selectedindex）。
    /// 验证 2026-08-18 用户决策：非 group_<int> 一级目录不参与数据更新。
    #[test]
    fn test_update_mod_data_does_not_touch_mutex_dirs() {
        let dir = TempDir::new().unwrap();
        let mods = dir.path().join("Mods");
        let managed = mods.join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();

        // # 互斥分组目录：模组 INI 含库定义 + 崩溃行（诱导注释写入）+ 哨兵文件
        let mutex_mod_dir = managed.join("#MyMutex").join("MyMod");
        fs::create_dir_all(&mutex_mod_dir).unwrap();
        let mutex_ini = mutex_mod_dir.join("lib.ini");
        let mutex_content =
            "[ResourceMyLib]\nhash = abc123\n[SomeSection]\ndrawindexed = crashval\n";
        fs::write(&mutex_ini, mutex_content).unwrap();
        let sentinel = mutex_mod_dir.join("sentinel.txt");
        fs::write(&sentinel, "keep").unwrap();

        // 同目录树的 group_1 正常分组（含一个启用模组），保证 update 有实际工作量
        let group_dir = managed.join("group_1");
        fs::create_dir_all(&group_dir).unwrap();
        fs::write(
            group_dir.join("group_1.ini"),
            ";template\n[Constants]\nglobal $group_id = 1\n",
        )
        .unwrap();
        let mod_dir = group_dir.join("EnabledMod");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(
            mod_dir.join("mod.ini"),
            "[TextureOverride]\nhash = 0xAAAA\n",
        )
        .unwrap();

        // 记录 # 目录快照（路径集合 + 内容 + mtime）
        let snapshot: std::collections::BTreeMap<PathBuf, (String, SystemTime)> = {
            let mut m = std::collections::BTreeMap::new();
            for entry in WalkDir::new(&mutex_mod_dir).into_iter().flatten() {
                let p = entry.path();
                if p.is_file() {
                    let content = fs::read_to_string(p).unwrap_or_default();
                    let mtime = fs::metadata(p).and_then(|md| md.modified()).ok().unwrap();
                    m.insert(p.to_path_buf(), (content, mtime));
                }
            }
            m
        };
        assert!(!snapshot.is_empty(), "快照应包含 # 目录下文件");

        let settings = AppSettings::default();
        update_mod_data(TargetGame::GenshinImpact, &mods, &settings).expect("update_mod_data 失败");

        // 断言 1：内容与 mtime 不变
        for (p, (content, mtime)) in &snapshot {
            let after = fs::read_to_string(p).unwrap_or_default();
            assert_eq!(&after, content, "# 目录文件内容不得被修改: {:?}", p);
            let after_mtime = fs::metadata(p).and_then(|md| md.modified()).ok().unwrap();
            assert_eq!(
                after_mtime, *mtime,
                "# 目录文件 mtime 不得变化（不得被读取后重写）: {:?}",
                p
            );
        }
        // 断言 2：未新增文件（无标记/备份/selectedindex 生成进 # 目录）
        let after_count = WalkDir::new(&mutex_mod_dir)
            .into_iter()
            .flatten()
            .filter(|e| e.path().is_file())
            .count();
        assert_eq!(
            after_count,
            snapshot.len(),
            "# 目录文件数量不得变化（不得新增任何文件）"
        );
    }

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

    /// 测试：fix_non_managed_crash_lines 复用 MOD_INI_CACHE 且缓存新鲜度正确
    ///
    /// 验证三点：
    /// 1. 首次调用注释崩溃行（`;-;` 前缀）；
    /// 2. 第二次调用同一文件未变化 → 缓存命中，检测无崩溃行，不重复写（内容不变）；
    /// 3. 文件被修改（mtime/len 变化）→ 缓存自动未命中重新解析，新增崩溃行被注释，
    ///    证明缓存不会返回旧内容。
    ///
    /// 同时验证 `_MANAGED_` 内文件与 `#` 目录不受触碰（沿用现有过滤逻辑）。
    #[test]
    fn test_fix_non_managed_crash_lines_cache_freshness() {
        let dir = TempDir::new().unwrap();
        let mods = dir.path().join("Mods");
        let managed = mods.join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();

        // 非托管 INI（Mods 根下、_MANAGED_ 外）：drawindexed 取非法值构成崩溃行
        let loose = mods.join("loose_mod.ini");
        fs::write(&loose, "[SomeSection]\ndrawindexed = crashval\n").unwrap();

        // _MANAGED_ 内与 # 目录下的文件：不应被 fix_non_managed 触碰
        let managed_ini = managed.join("managed_mod.ini");
        fs::write(&managed_ini, "[SomeSection]\ndrawindexed = crashval\n").unwrap();

        let known = HashSet::new();
        fix_non_managed_crash_lines(&mods, &managed, &known);

        let after1 = fs::read_to_string(&loose).unwrap();
        assert!(
            after1.contains(";-;drawindexed = crashval"),
            "首次调用应注释非托管崩溃行"
        );
        assert!(
            !fs::read_to_string(&managed_ini).unwrap().contains(";-;"),
            "_MANAGED_ 内文件不应被 fix_non_managed 修改"
        );

        // 第二次调用：文件未变化 → 缓存命中（不重复解析），无崩溃行 → 不写
        fix_non_managed_crash_lines(&mods, &managed, &known);
        let after2 = fs::read_to_string(&loose).unwrap();
        assert_eq!(after1, after2, "未变化文件二次调用不应重复解析后被改写");

        // 修改文件追加新崩溃行：mtime/len 变化 → 缓存失效重新解析 → 新行被注释
        fs::write(
            &loose,
            "[SomeSection]\n;-;drawindexed = crashval\ndraw = anothercrash\n",
        )
        .unwrap();
        fix_non_managed_crash_lines(&mods, &managed, &known);
        let after3 = fs::read_to_string(&loose).unwrap();
        assert!(
            after3.contains(";-;draw = anothercrash"),
            "文件变化后缓存不得返回旧内容，新崩溃行应被注释"
        );
    }

    /// 测试：prepare_managed_folder 正确创建根级 selectedindex 文件
    #[test]
    fn test_prepare_managed_folder_creates_root_selectedindex() {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();

        let root_selectedindex = managed.join(constants::SELECTED_INDEX_FILE);
        assert!(!root_selectedindex.exists());

        prepare_managed_folder(&managed, TargetGame::GenshinImpact).unwrap();

        // 验证根级 selectedindex 已创建
        assert!(
            root_selectedindex.exists(),
            "根级 selectedindex 文件应被创建"
        );
        let content = fs::read_to_string(&root_selectedindex).unwrap();
        assert_eq!(content, "0", "根级 selectedindex 默认值应为 0");

        // 再次调用不应覆盖已有内容
        fs::write(&root_selectedindex, "5").unwrap();
        prepare_managed_folder(&managed, TargetGame::GenshinImpact).unwrap();
        let content2 = fs::read_to_string(&root_selectedindex).unwrap();
        assert_eq!(content2, "5", "已有 selectedindex 不应被覆盖");
    }

    #[test]
    fn test_delete_group_ini_files() {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();

        let old_ini = managed.join("group_1.ini");
        fs::write(&old_ini, "").unwrap();
        let old_modfolder = managed.join("ModFolder.ini");
        fs::write(&old_modfolder, "").unwrap();
        let other_file = managed.join("other.txt");
        fs::write(&other_file, "").unwrap();

        assert!(old_ini.exists());
        delete_group_ini_files(&managed).unwrap();
        assert!(!old_ini.exists());
        assert!(!old_modfolder.exists());
        assert!(other_file.exists());
    }

    #[test]
    fn test_update_mod_data_empty() {
        let dir = setup_test_env();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        assert_eq!(result.total_mods, 0);
        assert_eq!(result.enabled_mods, 0);
        assert_eq!(result.processed_mods, 0);
        assert!(result.errors.is_empty());

        let main_ini = dir.path().join("d3dx.ini");
        let content = fs::read_to_string(&main_ini).unwrap();
        // 对齐 NRMM 原版：d3dx.ini 不再注入 include 标记，依赖 include_recursive 自动加载
        assert!(!content.contains(";NRMM_INI_START"));
        assert!(!content.contains(";NRMM_INI_END"));
        assert!(!content.contains("nrmm_include.ini"));

        // 验证管理文件已创建
        let managed = dir.path().join("_MANAGED_");
        assert!(managed.join("nrmm_keypress.txt").exists());
        assert!(managed.join("nrmm_include.ini").exists());
        assert!(managed.join("manager_group.ini").exists());
    }

    #[test]
    fn test_update_mod_data_with_mods() {
        let dir = setup_test_env();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(
            &group_path,
            "TestMod",
            "[TextureOverrideTest]\nhash = 0x12345678\nps-t0 = ResourceTest\ndrawindexed = auto\n",
        );

        let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        assert_eq!(result.total_mods, 1);
        assert_eq!(result.enabled_mods, 1);
        assert_eq!(result.processed_mods, 1);

        // 验证 group_1.ini 已创建（对齐 NRMM：仅包含组模板，不包含显式 include）
        let group_ini = group_path.join("group_1.ini");
        assert!(group_ini.exists());
        let group_content = fs::read_to_string(&group_ini).unwrap();
        assert!(group_content.contains("group_1"));
        assert!(group_content.contains("$group_id = 1"));
        // NRMM 不生成显式 include 指令，模组由 include_recursive=Mods 自动加载
        assert!(!group_content.contains("; === NRMM Managed Includes ==="));

        // 验证 nrmm_include.ini 仅包含 IncludeKeypress（对齐 NRMM）
        let include_content =
            fs::read_to_string(dir.path().join("_MANAGED_/nrmm_include.ini")).unwrap();
        assert!(include_content.contains("[IncludeKeypress]"));
        assert!(include_content.contains("nrmm_keypress.txt"));
        assert!(!include_content.contains("manager_group.ini"));
        assert!(!include_content.contains("group_1/group_1.ini"));

        let mod_ini_path = group_path.join("TestMod/mod.ini");
        let backup_path = mod_ini_path.with_extension(constants::BACKUP_EXTENSION);
        assert!(backup_path.exists());
    }

    #[test]
    fn test_update_mod_data_namespace_dedup() {
        // 引入原版语义：同组两个模组共享同一 namespace 时，其中一个应被重命名为唯一值
        let dir = setup_test_env();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(
            &group_path,
            "ModA",
            "namespace = Shared\n[Constants]\n$v = $Shared$Val\n",
        );
        create_mod_with_ini(
            &group_path,
            "ModB",
            "namespace = Shared\n[Constants]\n$w = $Shared$Other\n",
        );

        let _result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();

        let a_path = group_path.join("ModA/mod.ini");
        let b_path = group_path.join("ModB/mod.ini");
        let a = fs::read_to_string(&a_path).unwrap();
        let b = fs::read_to_string(&b_path).unwrap();

        let ns_of = |content: &str| -> Option<String> {
            for line in content.lines() {
                let t = line.trim();
                if t.to_lowercase().replace(' ', "").starts_with("namespace=") {
                    if let Some(eq) = t.find('=') {
                        return Some(t[eq + 1..].trim().to_string());
                    }
                }
            }
            None
        };
        let ns_a = ns_of(&a).unwrap();
        let ns_b = ns_of(&b).unwrap();

        // 去重后应为 {Shared, Shared_1}（顺序不定）
        let mut pair = [ns_a, ns_b];
        pair.sort();
        assert_eq!(pair, ["Shared", "Shared_1"]);

        // 每个模组内部的 namespace 引用应与自身声明一致（被 expand_ini_variables 展开为 $\ns\ 形式）
        for (content, ns) in [(&a, &pair[0]), (&b, &pair[1])] {
            let own_ref = format!("$\\{}\\", ns);
            let other_ns = if ns == "Shared" { "Shared_1" } else { "Shared" };
            let other_ref = format!("$\\{}\\", other_ns);
            assert!(
                content.contains(&own_ref),
                "模组 namespace={} 应包含自身引用 {}",
                ns,
                own_ref
            );
            assert!(
                !content.contains(&other_ref),
                "模组 namespace={} 不应残留对方引用 {}",
                ns,
                other_ref
            );
        }

        // 成功后不应残留 .baknamespace / .tmp
        assert!(!a_path.with_file_name("mod.ini.baknamespace").exists());
        assert!(!b_path.with_file_name("mod.ini.baknamespace").exists());
    }

    #[test]
    fn test_restore_all_inis() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        // d3dx.ini 在游戏根（Mods 父目录）；_MANAGED_ 在 Mods/ 下
        let dir = tempfile::TempDir::new().unwrap();
        let mods_path = dir.path().join("Mods");
        let managed = mods_path.join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        // 覆盖 include_recursive 值为 "Mods"，使 detect_include_recursive 匹配
        fs::write(
            dir.path().join("d3dx.ini"),
            "; original main ini\n[Include]\ninclude_recursive = Mods\n",
        )
        .unwrap();
        let settings = AppSettings::default();

        let group_path = managed.join("group_1");
        fs::create_dir_all(&group_path).unwrap();
        let _mod_path = {
            let p = group_path.join("RestoreMod");
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("mod.ini"), "[TextureOverrideTest]\nhash = 0x1\n").unwrap();
            p
        };

        update_mod_data(TargetGame::GenshinImpact, &mods_path, &settings).unwrap();

        let main_ini_path = dir.path().join("d3dx.ini");
        let modified_content = fs::read_to_string(&main_ini_path).unwrap();
        // 对齐 NRMM 原版：d3dx.ini 有 include_recursive=Mods 时不注入 ;NRMM_INI_START 块
        assert!(!modified_content.contains(";NRMM_INI_START"));

        let result = restore_all_inis(&mods_path).unwrap();
        assert!(result.restored >= 1);
        assert_eq!(result.failed, 0);

        let restored_content = fs::read_to_string(&main_ini_path).unwrap();
        assert!(!restored_content.contains(";NRMM_INI_START"));

        let backup_path = main_ini_path.with_extension(constants::BACKUP_EXTENSION);
        assert!(!backup_path.exists());

        // 验证管理文件已清理
        assert!(!managed.join("nrmm_include.ini").exists());
    }

    #[test]
    fn test_restore_managed_mod_single_ini_file() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        let dir = setup_test_env();
        // 构造一个含 NRMM 管理注入的单个 .ini 文件
        let ini_path = dir.path().join("mod.ini");
        fs::write(
            &ini_path,
            "[TextureOverrideTest]\nhash = 0x1\n; no reload mod manager\n",
        )
        .unwrap();

        // 单 .ini 文件应被还原
        let (restored, failed) = restore_managed_mod(&ini_path);
        assert_eq!(restored, 1);
        assert_eq!(failed, 0);
        let content = fs::read_to_string(&ini_path).unwrap();
        assert!(!content.contains("no reload mod manager"));
    }

    #[test]
    fn test_restore_managed_mod_skips_non_ini_file() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        let dir = setup_test_env();
        // 非 .ini 文件应被忽略（pass），不报错也不处理
        let txt_path = dir.path().join("readme.txt");
        fs::write(&txt_path, "hello").unwrap();

        let (restored, failed) = restore_managed_mod(&txt_path);
        assert_eq!(restored, 0);
        assert_eq!(failed, 0);
    }

    #[test]
    fn test_restore_managed_mod_directory_recursive() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        let dir = setup_test_env();
        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(
            &group_path,
            "ModA",
            "[TextureOverrideA]\nhash = 0x1\n; no reload mod manager\n",
        );
        create_mod_with_ini(
            &group_path,
            "ModB",
            "[TextureOverrideB]\nhash = 0x2\nglobal $managed_slot_id = 0\n",
        );

        // 目录应递归还原其下所有 .ini 文件
        let (restored, failed) = restore_managed_mod(&group_path);
        assert_eq!(restored, 2);
        assert_eq!(failed, 0);
        let a = fs::read_to_string(group_path.join("ModA").join("mod.ini")).unwrap();
        assert!(!a.contains("no reload mod manager"));
        let b = fs::read_to_string(group_path.join("ModB").join("mod.ini")).unwrap();
        assert!(!b.contains("global $managed_slot_id"));
    }

    // ============================================================================
    // 还原逻辑对齐测试（NRMM restoreManagedMod / restoreAllInis）
    // 验证「备份式还原（Restore All）」无损回放，以及「就地清理式还原（还原区/移除）」
    // 正确清除管理器注入的所有产物（manager if/endif、global $managed_slot_id、
    // condition 槽位段、4 空格缩进、管理器头注释），并保留非管理器内容。
    // ============================================================================

    /// 备份式还原（Restore All）必须是 update_mod_data 的「无损逆操作」：
    /// 每个被管理的 mod INI 都应从 .ini_managed_backup 精确恢复到更新前字节。
    #[test]
    fn test_restore_all_inis_lossless_roundtrip() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        // 结构与真实游戏一致：game_root/Mods/_MANAGED_/group_X/Mod/mod.ini
        // 且 game_root/d3dx.ini 位于 Mods 父目录。
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().join("Mods");
        let managed = mods_path.join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let group_path = managed.join("group_1");
        fs::create_dir_all(&group_path).unwrap();
        let original = "\
[TextureOverrideBody]
hash = 0x1
drawindexed = auto
condition = orig_cond

[KeySection]
condition = (orig_cond) && other

[Resource]
hash = 0x2
";
        let mod_dir = create_mod_with_ini(&group_path, "ModA", original);
        let mod_ini = mod_dir.join("mod.ini");

        // 更新（注入管理器内容 + 创建备份）
        let result = update_mod_data(TargetGame::GenshinImpact, &mods_path, &settings).unwrap();
        assert_eq!(result.processed_mods, 1);
        // 更新后文件应已被注入（与原始不同）
        let updated = fs::read_to_string(&mod_ini).unwrap();
        assert_ne!(updated, original, "更新后应注入管理器内容");
        // 备份必须存在且为更新前字节
        let backup = mod_ini.with_extension(constants::BACKUP_EXTENSION);
        assert!(backup.exists());
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);

        // 还原（Restore All）
        let rc = restore_all_inis(&mods_path).unwrap();
        assert_eq!(rc.failed, 0);

        // 还原后 mod INI 必须精确等于更新前字节（无损回放）
        let restored = fs::read_to_string(&mod_ini).unwrap();
        assert_eq!(restored, original, "备份式还原必须无损恢复到更新前状态");

        // 备份应被删除
        assert!(!backup.exists());
        // 管理器生成文件应被清理
        assert!(!managed.join("nrmm_include.ini").exists());
        assert!(!managed.join("manager_group.ini").exists());
        assert!(!managed.join("group_1.ini").exists());
    }

    /// 就地清理式还原：对一份「已经过 update_mod_data」的 INI，验证 restore_managed_mod
    /// 清除全部管理器注入产物，但保留用户原始内容与合法悬空 endif。
    #[test]
    fn test_restore_managed_mod_cleans_injected_artifacts() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        let dir = setup_test_env();
        let ini = dir.path().join("mod.ini");
        // 模拟 update_mod_data 输出的真实形态（头注释 + Constants 全局 + 包裹段 + 增强 condition + 悬空 endif）
        let managed = "\
; \";-;\" are errored conditional lines.
; \";+;\" are disabled keys.
; Errored conditional blocks (if/else/elif/endif) are handled correctly (newer syntax may require further testing), including namespaced variables.
; If certain syntax is only available in newer XXMI versions, make sure to use the latest XXMI.
[Constants]
global $managed_slot_id = 0
[TextureOverrideBody]
hash = 0x1
match_priority = 0
if $managed_slot_id == $\\modmanageragl\\group_1\\active_slot
    drawindexed = auto
    ; inner note
endif
[KeySection]
condition = (orig_cond) && $managed_slot_id == $\\modmanageragl\\group_1\\active_slot
[Resource]
hash = 0x2
endif
";
        fs::write(&ini, managed).unwrap();

        let (total, failed) = restore_managed_mod(&ini);
        assert_eq!(total, 1);
        assert_eq!(failed, 0);

        let out = fs::read_to_string(&ini).unwrap();

        // 1) 管理器头注释被移除
        assert!(!out.contains("are errored"), "管理器头注释应被移除");
        assert!(!out.contains("are disabled keys"), "管理器头注释应被移除");
        // 2) global $managed_slot_id 被移除
        assert!(
            !out.contains("global $managed_slot_id"),
            "global 声明应被移除"
        );
        // 3) manager if 与该段 endif 被移除，且不含任何管理器残留
        assert!(!out.contains("managed_slot_id"), "管理器槽位残留应被清除");
        assert!(!out.contains("active_slot"), "管理器槽位残留应被清除");
        // 4) 4 空格缩进被移除（段体行去缩进）
        assert!(
            out.contains("drawindexed = auto"),
            "缩进后的命令应去缩进保留"
        );
        assert!(!out.contains("    drawindexed"), "不应残留 4 空格缩进");
        // 5) 非管理器注释保留
        assert!(out.contains("; inner note"), "非管理器注释应保留");
        // 6) 增强 condition 中管理器段被清除，仅留用户原条件（圆括号为 sanitize 对称解包，详见报告）
        assert!(out.contains("orig_cond"), "用户原 condition 应保留");
        // 7) 合法悬空 endif 必须保留（对齐 Dart：不删除 INPUT 遗留悬空 endif）
        assert!(out.contains("endif"), "合法悬空 endif 应被保留");
        // 8) match_priority=0 由 update 注入，restore 不移除（非管理器专属标记；
        //    作为独立 3Dmigoto 指令在脱管后依然合法。此行为是否为 NRMM 严格一致项待 Dart 基线确认。）
        assert!(
            out.contains("match_priority = 0"),
            "match_priority 注入项当前保留（对齐待确认）"
        );
    }

    /// 真实管线回放：update_mod_data 产出后，对 mod 目录执行 restore_managed_mod，
    /// 确认管理器 if/endif/global/condition 全部清除。
    #[test]
    fn test_restore_managed_mod_after_real_update() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().join("Mods");
        let managed = mods_path.join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let group_path = managed.join("group_1");
        fs::create_dir_all(&group_path).unwrap();
        let mod_dir = create_mod_with_ini(
            &group_path,
            "ModA",
            "[TextureOverrideBody]\nhash = 0x1\ndrawindexed = auto\n",
        );

        update_mod_data(TargetGame::GenshinImpact, &mods_path, &settings).unwrap();

        let (total, failed) = restore_managed_mod(&mod_dir);
        assert_eq!(total, 1);
        assert_eq!(failed, 0);

        let out = fs::read_to_string(mod_dir.join("mod.ini")).unwrap();
        assert!(
            !out.contains("managed_slot_id"),
            "真实管线还原后不应残留管理器槽位"
        );
        assert!(
            !out.contains("active_slot"),
            "真实管线还原后不应残留管理器槽位"
        );
        // 段体命令保留且去缩进
        assert!(out.contains("drawindexed = auto"));
    }

    #[test]
    fn test_update_mod_data_creates_default_ini() {
        // 对齐原版 NRMM：d3dx.ini 不在 Mods/ 内创建，位于游戏根目录（Mods 父目录）。
        // 无 d3dx.ini 时 update_mod_data 应优雅跳过主 INI 处理，不报错、不在 Mods/ 创建。
        let dir = setup_test_env();
        let settings = AppSettings::default();

        assert!(!dir.path().join("d3dx.ini").exists());
        let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        assert_eq!(result.total_mods, 0);
        // d3dx.ini 不应在 Mods 目录内被创建
        assert!(!dir.path().join("d3dx.ini").exists());
        // 主 INI 缺失必须显式上报（而非静默跳过）
        assert_eq!(
            result.main_ini_status,
            crate::core::mod_manager::MainIniStatus::MissingMainIni
        );
    }

    #[test]
    fn test_update_mod_data_hsr_ini() {
        // 对齐原版 NRMM：全游戏统一 d3dx.ini（无 HSR RatioShot 特判），d3dx.ini 不在 Mods/ 内创建
        let dir = setup_test_env();
        let settings = AppSettings::default();

        let result = update_mod_data(TargetGame::HonkaiStarRail, dir.path(), &settings).unwrap();
        assert_eq!(result.total_mods, 0);
        assert!(!dir.path().join("d3dx.ini").exists());
        assert_eq!(
            result.main_ini_status,
            crate::core::mod_manager::MainIniStatus::MissingMainIni
        );
    }

    #[test]
    fn test_strip_nrmm_multiline() {
        let content = "before\n;NRMM_INI_START\na\nb\nc\n;NRMM_INI_END\nafter";
        let result = strip_nrmm_injected_content(content);
        assert_eq!(result.trim(), "before\nafter");
    }

    fn create_test_mod(parent: &Path, name: &str) -> PathBuf {
        let mod_path = parent.join(name);
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "").unwrap();
        mod_path
    }

    #[test]
    fn test_enable_mutex_mod_disables_siblings() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("MutexGroup");
        fs::create_dir_all(&parent).unwrap();

        let mod1 = create_test_mod(&parent, "DISABLEDModA");
        let mod2 = create_test_mod(&parent, "ModB");
        let mod3 = create_test_mod(&parent, "ModC");

        let target_mod = parent.join("ModA");
        enable_mutex_mod(&mod1).unwrap();

        assert!(target_mod.exists());
        assert!(!mod1.exists());
        assert!(!mod2.exists());
        assert!(parent.join("DISABLEDModB").exists());
        assert!(!mod3.exists());
        assert!(parent.join("DISABLEDModC").exists());
    }

    #[test]
    fn test_enable_mutex_idempotent() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("MutexGroup");
        fs::create_dir_all(&parent).unwrap();

        let mod1 = create_test_mod(&parent, "ModA");
        let _mod2 = create_test_mod(&parent, "DISABLEDModB");
        let _mod3 = create_test_mod(&parent, "DISABLEDModC");

        enable_mutex_mod(&mod1).unwrap();

        assert!(mod1.exists());
        assert!(parent.join("DISABLEDModB").exists());
        assert!(parent.join("DISABLEDModC").exists());

        enable_mutex_mod(&mod1).unwrap();

        assert!(mod1.exists());
        assert!(parent.join("DISABLEDModB").exists());
        assert!(parent.join("DISABLEDModC").exists());
    }

    #[test]
    fn test_disable_mutex_mod() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("MutexGroup");
        fs::create_dir_all(&parent).unwrap();

        let mod_path = create_test_mod(&parent, "MyMod");
        assert!(mod_path.exists());

        disable_mutex_mod(&mod_path).unwrap();

        assert!(!mod_path.exists());
        assert!(parent.join("DISABLEDMyMod").exists());
    }

    #[test]
    fn test_disable_mutex_idempotent() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("MutexGroup");
        fs::create_dir_all(&parent).unwrap();

        let disabled_path = create_test_mod(&parent, "DISABLEDMyMod");
        assert!(disabled_path.exists());

        disable_mutex_mod(&disabled_path).unwrap();

        assert!(disabled_path.exists());
    }

    #[test]
    fn test_mutex_does_not_affect_subgroups() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("MutexGroup");
        fs::create_dir_all(&parent).unwrap();

        let mod1 = create_test_mod(&parent, "ModA");
        let mod2 = create_test_mod(&parent, "ModB");

        let subgroup = parent.join("SubGroup");
        fs::create_dir_all(&subgroup).unwrap();
        let submod = create_test_mod(&subgroup, "SubMod1");
        let submod2 = create_test_mod(&subgroup, "SubMod2");

        enable_mutex_mod(&mod1).unwrap();

        assert!(mod1.exists());
        assert!(!mod2.exists());
        assert!(parent.join("DISABLEDModB").exists());
        assert!(submod.exists());
        assert!(submod2.exists());
    }

    #[test]
    fn test_is_mutex_mod_detection() {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();

        let normal_group = managed.join("group_1");
        fs::create_dir_all(&normal_group).unwrap();
        let normal_mod = create_test_mod(&normal_group, "NormalMod");

        let mutex_group = managed.join("#MutexMods");
        fs::create_dir_all(&mutex_group).unwrap();
        let mutex_mod = create_test_mod(&mutex_group, "MutexMod");

        assert!(!is_mutex_mod(&normal_mod, &managed));
        assert!(is_mutex_mod(&mutex_mod, &managed));
    }

    #[test]
    fn test_save_customizations() {
        // d3dx.ini 在游戏根（Mods 父目录）
        let dir = tempfile::TempDir::new().unwrap();
        let mods_path = dir.path().join("Mods");
        fs::create_dir_all(mods_path.join("_MANAGED_")).unwrap();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        // 先执行一次 update_mod_data 添加 NRMM 管理段
        update_mod_data(TargetGame::GenshinImpact, &mods_path, &settings).unwrap();

        // 保存自定义设置
        let result = save_customizations(&mods_path, TargetGame::GenshinImpact).unwrap();
        assert!(result.success);

        let user_ini = dir.path().join("d3dx_user.ini");
        assert!(user_ini.exists());
        let content = fs::read_to_string(&user_ini).unwrap();
        assert!(content.contains("Custom settings saved by NRMM"));
        assert!(!content.contains(";NRMM_INI_START"));
    }

    #[test]
    fn test_atomic_write_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        atomic_write_file(&path, b"hello world").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello world");
        assert!(!path.with_extension("tmp").exists());
    }

    /// 编码安全测试：`comment_crash_lines_in_file` 保留 BOM / CRLF / 末尾换行，
    /// 并对非 UTF-8 文件跳过修复（拒绝有损回写损坏）。
    #[test]
    fn test_comment_crash_lines_encoding_safe() {
        let dir = TempDir::new().unwrap();

        // 1) BOM + CRLF 文件：仅目标行被加前缀，其余字节原样保留
        let crlf_path = dir.path().join("crlf.ini");
        let crlf_content = "\u{FEFF}[Section]\r\nkey = 1\r\ndrawindexed = 999\r\n";
        fs::write(&crlf_path, crlf_content.as_bytes()).unwrap();
        let changed =
            comment_crash_lines_in_file(&crlf_path, &[(3, "drawindexed = 999".to_string())]);
        assert!(changed, "CRLF 文件应执行崩溃行修复");
        let bytes = fs::read(&crlf_path).unwrap();
        assert!(bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "BOM 应保留");
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            text.contains("\r\n;-;drawindexed = 999\r\n"),
            "CRLF 与目标行前缀应保留:\n{:?}",
            text
        );
        assert!(text.contains("\r\nkey = 1\r\n"), "未命中行应原样保留");

        // 2) 非 UTF-8 文件（含 GBK 字节）：C6 统一编码——先无损转 UTF-8 再注释崩溃行
        let gbk_path = dir.path().join("gbk.ini");
        // GBK 字节：\xb2\xe2\xca\xd4 = "测试"
        let gbk_bytes: Vec<u8> = b"[Section]\r\ndrawindexed = 5\r\n\xb2\xe2\xca\xd4\r\n".to_vec();
        fs::write(&gbk_path, &gbk_bytes).unwrap();
        let changed2 =
            comment_crash_lines_in_file(&gbk_path, &[(2, "drawindexed = 5".to_string())]);
        assert!(changed2, "非 UTF-8 文件应转换并修复崩溃行");
        let out_text = String::from_utf8(fs::read(&gbk_path).unwrap()).unwrap();
        assert!(
            out_text.contains(";-;drawindexed = 5") && out_text.contains("测试"),
            "GBK 内容应无损转为 UTF-8 并注释崩溃行:\n{:?}",
            out_text
        );

        // 3) LF 无 BOM 文件：正常修复且保留末尾换行
        let lf_path = dir.path().join("lf.ini");
        fs::write(&lf_path, "[Section]\nkey = 1\ndrawindexed = auto\n").unwrap();
        let changed3 =
            comment_crash_lines_in_file(&lf_path, &[(3, "drawindexed = auto".to_string())]);
        assert!(changed3);
        assert_eq!(
            fs::read_to_string(&lf_path).unwrap(),
            "[Section]\nkey = 1\n;-;drawindexed = auto\n",
            "LF 文件修复后应保留末尾换行"
        );
    }

    // ============================================================================
    // 步骤 5 按分组并行化（rayon）单元测试
    // ============================================================================

    /// 递归收集目录下所有 .ini 文件内容（路径 → 内容，BTreeMap 保证迭代顺序稳定）
    fn snapshot_ini_files(root: &Path) -> std::collections::BTreeMap<PathBuf, String> {
        let mut map = std::collections::BTreeMap::new();
        for entry in WalkDir::new(root).into_iter().flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("ini") {
                map.insert(p.to_path_buf(), fs::read_to_string(p).unwrap_or_default());
            }
        }
        map
    }


    /// 测试：多分组各自产生错误时，errors 合并顺序按 group_id 升序稳定（多次运行一致）
    #[test]
    fn test_parallel_merge_error_order_deterministic() {
        let dir = setup_test_env();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        // 3 个分组，各含一个引用未知库的模组（触发 NON EXISTENT LIB，error_type=5）
        for gid in [1u32, 2, 3] {
            let group_path = create_group_dir(dir.path(), &format!("group_{}", gid));
            create_mod_with_ini(
                &group_path,
                "ModWithMissingLib",
                &format!(
                    "[TextureOverrideTest]\nhash = 0x{0}\nrun = MissingLibG{0}\ndrawindexed = auto\n",
                    gid
                ),
            );
        }

        let fmt_errors = |r: &UpdateResult| -> Vec<(u8, u32, String, String)> {
            r.errors
                .iter()
                .map(|e| {
                    (
                        e.error_type,
                        e.line_number,
                        e.line.clone(),
                        e.error_message.clone(),
                    )
                })
                .collect()
        };

        // 第一次运行完成内容注入；后续运行进入稳态，errors 应逐条一致（顺序确定）
        let _ = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        let steady =
            fmt_errors(&update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap());
        let steady2 =
            fmt_errors(&update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap());
        assert_eq!(steady, steady2, "多次运行 errors 顺序应稳定");

        // 三组各产生一条 NON EXISTENT LIB 错误
        let nonexistent_count = steady.iter().filter(|(t, ..)| *t == 5).count();
        assert_eq!(nonexistent_count, 3, "每组应各产生一条缺失库错误");

        // 合并顺序按 group_id 升序：G1 的错误先于 G2，G2 先于 G3
        let pos = |name: &str| {
            steady
                .iter()
                .position(|(_, _, _, msg)| msg.contains(name))
                .unwrap_or_else(|| panic!("缺少包含 {} 的错误条目", name))
        };
        assert!(
            pos("MissingLibG1") < pos("MissingLibG2"),
            "group_1 错误应先于 group_2"
        );
        assert!(
            pos("MissingLibG2") < pos("MissingLibG3"),
            "group_2 错误应先于 group_3"
        );
    }

    /// 测试：单模组写入失败被隔离为错误条目，整体返回 Ok，同组其他模组正常处理且 group INI 生成。
    ///
    /// 失败构造方式：占位 write_atomic 的临时文件路径（mod.ini → mod.ini.tmp）为目录，
    /// 使 fs::File::create 必然失败。相比 Windows 只读属性（tmp+rename 策略可能绕过），
    /// 该方式在 Windows/Linux 上均可可靠复现写入失败。
    #[test]
    fn test_write_failure_isolated_per_mod() {
        let dir = setup_test_env();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(
            &group_path,
            "GoodMod",
            "[TextureOverrideGood]\nhash = 0x1\ndrawindexed = auto\n",
        );
        let bad_mod_dir = create_mod_with_ini(
            &group_path,
            "BadMod",
            "[TextureOverrideBad]\nhash = 0x2\ndrawindexed = auto\n",
        );
        fs::create_dir_all(bad_mod_dir.join("mod.ini.tmp")).unwrap();

        let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();

        // 整体 Ok，仅 GoodMod 计入 processed
        assert_eq!(result.processed_mods, 1);
        // BadMod 写入失败被收集为错误条目（error_type=3，消息含路径）
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.error_type == 3 && e.error_message.contains("BadMod")),
            "应包含 BadMod 写入失败错误条目: {:?}",
            result.errors
        );
        // 同组其他 ini 正常处理（已注入 NRMM 头注释）且 group INI 生成
        let good = fs::read_to_string(group_path.join("GoodMod").join("mod.ini")).unwrap();
        assert!(good.contains("; \";-;\" are errored conditional lines."));
        assert!(group_path.join("group_1.ini").exists());
        // 失败 INI 未被写入（无 NRMM 头注释）
        let bad = fs::read_to_string(group_path.join("BadMod").join("mod.ini")).unwrap();
        assert!(!bad.contains("; \";-;\" are errored conditional lines."));
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

    /// 测试：幂等性——update_mod_data 连续多次运行后，_MANAGED_ 下 group INI 与模组 ini
    /// 内容进入稳态且不再变化（比较第 2 次后与第 3 次后）。
    ///
    /// 说明：首次运行注入的守卫包裹整段；第 2 次运行起 reorder_by_ini_key_priority 会将
    /// hash / match_priority 等优先键提升到守卫之前（且已存在守卫时跳过重复注入），
    /// 故第 1 → 2 次存在一次性的注入归一化，第 2 次后进入稳态。该行为为预存在的
    /// 管线语义（与串行版一致），非并行化引入。
    #[test]
    fn test_update_mod_data_idempotent() {
        let dir = setup_test_env();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let group1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(
            &group1,
            "ModA",
            "[TextureOverrideBody]\nhash = 0x1\ndrawindexed = auto\n",
        );
        create_mod_with_ini(
            &group1,
            "ModB",
            "[TextureOverrideBody]\nhash = 0x2\ndrawindexed = auto\n",
        );
        let group2 = create_group_dir(dir.path(), "group_2");
        create_mod_with_ini(&group2, "ModC", "[Constants]\n$v = 1\n");

        let managed = dir.path().join("_MANAGED_");
        update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        assert!(group1.join("group_1.ini").exists());
        assert!(group2.join("group_2.ini").exists());

        // 第 2 次运行后进入稳态
        update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        let snapshot2 = snapshot_ini_files(&managed);
        assert!(
            snapshot2.len() >= 5,
            "应至少包含 3 个模组 ini 与 2 个 group ini"
        );

        // 第 3 次运行后内容应与稳态完全一致（幂等）
        update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        let snapshot3 = snapshot_ini_files(&managed);

        assert_eq!(
            snapshot2, snapshot3,
            "进入稳态后再次更新，_MANAGED_ 下 ini 内容应保持完全一致"
        );
    }

    /// 测试：toggle_mod 目标为 `DISABLEDDISABLEDMod` 时启用应一次移除全部前缀 → `Mod`
    #[test]
    fn test_toggle_mod_enable_removes_all_prefixes() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path();
        let mod_path = parent.join("DISABLEDDISABLEDMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[ShaderOverride]").unwrap();

        toggle_mod(&mod_path, true).unwrap();

        assert!(!mod_path.exists(), "原 DISABLEDDISABLEDMod 目录应被重命名");
        assert!(
            parent.join("Mod").exists(),
            "启用后应为 Mod（一次移除全部连续前缀）"
        );
    }

    /// 测试：toggle_mod 对已禁用目录重复禁用不重复加前缀（幂等）
    #[test]
    fn test_toggle_mod_disable_idempotent_no_extra_prefix() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path();
        let mod_path = parent.join("DISABLEDMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[ShaderOverride]").unwrap();

        // 已禁用 → 应幂等跳过，不新增前缀
        toggle_mod(&mod_path, false).unwrap();

        let entries: Vec<String> = fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            entries.iter().any(|n| n == "DISABLEDMod"),
            "目录仍应为 DISABLEDMod"
        );
        assert!(
            entries.iter().all(|n| !n.starts_with("DISABLEDDISABLED")),
            "不应重复加前缀产生 DISABLEDDISABLEDxxx"
        );
    }

    /// 测试：enable_mutex_mod 目标匹配大小写不敏感，目标正确启用、兄弟正确禁用
    ///
    /// Windows 下路径大小写不敏感，用大小写差异的 mod_path 调用时目标应被识别并启用，
    /// 而非被误判为"兄弟"禁用（缺陷 B 回归测试）。
    #[test]
    fn test_enable_mutex_mod_case_insensitive_target_match() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path();
        // 目标 ModA、兄弟 ModB（均含 .ini）
        let mod_a = parent.join("ModA");
        fs::create_dir_all(&mod_a).unwrap();
        fs::write(mod_a.join("mod.ini"), "[ShaderOverride]").unwrap();
        let mod_b = parent.join("ModB");
        fs::create_dir_all(&mod_b).unwrap();
        fs::write(mod_b.join("mod.ini"), "[ShaderOverride]").unwrap();

        // 以大小写差异的路径调用（Windows 下视为同一目录）
        let target = parent.join("moda");
        enable_mutex_mod(&target).unwrap();

        assert!(
            parent.join("ModA").exists(),
            "目标 ModA 应被识别并保持启用（未被改名）"
        );
        assert!(
            parent.join("DISABLEDModB").exists(),
            "兄弟 ModB 应被禁用（加 DISABLED 前缀）"
        );
    }

    /// 测试：rename_mod 互斥分支 final_name 前缀防重复
    ///
    /// 复刻 mod_commands::rename_mod 的 core 逻辑（validate + spawn_blocking 为 Tauri 命令
    /// 封装不便直接调用），验证 new_name 带 DISABLED 前缀时最终只保有一个前缀。
    #[test]
    fn test_rename_mod_final_name_no_duplicate_prefix() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path();
        let mod_path = parent.join("DISABLEDMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[ShaderOverride]").unwrap();

        // 危险场景：用户重命名 new_name 已带 DISABLED 前缀
        let is_disabled =
            mod_scanner::is_disabled_dir(&mod_path.file_name().unwrap().to_string_lossy());
        let new_name = "DISABLEDNewName".to_string();
        let final_name = if is_disabled {
            format!(
                "{}{}",
                constants::DISABLED_PREFIX,
                mod_scanner::strip_disabled_prefix(&new_name)
            )
        } else {
            new_name
        };

        assert_eq!(
            final_name, "DISABLEDNewName",
            "重命名结果应只含一个 DISABLED 前缀，而非 DISABLEDDISABLEDNewName"
        );
    }
}

#[cfg(test)]
mod tests_remove_group_ex {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_mod_dir(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{}.ini", name.trim_start_matches("DISABLED"))),
            "[ShaderOverride]",
        )
        .unwrap();
        dir
    }

    /// 底层移动：正常移动目录到目标删除目录
    #[test]
    fn test_move_dir_to_removed_basic() {
        let tmp = TempDir::new().unwrap();
        let removed_dir = tmp.path().join("removed");
        let src = tmp.path().join("MyItem");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), "x").unwrap();

        let moved = move_dir_to_removed(&src, &removed_dir).unwrap();

        assert!(!src.exists(), "源目录应被移除");
        assert!(removed_dir.exists(), "目标删除目录应被创建");
        assert_eq!(moved, removed_dir.join("MyItem"));
        assert!(moved.join("a.txt").exists(), "内容应随目录移动");
    }

    /// 底层移动：目标名冲突时追加 _1 后缀
    #[test]
    fn test_move_dir_to_removed_conflict() {
        let tmp = TempDir::new().unwrap();
        let removed_dir = tmp.path().join("removed");
        fs::create_dir_all(&removed_dir).unwrap();
        fs::create_dir_all(removed_dir.join("MyItem")).unwrap();

        let src = tmp.path().join("MyItem");
        fs::create_dir_all(&src).unwrap();

        let moved = move_dir_to_removed(&src, &removed_dir).unwrap();

        assert_eq!(moved, removed_dir.join("MyItem_1"), "冲突应追加 _1 后缀");
    }

    /// group_xx 场景：移至 DISABLED_MANAGED_REMOVED 目录
    #[test]
    fn test_remove_group_xx() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let group = managed.join("group_1");
        fs::create_dir_all(&group).unwrap();
        create_mod_dir(&group, "ModA");

        remove_group_ex(&group, true, mods_root).unwrap();

        // 原分组应不存在
        assert!(!group.exists(), "原 group_1 应被移除");
        // DISABLED_MANAGED_REMOVED 下应有 group_1
        let removed_root = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
        assert!(removed_root.exists());
        let removed_group = removed_root.join("group_1");
        assert!(
            removed_group.exists(),
            "DISABLED_MANAGED_REMOVED 下应存在 group_1"
        );
        assert!(removed_group.join("ModA").exists(), "模组应随之移动");
    }

    /// 非group场景：先移子分组到父级，再移至 DISABLED_MANAGED_REMOVED
    #[test]
    fn test_remove_group_non_group_with_subgroups() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let non_group = mods_root.join("#MyMutexGroup");
        fs::create_dir_all(&non_group).unwrap();
        // 叶子模组（随父目录移除）
        create_mod_dir(&non_group, "ModA");
        // 子分组（无 .ini，应移至父级 mods_root）
        let subgroup1 = non_group.join("SubGroup1");
        fs::create_dir_all(&subgroup1).unwrap();
        create_mod_dir(&subgroup1, "SubMod1");
        let subgroup2 = non_group.join("SubGroup2");
        fs::create_dir_all(&subgroup2).unwrap();

        remove_group_ex(&non_group, false, mods_root).unwrap();

        // 原分组应不存在
        assert!(!non_group.exists());
        // 子分组应在 mods_root 下
        assert!(
            mods_root.join("SubGroup1").exists(),
            "SubGroup1 应移至 Mods 根目录"
        );
        assert!(
            mods_root.join("SubGroup1").join("SubMod1").exists(),
            "SubGroup1 下的模组应保留"
        );
        assert!(
            mods_root.join("SubGroup2").exists(),
            "SubGroup2 应移至 Mods 根目录"
        );
        // 被移除目录在 DISABLED_MANAGED_REMOVED 下
        let removed = mods_root
            .join(constants::MANAGED_REMOVED_FOLDER)
            .join("#MyMutexGroup");
        assert!(removed.exists());
        assert!(removed.join("ModA").exists(), "ModA 随原分组被移除");
        assert!(
            !removed.join("SubGroup1").exists(),
            "SubGroup1 不应随原分组被移除"
        );
    }

    /// 名称冲突场景：DISABLED_MANAGED_REMOVED/group_1 已存在应追加 _1
    #[test]
    fn test_remove_group_conflict() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let group = managed.join("group_1");
        let removed_root = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
        fs::create_dir_all(&group).unwrap();
        // 已存在的已删除组
        fs::create_dir_all(removed_root.join("group_1")).unwrap();

        remove_group_ex(&group, true, mods_root).unwrap();

        assert!(!group.exists());
        assert!(removed_root.join("group_1").exists(), "原冲突项保留");
        assert!(
            removed_root.join("group_1_1").exists(),
            "新移除项追加 _1 后缀"
        );
    }
}

#[cfg(test)]
mod tests_remove_mod {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// locate_mods_root：模组位于 _MANAGED_/group_X 下时，应返回 _MANAGED_ 的父目录（Mods 根目录）
    #[test]
    fn test_locate_mods_root_from_managed_group() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let group = managed.join("group_1");
        let mod_path = group.join("MyMod");
        fs::create_dir_all(&mod_path).unwrap();

        let root = locate_mods_root(&mod_path).unwrap();
        assert_eq!(root, mods_root, "应返回 Mods 根目录（_MANAGED_ 的父目录）");
    }

    /// locate_mods_root：模组位于 _MANAGED_/#MutexGroup 下时，同样返回 Mods 根目录
    #[test]
    fn test_locate_mods_root_from_mutex_group() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let mutex_group = managed.join("#MutexGroup");
        let mod_path = mutex_group.join("MyMod");
        fs::create_dir_all(&mod_path).unwrap();

        let root = locate_mods_root(&mod_path).unwrap();
        assert_eq!(root, mods_root);
    }

    /// remove_mod：移除的模组必须位于 Mods/DISABLED_MANAGED_REMOVED 下，
    /// 而非 Mods/_MANAGED_/DISABLED_MANAGED_REMOVED 下（用户报告的关键场景）
    #[test]
    fn test_remove_mod_moved_to_mods_root_removed_folder() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let group = managed.join("group_1");
        let mod_path = group.join("MyMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("MyMod.ini"), "[ShaderOverride]").unwrap();

        let result = remove_mod(&mod_path).unwrap();

        // 原模组路径应不存在
        assert!(!mod_path.exists(), "原模组路径应被移除");

        // 目标必须位于 Mods/DISABLED_MANAGED_REMOVED（与 _MANAGED_ 同级）
        let expected = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
        assert!(
            expected.exists(),
            "DISABLED_MANAGED_REMOVED 应创建在 Mods 根目录"
        );
        assert_eq!(result.moved_to.parent(), Some(expected.as_path()));

        // 明确验证：不得创建在 Mods/_MANAGED_ 之下
        let wrongly_placed = managed.join(constants::MANAGED_REMOVED_FOLDER);
        assert!(
            !wrongly_placed.join("MyMod").exists(),
            "不得创建到 _MANAGED_ 之下"
        );
        assert!(
            result.moved_to.join("MyMod.ini").exists(),
            "模组文件应随目录移动"
        );
    }

    /// remove_mod：删除多个模组时，DISABLED_MANAGED_REMOVED 始终位于 Mods 根目录，冲突追加 _1
    #[test]
    fn test_remove_mod_conflict_appends_suffix() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let group = managed.join("group_1");
        let mod1 = group.join("MyMod");
        let mod2 = group.join("MyMod2");
        fs::create_dir_all(&mod1).unwrap();
        fs::create_dir_all(&mod2).unwrap();
        fs::write(mod1.join("MyMod.ini"), "[ShaderOverride]").unwrap();
        fs::write(mod2.join("MyMod2.ini"), "[ShaderOverride]").unwrap();

        let r1 = remove_mod(&mod1).unwrap();
        let r2 = remove_mod(&mod2).unwrap();

        let removed_root = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
        assert_eq!(r1.moved_to, removed_root.join("MyMod"));
        assert_eq!(r2.moved_to, removed_root.join("MyMod2"));
        assert!(
            !managed.join(constants::MANAGED_REMOVED_FOLDER).exists(),
            "不得创建到 _MANAGED_ 之下"
        );
    }
}


// ============================================================================
// 非group目录隔离性测试
//
// 验证核心需求：更新模组数据、groupname 等文件的生成不涉及非group目录，
// 非group目录仅参与模组读取和 hash 冲突检测。
// ============================================================================
#[cfg(test)]
mod tests_non_group_isolation {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// 创建带 _MANAGED_ 子目录的临时根目录
    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        dir
    }

    /// 在分组目录下创建带 INI 文件的模组目录
    fn create_mod_with_ini(group_path: &Path, mod_name: &str, ini_content: &str) -> PathBuf {
        let mod_path = group_path.join(mod_name);
        fs::create_dir_all(&mod_path).unwrap();
        let ini_path = mod_path.join("mod.ini");
        fs::write(&ini_path, ini_content).unwrap();
        mod_path
    }

    /// 在根目录创建 d3dx.ini
    fn create_d3dx_ini(base: &Path) {
        fs::write(base.join("d3dx.ini"), "; test\n").unwrap();
    }



    /// 测试：collect_mod_ini_files 正确收集模组目录下的 .ini 文件
    ///
    /// 验证：
    /// - 收集直接子 .ini 文件
    /// - 不递归子目录
    /// - 排除 desktop.ini
    /// - 排除 .ini_managed_backup 备份文件
    #[test]
    fn test_collect_mod_ini_files() {
        let dir = TempDir::new().unwrap();
        let mod_dir = dir.path().join("TestMod");
        fs::create_dir_all(&mod_dir).unwrap();

        // 正常 .ini 文件
        fs::write(mod_dir.join("mod.ini"), "[Section]\n").unwrap();
        fs::write(mod_dir.join("config.ini"), "[Other]\n").unwrap();
        // desktop.ini 应被排除
        fs::write(mod_dir.join("desktop.ini"), "[System]\n").unwrap();
        // .ini_managed_backup 备份文件应被排除
        fs::write(mod_dir.join("mod.ini_managed_backup"), "[Backup]\n").unwrap();
        // 子目录中的 .ini 应被排除（不递归）
        let subdir = mod_dir.join("SubDir");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(subdir.join("nested.ini"), "[Nested]\n").unwrap();
        // 非 .ini 文件应被排除
        fs::write(mod_dir.join("readme.txt"), "text\n").unwrap();

        let ini_files = collect_mod_ini_files(&mod_dir);
        // 应收集到 mod.ini 和 config.ini（2 个文件）
        assert_eq!(
            ini_files.len(),
            2,
            "应仅收集直接 .ini 文件，排除 desktop/backup/nested"
        );
        let file_names: Vec<String> = ini_files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(file_names.contains(&"mod.ini".to_string()));
        assert!(file_names.contains(&"config.ini".to_string()));
    }

    /// 测试：update_mod_data 不涉及非group目录的标记文件生成
    ///
    /// 验证：调用 update_mod_data 后，非group目录下不会创建 groupname/selectedindex/modname
    #[test]
    fn test_update_mod_data_no_markers_in_non_group() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let settings = AppSettings::default();

        // group_1 下的模组
        let group1 = dir.path().join("_MANAGED_").join("group_1");
        fs::create_dir_all(&group1).unwrap();
        create_mod_with_ini(&group1, "NormalMod", "[TextureOverride]\nhash=0x1\n");

        // 非group目录下的模组
        let mutex_group = dir.path().join("_MANAGED_").join("#Mutex");
        fs::create_dir_all(&mutex_group).unwrap();
        let mutex_mod =
            create_mod_with_ini(&mutex_group, "MutexMod", "[TextureOverride]\nhash=0x2\n");

        let _result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();

        // 验证非group目录下未创建标记文件
        assert!(
            !mutex_group.join("groupname").exists(),
            "update_mod_data 不应在非group目录创建 groupname 文件"
        );
        assert!(
            !mutex_group.join(constants::SELECTED_INDEX_FILE).exists(),
            "update_mod_data 不应在非group目录创建 selectedindex 文件"
        );
        assert!(
            !mutex_mod.join("modname").exists(),
            "update_mod_data 不应在非group目录下模组创建 modname 文件"
        );
    }


    #[test]
    fn test_validate_group_ini_consistency() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        // 创建 group_1 分组目录和模组
        let group_path = dir.path().join("_MANAGED_").join("group_1");
        fs::create_dir_all(&group_path).unwrap();
        create_mod_with_ini(&group_path, "TestMod", "[Constants]\nx=1\n");

        // 创建一致的 group_1.ini：global $group_id = 1（与目录 group_1 匹配）
        let good_ini = "[Constants]\nglobal $group_id = 1\n";
        fs::write(group_path.join("group_1.ini"), good_ini).unwrap();

        let scan = mod_scanner::scan_mods_deep(dir.path()).unwrap();
        let enabled: Vec<&ModData> = scan
            .mods
            .iter()
            .filter(|m| !m.disabled)
            .map(|m| m.as_ref())
            .collect();
        let managed = dir.path().join("_MANAGED_");
        let mismatched = validate_group_ini_consistency(&managed, &enabled);
        assert!(mismatched.is_empty(), "一致的 group_ini 不应报错");

        // 创建不一致的 group_1.ini：global $group_id = 99（与目录 group_1 不匹配）
        let bad_ini = "[Constants]\nglobal $group_id = 99\n";
        fs::write(group_path.join("group_1.ini"), bad_ini).unwrap();
        let mismatched = validate_group_ini_consistency(&managed, &enabled);
        assert_eq!(mismatched, vec![1], "不一致的 group_ini 应返回 gid=1");
    }
}

