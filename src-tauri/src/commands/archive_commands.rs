//! 压缩包导入命令域
//!
//! 从 core/archive_handler.rs 迁出（2026-09-19 Phase 6-4）。
//! 承载压缩包支持检测与导入（含自动导入/批量导入）命令。
//! 底层解压/归档逻辑分散于 core/archive_detect|extract|import|export（archive_handler.rs 拆分），此处仅薄适配。
//! 导出侧暂无 IPC 出口（批次 3 摘除 `export_mod_cmd` 后，`core::archive_export::export_mod` 仍在，随时可再挂）。

use crate::core::archive_detect::ExtractResult;
use crate::core::archive_import::import_item;
use crate::core::watcher_core::FileWatcher;
use crate::core::watcher_ring::WatcherGuard;
use crate::ipc::typed_paths::{ManagedPath, SourcePath};
use std::sync::{Arc, Mutex};
use tauri::State;

/// 批量导入的 IPC 入参
///
/// # 为什么在命令层重新定义
/// - 校验属于上层职责：`target_group_dir` 用 [`ManagedPath`]、`items` 用
///   [`SourcePath`]，边界在反序列化期完成，`core::archive_detect::ImportItemRequest`
///   继续保持无校验的纯数据结构。
/// - `#[serde(rename_all = "camelCase")]`：Tauri 的 `#[tauri::command]` 只对
///   **顶层**参数名做 camelCase 映射，嵌套结构体字段按 serde 默认（snake_case）匹配。
///   前端 `importItems()` 发送的是 `targetGroupDir`，而原结构体字段是
///   `target_group_dir` —— 该命令此前在反序列化阶段即失败（拖拽导入的第二层缺陷：
///   第一层是 HTML5 `File.path` 在 Tauri 下恒不存在，取不到任何路径）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItemArgs {
    /// 待导入的文件或目录（只读来源：绝对、存在）
    pub items: Vec<SourcePath>,
    /// 目标分组目录（必须位于已登记 Mods 根内）
    pub target_group_dir: ManagedPath,
    /// 可选压缩包密码（`None` 表示尝试无密码解压，`Some("")` 表示空密码）
    pub password: Option<String>,
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
    req: ImportItemArgs,
) -> Result<Vec<ExtractResult>, String> {
    let _busy = crate::window::lifecycle::BusyOpGuard::new();
    // 路径边界由类型保证：items 为存在性已校验的来源，target_group_dir 已在 Mods 根内
    let target = req.target_group_dir.into_canonical();
    let items = req.items;
    let password = req.password;
    // 暂停文件监控
    {
        let w = watcher.lock().map_err(|e| e.to_string())?;
        w.pause();
    }
    let _watcher_guard = WatcherGuard::new(); // 全局暂停（引用计数，函数结束自动恢复）

    let result = tauri::async_runtime::spawn_blocking(move || -> Vec<ExtractResult> {
        let mut outs = Vec::with_capacity(items.len());
        for item in items {
            let res = import_item(item.canonical(), &target, password.as_deref());
            match res {
                Ok(r) => {
                    // 新导入目录补做 INI 备份（幂等，白名单同样生效，后台线程池队首优先执行）
                    if let ExtractResult::Success { ref mod_path, .. } = r {
                        crate::core::ini_backup_scheduler::enqueue_incremental(mod_path.clone());
                    }
                    outs.push(r);
                }
                Err(e) => outs.push(ExtractResult::ExtractFailed {
                    message: format!("{}: {}", item.requested(), e),
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

#[cfg(test)]
mod tests {
    use super::ImportItemArgs;
    use crate::ipc::mods_root_registry::{register, sync_from, test_guard};

    /// 字段命名回归：Tauri 的 `#[tauri::command]` 只对**顶层**参数名做 camelCase 映射，
    /// 嵌套结构体字段按 serde 默认（snake_case）匹配。前端 `importItems()` 发送的是
    /// `targetGroupDir`，而原 `core::archive_detect::ImportItemRequest` 字段是
    /// `target_group_dir`，导致该命令在反序列化阶段即失败（拖拽导入的第二层缺陷）。
    /// `ImportItemArgs` 带 `#[serde(rename_all = "camelCase")]`，本用例钉住这一口径。
    #[test]
    fn import_item_args_accepts_frontend_camel_case_shape() {
        let _g = test_guard();
        let root = tempfile::tempdir().unwrap();
        register(root.path()).unwrap();
        let group = root.path().join("_MANAGED_").join("group_1");
        std::fs::create_dir_all(&group).unwrap();
        let archive = root.path().join("mod.7z");
        std::fs::write(&archive, b"PK").unwrap();

        let json = serde_json::json!({
            "items": [archive.to_string_lossy()],
            "targetGroupDir": group.to_string_lossy(),
            "password": null,
        });
        let req = serde_json::from_value::<ImportItemArgs>(json)
            .expect("前端 camelCase 载荷应可反序列化");
        assert_eq!(req.items.len(), 1);

        // 旧的 snake_case 形态应被拒绝（证明缺陷根因确为字段命名不一致）
        let legacy = serde_json::json!({
            "items": [archive.to_string_lossy()],
            "target_group_dir": group.to_string_lossy(),
        });
        assert!(serde_json::from_value::<ImportItemArgs>(legacy).is_err());

        sync_from(Vec::<String>::new());
    }

    /// 来源与目标的语义差别：压缩包可在任意位置（SourcePath），
    /// 而目标分组必须在已登记 Mods 根内（ManagedPath）——写错一侧都会打坏导入。
    #[test]
    fn import_source_may_live_outside_root_but_target_may_not() {
        let _g = test_guard();
        let root = tempfile::tempdir().unwrap();
        register(root.path()).unwrap();
        let group = root.path().join("_MANAGED_").join("group_1");
        std::fs::create_dir_all(&group).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let archive = elsewhere.path().join("mod.7z");
        std::fs::write(&archive, b"PK").unwrap();

        let ok = serde_json::json!({
            "items": [archive.to_string_lossy()],
            "targetGroupDir": group.to_string_lossy(),
        });
        serde_json::from_value::<ImportItemArgs>(ok).expect("根外来源 + 根内目标应通过");

        // 目标必须真实存在才谈得上「根外越界」（不存在时先命中 PATH_NOT_FOUND）
        let bad_target_dir = elsewhere.path().join("group_9");
        std::fs::create_dir_all(&bad_target_dir).unwrap();
        let bad_target = serde_json::json!({
            "items": [archive.to_string_lossy()],
            "targetGroupDir": bad_target_dir.to_string_lossy(),
        });
        let err = serde_json::from_value::<ImportItemArgs>(bad_target)
            .expect_err("根外目标必须被拒绝")
            .to_string();
        assert!(err.contains("[OUT_OF_MANAGED_ROOT]"), "实际：{err}");

        sync_from(Vec::<String>::new());
    }
}
