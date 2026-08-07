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

use anyhow::{Result, Context};
use std::path::{Path, PathBuf};
use std::collections::HashSet;
use std::fs;
use crate::core::constants;
use crate::core::ini_handler::IniFile;
use crate::core::d3dxini_cache::D3DX_INI_CACHE;
use crate::core::namespace_handler;
use crate::core::mod_scanner;
use crate::models::enums::TargetGame;
use crate::models::mod_data::{ModData, ModGroupData, ErroredLines, HashConflict, HashConflictEntry, LibInMod, DuplicateLib, NonExistentLib};
use crate::models::enums::GroupType;
use crate::models::settings::AppSettings;

// 重导出供 commands 层使用（避免 commands 直接依赖 models 模块）
// 同时供本模块内部使用（pub use 也会将名称引入当前作用域）
pub use crate::models::mod_data::{HashConflictResult, OrfixDetection};

/// 模组更新结果结构体
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct UpdateResult {
    /// 总分组数
    pub total_groups: u32,
    /// 总模组数
    pub total_mods: u32,
    /// 启用的模组数
    pub enabled_mods: u32,
    /// 禁用的模组数
    pub disabled_mods: u32,
    /// 实际处理（INI 注入）的模组数
    pub processed_mods: u32,
    /// INI 解析/处理过程中的错误
    pub errors: Vec<ErroredLines>,
    /// 是否需要用户手动重载（3Dmigoto）
    #[serde(default)]
    pub need_reload_manual: bool,
    /// 选择操作（switch_mod）成功写入磁盘 selectedindex 文件的最终值。
    /// - NormalGroup：范围为 [0, g.mods.len()-1]，0 代表 None 槽位
    /// - update_mod_data（非选择类操作）：默认返回 None（null）
    /// - MutexGroup：不使用 switch_mod 路径，始终 None
    #[serde(default)]
    pub selected_mod_index: Option<i32>,
    /// 是否检测到标准 XXMI/3DMigoto 环境（[Constants] + XXMI 注入标记 + [Inject] 段）
    #[serde(default)]
    pub is_standard_xxmi: bool,
    /// ORFix/TexFx 检测结果（在 INI 修改之前使用原始内容检测）
    #[serde(default)]
    pub orfix_detection: OrfixDetection,
}

/// INI 恢复结果统计
#[derive(Debug, Clone, serde::Serialize)]
pub struct RestoredCount {
    /// 成功恢复的文件数
    pub restored: u32,
    /// 恢复失败的文件数
    pub failed: u32,
}

/// Save Customizations 结果
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct SaveCustomizationsResult {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
}

/// 检测当前游戏是否为标准 XXMI/3DMigoto 环境
///
/// 参考 NRMM `ini_handler_bridge.dart` 逻辑：
/// 1. 读取 `{game_mods_path}/d3dx.ini`（或游戏特定主 INI）
/// 2. 检查 `[Constants]` 段中是否存在 XXMI Launcher 注入标记
///    （包含 `xxmi`、`xxmi_inject`、`global $xxmi`、`global $inject` 等键名或值）
/// 3. 检查是否存在 `[Inject]` 段或包含 `run = CommandList` 引用
/// 4. 以上条件均满足时返回 `true`
///
/// 任何错误（文件不存在、解析失败等）均返回 `false` 且不报错，
/// 仅记录 `log::debug!` 供开发调试。
///
/// # 参数
/// - `game_mods_path`: 游戏 Mods 目录路径
/// - `main_ini_name`: 主 INI 文件名（由 TargetGame::d3dx_ini_name() 提供）
///
/// # 返回
/// - `true`: 检测到标准 XXMI/3DMigoto 环境
/// - `false`: 未检测到或检测过程发生错误
pub fn detect_standard_xxmi(game_mods_path: &Path, main_ini_name: &str) -> bool {
    let _start = std::time::Instant::now();
    let main_ini_path = game_mods_path.join(main_ini_name);

    // 1. 读取主 INI 文件
    let content = match fs::read_to_string(&main_ini_path) {
        Ok(c) => c,
        Err(e) => {
            log::debug!(
                "[mod_manager] [detect_standard_xxmi] Main INI not found or unreadable: {:?}, err={}",
                main_ini_path, e
            );
            return false;
        }
    };

    // 2. 解析 INI，检查 [Constants] 和 [Inject] 段
    let mut in_constants = false;
    let mut has_xxmi_mark = false;
    let mut has_inject_section = false;
    let mut has_run_commandlist = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // 跳过空行和注释
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
            continue;
        }

        // 段头
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = trimmed[1..trimmed.len() - 1].trim().to_lowercase();
            in_constants = section == "constants";
            if section == "inject" {
                has_inject_section = true;
            }
            continue;
        }

        // [Constants] 段内：查找 XXMI 注入标记变量
        if in_constants {
            let lower = trimmed.to_lowercase();
            // XXMI Launcher 常见注入标记：global $xxmi_xxx、global $inject、xxmi_inject、$xxmi_inject 等
            if lower.contains("xxmi")
                || (lower.contains("global") && (lower.contains("$inject") || lower.contains("inject")))
                || lower.contains("xxmi_inject")
                || lower.contains("$xxmi")
            {
                has_xxmi_mark = true;
            }
        }

        // 任意位置：检查 run = CommandList
        let lower = trimmed.to_lowercase();
        if lower.contains("run") && lower.contains("commandlist") {
            has_run_commandlist = true;
        }
    }

    let result = has_xxmi_mark && (has_inject_section || has_run_commandlist);

    log::debug!(
        "[mod_manager] [detect_standard_xxmi] result={} | has_xxmi_mark={}, has_inject_section={}, has_run_commandlist={}, elapsed={:?}ms",
        result,
        has_xxmi_mark,
        has_inject_section,
        has_run_commandlist,
        _start.elapsed().as_millis()
    );

    result
}

/// 检测 hash 冲突
///
/// 全量扫描模组 INI 中的 hash 值，按模组为单位返回冲突列表。
///
/// 扫描策略（对齐 NRMM）：
/// - NormalGroup（group_xx）：仅扫描当前选中模组（`is_active=true`）的 INI
/// - MutexGroup（非 group_xx）：扫描所有启用模组（`!disabled && !mod_disabled`）的 INI
///
/// hash 值来源：`[TextureOverride.xxx]` / `[ShaderOverride.xxx]` 段下的 `hash` 键，
/// 提取时统一转小写做归一化。同一模组内多次使用同一 hash 不算冲突，
/// 仅当同一 hash 被 ≥2 个不同模组使用时才计入冲突。
///
/// # entries 字段
/// 每条 `HashConflict` 的 `entries` 字段记录单个模组使用该 hash 的详情，
/// 每个模组一条 `HashConflictEntry`，其 `ini_vec` 保存该模组所有包含此 hash 的
/// INI 文件绝对路径（去重粒度为 (mod_name, ini_path)：同一模组同一 INI 内多次出现
/// 同一 hash 只算一条，同一模组不同 INI 各保留一条）。前端据此实现悬浮提示
/// （显示模组目录）和点击展开详情（按模组分组显示 INI 路径列表）。
///
/// # 非group目录参与策略
/// 使用 `scan_mods_light` 而非 `scan_mods_deep`，确保 MutexGroup（非group目录）
/// 下的模组也参与 hash 冲突检测。轻量扫描不解析 INI 内容，因此本函数会在
/// 扫描时按需从模组目录查找 .ini 文件并解析。非group目录不涉及标记文件生成，
/// 仅参与模组读取和 hash 冲突检测。
///
/// # 参数
/// - `game_mods_path`: 游戏 Mods 目录路径
///
/// # 返回
/// `HashConflictResult` — 含所有冲突列表（按 hash 值升序排序）、扫描统计
pub fn detect_hash_conflicts(game_mods_path: &Path) -> Result<HashConflictResult> {
    let _s = std::time::Instant::now();
    // 使用 scan_mods_light 而非 scan_mods_deep：
    // 深度扫描仅处理 group_xx 目录，会遗漏 MutexGroup（非group目录）下的模组。
    // 需求要求非group目录参与 hash 冲突检测，故改用轻量扫描以包含两类分组。
    let scan_result = mod_scanner::scan_mods_light(game_mods_path)?;
    // 三元组：(mod_name, mod_path, ini_path)
    // 记录 INI 文件路径以支持前端悬浮提示和点击展开详情
    let mut hash_to_mods: std::collections::HashMap<String, Vec<(String, String, PathBuf)>> =
        std::collections::HashMap::new();
    let mut scanned_mods = 0u32;
    let mut scanned_hashes = 0u32;

    fn process_group(
        group: &ModGroupData,
        hash_to_mods: &mut std::collections::HashMap<String, Vec<(String, String, PathBuf)>>,
        scanned_mods: &mut u32,
        scanned_hashes: &mut u32,
    ) {
        let is_normal = group.group_type == GroupType::NormalGroup;
        for mod_data in &group.mods {
            // NormalGroup 仅扫描 is_active 模组；MutexGroup 扫描所有启用模组
            let should_scan = if is_normal {
                mod_data.is_active && !mod_data.disabled && !mod_data.mod_disabled
            } else {
                !mod_data.disabled && !mod_data.mod_disabled
            };
            if !should_scan || mod_data.mod_name == "None" {
                continue;
            }

            let mod_name = mod_data.name.clone();
            let mod_path = mod_data.mod_path.clone();
            *scanned_mods += 1;

            // 轻量扫描未解析 INI，需从模组目录查找 .ini 文件并按需解析
            for ini_path in collect_mod_ini_files(&mod_data.full_path) {
                if let Ok(ini) = D3DX_INI_CACHE.write().get_or_parse(&ini_path) {
                    for (_section, hash) in ini.extract_hashes() {
                        *scanned_hashes += 1;
                        hash_to_mods
                            .entry(hash)
                            .or_default()
                            .push((mod_name.clone(), mod_path.clone(), ini_path.clone()));
                    }
                }
            }
        }
        // 递归子分组
        for child in &group.children {
            process_group(child, hash_to_mods, scanned_mods, scanned_hashes);
        }
    }

    for group in &scan_result.groups {
        process_group(
            group,
            &mut hash_to_mods,
            &mut scanned_mods,
            &mut scanned_hashes,
        );
    }

    // 筛选冲突：同一 hash 被 ≥2 个不同模组使用
    let mut conflicts: Vec<HashConflict> = hash_to_mods
        .into_iter()
        .filter_map(|(hash, mods)| {
            // 去重粒度为 (mod_name, ini_path)：
            // 同一模组同一 INI 内多次出现同一 hash 只算一条；
            // 同一模组不同 INI 各保留一条，聚合到该模组的 ini_vec。
            let mut unique_entries: Vec<(String, String, PathBuf)> = Vec::new();
            for (name, path, ini) in mods.into_iter() {
                if !unique_entries.iter().any(|(n, _, i)| n == &name && i == &ini) {
                    unique_entries.push((name, path, ini));
                }
            }
            // 仅当涉及 ≥2 个不同模组时才算冲突
            let unique_mod_count = unique_entries.iter()
                .map(|(n, _, _)| n.clone())
                .collect::<std::collections::HashSet<_>>()
                .len();
            if unique_mod_count >= 2 {
                // 按模组聚合：key=mod_name, value=(mod_path, ini 列表)，每个模组一条 entry
                let mut mod_map: std::collections::HashMap<String, (String, Vec<String>)> =
                    std::collections::HashMap::new();
                for (n, p, ini) in unique_entries.into_iter() {
                    let ini_str = ini.to_string_lossy().to_string();
                    mod_map
                        .entry(n.clone())
                        .or_insert_with(|| (p.clone(), Vec::new()))
                        .1
                        .push(ini_str);
                }
                let entries: Vec<HashConflictEntry> = mod_map
                    .into_iter()
                    .map(|(mod_name, (mod_path, ini_vec))| HashConflictEntry {
                        mod_name,
                        mod_path,
                        ini_vec,
                    })
                    .collect();
                Some(HashConflict { hash, entries })
            } else {
                None
            }
        })
        .collect();

    // 按 hash 值排序，保证输出稳定
    conflicts.sort_by(|a, b| a.hash.cmp(&b.hash));

    log::info!(
        "[mod_manager] [detect_hash_conflicts] done | elapsed={:?}ms | scanned_mods={} scanned_hashes={} conflicts={}",
        _s.elapsed().as_millis(),
        scanned_mods,
        scanned_hashes,
        conflicts.len()
    );

    Ok(HashConflictResult {
        conflicts,
        scanned_mods,
        scanned_hashes,
    })
}

