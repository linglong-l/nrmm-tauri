//! 压缩包处理 —— 导出域
//!
//! 从 archive_handler.rs 拆出（2026-09-19 剩余巨石拆分 PA-3）。
//! 承载模组目录导出为压缩包（`export_mod`，三级回退：用户 7z CLI → 打包 7z CLI →
//! 自维护压缩）。类型契约与 7-Zip 路径解析见 [`crate::core::archive_detect`]，
//! 解压实现见 [`crate::core::archive_extract`]，扁平化/导入见
//! [`crate::core::archive_import`]。

use crate::core::archive_detect::{get_bundled_7z_path, get_system_7z_path};
use anyhow::{anyhow, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

/// 导出模组目录为压缩包（三级回退：用户 7z CLI → 打包 7z CLI → 自维护 7z 压缩）
///
/// 对齐用户要求：优先调用用户级 7z CLI 进行压缩（默认 `-mx5` 均衡级别），
/// 其次打包的 7z CLI，最后采用**自维护 7z 压缩**（`sevenz-rust`，仅 7z）。
///
/// 压缩能力分层（关键约束）：
/// - 压缩默认用 7z CLI（用户级 → 打包级），自维护压缩**仅支持 `.7z`**。
/// - 自维护层**不**提供 zip 压缩——若两级 7z CLI 均不可用，导出 zip 请先安装 7z CLI。
/// - 解压能力不受影响：zip / rar / 7z 的自维护解压仍完整可用（仅压缩收紧）。
///
/// 归档内容以模组目录自身为根（解压后得到 `mod_name/`），与导入时的
/// `unwrap_single_folder_nesting` 展平逻辑形成对称闭环。
///
/// # 参数
/// - `mod_dir`: 待导出的模组目录
/// - `archive_path`: 目标压缩包路径（扩展名决定格式：`.7z` / `.zip`，其余交给 7z CLI）
///
/// # Errors
/// - 源不是目录
/// - 三级压缩全部失败
pub fn export_mod(mod_dir: &Path, archive_path: &Path) -> Result<()> {
    if !mod_dir.is_dir() {
        return Err(anyhow!("export_mod: 源不是目录: {:?}", mod_dir));
    }
    let ext = archive_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    // --- 第一级：用户级 7z CLI ---
    if let Some(sys) = get_system_7z_path() {
        if run_7z_add(&sys, archive_path, mod_dir, &ext).is_ok() {
            log::info!("export_mod: 用户 7z CLI 压缩成功 {:?}", archive_path);
            return Ok(());
        }
    }

    // --- 第二级：打包的 7z CLI ---
    if let Some(bundled) = get_bundled_7z_path() {
        if run_7z_add(&bundled, archive_path, mod_dir, &ext).is_ok() {
            log::info!("export_mod: 打包 7z CLI 压缩成功 {:?}", archive_path);
            return Ok(());
        }
    }

    // --- 第三级：自维护压缩逻辑 ---
    log::info!("export_mod: 回退到自维护压缩 {:?}", archive_path);
    export_internal(mod_dir, archive_path, &ext)
}

/// 调用 7z CLI 执行 `a`（添加/压缩）命令
///
/// `7z a -y [-t<fmt>] <archive> <mod_dir>`，将模组目录整体归档。
/// `.7z` 为 7z 默认格式无需 `-t`；其余格式显式指定（`zip`/`tar`/`gzip` 等）。
fn run_7z_add(seven_zip: &Path, archive: &Path, mod_dir: &Path, fmt: &str) -> Result<()> {
    if let Some(parent) = archive.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut cmd = Command::new(seven_zip);
    // -mx5 = 均衡（normal）压缩级别：速度与压缩率折中，符合「压缩默认采用均衡」要求。
    // 注：解压（-mx 不影响）无需此参数；仅压缩场景生效。
    cmd.arg("a").arg("-y").arg("-mx5");
    if fmt != "7z" && !fmt.is_empty() {
        cmd.arg(format!("-t{}", fmt));
    }
    cmd.arg(archive).arg(mod_dir);

    let output = cmd.output()?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(anyhow!("7z CLI 压缩失败: {}{}", stdout, stderr))
    }
}

/// 自维护压缩（第三级回退，**仅 7z 支持**）
///
/// 设计约束（压缩能力分层）：
/// - 压缩默认走 7z CLI（用户级 → 打包级），兜底才是自维护。
/// - **自维护压缩仅支持 `.7z`**（通过 `sevenz-rust`）：因项目仅维护 7z 的压缩实现。
/// - `.zip` 压缩**不**在自维护层实现——若两级 7z CLI 均不可用，请用户安装 7z CLI。
///   这避免引入第二套压缩实现（zip crate 仅用于解压回退，职责单一）。
/// - `.rar` 本身不支持创建，任何层都不压缩 rar。
///
/// 解压能力不受影响：zip / rar / 7z 的自维护解压仍分别由 `zip`/`unrar`/`sevenz-rust` 提供。
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
