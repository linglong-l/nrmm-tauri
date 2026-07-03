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
use std::path::{Path, PathBuf};

use tauri::{AppHandle, State};

use crate::mod_manager::{ModGroupData, ModsPathStatus, UpdateModDataResult};
use crate::state::AppState;
use crate::task_queue::TaskQueueError;

/// 加载模组数据。
///
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
            "Wuthering_Waves" => TargetGame::WutheringWaves,
            "Genshin_Impact" => TargetGame::GenshinImpact,
            "Honkai_Star_Rail" => TargetGame::HonkaiStarRail,
            "Zenless_Zone_Zero" => TargetGame::ZenlessZoneZero,
            "Arknights_Endfield" => TargetGame::ArknightsEndfield,
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
            "Wuthering_Waves" => TargetGame::WutheringWaves,
            "Genshin_Impact" => TargetGame::GenshinImpact,
            "Honkai_Star_Rail" => TargetGame::HonkaiStarRail,
            "Zenless_Zone_Zero" => TargetGame::ZenlessZoneZero,
            "Arknights_Endfield" => TargetGame::ArknightsEndfield,
            _ => TargetGame::None,
        };
        settings.target_game = target;
    }
    // 克隆参数以满足 'static 生命周期要求
    let mod_manager = state.mod_manager.clone();
    let settings_clone = settings.clone();

    Ok(state
        .task_queue
        .run_task("load_mods", async move {
            mod_manager.refresh_mods(&settings_clone).await
        })
        .await
        .map_err(|e| match e {
            TaskQueueError::TaskCancelled(t) => format!("Task '{}' was cancelled", t),
            TaskQueueError::ExecutionError(e) => format!("Task execution failed: {}", e),
        })?)
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
/// - `mods_path`: Mods 根目录路径。
/// - `known_libraries`: 已知模组库命名空间映射。
///
/// 返回：`UpdateModDataResult`，包含成功状态、日志、耗时及各项检测报告。
#[tauri::command]
pub async fn update_mod_data(
    _app: AppHandle,
    state: State<'_, AppState>,
    mods_path: String,
    known_libraries: HashMap<String, String>,
) -> Result<UpdateModDataResult, String> {
    // 克隆参数以满足 'static 生命周期要求
    let mod_manager = state.mod_manager.clone();
    let mods_path_clone = mods_path.clone();
    let known_libraries_clone = known_libraries.clone();

    Ok(state
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
        })?)
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

/// 切换模组的启用/禁用状态（通过重命名添加/移除 `DISABLED` 前缀）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `mod_path`: 模组目录路径。
///
/// 返回：操作后该模组是否处于禁用状态（`true` 表示已禁用）。
#[tauri::command]
pub async fn toggle_mod_disabled(
    state: State<'_, AppState>,
    mod_path: String,
) -> Result<bool, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::toggle_mod_disabled(&mod_path).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 切换树节点（# 目录）下模组的启用/禁用状态（互斥模式）。
///
/// 与普通 `toggle_mod_disabled` 的区别：
/// - 启用操作：先禁用同 # 目录下所有其他模组，再启用目标模组（单选互斥）。
/// - 禁用操作：直接禁用目标模组，不影响其他模组。
/// - 不涉及 INI 文件修改，纯靠目录重命名实现。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `mod_path`: 目标模组目录路径。
///
/// 返回：`(新模组路径, 操作后是否禁用)`。
#[tauri::command]
pub async fn toggle_tree_node_mod_disabled(
    state: State<'_, AppState>,
    mod_path: String,
) -> Result<(String, bool), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::toggle_tree_node_mod_disabled(&mod_path)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}