/// 检测 ORFix/TexFx 命名空间异常
///
/// 三种检测（对齐 NRMM `checkLibraries` 逻辑）：
/// 1. **库在模组内**（`libs_in_mods`）：模组 INI 声明了已知库命名空间
///    （`global\orfix` / `texfx`），这些库应位于 Mods 根目录而非模组内。
/// 2. **重复声明**（`duplicate_libs`）：同一已知库命名空间被多个模组声明。
/// 3. **引用未声明**（`nonexistent_libs`）：`run =` 引用了已知库但全局无任何模组声明。
///
/// 仅扫描传入的启用模组列表，跳过 None 空槽位。
///
/// # 参数
/// - `enabled_mods`: 启用模组列表（引用）
///
/// # 返回
/// `OrfixDetection` — 含三类检测结果及 `has_detection` 汇总标志
pub fn detect_orfix_texfx(enabled_mods: &[&ModData]) -> OrfixDetection {
    let known_libs = constants::known_lib_namespaces_set();
    let mut libs_in_mods: Vec<LibInMod> = Vec::new();
    // lib_display → 声明该库的模组名列表
    let mut lib_declarations: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    // lib_display → 引用该库的模组名列表
    let mut lib_references: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    for mod_data in enabled_mods {
        if mod_data.mod_name == "None" {
            continue;
        }
        let mod_name = mod_data.name.clone();
        let mod_path = mod_data.mod_path.clone();

        for ini_data in &mod_data.mod_ini_data {
            let ini_path = PathBuf::from(&ini_data.ini_path);
            if let Ok(ini) = D3DX_INI_CACHE.write().get_or_parse(&ini_path) {
                // 1. 检测库在模组内（同时记入声明表，用于重复声明检测）
                let detected = ini.detect_known_lib_declarations(&known_libs);
                if !detected.is_empty() {
                    libs_in_mods.push(LibInMod {
                        mod_name: mod_name.clone(),
                        mod_path: mod_path.clone(),
                        lib_names: detected.clone(),
                    });
                    for lib in &detected {
                        lib_declarations
                            .entry(lib.clone())
                            .or_default()
                            .push(mod_name.clone());
                    }
                }

                // 2. 收集 run = 引用（仅引用已知库命名空间的）
                for (ns, _) in ini.extract_run_references(&known_libs) {
                    if let Some(display) = constants::lookup_lib_display_name(&ns) {
                        lib_references
                            .entry(display.to_string())
                            .or_default()
                            .push(mod_name.clone());
                    }
                }
            }
        }
    }

    // 3. 重复声明：同一库被 ≥2 个模组声明
    let duplicate_libs: Vec<DuplicateLib> = lib_declarations
        .into_iter()
        .filter_map(|(lib, mods)| {
            let mut unique: Vec<String> = mods.into_iter().collect::<std::collections::HashSet<_>>().into_iter().collect();
            unique.sort();
            if unique.len() >= 2 {
                Some(DuplicateLib {
                    lib_name: lib,
                    mod_names: unique,
                })
            } else {
                None
            }
        })
        .collect();

    // 4. 引用未声明：引用了但全局无任何模组声明
    let nonexistent_libs: Vec<NonExistentLib> = lib_references
        .into_iter()
        .filter_map(|(lib, mods)| {
            // 检查该库是否被任何模组声明
            let is_declared = libs_in_mods.iter().any(|m| m.lib_names.contains(&lib));
            if !is_declared {
                let mut unique: Vec<String> = mods.into_iter().collect::<std::collections::HashSet<_>>().into_iter().collect();
                unique.sort();
                Some(NonExistentLib {
                    lib_name: lib,
                    mod_names: unique,
                })
            } else {
                None
            }
        })
        .collect();

    let has_detection =
        !libs_in_mods.is_empty() || !duplicate_libs.is_empty() || !nonexistent_libs.is_empty();

    OrfixDetection {
        libs_in_mods,
        duplicate_libs,
        nonexistent_libs,
        has_detection,
    }
}

/// 重量级更新模组数据（完整 INI 解析 + 注入）
///
/// 这是模组管理的核心函数，严格复刻 NRMM 的 updateModData 流程：
/// 1. 准备 _MANAGED_ 目录（创建必要的模板 INI 文件）
/// 2. 深度扫描所有模组（完整解析 INI）
/// 3. 备份主 INI（d3dx.ini/RatioShot.ini）
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
pub fn update_mod_data(game: TargetGame, game_mods_path: &Path, _settings: &AppSettings) -> Result<UpdateResult> {
    log::debug!("[core::mod_manager] [update_mod_data] Starting heavy update | game={:?} path={:?}", game, game_mods_path);
    let _s = std::time::Instant::now();
    let managed_folder = game_mods_path.join(constants::MANAGED_FOLDER);
    if !managed_folder.exists() {
        fs::create_dir_all(&managed_folder)?;
    }

    // 步骤1: 准备 _MANAGED_ 目录，创建模板 INI 文件
    let mut need_reload_manual = prepare_managed_folder(&managed_folder, game)?;
    log::debug!("[mod_manager] [update_mod_data] step=prepare_managed_folder done need_reload_manual={}", need_reload_manual);

    // 步骤2: 扫描模组
    let scan_result = mod_scanner::scan_mods(game_mods_path)?;
    log::debug!("[mod_manager] [update_mod_data] step=scan_mods done total_mods={} total_groups={}", scan_result.total_mods_count, scan_result.groups.len());

    let main_ini_name = game.d3dx_ini_name();
    let main_ini_path = game_mods_path.join(main_ini_name);

    if !main_ini_path.exists() {
        create_default_main_ini(&main_ini_path, main_ini_name)?;
    }

    // 备份主 INI
    let backup_path = main_ini_path.with_extension(constants::BACKUP_EXTENSION);
    if !backup_path.exists() {
        fs::copy(&main_ini_path, &backup_path)
            .with_context(|| format!("Failed to backup main INI: {:?}", main_ini_path))?;
    }

    let main_ini_content = IniFile::force_read_as_utf8(&main_ini_path)?;
    let main_ini_content = strip_nrmm_injected_content(&main_ini_content);

    // 步骤3: 收集启用模组和已知库
    let enabled_mods: Vec<&ModData> = scan_result.mods.iter()
        .filter(|m| !m.disabled && !m.mod_disabled)
        .collect();
    log::debug!("[mod_manager] [update_mod_data] step=collect_enabled done enabled_mods_count={}", enabled_mods.len());

    let mut known_libraries = HashSet::new();
    for mod_data in &enabled_mods {
        for ini_data in &mod_data.mod_ini_data {
            let ini_path = PathBuf::from(&ini_data.ini_path);
            if let Ok(ini) = D3DX_INI_CACHE.write().get_or_parse(&ini_path) {
                for lib in ini.defined_libraries() {
                    known_libraries.insert(lib);
                }
            }
        }
    }

    // 步骤3.5: ORFix/TexFx 检测（在 INI 修改之前，使用原始 INI 内容）
    let orfix_detection = detect_orfix_texfx(&enabled_mods);
    log::debug!("[mod_manager] [update_mod_data] step=detect_orfix_texfx done has_detection={}", orfix_detection.has_detection);

    // 步骤4: 清理旧的 group INI 文件
    delete_group_ini_files(&managed_folder)?;
    log::debug!("[mod_manager] [update_mod_data] step=delete_group_ini_files done");

    // 步骤5: 按 group 组织启用的模组 INI
    let mut group_mod_inis: std::collections::HashMap<u32, Vec<PathBuf>> = std::collections::HashMap::new();
    let mut all_errors: Vec<ErroredLines> = Vec::new();
    let mut processed_mods = 0u32;

    for mod_data in enabled_mods.iter() {
        let group_id = mod_data.group_index;
        let mut mod_inis: Vec<PathBuf> = Vec::new();

        for ini_data in &mod_data.mod_ini_data {
            let ini_path = PathBuf::from(&ini_data.ini_path);

            // 备份模组 INI
            let mod_backup = ini_path.with_extension(constants::BACKUP_EXTENSION);
            if !mod_backup.exists() {
                if let Err(e) = fs::copy(&ini_path, &mod_backup) {
                    log::warn!("Failed to backup mod INI {}: {}", ini_path.display(), e);
                }
            }

            match D3DX_INI_CACHE.write().get_or_parse(&ini_path) {
                Ok(mut ini) => {
                    // 错误检测
                    let errors = ini.detect_errors(&ini_path, &known_libraries);
                    if !errors.is_empty() {
                        all_errors.extend(errors);
                    }

                    // 展开 namespace 变量
                    if let Some(ns) = namespace_handler::extract_namespace(&ini) {
                        namespace_handler::expand_ini_variables(&mut ini, &ns);
                    }

                    // 注入槽位条件
                    ini.inject_slot_conditions(group_id);

                    // 注释崩溃行
                    let crash_lines = ini.comment_crash_lines();
                    if !crash_lines.is_empty() {
                        log::info!("Commented {} crash lines in {}", crash_lines.len(), ini_path.display());
                    }

                    // 移除空 if 块，应用缩进
                    ini.remove_empty_if_blocks();
                    ini.apply_indentation();
                    ini.prepend_header_comment();

                    // 原子写入
                    ini.write_atomic(&ini_path)?;

                    mod_inis.push(ini_path.clone());
                    processed_mods += 1;
                }
                Err(e) => {
                    log::error!("Failed to process mod INI {}: {}", ini_path.display(), e);
                    all_errors.push(ErroredLines {
                        error_type: 3,
                        error_message: format!("Processing error: {}", e),
                        ..Default::default()
                    });
                }
            }
        }

        group_mod_inis.entry(group_id).or_default().extend(mod_inis);
    }
    log::debug!("[mod_manager] [update_mod_data] step=process_mod_inis done processed={} groups={} errors={}", processed_mods, group_mod_inis.len(), all_errors.len());

    // 步骤6: 为每个 group 创建 ModFolder.ini
    let mut group_ini_paths: Vec<PathBuf> = Vec::new();
    for (group_id, ini_paths) in &group_mod_inis {
        let group_dir = managed_folder.join(format!("group_{}", group_id));
        let group_ini_path = create_group_ini(&group_dir, *group_id, ini_paths, game_mods_path)?;
        if let Some(p) = group_ini_path {
            group_ini_paths.push(p);
        }
    }

    // 步骤7: 生成 nrmm_include.ini
    let nrmm_include_path = managed_folder.join(constants::INCLUDE_FILENAME);
    create_nrmm_include_ini(&nrmm_include_path, &managed_folder, &group_ini_paths, game_mods_path)?;

    // 步骤8: 不再向 d3dx.ini 注入 include（对齐 NRMM 原版）
    // NRMM 原版依赖标准 XXMI 的 include_recursive = Mods 自动加载 _MANAGED_ 下的 .ini 文件。
    // 之前的实现将 include 放入 [Constants] 段，3Dmigoto 不处理此位置的 include 指令，
    // 导致 nrmm_keypress.txt / manager_group.ini / group_N.ini 未被加载。
    // 现仅保留清理旧注入内容（main_ini_content 已通过 strip_nrmm_injected_content 清理）。
    let final_content = main_ini_content;

    // 原子写入主 INI
    let tmp_path = main_ini_path.with_extension("ini.tmp");
    fs::write(&tmp_path, &final_content)
        .with_context(|| format!("Failed to write temp main INI: {:?}", tmp_path))?;
    fs::rename(&tmp_path, &main_ini_path)
        .with_context(|| format!("Failed to rename temp main INI to: {:?}", main_ini_path))?;
    log::debug!("[mod_manager] [update_mod_data] step=write_main_ini done path={:?}", main_ini_path);

    // 检测 d3dx.ini 是否配置了 include_recursive
    // 若缺失，_MANAGED_ 下的管理文件不会被 3Dmigoto 加载，按键模拟将无效
    let has_include_recursive = detect_include_recursive(&main_ini_path, game_mods_path);
    if !has_include_recursive {
        log::warn!(
            "[mod_manager] [update_mod_data] d3dx.ini 缺少 include_recursive 配置，模组可能无法在游戏内切换。\
             请确保 d3dx.ini 包含 [Include] 段及 include_recursive = Mods 指令"
        );
        need_reload_manual = true;
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
        orfix_detection,
        ..Default::default()
    };
    log::debug!("[core::mod_manager] [update_mod_data] done | elapsed={:?}ms | processed={} errors={}", _s.elapsed().as_millis(), result.processed_mods, result.errors.len());
    Ok(result)
}

