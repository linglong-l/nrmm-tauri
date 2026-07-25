//! Tauri 命令模块
//!
//! 该模块定义了所有暴露给前端的 Tauri 命令（`#[tauri::command]`）。
//! 前端通过 `invoke('command_name', { ... })` 调用这些命令，实现前后端通信。
//!
//! 命令分类：
//! - **模组管理**：load_mods、refresh_mods、update_mod_data 等
//! - **分组与模组操作**：add_group、remove_group、toggle_mod_disabled 等
//! - **INI 处理**：load_ini、save_ini、check_ini_syntax 等
//! - **文件监视**：start_file_watcher、stop_file_watcher 等
//! - **快捷键管理**：register_hotkey、unregister_hotkey 等
//! - **窗口管理**：show_window、toggle_window、set_window_size 等
//! - **托盘管理**：setup_tray、update_tray_tooltip
//! - **进程检测**：is_process_running、get_foreground_game 等
//! - **设置管理**：get_settings、save_settings、reset_settings
//! - **云端数据**：get_cloud_data、fetch_cloud_data、sync_cloud_data
//! - **输入模拟**：simulate_key_press、simulate_mouse_move 等
//!
//! 错误处理约定：所有命令返回 `Result<T, String>`，将底层错误转换为字符串返回给前端。

use std::collections::HashMap;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::mod_manager::{HashConflictReport, ModGroupData, ModsPathStatus, UpdateModDataResult};
use crate::state::AppState;
use crate::task_queue::TaskQueueError;

/// 加载模组数据。
/// 若指定了 `game` 参数则按该游戏加载（覆盖设置中的 target_game）；
/// 未指定则从全局设置中读取当前目标游戏。
/// 通过 `TaskQueue` 保证同一时刻只有一个 `load_mods` 任务在执行，避免并发冲突。
///
/// 参数：
/// - `state`: 应用全局状态（包含设置、模组管理器、任务队列等）。
/// - `game`: 可选的目标游戏字符串（前端枚举值，带下划线，如 "Wuthering_Waves"）。
///
/// 返回：分组数据列表。
/// 错误：任务已在运行时返回 `"Task 'load_mods' is already running"`；
///       扫描失败时返回底层错误信息。
#[tauri::command]
pub async fn load_mods(
    state: State<'_, AppState>,
    game: Option<String>,
) -> Result<Vec<ModGroupData>, String> {
    // 记录请求开始时间（用于计算耗时）
    let start_time = std::time::Instant::now();

    // 记录请求参数
    log::debug!("[load_mods] Request received - game: {:?}", game);

    let mut settings = state.settings.read().clone();
    let target_game_before = settings.target_game;

    if let Some(ref g) = game {
        use crate::process::TargetGame;
        let target = match g.as_str() {
            "Wuthering_Waves" | "WutheringWaves" => TargetGame::WutheringWaves,
            "Genshin_Impact" | "GenshinImpact" => TargetGame::GenshinImpact,
            "Honkai_Star_Rail" | "HonkaiStarRail" => TargetGame::HonkaiStarRail,
            "Zenless_Zone_Zero" | "ZenlessZoneZero" => TargetGame::ZenlessZoneZero,
            "Arknights_Endfield" | "ArknightsEndfield" => TargetGame::ArknightsEndfield,
            e => {
                log::error!(
                    "[load_mods] Unknown game string: {:?}, defaulting to None",
                    e
                );
                Err("Unknown game string")?;
                TargetGame::None
            }
        };
        settings.target_game = target;
    }

    // 记录设置信息
    log::debug!(
        "[load_mods] Settings - target_game: {:?} -> {:?}",
        target_game_before,
        settings.target_game
    );

    // 克隆参数以满足 'static 生命周期要求
    let mod_manager = state.mod_manager.clone();
    let settings_clone = settings.clone();

    // 执行任务队列
    let result = state
        .task_queue
        .run_task("load_mods", async move {
            mod_manager.load_mods(&settings_clone).await
        })
        .await;

    // 计算耗时
    let duration = start_time.elapsed();
    match result {
        Ok(groups) => {
            log::debug!(
                "[load_mods] Success - game: {:?}, groups: {}, duration: {:?}",
                game,
                groups.len(),
                duration
            );
            Ok(groups)
        }
        Err(e) => {
            let error_msg = match e {
                TaskQueueError::TaskCancelled(t) => {
                    log::debug!(
                        "[load_mods] Cancelled - task: {}, game: {:?}, duration: {:?}",
                        t,
                        game,
                        duration
                    );
                    format!("Task '{}' was cancelled", t)
                }
                TaskQueueError::ExecutionError(e) => {
                    log::debug!(
                        "[load_mods] Failed - game: {:?}, error: {}, duration: {:?}",
                        game,
                        e,
                        duration
                    );
                    format!("Task execution failed: {}", e)
                }
            };
            log::error!("[load_mods] Error - {:?}", error_msg);
            Err(error_msg)
        }
    }
}

/// 刷新模组数据（语义上表示强制重新加载，与 `load_mods` 等价）。
///
/// 复用 `load_mods` 的任务类型标识，因此与 `load_mods` 互斥执行。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `game`: 可选的目标游戏字符串（前端枚举值，带下划线）。
///
/// 返回：分组数据列表。
#[tauri::command]
pub async fn refresh_mods(
    state: State<'_, AppState>,
    game: Option<String>,
) -> Result<Vec<ModGroupData>, String> {
    let mut settings = state.settings.read().clone();
    if let Some(g) = game {
        use crate::process::TargetGame;
        let target = match g.as_str() {
            "Wuthering_Waves" | "WutheringWaves" => TargetGame::WutheringWaves,
            "Genshin_Impact" | "GenshinImpact" => TargetGame::GenshinImpact,
            "Honkai_Star_Rail" | "HonkaiStarRail" => TargetGame::HonkaiStarRail,
            "Zenless_Zone_Zero" | "ZenlessZoneZero" => TargetGame::ZenlessZoneZero,
            "Arknights_Endfield" | "ArknightsEndfield" => TargetGame::ArknightsEndfield,
            _ => TargetGame::None,
        };
        settings.target_game = target;
    }
    // 克隆参数以满足 'static 生命周期要求
    let mod_manager = state.mod_manager.clone();
    let settings_clone = settings.clone();

    state
        .task_queue
        .run_task("load_mods", async move {
            mod_manager.load_mods(&settings_clone).await
        })
        .await
        .map_err(|e| match e {
            TaskQueueError::TaskCancelled(t) => format!("Task '{}' was cancelled", t),
            TaskQueueError::ExecutionError(e) => format!("Task execution failed: {}", e),
        })
}

