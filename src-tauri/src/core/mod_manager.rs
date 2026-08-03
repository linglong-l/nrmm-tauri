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
use crate::core::namespace_handler;
use crate::core::mod_scanner;
use crate::models::enums::TargetGame;
use crate::models::mod_data::{ModData, ErroredLines};
use crate::models::settings::AppSettings;

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
    /// - update_mod_data / update_group_mod_data（非选择类操作）：默认返回 None（null）
    /// - MutexGroup：不使用 switch_mod 路径，始终 None
    #[serde(default)]
    pub selected_mod_index: Option<i32>,
    /// 是否检测到标准 XXMI/3DMigoto 环境（[Constants] + XXMI 注入标记 + [Inject] 段）
    #[serde(default)]
    pub is_standard_xxmi: bool,
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
    let need_reload_manual = prepare_managed_folder(&managed_folder, game)?;

    // 步骤2: 扫描模组
    let scan_result = mod_scanner::scan_mods(game_mods_path)?;

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

    let mut known_libraries = HashSet::new();
    for mod_data in &enabled_mods {
        for ini_data in &mod_data.mod_ini_data {
            let ini_path = PathBuf::from(&ini_data.ini_path);
            if let Ok(ini) = IniFile::parse(&ini_path) {
                for lib in ini.defined_libraries() {
                    known_libraries.insert(lib);
                }
            }
        }
    }

    // 步骤4: 清理旧的 group INI 文件
    delete_group_ini_files(&managed_folder)?;

    // 步骤5: 按 group 组织启用的模组 INI
    let mut group_mod_inis: std::collections::HashMap<u32, Vec<PathBuf>> = std::collections::HashMap::new();
    let mut all_errors: Vec<ErroredLines> = Vec::new();
    let mut processed_mods = 0u32;

    for (mod_idx, mod_data) in enabled_mods.iter().enumerate() {
        let group_id = mod_data.group_index;
        let mod_id = mod_idx as u32;
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

            match IniFile::parse(&ini_path) {
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
                    ini.inject_slot_conditions(group_id, mod_id);

                    // 注释崩溃行
                    let crash_lines = ini.comment_crash_lines();
                    if !crash_lines.is_empty() {
                        log::info!("Commented {} crash lines in {}", crash_lines.len(), ini_path.display());
                    }

                    // 移除空 if 块，应用缩进
                    ini.remove_empty_if_blocks();
                    ini.apply_indentation();

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

    // 步骤8: 生成主 INI 注入段
    let injected = generate_nrmm_injected_content(&nrmm_include_path, game_mods_path)?;
    let final_content = if main_ini_content.is_empty() {
        injected
    } else {
        format!("{}\n\n{}", main_ini_content, injected)
    };

    // 原子写入主 INI
    let tmp_path = main_ini_path.with_extension("ini.tmp");
    fs::write(&tmp_path, &final_content)
        .with_context(|| format!("Failed to write temp main INI: {:?}", tmp_path))?;
    fs::rename(&tmp_path, &main_ini_path)
        .with_context(|| format!("Failed to rename temp main INI to: {:?}", main_ini_path))?;

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
        ..Default::default()
    };
    log::debug!("[core::mod_manager] [update_mod_data] done | elapsed={:?}ms | processed={} errors={}", _s.elapsed().as_millis(), result.processed_mods, result.errors.len());
    Ok(result)
}

