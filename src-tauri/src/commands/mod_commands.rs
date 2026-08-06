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

use crate::core::mod_manager;
use crate::core::mod_scanner;
use crate::core::constants;
use crate::core::mod_cache;
use crate::models::enums::TargetGame;
use crate::config::settings_store;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::fs;
use std::sync::{Mutex, LazyLock};
use std::collections::HashMap;
use std::time::{Instant, Duration};

static SELECTION_DEBOUNCE: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

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
    log::debug!("[commands::mod_commands] [get_mods] game={} mods_path={}", game, mods_path);
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
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<mod_scanner::ScanResult, String> {
        mod_scanner::scan_mods_light(&scan_path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    let elapsed = start.elapsed();
    log::info!("Light scan completed in {}ms, {} mods, {} groups",
        elapsed.as_millis(), result.total_mods_count, result.groups.len());

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.set(game, &mods_path, result.clone());
    }

    Ok(result)
}

#[tauri::command]
pub fn check_mods_path_status(game: String, mods_path: String) -> Result<crate::models::enums::ModsPathStatus, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    Ok(mod_scanner::check_mods_path(game, &mods_path))
}

/// 刷新模组列表（轻量扫描，更新缓存）
#[tauri::command]
pub async fn refresh_mods(game: String, mods_path: String) -> Result<mod_scanner::ScanResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    log::info!("[refresh_mods] Scanning light...");
    let start = std::time::Instant::now();

    let scan_path = mods_path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<mod_scanner::ScanResult, String> {
        mod_scanner::scan_mods_light(&scan_path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    let elapsed = start.elapsed();
    log::info!("Light refresh completed in {}ms, {} mods, {} groups",
        elapsed.as_millis(), result.total_mods_count, result.groups.len());

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.set(game, &mods_path, result.clone());
    }

    Ok(result)
}