/// 根据指定路径刷新模组数据（不依赖全局设置）。
///
/// 直接接受 Mods 路径参数，固定使用按索引排序。不经过任务队列。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `path`: Mods 根目录路径。
///
/// 返回：分组数据列表。
#[tauri::command]
pub async fn refresh_mod_data(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<ModGroupData>, String> {
    state
        .mod_manager
        .refresh_mod_data(&path)
        .await
        .map_err(|e| e.to_string())
}

/// 执行模组数据更新（核心管理流程）。
///
/// 触发 INI 注入、错误检测、hash 冲突检测等完整流程。
/// 通过 `TaskQueue` 保证同一时刻只有一个 `update_mod_data` 任务在执行。
///
/// 参数：
/// - `_app`: Tauri 应用句柄（当前未使用）。
/// - `state`: 应用全局状态。
/// - `game`: 目标游戏标识（如 `"WutheringWaves"`），与 `load_mods`/`refresh_mods` 保持一致。
/// - `known_libraries`: 已知模组库命名空间映射（可选）。
///
/// 返回：`UpdateModDataResult`，包含成功状态、日志、耗时及各项检测报告。
#[tauri::command]
pub async fn update_mod_data(
    app: AppHandle,
    state: State<'_, AppState>,
    game: Option<String>,
    known_libraries: Option<HashMap<String, String>>,
) -> Result<UpdateModDataResult, String> {
    log::debug!("[update_mod_data] command received - game: {:?}", game);

    // 从 settings 获取 mods_path，与 load_mods/refresh_mods 保持一致
    let mut settings = state.settings.read().clone();
    if let Some(g) = game {
        use crate::process::TargetGame;
        let target = match g.as_str() {
            "Wuthering_Waves" | "WutheringWaves" => TargetGame::WutheringWaves,
            "Genshin_Impact" | "GenshinImpact" => TargetGame::GenshinImpact,
            "Honkai_Star_Rail" | "HonkaiStarRail" => TargetGame::HonkaiStarRail,
            "Zenless_Zone_Zero" | "ZenlessZoneZero" => TargetGame::ZenlessZoneZero,
            "Arknights_Endfield" | "ArknightsEndfield" => TargetGame::ArknightsEndfield,
            e => {
                log::error!(
                    "[update_mod_data] Unknown game string: {:?}, defaulting to None",
                    e
                );
                return Err(format!("Unknown game string: {}", e));
            }
        };
        settings.target_game = target;
    }

    let mods_path = crate::mod_manager::ModManager::get_mods_path_for_game(&settings, settings.target_game);
    if mods_path.is_empty() {
        return Err("Mods path is not configured for the selected game".to_string());
    }
    log::debug!("[update_mod_data] mods_path resolved: {:?}", mods_path);

    let known_libraries = known_libraries.unwrap_or_default();
    log::debug!(
        "[update_mod_data] known_libraries count: {}",
        known_libraries.len()
    );

    // 克隆参数以满足 'static 生命周期要求
    let mod_manager = state.mod_manager.clone();
    let mods_path_clone = mods_path.clone();
    let known_libraries_clone = known_libraries.clone();

    log::debug!("[update_mod_data] submitting task to queue...");
    let result = state
        .task_queue
        .run_task("update_mod_data", async move {
            mod_manager
                .update_mod_data(&mods_path_clone, &known_libraries_clone)
                .await
        })
        .await
        .map_err(|e| match e {
            TaskQueueError::TaskCancelled(t) => format!("Task '{}' was cancelled", t),
            TaskQueueError::ExecutionError(e) => format!("Task execution failed: {}", e),
        })?;

    log::debug!("[update_mod_data] task completed successfully");

    // 更新完成后异步触发独立 Hash 冲突检测
    // （`update_mod_data` 内部已包含 hash 检测，但独立检测可保证报告
    //  通过事件推送给前端，并支持后续独立调用入口）
    trigger_hash_conflict_check(&app, &state);
    log::debug!("[update_mod_data] trigger_hash_conflict_check dispatched");

    Ok(result)
}

/// 校验 Mods 路径是否为合法的 3DMigoto Mods 目录。
///
/// 检查路径存在性、目录名、d3dx.ini、d3d11.dll、_MANAGED_ 文件夹等。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 待校验的 Mods 目录路径。
///
/// 返回：`ModsPathStatus` 枚举值，表示首个失败的检查项或 `Valid`。
#[tauri::command]
pub async fn validate_mods_path(
    state: State<'_, AppState>,
    path: String,
) -> Result<ModsPathStatus, String> {
    let _ = state;
    Ok(crate::mod_manager::ModManager::validate_mods_path(&path))
}

/// 读取分组的显示名称。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `group_path`: 分组目录路径。
///
/// 返回：分组显示名称字符串。
#[tauri::command]
pub async fn get_group_name(
    state: State<'_, AppState>,
    group_path: String,
) -> Result<String, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::get_group_name(&group_path).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 设置分组的显示名称（覆盖写入 `groupname` 文件）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `group_path`: 分组目录路径。
/// - `name`: 要写入的分组名称。
#[tauri::command]
pub async fn set_group_name(
    state: State<'_, AppState>,
    group_path: String,
    name: String,
) -> Result<(), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::set_group_name(&group_path, &name)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 获取分组内当前选中的模组索引。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `group_path`: 分组目录路径。
/// - `mods_count`: 该分组内模组总数（用于边界校验）。
///
/// 返回：当前选中的模组索引（保证在 `[0, mods_count)` 范围内）。
#[tauri::command]
pub async fn get_selected_mod(
    state: State<'_, AppState>,
    group_path: String,
    mods_count: usize,
) -> Result<i32, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::get_selected_mod_in_group(&group_path, mods_count)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 持久化分组内当前选中的模组索引。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `group_path`: 分组目录路径。
/// - `index`: 要保存的模组索引。
#[tauri::command]
pub async fn set_selected_mod(
    state: State<'_, AppState>,
    group_path: String,
    index: i32,
) -> Result<(), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::set_selected_mod_in_group(&group_path, index)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 获取 `_MANAGED_` 目录下当前选中的分组索引。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `managed_path`: `_MANAGED_` 目录路径。
/// - `group_count`: 分组总数（用于边界校验）。
///
/// 返回：当前选中的分组索引。
#[tauri::command]
pub async fn get_selected_group(
    state: State<'_, AppState>,
    managed_path: String,
    group_count: usize,
) -> Result<i32, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::get_selected_group_index(&managed_path, group_count)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 持久化 `_MANAGED_` 目录下当前选中的分组索引。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `managed_path`: `_MANAGED_` 目录路径。
/// - `index`: 要保存的分组索引。
#[tauri::command]
pub async fn set_selected_group(
    state: State<'_, AppState>,
    managed_path: String,
    index: i32,
) -> Result<(), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::set_selected_group_index(&managed_path, index)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 查询指定路径是否已收藏，并返回收藏时间戳。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 模组或分组目录路径。
///
/// 返回：`Some(时间戳)` 表示已收藏，`None` 表示未收藏。
#[tauri::command]
pub async fn is_favorite(
    state: State<'_, AppState>,
    path: String,
) -> Result<Option<String>, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::is_favorite(&path).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 切换路径的收藏状态（收藏 ↔ 取消收藏）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 模组或分组目录路径。
///
/// 返回：操作后该路径是否处于收藏状态。
#[tauri::command]
pub async fn toggle_favorite(state: State<'_, AppState>, path: String) -> Result<bool, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::toggle_favorite(&path).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 切换分组的收藏状态（`toggle_favorite` 的分组专用别名）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `group_path`: 分组目录路径。
///
/// 返回：操作后该分组是否处于收藏状态。
#[tauri::command]
pub async fn toggle_group_favorite(
    state: State<'_, AppState>,
    group_path: String,
) -> Result<bool, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::toggle_favorite(&group_path).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 切换 # 目录分组的启用/禁用状态。
///
/// 通过在目录名前添加或移除 `DISABLED` 前缀实现状态切换。
/// 该功能仅对 # 目录分组生效，group_xx 分组调用将返回错误。
///
/// 参数：
/// - `group_path`: 目标分组目录路径。
///
/// 返回：操作后的禁用状态（true = 已禁用，false = 已启用）。
#[tauri::command]
pub async fn toggle_tree_node_group_disabled(group_path: String) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::toggle_tree_node_group_disabled(&group_path)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 切换模组的收藏状态（`toggle_favorite` 的模组专用别名）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `mod_path`: 模组目录路径。
///
/// 返回：操作后该模组是否处于收藏状态。
#[tauri::command]
pub async fn toggle_mod_favorite(
    state: State<'_, AppState>,
    mod_path: String,
) -> Result<bool, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::toggle_favorite(&mod_path).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 查找目录中的图标文件路径。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 模组或分组目录路径。
///
/// 返回：图标文件路径（`Some`）或 `None`（无图标）。
#[tauri::command]
pub async fn get_icon_path(
    state: State<'_, AppState>,
    path: String,
) -> Result<Option<String>, String> {
    let _ = state;
    Ok(
        tokio::task::spawn_blocking(move || crate::mod_manager::ModManager::get_icon_path(path))
            .await
            .unwrap_or_default(),
    )
}

/// 将源图标文件复制到目标目录下作为 `icon.png`。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 目标目录路径。
/// - `icon_source_path`: 源图标文件路径。
#[tauri::command]
pub async fn set_icon(
    state: State<'_, AppState>,
    path: String,
    icon_source_path: String,
) -> Result<(), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::set_icon(&path, &icon_source_path)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 移除目录下的图标文件（仅删除 `icon.<ext>`）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 目标目录路径。
#[tauri::command]
pub async fn remove_icon(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::remove_icon(&path).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// Hash 冲突检测事件名（前后端共享）。
///
/// 在 `toggle_mod_disabled`、`toggle_tree_node_mod_disabled`、`refresh_mods`、
/// `update_mod_data` 等命令完成后通过 `AppHandle::emit` 推送。
pub const HASH_CONFLICTS_DETECTED_EVENT: &str = "hash-conflicts-detected";

/// 在模组操作完成后异步触发 Hash 冲突检测。
///
/// 该函数不阻塞原命令返回：
/// - 启动一个独立的 Tokio 任务在后台执行检测；
/// - 检测完成后通过 `HASH_CONFLICTS_DETECTED_EVENT` 事件推送结果；
/// - 检测失败仅记录错误日志，不推送事件，不影响原操作。
///
/// 参数：
/// - `app`: Tauri 应用句柄，用于 emit 事件。
/// - `state`: 应用全局状态。
fn trigger_hash_conflict_check(app: &AppHandle, state: &AppState) {
    let app_clone = app.clone();
    let mod_manager = state.mod_manager.clone();
    let settings_clone = state.settings.read().clone();

    tokio::spawn(async move {
        match mod_manager
            .check_hash_conflicts_async(&settings_clone)
            .await
        {
            Ok(report) => {
                let payload = serde_json::json!({
                    "game": settings_clone.target_game,
                    "report": report,
                    "completedAt": std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0),
                });
                if let Err(e) = app_clone.emit(HASH_CONFLICTS_DETECTED_EVENT, payload) {
                    log::warn!("Failed to emit HASH_CONFLICTS_DETECTED event: {}", e);
                }
            }
            Err(e) => {
                log::error!("Auto hash conflict check failed: {}", e);
            }
        }
    });
}

/// 切换模组的启用/禁用状态（通过重命名添加/移除 `DISABLED` 前缀）。
///
/// 操作成功后异步触发 Hash 冲突检测（不阻塞本命令返回）。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
/// - `mod_path`: 模组目录路径。
///
/// 返回：操作后该模组是否处于禁用状态（`true` 表示已禁用）。
#[tauri::command]
pub async fn toggle_mod_disabled(
    app: AppHandle,
    state: State<'_, AppState>,
    mod_path: String,
) -> Result<bool, String> {
    let _ = state;
    let result = tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::toggle_mod_disabled(&mod_path).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))?;

    // 操作成功后异步触发 Hash 冲突检测
    trigger_hash_conflict_check(&app, &state);

    Ok(result)
}