/// 准备 _MANAGED_ 目录，创建 NRMM 所需的模板 INI 文件
///
/// 复刻 NRMM 的 _prepareManagedFolder 流程：
/// 1. 创建 nrmm_keypress.txt（按键监听配置）
/// 2. 创建 nrmm_include.ini（include 入口点）
/// 3. 创建 manager_group.ini（全局管理组，定义 active_group_id 等变量）
///
/// # 返回值
/// - `true`: 需要用户手动重载（F10）
/// - `false`: 支持自动重载
fn prepare_managed_folder(managed_path: &Path, _game: TargetGame) -> Result<bool> {
    let mut need_reload_manual = false;

    // 检查是否已存在必要文件，不存在则需要手动重载
    let keypress_path = managed_path.join(constants::KEYPRESS_FILENAME);
    let include_path = managed_path.join(constants::INCLUDE_FILENAME);
    let manager_group_path = managed_path.join("manager_group.ini");

    if !keypress_path.exists() || !include_path.exists() {
        need_reload_manual = true;
    }

    // 创建 nrmm_keypress.txt - 默认配置（后台监听按键）
    let keypress_template = String::from_utf8_lossy(crate::resources::LISTEN_KEYPRESS_EVEN_ON_BACKGROUND);
    atomic_write_file(&keypress_path, keypress_template.as_bytes())?;

    // 创建 nrmm_include.ini - 将由后续步骤填充 include 列表
    // 这里仅写入 [IncludeKeypress] section include keypress 文件
    let include_content = format!(
        "[IncludeKeypress]\ninclude = {}\n",
        constants::KEYPRESS_FILENAME
    );
    atomic_write_file(&include_path, include_content.as_bytes())?;

    // 创建 manager_group.ini - 全局管理组
    let manager_template = String::from_utf8_lossy(crate::resources::TEMPLATE_MANAGER_GROUP);
    atomic_write_file(&manager_group_path, manager_template.as_bytes())?;

    Ok(need_reload_manual)
}

/// 清理 _MANAGED_ 目录下旧的 group_*.ini 文件
///
/// 复刻 NRMM 的 _deleteGroupIniFiles：删除 group_1.ini ~ group_500.ini（包括子目录）
fn delete_group_ini_files(managed_path: &Path) -> Result<()> {
    if !managed_path.exists() {
        return Ok(());
    }

    fn delete_in_dir(dir: &Path) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    // 匹配 group_X.ini（X 为数字）
                    let is_group_ini = if let Some(rest) = name.strip_prefix("group_") {
                        if let Some(num_str) = rest.strip_suffix(".ini") {
                            num_str.parse::<u32>().is_ok()
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    // 也清理 ModFolder.ini（旧版本命名）
                    if is_group_ini || name == "ModFolder.ini" {
                        if let Err(e) = fs::remove_file(&path) {
                            log::warn!("Failed to delete old group INI {:?}: {}", path, e);
                        }
                    }
                }
            } else if path.is_dir() {
                // 递归清理子目录
                let dir_name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                if dir_name.starts_with("group_") || dir_name == "_MANAGED_" {
                    let _ = delete_in_dir(&path);
                }
            }
        }
        Ok(())
    }

    delete_in_dir(managed_path)
}

/// 为单个分组创建 ModFolder.ini（或 group_X.ini）
///
/// 复刻 NRMM 的 _createGroupIni：
/// - 使用 TEMPLATE_GROUP 模板
/// - 替换 {x} 为分组索引，{group_x} 为 group_X 目录名
/// - 在文件末尾 include 该组所有启用模组的 INI 文件
///
/// # 返回值
/// - Some(path): 创建成功，返回 INI 文件路径
/// - None: 分组目录不存在或无启用模组
fn create_group_ini(
    group_dir: &Path,
    group_index: u32,
    mod_ini_paths: &[PathBuf],
    game_mods_path: &Path,
) -> Result<Option<PathBuf>> {
    // 确保分组目录存在
    if !group_dir.exists() {
        fs::create_dir_all(group_dir)?;
    }

    let group_folder_name = format!("group_{}", group_index);
    let group_ini_filename = format!("group_{}.ini", group_index);
    let group_ini_path = group_dir.join(&group_ini_filename);

    // 从模板生成基础内容
    let template_str = String::from_utf8_lossy(crate::resources::TEMPLATE_GROUP);
    let mut content = template_str
        .replace("{group_x}", &group_folder_name)
        .replace("{x}", &group_index.to_string());

    // 添加该组所有启用模组的 INI include
    content.push_str("\n; === NRMM Managed Includes ===\n");
    for ini_path in mod_ini_paths {
        if let Ok(rel_path) = ini_path.strip_prefix(game_mods_path) {
            let rel_str = rel_path.to_string_lossy().replace('\\', "/");
            content.push_str(&format!("include = {}\n", rel_str));
        }
    }

    atomic_write_file(&group_ini_path, content.as_bytes())?;
    Ok(Some(group_ini_path))
}

/// 创建 nrmm_include.ini：include manager_group.ini 和所有 group_X.ini
///
/// 这是 NRMM 的 include 入口点：
/// - d3dx.ini include nrmm_include.ini
/// - nrmm_include.ini include manager_group.ini 和各 group_X/ModFolder.ini
fn create_nrmm_include_ini(
    include_path: &Path,
    managed_path: &Path,
    group_ini_paths: &[PathBuf],
    game_mods_path: &Path,
) -> Result<()> {
    let mut content = String::new();

    // Include keypress 配置
    content.push_str("[IncludeKeypress]\n");
    content.push_str(&format!("include = {}\n\n", constants::KEYPRESS_FILENAME));

    // Include manager_group.ini
    if let Ok(rel) = managed_path.join("manager_group.ini").strip_prefix(game_mods_path) {
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        content.push_str(&format!("include = {}\n", rel_str));
    }

    // Include 所有 group INI
    for group_ini in group_ini_paths {
        if let Ok(rel) = group_ini.strip_prefix(game_mods_path) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            content.push_str(&format!("include = {}\n", rel_str));
        }
    }

    atomic_write_file(include_path, content.as_bytes())?;
    Ok(())
}

/// 原子写入文件（先写 tmp，再 rename）
fn atomic_write_file(path: &Path, content: &[u8]) -> Result<()> {
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, content)
        .with_context(|| format!("Failed to write temp file: {:?}", tmp_path))?;
    fs::rename(&tmp_path, path)
        .with_context(|| format!("Failed to rename temp file to: {:?}", path))?;
    Ok(())
}

/// 移除主 INI 中 NRMM 之前注入的内容
fn strip_nrmm_injected_content(content: &str) -> String {
    let start_marker = ";NRMM_INI_START";
    let end_marker = ";NRMM_INI_END";

    let mut result = String::new();
    let mut in_injected = false;

    for line in content.lines() {
        if line.contains(start_marker) {
            in_injected = true;
            continue;
        }
        if line.contains(end_marker) {
            in_injected = false;
            continue;
        }
        if !in_injected {
            result.push_str(line);
            result.push('\n');
        }
    }

    result.trim_end().to_string()
}

/// 生成主 INI 中 NRMM 注入的内容段
///
/// 复刻 NRMM 的注入格式：
/// - ;NRMM_INI_START / ;NRMM_INI_END 标记
/// - [Constants] 段定义管理变量
/// - include nrmm_include.ini
#[allow(dead_code)]
fn generate_nrmm_injected_content(nrmm_include_path: &Path, game_mods_path: &Path) -> Result<String> {
    let mut content = String::new();

    content.push_str(";NRMM_INI_START\n");
    content.push_str("; ==========================================\n");
    content.push_str("; No-Reload Mod Manager managed section\n");
    content.push_str("; Do not edit this section manually\n");
    content.push_str("; ==========================================\n\n");

    content.push_str("[Constants]\n");
    content.push_str("global $managed_slot_id = 0\n\n");

    // include nrmm_include.ini（而不是直接 include 所有模组 INI）
    if let Ok(rel_path) = nrmm_include_path.strip_prefix(game_mods_path) {
        let rel_str = rel_path.to_string_lossy().replace('\\', "/");
        content.push_str(&format!("include = {}\n", rel_str));
    }

    content.push_str("\n; ==========================================\n");
    content.push_str("; End of NRMM managed section\n");
    content.push_str(";NRMM_INI_END\n");

    Ok(content)
}

/// 检测 d3dx.ini 是否配置了指向 game_mods_path 的 include_recursive 指令
///
/// 3Dmigoto 通过 [Include] 段内的 include_recursive 指令递归加载目录下所有 .ini 文件。
/// NRMM 依赖此机制自动加载 _MANAGED_ 下的 nrmm_include.ini 等文件。
/// 若 d3dx.ini 缺少此配置，_MANAGED_ 下的管理文件不会被 3Dmigoto 加载，
/// 导致按键模拟钩子（[KeyMod]/[KeyGroup]）和前台窗口设置（check_foreground_window）无效。
///
/// # 参数
/// - `main_ini_path`: d3dx.ini 文件路径
/// - `game_mods_path`: Mods 目录路径（include_recursive 应指向此目录）
///
/// # 返回值
/// - `true`: 检测到 [Include] 段内包含指向 Mods 目录的 include_recursive 指令
/// - `false`: 未检测到，需警告用户手动添加
fn detect_include_recursive(main_ini_path: &Path, game_mods_path: &Path) -> bool {
    let content = match IniFile::force_read_as_utf8(main_ini_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let mods_dir_name = game_mods_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Mods");

    let mut in_include_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = &trimmed[1..trimmed.len() - 1];
            in_include_section = section.eq_ignore_ascii_case("Include");
            continue;
        }
        if in_include_section {
            if let Some(rest) = trimmed.strip_prefix("include_recursive") {
                let value = rest.trim_start_matches(['=', ' ']).trim();
                if value.contains(mods_dir_name) {
                    return true;
                }
            }
        }
    }
    false
}

fn create_default_main_ini(path: &Path, ini_name: &str) -> Result<()> {
    let content = format!(
r#"; {} - Generated by NRMM
[Constants]
global $managed_slot_id = 0
"#,
        ini_name
    );
    fs::write(path, content)
        .with_context(|| format!("Failed to create default main INI: {:?}", path))?;
    Ok(())
}

/// Save Mod Customizations：将当前模组自定义状态写入 d3dx_user.ini
///
/// 复刻 NRMM 的 Save Customizations 功能：
/// - 读取当前 d3dx.ini 中的用户自定义 section（非 NRMM 管理的 section）
/// - 将这些自定义内容写入 d3dx_user.ini
/// - 用户可以在 d3dx_user.ini 中添加自定义配置，这些配置不会被 NRMM 覆盖
///
/// # 注意
/// - d3dx_user.ini 会被 3Dmigoto 自动加载（如果存在）
/// - NRMM 不会修改或覆盖 d3dx_user.ini 中的内容
pub fn save_customizations(game_mods_path: &Path, game: TargetGame) -> Result<SaveCustomizationsResult> {
    let main_ini_name = game.d3dx_ini_name();
    let main_ini_path = game_mods_path.join(main_ini_name);
    let user_ini_path = game_mods_path.join("d3dx_user.ini");

    if !main_ini_path.exists() {
        return Ok(SaveCustomizationsResult {
            success: false,
            message: "Main INI file not found".to_string(),
        });
    }

    let content = IniFile::force_read_as_utf8(&main_ini_path)?;
    let stripped = strip_nrmm_injected_content(&content);

    // 将非 NRMM 管理的内容写入 d3dx_user.ini
    // 注意：这里只做简单复制，实际 NRMM 会更智能地提取用户自定义 section
    let header = "; Custom settings saved by NRMM\n";
    let header = format!("{}; This file will NOT be overwritten by NRMM\n; Add your custom [Constants] and other sections here\n\n", header);
    let final_content = format!("{}{}", header, stripped);

    atomic_write_file(&user_ini_path, final_content.as_bytes())?;

    Ok(SaveCustomizationsResult {
        success: true,
        message: "Customizations saved to d3dx_user.ini".to_string(),
    })
}

