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
use crate::models::settings::AppSettings;
use crate::models::enums::TargetGame;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

/// 获取设置（从内存读取，无IO，保持同步）
#[tauri::command]
pub fn get_settings() -> AppSettings {
    settings_store::get_settings()
}

/// 保存设置（写文件是阻塞IO，使用spawn_blocking）
#[tauri::command]
pub async fn save_settings(settings: AppSettings) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        settings_store::save_settings(&settings).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
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
    let path_buf = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        settings_store::export_settings(&path_buf).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 导入设置（读+写文件是阻塞IO，使用spawn_blocking）
#[tauri::command]
pub async fn import_settings(path: String) -> Result<AppSettings, String> {
    let path_buf = PathBuf::from(path);
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
        let _ = app_clone.emit("target-game-switched", game);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

