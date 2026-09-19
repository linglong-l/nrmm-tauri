//! 文件监控器 —— 事件发射域
//!
//! 从 file_watcher.rs 拆出（2026-09-19 剩余巨石拆分 PA-2）。
//! 承载 `managed-folder-changed` / `managed-partial-update` 前端事件发射
//! 与 `ManagedPartialUpdatePayload` 序列化契约（R2），并据此判定主窗口可见性。
//! 事件环形缓冲/暂停守卫见 [`crate::core::watcher_ring`]，增量根分类/批次路由见
//! [`crate::core::watcher_incremental`]，FileWatcher 主体见 [`crate::core::watcher_core`]。

use crate::models::mod_data::GroupDelta;
use std::path::Path;
use tauri::{AppHandle, Emitter, Manager};

/// `managed-partial-update` 事件 payload（R2）：直接携带分组增量列表。
///
/// 前端零额外 IPC 往返：对 `deltas` 逐组 `applyGroupDelta` 原地 patch
/// （复用 R1 契约），随后到达的 folder-changed 防抖 refresh 被回声抑制跳过。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedPartialUpdatePayload {
    /// 当前游戏（与既有事件一致的 Debug 字符串形态）
    game: String,
    /// 受影响分组的增量列表（多 root 批次可能含多个组）
    deltas: Vec<GroupDelta>,
}

/// 主窗口当前是否可见（不可见时静默合并缓存，不 emit 前端事件）
fn window_visible(app_handle: &AppHandle) -> bool {
    app_handle
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// 处理完成后通知前端：窗口可见时 emit `managed-folder-changed`（全量 refresh
/// 兜底信号）；窗口隐藏时静默（缓存已更新）。
pub(crate) fn emit_folder_changed(app_handle: &AppHandle, managed_path: &Path) {
    if window_visible(app_handle) {
        let _ = app_handle.emit(
            crate::core::events::MANAGED_FOLDER_CHANGED,
            managed_path.as_os_str(),
        );
    } else {
        log::info!("[Incremental] window hidden -> merged cache silently");
    }
}

/// 增量批次补充通知（R2）：emit `managed-partial-update`，payload 直接携带
/// [`GroupDelta`] 列表（推送替代拉取，前端零额外 IPC 往返直接 patch）。
///
/// 须在 [`emit_folder_changed`] **之后**调用：folder-changed 先到 → 前端 500ms
/// 防抖 timer 先建 → partial-update patch 置位 lastLocalPatchAt → timer 到期时
/// 被回声抑制跳过 → 全量 refresh 仅余兜底路径。窗口隐藏时静默。
///
/// # 参数
/// - `app_handle`: Tauri 应用句柄
/// - `game`: 当前游戏（payload 字段）
/// - `deltas`: 本批次组装的分组增量列表
pub(crate) fn emit_managed_partial_update(
    app_handle: &AppHandle,
    game: crate::models::enums::TargetGame,
    deltas: &[GroupDelta],
) {
    if window_visible(app_handle) {
        let payload = ManagedPartialUpdatePayload {
            game: format!("{game:?}"),
            deltas: deltas.to_vec(),
        };
        let _ = app_handle.emit(crate::core::events::MANAGED_PARTIAL_UPDATE, payload);
    }
}

#[cfg(test)]
#[path = "watcher_emit_tests.rs"]
mod tests;
