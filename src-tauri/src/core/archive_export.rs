//! 压缩包处理 —— 导出域
//!
//! 从 archive_handler.rs 拆出（2026-09-19 剩余巨石拆分 PA-3）。
//!
//! **导出功能当前没有 IPC 出口**：`export_mod_cmd` 因前端零调用被摘除后，
//! `export_mod`（三级回退：用户 7z CLI → 打包 7z CLI → 自维护压缩）与其私有的
//! `run_7z_add` 按「零调用即删」判据一并删除，完整实现见 git 历史。
//! 本模块只剩自维护压缩 `export_internal`，且**仅在测试构型下编译**：它是
//! `archive_extract_tests` 生成 .7z 夹具的唯一手段，同时钉住「自维护压缩仅支持 7z」
//! 这条口径。若日后恢复单模组导出，需重新实现三级回退并把本函数改回 `pub(crate)`。
//!
//! 类型契约与 7-Zip 路径解析见 [`crate::core::archive_detect`]，
//! 解压实现见 [`crate::core::archive_extract`]，扁平化/导入见
//! [`crate::core::archive_import`]。

// 本模块唯一的条目是 `#[cfg(test)]` 的 export_internal，故 import 同门否则非测试构型报 unused。
#[cfg(test)]
use anyhow::{anyhow, Result};
#[cfg(test)]
use std::path::Path;

/// 自维护压缩（**仅 7z 支持**）
///
/// 设计约束（压缩能力分层）：
/// - **自维护压缩仅支持 `.7z`**（通过 `sevenz-rust`）：项目只维护 7z 的压缩实现。
/// - `.zip` 压缩**不**在自维护层实现——这避免引入第二套压缩实现
///   （zip crate 仅用于解压回退，职责单一）。
/// - `.rar` 本身不支持创建，任何层都不压缩 rar。
///
/// 解压能力不受影响：zip / rar / 7z 的自维护解压仍分别由 `zip`/`unrar`/`sevenz-rust` 提供。
#[cfg(test)]
pub(crate) fn export_internal(mod_dir: &Path, archive_path: &Path, ext: &str) -> Result<()> {
    match ext {
        "7z" => sevenz_rust::compress_to_path(mod_dir, archive_path)
            .map(|_| ())
            .map_err(|e| anyhow!("sevenz-rust 自维护压缩失败: {:?}", e)),
        other => Err(anyhow!(
            "自维护压缩仅支持 .7z（zip/rar 需安装 7z CLI 才能压缩 .{}）；导出 .{} 格式请安装 7z CLI",
            other,
            other
        )),
    }
}

#[cfg(test)]
#[path = "archive_export_tests.rs"]
mod tests;
