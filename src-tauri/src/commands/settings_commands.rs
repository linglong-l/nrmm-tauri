//! 设置相关 Tauri 命令
//!
//! 提供应用设置的 CRUD 操作：
//! - get_settings: 从内存缓存读取设置（同步，无 IO）
//! - save_settings: 保存设置到磁盘（异步，spawn_blocking）
//! - reset_settings: 重置为默认设置
//! - export_settings/import_settings: 设置导入导出（JSON 格式）
//! - switch_target_game: 切换目标游戏并通知前端
//!
//! 所有涉及文件 IO 的操作都使用 spawn_blocking 避免阻塞 UI 线程

use crate::config::settings_store;
use crate::ipc::typed_paths::validate_write_target;
use crate::ipc::typed_settings::AppSettingsWritable;
use crate::models::enums::TargetGame;
use crate::models::settings::AppSettings;
use std::fs;
use std::path::Path;
use tauri::{AppHandle, Emitter};

/// 获取设置（从内存读取，无IO，保持同步）
#[tauri::command]
pub fn get_settings() -> AppSettings {
    settings_store::get_settings()
}

/// 保存设置（写文件是阻塞IO，使用spawn_blocking）
///
/// 入参用 [`AppSettingsWritable`]：`game_mods_path`（路径校验锚点）一律取服务端现值，
/// 前端经由本命令改写锚点会被忽略并留审计日志。改写 Mods 根请走
/// [`set_game_mods_path`]。
#[tauri::command]
pub async fn save_settings(settings: AppSettingsWritable) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        settings_store::save_settings(&settings.into_inner()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 为指定游戏设置 Mods 根目录（改写校验锚点的唯一入口）
///
/// 由前端在原生文件夹对话框中选定的路径提交（保持既有交互不变），但后端在此处完成
/// 三件原本散落在「通用保存」里做不到的事：目录存在性与类型校验、锚点登记、以及仅
/// 更新该单一字段（其余设置不受这次调用的影响）。
///
/// # Errors
/// - `game` 不是受支持的游戏标识
/// - `path` 非绝对路径 / 不存在 / 不是目录
/// - 写盘失败
///
/// # 返回
/// 更新后的完整设置（含新锚点，供前端直接刷新本地状态）
#[tauri::command]
pub async fn set_game_mods_path(game: String, path: String) -> Result<AppSettings, String> {
    let game_enum = crate::commands::mod_commands_util::parse_game(&game)?;
    let p = Path::new(&path);
    if !p.is_absolute() {
        return Err(format!(
            "[PATH_NOT_ABSOLUTE] mods path must be absolute: {path}"
        ));
    }
    let canon = fs::canonicalize(p)
        .map_err(|e| format!("[PATH_NOT_FOUND] cannot resolve mods path {path}: {e}"))?;
    if !canon.is_dir() {
        return Err(format!(
            "[NOT_A_DIRECTORY] mods path is not a directory: {canon:?}"
        ));
    }
    // 先登记锚点，保证「设置已应用」副作用之外的同期命令也能立刻看到新根
    crate::ipc::mods_root_registry::register(&canon).map_err(|e| e.to_string())?;

    let updated = settings_store::update_settings(|s| {
        s.game_mods_path
            .insert(game_enum, canon.to_string_lossy().into_owned());
    })
    .map_err(|e| e.to_string())?;

    log::info!("[AUDIT] {game_enum:?} 的 Mods 根已登记：{canon:?}");
    Ok(updated)
}

/// 重置设置（写文件是阻塞IO，使用spawn_blocking）
#[tauri::command]
pub async fn reset_settings() -> Result<AppSettings, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<AppSettings, String> {
        settings_store::reset_settings().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 导出设置（写文件是阻塞IO，使用spawn_blocking）
#[tauri::command]
pub async fn export_settings(path: String) -> Result<(), String> {
    // 导出目标不受 Mods 根约束，但仍须绝对、无 ..、父目录存在（否则可创建未知层级）
    let path_buf = validate_write_target(&path).map_err(|(code, msg)| format!("{code} {msg}"))?;
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        settings_store::export_settings(&path_buf).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 导入设置（读+写文件是阻塞IO，使用spawn_blocking）
#[tauri::command]
pub async fn import_settings(
    path: crate::ipc::typed_paths::SourcePath,
) -> Result<AppSettings, String> {
    // 来源设置文件：绝对、存在（SourcePath 已保证），可为任意位置
    let path_buf = path.into_canonical();
    tauri::async_runtime::spawn_blocking(move || -> Result<AppSettings, String> {
        settings_store::import_settings(&path_buf).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 切换目标游戏（写设置文件是阻塞IO，使用spawn_blocking）
///
/// 切换成功后会发出 "target-game-switched" 事件通知前端刷新
#[tauri::command]
pub async fn switch_target_game(app: AppHandle, game: TargetGame) -> Result<(), String> {
    let app_clone = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        settings_store::set_target_game(game).map_err(|e| e.to_string())?;
        let _ = app_clone.emit(crate::core::events::TARGET_GAME_SWITCHED, game);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