/// 重量级更新模组数据（仅用户按钮触发）
#[tauri::command]
pub async fn update_mod_data(game: String, mods_path: String) -> Result<mod_manager::UpdateResult, String> {
    log::debug!("[commands::mod_commands] [update_mod_data] game={} mods_path={}", game, mods_path);
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let settings = settings_store::get_settings();
    let managed_path = mods_path.join(constants::MANAGED_FOLDER);

    log::info!("[update_mod_data] Running heavy update...");
    let start = std::time::Instant::now();

    let update_path = mods_path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<mod_manager::UpdateResult, String> {
        mod_manager::update_mod_data(game, &update_path, &settings).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    let elapsed = start.elapsed();
    log::info!("Heavy update completed in {}ms, processed {} mods",
        elapsed.as_millis(), result.processed_mods);

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
pub async fn detect_hash_conflicts(mods_path: String) -> Result<mod_manager::HashConflictResult, String> {
    let mods_path = PathBuf::from(mods_path);
    log::info!("[detect_hash_conflicts] Starting full hash conflict scan: {}", mods_path.display());
    let start = std::time::Instant::now();

    let scan_path = mods_path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<mod_manager::HashConflictResult, String> {
        mod_manager::detect_hash_conflicts(&scan_path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    log::info!("[detect_hash_conflicts] Completed in {}ms, found {} conflicts",
        start.elapsed().as_millis(), result.conflicts.len());
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
    log::debug!("[commands::mod_commands] [select_mod] game={} group={} mod={} mutex={}", game, group_index, mod_index, is_mutex);
    let _start = std::time::Instant::now();
    let game_enum = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    if is_mutex {
        // Mutex 分支防抖（与非 Mutex 分支一致，防止快速重复点击触发多次互斥切换）
        {
            let mut debounce_map = SELECTION_DEBOUNCE.lock().map_err(|_| "debounce lock poisoned")?;
            if let Some(last_time) = debounce_map.get(&group_path) {
                if Instant::now() - *last_time < Duration::from_millis(500) {
                    return Err("debounced".to_string());
                }
            }
            debounce_map.insert(group_path.clone(), Instant::now());
        }

        let mod_path_buf = PathBuf::from(mod_path);
        let managed_path = mods_path.join(constants::MANAGED_FOLDER);

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
        {
            let mut debounce_map = SELECTION_DEBOUNCE.lock().map_err(|_| "debounce lock poisoned")?;
            if let Some(last_time) = debounce_map.get(&group_path) {
                if Instant::now() - *last_time < Duration::from_millis(500) {
                    return Err("debounced".to_string());
                }
            }
            debounce_map.insert(group_path.clone(), Instant::now());
        }
        // 检查设置是否允许按键模拟
        let settings = settings_store::get_settings();
        let simulate_enabled = settings.simulate_key_on_selection;
        log::debug!("[commands::mod_commands] [select_mod] simulate_key_on_selection={} mutex={}", simulate_enabled, is_mutex);

        if simulate_enabled {
            let mut simulator = crate::platform::get_key_simulator();
            let process_names = game_enum.process_names();
            for pn in process_names {
                if simulator.set_target_process(pn).is_ok() {
                    break;
                }
            }
            // 参数语义对齐NRMM：优先使用调用方传入的屏幕坐标（cursor_x/cursor_y 为实际像素时），
            // 否则 fallback 到 (mod_index, group_index) 作为虚拟坐标（3Dmigoto/xxmi 据此识别）。
            // 注意：NRMM 将 x=realModIndex y=realGroupIndex 直接传入 SetCursorPos，
            // 因此当未传像素坐标时，索引值本身即作为坐标输入。
            let result: Result<(), String> = match (cursor_x, cursor_y) {
                (Some(px), Some(py)) => {
                    let g = u32::try_from(py.max(0)).unwrap_or(group_index);
                    let m = u32::try_from(px.max(0)).unwrap_or(mod_index);
                    simulator
                        .simulate_select_full(g, m)
                        .map_err(|e| e.to_string())
                }
                _ => simulator
                    .simulate_select_full(group_index, mod_index)
                    .map_err(|e| e.to_string()),
            };
            log::debug!("[commands::mod_commands] [select_mod] simulate_result={:?}", result);
            if let Err(e) = result {
                log::warn!(
                    "select_mod: simulate_select_full failed (g={}, m={}): {}",
                    group_index,
                    mod_index,
                    e
                );
            }
        }

        let managed_path = mods_path.join(constants::MANAGED_FOLDER);

        let result = tauri::async_runtime::spawn_blocking(move || -> Result<mod_manager::UpdateResult, String> {
            mod_manager::switch_mod(game_enum, &mods_path, &settings, group_index, mod_index).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;
        log::debug!("[commands::mod_commands] [select_mod] switch_result={:?}", result);

        {
            let mut cache = crate::core::mod_cache::MOD_CACHE.write();
            cache.invalidate_by_prefix(&managed_path);
        }
        log::debug!("[commands::mod_commands] [select_mod] cache invalidated | prefix={:?}", managed_path);

        log::debug!("[commands::mod_commands] [select_mod] completed | elapsed={:?}ms | selected_mod_index={:?}", _start.elapsed().as_millis(), result.selected_mod_index);
        Ok(result)
    }
}

/// 取消选中分组内模组
#[tauri::command]
pub async fn deselect_group_mod(game: String, mods_path: String, group_index: u32) -> Result<mod_manager::UpdateResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let settings = settings_store::get_settings();
    let managed_path = mods_path.join(constants::MANAGED_FOLDER);

    let result = tauri::async_runtime::spawn_blocking(move || -> Result<mod_manager::UpdateResult, String> {
        mod_manager::deselect_group_mods(game, &mods_path, &settings, group_index).map_err(|e| e.to_string())
    })
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
pub async fn add_group(game: String, mods_path: String, group_name: Option<String>) -> Result<mod_manager::UpdateResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    let result = tauri::async_runtime::spawn_blocking(move || -> Result<mod_manager::UpdateResult, String> {
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
    })
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
pub async fn remove_mod(mod_path: String) -> Result<crate::core::mod_manager::RemoveModResult, String> {
    let path = PathBuf::from(mod_path);

    let result = tauri::async_runtime::spawn_blocking(move || -> Result<crate::core::mod_manager::RemoveModResult, String> {
        crate::core::mod_manager::remove_mod(&path).map_err(|e| e.to_string())
    })
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
        let parent = path.parent().ok_or_else(|| "Invalid mod path".to_string())?;
        let parent_name = parent.file_name().unwrap_or_default().to_string_lossy().to_string();

        // NRMM 逻辑：group_xx 普通分组下的模组，重命名仅修改 modname 标记文件（展示名），文件夹名保持不变
        if mod_scanner::is_group_xx_dir(&parent_name) {
            let modname_path = path.join("modname");
            fs::write(&modname_path, &new_name)
                .map_err(|e| format!("Failed to write modname file: {}", e))?;
            return Ok(path);
        }

        // 其余情况（互斥组等）：重命名文件夹
        let dir_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
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
pub async fn rename_group(group_path: String, new_name: String, is_group_xx: bool) -> Result<PathBuf, String> {
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
            let parent = path.parent().ok_or_else(|| "Invalid group path".to_string())?;
            let target = parent.join(&new_name);
            if target.exists() {
                return Err(format!("Target directory already exists: {}", target.display()));
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
pub async fn toggle_mod_disabled(mod_path: String, enable: bool, is_mutex: bool) -> Result<(), String> {
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
        return Err(format!("{}{}", ERR_PREFIX_PATH_NOT_FOUND, "Path does not exist"));
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
        return Err(format!("{}{}", ERR_PREFIX_PATH_NOT_FOUND, "Path does not exist"));
    }
    open_path_in_explorer(&path).map_err(|e| e.to_string())
}

/// 恢复所有INI文件备份
#[tauri::command]
pub async fn restore_all_inis(mods_path: String) -> Result<mod_manager::RestoredCount, String> {
    let path = PathBuf::from(&mods_path);

    let result = tauri::async_runtime::spawn_blocking(move || -> Result<mod_manager::RestoredCount, String> {
        mod_manager::restore_all_inis(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(result)
}

/// Save Mod Customizations：保存用户自定义INI设置到d3dx_user.ini
#[tauri::command]
pub async fn save_customizations(game: String, mods_path: String) -> Result<mod_manager::SaveCustomizationsResult, String> {
    let game = parse_game(&game)?;
    let path = PathBuf::from(&mods_path);

    let result = tauri::async_runtime::spawn_blocking(move || -> Result<mod_manager::SaveCustomizationsResult, String> {
        mod_manager::save_customizations(&path, game).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(result)
}

/// 批量切换模组启用/禁用状态
#[tauri::command]
pub async fn batch_toggle_mods(mod_paths: Vec<String>, enable: bool, is_mutex: bool) -> Result<u32, String> {
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
#[tauri::command]
pub async fn simulate_f10(game: Option<String>) -> Result<(), String> {
    let mut simulator = crate::platform::get_key_simulator();
    if let Some(game_str) = game {
        if let Ok(game_enum) = parse_game(&game_str) {
            let process_names = game_enum.process_names();
            if let Some(first_pn) = process_names.first() {
                let _ = simulator.set_target_process(first_pn);
            }
        }
    }
    simulator
        .simulate_f10()
        .map_err(|e| e.to_string())
}

fn parse_game(game: &str) -> Result<TargetGame, String> {
    match game.to_lowercase().as_str() {
        "genshinimpact" | "genshin" | "gi" => Ok(TargetGame::GenshinImpact),
        "honkaistarrail" | "starrail" | "hsr" => Ok(TargetGame::HonkaiStarRail),
        "wuwa" | "wutheringwaves" | "wuthering waves" => Ok(TargetGame::Wuwa),
        "zzz" | "zenlesszonezero" => Ok(TargetGame::ZZZ),
        "honkaiimpact3rd" | "honkaiimpact3" | "hi3" => Ok(TargetGame::HonkaiImpact3rd),
        "arknightsendfield" | "endfield" | "af" | "arknights endfield" => Ok(TargetGame::ArknightsEndfield),
        _ => Err(format!("Unknown game: {}", game)),
    }
}

fn trash_delete(path: &Path) -> Result<()> {
    match trash::delete(path) {
        Ok(_) => Ok(()),
        Err(e) => {
            log::warn!("Failed to move to trash: {}, falling back to permanent delete", e);
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
pub fn validate_subfolder_name(parent_path: String, folder_name: String) -> Result<(String, bool, String), String> {
    let parent = PathBuf::from(&parent_path);

    // 1. 清理名称：trim首尾空白
    let sanitized = folder_name.trim().to_string();

    // 2. 空名检查
    if sanitized.is_empty() {
        return Ok((sanitized, false, "分组名不能为空".to_string()));
    }

    // 3. 通用禁止名称：. 和 ..
    if sanitized == "." || sanitized == ".." {
        return Ok((sanitized, false, "文件夹名称不能为 \".\" 或 \"..\"".to_string()));
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
            return Ok((sanitized, false, format!("目录名包含非法字符: {}", chars_str)));
        }

        // Windows: 末尾不能是点或空格
        if sanitized.ends_with('.') || sanitized.ends_with(' ') {
            return Ok((sanitized.trim_end_matches(&['.', ' '][..]).to_string(), false, "文件夹名称末尾不能是点或空格".to_string()));
        }

        // Windows保留名称检查（不区分大小写）
        let upper_name = sanitized.to_uppercase();
        let reserved_names = [
            "CON", "PRN", "AUX", "NUL",
            "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
            "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        for name in &reserved_names {
            if upper_name == *name {
                return Ok((sanitized, false, "该名称为系统保留名称，请换一个名称".to_string()));
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
        return Ok((sanitized, false, "该名称的文件夹已存在，请换一个名称".to_string()));
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
                std::io::ErrorKind::PermissionDenied => "没有权限在此位置创建文件夹，请检查权限设置".to_string(),
                std::io::ErrorKind::AlreadyExists => "该名称的文件夹已存在，请换一个名称".to_string(),
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
        crate::core::mod_manager::disable_all_mods_in_group(&path)
            .map_err(|e| e.to_string())
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
        crate::core::mod_manager::enable_all_mods_in_group(&path)
            .map_err(|e| e.to_string())
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
#[tauri::command]
pub async fn remove_group_ex(group_path: String, is_group_xx: bool) -> Result<(), String> {
    let path = PathBuf::from(group_path);

    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        crate::core::mod_manager::remove_group_ex(&path, is_group_xx)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(())
}