/// 切换树节点（# 目录）下模组的启用/禁用状态（互斥模式）。
///
/// 与普通 `toggle_mod_disabled` 的区别：
/// - 启用操作：先禁用同 # 目录下所有其他模组，再启用目标模组（单选互斥）。
/// - 禁用操作：直接禁用目标模组，不影响其他模组。
/// - 不涉及 INI 文件修改，纯靠目录重命名实现。
///
/// 操作成功后异步触发 Hash 冲突检测。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
/// - `mod_path`: 目标模组目录路径。
///
/// 返回：`(新模组路径, 操作后是否禁用)`。
#[tauri::command]
pub async fn toggle_tree_node_mod_disabled(
    app: AppHandle,
    state: State<'_, AppState>,
    mod_path: String,
) -> Result<(String, bool), String> {
    let _ = state;
    let result = tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::toggle_tree_node_mod_disabled(&mod_path)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))?;

    trigger_hash_conflict_check(&app, &state);

    Ok(result)
}

/// 安全禁用树节点（# 目录）下的指定模组目录。
///
/// 仅对目标目录添加 `DISABLED` 前缀；若已处于禁用状态则直接返回原路径。
/// 与 `toggle_tree_node_mod_disabled` 不同，本函数不会启用已禁用的模组，也不影响同目录下其他模组。
///
/// 参数：
/// - `mod_path`: 目标模组目录路径。
///
/// 返回：操作后的新路径字符串。
#[tauri::command]
pub fn disable_tree_node_mod(mod_path: String) -> Result<String, String> {
    crate::mod_manager::ModManager::disable_tree_node_mod(&mod_path).map_err(|e| e.to_string())
}

/// 独立执行 Hash 冲突检测。
///
/// 通过 `TaskQueue` 任务类型 `"check_hash_conflicts"` 互斥执行：
/// - 同类型任务并发时，新请求会取消旧请求（最新请求优先）。
/// - 与 `update_mod_data` 互不阻塞（不同任务类型）。
///
/// 参数：
/// - `_app`: Tauri 应用句柄（当前未使用）。
/// - `state`: 应用全局状态。
///
/// 返回：`HashConflictReport`（包含 `enabled_mod_hashes` 与 `conflicts` 字段）。
/// 错误：任务被取消时返回 `"Task 'check_hash_conflicts' was cancelled"`。
#[tauri::command]
pub async fn check_hash_conflicts(
    _app: AppHandle,
    state: State<'_, AppState>,
) -> Result<HashConflictReport, String> {
    let mod_manager = state.mod_manager.clone();
    let settings_clone = state.settings.read().clone();

    state
        .task_queue
        .run_task("check_hash_conflicts", async move {
            mod_manager
                .check_hash_conflicts_async(&settings_clone)
                .await
        })
        .await
        .map_err(|e| match e {
            TaskQueueError::TaskCancelled(t) => format!("Task '{}' was cancelled", t),
            TaskQueueError::ExecutionError(e) => format!("Task execution failed: {}", e),
        })
}