/// 切换普通分组选中的模组
///
/// 普通分组（group_xx）是互斥的：选中一个模组会自动禁用同组其他模组。
/// 实现方式：重命名目录（移除目标的 DISABLED 前缀，给其他模组添加前缀），然后调用 update_mod_data。
///
/// # 参数
/// - `game`: 目标游戏
/// - `game_mods_path`: 游戏 Mods 目录
/// - `settings`: 应用设置
/// - `group_index`: 分组索引
/// - `mod_index`: 模组索引（在分组内的索引）
pub fn switch_mod(
    _game: TargetGame,
    game_mods_path: &Path,
    _settings: &AppSettings,
    group_index: u32,
    mod_index: u32,
) -> Result<UpdateResult> {
    log::debug!("[core::mod_manager] [switch_mod] Starting | group={} mod={} path={:?}", group_index, mod_index, game_mods_path);
    let _s = std::time::Instant::now();
    // 使用轻量扫描以匹配前端索引（None 在 mod_index=0，真实模组从 1 开始）
    let scan_result = mod_scanner::scan_mods_light(game_mods_path)?;
    log::debug!(
        "[core::mod_manager] [switch_mod] scan completed | total_groups={} total_mods={}",
        scan_result.groups.len(),
        scan_result.mods.len()
    );

    // 先找出分组目录路径（用于写入 selectedindex 文件）
    // 严格限制：仅 NormalGroup（group_xx 目录）允许写入 selectedindex 标记文件。
    // MutexGroup（非group目录）不参与标记文件生成，仅参与模组读取和 hash 冲突检测。
    // 通过 group_type == NormalGroup 校验避免误在非group目录下写入标记文件。
    let group_dir = scan_result.groups.iter()
        .find(|g| g.group_index == group_index && g.group_type == GroupType::NormalGroup)
        .map(|g| g.full_path.clone());
    log::debug!(
        "[core::mod_manager] [switch_mod] looking for NormalGroup by group_index={} found={}",
        group_index,
        group_dir.is_some()
    );

    // 注意：group 分组下选择模组不使用互斥逻辑，不得自动禁用/启用同组其他模组。
    // 各模组的启用/禁用状态仅由用户通过开关显式抉择，选择模组只更新选中状态。

    // 将选中的 mod_index 写入该分组的 selectedindex 文件，使 is_active 状态持久化
    let sel_idx_i32 = mod_index as i32;
    if let Some(g_dir) = group_dir {
        let selectedindex_path = g_dir.join(constants::SELECTED_INDEX_FILE);
        if let Err(e) = fs::write(&selectedindex_path, mod_index.to_string()) {
            log::warn!("Failed to write selectedindex file {:?}: {}", selectedindex_path, e);
        } else {
            log::debug!(
                "[core::mod_manager] [switch_mod] wrote selectedindex file | path={} content={}",
                selectedindex_path.display(),
                mod_index
            );
        }
    } else {
        log::warn!(
            "[core::mod_manager] [switch_mod] group not found by group_index={} (no selectedindex written)",
            group_index
        );
    }

    let result = UpdateResult {
        selected_mod_index: Some(sel_idx_i32),
        ..Default::default()
    };
    log::debug!("[core::mod_manager] [switch_mod] done | elapsed={:?}ms | selected={:?}", _s.elapsed().as_millis(), result.selected_mod_index);
    Ok(result)
}

/// 切换单个模组的启用/禁用状态（独立开关，不影响同组其他模组）
///
/// 检查磁盘实际状态，支持幂等操作：
/// - 传入路径不存在时，检查父目录下是否有对应的启用/禁用版本
/// - 传入路径存在时，检查是否需要重命名
/// - 已经是目标状态时，直接返回（幂等）
///
/// # 参数
/// - `mod_path`: 模组目录路径
/// - `enable`: true = 启用（移除 DISABLED 前缀），false = 禁用（添加 DISABLED 前缀）
pub fn toggle_mod(mod_path: &Path, enable: bool) -> Result<()> {
    let dir_name = mod_path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let parent = mod_path.parent().unwrap_or(mod_path);
    let is_disabled = dir_name.to_uppercase().starts_with("DISABLED");

    if enable {
        if mod_path.exists() {
            // 传入路径存在 → 检查是否需要启用
            if is_disabled {
                let new_name = dir_name
                    .trim_start_matches("DISABLED")
                    .trim_start_matches("disabled")
                    .trim_start_matches(|c: char| ['_', ' ', '-'].contains(&c));
                let new_path = parent.join(new_name);
                if mod_path != new_path {
                    fs::rename(mod_path, &new_path)
                        .with_context(|| format!("Failed to enable mod: {:?}", mod_path))?;
                }
            }
            // 已启用 → 幂等，直接返回
        } else {
            // 传入路径不存在 → 检查父目录下是否有启用版本
            if is_disabled {
                let enabled_name = dir_name
                    .trim_start_matches("DISABLED")
                    .trim_start_matches("disabled")
                    .trim_start_matches(|c: char| ['_', ' ', '-'].contains(&c));
                let enabled_path = parent.join(enabled_name);
                if enabled_path.exists() {
                    // 已启用 → 幂等，直接返回
                    return Ok(());
                }
            }
            // 路径不存在且无启用版本 → 返回错误
            return Err(anyhow::anyhow!("Mod path does not exist: {:?}", mod_path));
        }
    } else {
        // disable
        if mod_path.exists() {
            // 传入路径存在 → 检查是否需要禁用
            if !is_disabled {
                let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                let new_path = parent.join(new_name);
                if mod_path != new_path {
                    if new_path.exists() {
                        log::warn!("Target path already exists, skipping disable: {:?}", new_path);
                    } else {
                        fs::rename(mod_path, &new_path)
                            .with_context(|| format!("Failed to disable mod: {:?}", mod_path))?;
                    }
                }
            }
            // 已禁用 → 幂等，直接返回
        } else {
            // 传入路径不存在 → 检查父目录下是否有禁用版本
            if !is_disabled {
                let disabled_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                let disabled_path = parent.join(disabled_name);
                if disabled_path.exists() {
                    // 已禁用 → 幂等，直接返回
                    return Ok(());
                }
            }
            // 路径不存在且无禁用版本 → 返回错误
            return Err(anyhow::anyhow!("Mod path does not exist: {:?}", mod_path));
        }
    }

    Ok(())
}

/// 互斥启用模组：启用指定模组，禁用其父目录下同级的其他模组叶子节点（含.ini的目录）
///
/// - 只处理父目录下的一级子目录，不递归子分组
/// - 没有.ini的目录（分组节点/子分组）不处理
/// - 重命名保持幂等，检查目标路径是否存在避免覆盖
pub fn enable_mutex_mod(mod_path: &Path) -> Result<()> {
    let parent_dir = mod_path.parent()
        .with_context(|| format!("Failed to get parent directory of: {:?}", mod_path))?;

    let entries = fs::read_dir(parent_dir)
        .with_context(|| format!("Failed to read parent directory: {:?}", parent_dir))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }

        let has_ini = mod_scanner::dir_has_ini_file(&path)?;
        if !has_ini {
            continue;
        }

        if path == mod_path {
            let current_name = path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_disabled = current_name.to_uppercase().starts_with("DISABLED");
            if is_disabled {
                let new_name = current_name
                    .trim_start_matches("DISABLED")
                    .trim_start_matches("disabled")
                    .trim_start_matches(|c: char| ['_', ' ', '-'].contains(&c));
                let new_path = path.parent().unwrap_or(&path).join(new_name);
                if path != new_path {
                    if new_path.exists() {
                        log::warn!("Target path already exists, skipping enable: {:?}", new_path);
                    } else {
                        fs::rename(&path, &new_path)
                            .with_context(|| format!("Failed to enable mod: {:?}", path))?;
                    }
                }
            }
        } else {
            let current_name = path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_disabled = current_name.to_uppercase().starts_with("DISABLED");
            if !is_disabled {
                let new_name = format!("{}{}", constants::DISABLED_PREFIX, current_name);
                let new_path = path.parent().unwrap_or(&path).join(new_name);
                if path != new_path {
                    if new_path.exists() {
                        log::warn!("Target path already exists, skipping disable: {:?}", new_path);
                    } else {
                        fs::rename(&path, &new_path)
                            .with_context(|| format!("Failed to disable sibling mod: {:?}", path))?;
                    }
                }
            }
        }
    }

    Ok(())
}

/// 禁用互斥模组：给模组目录添加DISABLED前缀
///
/// - 检查磁盘实际状态，支持幂等操作
/// - 传入路径不存在时，检查父目录下是否已有禁用版本
/// - 如果已经以DISABLED开头（大小写不敏感）则不做任何操作（幂等）
/// - 检查目标路径不存在才重命名
pub fn disable_mutex_mod(mod_path: &Path) -> Result<()> {
    let dir_name = mod_path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let parent = mod_path.parent().unwrap_or(mod_path);
    let is_disabled = dir_name.to_uppercase().starts_with("DISABLED");

    // 传入路径不存在 → 检查磁盘上是否已禁用
    if !mod_path.exists() {
        if !is_disabled {
            let disabled_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
            let disabled_path = parent.join(disabled_name);
            if disabled_path.exists() {
                // 已禁用 → 幂等，直接返回
                return Ok(());
            }
        }
        return Err(anyhow::anyhow!("Mod path does not exist: {:?}", mod_path));
    }

    // 传入路径存在 → 检查是否需要禁用
    if !is_disabled {
        let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
        let new_path = parent.join(new_name);
        if mod_path != new_path {
            if new_path.exists() {
                log::warn!("Target path already exists, skipping disable: {:?}", new_path);
            } else {
                fs::rename(mod_path, &new_path)
                    .with_context(|| format!("Failed to disable mod: {:?}", mod_path))?;
            }
        }
    }

    Ok(())
}

/// 禁用指定分组下的所有一级模组（叶子节点，含 .ini 文件的目录）
///
/// 遍历分组目录下的所有直接子目录，对每个未禁用的模组添加 DISABLED 前缀。
/// 跳过：
/// - 非目录
/// - 已禁用的模组目录（名称以 DISABLED 开头）
/// - 不含 .ini 文件的目录（分组节点/子分组目录）
///
/// 不递归处理子分组目录，仅处理一级子目录。
///
/// # 参数
/// - `group_path`: 分组目录路径
///
/// # 返回
/// 成功禁用的模组数量（已禁用的跳过不计入）
///
/// # 错误
/// - 读取目录失败时返回错误
/// - 重命名失败时返回错误
pub fn disable_all_mods_in_group(group_path: &Path) -> Result<u32> {
    if !group_path.exists() {
        return Ok(0);
    }
    if !group_path.is_dir() {
        return Ok(0);
    }

    let entries = fs::read_dir(group_path)
        .with_context(|| format!("Failed to read group directory: {:?}", group_path))?;

    let mut disabled_count: u32 = 0;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }

        // 仅处理含 .ini 的叶子模组节点（不含 .ini 的是分组节点/子分组，跳过）
        let has_ini = match mod_scanner::dir_has_ini_file(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !has_ini {
            continue;
        }

        // 检查是否已禁用（名称以 DISABLED 开头，大小写不敏感）
        let is_disabled = dir_name.to_uppercase().starts_with(constants::DISABLED_PREFIX);
        if is_disabled {
            continue;
        }

        // 构建新目录名：DISABLED + 原名称
        let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
        let new_path = group_path.join(&new_name);

        // 检查目标路径是否已存在（避免覆盖）
        if new_path.exists() {
            log::warn!("Target path already exists, skipping disable: {:?}", new_path);
            continue;
        }

        // 重命名添加前缀
        fs::rename(&path, &new_path)
            .with_context(|| format!("Failed to disable mod: {:?}", path))?;

        disabled_count += 1;
    }

    Ok(disabled_count)
}

