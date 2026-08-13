//! 模组相关 Tauri 命令
//!
//! 提供模组管理的前端接口：
//! - get_mods: 获取模组列表（优先缓存，轻量扫描）
//! - refresh_mods: 强制刷新模组列表（轻量扫描，更新缓存）
//! - check_mods_path_status: 检查模组路径有效性
//! - apply_mods: 应用模组选择（深度扫描+INI注入）
//! - switch_mod: 切换普通分组选中模组
//! - deselect_group: 取消选中分组
//! - toggle_mod_enabled/toggle_mutex_mod_enabled: 启用/禁用模组
//! - toggle_favorite: 切换收藏
//! - open_mod_folder: 打开模组目录
//! - import_mod_from_archive: 从压缩包导入模组
//! - delete_mod: 删除模组
//! - restore_all_inis: 恢复所有 INI 备份
//! - select_folder: 系统文件夹选择对话框
//!
//! # 性能设计
//! - 列表查询用轻量扫描+缓存（<100ms）
//! - apply 操作用深度扫描（完整 INI 解析，可能几秒）
//! - 所有 IO 操作使用 spawn_blocking 避免阻塞 UI

use crate::config::settings_store;
use crate::core::constants;
use crate::core::mod_cache;
use crate::core::mod_manager;
use crate::core::mod_scanner;
use crate::core::resolution;
use crate::models::enums::TargetGame;
use crate::sel_dbg;
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

static SELECTION_DEBOUNCE: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 选择模组命令的参数结构体
///
/// 将 select_mod 命令的 9 个独立参数封装为单个结构体，
/// 以避免 clippy::too_many_arguments 警告，并提升参数可读性。
/// 通过 #[serde(rename_all = "camelCase")] 与前端 camelCase 键名保持一致。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectModArgs {
    /// 目标游戏标识（如 "wuwa"、"genshin"）
    game: String,
    /// 模组根目录路径
    mods_path: String,
    /// 分组索引
    group_index: u32,
    /// 模组索引
    mod_index: u32,
    /// 是否为互斥组
    is_mutex: bool,
    /// 分组路径
    group_path: String,
    /// 模组路径
    mod_path: String,
    /// 屏幕光标 X 坐标（可选，像素值）
    cursor_x: Option<i32>,
    /// 屏幕光标 Y 坐标（可选，像素值）
    cursor_y: Option<i32>,
}

/// 获取模组列表（轻量扫描+缓存）
///
/// 优先从内存缓存返回，缓存未命中时执行轻量扫描。
/// 这是 UI 初始化和列表展示的主要入口。
#[tauri::command]
pub async fn get_mods(game: String, mods_path: String) -> Result<mod_scanner::ScanResult, String> {
    log::debug!(
        "[commands::mod_commands] [get_mods] game={} mods_path={}",
        game,
        mods_path
    );
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    {
        let cache = crate::core::mod_cache::MOD_CACHE.read();
        if let Some(result) = cache.get(game, &mods_path) {
            log::info!("[get_mods] Cache hit for {}", game.as_str());
            return Ok(result);
        }
    }

    log::info!("[get_mods] Cache miss, scanning light...");
    let start = std::time::Instant::now();

    let scan_path = mods_path.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<mod_scanner::ScanResult, String> {
            mod_scanner::scan_mods_light(&scan_path)
                .map_err(crate::core::error_normalizer::err_to_ui)
        })
        .await
        .map_err(|e| {
            log::error!("[get_mods] spawn_blocking join error: {}", e);
            crate::core::error_normalizer::join_error_to_ui()
        })??;

    let elapsed = start.elapsed();
    log::info!(
        "Light scan completed in {}ms, {} mods, {} groups",
        elapsed.as_millis(),
        result.total_mods_count,
        result.groups.len()
    );

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.set(game, &mods_path, result.clone());
    }

    Ok(result)
}

#[tauri::command]
pub fn check_mods_path_status(
    game: String,
    mods_path: String,
) -> Result<crate::models::enums::ModsPathStatus, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    Ok(mod_scanner::check_mods_path(game, &mods_path))
}

/// 刷新模组列表（轻量扫描，更新缓存）
#[tauri::command]
pub async fn refresh_mods(
    game: String,
    mods_path: String,
) -> Result<mod_scanner::ScanResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    log::info!("[refresh_mods] Scanning light...");
    let start = std::time::Instant::now();

    let scan_path = mods_path.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<mod_scanner::ScanResult, String> {
            mod_scanner::scan_mods_light(&scan_path)
                .map_err(crate::core::error_normalizer::err_to_ui)
        })
        .await
        .map_err(|e| {
            log::error!("[refresh_mods] spawn_blocking join error: {}", e);
            crate::core::error_normalizer::join_error_to_ui()
        })??;

    let elapsed = start.elapsed();
    log::info!(
        "Light refresh completed in {}ms, {} mods, {} groups",
        elapsed.as_millis(),
        result.total_mods_count,
        result.groups.len()
    );

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.set(game, &mods_path, result.clone());
    }

    Ok(result)
}

/// 重量级更新模组数据（仅用户按钮触发）
#[tauri::command]
pub async fn update_mod_data(
    game: String,
    mods_path: String,
) -> Result<mod_manager::UpdateResult, String> {
    log::debug!(
        "[commands::mod_commands] [update_mod_data] game={} mods_path={}",
        game,
        mods_path
    );
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let settings = settings_store::get_settings();
    let managed_path = mods_path.join(constants::MANAGED_FOLDER);

    log::info!("[update_mod_data] Running heavy update...");
    let start = std::time::Instant::now();

    let update_path = mods_path.clone();
    // spawn_blocking 内部若 panic，JoinError 会被转为 String 错误返回，不会导致应用崩溃
    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<mod_manager::UpdateResult, String> {
            mod_manager::update_mod_data(game, &update_path, &settings)
                .map_err(crate::core::error_normalizer::err_to_ui)
        },
    )
    .await
    .map_err(|e| {
        log::error!("[update_mod_data] spawn_blocking join error: {}", e);
        crate::core::error_normalizer::join_error_to_ui()
    })?
    .map_err(|e| {
        log::error!("[update_mod_data] inner error: {}", e);
        e
    })?;

    let elapsed = start.elapsed();
    log::info!(
        "Heavy update completed in {}ms, processed {} mods",
        elapsed.as_millis(),
        result.processed_mods
    );

    // 缓存失效
    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_by_prefix(&managed_path);
    }

    Ok(result)
}