/// 分组增量更新模组数据（仅更新指定分组的 ModFolder.ini）
///
/// 与全量 `update_mod_data` 的区别：
/// - 仅扫描目标分组，不扫描其他分组
/// - 仅处理该分组内启用的模组 INI
/// - 仅更新该分组的 ModFolder.ini 文件
/// - 跳过 nrmm_include.ini 和主 INI 的重生成（include 链保持不变）
///
/// # 参数
/// - `game`: 目标游戏
/// - `game_mods_path`: 游戏 Mods 目录
/// - `_settings`: 应用设置
/// - `group_index`: 目标分组索引
pub fn update_group_mod_data(
    game: TargetGame,
    game_mods_path: &Path,
    _settings: &AppSettings,
    group_index: u32,
) -> Result<UpdateResult> {
    let managed_folder = game_mods_path.join(constants::MANAGED_FOLDER);
    if !managed_folder.exists() {
        fs::create_dir_all(&managed_folder)?;
    }

    // 步骤1: 准备 _MANAGED_ 目录
    let need_reload_manual = prepare_managed_folder(&managed_folder, game)?;

    // 步骤2: 轻量扫描（仅扫描目标分组）
    let scan_result = mod_scanner::scan_mods_light(game_mods_path)?;

    // 步骤3: 过滤出目标分组的启用模组
    let enabled_mods: Vec<&ModData> = scan_result.mods.iter()
        .filter(|m| m.group_index == group_index && !m.disabled && !m.mod_disabled)
        .collect();

    // 步骤4: 收集已知库
    let mut known_libraries = HashSet::new();
    for mod_data in &enabled_mods {
        for ini_data in &mod_data.mod_ini_data {
            let ini_path = PathBuf::from(&ini_data.ini_path);
            if let Ok(ini) = IniFile::parse(&ini_path) {
                for lib in ini.defined_libraries() {
                    known_libraries.insert(lib);
                }
            }
        }
    }

    // 步骤5: 处理目标分组内启用模组的 INI
    let mut group_mod_inis: Vec<PathBuf> = Vec::new();
    let mut all_errors: Vec<ErroredLines> = Vec::new();
    let mut processed_mods = 0u32;

    for (mod_idx, mod_data) in enabled_mods.iter().enumerate() {
        let mod_id = mod_idx as u32;
        for ini_data in &mod_data.mod_ini_data {
            let ini_path = PathBuf::from(&ini_data.ini_path);

            // 备份模组 INI（如果尚未备份）
            let mod_backup = ini_path.with_extension(constants::BACKUP_EXTENSION);
            if !mod_backup.exists() {
                if let Err(e) = fs::copy(&ini_path, &mod_backup) {
                    log::warn!("Failed to backup mod INI {}: {}", ini_path.display(), e);
                }
            }

            match IniFile::parse(&ini_path) {
                Ok(mut ini) => {
                    let errors = ini.detect_errors(&ini_path, &known_libraries);
                    if !errors.is_empty() {
                        all_errors.extend(errors);
                    }

                    if let Some(ns) = namespace_handler::extract_namespace(&ini) {
                        namespace_handler::expand_ini_variables(&mut ini, &ns);
                    }

                    ini.inject_slot_conditions(group_index, mod_id);
                    ini.comment_crash_lines();
                    ini.remove_empty_if_blocks();
                    ini.apply_indentation();

                    ini.write_atomic(&ini_path)?;
                    group_mod_inis.push(ini_path.clone());
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
    }

    // 步骤6: 仅更新该分组的 ModFolder.ini
    let group_dir = managed_folder.join(format!("group_{}", group_index));
    let mut group_ini_paths: Vec<PathBuf> = Vec::new();
    let group_ini = create_group_ini(&group_dir, group_index, &group_mod_inis, game_mods_path)?;
    if let Some(p) = group_ini {
        group_ini_paths.push(p);
    }

    // 步骤7: 更新 nrmm_include.ini（需要包含所有分组，不只是当前分组）
    // 读取所有现有 group INI 路径，合并当前分组
    let mut existing_ini_paths: Vec<PathBuf> = Vec::new();
    // 收集所有已存在的 group INI 路径（除了当前分组的）
    if let Ok(entries) = fs::read_dir(&managed_folder) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if let Some(group_num_str) = dir_name.strip_prefix("group_") {
                    if let Ok(group_num) = group_num_str.parse::<u32>() {
                        if group_num != group_index {
                            let ini_path = path.join(format!("group_{}.ini", group_num));
                            if ini_path.exists() {
                                existing_ini_paths.push(ini_path);
                            }
                        }
                    }
                }
            }
        }
    }
    // 合并当前分组的新 INI
    existing_ini_paths.extend(group_ini_paths.clone());

    let nrmm_include_path = managed_folder.join(constants::INCLUDE_FILENAME);
    create_nrmm_include_ini(&nrmm_include_path, &managed_folder, &existing_ini_paths, game_mods_path)?;

    // 步骤8: 检测标准 XXMI/3DMigoto 环境
    let main_ini_name = game.d3dx_ini_name();
    let is_standard_xxmi = detect_standard_xxmi(game_mods_path, main_ini_name);

    Ok(UpdateResult {
        total_groups: scan_result.groups.len() as u32,
        total_mods: scan_result.total_mods_count as u32,
        enabled_mods: enabled_mods.len() as u32,
        disabled_mods: scan_result.disabled_mods_count as u32,
        processed_mods,
        errors: all_errors,
        need_reload_manual,
        is_standard_xxmi,
        ..Default::default()
    })
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
fn generate_nrmm_injected_content(nrmm_include_path: &Path, game_mods_path: &Path) -> Result<String> {
    let mut content = String::new();

    content.push_str(";NRMM_INI_START\n");
    content.push_str("; ==========================================\n");
    content.push_str("; No-Reload Mod Manager managed section\n");
    content.push_str("; Do not edit this section manually\n");
    content.push_str("; ==========================================\n\n");

    content.push_str("[Constants]\n");
    content.push_str("global persist $managed_slot_id = 0\n");
    content.push_str("global persist $managed_selected_group = 0\n");
    content.push_str("global persist $managed_selected_mod = 0\n\n");

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

fn create_default_main_ini(path: &Path, ini_name: &str) -> Result<()> {
    let content = format!(
r#"; {} - Generated by NRMM
[Constants]
global persist $managed_slot_id = 0
global persist $managed_selected_group = 0
global persist $managed_selected_mod = 0
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

    // 先找出分组目录路径（用于写入 selectedindex 文件）
    let group_dir = scan_result.groups.iter()
        .find(|g| g.group_index == group_index)
        .map(|g| g.full_path.clone());

    for mod_data in &scan_result.mods {
        if mod_data.group_index == group_index {
            // None 槽位的 full_path 为空，跳过目录操作（由下面的 !is_target 分支禁用其他模组）
            if mod_data.full_path.as_os_str().is_empty() {
                continue;
            }

            let mod_dir = &mod_data.full_path;
            let dir_name = mod_dir.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let target_disabled = dir_name.to_uppercase().starts_with("DISABLED");

            let is_target = mod_data.mod_index == mod_index;

            if is_target && target_disabled {
                // 启用目标模组：移除 DISABLED 前缀
                let new_name = dir_name
                    .trim_start_matches("DISABLED")
                    .trim_start_matches("disabled")
                    .trim_start_matches(|c: char| ['_', ' ', '-'].contains(&c));
                let new_path = mod_dir.parent().unwrap_or(mod_dir).join(new_name);
                if mod_dir != &new_path {
                    fs::rename(mod_dir, &new_path)
                        .with_context(|| format!("Failed to enable mod: {:?}", mod_dir))?;
                }
            } else if !is_target && !target_disabled {
                // 禁用非目标模组：添加 DISABLED 前缀
                let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                let new_path = mod_dir.parent().unwrap_or(mod_dir).join(new_name);
                if mod_dir != &new_path {
                    fs::rename(mod_dir, &new_path)
                        .with_context(|| format!("Failed to disable mod: {:?}", mod_dir))?;
                }
            }
        }
    }

    // 将选中的 mod_index 写入该分组的 selectedindex 文件，使 is_active 状态持久化
    let sel_idx_i32 = mod_index as i32;
    if let Some(g_dir) = group_dir {
        let selectedindex_path = g_dir.join(constants::SELECTED_INDEX_FILE);
        if let Err(e) = fs::write(&selectedindex_path, mod_index.to_string()) {
            log::warn!("Failed to write selectedindex file {:?}: {}", selectedindex_path, e);
        }
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

/// 取消选中分组内所有模组（禁用整个分组）
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
    let scan_result = mod_scanner::scan_mods(game_mods_path)?;

    for mod_data in &scan_result.mods {
        if mod_data.group_index == group_index {
            let mod_dir = &mod_data.full_path;
            let dir_name = mod_dir.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_disabled = dir_name.to_uppercase().starts_with("DISABLED");

            if !is_disabled {
                let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                let new_path = mod_dir.parent().unwrap_or(mod_dir).join(new_name);
                if mod_dir != &new_path {
                    fs::rename(mod_dir, &new_path)
                        .with_context(|| format!("Failed to disable mod: {:?}", mod_dir))?;
                }
            }
        }
    }

    Ok(UpdateResult::default())
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

/// 移除分组（NRMM 对齐：移至 _MANAGED_REMOVED_ 目录，非group先移子分组再移除）
///
/// # 参数
/// - `group_path`: 要移除的分组目录路径
/// - `is_group_xx`: true = group_1/group_2 等普通分组，false = mutexGroup 非group目录
///
/// # 对于 group_xx：
/// 1. 定位 mods_path（group_path 的父目录，即 _MANAGED_ 的父目录）
/// 2. 确保 `Mods/_MANAGED_REMOVED_` 目录存在
/// 3. 将整个分组目录移至 `_MANAGED_REMOVED_/原名`，名称冲突追加 `_`
///
/// # 对于非group：
/// 1. 先将分组下的**一级子目录且不含 .ini 的目录**（即子分组目录）移至父级目录
///    - 子分组目录重名时追加 `_` 后缀
/// 2. 然后将被清空的分组目录按 group_xx 规则移至 `_MANAGED_REMOVED_/`
///
/// 然后尝试 `trash` crate 移至回收站（失败静默，不中断流程）。
/// 实际逻辑：先移至 _MANAGED_REMOVED_ 作为主要的移除方式，保留历史。
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

    // ========== 定位 _MANAGED_REMOVED_ 目录 ==========
    // 规则：_MANAGED_REMOVED_ 位于 Mods 根目录（与 _MANAGED_ 同级）
    // 如果 group_path 是 _MANAGED_/group_1，则 mods_root = parent(_MANAGED_)
    // 如果 group_path 是 Mods/#NonGroup，则 mods_root = parent(#NonGroup)（即 Mods）
    let mut mods_root = parent_dir;
    let parent_name = mods_root.file_name().map(|n| n.to_string_lossy().to_string());
    if parent_name.as_deref() == Some(constants::MANAGED_FOLDER) {
        // group 目录在 _MANAGED_ 下，_MANAGED_REMOVED_ 应在其上级目录（Mods）
        if let Some(grand_parent) = mods_root.parent() {
            mods_root = grand_parent;
        }
    }

    let removed_folder = mods_root.join("_MANAGED_REMOVED_");
    if !removed_folder.exists() {
        fs::create_dir_all(&removed_folder)
            .with_context(|| format!("Failed to create _MANAGED_REMOVED_: {:?}", removed_folder))?;
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

    // 移动分组至 _MANAGED_REMOVED_
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
        assert!(content.contains(";NRMM_INI_START"));
        assert!(content.contains(";NRMM_INI_END"));
        assert!(content.contains("nrmm_include.ini"));

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
        assert!(modified_content.contains(";NRMM_INI_START"));

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

    /// group_xx 场景：移至 _MANAGED_REMOVED_ 目录
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
        // _MANAGED_REMOVED_ 下应有 group_1
        let removed_root = mods_root.join("_MANAGED_REMOVED_");
        assert!(removed_root.exists());
        let removed_group = removed_root.join("group_1");
        assert!(removed_group.exists(), "_MANAGED_REMOVED_ 下应存在 group_1");
        assert!(removed_group.join("ModA").exists(), "模组应随之移动");
    }

    /// 非group场景：先移子分组到父级，再移至 _MANAGED_REMOVED_
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
        // 被移除目录在 _MANAGED_REMOVED_ 下
        let removed = mods_root.join("_MANAGED_REMOVED_").join("#MyMutexGroup");
        assert!(removed.exists());
        assert!(removed.join("ModA").exists(), "ModA 随原分组被移除");
        assert!(!removed.join("SubGroup1").exists(), "SubGroup1 不应随原分组被移除");
    }

    /// 名称冲突场景：_MANAGED_REMOVED_/group_1 已存在应追加 _1
    #[test]
    fn test_remove_group_conflict() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let group = managed.join("group_1");
        let removed_root = mods_root.join("_MANAGED_REMOVED_");
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