/// 判断模组是否属于MutexGroup（非group_xx目录下的模组）
///
/// 逻辑：获取mod_path相对于managed_path的路径，检查第一个路径段是否匹配group_xx正则；
/// 如果第一个路径段不匹配group_xx（即不是NormalGroup），则是mutex mod。
pub fn is_mutex_mod(mod_path: &Path, managed_path: &Path) -> bool {
    let rel_path = match mod_path.strip_prefix(managed_path) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let first_component = rel_path.components().next();
    match first_component {
        Some(std::path::Component::Normal(name)) => {
            let name_str = name.to_string_lossy();
            mod_scanner::is_normal_group_dir(&name_str).is_none()
        }
        _ => false,
    }
}

/// 取消选中分组内模组（NormalGroup 专用）
///
/// 对齐 NRMM `setSelectedModIndex(ref, 0, groupPath)`：向分组目录写入 `selectedindex=0`，
/// 选择 None 槽位以取消选中，而非重命名禁用模组。
///
/// # 参数
/// - `game`: 目标游戏
/// - `game_mods_path`: 游戏 Mods 目录
/// - `settings`: 应用设置
/// - `group_index`: 要取消选中的分组索引
pub fn deselect_group_mods(
    _game: TargetGame,
    game_mods_path: &Path,
    _settings: &AppSettings,
    group_index: u32,
) -> Result<UpdateResult> {
    let scan_result = mod_scanner::scan_mods_light(game_mods_path)?;

    // 仅 NormalGroup 写入 selectedindex，MutexGroup 不参与标记文件
    let group_dir = scan_result.groups.iter()
        .find(|g| g.group_index == group_index && g.group_type == GroupType::NormalGroup)
        .map(|g| g.full_path.clone());

    if let Some(g_dir) = group_dir {
        let selectedindex_path = g_dir.join(constants::SELECTED_INDEX_FILE);
        // 写入 0 表示选择 None 槽位（取消选择）
        if let Err(e) = fs::write(&selectedindex_path, "0") {
            log::warn!("Failed to write selectedindex file {:?}: {}", selectedindex_path, e);
        } else {
            log::debug!(
                "[core::mod_manager] [deselect_group_mods] wrote selectedindex=0 | path={}",
                selectedindex_path.display()
            );
        }
    } else {
        log::warn!(
            "[core::mod_manager] [deselect_group_mods] NormalGroup not found by group_index={}",
            group_index
        );
    }

    Ok(UpdateResult {
        selected_mod_index: Some(0),
        ..Default::default()
    })
}

/// 批量切换模组启用/禁用状态
///
/// 只计数实际发生变更的模组数量。
///
/// # 参数
/// - `mod_paths`: 模组路径列表
/// - `enable`: true=启用, false=禁用
/// - `is_mutex`: 是否为互斥组模组
pub fn batch_toggle_mods(mod_paths: &[String], enable: bool, is_mutex: bool) -> Result<u32> {
    let mut count = 0u32;
    for path_str in mod_paths {
        let path = PathBuf::from(path_str);
        
        // 预检查是否需要操作
        let dir_name = path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let parent = path.parent().unwrap_or(&path);
        let is_disabled = dir_name.to_uppercase().starts_with("DISABLED");
        
        // 判断是否需要操作
        let needs_change = if enable {
            // 启用：路径存在且已禁用，或路径不存在但父目录有禁用版本
            if path.exists() {
                is_disabled
            } else {
                !is_disabled || {
                    let enabled_name = dir_name.trim_start_matches("DISABLED")
                        .trim_start_matches("disabled")
                        .trim_start_matches(|c: char| ['_', ' ', '-'].contains(&c));
                    !parent.join(enabled_name).exists()
                }
            }
        } else {
            // 禁用：路径存在且未禁用，或路径不存在但父目录有启用版本
            if path.exists() {
                !is_disabled
            } else {
                is_disabled || {
                    let disabled_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                    !parent.join(disabled_name).exists()
                }
            }
        };
        
        if !needs_change {
            continue;
        }
        
        if is_mutex {
            if enable {
                enable_mutex_mod(&path)?;
            } else {
                disable_mutex_mod(&path)?;
            }
        } else {
            toggle_mod(&path, enable)?;
        }
        count += 1;
    }
    Ok(count)
}

/// 启用指定分组下的所有一级模组（叶子节点，含 .ini 文件的目录）
///
/// 遍历分组目录下的所有直接子目录，对每个已禁用的模组移除 DISABLED 前缀。
/// 跳过：
/// - 非目录
/// - 已启用的模组目录（名称不以 DISABLED 开头）
/// - 不含 .ini 文件的目录（分组节点/子分组目录）
///
/// 不递归处理子分组目录，仅处理一级子目录。
///
/// # 参数
/// - `group_path`: 分组目录路径
///
/// # 返回
/// 成功启用的模组数量（已启用的跳过不计入）
///
/// # 错误
/// - 读取目录失败时返回错误
/// - 重命名失败时返回错误
pub fn enable_all_mods_in_group(group_path: &Path) -> Result<u32> {
    if !group_path.exists() {
        return Ok(0);
    }
    if !group_path.is_dir() {
        return Ok(0);
    }

    let entries = fs::read_dir(group_path)
        .with_context(|| format!("Failed to read group directory: {:?}", group_path))?;

    let mut enabled_count: u32 = 0;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }

        // 仅处理含 .ini 的叶子模组节点
        let has_ini = match mod_scanner::dir_has_ini_file(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !has_ini {
            continue;
        }

        // 检查是否已禁用（名称以 DISABLED 开头，大小写不敏感）
        let is_disabled = dir_name.to_uppercase().starts_with(constants::DISABLED_PREFIX);
        if !is_disabled {
            continue;
        }

        // 去除 DISABLED 前缀，再去除前导分隔符 _ 空格 - 等
        let prefix = constants::DISABLED_PREFIX;
        let stripped_upper = dir_name.to_uppercase();
        let after_prefix = if stripped_upper.starts_with(prefix) {
            &dir_name[prefix.len()..]
        } else {
            // 大小写混合情况，按字节长度截取
            let len = std::cmp::min(prefix.len(), dir_name.len());
            &dir_name[len..]
        };
        let new_name = after_prefix.trim_start_matches(|c: char| ['_', ' ', '-'].contains(&c));
        let new_name = if new_name.is_empty() {
            // 极端情况：整个目录名就是前缀，跳过
            continue;
        } else {
            new_name.to_string()
        };

        let new_path = group_path.join(&new_name);

        if new_path.exists() {
            log::warn!("Target path already exists, skipping enable: {:?}", new_path);
            continue;
        }

        // 源路径和目标路径相同则跳过
        if path == new_path {
            continue;
        }

        fs::rename(&path, &new_path)
            .with_context(|| format!("Failed to enable mod: {:?}", path))?;

        enabled_count += 1;
    }

    Ok(enabled_count)
}

