//! 文件监控命令域
//!
//! 从 core/file_watcher.rs 迁出（2026-09-19 Phase 6-3）。
//! 承载文件监控器的启停 / 切换 / 状态查询命令。
//! 暂停/恢复已无 IPC 出口（批次 3 摘除 `pause_file_watcher` / `resume_file_watcher`，
//! 前端从未调用）；`FileWatcher::pause/resume` 仍由导入流程在命令层内部成对使用。
//! `FileWatcher` 作为 managed state 由 lib.rs `.manage()` 注入，跨模块 `tauri::State` 提取合法。

use crate::core::watcher_core::FileWatcher;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::AppHandle;

/// 停止文件监控 Tauri 命令
///
/// 前端调用 `stopFileWatcher` 触发。
/// `thread::join` 可能阻塞，使用 `spawn_blocking` 避免阻塞 async 运行时。
///
/// # Errors
/// - 锁获取失败（Mutex  poisoned）
/// - 线程 join 失败
#[tauri::command]
pub async fn stop_file_watcher(
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
) -> Result<(), String> {
    let watcher_arc = watcher.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let mut w = watcher_arc.lock().map_err(|e| e.to_string())?;
        w.stop_watching();
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 切换监控路径 Tauri 命令
///
/// 前端调用 `switchFileWatcher` 触发。
/// 涉及 IO 和线程操作，使用 `spawn_blocking`。
///
/// # Errors
/// - 同 `start_watching`：目录创建失败或 watcher 初始化失败
#[tauri::command]
pub async fn switch_file_watcher(
    app_handle: AppHandle,
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
    mods_path: String,
) -> Result<(), String> {
    let watcher_arc = watcher.inner().clone();
    let mods_path_buf = PathBuf::from(mods_path);
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let mut w = watcher_arc.lock().map_err(|e| e.to_string())?;
        w.switch_watched_path(app_handle, &mods_path_buf)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 检查文件监控是否正在运行（Tauri 命令）
///
/// 前端调用 `isFileWatcherRunning` 触发，用于 `window-shown` 时判断监控是否需要重启。
/// 例如窗口从隐藏到显示时，如果监控已停止则需要重新启动。
#[tauri::command]
pub fn is_file_watcher_running(watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>) -> bool {
    watcher.lock().map(|w| w.is_running()).unwrap_or(false)
}

/// 获取当前监控的 `_MANAGED_` 路径（Tauri 命令）
///
/// 前端调用 `currentWatchedPath` 触发，供前端对比是否需要切换监控路径。
/// 例如用户切换游戏时，前端调用此命令获取当前路径并与新路径比较。
#[tauri::command]
pub fn current_watched_path(watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>) -> Option<String> {
    watcher
        .lock()
        .ok()
        .and_then(|w| w.watched_path().map(|p| p.to_string_lossy().into_owned()))
}
