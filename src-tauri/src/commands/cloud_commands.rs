//! 云端数据刷新命令域
//!
//! 从 core/cloud_data.rs 迁出（2026-09-19 Phase 6-4）。
//! 承载单文件刷新与全量刷新命令，底层经 `CloudDataManager` 执行异步刷新。

use crate::config::settings_store;
use crate::core::cloud_data::CloudDataManager;

/// Tauri 命令：刷新单个云端数据文件
#[tauri::command]
pub async fn refresh_cloud_data(file_name: String) -> Result<(), String> {
    // 路径边界校验（R1）：file_name 用于 join 落盘到缓存目录，必须是合法文件名
    // 拒绝含 / \ .. \0 的输入，防止路径穿越写出缓存目录外
    settings_store::validate_filename(&file_name).map_err(|e| e.to_string())?;
    CloudDataManager::refresh_async(&file_name)
        .await
        .map_err(|e| e.to_string())
}

/// Tauri 命令：刷新所有云端数据文件
///
/// 返回错误列表（空列表表示全部成功）
#[tauri::command]
pub async fn refresh_all_cloud_data() -> Result<Vec<String>, String> {
    let results = CloudDataManager::refresh_all_async().await;
    let mut errors = Vec::new();
    for (name, res) in results {
        if let Err(e) = res {
            errors.push(format!("{}: {}", name, e));
        }
    }
    if errors.is_empty() {
        Ok(Vec::new())
    } else {
        Ok(errors)
    }
}