/// 从备份恢复所有 INI 文件
///
/// 恢复顺序：
/// 1. 主 INI 文件（d3dx.ini, RatioShot.ini）
/// 2. 递归恢复 _MANAGED_ 目录下所有模组的 INI
/// 3. 清理 NRMM 生成的管理文件（nrmm_include.ini, manager_group.ini, nrmm_keypress.txt, group_*.ini）
///
/// 恢复成功后删除备份文件。
///
/// # 参数
/// - `game_mods_path`: 游戏 Mods 目录
///
/// # 返回
/// 恢复统计（成功数/失败数）
pub fn restore_all_inis(game_mods_path: &Path) -> Result<RestoredCount> {
    let mut restored = 0u32;
    let mut failed = 0u32;

    for &ini_name in &["d3dx.ini", "RatioShot.ini"] {
        let ini_path = game_mods_path.join(ini_name);
        let backup_path = ini_path.with_extension(constants::BACKUP_EXTENSION);
        if backup_path.exists() {
            match fs::copy(&backup_path, &ini_path) {
                Ok(_) => {
                    restored += 1;
                    fs::remove_file(&backup_path).ok();
                }
                Err(e) => {
                    log::error!("Failed to restore main INI {}: {}", ini_path.display(), e);
                    failed += 1;
                }
            }
        }
    }

    let managed_folder = game_mods_path.join(constants::MANAGED_FOLDER);
    if managed_folder.exists() {
        restore_inis_recursive(&managed_folder, &mut restored, &mut failed)?;

        // 清理 NRMM 生成的管理文件
        let _ = fs::remove_file(managed_folder.join(constants::INCLUDE_FILENAME));
        let _ = fs::remove_file(managed_folder.join(constants::KEYPRESS_FILENAME));
        let _ = fs::remove_file(managed_folder.join("manager_group.ini"));

        // 清理 group_*.ini 文件
        if let Ok(entries) = fs::read_dir(&managed_folder) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if let Some(rest) = name.strip_prefix("group_") {
                            if rest.ends_with(".ini") {
                                let _ = fs::remove_file(&path);
                            }
                        }
                    }
                }
                // 清理子目录下的 group_X.ini 和 ModFolder.ini
                if path.is_dir() {
                    if let Ok(sub_entries) = fs::read_dir(&path) {
                        for sub_entry in sub_entries.flatten() {
                            let sub_path = sub_entry.path();
                            if sub_path.is_file() {
                                if let Some(name) = sub_path.file_name().and_then(|n| n.to_str()) {
                                    if name.starts_with("group_") && name.ends_with(".ini") {
                                        let _ = fs::remove_file(&sub_path);
                                    }
                                    if name == "ModFolder.ini" {
                                        let _ = fs::remove_file(&sub_path);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(RestoredCount { restored, failed })
}

fn restore_inis_recursive(dir: &Path, restored: &mut u32, failed: &mut u32) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            restore_inis_recursive(&path, restored, failed)?;
        } else if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("ini") {
                let backup_path = path.with_extension(constants::BACKUP_EXTENSION);
                if backup_path.exists() {
                    match fs::copy(&backup_path, &path) {
                        Ok(_) => {
                            *restored += 1;
                            fs::remove_file(&backup_path).ok();
                        }
                        Err(e) => {
                            log::error!("Failed to restore mod INI {}: {}", path.display(), e);
                            *failed += 1;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

// ============================================================================
// 模组移除与 INI 还原（NRMM 对齐）
// 严格复刻 NRMM 的 restoreManagedMod + renameOrMoveFolder 流程
// ============================================================================

/// 模组移除结果
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveModResult {
    /// 模组名称
    pub mod_name: String,
    /// 移动后的目标路径
    pub moved_to: PathBuf,
    /// INI 还原是否成功（true=全部成功，false=部分或全部失败）
    pub restored: bool,
    /// 还原过程中处理的 INI 文件数
    pub ini_count: u32,
    /// 还原过程中失败的文件数
    pub failed_count: u32,
}

/// 还原模组 INI 结果（还原区功能）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreManagedResult {
    /// 被还原的目录路径
    pub path: PathBuf,
    /// 处理的 INI 文件数
    pub ini_count: u32,
    /// 还原失败的文件数
    pub failed_count: u32,
    /// 是否全部还原成功
    pub success: bool,
}

/// 还原模组 INI 到管理前状态（NRMM 对齐：`restoreManagedMod`）
///
/// 严格复刻 NRMM 的 `restoreManagedMod` 逻辑，对指定目录下递归找到的每个 INI 文件执行：
/// 1. 移除 NRMM 管理注释（含 `no reload mod manager`、`";-;" are errored`、
///    `";+;" are disabled keys`、`errored conditional blocks`、
///    `if certain syntax is only available` 关键字的注释行）
/// 2. 移除 `global $managed_slot_id =` 变量声明
/// 3. 净化 `condition=` 表达式（移除 `$managed_slot_id == $\modmanageragl\group_X\...` 段，
///    清理孤立的 `&&`/`||`/空括号等）
/// 4. 移除管理器 `if $managed_slot_id == ... endif` 块（栈匹配 if/endif）
/// 5. 移除 NRMM 管理时添加的前 4 个空格缩进
///
/// # 参数
/// - `mod_dir`: 已移动到回收目录的模组文件夹路径
///
/// # 返回值
/// 返回 `(处理的 INI 文件数, 失败数)`。单个文件失败不会中断整体流程。
pub fn restore_managed_mod(mod_dir: &Path) -> (u32, u32) {
    let mut total = 0u32;
    let mut failed = 0u32;

    // 递归收集所有 .ini 文件
    let ini_files = match collect_ini_files_recursive(mod_dir) {
        Ok(files) => files,
        Err(e) => {
            log::error!("[restore_managed_mod] Failed to collect INI files in {:?}: {}", mod_dir, e);
            return (0, 1);
        }
    };

    for ini_path in &ini_files {
        total += 1;
        if let Err(e) = restore_single_ini(ini_path) {
            log::warn!("[restore_managed_mod] Failed to restore {:?}: {}", ini_path, e);
            failed += 1;
        }
    }

    (total, failed)
}

/// 递归收集目录下所有 .ini 文件（对齐 NRMM `_findIniFilesRecursive`）
fn collect_ini_files_recursive(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    collect_ini_files_recursive_inner(dir, &mut result)?;
    Ok(result)
}

fn collect_ini_files_recursive_inner(dir: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_ini_files_recursive_inner(&path, result)?;
        } else if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("ini") {
                // 跳过 .ini_managed_backup 备份文件
                if !path.to_string_lossy().ends_with(&format!(".{}", constants::BACKUP_EXTENSION)) {
                    result.push(path);
                }
            }
        }
    }
    Ok(())
}

/// 收集模组目录下的直接 .ini 文件（不递归子目录）
///
/// 用于 `detect_hash_conflicts` 中从轻量扫描结果（`ModData.full_path`）查找
/// 模组的 INI 文件。轻量扫描不解析 INI 内容，故需按需查找并解析。
///
/// 过滤规则：
/// - 仅扫描目录下的直接文件，不递归子目录（与深度扫描 `check_directory_for_mod_deep` 一致）
/// - 排除 `desktop.ini` 系统配置文件（NRMM 对齐）
/// - 排除 `.ini_managed_backup` 备份文件
///
/// # 参数
/// - `mod_dir`: 模组目录路径
///
/// # 返回
/// 目录下所有符合条件的 .ini 文件路径列表（无序）
fn collect_mod_ini_files(mod_dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let entries = match fs::read_dir(mod_dir) {
        Ok(e) => e,
        Err(_) => return result,
    };
    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(f) => f,
            Err(_) => continue,
        };
        if !ft.is_file() && !ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        // 排除 desktop.ini 系统配置文件
        if constants::is_desktop_ini(&path) {
            continue;
        }
        if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("ini") {
                // 排除 .ini_managed_backup 备份文件
                if !path.to_string_lossy().ends_with(&format!(".{}", constants::BACKUP_EXTENSION)) {
                    result.push(path);
                }
            }
        }
    }
    result
}

/// 还原单个 INI 文件（对齐 NRMM `restoreManagedMod` 内层循环）
///
/// 处理流程严格对齐 NRMM：
/// 1. 读取文件（UTF-8 强制转换）
/// 2. 逐行扫描，标记需删除的行（管理注释、变量声明、管理器 if/endif）
/// 3. 净化 condition= 表达式
/// 4. 移除前 4 个空格缩进
/// 5. 若有修改则写回文件
fn restore_single_ini(ini_path: &Path) -> Result<()> {
    let content = IniFile::force_read_as_utf8(ini_path)?;
    let raw_lines: Vec<&str> = content.lines().collect();
    let mut lines: Vec<String> = raw_lines.iter().map(|s| s.to_string()).collect();
    let mut modified = false;

    // if 栈：跟踪 if/endif 配对，用于识别管理器 if 块的 endif
    let mut if_stack: Vec<bool> = Vec::new(); // true = 管理器 if

    for line in lines.iter_mut() {
        let trimmed_lower = line.trim().to_lowercase();

        // 新 section 重置 if 栈
        if trimmed_lower.starts_with('[') {
            if_stack.clear();
            // section 头不移除缩进，继续
            *line = remove_first_four_spaces(line);
            continue;
        }

        // 1. 移除 NRMM 管理注释
        if trimmed_lower.starts_with(';')
            && (trimmed_lower.contains("no reload mod manager")
                || trimmed_lower.contains(r#"";-;" are errored"#)
                || trimmed_lower.contains(r#"";+;" are disabled keys"#)
                || trimmed_lower.contains("errored conditional blocks")
                || trimmed_lower.contains("if certain syntax is only available"))
        {
            *line = "-----".to_string(); // 标记删除
            modified = true;
            continue;
        }

        // 2. 移除 `global $managed_slot_id =` 变量声明
        let no_space = trimmed_lower.replace(' ', "");
        if no_space.starts_with("global$managed_slot_id=") {
            *line = "-----".to_string();
            modified = true;
            continue;
        }

        // 3. 净化 condition= 表达式（含 ;-;condition= 和 ;+;condition= 变体）
        let starts_with_condition = no_space.starts_with("condition=");
        let starts_with_special_comment1 = no_space.starts_with(";-;condition=");
        let starts_with_special_comment2 = no_space.starts_with(";+;condition=");

        if starts_with_condition || starts_with_special_comment1 || starts_with_special_comment2 {
            if let Some(equal_idx) = line.find('=') {
                let expression = line[equal_idx + 1..].trim();
                let modified_expr = sanitize_condition_expression_inline(expression);

                if expression != modified_expr {
                    if modified_expr.is_empty() {
                        *line = "-----".to_string();
                    } else if starts_with_special_comment1 {
                        *line = format!(";-;condition = {}", modified_expr);
                    } else if starts_with_special_comment2 {
                        *line = format!(";+;condition = {}", modified_expr);
                    } else {
                        *line = format!("condition = {}", modified_expr);
                    }
                    modified = true;
                    continue;
                }
            }
        }

        // 4. 移除管理器 if 行 + 跟踪 if 栈
        if trimmed_lower.starts_with("if ") {
            let is_manager_if = no_space
                .contains("if$managed_slot_id==$\\modmanageragl\\group_");
            if_stack.push(is_manager_if);
            if is_manager_if {
                *line = "-----".to_string();
                modified = true;
                continue;
            }
        }

        // 5. 移除与管理器 if 配对的 endif
        if trimmed_lower == "endif" {
            if let Some(is_manager) = if_stack.pop() {
                if is_manager {
                    *line = "-----".to_string();
                    modified = true;
                    continue;
                }
            }
        }

        // 移除前 4 个空格缩进（NRMM 管理时添加的缩进）
        *line = remove_first_four_spaces(line);
    }

    if modified {
        let filtered: Vec<&String> = lines.iter().filter(|s| s.as_str() != "-----").collect();
        let new_content = filtered.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join("\n");
        atomic_write_file(ini_path, new_content.as_bytes())?;
    }

    Ok(())
}

/// 从行首移除最多 4 个空格（对齐 NRMM `_removeFirstFourSpaces`）
fn remove_first_four_spaces(line: &str) -> String {
    let mut count = 0;
    let mut idx = 0;
    for (i, ch) in line.chars().enumerate() {
        if ch == ' ' && count < 4 {
            count += 1;
            idx = i + 1;
        } else {
            break;
        }
    }
    line[idx..].to_string()
}

/// 净化 condition 表达式中的管理器注入部分（对齐 NRMM `_sanitizeKeyConditionExpressionFromModManager`）
///
/// 复用 `ini_handler::sanitize_condition_expression` 的核心逻辑，通过公开包装调用。
fn sanitize_condition_expression_inline(expression: &str) -> String {
    crate::core::ini_handler::sanitize_condition_expression_public(expression)
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
        return Err(anyhow::anyhow!("Mod path is not a directory: {:?}", mod_path));
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
        fs::create_dir_all(&removed_folder)
            .with_context(|| format!("Failed to create {}: {:?}", constants::MANAGED_REMOVED_FOLDER, removed_folder))?;
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
    })
}

/// 向上查找 Mods 根目录（_MANAGED_ 的父目录）
///
/// 从 `start_path` 开始逐级向上，找到包含 `_MANAGED_` 子目录的路径即为 Mods 根目录。
/// 若向上 5 级仍未找到，返回 `start_path` 的父目录作为兜底。
fn locate_mods_root(start_path: &Path) -> Result<PathBuf> {
    let mut current = start_path.parent();
    for _ in 0..5 {
        if let Some(dir) = current {
            if dir.join(constants::MANAGED_FOLDER).exists() {
                return Ok(dir.to_path_buf());
            }
            current = dir.parent();
        } else {
            break;
        }
    }
    // 兜底：使用 start_path 的父目录
    start_path
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Cannot locate Mods root from {:?}", start_path))
}

/// 构造可用文件夹名（冲突时追加 _1、_2…）
///
/// 对齐 NRMM `_getAvailableFolderName`：在 `base_dir` 下查找不冲突的文件夹名。
fn get_available_folder_name(base_name: &str, base_dir: &Path) -> PathBuf {
    let target = base_dir.join(base_name);
    if !target.exists() {
        return target;
    }

    let mut i = 1u32;
    loop {
        let resolved = base_dir.join(format!("{}_{}", base_name, i));
        if !resolved.exists() {
            return resolved;
        }
        i += 1;
    }
}


/// 移除分组（NRMM 对齐：移至 DISABLED_MANAGED_REMOVED 目录，非group先移子分组再移除）
///
/// # 参数
/// - `group_path`: 要移除的分组目录路径
/// - `is_group_xx`: true = group_1/group_2 等普通分组，false = mutexGroup 非group目录
///
/// # 对于 group_xx：
/// 1. 定位 mods_path（group_path 的父目录，即 _MANAGED_ 的父目录）
/// 2. 确保 `Mods/DISABLED_MANAGED_REMOVED` 目录存在
/// 3. 将整个分组目录移至 `DISABLED_MANAGED_REMOVED/原名`，名称冲突追加 `_1`、`_2`…
///
/// # 对于非group：
/// 1. 先将分组下的**一级子目录且不含 .ini 的目录**（即子分组目录）移至父级目录
///    - 子分组目录重名时追加 `_` 后缀
/// 2. 然后将被清空的分组目录按 group_xx 规则移至 `DISABLED_MANAGED_REMOVED/`
///
/// # 错误
/// 路径不存在或目录移动失败时返回错误
pub fn remove_group_ex(group_path: &Path, is_group_xx: bool) -> Result<()> {
    if !group_path.exists() {
        return Err(anyhow::anyhow!("Group path does not exist: {:?}", group_path));
    }
    if !group_path.is_dir() {
        return Err(anyhow::anyhow!("Group path is not a directory: {:?}", group_path));
    }

    let group_name = group_path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let parent_dir = group_path.parent()
        .with_context(|| format!("Invalid group path (no parent): {:?}", group_path))?;

    // ========== 非group：先将一级子分组（无 .ini 的子目录）移至父级 ==========
    if !is_group_xx {
        // 收集所有一级子分组（子目录且含子目录结构/不含 .ini 的目录）
        let entries = match fs::read_dir(group_path) {
            Ok(e) => e,
            Err(_) => return Err(anyhow::anyhow!("Failed to read group directory: {:?}", group_path)),
        };

        // 先收集再处理（避免在迭代期间修改目录）
        let mut to_move: Vec<(PathBuf, String)> = Vec::new();
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
            // 不含 .ini 的目录 → 子分组目录，需移至父级
            to_move.push((child_path, child_name));
        }

        for (src, child_name) in to_move {
            let target = parent_dir.join(&child_name);
            if target.exists() {
                // 重名处理：追加 _ 直到不存在
                let mut i = 1u32;
                let base = child_name.clone();
                let mut resolved = parent_dir.join(format!("{}_{}", base, i));
                while resolved.exists() {
                    i += 1;
                    resolved = parent_dir.join(format!("{}_{}", base, i));
                }
                fs::rename(&src, &resolved)
                    .with_context(|| format!("Failed to move subgroup {:?} to parent", src))?;
            } else {
                fs::rename(&src, &target)
                    .with_context(|| format!("Failed to move subgroup {:?} to parent", src))?;
            }
        }
    }

    // ========== 定位 DISABLED_MANAGED_REMOVED 目录 ==========
    // 规则：DISABLED_MANAGED_REMOVED 位于 Mods 根目录（与 _MANAGED_ 同级）
    // 如果 group_path 是 _MANAGED_/group_1，则 mods_root = parent(_MANAGED_)
    // 如果 group_path 是 Mods/#NonGroup，则 mods_root = parent(#NonGroup)（即 Mods）
    let mut mods_root = parent_dir;
    let parent_name = mods_root.file_name().map(|n| n.to_string_lossy().to_string());
    if parent_name.as_deref() == Some(constants::MANAGED_FOLDER) {
        // group 目录在 _MANAGED_ 下，DISABLED_MANAGED_REMOVED 应在其上级目录（Mods）
        if let Some(grand_parent) = mods_root.parent() {
            mods_root = grand_parent;
        }
    }

    let removed_folder = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
    if !removed_folder.exists() {
        fs::create_dir_all(&removed_folder)
            .with_context(|| format!("Failed to create {}: {:?}", constants::MANAGED_REMOVED_FOLDER, removed_folder))?;
    }

    // 构造目标路径，冲突追加 _
    let mut target = removed_folder.join(&group_name);
    if target.exists() {
        let mut i = 1u32;
        let base = group_name.clone();
        loop {
            let resolved = removed_folder.join(format!("{}_{}", base, i));
            if !resolved.exists() {
                target = resolved;
                break;
            }
            i += 1;
        }
    }

    // 移动分组至 DISABLED_MANAGED_REMOVED
    fs::rename(group_path, &target)
        .with_context(|| format!("Failed to move group {:?} to {:?}", group_path, target))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

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
        fs::write(base.join(ini_name), "; original main ini\n").unwrap();
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
        fs::write(&ini_path, "[Include]\ninclude_recursive = Mods\n\n[Constants]\nglobal $test = 0\n").unwrap();
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
    fn test_create_default_main_ini() {
        let dir = TempDir::new().unwrap();
        let ini_path = dir.path().join("d3dx.ini");
        create_default_main_ini(&ini_path, "d3dx.ini").unwrap();

        assert!(ini_path.exists());
        let content = fs::read_to_string(&ini_path).unwrap();
        assert!(content.contains("d3dx.ini - Generated by NRMM"));
        assert!(content.contains("[Constants]"));
        assert!(content.contains("$managed_slot_id"));
    }

    #[test]
    fn test_toggle_mod_enable_disable() {
        let dir = TempDir::new().unwrap();
        let enabled_path = dir.path().join("MyMod");
        fs::create_dir_all(&enabled_path).unwrap();

        toggle_mod(&enabled_path, false).unwrap();
        let disabled_name = enabled_path.file_name().unwrap().to_str().unwrap().to_string();
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

        let ini_paths = vec![mod1.clone(), mod2.clone()];
        let result = create_group_ini(&group_dir, 1, &ini_paths, dir.path()).unwrap();
        assert!(result.is_some());

        let group_ini = result.unwrap();
        assert!(group_ini.exists());
        let content = fs::read_to_string(&group_ini).unwrap();
        assert!(content.contains("group_1"));
        assert!(content.contains("$group_id = 1"));
        assert!(content.contains("include = _MANAGED_/group_1/Mod1/mod.ini"));
        assert!(content.contains("include = _MANAGED_/group_1/Mod2/config.ini"));
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
            "[TextureOverrideTest]\nhash = 0x12345678\nps-t0 = ResourceTest\ndrawindexed = auto\n"
        );

        let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        assert_eq!(result.total_mods, 1);
        assert_eq!(result.enabled_mods, 1);
        assert_eq!(result.processed_mods, 1);

        // 验证 group_1.ini 已创建
        let group_ini = group_path.join("group_1.ini");
        assert!(group_ini.exists());
        let group_content = fs::read_to_string(&group_ini).unwrap();
        assert!(group_content.contains("include = _MANAGED_/group_1/TestMod/mod.ini"));

        // 验证 nrmm_include.ini 包含 group_1.ini
        let include_content = fs::read_to_string(dir.path().join("_MANAGED_/nrmm_include.ini")).unwrap();
        assert!(include_content.contains("manager_group.ini"));
        assert!(include_content.contains("group_1/group_1.ini"));

        let mod_ini_path = group_path.join("TestMod/mod.ini");
        let backup_path = mod_ini_path.with_extension(constants::BACKUP_EXTENSION);
        assert!(backup_path.exists());
    }

    #[test]
    fn test_restore_all_inis() {
        let dir = setup_test_env();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let group_path = create_group_dir(dir.path(), "group_1");
        let _mod_path = create_mod_with_ini(
            &group_path,
            "RestoreMod",
            "[TextureOverrideTest]\nhash = 0x1\n"
        );

        update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();

        let main_ini_path = dir.path().join("d3dx.ini");
        let modified_content = fs::read_to_string(&main_ini_path).unwrap();
        // 对齐 NRMM 原版：d3dx.ini 不再注入 include 标记，依赖 include_recursive 自动加载
        assert!(!modified_content.contains(";NRMM_INI_START"));

        let result = restore_all_inis(dir.path()).unwrap();
        assert!(result.restored >= 2);
        assert_eq!(result.failed, 0);

        let restored_content = fs::read_to_string(&main_ini_path).unwrap();
        assert!(!restored_content.contains(";NRMM_INI_START"));

        let backup_path = main_ini_path.with_extension(constants::BACKUP_EXTENSION);
        assert!(!backup_path.exists());

        // 验证管理文件已清理
        let managed = dir.path().join("_MANAGED_");
        assert!(!managed.join("nrmm_include.ini").exists());
    }

    #[test]
    fn test_update_mod_data_creates_default_ini() {
        let dir = setup_test_env();
        let settings = AppSettings::default();

        assert!(!dir.path().join("d3dx.ini").exists());
        let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        assert_eq!(result.total_mods, 0);
        assert!(dir.path().join("d3dx.ini").exists());
    }

    #[test]
    fn test_update_mod_data_hsr_ini() {
        let dir = setup_test_env();
        let settings = AppSettings::default();

        let result = update_mod_data(TargetGame::HonkaiStarRail, dir.path(), &settings).unwrap();
        assert_eq!(result.total_mods, 0);
        assert!(dir.path().join("RatioShot.ini").exists());
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
        let dir = setup_test_env();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        // 先执行一次 update_mod_data 添加 NRMM 管理段
        update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();

        // 保存自定义设置
        let result = save_customizations(dir.path(), TargetGame::GenshinImpact).unwrap();
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
}

