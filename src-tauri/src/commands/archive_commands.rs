//! 压缩包导入导出命令域
//!
//! 从 core/archive_handler.rs 迁出（2026-09-19 Phase 6-4）。
//! 承载压缩包支持检测、导入（含自动导入/批量导入）与导出、分辨率上限查询命令。
//! 底层解压/归档逻辑保留于 core/archive_handler.rs，此处仅薄适配。

use crate::core::archive_handler::{
    export_mod, import_item, import_mod, import_mod_auto, is_supported_archive, ExtractResult,
    ImportItemRequest,
};
use crate::core::resolution::{compute_limits, ResolutionLimits};
use crate::core::watcher_core::FileWatcher;
use crate::core::watcher_ring::WatcherGuard;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::State;

/// Tauri 命令：检查路径是否为支持的压缩包格式
///
/// 前端调用 `isSupportedArchive` 触发。
/// 使用 `is_supported_archive`（魔数优先 + 扩展名回退）进行鲁棒检测。
#[tauri::command]
pub fn is_supported_archive_cmd(path: String) -> bool {
    is_supported_archive(Path::new(&path))
}

/// Tauri 命令：导入模组压缩包到指定分组
///
/// 前端调用 `importMod` 触发。
/// 使用 `spawn_blocking` 在后台线程解压，避免阻塞 async 运行时。
///
/// # Errors
/// - 解压失败（格式不支持、7z 未找到、密码错误等）
#[tauri::command]
pub async fn import_mod_cmd(
    archive_path: String,
    group_dir: String,
    password: Option<String>,
) -> Result<ExtractResult, String> {
    // 路径边界校验（R1）：group_dir 必须位于任一已配置 mods_root 内
    // archive_path 是用户选择的压缩包来源，路径任意，不校验
    let group_dir = crate::config::settings_store::validate_managed_path(&group_dir)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .to_string();
    tauri::async_runtime::spawn_blocking(move || -> Result<ExtractResult, String> {
        import_mod(
            Path::new(&archive_path),
            Path::new(&group_dir),
            password.as_deref(),
        )
        .map_err(|e| e.to_string())
        .map(|result| {
            // 新导入目录补做 INI 备份（幂等，白名单同样生效，后台线程池队首优先执行）
            if let ExtractResult::Success { ref mod_path, .. } = result {
                crate::core::ini_backup_scheduler::enqueue_incremental(mod_path.clone());
            }
            result
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Tauri 命令：自动导入模组压缩包（自动选择第一个存在的分组）
///
/// 前端调用 `importModAuto` 触发。
/// 使用 `spawn_blocking` 在后台线程解压。
#[tauri::command]
pub async fn import_mod_auto_cmd(
    archive_path: String,
    mods_path: String,
    password: Option<String>,
) -> Result<ExtractResult, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<ExtractResult, String> {
        import_mod_auto(
            Path::new(&archive_path),
            Path::new(&mods_path),
            password.as_deref(),
        )
        .map_err(|e| e.to_string())
        .map(|result| {
            // 新导入目录补做 INI 备份（幂等，白名单同样生效，后台线程池队首优先执行）
            if let ExtractResult::Success { ref mod_path, .. } = result {
                crate::core::ini_backup_scheduler::enqueue_incremental(mod_path.clone());
            }
            result
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Tauri 命令：批量导入文件或目录（支持压缩包和已解压目录混合）
///
/// 前端调用 `importItem` 触发。
///
/// # 流程
/// 1. **暂停文件监控**：先锁定 FileWatcher 调用 `pause()`，再设置全局 `WATCHER_PAUSED = true`，避免导入过程中触发循环刷新
/// 2. **spawn_blocking 批量处理**：遍历 `req.items`，逐个调用 `import_item` 收集结果（失败时记录为 `ExtractResult::ExtractFailed`）
/// 3. **恢复文件监控**：无论成功与否，恢复全局 `WATCHER_PAUSED` 和实例级暂停标志
///
/// # 参数
/// - `watcher`: FileWatcher 状态（用于暂停/恢复监控）
/// - `req`: 批量导入请求体（包含路径列表、目标分组、可选密码）
///
/// # 返回
/// `Vec<ExtractResult>`，每个原输入对应一个结果
///
/// # Errors
/// - 批量处理失败时返回错误字符串
/// - 单个项失败不会导致整体失败，错误结果会包含在返回列表中
#[tauri::command]
pub async fn import_item_cmd(
    watcher: State<'_, Arc<Mutex<FileWatcher>>>,
    req: ImportItemRequest,
) -> Result<Vec<ExtractResult>, String> {
    let _busy = crate::window::lifecycle::BusyOpGuard::new();
    // 路径边界校验（R1）：target_group_dir 必须位于任一已配置 mods_root 内
    // items 是用户选择的源文件/压缩包，路径任意，不校验
    let mut req = req;
    req.target_group_dir =
        crate::config::settings_store::validate_managed_path(&req.target_group_dir)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string();
    // 暂停文件监控
    {
        let w = watcher.lock().map_err(|e| e.to_string())?;
        w.pause();
    }
    let _watcher_guard = WatcherGuard::new(); // 全局暂停（引用计数，函数结束自动恢复）

    let result = tauri::async_runtime::spawn_blocking(move || -> Vec<ExtractResult> {
        let target = PathBuf::from(&req.target_group_dir);
        let mut outs = Vec::with_capacity(req.items.len());
        for item in req.items {
            let p = PathBuf::from(&item);
            let res = import_item(&p, &target, req.password.as_deref());
            match res {
                Ok(r) => {
                    // 新导入目录补做 INI 备份（幂等，白名单同样生效，后台线程池队首优先执行）
                    if let ExtractResult::Success { ref mod_path, .. } = r {
                        crate::core::ini_backup_scheduler::enqueue_incremental(mod_path.clone());
                    }
                    outs.push(r);
                }
                Err(e) => outs.push(ExtractResult::ExtractFailed {
                    message: format!("{}: {}", item, e),
                }),
            }
        }
        outs
    })
    .await
    .map_err(|e| e.to_string());

    // 恢复文件监控（全局 WATCHER_PAUSED 由 _watcher_guard 在函数结束 Drop 时自动恢复）
    if let Ok(w) = watcher.lock() {
        w.resume();
    }

    result
}

/// Tauri 命令：获取当前屏幕分辨率推导出的分组/模组上限及选用整数宽度。
///
/// 前端可据此限制「新建分组 / 添加模组」的 UI 上限，对齐 NRMM 的 `group_int` 绑定 xy 轴语义。
#[tauri::command]
pub fn get_resolution_limits_cmd() -> ResolutionLimits {
    compute_limits()
}

/// Tauri 命令：导出模组目录为压缩包
///
/// 前端调用 `exportMod` 触发。三级回退：用户 7z CLI → 打包 7z CLI → 自维护压缩。
///
/// # Errors
/// - 压缩失败（源非目录、7z 未找到、自维护压缩不支持该格式等）
#[tauri::command]
pub async fn export_mod_cmd(mod_dir: String, archive_path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        export_mod(Path::new(&mod_dir), Path::new(&archive_path)).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