/// 在 `_MANAGED_` 目录下新建一个分组。
///
/// 从全局设置中获取当前游戏的 Mods 路径，自动寻找最小可用索引创建分组。
///
/// 参数：
/// - `state`: 应用全局状态。
/// - `group_name`: 新分组的显示名称。
///
/// 返回：新分组的索引。
/// 错误：未配置 Mods 路径时返回 `"No mods path configured"`。
#[tauri::command]
pub async fn add_group(state: State<'_, AppState>, group_name: String) -> Result<i32, String> {
    let managed_path = {
        let settings = state.settings.read();
        let mods_path =
            crate::mod_manager::ModManager::get_mods_path_for_game(&settings, settings.target_game);

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
        let mods_in_group =
            ModManager::get_mods_on_group(&group_path_str).map_err(|e| e.to_string())?;
        let mods_count = mods_in_group.len();
        let previous_selected_mod_on_group =
            ModManager::get_selected_mod_in_group(&group_path_str, mods_count).unwrap_or(0);

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
    crate::hotkey::HotkeyManager::register_hotkey(&app, &key).map_err(|e| e.to_string())
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
    crate::hotkey::HotkeyManager::unregister_hotkey(&app, &key).map_err(|e| e.to_string())
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
    crate::hotkey::HotkeyManager::unregister_all(&app).map_err(|e| e.to_string())
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
    crate::window_manager::WindowManager::toggle_window(&app).map_err(|e| e.to_string())
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
    state: State<'_, AppState>,
    process_name: String,
) -> Result<bool, String> {
    state
        .process_detector
        .is_process_running(&process_name)
        .await
        .map_err(|e| e.to_string())
}

/// 获取当前系统中所有正在运行的进程名称列表。
///
/// 参数：
/// - `state`: 应用全局状态。
///
/// 返回：进程名称字符串列表。
#[tauri::command]
pub async fn get_process_list(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    state
        .process_detector
        .get_process_list()
        .await
        .map_err(|e| e.to_string())
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

    Ok(game.as_str().to_string())
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

/// 保存设置到内存并持久化到文件。
///
/// 参数：
/// - `app`: Tauri 应用句柄。
/// - `state`: 应用全局状态。
/// - `settings`: 新的设置内容。
#[tauri::command]
pub async fn save_settings(
    _app: tauri::AppHandle,
    state: State<'_, AppState>,
    settings: crate::settings::Settings,
) -> Result<(), String> {
    let app_data_dir =
        crate::get_app_data_dir().ok_or_else(|| "Failed to get app data dir".to_string())?;

    {
        let mut current = state.settings.write();
        *current = settings.clone();
    }

    let settings_arc = state.settings.clone();
    let app_data_dir_clone = app_data_dir.clone();
    tokio::spawn(async move {
        let settings_clone = {
            let settings = settings_arc.read();
            settings.clone()
        };
        if let Err(e) = settings_clone.save_async(&app_data_dir_clone).await {
            log::error!("Failed to save settings: {}", e);
        }
    });

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
/// Windows 下使用 explorer /select, 打开并选中目标文件/文件夹；
/// Linux 下使用 xdg-open 打开所在文件夹。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `path`: 要打开的文件或目录路径。
///
/// 返回：`Ok(())` 表示命令执行成功。
#[tauri::command]
pub async fn open_path(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let _ = state;
    let path_buf = PathBuf::from(&path);

    if !path_buf.exists() {
        return Err(format!("Path does not exist: {}", path));
    }

    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path_buf.display()))
            .spawn()
            .map_err(|e| format!("Failed to open explorer: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        let parent = path_buf
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf();
        std::process::Command::new("xdg-open")
            .arg(&parent)
            .spawn()
            .map_err(|e| format!("Failed to open xdg-open: {}", e))?;
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    {
        return Err("Unsupported platform".to_string());
    }

    Ok(())
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
        "Wuthering_Waves" => TargetGame::WutheringWaves,
        "Genshin_Impact" => TargetGame::GenshinImpact,
        "Honkai_Star_Rail" => TargetGame::HonkaiStarRail,
        "Zenless_Zone_Zero" => TargetGame::ZenlessZoneZero,
        "Arknights_Endfield" => TargetGame::ArknightsEndfield,
        _ => TargetGame::None,
    };

    let settings = state.settings.read();
    let mods_path = ModManager::get_mods_path_for_game(&settings, target_game);

    let path_buf = PathBuf::from(&mods_path);
    if !path_buf.exists() {
        return Err(format!("Mods folder does not exist: {}", mods_path));
    }

    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(&mods_path)
            .spawn()
            .map_err(|e| format!("Failed to open explorer: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&mods_path)
            .spawn()
            .map_err(|e| format!("Failed to open xdg-open: {}", e))?;
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    {
        return Err("Unsupported platform".to_string());
    }

    Ok(())
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
pub async fn find_ini_files(state: State<'_, AppState>, path: String) -> Result<Vec<String>, String> {
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
pub async fn process_ini_files(state: State<'_, AppState>, paths: Vec<String>) -> Result<bool, String> {
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
pub async fn validate_archive_file(state: State<'_, AppState>, path: String) -> Result<(bool, String), String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        let (valid, archive_type) = crate::mod_manager::ModManager::validate_archive_file(Path::new(&path));
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
pub async fn find_all_files(state: State<'_, AppState>, path: String) -> Result<Vec<String>, String> {
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

/// 解压压缩文件到指定目录（自动识别文件类型）。
///
/// 参数：
/// - `state`: 应用全局状态（当前未使用）。
/// - `file_path`: 压缩文件路径。
/// - `dest_dir`: 目标目录路径。
///
/// 返回：是否解压成功。
#[tauri::command]
pub async fn extract_archive(state: State<'_, AppState>, file_path: String, dest_dir: String) -> Result<bool, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::extract_archive(Path::new(&file_path), Path::new(&dest_dir))
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
pub async fn export_mod(state: State<'_, AppState>, mod_path: String, dest_dir: String) -> Result<String, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::export_mod(&mod_path, &dest_dir)
            .map_err(|e| e.to_string())
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
pub async fn export_group(state: State<'_, AppState>, group_path: String, dest_dir: String) -> Result<String, String> {
    let _ = state;
    tokio::task::spawn_blocking(move || {
        crate::mod_manager::ModManager::export_group(&group_path, &dest_dir)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
}