#[cfg(test)]
mod tests_disable_all {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// 创建测试辅助：在指定目录下创建含 .ini 的模组目录
    fn create_mod_dir(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        fs::create_dir_all(&dir).unwrap();
        // 创建一个 .ini 文件使该目录被识别为叶子模组节点
        fs::write(dir.join(format!("{}.ini", name)), "[ShaderOverride]").unwrap();
        dir
    }

    /// 正常场景：禁用3个启用模组，跳过1个已禁用模组，跳过无子分组目录
    #[test]
    fn test_disable_all_basic() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();

        let _mod_a = create_mod_dir(&group, "ModA");       // 启用
        let _mod_b = create_mod_dir(&group, "DISABLEDModB"); // 已禁用
        let _mod_c = create_mod_dir(&group, "ModC");       // 启用
        // 子分组目录（无 .ini），应跳过
        let subgroup = group.join("SubGroup");
        fs::create_dir_all(&subgroup).unwrap();
        let _mod_d = create_mod_dir(&group, "ModD");       // 启用

        let count = disable_all_mods_in_group(&group).unwrap();
        assert_eq!(count, 3); // ModA, ModC, ModD

        // 验证目录状态
        let entries: Vec<String> = fs::read_dir(&group).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.iter().any(|n| n == "DISABLEDModA"), "ModA 应被禁用");
        assert!(entries.iter().any(|n| n == "DISABLEDModB"), "DisabledModB 应保持不变");
        assert!(entries.iter().any(|n| n == "DISABLEDModC"), "ModC 应被禁用");
        assert!(entries.iter().any(|n| n == "DISABLEDModD"), "ModD 应被禁用");
        assert!(entries.iter().any(|n| n == "SubGroup"), "子分组目录应保持不变");
        assert_eq!(entries.len(), 5, "目录数量不变（重命名不改变数量）");
    }

    /// 幂等性：对已全部禁用的目录不产生任何变化
    #[test]
    fn test_disable_all_idempotent() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();

        create_mod_dir(&group, "DISABLEDModA");
        create_mod_dir(&group, "DISABLEDModB");

        let count = disable_all_mods_in_group(&group).unwrap();
        assert_eq!(count, 0, "全部已禁用时计数为0");
    }

    /// 无模组场景：仅有子分组或空目录
    #[test]
    fn test_disable_all_no_mods() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();

        let subgroup = group.join("SubGroup");
        fs::create_dir_all(&subgroup).unwrap();

        let count = disable_all_mods_in_group(&group).unwrap();
        assert_eq!(count, 0);
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
        fs::write(dir.join(format!("{}.ini", name.trim_start_matches("DISABLED"))), "[ShaderOverride]").unwrap();
        dir
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

        remove_group_ex(&group, true).unwrap();

        // 原分组应不存在
        assert!(!group.exists(), "原 group_1 应被移除");
        // DISABLED_MANAGED_REMOVED 下应有 group_1
        let removed_root = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
        assert!(removed_root.exists());
        let removed_group = removed_root.join("group_1");
        assert!(removed_group.exists(), "DISABLED_MANAGED_REMOVED 下应存在 group_1");
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

        remove_group_ex(&non_group, false).unwrap();

        // 原分组应不存在
        assert!(!non_group.exists());
        // 子分组应在 mods_root 下
        assert!(mods_root.join("SubGroup1").exists(), "SubGroup1 应移至 Mods 根目录");
        assert!(mods_root.join("SubGroup1").join("SubMod1").exists(), "SubGroup1 下的模组应保留");
        assert!(mods_root.join("SubGroup2").exists(), "SubGroup2 应移至 Mods 根目录");
        // 被移除目录在 DISABLED_MANAGED_REMOVED 下
        let removed = mods_root.join(constants::MANAGED_REMOVED_FOLDER).join("#MyMutexGroup");
        assert!(removed.exists());
        assert!(removed.join("ModA").exists(), "ModA 随原分组被移除");
        assert!(!removed.join("SubGroup1").exists(), "SubGroup1 不应随原分组被移除");
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

        remove_group_ex(&group, true).unwrap();

        assert!(!group.exists());
        assert!(removed_root.join("group_1").exists(), "原冲突项保留");
        assert!(removed_root.join("group_1_1").exists(), "新移除项追加 _1 后缀");
    }
}

#[cfg(test)]
mod tests_enable_all {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_mod_dir(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{}.ini", name.trim_start_matches("DISABLED"))), "[ShaderOverride]").unwrap();
        dir
    }

    /// 正常场景：启用3个禁用模组，跳过1个启用模组，跳过子分组
    #[test]
    fn test_enable_all_basic() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();

        create_mod_dir(&group, "DISABLEDModA");   // 需启用
        create_mod_dir(&group, "ModB");            // 已启用，跳过
        create_mod_dir(&group, "DISABLEDModC");   // 需启用
        // 子分组目录（无 .ini），跳过
        let subgroup = group.join("SubGroup");
        fs::create_dir_all(&subgroup).unwrap();
        create_mod_dir(&group, "DISABLEDModD");   // 需启用

        let count = enable_all_mods_in_group(&group).unwrap();
        assert_eq!(count, 3); // ModA, ModC, ModD