/// 检测 hash 冲突命令
///
/// 全量扫描所有模组 INI 中的 hash 值，返回冲突列表。
/// 由用户在设置页面主动触发，扫描策略：
/// - NormalGroup（group_xx）：仅扫描当前选中模组
/// - MutexGroup（非 group_xx）：扫描所有启用模组
#[tauri::command]
pub async fn detect_hash_conflicts(
    mods_path: String,
) -> Result<mod_manager::HashConflictResult, String> {
    let mods_path = PathBuf::from(mods_path);
    log::info!(
        "[detect_hash_conflicts] Starting full hash conflict scan: {}",
        mods_path.display()
    );
    let start = std::time::Instant::now();

    let scan_path = mods_path.clone();
    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<mod_manager::HashConflictResult, String> {
            mod_manager::detect_hash_conflicts(&scan_path)
                .map_err(crate::core::error_normalizer::err_to_ui)
        },
    )
    .await
    .map_err(|e| {
        log::error!("[detect_hash_conflicts] spawn_blocking join error: {}", e);
        crate::core::error_normalizer::join_error_to_ui()
    })??;

    log::info!(
        "[detect_hash_conflicts] Completed in {}ms, found {} conflicts",
        start.elapsed().as_millis(),
        result.conflicts.len()
    );
    Ok(result)
}