/// 在指定位置新建一个分组。
///
/// 根据 `target_group_path` 参数决定创建位置：
/// - 未指定或指定的是 `_MANAGED_` 下的 `group_xx` 分组：在 `_MANAGED_` 目录下创建新的 `group_<index>` 分组
/// - 指定的是 `#` 目录下的分组：在同一父目录下创建用户命名的分组目录
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `group_name`: 新分组的显示名称。
/// - `target_group_path`: 目标分组路径（可选）。指定后新分组将与该分组处于同一目录层级。
///
/// 返回：新分组的索引（对于 group_xx 分组）或 0（对于 # 目录下的分组）。
/// 错误：未配置 Mods 路径时返回 `"No mods path configured"`。
#[tauri::command]
pub async fn add_group(
    state: State<'_, AppState>,
    group_name: String,
    target_group_path: Option<String>,
) -> Result<i32, String> {
    match target_group_path {
        Some(target_path) => {
            let target_path = Path::new(&target_path);
            if !target_path.exists() || !target_path.is_dir() {
                return Err("Target group path does not exist".to_string());
            }

            let parent_path = target_path.parent();
            if parent_path.is_none() {
                return Err("Invalid target group path".to_string());
            }

            let parent_path_str = parent_path
                .ok_or_else(|| "Invalid target group path: no parent directory".to_string())?
                .to_string_lossy()
                .to_string();

            tokio::task::spawn_blocking(move || {
                crate::mod_manager::ModManager::add_child_group(&parent_path_str, &group_name)
                    .map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
        }
        None => {
            let managed_path = {
                let settings = state.settings.read();
                let mods_path = crate::mod_manager::ModManager::get_mods_path_for_game(
                    &settings,
                    settings.target_game,
                );

                if mods_path.is_empty() {
                    return Err("No mods path configured".to_string());
                }

                Path::new(&mods_path)
                    .join(crate::mod_manager::MANAGED_FOLDER)
                    .to_string_lossy()
                    .to_string()
            };

            tokio::task::spawn_blocking(move || {
                crate::mod_manager::ModManager::add_group(&managed_path, &group_name)
                    .map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
        }
    }
}

/// 移除分组（移动到 `_MANAGED_` 下并加时间戳后缀，便于恢复）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `group_path`: 待移除的分组目录路径。
#[tauri::command]
pub async fn remove_group(state: State<'_, AppState>, group_path: String) -> Result<(), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::remove_group(&group_path).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 移除单个模组：先还原（启用）再移动到 DISABLED_MANAGED_REMOVED 目录。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `mod_path`: 待移除的模组目录路径。
#[tauri::command]
pub async fn remove_mod(state: State<'_, AppState>, mod_path: String) -> Result<(), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::remove_mod(&mod_path).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 重命名分组目录（直接修改目录名）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `group_path`: 分组目录路径。
/// - `new_name`: 新的目录名称。
#[tauri::command]
pub async fn rename_group(
    state: State<'_, AppState>,
    group_path: String,
    new_name: String,
) -> Result<(), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::rename_group(&group_path, &new_name)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 重命名模组目录。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `mod_path`: 模组目录路径。
/// - `new_name`: 新的目录名称（不含 DISABLED 前缀）。
///
/// 返回：`Ok(())` 表示重命名成功。
#[tauri::command]
pub async fn rename_mod(
    state: State<'_, AppState>,
    mod_path: String,
    new_name: String,
) -> Result<(), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::rename_mod(&mod_path, &new_name).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 搜索名称包含关键词的模组（当前为占位实现，返回空列表）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `keyword`: 搜索关键词。
///
/// 返回：`Vec<(分组索引, 模组索引)>`（当前始终为空）。
#[tauri::command]
pub async fn search_mods(
    state: State<'_, AppState>,
    keyword: String,
) -> Result<Vec<(usize, usize)>, String> {
    let _ = state;
    let _ = keyword;
    Ok(Vec::new())
}

/// 刷新单个分组的模组列表。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `group_path`: 分组目录路径。
///
/// 返回：更新后的分组数据（仅包含最新的 mods，保留原有 children）。
#[tauri::command]
pub async fn refresh_single_group(
    state: State<'_, AppState>,
    group_path: String,
) -> Result<crate::mod_manager::ModGroupData, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        use crate::mod_manager::ModGroupData;
        use crate::mod_manager::ModManager;

        let group_path_str = group_path.clone();
        let path = std::path::Path::new(&group_path);

        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let icon_path = ModManager::get_icon_path(&group_path_str);
        let favorite_date_time = ModManager::is_favorite(&group_path_str).unwrap_or(None);
        let is_under_hash_dir = crate::mod_manager::path_utils::is_path_under_hash_dir(path);
        let mods_in_group = if is_under_hash_dir {
            ModManager::get_mods_on_group_readonly(&group_path_str).map_err(|e| e.to_string())?
        } else {
            ModManager::get_mods_on_group(&group_path_str).map_err(|e| e.to_string())?
        };
        let mods_count = mods_in_group.len();
        // # 目录分组根据当前启用的模组推导索引（互斥模式下同一分组最多一个启用模组）
        let is_hash_group = dir_name.starts_with('#')
            || (ModManager::is_disabled_name(&dir_name)
                && dir_name[crate::mod_manager::DISABLED_PREFIX.len()..]
                    .trim_start_matches('_')
                    .starts_with('#'));
        let previous_selected_mod_on_group = if is_hash_group {
            ModManager::get_enabled_mod_index_in_group(&mods_in_group)
        } else {
            ModManager::get_selected_mod_in_group(&group_path_str, mods_count).unwrap_or(0)
        };

        Ok(ModGroupData {
            group_path: group_path_str,
            icon_path,
            group_name: dir_name,
            favorite_date_time,
            mods_in_group,
            real_index: 0,
            previous_selected_mod_on_group,
            children: vec![],
            is_tree_node: true,
            is_virtual: false,
            is_disabled: false,
        })
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 异步加载 INI 文件并返回可序列化数据结构。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `path`: INI 文件路径。
///
/// 返回：`IniFileData`（用于前后端 JSON 传输）。
#[tauri::command]
pub async fn load_ini(
    state: State<'_, AppState>,
    path: String,
) -> Result<crate::ini_handler::IniFileData, String> {
    let ini_file = state
        .ini_handler
        .load_ini(&path)
        .await
        .map_err(|e| e.to_string())?;
    Ok(crate::ini_handler::IniFileData::from(&ini_file))
}

/// 异步保存 INI 文件。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `path`: 目标文件路径。
/// - `data`: INI 文件数据结构。
#[tauri::command]
pub async fn save_ini(
    state: State<'_, AppState>,
    path: String,
    data: crate::ini_handler::IniFileData,
) -> Result<(), String> {
    let ini_file = crate::ini_handler::IniFile::from(data);
    state
        .ini_handler
        .save_ini(&ini_file, &path)
        .await
        .map_err(|e| e.to_string())
}

/// 保存按键绑定：修改指定 INI 文件中指定段的按键值。
///
/// 参数：
/// - `ini_path`: INI 文件路径。
/// - `section_name`: 段名（如 `"Key.Toggle"`）。
/// - `key_index`: 该段中 key= 行的序号（从 0 开始）。
/// - `new_key_value`: 新的按键值。
#[tauri::command]
pub async fn save_keybind(
    ini_path: String,
    section_name: String,
    key_index: usize,
    new_key_value: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::ini_handler::save_keybind(&ini_path, &section_name, key_index, &new_key_value)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 切换按键绑定的启用/禁用状态。
///
/// 参数：
/// - `ini_path`: INI 文件路径。
/// - `section_name`: 段名。
/// - `key_index`: key= 行序号（从 0 开始）。
/// - `enabled`: `true` 启用，`false` 禁用。
#[tauri::command]
pub async fn toggle_keybind_enabled(
    ini_path: String,
    section_name: String,
    key_index: usize,
    enabled: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::ini_handler::toggle_keybind_enabled(&ini_path, &section_name, key_index, enabled)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 从 INI 文件中提取所有命名空间。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `path`: INI 文件路径。
///
/// 返回：命名空间字符串列表（去重，保留首次出现顺序）。
#[tauri::command]
pub async fn extract_namespaces(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<String>, String> {
    let ini_file = state
        .ini_handler
        .load_ini(&path)
        .await
        .map_err(|e| e.to_string())?;
    Ok(crate::ini_handler::extract_namespaces(&ini_file))
}

/// 启动对指定目录的文件监视。
///
/// 参数：
/// - `app`: Tauri 应用句柄（用于发送事件给前端）。
/// - `state`: 应用全局状态。
/// - `path`: 待监视的目录路径。
#[tauri::command]
pub async fn start_file_watcher(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let mut watcher = state.file_watcher.lock();
    watcher
        .start_watching(&path, app)
        .map_err(|e| e.to_string())
}

/// 停止文件监视。
///
/// 参数：
/// - `state`: 应用全局状态。
#[tauri::command]
pub async fn stop_file_watcher(state: State<'_, AppState>) -> Result<(), String> {
    let mut watcher = state.file_watcher.lock();
    watcher.stop_watching().map_err(|e| e.to_string())
}

/// 查询文件监视器是否正在运行。
///
/// 参数：
/// - `state`: 应用全局状态。
///
/// 返回：`true` 表示正在监视，`false` 表示已停止。
#[tauri::command]
pub async fn is_file_watcher_running(state: State<'_, AppState>) -> Result<bool, String> {
    let watcher = state.file_watcher.lock();
    Ok(watcher.is_watching())
}

/// 注册全局快捷键。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
/// - `key`: 快捷键描述字符串（如 `"F10"`、`"Ctrl+Shift+F"`）。
#[tauri::command]
pub async fn register_hotkey(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    key: String,
) -> Result<(), String> {
    let _ = state;
    log::debug!("[register_hotkey] Received request: key={}", key);
    match crate::hotkey::HotkeyManager::register_hotkey(&app, &key) {
        Ok(()) => {
            log::debug!("[register_hotkey] Succeeded: key={}", key);
            Ok(())
        }
        Err(e) => {
            log::warn!("[register_hotkey] Failed: key={}, error={}", key, e);
            Err(e.to_string())
        }
    }
}

/// 注销指定快捷键。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
/// - `key`: 要注销的快捷键描述字符串。
#[tauri::command]
pub async fn unregister_hotkey(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    key: String,
) -> Result<(), String> {
    let _ = state;
    log::debug!("[unregister_hotkey] Received request: key={}", key);
    match crate::hotkey::HotkeyManager::unregister_hotkey(&app, &key) {
        Ok(()) => {
            log::debug!("[unregister_hotkey] Succeeded: key={}", key);
            Ok(())
        }
        Err(e) => {
            log::warn!("[unregister_hotkey] Failed: key={}, error={}", key, e);
            Err(e.to_string())
        }
    }
}

/// 注销所有已注册的快捷键。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
#[tauri::command]
pub async fn unregister_all_hotkeys(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _ = state;
    log::debug!("[unregister_all_hotkeys] Received request");
    match crate::hotkey::HotkeyManager::unregister_all(&app) {
        Ok(()) => {
            log::debug!("[unregister_all_hotkeys] Succeeded");
            Ok(())
        }
        Err(e) => {
            log::warn!("[unregister_all_hotkeys] Failed: error={}", e);
            Err(e.to_string())
        }
    }
}

/// 查询指定快捷键是否已注册。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
/// - `key`: 快捷键描述字符串。
///
/// 返回：`true` 表示已注册。
#[tauri::command]
pub async fn is_hotkey_registered(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    key: String,
) -> Result<bool, String> {
    let _ = state;
    crate::hotkey::HotkeyManager::is_registered(&app, &key).map_err(|e| e.to_string())
}

/// 显示主窗口。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
#[tauri::command]
pub async fn show_window(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _ = state;
    crate::window_manager::WindowManager::show_window(&app).map_err(|e| e.to_string())
}

/// 隐藏主窗口。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
#[tauri::command]
pub async fn hide_window(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _ = state;
    crate::window_manager::WindowManager::hide_window(&app).map_err(|e| e.to_string())
}

/// 切换主窗口的显示/隐藏状态。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
///
/// 返回：操作后窗口是否处于显示状态。
#[tauri::command]
pub async fn toggle_window(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let _ = state;
    log::debug!("[toggle_window] Received request");
    match crate::window_manager::WindowManager::toggle_window(&app) {
        Ok(shown) => {
            log::debug!("[toggle_window] Succeeded, visible: {}", shown);
            Ok(shown)
        }
        Err(e) => {
            log::warn!("[toggle_window] Failed: {}", e);
            Err(e.to_string())
        }
    }
}

/// 设置窗口是否置顶。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
/// - `on_top`: `true` 表示置顶，`false` 表示取消置顶。
#[tauri::command]
pub async fn set_always_on_top(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    on_top: bool,
) -> Result<(), String> {
    let _ = state;
    crate::window_manager::WindowManager::set_always_on_top(&app, on_top).map_err(|e| e.to_string())
}

/// 查询窗口是否处于置顶状态。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
///
/// 返回：`true` 表示已置顶。
#[tauri::command]
pub async fn is_always_on_top(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let _ = state;
    crate::window_manager::WindowManager::is_always_on_top(&app).map_err(|e| e.to_string())
}

/// 设置窗口尺寸。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
/// - `width`: 窗口宽度（像素）。
/// - `height`: 窗口高度（像素）。
#[tauri::command]
pub async fn set_window_size(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let _ = state;
    crate::window_manager::WindowManager::set_size(&app, width, height).map_err(|e| e.to_string())
}

/// 获取窗口尺寸。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
///
/// 返回：`(宽度, 高度)`（像素）。
#[tauri::command]
pub async fn get_window_size(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(f64, f64), String> {
    let _ = state;
    crate::window_manager::WindowManager::get_size(&app).map_err(|e| e.to_string())
}

/// 设置窗口位置。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
/// - `x`: 水平坐标（像素）。
/// - `y`: 垂直坐标（像素）。
#[tauri::command]
pub async fn set_window_position(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    let _ = state;
    crate::window_manager::WindowManager::set_position(&app, x, y).map_err(|e| e.to_string())
}

/// 重置窗口位置到默认位置（通常为屏幕居中）。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
#[tauri::command]
pub async fn reset_window_position(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _ = state;
    crate::window_manager::WindowManager::reset_position(&app).map_err(|e| e.to_string())
}

/// 保存窗口状态（位置、尺寸、置顶等）到设置文件。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态。
#[tauri::command]
pub async fn save_window_state(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::window_manager::WindowManager::save_window_state(&app, &state.settings)
        .map_err(|e| e.to_string())?;

    let app_data_dir =
        crate::get_app_data_dir().ok_or_else(|| "Failed to get app data dir".to_string())?;

    let settings_arc = state.settings.clone();
    tokio::spawn(async move {
        let settings = settings_arc.read();
        if let Err(e) = settings.save(&app_data_dir) {
            log::error!("Failed to save settings file: {}", e);
        }
    });

    Ok(())
}

/// 初始化系统托盘。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（用于获取当前语言设置）。
#[tauri::command]
pub async fn setup_tray(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings.read();
    let locale = settings.language.as_str();
    crate::tray::TrayManager::setup_tray(&app, locale).map_err(|e| e.to_string())
}

/// 更新系统托盘的提示文字（tooltip）。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态（当前未使用）。
/// - `tooltip`: 新的提示文字。
#[tauri::command]
pub async fn update_tray_tooltip(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    tooltip: String,
) -> Result<(), String> {
    let _ = state;
    crate::tray::TrayManager::update_tray_tooltip(&app, &tooltip).map_err(|e| e.to_string())
}

/// 查询指定进程是否正在运行。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `process_name`: 进程名称。
///
/// 返回：`true` 表示进程正在运行。
#[tauri::command]
pub async fn is_process_running(
    _state: State<'_, AppState>,
    process_name: String,
) -> Result<bool, String> {
    let process_name = process_name.clone();
    tokio::task::spawn_blocking(move || {
        crate::process::ProcessDetector::new()
            .is_process_running(&process_name)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 获取当前系统中所有正在运行的进程名称列表。
///
/// 参数：
/// - `state`: 应用全局状态。
///
/// 返回：进程名称字符串列表。
#[tauri::command]
pub async fn get_process_list(_state: State<'_, AppState>) -> Result<Vec<String>, String> {
    tokio::task::spawn_blocking(|| {
        crate::process::ProcessDetector::new()
            .get_process_list()
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 获取当前前台进程的名称。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
///
/// 返回：前台进程名称字符串。
#[tauri::command]
pub async fn get_foreground_process(state: State<'_, AppState>) -> Result<String, String> {
    let _ = state;
    crate::process::ProcessDetector::get_foreground_process_name().map_err(|e| e.to_string())
}

/// 获取当前前台进程对应的目标游戏。
///
/// 通过匹配前台进程名称与已知游戏进程名，判断用户当前正在玩哪款游戏。
///
/// 参数：
/// - `state`: 应用全局状态。
///
/// 返回：目标游戏名称字符串（如 `"WutheringWaves"`）；未匹配时返回空字符串。
#[tauri::command]
pub async fn get_foreground_game(state: State<'_, AppState>) -> Result<String, String> {
    let foreground_process = crate::process::ProcessDetector::get_foreground_process_name()
        .map_err(|e| e.to_string())?;

    let settings = state.settings.read();
    let game = crate::process::ProcessDetector::match_game_process(&foreground_process, &settings);

    Ok(format!("{:?}", game))
}

/// 获取全局设置。
///
/// 参数：
/// - `state`: 应用全局状态。
///
/// 返回：当前设置的克隆副本。
#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<crate::settings::Settings, String> {
    Ok(state.settings.read().clone())
}

/// 更新单个设置字段并持久化。
///
/// 替代全量 `save_settings`，支持按字段增量更新。
/// 每个字段独立校验，失败时仅影响该字段，不污染其他字段。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `key`: 设置字段名（驼峰式，如 `"language"`、`"overallScale"`）。
/// - `value`: 字段新值的 JSON Value。
///
/// 返回：成功返回 `Ok(())`，失败返回错误描述。
#[tauri::command]
pub async fn update_setting(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> Result<(), String> {
    let app_data_dir = crate::get_app_data_dir().ok_or_else(|| "Failed to get app data dir".to_string())?;

    // 1. 读取当前 settings，克隆以释放读锁
    let settings_snapshot = state.settings.read().clone();

    // 2. 解析 JSON 值
    let parsed: serde_json::Value = serde_json::from_str(&value)
        .map_err(|e| format!("Invalid JSON value for key '{}': {}", key, e))?;

    // 3. 根据 key 更新对应字段
    let mut settings = settings_snapshot;
    let json_to_string = |v: &serde_json::Value| -> Result<String, String> {
        v.as_str().map(String::from).ok_or_else(|| format!("Expected string for key '{}', got {:?}", key, v))
    };
    let json_to_f64 = |v: &serde_json::Value| -> Result<f64, String> {
        v.as_f64().ok_or_else(|| format!("Expected number for key '{}', got {:?}", key, v))
    };
    let json_to_i32 = |v: &serde_json::Value| -> Result<i32, String> {
        v.as_i64().map(|n| n as i32).ok_or_else(|| format!("Expected integer for key '{}', got {:?}", key, v))
    };
    let json_to_bool = |v: &serde_json::Value| -> Result<bool, String> {
        v.as_bool().ok_or_else(|| format!("Expected boolean for key '{}', got {:?}", key, v))
    };

    match key.as_str() {
        "language" => settings.language = json_to_string(&parsed)?,
        "theme" => settings.theme = json_to_string(&parsed)?,
        "hotkeyKeyboard" => settings.hotkey_keyboard = json_to_string(&parsed)?,
        "hotkeyGamepad" => settings.hotkey_gamepad = json_to_string(&parsed)?,
        "searchHotkey" => settings.search_hotkey = json_to_string(&parsed)?,
        "overallScale" => settings.overall_scale = json_to_f64(&parsed)?,
        "bgTransparency" => settings.bg_transparency = json_to_f64(&parsed)?,
        "layoutMode" => settings.layout_mode = json_to_i32(&parsed)?,
        "targetGame" => {
            settings.target_game = serde_json::from_value(parsed.clone())
                .map_err(|e| format!("Invalid targetGame value: {}", e))?;
        }
        "isAutoGenerateFolderIcon" => settings.is_auto_generate_folder_icon = json_to_bool(&parsed)?,
        "isAutoPinWindow" => settings.is_auto_pin_window = json_to_bool(&parsed)?,
        "showMenuWhenTogglingOutsideGame" => settings.show_menu_when_toggling_outside_game = json_to_bool(&parsed)?,
        "keybindSimulateKeypress" => settings.keybind_simulate_keypress = json_to_bool(&parsed)?,
        "sortGroupMethod" => settings.sort_group_method = json_to_i32(&parsed)?,
        "targetProcessWuwa" => settings.target_process_wuwa = json_to_string(&parsed)?,
        "targetProcessGenshin" => settings.target_process_genshin = json_to_string(&parsed)?,
        "targetProcessHsr" => settings.target_process_hsr = json_to_string(&parsed)?,
        "targetProcessZzz" => settings.target_process_zzz = json_to_string(&parsed)?,
        "targetProcessEndfield" => settings.target_process_endfield = json_to_string(&parsed)?,
        "modsPathWuwa" => settings.mods_path_wuwa = json_to_string(&parsed)?,
        "modsPathGenshin" => settings.mods_path_genshin = json_to_string(&parsed)?,
        "modsPathHsr" => settings.mods_path_hsr = json_to_string(&parsed)?,
        "modsPathZzz" => settings.mods_path_zzz = json_to_string(&parsed)?,
        "modsPathEndfield" => settings.mods_path_endfield = json_to_string(&parsed)?,
        "savedWindowWidth" => settings.saved_window_width = json_to_i32(&parsed)?,
        "savedWindowHeight" => settings.saved_window_height = json_to_i32(&parsed)?,
        "enableAutoUpdate" => settings.enable_auto_update = json_to_bool(&parsed)?,
        _ => return Err(format!("Unknown setting key: {}", key)),
    }

    // 4. 校验并修复
    settings.validate_and_fix();

    // 5. 写回内存
    {
        let mut current = state.settings.write();
        *current = settings.clone();
    }

    // 6. 落盘
    let app_data_dir_clone = app_data_dir.clone();
    let settings_clone = settings.clone();
    let save_result = tokio::task::spawn_blocking(move || {
        settings_clone.save(&app_data_dir_clone)
    }).await;

    match save_result {
        Ok(Ok(())) => {
            log::info!("Setting '{}' saved successfully", key);
        }
        Ok(Err(e)) => {
            log::error!("Failed to save setting '{}': {}", key, e);
            return Err(format!("Failed to save setting '{}': {}", key, e));
        }
        Err(e) => {
            log::error!("Failed to join save setting task: {}", e);
            return Err(format!("Failed to save setting '{}': {}", key, e));
        }
    }

    // 7. 热键变更检测（只在热键相关字段变化时执行）
    if key == "hotkeyKeyboard" || key == "hotkeyGamepad" {
        log::debug!("[update_setting] Hotkey config changed (key: {}), re-registering", key);
        if let Err(e) = crate::hotkey::HotkeyManager::register_from_settings(&app, &settings) {
            log::error!("[update_setting] Failed to re-register hotkeys: {}", e);
        } else {
            log::debug!("[update_setting] Hotkeys re-registered successfully");
        }
    }

    Ok(())
}

/// 检测当前应用是否以管理员权限运行。
///
/// 前端可调用此命令在设置页或启动时展示权限提示。
///
/// 返回：当前进程是否拥有管理员/Root 权限。
#[tauri::command]
pub fn is_admin() -> bool {
    crate::admin_check::is_admin()
}

/// 保存设置到内存并持久化到文件。
///
/// 若设置中的热键配置（`hotkey_keyboard` / `hotkey_gamepad`）发生变化，
/// 后端会自动调用 `HotkeyManager::register_from_settings` 重新注册全局热键，
/// 使系统级快捷键与设置保持一致。
///
/// 参数：
/// - `app`: Tauri 应用句柄，用于热键重注册。
/// - `state`: 应用全局状态。
/// - `settings`: 新的设置内容。
#[tauri::command]
pub async fn save_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    settings: crate::settings::Settings,
) -> Result<(), String> {
    let app_data_dir =
        crate::get_app_data_dir().ok_or_else(|| "Failed to get app data dir".to_string())?;

    let (old_hotkey_keyboard, old_hotkey_gamepad) = {
        let current = state.settings.read();
        (
            current.hotkey_keyboard.clone(),
            current.hotkey_gamepad.clone(),
        )
    };
    let hotkeys_changed = old_hotkey_keyboard != settings.hotkey_keyboard
        || old_hotkey_gamepad != settings.hotkey_gamepad;

    {
        let mut current = state.settings.write();
        *current = settings.clone();
    }

    let app_data_dir_clone = app_data_dir.clone();
    let settings_clone = settings.clone();
    let save_result = tokio::task::spawn_blocking(move || {
        settings_clone.save(&app_data_dir_clone)
    }).await;

    match save_result {
        Ok(Ok(())) => {
            log::info!("Settings saved successfully to {:?}", app_data_dir);
        }
        Ok(Err(e)) => {
            log::error!("Failed to save settings: {}", e);
            return Err(format!("Failed to save settings: {}", e));
        }
        Err(e) => {
            log::error!("Failed to join save settings task: {}", e);
            return Err(format!("Failed to save settings: {}", e));
        }
    }

    if hotkeys_changed {
        log::debug!(
            "[save_settings] Hotkey config changed (keyboard: {} -> {}, gamepad: {} -> {}), re-registering from backend",
            old_hotkey_keyboard,
            settings.hotkey_keyboard,
            old_hotkey_gamepad,
            settings.hotkey_gamepad
        );
        if let Err(e) = crate::hotkey::HotkeyManager::register_from_settings(&app, &settings) {
            log::error!(
                "[save_settings] Failed to re-register hotkeys after settings change: {}",
                e
            );
        } else {
            log::debug!("[save_settings] Hotkeys re-registered successfully");
        }
    }

    Ok(())
}

/// 获取已缓存的云端数据。
///
/// 参数：
/// - `state`: 应用全局状态。
///
/// 返回：当前缓存的 `CloudData` 副本。
#[tauri::command]
pub async fn get_cloud_data(
    state: State<'_, AppState>,
) -> Result<crate::cloud_data::CloudData, String> {
    Ok(state.cloud_data.read().clone())
}

/// 从远程获取云端数据并返回（同时更新缓存）。
///
/// 参数：
/// - `state`: 应用全局状态。
///
/// 返回：获取到的 `CloudData`。
#[tauri::command]
pub async fn fetch_cloud_data(
    state: State<'_, AppState>,
) -> Result<crate::cloud_data::CloudData, String> {
    let cloud_data = crate::cloud_data::CloudData::fetch().await.map_err(|e| {
        log::error!("Failed to fetch cloud data: {}", e);
        e.to_string()
    })?;

    {
        let mut state_cloud_data = state.cloud_data.write();
        *state_cloud_data = cloud_data.clone();
    }

    Ok(cloud_data)
}

/// 同步云端数据（获取并更新缓存，不返回数据）。
///
/// 参数：
/// - `state`: 应用全局状态。
#[tauri::command]
pub async fn sync_cloud_data(state: State<'_, AppState>) -> Result<(), String> {
    let cloud_data = crate::cloud_data::CloudData::fetch().await.map_err(|e| {
        log::error!("Failed to sync cloud data: {}", e);
        e.to_string()
    })?;

    {
        let mut state_cloud_data = state.cloud_data.write();
        *state_cloud_data = cloud_data;
    }

    log::info!("Cloud data synced successfully");
    Ok(())
}

/// 模拟单键按下并释放。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `key`: 键名（如 `"f10"`、`"ctrl"`）。
#[tauri::command]
pub async fn simulate_key_press(state: State<'_, AppState>, key: String) -> Result<(), String> {
    state
        .keypress_simulator
        .simulate_key_press(&key)
        .await
        .map_err(|e| e.to_string())
}

/// 模拟组合键（如 Ctrl+Shift+F）。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `keys`: 键名列表，按按下顺序排列。
#[tauri::command]
pub async fn simulate_key_combination(
    state: State<'_, AppState>,
    keys: Vec<String>,
) -> Result<(), String> {
    state
        .keypress_simulator
        .simulate_key_combination(keys)
        .await
        .map_err(|e| e.to_string())
}

/// 模拟鼠标相对移动。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `dx`: 水平方向相对移动量。
/// - `dy`: 垂直方向相对移动量。
#[tauri::command]
pub async fn simulate_mouse_move(
    state: State<'_, AppState>,
    dx: i32,
    dy: i32,
) -> Result<(), String> {
    state
        .keypress_simulator
        .simulate_mouse_move(dx, dy)
        .await
        .map_err(|e| e.to_string())
}
/// 模拟选择模组的按键序列（向游戏发送模组切换信号）。
///
/// 对应 NRMM 的 `simulateKeySelectMod(realGroupIndex, realModIndex)`。
/// 通过全局键盘事件（SendInput）让 3DMigoto 感知模组切换：
/// - VK_CLEAR + VK_SPACE → 触发 [KeyGroup]，更新 $active_group_id
/// - VK_CLEAR + VK_RETURN → 触发 [KeyMod]，更新 $active_slot
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `group_index`: 分组索引。
/// - `mod_index`: 模组索引。
#[tauri::command]
pub async fn simulate_key_select_mod(
    state: State<'_, AppState>,
    group_index: i32,
    mod_index: i32,
) -> Result<(), String> {
    state
        .task_queue
        .run_task("simulate_key_select_mod", async move {
            crate::mod_manager::game_interaction::select_mod_key_sequence(
                group_index,
                mod_index,
            )
            .await
        })
        .await
        .map_err(|e| match e {
            TaskQueueError::TaskCancelled(t) => format!("Task '{}' was cancelled", t),
            TaskQueueError::ExecutionError(e) => format!("Task execution failed: {}", e),
        })?;
    Ok(())
}

/// 检查单个 INI 文件的语法错误。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: INI 文件路径。
///
/// 返回：`ErroredLinesReport` 错误检测报告。
#[tauri::command]
pub async fn check_ini_syntax(
    state: State<'_, AppState>,
    path: String,
) -> Result<crate::ini_handler::error_detection::ErroredLinesReport, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::ini_handler::error_detection::check_single_file(&path).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 在系统文件管理器中打开指定路径。
///
/// 对于目录：直接打开该目录（进入目录内部）；
/// 对于文件：打开所在目录并选中该文件。
/// Windows 下对文件使用 explorer /select, 选中目标；
/// Linux 下文件场景使用 xdg-open 打开所在目录。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 要打开的文件或目录路径。
///
/// 返回：`Ok(())` 表示命令执行成功。
#[tauri::command]
pub async fn open_path(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let _ = state;

    if path.trim().is_empty() {
        return Err("Path is empty, cannot open".to_string());
    }

    let path_buf = PathBuf::from(&path);

    if !path_buf.exists() {
        return Err(format!("Path does not exist: {}", path));
    }

    let is_dir = path_buf.is_dir();

    tokio::task::spawn_blocking(move || {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if is_dir {
                std::process::Command::new("explorer")
                    .arg(&path_buf)
                    .spawn()
                    .map_err(|e| format!("Failed to open explorer: {}", e))?;
            } else {
                let cmd_line = format!(r#"/select,"{}""#, path_buf.display());
                std::process::Command::new("explorer")
                    .raw_arg(&cmd_line)
                    .spawn()
                    .map_err(|e| format!("Failed to open explorer: {}", e))?;
            }
        }

        #[cfg(target_os = "linux")]
        {
            let target = if is_dir {
                path_buf.clone()
            } else {
                path_buf
                    .parent()
                    .unwrap_or_else(|| Path::new("/"))
                    .to_path_buf()
            };
            std::process::Command::new("xdg-open")
                .arg(&target)
                .spawn()
                .map_err(|e| format!("Failed to open xdg-open: {}", e))?;
        }

        #[cfg(not(any(windows, target_os = "linux")))]
        {
            return Err("Unsupported platform".to_string());
        }

        Ok(())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 打开指定游戏的 Mods 目录。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `game`: 目标游戏字符串标识。
///
/// 返回：`Ok(())` 表示命令执行成功。
#[tauri::command]
pub async fn open_mod_folder(state: State<'_, AppState>, game: String) -> Result<(), String> {
    use crate::mod_manager::ModManager;
    use crate::process::TargetGame;

    let target_game = match game.as_str() {
        "Wuthering_Waves" | "WutheringWaves" => TargetGame::WutheringWaves,
        "Genshin_Impact" | "GenshinImpact" => TargetGame::GenshinImpact,
        "Honkai_Star_Rail" | "HonkaiStarRail" => TargetGame::HonkaiStarRail,
        "Zenless_Zone_Zero" | "ZenlessZoneZero" => TargetGame::ZenlessZoneZero,
        "Arknights_Endfield" | "ArknightsEndfield" => TargetGame::ArknightsEndfield,
        _ => TargetGame::None,
    };

    let mods_path = {
        let settings = state.settings.read();
        ModManager::get_mods_path_for_game(&settings, target_game)
    };

    let path_buf = PathBuf::from(&mods_path);
    if !path_buf.exists() {
        return Err(format!("Mods folder does not exist: {}", mods_path));
    }

    tokio::task::spawn_blocking(move || {
        #[cfg(windows)]
        {
            std::process::Command::new("explorer")
                .arg(&path_buf)
                .spawn()
                .map_err(|e| format!("Failed to open explorer: {}", e))?;
        }

        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open")
                .arg(&path_buf)
                .spawn()
                .map_err(|e| format!("Failed to open xdg-open: {}", e))?;
        }

        #[cfg(not(any(windows, target_os = "linux")))]
        {
            return Err("Unsupported platform".to_string());
        }

        Ok(())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 检查当前游戏 Mods 目录下所有 INI 文件的语法错误。
///
/// 参数：
/// - `state`: 应用全局状态。
///
/// 返回：`ErroredLinesReport` 错误检测报告。
/// 错误：未配置 Mods 路径时返回 `"No mods path configured"`。
#[tauri::command]
pub async fn check_all_mods_syntax(
    state: State<'_, AppState>,
) -> Result<crate::ini_handler::error_detection::ErroredLinesReport, String> {
    let mods_path = {
        let settings = state.settings.read();
        let path =
            crate::mod_manager::ModManager::get_mods_path_for_game(&settings, settings.target_game);

        if path.is_empty() {
            return Err("No mods path configured".to_string());
        }
        path
    };

    let known_lib_namespaces: Vec<String> = Vec::new();

    tokio::task::spawn_blocking(move || {
        crate::ini_handler::error_detection::check_all_errors(&mods_path, &known_lib_namespaces)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 打开目录选择对话框（使用后端命令确保权限正确）。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
///
/// 返回：
/// - `Some(路径字符串)`：用户选择了目录。
/// - `None`：用户取消了选择。
#[tauri::command]
pub async fn select_directory(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    // 将同步阻塞的对话框调用放到 spawn_blocking 中，避免阻塞 tokio 工作线程
    let result = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Select Mods Folder")
            .blocking_pick_folder()
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?;

    match result {
        Some(path) => {
            let path_str = path.to_string();
            log::info!("Directory selected: {}", path_str);
            Ok(Some(path_str))
        }
        None => {
            log::debug!("Directory selection cancelled");
            Ok(None)
        }
    }
}

/// 从指定路径添加 Mod（复制文件/目录到目标分组目录）。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `source_paths`: 源文件/目录路径列表。
/// - `target_group_path`: 目标分组目录路径。
///
/// 返回：`true` 表示添加成功。
#[tauri::command]
pub async fn add_mods(
    state: State<'_, AppState>,
    source_paths: Vec<String>,
    target_group_path: String,
) -> Result<bool, String> {
    state
        .mod_manager
        .add_mods(source_paths, &target_group_path)
        .await
        .map_err(|e| e.to_string())
}

/// 使用 BFS（广度优先搜索）算法查找指定路径下的所有 `.ini` 文件。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 起始路径（文件或目录）。
///
/// 返回：INI 文件路径列表。
#[tauri::command]
pub async fn find_ini_files(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<String>, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        let path = Path::new(&path);
        let ini_files = crate::mod_manager::ModManager::find_ini_files_bfs(path);
        Ok(ini_files
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 处理 INI 文件，移除 xxmi 专属的 INI 语句。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `paths`: INI 文件路径列表。
///
/// 返回：是否处理成功。
#[tauri::command]
pub async fn process_ini_files(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<bool, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::process_ini_files(&paths).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 验证压缩文件的有效性。
///
/// 验证策略：
/// 1. 优先检查文件扩展名（.zip, .7z, .rar）
/// 2. 同时检查文件头魔数确保文件格式真实性
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 文件路径。
///
/// 返回：(是否有效, 文件类型字符串: "zip"/"7z"/"rar"/"unknown")。
#[tauri::command]
pub async fn validate_archive_file(
    state: State<'_, AppState>,
    path: String,
) -> Result<(bool, String), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        let (valid, archive_type) =
            crate::mod_manager::ModManager::validate_archive_file(Path::new(&path));
        let type_str = match archive_type {
            crate::mod_manager::ArchiveType::Zip => "zip",
            crate::mod_manager::ArchiveType::SevenZip => "7z",
            crate::mod_manager::ArchiveType::Rar => "rar",
            crate::mod_manager::ArchiveType::Unknown => "unknown",
        };
        Ok((valid, type_str.to_string()))
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 使用 BFS 算法递归查找目录下所有文件。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 起始目录路径。
///
/// 返回：目录下所有文件的路径列表。
#[tauri::command]
pub async fn find_all_files(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<String>, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        let files = crate::mod_manager::ModManager::find_all_files_bfs(Path::new(&path));
        Ok(files
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 检测归档文件是否加密（需要密码才能解压）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 归档文件路径。
///
/// 返回：是否需要密码。
#[tauri::command]
pub async fn is_archive_encrypted(
    state: State<'_, AppState>,
    path: String,
) -> Result<bool, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::is_archive_encrypted(Path::new(&path))
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 解压压缩文件到指定目录（自动识别文件类型，支持可选密码）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `file_path`: 压缩文件路径。
/// - `dest_dir`: 目标目录路径。
/// - `password`: 可选解压密码。
///
/// 返回：是否解压成功。
#[tauri::command]
pub async fn extract_archive(
    state: State<'_, AppState>,
    file_path: String,
    dest_dir: String,
    password: Option<String>,
) -> Result<bool, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        let pwd = password.as_deref();
        crate::mod_manager::ModManager::extract_archive(Path::new(&file_path), Path::new(&dest_dir), pwd)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 将文件移动到系统回收站。
///
/// 参数：
/// - `file_path`: 要移入回收站的文件路径。
///
/// 返回：操作是否成功。
#[tauri::command]
pub async fn move_to_trash(
    state: State<'_, AppState>,
    file_path: String,
) -> Result<bool, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::move_to_trash(Path::new(&file_path))
            .map(|_| true)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 导出单个模组为 7z 压缩文件。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `mod_path`: 模组目录路径。
/// - `dest_dir`: 目标目录路径。
///
/// 返回：导出文件的完整路径。
#[tauri::command]
pub async fn export_mod(
    state: State<'_, AppState>,
    mod_path: String,
    dest_dir: String,
) -> Result<String, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::export_mod(&mod_path, &dest_dir).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 导出分组模组为 7z 压缩文件（保持目录结构）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `group_path`: 分组目录路径。
/// - `dest_dir`: 目标目录路径。
///
/// 返回：导出文件的完整路径。
#[tauri::command]
pub async fn export_group(
    state: State<'_, AppState>,
    group_path: String,
    dest_dir: String,
) -> Result<String, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::export_group(&group_path, &dest_dir)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 在系统默认浏览器中打开指定 URL。
///
/// 参数：
/// - `url`: 要打开的 URL 字符串（必须包含协议，如 `https://`）。
///
/// 返回：成功返回 `Ok(())`，失败返回 `Err(String)` 错误信息。
#[tauri::command]
pub async fn open_url(url: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        open::that(&url).map_err(|e| format!("Failed to open URL: {}", e))
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 创建桌面快捷方式（Linux 为 .desktop 文件，Windows 为 .lnk 文件）。
///
/// 参数：
/// - `name`: 可选的快捷方式显示名称，未提供时使用默认程序名 "NRMM-Rust"。
///
/// 返回：成功返回 Ok(())，若文件已存在则返回错误信息。
#[tauri::command]
pub async fn create_desktop_icon(name: Option<String>) -> Result<(), String> {
    crate::desktop_entry::DesktopEntryManager::create_desktop_entry(name)
}

// =========================================================================
// 自更新：官方 tauri-plugin-updater 的透传命令 + 版本信息
// =========================================================================

#[derive(serde::Serialize, Clone)]
pub struct VersionInfo {
    pub version: String,
    pub commit: Option<String>,
    pub build_date: Option<String>,
}

/// 返回当前应用的版本号（Cargo.toml package.version），供前端 TitleBar/Settings 显示。
#[tauri::command]
pub fn get_version_info() -> VersionInfo {
    VersionInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: option_env!("GIT_COMMIT").map(|s| s.to_string()),
        build_date: option_env!("BUILD_DATE").map(|s| s.to_string()).or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| format!("{}", d.as_secs()))
                .ok()
        }),
    }
}

// =========================================================================
// 自更新（Updater）- 通过 Gitee Releases API 手动检查更新，不依赖 tauri-plugin-updater endpoints
// =========================================================================

const GITEE_API_URL: &str = "https://gitee.com/api/v5/repos/Yezi26/nrmm-tauri/releases/latest";

/// 更新下载缓存：避免 check 和 download 之间重复请求 Gitee API
pub struct UpdateCache {
    pub download_url: parking_lot::Mutex<Option<String>>,
    pub version: parking_lot::Mutex<Option<String>>,
    pub body: parking_lot::Mutex<Option<String>>,
    pub date: parking_lot::Mutex<Option<String>>,
}

impl Default for UpdateCache {
    fn default() -> Self {
        Self {
            download_url: parking_lot::Mutex::new(None),
            version: parking_lot::Mutex::new(None),
            body: parking_lot::Mutex::new(None),
            date: parking_lot::Mutex::new(None),
        }
    }
}

/// Gitee Releases API 返回的单个资产信息
#[derive(Debug, Deserialize)]
struct GiteeAsset {
    name: String,
    browser_download_url: String,
    #[allow(dead_code)]
    size: Option<u64>,
}

/// Gitee Releases API /releases/latest 响应（只解析需要的字段）
#[derive(Debug, Deserialize)]
struct GiteeRelease {
    tag_name: String,
    #[allow(dead_code)]
    name: Option<String>,
    body: Option<String>,
    created_at: Option<String>,
    #[allow(dead_code)]
    prerelease: bool,
    assets: Vec<GiteeAsset>,
}

/// 比较两个 semver 版本字符串（如 "0.3.0" vs "0.1.1"，支持带 v 前缀）
fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    fn parse_parts(v: &str) -> Option<Vec<u64>> {
        let stripped = v.trim().trim_start_matches('v');
        let parts: Vec<u64> = stripped
            .split('.')
            .map(|s| s.parse::<u64>().ok())
            .collect::<Option<Vec<_>>>()?;
        if parts.is_empty() || parts.len() > 4 {
            return None;
        }
        Some(parts)
    }
    let pa = parse_parts(a)?;
    let pb = parse_parts(b)?;
    let max_len = pa.len().max(pb.len());
    for i in 0..max_len {
        let ai = pa.get(i).copied().unwrap_or(0);
        let bi = pb.get(i).copied().unwrap_or(0);
        match ai.cmp(&bi) {
            Ordering::Equal => continue,
            ord => return Some(ord),
        }
    }
    Some(Ordering::Equal)
}

/// 根据当前平台从资产列表中匹配安装包
fn find_installer_asset(assets: &[GiteeAsset]) -> Option<&GiteeAsset> {
    #[cfg(target_os = "windows")]
    {
        // Windows: 优先 NSIS 安装包（.exe 安装程序），回退到任意 .exe
        let mut best: Option<&GiteeAsset> = None;
        for a in assets {
            let lower = a.name.to_lowercase();
            if lower.ends_with(".nsis.zip") || lower.contains("setup") && lower.ends_with(".exe") {
                best = Some(a);
                break;
            }
        }
        if best.is_none() {
            for a in assets {
                let lower = a.name.to_lowercase();
                if lower.ends_with(".exe") && !lower.contains("debug") {
                    best = Some(a);
                    break;
                }
            }
        }
        best
    }
    #[cfg(target_os = "linux")]
    {
        // Linux: 优先 AppImage，回退到 deb
        let mut best: Option<&GiteeAsset> = None;
        for a in assets {
            if a.name.to_lowercase().ends_with(".appimage") {
                best = Some(a);
                break;
            }
        }
        if best.is_none() {
            for a in assets {
                let lower = a.name.to_lowercase();
                if lower.ends_with(".deb") && (lower.contains("amd64") || lower.contains("x86_64")) {
                    best = Some(a);
                    break;
                }
            }
        }
        best
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

/// 从 Gitee API 获取最新 Release 信息
async fn fetch_latest_release() -> Result<GiteeRelease, String> {
    let client = reqwest::Client::builder()
        .user_agent("nrmm-rust-updater")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .get(GITEE_API_URL)
        .send()
        .await
        .map_err(|e| format!("网络请求失败，请检查网络连接。{}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        // Gitee API 错误可能返回中文 HTML（如 404 页面），截取前 200 字符即可
        let snippet: String = text.chars().take(200).collect();
        return Err(format!("Gitee API 返回错误 (HTTP {}): {}", status.as_u16(), snippet));
    }

    let release: GiteeRelease = resp
        .json()
        .await
        .map_err(|e| format!("解析 Gitee API 响应失败: {}", e))?;

    Ok(release)
}

/// 触发一次更新检查（异步）；
/// 返回是否发现可用更新（true = 有更新；false = 无；Err 串描述失败原因）。
#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    cache: State<'_, UpdateCache>,
) -> Result<bool, String> {
    let release = fetch_latest_release().await?;

    let remote_version = release.tag_name.trim().trim_start_matches('v').to_string();
    let current_version = env!("CARGO_PKG_VERSION");

    let cmp = compare_versions(&remote_version, current_version)
        .ok_or_else(|| format!("无法解析版本号: 远程='{}', 当前='{}'", remote_version, current_version))?;

    // 预发布版本忽略（可选，这里默认不忽略 prerelease 标记，因为 Gitee 上都是正式发布）
    if cmp != Ordering::Greater {
        // 远程版本 <= 当前版本，已是最新
        // 清除缓存
        *cache.download_url.lock() = None;
        *cache.version.lock() = None;
        *cache.body.lock() = None;
        *cache.date.lock() = None;
        return Ok(false);
    }

    // 有新版本，查找安装包
    let asset = find_installer_asset(&release.assets)
        .ok_or_else(|| format!("版本 {} 未找到适用于当前平台的安装包", remote_version))?;

    let notes = release.body.clone().unwrap_or_default();
    let date = release.created_at.clone().unwrap_or_default();

    // 缓存更新信息
    *cache.download_url.lock() = Some(asset.browser_download_url.clone());
    *cache.version.lock() = Some(remote_version.clone());
    *cache.body.lock() = Some(notes.clone());
    *cache.date.lock() = Some(date.clone());

    log::info!(
        "[updater] New version found: v{} (current: v{}), asset: {}",
        remote_version, current_version, asset.name
    );

    let _ = app.emit(
        "tauri://update-available",
        serde_json::json!({
            "version": remote_version,
            "body": notes,
            "date": date,
        }),
    );

    Ok(true)
}

/// 触发 下载 + 安装（后台异步执行，通过事件推送进度给前端）。
/// 本命令 spawn 任务后立即返回 Ok(())，不阻塞前端 UI。
#[tauri::command]
pub async fn download_and_install_update(
    app: AppHandle,
    cache: State<'_, UpdateCache>,
) -> Result<(), String> {
    // 获取下载 URL（优先从缓存取，缓存为空则重新请求 API）
    // 注意：必须在 await 前释放 MutexGuard（parking_lot::MutexGuard 不是 Send）
    let (download_url, version_str) = {
        let cached_url = cache.download_url.lock().clone();
        let cached_ver = cache.version.lock().clone();
        if let (Some(url), Some(ver)) = (cached_url, cached_ver) {
            (url, ver)
        } else {
            log::info!("[updater] Cache empty, re-fetching release info...");
            let release = fetch_latest_release().await?;
            let remote_version = release.tag_name.trim().trim_start_matches('v').to_string();
            let asset = find_installer_asset(&release.assets)
                .ok_or_else(|| format!("版本 {} 未找到适用于当前平台的安装包", remote_version))?;
            let notes = release.body.unwrap_or_default();
            let date = release.created_at.unwrap_or_default();
            *cache.download_url.lock() = Some(asset.browser_download_url.clone());
            *cache.version.lock() = Some(remote_version.clone());
            *cache.body.lock() = Some(notes);
            *cache.date.lock() = Some(date);
            (asset.browser_download_url.clone(), remote_version)
        }
    };

    let app_c = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = do_download_and_install(app_c.clone(), &download_url, &version_str).await {
            log::error!("[updater] Download/Install failed: {}", e);
            let _ = app_c.emit(
                "tauri://update-status",
                serde_json::json!({
                    "status": "error",
                    "error": e,
                }),
            );
        }
    });

    Ok(())
}

/// 实际的下载+安装逻辑（在 spawned task 中执行）
async fn do_download_and_install(
    app: AppHandle,
    download_url: &str,
    version_str: &str,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    log::info!("[updater] Starting download: v{} from {}", version_str, download_url);

    let _ = app.emit(
        "tauri://update-status",
        serde_json::json!({
            "status": "downloading",
            "version": version_str,
        }),
    );

    // 创建 HTTP 客户端并发起请求
    let client = reqwest::Client::builder()
        .user_agent("nrmm-rust-updater")
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .get(download_url)
        .send()
        .await
        .map_err(|e| format!("下载请求失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("下载失败，服务器返回 HTTP {}", resp.status().as_u16()));
    }

    let total_size = resp.content_length();
    log::info!(
        "[updater] Download started, total size: {} bytes",
        total_size.unwrap_or(0)
    );

    // 通知前端总大小
    if let Some(total) = total_size {
        let _ = app.emit(
            "tauri://update-download-progress",
            serde_json::json!({
                "chunkLength": 0,
                "contentLength": total,
            }),
        );
    }

    // 创建临时文件
    let ext = if cfg!(windows) { ".exe" } else { ".AppImage" };
    let temp_dir = std::env::temp_dir();
    let installer_name = format!("nrmm-rust-update-{}{}", version_str, ext);
    let installer_path = temp_dir.join(&installer_name);

    let mut file = tokio::fs::File::create(&installer_path)
        .await
        .map_err(|e| format!("创建临时文件失败: {}", e))?;

    let mut downloaded: u64 = 0;
    let mut last_emit = std::time::Instant::now();

    // 流式下载
    let mut response = resp;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("下载数据失败: {}", e))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入临时文件失败: {}", e))?;
        downloaded += chunk.len() as u64;

        // 每 200ms 或 每 256KB 发射一次进度事件，避免事件风暴
        let now = std::time::Instant::now();
        if now.duration_since(last_emit).as_millis() > 200 || chunk.len() > 256 * 1024 {
            last_emit = now;
            let _ = app.emit(
                "tauri://update-download-progress",
                serde_json::json!({
                    "chunkLength": chunk.len(),
                    "contentLength": total_size,
                }),
            );
        }
    }
    file.flush().await.map_err(|e| format!("刷新文件失败: {}", e))?;
    drop(file);

    log::info!(
        "[updater] Download complete: {} bytes, saved to {:?}",
        downloaded, installer_path
    );

    // 发射 installing 状态
    let _ = app.emit(
        "tauri://update-status",
        serde_json::json!({
            "status": "installing",
            "version": version_str,
        }),
    );

    // 执行安装
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // Windows NSIS 安装包: /S 静默安装，安装完后自动启动新版本
        // CREATE_NO_WINDOW (0x08000000) 防止弹出命令行窗口
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let status = std::process::Command::new(&installer_path)
            .arg("/S")
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();

        match status {
            Ok(_child) => {
                log::info!("[updater] Installer launched successfully, restarting app...");
                let _ = app.emit(
                    "tauri://update-status",
                    serde_json::json!({
                        "status": "done",
                    }),
                );
                // 延迟一点再重启，确保安装程序启动
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    tauri::process::restart(&app.env());
                });
            }
            Err(e) => {
                return Err(format!("启动安装程序失败: {}", e));
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        // Linux: 给 AppImage 添加执行权限并启动
        if let Err(e) = std::fs::set_permissions(&installer_path, std::fs::Permissions::from_mode(0o755)) {
            log::warn!("[updater] Failed to set executable permission: {}", e);
        }
        match std::process::Command::new(&installer_path).spawn() {
            Ok(_child) => {
                log::info!("[updater] AppImage launched, restarting app...");
                let _ = app.emit(
                    "tauri://update-status",
                    serde_json::json!({
                        "status": "done",
                    }),
                );
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    tauri::process::restart(&app.env());
                });
            }
            Err(e) => {
                return Err(format!("启动安装程序失败: {}", e));
            }
        }
    }

    Ok(())
}

/// 重启应用（安装完更新后由前端弹窗确认后调用）。
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    tauri::process::restart(&app.env());
}