        let entries: Vec<String> = fs::read_dir(&group).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.iter().any(|n| n == "ModA"), "ModA 应被启用");
        assert!(entries.iter().any(|n| n == "ModB"), "ModB 保持已启用");
        assert!(entries.iter().any(|n| n == "ModC"), "ModC 应被启用");
        assert!(entries.iter().any(|n| n == "ModD"), "ModD 应被启用");
        assert!(entries.iter().any(|n| n == "SubGroup"), "子分组不变");
    }

    /// 幂等性：全部已启用计数为0
    #[test]
    fn test_enable_all_idempotent() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();
        create_mod_dir(&group, "ModA");
        create_mod_dir(&group, "ModB");

        let count = enable_all_mods_in_group(&group).unwrap();
        assert_eq!(count, 0);
    }

    /// 前缀后跟分隔符场景：DISABLED_MyMod 应为 MyMod
    #[test]
    fn test_enable_all_prefix_with_separator() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();
        create_mod_dir(&group, "DISABLED_MyMod");   // 下划线分隔
        create_mod_dir(&group, "DISABLED-OtherMod"); // 横杠分隔
        create_mod_dir(&group, "DISABLED Third");    // 空格分隔

        let count = enable_all_mods_in_group(&group).unwrap();
        assert_eq!(count, 3);

        let entries: Vec<String> = fs::read_dir(&group).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.iter().any(|n| n == "MyMod"));
        assert!(entries.iter().any(|n| n == "OtherMod"));
        assert!(entries.iter().any(|n| n == "Third"));
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

    /// 测试：detect_hash_conflicts 应扫描 MutexGroup（非group目录）下的模组
    ///
    /// 验证：非group目录下的模组参与 hash 冲突检测
    /// - 创建两个非group目录，各含一个使用相同 hash 的模组
    /// - 调用 detect_hash_conflicts 应检测到冲突
    #[test]
    fn test_hash_conflict_detects_mutex_group_mods() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // 非group目录1: #MutexA/ModA（使用 hash=0x12345678）
        let mutex_a = dir.path().join("_MANAGED_").join("#MutexA");
        fs::create_dir_all(&mutex_a).unwrap();
        create_mod_with_ini(
            &mutex_a,
            "ModA",
            "[TextureOverrideTexA]\nhash = 0x12345678\n",
        );

        // 非group目录2: #MutexB/ModB（使用相同 hash=0x12345678，应冲突）
        let mutex_b = dir.path().join("_MANAGED_").join("#MutexB");
        fs::create_dir_all(&mutex_b).unwrap();
        create_mod_with_ini(
            &mutex_b,
            "ModB",
            "[TextureOverrideTexB]\nhash = 0x12345678\n",
        );

        let result = detect_hash_conflicts(dir.path()).unwrap();
        // 应检测到至少 1 个冲突（hash=0x12345678 被 ModA 和 ModB 同时使用）
        assert!(
            !result.conflicts.is_empty(),
            "非group目录下的模组应参与 hash 冲突检测，但未检测到冲突"
        );
        // 验证冲突涉及两个模组
        let first_conflict = &result.conflicts[0];
        assert_eq!(first_conflict.entries.len(), 2);
        let mod_names: Vec<&str> = first_conflict.entries.iter().map(|e| e.mod_name.as_str()).collect();
        assert!(mod_names.contains(&"ModA"));
        assert!(mod_names.contains(&"ModB"));
        // 验证 entries 已填充且 ini_vec 指向 .ini 文件
        assert!(!first_conflict.entries.is_empty(), "entries 应填充详情");
        for entry in &first_conflict.entries {
            assert!(!entry.ini_vec.is_empty(), "ini_vec 不应为空");
            assert!(entry.ini_vec[0].ends_with(".ini"), "ini_vec 应指向 .ini 文件");
        }
    }

    /// 测试：detect_hash_conflicts 同时扫描 NormalGroup 和 MutexGroup
    ///
    /// 验证：group_xx 目录和非group目录下的模组都参与 hash 冲突检测
    #[test]
    fn test_hash_conflict_scans_both_group_types() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // group_1 下的启用模组（需要在 selectedindex 中标记为选中）
        let group1 = dir.path().join("_MANAGED_").join("group_1");
        fs::create_dir_all(&group1).unwrap();
        fs::write(group1.join("selectedindex"), "1").unwrap();
        create_mod_with_ini(
            &group1,
            "NormalMod",
            "[TextureOverrideNormal]\nhash = 0xdeadbeef\n",
        );

        // 非group目录下的启用模组
        let mutex_group = dir.path().join("_MANAGED_").join("#Mutex");
        fs::create_dir_all(&mutex_group).unwrap();
        create_mod_with_ini(
            &mutex_group,
            "MutexMod",
            "[TextureOverrideMutex]\nhash = 0xdeadbeef\n",
        );

        let result = detect_hash_conflicts(dir.path()).unwrap();
        // 应检测到冲突：NormalMod 和 MutexMod 使用相同 hash
        assert!(
            !result.conflicts.is_empty(),
            "应检测到 NormalGroup 和 MutexGroup 之间的 hash 冲突"
        );
        let conflict = &result.conflicts[0];
        assert_eq!(conflict.entries.len(), 2);
        let mod_names: Vec<&str> = conflict.entries.iter().map(|e| e.mod_name.as_str()).collect();
        assert!(mod_names.contains(&"NormalMod"));
        assert!(mod_names.contains(&"MutexMod"));
        // 验证 entries 已填充且 ini_vec 指向 .ini 文件
        assert!(!conflict.entries.is_empty(), "entries 应填充详情");
        for entry in &conflict.entries {
            assert!(!entry.ini_vec.is_empty(), "ini_vec 不应为空");
            assert!(entry.ini_vec[0].ends_with(".ini"), "ini_vec 应指向 .ini 文件");
        }
    }

    /// 测试：switch_mod 不会在 MutexGroup 目录下写入 selectedindex 文件
    ///
    /// 验证：传入 group_index=0（MutexGroup 默认 group_index）时，
    /// 不会在 MutexGroup 目录下创建 selectedindex 文件
    #[test]
    fn test_switch_mod_no_selectedindex_in_mutex_group() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let settings = AppSettings::default();

        // 非group目录（MutexGroup 根分组 group_index 默认为 0）
        let mutex_group = dir.path().join("_MANAGED_").join("#MutexGroup");
        fs::create_dir_all(&mutex_group).unwrap();
        create_mod_with_ini(&mutex_group, "MutexMod", "[Section]\n");

        // 调用 switch_mod 传入 group_index=0（对应 MutexGroup）
        let _result = switch_mod(
            TargetGame::GenshinImpact,
            dir.path(),
            &settings,
            0, // group_index=0 对应 MutexGroup
            1,
        ).unwrap();

        // 验证 MutexGroup 目录下未创建 selectedindex 文件
        assert!(
            !mutex_group.join(constants::SELECTED_INDEX_FILE).exists(),
            "非group目录不应创建 selectedindex 文件"
        );
        // 验证 MutexGroup 目录下未创建 groupname 文件
        assert!(
            !mutex_group.join("groupname").exists(),
            "非group目录不应创建 groupname 文件"
        );
    }

    /// 测试：switch_mod 对 NormalGroup 正常写入 selectedindex 文件
    ///
    /// 验证：group_xx 目录下调用 switch_mod 仍正常工作
    #[test]
    fn test_switch_mod_writes_selectedindex_for_normal_group() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let settings = AppSettings::default();

        // group_1（NormalGroup，group_index=1）
        let group1 = dir.path().join("_MANAGED_").join("group_1");
        fs::create_dir_all(&group1).unwrap();
        create_mod_with_ini(&group1, "TestMod", "[Section]\n");

        // 调用 switch_mod 传入 group_index=1（对应 NormalGroup）
        let _result = switch_mod(
            TargetGame::GenshinImpact,
            dir.path(),
            &settings,
            1, // group_index=1 对应 group_1
            1, // mod_index=1（TestMod）
        ).unwrap();

        // 验证 group_1 目录下已创建/更新 selectedindex 文件
        assert!(
            group1.join(constants::SELECTED_INDEX_FILE).exists(),
            "NormalGroup 应创建 selectedindex 文件"
        );
        let content = fs::read_to_string(group1.join(constants::SELECTED_INDEX_FILE)).unwrap();
        assert_eq!(content.trim(), "1");
    }

    /// 测试：deselect_group_mods 写入 selectedindex=0 且不重命名禁用模组
    ///
    /// 验证：对 NormalGroup 调用 deselect_group_mods 应写入 "0"（选择 None 槽位），
    /// 且不应通过 DISABLED 前缀重命名模组目录。
    #[test]
    fn test_deselect_group_mods_writes_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let mods_path = tmp.path();
        let managed = mods_path.join("_MANAGED_");
        let group_dir = managed.join("group_1");
        fs::create_dir_all(&group_dir).unwrap();
        let mod_dir = group_dir.join("TestMod");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("test.ini"), "[Section]\nkey=val\n").unwrap();

        let settings = AppSettings::default();
        let result = deselect_group_mods(
            TargetGame::GenshinImpact,
            mods_path,
            &settings,
            1,
        ).unwrap();

        assert_eq!(result.selected_mod_index, Some(0));
        let sel = fs::read_to_string(group_dir.join("selectedindex")).unwrap();
        assert_eq!(sel, "0");
        assert!(mod_dir.exists(), "mod directory should not be renamed");
        assert!(!group_dir.join("DISABLEDTestMod").exists());
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
        assert_eq!(ini_files.len(), 2, "应仅收集直接 .ini 文件，排除 desktop/backup/nested");
        let file_names: Vec<String> = ini_files.iter()
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
        let mutex_mod = create_mod_with_ini(&mutex_group, "MutexMod", "[TextureOverride]\nhash=0x2\n");

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

    /// 测试：同一模组在多个 INI 文件中使用相同 hash 时，聚合到该模组的 ini_vec
    ///
    /// 验证边界场景：
    /// - 模组 ModA 有两个 INI 文件（a1.ini、a2.ini），都使用 hash=0xcafebabe
    /// - 模组 ModB 有一个 INI 文件，也使用 hash=0xcafebabe
    /// - entries 应包含 2 条记录（ModA、ModB 各一条）
    /// - ModA 的 ini_vec 应有 2 个元素（a1.ini、a2.ini），ModB 的 ini_vec 应有 1 个元素
    #[test]
    fn test_hash_conflict_entries_tracks_ini_path() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // ModA：两个 INI 文件都使用 hash=0xcafebabe
        let mutex_a = dir.path().join("_MANAGED_").join("#MutexA");
        fs::create_dir_all(&mutex_a).unwrap();
        let mod_a_dir = mutex_a.join("ModA");
        fs::create_dir_all(&mod_a_dir).unwrap();
        fs::write(mod_a_dir.join("a1.ini"), "[TextureOverrideT1]\nhash = 0xcafebabe\n").unwrap();
        fs::write(mod_a_dir.join("a2.ini"), "[TextureOverrideT2]\nhash = 0xcafebabe\n").unwrap();

        // ModB：一个 INI 文件使用相同 hash
        let mutex_b = dir.path().join("_MANAGED_").join("#MutexB");
        fs::create_dir_all(&mutex_b).unwrap();
        create_mod_with_ini(
            &mutex_b,
            "ModB",
            "[TextureOverrideT3]\nhash = 0xcafebabe\n",
        );

        let result = detect_hash_conflicts(dir.path()).unwrap();
        assert!(!result.conflicts.is_empty(), "应检测到 hash 冲突");

        let conflict = &result.conflicts[0];
        assert_eq!(conflict.hash, "0xcafebabe");

        // entries 按模组聚合：ModA、ModB 各一条
        assert_eq!(
            conflict.entries.len(),
            2,
            "entries 应包含 2 条记录（ModA、ModB 各一条），实际：{:?}",
            conflict.entries.iter().map(|e| &e.mod_name).collect::<Vec<_>>()
        );

        // 验证 ModA 聚合了两条 INI 路径
        let mod_a_entry = conflict.entries.iter()
            .find(|e| e.mod_name == "ModA")
            .expect("应存在 ModA 的 entry");
        assert_eq!(mod_a_entry.ini_vec.len(), 2, "ModA 的 ini_vec 应有 2 个元素");
        assert!(mod_a_entry.ini_vec.iter().any(|p| p.ends_with("a1.ini")), "应包含 a1.ini 路径");
        assert!(mod_a_entry.ini_vec.iter().any(|p| p.ends_with("a2.ini")), "应包含 a2.ini 路径");

        // 验证 ModB 聚合了一条 INI 路径
        let mod_b_entry = conflict.entries.iter()
            .find(|e| e.mod_name == "ModB")
            .expect("应存在 ModB 的 entry");
        assert_eq!(mod_b_entry.ini_vec.len(), 1, "ModB 的 ini_vec 应有 1 个元素");
    }
}