/// 选择模组（支持互斥组）
#[tauri::command]
pub async fn select_mod(args: SelectModArgs) -> Result<mod_manager::UpdateResult, String> {
    let SelectModArgs {
        game,
        mods_path,
        group_index,
        mod_index,
        is_mutex,
        group_path,
        mod_path,
        cursor_x,
        cursor_y,
    } = args;
    // 提取模组名称与分组名称，便于在调试日志中直接定位所属模组
    let mod_name = Path::new(&mod_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let group_name = Path::new(&group_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    sel_dbg!(
        "mod_commands",
        "select_mod",
        "入口 | 所属模组名称={} 所属分组名称={} game={} 分组索引={} 模组索引={} 是否互斥组={} 传入光标坐标=({:?},{:?}) 模组路径={} 分组路径={}",
        mod_name, group_name, game, group_index, mod_index, is_mutex, cursor_x, cursor_y, mod_path, group_path
    );
    log::debug!(
        "[commands::mod_commands] [select_mod] game={} group={} mod={} mutex={}",
        game,
        group_index,
        mod_index,
        is_mutex
    );
    let _start = std::time::Instant::now();
    let game_enum = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    sel_dbg!(
        "mod_commands",
        "select_mod",
        "分支判定 | is_mutex={} → 进入 {} 分支",
        is_mutex,
        if is_mutex {
            "互斥组(enable_mutex_mod，无按键模拟)"
        } else {
            "普通组(switch_mod 持久化 + 按键模拟)"
        }
    );
    if is_mutex {
        // Mutex 分支防抖（与非 Mutex 分支一致，防止快速重复点击触发多次互斥切换）
        {
            let mut debounce_map = SELECTION_DEBOUNCE
                .lock()
                .map_err(|_| "debounce lock poisoned")?;
            if let Some(last_time) = debounce_map.get(&group_path) {
                if Instant::now() - *last_time < Duration::from_millis(500) {
                    return Err("debounced".to_string());
                }
            }
            debounce_map.insert(group_path.clone(), Instant::now());
        }

        let mod_path_buf = PathBuf::from(mod_path.clone());
        let managed_path = mods_path.join(constants::MANAGED_FOLDER);
        sel_dbg!(
            "mod_commands",
            "select_mod",
            "互斥组分支 | 准备调用 enable_mutex_mod | 模组名称={} 模组路径={}",
            mod_name,
            mod_path
        );

        tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            mod_manager::enable_mutex_mod(&mod_path_buf).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;

        {
            let mut cache = crate::core::mod_cache::MOD_CACHE.write();
            cache.invalidate_by_prefix(&managed_path);
        }

        Ok(mod_manager::UpdateResult {
            need_reload_manual: false,
            ..Default::default()
        })
    } else {
        // === NormalGroup 分支 ===
        // 执行顺序对齐用户要求："先持久化再按键模拟"
        // 1. 防抖检查
        {
            let mut debounce_map = SELECTION_DEBOUNCE
                .lock()
                .map_err(|_| "debounce lock poisoned")?;
            if let Some(last_time) = debounce_map.get(&group_path) {
                if Instant::now() - *last_time < Duration::from_millis(500) {
                    return Err("debounced".to_string());
                }
            }
            debounce_map.insert(group_path.clone(), Instant::now());
        }

        // 2. 先持久化：写 selectedindex（switch_mod 内部有 WATCHER_PAUSED 保护，防止文件监听器竞态）
        let settings = settings_store::get_settings();
        let simulate_enabled = settings.simulate_key_on_selection;
        let managed_path = mods_path.join(constants::MANAGED_FOLDER);
        // TargetGame 是 Copy，提前复制用于后续按键模拟
        let game_enum_for_sim = game_enum;

        let result = tauri::async_runtime::spawn_blocking(
            move || -> Result<mod_manager::UpdateResult, String> {
                mod_manager::switch_mod(game_enum, &mods_path, &settings, group_index, mod_index)
                    .map_err(|e| e.to_string())
            },
        )
        .await
        .map_err(|e| {
            log::error!("[select_mod] spawn_blocking join error: {}", e);
            format!("switch task failed: {}", e)
        })?
        .map_err(|e| {
            log::error!("[select_mod] switch_mod error: {}", e);
            e
        })?;
        log::debug!(
            "[commands::mod_commands] [select_mod] switch_result={:?}",
            result
        );

        // 3. 清缓存（持久化完成后立即失效，确保下次 get_mods 全量扫描）
        {
            let mut cache = crate::core::mod_cache::MOD_CACHE.write();
            cache.invalidate_by_prefix(&managed_path);
        }
        log::debug!(
            "[commands::mod_commands] [select_mod] cache invalidated | prefix={:?}",
            managed_path
        );

        // 4. 再按键模拟（确保文件状态已就绪）
        // 对齐 NRMM：选择后不发送 F10，$active_slot 通过 persist 跨重载保持，
        // 3Dmigoto 会在下次自然重载时重新求值 if/endif 条件
        log::debug!(
            "[commands::mod_commands] [select_mod] simulate_key_on_selection={} (after persist)",
            simulate_enabled
        );
        // 选择逻辑(按键模拟)触发前：记录当前光标位置（屏幕坐标），用于对比模拟前后变化
        let cursor_before = crate::platform::get_foreground_detector()
            .get_cursor_position()
            .ok();
        sel_dbg!(
            "mod_commands",
            "select_mod",
            "选择逻辑(按键模拟)触发前光标位置={:?}（屏幕坐标，simulate_key_on_selection={}）",
            cursor_before,
            simulate_enabled
        );
        if simulate_enabled {
            let mut simulator = crate::platform::get_key_simulator();
            let process_names = game_enum_for_sim.process_names();
            for pn in process_names {
                if simulator.set_target_process(pn).is_ok() {
                    break;
                }
            }
            // 参数语义对齐NRMM：优先使用调用方传入的屏幕坐标（cursor_x/cursor_y 为实际像素时），
            // 否则 fallback 到 (mod_index, group_index) 作为虚拟坐标（3Dmigoto/xxmi 据此识别）。
            // 注意：NRMM 将 x=realModIndex y=realGroupIndex 直接传入 SetCursorPos，
            // 因此当未传像素坐标时，索引值本身即作为坐标输入。
            let (sim_g, sim_m) = match (cursor_x, cursor_y) {
                (Some(px), Some(py)) => {
                    let g = u32::try_from(py.max(0)).unwrap_or(group_index);
                    let m = u32::try_from(px.max(0)).unwrap_or(mod_index);
                    (g, m)
                }
                _ => (group_index, mod_index),
            };
            sel_dbg!(
                "mod_commands",
                "select_mod",
                "准备调用 simulate_select_full | 坐标模式={} g(分组索引)={} m(模组索引)={} 模组名称={}",
                if cursor_x.is_some() && cursor_y.is_some() { "屏幕像素坐标" } else { "索引虚拟坐标" },
                sim_g,
                sim_m,
                mod_name
            );
            let result: Result<(), String> = simulator
                .simulate_select_full(sim_g, sim_m)
                .map_err(|e| e.to_string());
            log::debug!(
                "[commands::mod_commands] [select_mod] simulate_result={:?}",
                result
            );
            if let Err(e) = result {
                log::warn!(
                    "select_mod: simulate_select_full failed (g={}, m={}): {}",
                    group_index,
                    mod_index,
                    e
                );
            }
        }

        // 选择逻辑(按键模拟)触发后：再次记录光标位置，与触发前对比，验证光标是否被正确还原
        let cursor_after = crate::platform::get_foreground_detector()
            .get_cursor_position()
            .ok();
        sel_dbg!(
            "mod_commands",
            "select_mod",
            "选择逻辑(按键模拟)触发后光标位置={:?}（屏幕坐标）| 触发前={:?} 是否一致={}",
            cursor_after,
            cursor_before,
            cursor_before == cursor_after
        );

        log::debug!("[commands::mod_commands] [select_mod] completed | elapsed={:?}ms | selected_mod_index={:?}", _start.elapsed().as_millis(), result.selected_mod_index);
        Ok(result)
    }
}

/// 取消选中分组内模组
#[tauri::command]
pub async fn deselect_group_mod(
    game: String,
    mods_path: String,
    group_index: u32,
) -> Result<mod_manager::UpdateResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let settings = settings_store::get_settings();
    let managed_path = mods_path.join(constants::MANAGED_FOLDER);

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<mod_manager::UpdateResult, String> {
            mod_manager::deselect_group_mods(game, &mods_path, &settings, group_index)
                .map_err(|e| e.to_string())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_by_prefix(&managed_path);
    }

    Ok(result)
}

/// 添加分组
#[tauri::command]
pub async fn add_group(
    game: String,
    mods_path: String,
    group_name: Option<String>,
) -> Result<mod_manager::UpdateResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<mod_manager::UpdateResult, String> {
            let managed_folder = mods_path.join(constants::MANAGED_FOLDER);
            if !managed_folder.exists() {
                fs::create_dir_all(&managed_folder).map_err(|e| e.to_string())?;
            }

            let mut used_numbers: Vec<u32> = Vec::new();
            if let Ok(entries) = fs::read_dir(&managed_folder) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if let Some(rest) = name.strip_prefix("group_") {
                        if let Ok(num) = rest.parse::<u32>() {
                            used_numbers.push(num);
                        }
                    }
                }
            }
            used_numbers.sort();
            let mut group_num = 1u32;
            for used in &used_numbers {
                if *used == group_num {
                    group_num += 1;
                } else if *used > group_num {
                    break;
                }
            }

            // 对齐 NRMM 约定：分组上限 = 屏幕 Y 轴（高度）上限，由动态分辨率上限决定。
            // Dart 以 500 作为硬编码安全上限；NRMM 实际约定为基于主屏幕分辨率的动态上限
            // （group=y / mod=x），故此处采用 resolution::compute_limits().max_groups 作为权威上限。
            let max_groups = resolution::compute_limits().max_groups;
            if group_num > max_groups {
                return Err(format!(
                    "分组数量已达动态上限（{}，基于屏幕分辨率 Y 轴）",
                    max_groups
                ));
            }

            let dir_name = format!("group_{}", group_num);
            let group_path = managed_folder.join(&dir_name);
            fs::create_dir(&group_path).map_err(|e| e.to_string())?;

            let template_str = String::from_utf8_lossy(crate::resources::TEMPLATE_GROUP);
            let ini_content = template_str
                .replace("{group_x}", &dir_name)
                .replace("{x}", &group_num.to_string());
            let ini_path = group_path.join("ModFolder.ini");
            fs::write(&ini_path, ini_content).map_err(|e| e.to_string())?;

            if let Some(custom_name) = group_name {
                let trimmed = custom_name.trim();
                if !trimmed.is_empty() && trimmed != dir_name {
                    let new_group_path = managed_folder.join(trimmed);
                    if !new_group_path.exists() {
                        fs::rename(&group_path, &new_group_path).map_err(|e| e.to_string())?;
                    }
                }
            }

            Ok(mod_manager::UpdateResult::default())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    let _ = game;
    Ok(result)
}

/// 删除分组（移至回收站）
#[tauri::command]
pub async fn remove_group(group_path: String) -> Result<(), String> {
    let path = PathBuf::from(group_path);

    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        if !path.exists() {
            return Err("Group path does not exist".to_string());
        }
        trash_delete(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(())
}

/// 移除模组（NRMM 对齐：移至 DISABLED_MANAGED_REMOVED + 还原 INI）
///
/// 完整流程：
/// 1. 将模组文件夹移动到 `Mods/DISABLED_MANAGED_REMOVED/<modname>`（冲突追加 _1、_2…）
/// 2. 对移动后的模组执行 INI 还原（清理所有 NRMM 注入内容）
/// 3. 清除模组缓存
///
/// 返回 `RemoveModResult`，包含移动后路径、INI 还原状态等信息
#[tauri::command]
pub async fn remove_mod(
    mod_path: String,
) -> Result<crate::core::mod_manager::RemoveModResult, String> {
    let path = PathBuf::from(mod_path);

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::core::mod_manager::RemoveModResult, String> {
            crate::core::mod_manager::remove_mod(&path).map_err(|e| e.to_string())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(result)
}

/// 重命名模组
#[tauri::command]
pub async fn rename_mod(mod_path: String, new_name: String) -> Result<PathBuf, String> {
    let path = PathBuf::from(&mod_path);

    let new_path = tauri::async_runtime::spawn_blocking(move || -> Result<PathBuf, String> {
        if !path.exists() {
            return Err("Mod path does not exist".to_string());
        }
        let parent = path
            .parent()
            .ok_or_else(|| "Invalid mod path".to_string())?;
        let parent_name = parent
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // NRMM 逻辑：group_xx 普通分组下的模组，重命名仅修改 modname 标记文件（展示名），文件夹名保持不变
        if mod_scanner::is_group_xx_dir(&parent_name) {
            let modname_path = path.join("modname");
            fs::write(&modname_path, &new_name)
                .map_err(|e| format!("Failed to write modname file: {}", e))?;
            return Ok(path);
        }

        // 其余情况（互斥组等）：重命名文件夹
        let dir_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let is_disabled = dir_name.to_uppercase().starts_with("DISABLED");
        let final_name = if is_disabled {
            format!("{}{}", constants::DISABLED_PREFIX, new_name)
        } else {
            new_name.clone()
        };
        let new_path = parent.join(final_name);
        fs::rename(&path, &new_path).map_err(|e| e.to_string())?;
        Ok(new_path)
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(new_path)
}

/// 重命名分组
///
/// 根据分组类型采用不同策略：
/// - group_xx（is_group_xx=true）：修改 groupname 文件内容（无则创建）
/// - 非group（is_group_xx=false）：重命名目录
///
/// 路径不存在时返回异常
#[tauri::command]
pub async fn rename_group(
    group_path: String,
    new_name: String,
    is_group_xx: bool,
) -> Result<PathBuf, String> {
    let path = PathBuf::from(&group_path);

    let new_path = tauri::async_runtime::spawn_blocking(move || -> Result<PathBuf, String> {
        if !path.exists() {
            return Err("Group path does not exist".to_string());
        }

        if is_group_xx {
            let groupname_path = path.join("groupname");
            fs::write(&groupname_path, &new_name)
                .map_err(|e| format!("Failed to write groupname file: {}", e))?;
            Ok(groupname_path)
        } else {
            let parent = path
                .parent()
                .ok_or_else(|| "Invalid group path".to_string())?;
            let target = parent.join(&new_name);
            if target.exists() {
                return Err(format!(
                    "Target directory already exists: {}",
                    target.display()
                ));
            }
            fs::rename(&path, &target).map_err(|e| e.to_string())?;
            Ok(target)
        }
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(new_path)
}

/// 切换模组启用/禁用状态（支持互斥组）
#[tauri::command]
pub async fn toggle_mod_disabled(
    mod_path: String,
    enable: bool,
    is_mutex: bool,
) -> Result<(), String> {
    let path = PathBuf::from(&mod_path);

    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        if is_mutex {
            if enable {
                mod_manager::enable_mutex_mod(&path).map_err(|e| e.to_string())
            } else {
                mod_manager::disable_mutex_mod(&path).map_err(|e| e.to_string())
            }
        } else {
            mod_manager::toggle_mod(&path, enable).map_err(|e| e.to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(())
}

/// 切换模组收藏状态
#[tauri::command]
pub async fn toggle_favorite(mod_path: String) -> Result<bool, String> {
    let path = PathBuf::from(&mod_path);

    let is_fav = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
        let fav_path = path.join(constants::FAV_MARKER);
        if fav_path.exists() {
            fs::remove_file(&fav_path).map_err(|e| e.to_string())?;
            Ok(false)
        } else {
            fs::write(&fav_path, "").map_err(|e| e.to_string())?;
            Ok(true)
        }
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(is_fav)
}

#[tauri::command]
pub fn is_favorite(mod_path: String) -> bool {
    PathBuf::from(mod_path).join(constants::FAV_MARKER).exists()
}

/// 错误前缀标识：路径不存在，前端捕获后需执行清除缓存+重读模组
const ERR_PREFIX_PATH_NOT_FOUND: &str = "[PATH_NOT_FOUND]";

#[tauri::command]
pub fn open_mod_folder(mod_path: String) -> Result<(), String> {
    let path = PathBuf::from(&mod_path);
    if !path.exists() {
        // 路径不存在 → 清除后端模组缓存，返回带标识的错误通知前端刷新数据
        let mut cache = mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
        drop(cache);
        log::warn!("Open mod folder: path not found, invalidated cache and requesting frontend refresh: {:?}", path);
        return Err(format!(
            "{}{}",
            ERR_PREFIX_PATH_NOT_FOUND, "Path does not exist"
        ));
    }
    open_path_in_explorer(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_group_folder(group_path: String) -> Result<(), String> {
    let path = PathBuf::from(&group_path);
    if !path.exists() {
        // 路径不存在 → 清除后端模组缓存，返回带标识的错误通知前端刷新数据
        let mut cache = mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
        drop(cache);
        log::warn!("Open group folder: path not found, invalidated cache and requesting frontend refresh: {:?}", path);
        return Err(format!(
            "{}{}",
            ERR_PREFIX_PATH_NOT_FOUND, "Path does not exist"
        ));
    }
    open_path_in_explorer(&path).map_err(|e| e.to_string())
}

/// 恢复所有INI文件备份
#[tauri::command]
pub async fn restore_all_inis(mods_path: String) -> Result<mod_manager::RestoredCount, String> {
    let path = PathBuf::from(&mods_path);

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<mod_manager::RestoredCount, String> {
            mod_manager::restore_all_inis(&path).map_err(|e| e.to_string())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(result)
}

/// 还原模组 INI 到管理前状态（还原区功能）
///
/// 对用户拖拽/选择的目录递归清理 NRMM 管理器注入的专属行，
/// 使模组无需模组管理器即可在 3Dmigoto 中使用。
///
/// # 参数
/// - `path`: 要还原的模组文件夹路径
///
/// # 返回值
/// 返回 `RestoreManagedResult`，包含路径、INI 处理数、失败数与成功标志。
///
/// # 错误
/// - 路径不存在（带 `[PATH_NOT_FOUND]` 前缀，供前端识别）
#[tauri::command]
pub async fn restore_managed_folder(
    path: String,
) -> Result<mod_manager::RestoreManagedResult, String> {
    let path_buf = PathBuf::from(&path);
    if !path_buf.exists() {
        log::warn!("Restore managed folder: path not found: {:?}", path_buf);
        return Err(format!(
            "{}{}",
            ERR_PREFIX_PATH_NOT_FOUND, "Path does not exist"
        ));
    }

    let result =
        tauri::async_runtime::spawn_blocking(move || -> mod_manager::RestoreManagedResult {
            let (ini_count, failed_count) = mod_manager::restore_managed_mod(&path_buf);
            mod_manager::RestoreManagedResult {
                path: path_buf,
                ini_count,
                failed_count,
                success: failed_count == 0,
            }
        })
        .await
        .map_err(|e| e.to_string())?;

    Ok(result)
}

/// Save Mod Customizations：保存用户自定义INI设置到d3dx_user.ini
#[tauri::command]
pub async fn save_customizations(
    game: String,
    mods_path: String,
) -> Result<mod_manager::SaveCustomizationsResult, String> {
    let game = parse_game(&game)?;
    let path = PathBuf::from(&mods_path);

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<mod_manager::SaveCustomizationsResult, String> {
            mod_manager::save_customizations(&path, game).map_err(|e| e.to_string())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    Ok(result)
}

/// 批量切换模组启用/禁用状态
#[tauri::command]
pub async fn batch_toggle_mods(
    mod_paths: Vec<String>,
    enable: bool,
    is_mutex: bool,
) -> Result<u32, String> {
    let count = tauri::async_runtime::spawn_blocking(move || -> Result<u32, String> {
        mod_manager::batch_toggle_mods(&mod_paths, enable, is_mutex).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(count)
}

/// 模拟 F10 按键（3Dmigoto 重载快捷键）
///
/// 与 NRMM 的 simulateKeyF10() 对齐，用于 update_mod_data 完成后
/// 需要手动重载 3Dmigoto 时向游戏发送 F10 按键。
/// 若传入 game 参数则尝试定向发送到目标游戏窗口，否则全局发送。
///
/// 若用户在设置中关闭了 `simulate_key_on_selection`（触发选择功能），
/// 则跳过 F10 发送，避免关闭后仍触发刷新的异常。
#[tauri::command]
pub async fn simulate_f10(game: Option<String>) -> Result<(), String> {
    let settings = settings_store::get_settings();
    sel_dbg!(
        "mod_commands",
        "simulate_f10",
        "入口 | game={:?} simulate_key_on_selection={}",
        game,
        settings.simulate_key_on_selection
    );
    if !settings.simulate_key_on_selection {
        log::debug!("[simulate_f10] simulate_key_on_selection=false, 跳过 F10 发送");
        sel_dbg!(
            "mod_commands",
            "simulate_f10",
            "已跳过 F10 发送（simulate_key_on_selection=false）"
        );
        return Ok(());
    }
    let game_for_log = game.clone();

    let mut simulator = crate::platform::get_key_simulator();
    if let Some(game_str) = game {
        if let Ok(game_enum) = parse_game(&game_str) {
            let process_names = game_enum.process_names();
            if let Some(first_pn) = process_names.first() {
                let _ = simulator.set_target_process(first_pn);
            }
        }
    }
    simulator.simulate_f10().map_err(|e| e.to_string())?;
    sel_dbg!(
        "mod_commands",
        "simulate_f10",
        "已发送 F10 重载按键（目标游戏={:?}）",
        game_for_log
    );
    Ok(())
}

fn parse_game(game: &str) -> Result<TargetGame, String> {
    match game.to_lowercase().as_str() {
        "genshinimpact" | "genshin" | "gi" => Ok(TargetGame::GenshinImpact),
        "honkaistarrail" | "starrail" | "hsr" => Ok(TargetGame::HonkaiStarRail),
        "wuwa" | "wutheringwaves" | "wuthering waves" => Ok(TargetGame::Wuwa),
        "zzz" | "zenlesszonezero" => Ok(TargetGame::ZZZ),
        "honkaiimpact3rd" | "honkaiimpact3" | "hi3" => Ok(TargetGame::HonkaiImpact3rd),
        "arknightsendfield" | "endfield" | "af" | "arknights endfield" => {
            Ok(TargetGame::ArknightsEndfield)
        }
        _ => Err(format!("Unknown game: {}", game)),
    }
}

fn trash_delete(path: &Path) -> Result<()> {
    match trash::delete(path) {
        Ok(_) => Ok(()),
        Err(e) => {
            log::warn!(
                "Failed to move to trash: {}, falling back to permanent delete",
                e
            );
            if path.is_dir() {
                fs::remove_dir_all(path)?;
            } else {
                fs::remove_file(path)?;
            }
            Ok(())
        }
    }
}

fn open_path_in_explorer(path: &Path) -> Result<()> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| anyhow::anyhow!(e.to_string()))
}

/// 校验子文件夹名称合法性
///
/// # 参数
/// - parent_path: 父目录路径
/// - folder_name: 待校验的文件夹名称
///
/// # 返回值
/// - Ok((sanitized_name, true)): 名称合法，返回清理后的名称
/// - Ok((sanitized_name, false)): 名称不合法，sanitized_name为清理后名称，附带错误信息
/// - Err: 校验过程出错
#[tauri::command]
pub fn validate_subfolder_name(
    parent_path: String,
    folder_name: String,
) -> Result<(String, bool, String), String> {
    let parent = PathBuf::from(&parent_path);

    // 1. 清理名称：trim首尾空白
    let sanitized = folder_name.trim().to_string();

    // 2. 空名检查
    if sanitized.is_empty() {
        return Ok((sanitized, false, "分组名不能为空".to_string()));
    }

    // 3. 通用禁止名称：. 和 ..
    if sanitized == "." || sanitized == ".." {
        return Ok((
            sanitized,
            false,
            "文件夹名称不能为 \".\" 或 \"..\"".to_string(),
        ));
    }

    // 4. 平台非法字符检查
    #[cfg(target_os = "windows")]
    {
        let illegal_chars: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
        let mut found_chars = Vec::new();
        for c in illegal_chars {
            if sanitized.contains(*c) {
                found_chars.push(*c);
            }
        }
        // 检查控制字符 0x00-0x1F
        for c in sanitized.chars() {
            if (c as u32) < 0x20 {
                found_chars.push(' ');
                break;
            }
        }
        if !found_chars.is_empty() {
            let chars_str: String = found_chars.iter().collect();
            return Ok((
                sanitized,
                false,
                format!("目录名包含非法字符: {}", chars_str),
            ));
        }

        // Windows: 末尾不能是点或空格
        if sanitized.ends_with('.') || sanitized.ends_with(' ') {
            return Ok((
                sanitized.trim_end_matches(&['.', ' '][..]).to_string(),
                false,
                "文件夹名称末尾不能是点或空格".to_string(),
            ));
        }

        // Windows保留名称检查（不区分大小写）
        let upper_name = sanitized.to_uppercase();
        let reserved_names = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        for name in &reserved_names {
            if upper_name == *name {
                return Ok((
                    sanitized,
                    false,
                    "该名称为系统保留名称，请换一个名称".to_string(),
                ));
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if sanitized.contains('/') || sanitized.contains('\0') {
            return Ok((sanitized, false, "目录名包含非法字符: /".to_string()));
        }
    }

    // 5. 父目录存在检查
    if !parent.exists() {
        return Ok((sanitized, false, "父目录不存在，请刷新后重试".to_string()));
    }

    // 6. 目标路径是否已存在
    let target_path = parent.join(&sanitized);
    if target_path.exists() {
        return Ok((
            sanitized,
            false,
            "该名称的文件夹已存在，请换一个名称".to_string(),
        ));
    }

    // 7. 路径长度检查（Windows MAX_PATH 限制约260字符）
    #[cfg(target_os = "windows")]
    {
        let path_len = target_path.as_os_str().len();
        if path_len > 240 {
            return Ok((sanitized, false, "文件夹路径过长，请缩短名称".to_string()));
        }
    }

    Ok((sanitized, true, String::new()))
}

/// 创建子文件夹
///
/// # 参数
/// - parent_path: 父目录路径
/// - folder_name: 文件夹名称（应先通过validate_subfolder_name校验）
///
/// # 返回值
/// - Ok(()): 创建成功
/// - Err: 错误信息（用户友好描述）
#[tauri::command]
pub fn create_subfolder(parent_path: String, folder_name: String) -> Result<(), String> {
    let parent = PathBuf::from(&parent_path);

    // 二次校验
    let (sanitized, valid, err_msg) = validate_subfolder_name(parent_path, folder_name)?;
    if !valid {
        return Err(err_msg);
    }

    let target_path = parent.join(&sanitized);

    // 创建目录（只创建最后一级，不递归创建父目录）
    match fs::create_dir(&target_path) {
        Ok(_) => Ok(()),
        Err(e) => {
            let err_msg = match e.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    "没有权限在此位置创建文件夹，请检查权限设置".to_string()
                }
                std::io::ErrorKind::AlreadyExists => {
                    "该名称的文件夹已存在，请换一个名称".to_string()
                }
                std::io::ErrorKind::InvalidFilename => "文件夹名称包含不允许的字符".to_string(),
                _ => "创建文件夹失败，请检查路径是否可访问".to_string(),
            };
            log::error!("Failed to create subfolder {:?}: {}", target_path, e);
            Err(err_msg)
        }
    }
}

/// 禁用指定分组下所有一级模组（不含 .ini 的子分组目录不处理）
#[tauri::command]
pub async fn disable_all_mods_in_group(group_path: String) -> Result<u32, String> {
    let path = PathBuf::from(group_path);

    let count = tauri::async_runtime::spawn_blocking(move || -> Result<u32, String> {
        crate::core::mod_manager::disable_all_mods_in_group(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(count)
}

/// 启用指定分组下所有一级禁用模组（不含 .ini 的子分组目录不处理）
#[tauri::command]
pub async fn enable_all_mods_in_group(group_path: String) -> Result<u32, String> {
    let path = PathBuf::from(group_path);

    let count = tauri::async_runtime::spawn_blocking(move || -> Result<u32, String> {
        crate::core::mod_manager::enable_all_mods_in_group(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(count)
}

/// 移除分组（NRMM 对齐：移至 DISABLED_MANAGED_REMOVED；非group先移子分组到父级再移除）
///
/// 顶层函数：负责从设置数据获取 Mods 根目录、在线程池中执行底层移动、
/// 处理错误并记录日志。具体「哪些目录移动到哪里」由底层编排层 `remove_group_ex` 决定。
#[tauri::command]
pub async fn remove_group_ex(group_path: String, is_group_xx: bool) -> Result<(), String> {
    let path = PathBuf::from(group_path);
    log::info!(
        "[remove_group_ex] start | group_path={:?} is_group_xx={}",
        path,
        is_group_xx
    );

    // 从设置数据定位 Mods 根目录（与 _MANAGED_ 同级）
    let settings = settings_store::get_settings();
    let mods_root = settings
        .game_mods_path
        .values()
        .map(PathBuf::from)
        .find(|p| path.starts_with(p))
        .ok_or_else(|| {
            log::error!(
                "[remove_group_ex] failed to locate Mods root from settings for {:?}",
                path
            );
            "无法从设置数据定位 Mods 根目录".to_string()
        })?;
    log::debug!("[remove_group_ex] mods_root={:?}", mods_root);

    let path_for_log = path.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        crate::core::mod_manager::remove_group_ex(&path, is_group_xx, &mods_root)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    log::info!(
        "[remove_group_ex] completed | group_path={:?}",
        path_for_log
    );
    Ok(())
}

// =========================================================================
// 单元测试：update_mod_data 逻辑整合于此（项目内测试）
//
// 设计要点：
// - 修复原外部集成测试（tests/update_mod_data_test.rs）的两个缺陷：
//   1) 使用 tempfile::TempDir 自动销毁 → 测试后磁盘无残留，无法观测"目录是否变化"；
//   2) 基线映射颠倒（input=NRMM-test 已处理产物，baseline=NRMM-Rust-test 原始输入）
//      → 实际上从不验证 update_mod_data 是否真正生成了 managed 文件。
// - 本模块使用【持久化运行时目录】tests/update_mod_data_runtime/，测试执行后可直接在
//   磁盘查看 _MANAGED_ 产物，验证目录确实发生变化。
// - 数据集映射保持正确：NRMM-Rust-test = 输入（原始），NRMM-test = 预期输出基线。
// - 若测试中发现异常（目录未变化 / 与基线不一致），应直接修正项目源码
//   （mod_manager.rs / ini_handler.rs 等），而非仅修改此处测试脚手架。
// =========================================================================

#[cfg(test)]
mod tests {
    use crate::core::constants;
    use crate::core::mod_manager;
    use crate::models::enums::TargetGame;
    use crate::models::settings::AppSettings;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// 持久化运行时目录：cargo test 后可直接在磁盘查看产物，验证目录确实变化
    const RUNTIME_DIR: &str = "tests/update_mod_data_runtime";

    /// 输入数据集（原始，未经 update_mod_data 处理）
    fn input_dataset() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("NRMM-Rust-test");
        if !manifest_dir.exists() {
            panic!(
                "Input dataset directory does not exist: {}",
                manifest_dir.display()
            );
        }
        manifest_dir
    }

    /// 预期输出基线（Dart 原版 NRMM 实测产物）
    fn baseline_managed() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("NRMM-test")
            .join("Mods")
            .join("_MANAGED_")
    }

    fn read_file(p: &Path) -> String {
        fs::read_to_string(p).unwrap_or_default()
    }

    fn normalize_paths(s: &str) -> String {
        s.replace('\\', "/")
    }

    /// 规范化 INI 内容，忽略绝对路径 / 注释噪声 / $managed_slot_id 值，保留结构
    fn normalize_ini_content(s: &str) -> String {
        let mut out = String::new();
        for line in s.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lower = trimmed.to_lowercase();
            if lower.starts_with("ini_path_absolute") {
                continue;
            }
            if lower.starts_with("global $managed_slot_id") {
                out.push_str("global $managed_slot_id = <SLOT>\n");
                continue;
            }
            if trimmed.starts_with(';') {
                if trimmed.contains("NRMM")
                    || trimmed.contains("DISABLED")
                    || trimmed.starts_with("; \"")
                {
                    out.push_str(line.trim_end());
                    out.push('\n');
                }
                continue;
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out.replace("\r\n", "\n")
    }

    /// 递归复制目录
    fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
        // 确保源目录存在且为目录
        if !src.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} is not a directory", src.display()),
            ));
        }

        for entry in walkdir::WalkDir::new(src).follow_links(false) {
            let entry =
                entry.map_err(|e| std::io::Error::other(e.to_string()))?;

            // ✅ 核心：计算相对于源根的路径
            let relative: PathBuf = entry
                .path()
                .strip_prefix(src)
                .map_err(std::io::Error::other)?
                .to_path_buf();

            // 跳过根目录本身（relative == ""）
            if relative.as_os_str().is_empty() {
                continue;
            }

            let target = dst.join(&relative);

            if entry.file_type().is_dir() {
                fs::create_dir_all(&target)?;
            } else if entry.file_type().is_symlink() {
                // 符号链接：读取原始目标并重建
                #[cfg(unix)]
                {
                    let link_target = fs::read_link(entry.path())?;
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let _ = fs::remove_file(&target);
                    std::os::unix::fs::symlink(link_target, &target)?;
                }
                #[cfg(windows)]
                {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(entry.path(), &target)?;
                }
            } else {
                // 普通文件：确保父目录存在后复制
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(entry.path(), &target)?;
            }

            // println!("copied: {}", relative.display());
        }

        Ok(())
    }

    /// 将输入数据集还原到持久化运行时目录。
    /// 路径约定（与原版 NRMM / 集成测试一致）：
    /// - d3dx.ini → <runtime>/d3dx.ini（游戏根目录）
    /// - Mods/_MANAGED_ → <runtime>/Mods/_MANAGED_/
    /// - 其余目录/文件 → <runtime>/Mods/<name>
    ///
    /// 返回 game_mods_path（必须以 "Mods" 结尾）
    fn restore_dataset_to(runtime: &Path) -> PathBuf {
        let _ = fs::remove_dir_all(runtime);
        fs::create_dir_all(runtime).expect("创建运行时根目录失败");

        let src_root = input_dataset();
        copy_dir(&src_root, runtime).unwrap_or_else(|_| panic!("复制 {:?} 失败", src_root));

        runtime.join("Mods")
    }

    /// 验证测试目录确实发生变化：运行后必须生成以下文件（运行前不存在），
    /// 且关键文件与 NRMM-test 基线语义一致。
    fn assert_managed_outputs(managed: &Path) {
        // 1) 目录变化验证：以下文件在运行前不存在，运行后必须存在且非空
        for f in [
            "manager_group.ini",
            "nrmm_include.ini",
            "nrmm_keypress.txt",
            "selectedindex",
        ] {
            let p = managed.join(f);
            assert!(p.exists(), "[目录变化] 缺失生成文件: {}", f);
            let c = read_file(&p);
            assert!(!c.trim().is_empty(), "[目录变化] 生成文件为空: {}", f);
        }
        // group_1 下由 update_mod_data 生成的分组文件
        let g1_ini = managed.join("group_1").join("group_1.ini");
        let g1_sel = managed.join("group_1").join("selectedindex");
        assert!(g1_ini.exists(), "[目录变化] group_1.ini 应被生成");
        assert!(g1_sel.exists(), "[目录变化] group_1/selectedindex 应被生成");

        // 2) 与 NRMM-test 基线语义比对（已知对齐项）
        let base = baseline_managed();
        for f in ["nrmm_include.ini", "nrmm_keypress.txt"] {
            let a = normalize_ini_content(&normalize_paths(&read_file(&managed.join(f))));
            let e = normalize_ini_content(&normalize_paths(&read_file(&base.join(f))));
            assert_eq!(a, e, "[基线比对] {} 与 NRMM-test 不一致", f);
        }
        let g1 = normalize_ini_content(&normalize_paths(&read_file(&g1_ini)));
        let g1b = normalize_ini_content(&normalize_paths(&read_file(
            &base.join("group_1").join("group_1.ini"),
        )));
        assert_eq!(g1, g1b, "[基线比对] group_1.ini 与 NRMM-test 不一致");

        let sel = read_file(&managed.join("selectedindex")).trim().to_string();
        assert_eq!(sel, "0", "[基线比对] 根 selectedindex 应为 0");
    }

    /// 主测试：构造参数 → 调用 update_mod_data → 验证目录变化 + 基线一致
    ///
    /// 使用持久化运行时目录，测试执行后可直接在磁盘检查
    /// `src-tauri/tests/update_mod_data_runtime/` 下的 _MANAGED_ 产物。
    #[test]
    fn update_mod_data_test() {
        println!("\n=== update_mod_data_test（集成到 mod_commands.rs）===");
        let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(RUNTIME_DIR);
        let game_mods_path = restore_dataset_to(&runtime);
        let managed = game_mods_path.join(constants::MANAGED_FOLDER);

        // 运行前快照：输入数据集仅有 group_1 目录，尚无生成文件
        assert!(
            !managed.join("manager_group.ini").exists(),
            "运行前不应存在 manager_group.ini"
        );
        assert!(
            !managed.join("nrmm_include.ini").exists(),
            "运行前不应存在 nrmm_include.ini"
        );

        // 构造参数并调用 update_mod_data（核心函数，命令层仅为其异步包装）
        let settings = AppSettings::default();
        // 注意：NRMM-test 基线由 NRMM 以 Zenless（nrmm_name = "Zenless_Zone_Zero"）生成，
        // keypress 模板 `{game}` 替换依赖游戏标识，故此处必须用同款游戏以保证 parity 比对一致。
        let result =
            mod_manager::update_mod_data(TargetGame::ZZZ, &game_mods_path, &settings)
                .expect("update_mod_data 执行失败");

        println!(
            "  [result] enabled={} disabled={} processed={} errors={} groups={}",
            result.enabled_mods,
            result.disabled_mods,
            result.processed_mods,
            result.errors.len(),
            result.total_groups
        );

        // 验证返回结果合理
        assert!(
            result.processed_mods >= 1,
            "至少应处理 1 个模组，实际 {}",
            result.processed_mods
        );

        // 验证测试目录确实发生变化（生成产物存在且与基线一致）
        assert_managed_outputs(&managed);

        println!("  [PASS] update_mod_data_test");
        println!("  运行时产物目录: {:?}", runtime);
        println!("=== update_mod_data_test 完成 ===\n");
    }

    /// 场景：空 Mods 目录（无模组）
    #[test]
    fn update_mod_data_test_empty_mods() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mods = tmp.path().join("Mods");
        fs::create_dir_all(&mods).unwrap();
        let managed = mods.join(constants::MANAGED_FOLDER);
        fs::create_dir_all(&managed).unwrap();

        let settings = AppSettings::default();
        let result = mod_manager::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
            .expect("update_mod_data 失败");
        assert_eq!(result.processed_mods, 0, "空目录不应处理任何模组");
        // 仍应生成 managed 基础文件（目录发生变化）
        assert!(managed.join("nrmm_include.ini").exists());
        assert!(managed.join("nrmm_keypress.txt").exists());
        assert!(managed.join("manager_group.ini").exists());
        assert!(managed.join("selectedindex").exists());
        println!("[PASS] update_mod_data_test_empty_mods");
    }

    /// 场景：DISABLED 模组被跳过
    #[test]
    fn update_mod_data_test_disabled_mod_skipped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mods = tmp.path().join("Mods");
        fs::create_dir_all(&mods).unwrap();
        let managed = mods.join(constants::MANAGED_FOLDER);
        fs::create_dir_all(&managed).unwrap();

        let group_dir = managed.join("group_1");
        fs::create_dir_all(&group_dir).unwrap();
        fs::write(
            group_dir.join("group_1.ini"),
            ";template\n[Constants]\nglobal $group_id = 1\n",
        )
        .unwrap();

        // 启用模组
        let enabled = group_dir.join("EnabledMod");
        fs::create_dir_all(&enabled).unwrap();
        fs::write(
            enabled.join("mod.ini"),
            "[TextureOverride]\nhash = 0xAAAA\n",
        )
        .unwrap();

        // 禁用模组（DISABLED 前缀）
        let disabled = group_dir.join("DISABLED_TestMod");
        fs::create_dir_all(&disabled).unwrap();
        fs::write(
            disabled.join("mod.ini"),
            "[TextureOverride]\nhash = 0xBBBB\n",
        )
        .unwrap();

        let settings = AppSettings::default();
        let result = mod_manager::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
            .expect("update_mod_data 失败");
        assert_eq!(result.enabled_mods, 1, "应只有 1 个启用模组");
        assert_eq!(result.disabled_mods, 1, "应有 1 个禁用模组");
        assert_eq!(result.processed_mods, 1, "应仅处理启用的模组");
        println!("[PASS] update_mod_data_test_disabled_mod_skipped");
    }

    /// 场景：多分组更新
    #[test]
    fn update_mod_data_test_multiple_groups() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mods = tmp.path().join("Mods");
        fs::create_dir_all(&mods).unwrap();
        let managed = mods.join(constants::MANAGED_FOLDER);
        fs::create_dir_all(&managed).unwrap();

        for g in 1..=2u32 {
            let gd = managed.join(format!("group_{}", g));
            fs::create_dir_all(&gd).unwrap();
            fs::write(
                gd.join(format!("group_{}.ini", g)),
                format!(";t\n[Constants]\nglobal $group_id = {}\n", g),
            )
            .unwrap();
            for m in 1..=2u32 {
                let md = gd.join(format!("Mod_{}", m));
                fs::create_dir_all(&md).unwrap();
                fs::write(
                    md.join("mod.ini"),
                    format!(
                        "[TextureOverride_G{}_M{}]\nhash = 0x{:08X}\n",
                        g,
                        m,
                        0x12345678 + g * 256 + m * 16
                    ),
                )
                .unwrap();
            }
        }

        let settings = AppSettings::default();
        let result = mod_manager::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
            .expect("update_mod_data 失败");
        assert!(result.total_groups >= 2, "应检测到至少 2 个分组");
        assert!(result.processed_mods >= 4, "应处理至少 4 个模组");
        assert!(managed.join("group_1/group_1.ini").exists());
        assert!(managed.join("group_2/group_2.ini").exists());
        println!("[PASS] update_mod_data_test_multiple_groups");
    }

    /// 场景：幂等性（连续两次更新结果一致）
    #[test]
    fn update_mod_data_test_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mods = tmp.path().join("Mods");
        fs::create_dir_all(&mods).unwrap();
        let managed = mods.join(constants::MANAGED_FOLDER);
        fs::create_dir_all(&managed).unwrap();

        let group_dir = managed.join("group_1");
        fs::create_dir_all(&group_dir).unwrap();
        fs::write(
            group_dir.join("group_1.ini"),
            ";template\n[Constants]\nglobal $group_id = 1\n",
        )
        .unwrap();
        let en = group_dir.join("EnabledMod");
        fs::create_dir_all(&en).unwrap();
        fs::write(en.join("mod.ini"), "[TextureOverride]\nhash = 0xAAAA\n").unwrap();

        let settings = AppSettings::default();
        let r1 = mod_manager::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
            .expect("第一次 update_mod_data 失败");
        let r2 = mod_manager::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
            .expect("第二次 update_mod_data 失败");
        assert_eq!(
            r1.processed_mods, r2.processed_mods,
            "两次更新 processed_mods 应一致"
        );
        assert_eq!(
            r1.enabled_mods, r2.enabled_mods,
            "两次更新 enabled_mods 应一致"
        );
        println!("[PASS] update_mod_data_test_idempotent");
    }
}
