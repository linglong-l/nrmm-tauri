//! 压缩包处理模块
//!
//! 负责处理模组压缩包的导入，支持格式：ZIP、RAR、7Z。
//! 核心功能：
//! - 自动检测压缩包格式（魔数优先 + 扩展名回退的双保险策略）
//! - 调用内置 7-Zip 或系统 `7z` 命令行工具解压
//! - 自动扁平化单层目录（压缩包内只有一个根目录时提升内容到父级）
//! - 检测密码保护的压缩包（通过 `7z x` 输出中匹配 "password"/"Wrong password" 等关键词）
//! - 检查解压后是否包含 INI 文件（有效模组标志）
//! - 自动处理重名（追加 `_1`, `_2` 等数字后缀）
//! - 支持指定分组导入和自动选择分组导入（扫描 `group_1`, `group_2`... 找到第一个存在的分组）
//! - 支持直接导入已解压的模组目录（同盘移动 / 跨盘复制 + trash 回收站）

use anyhow::{Result, anyhow, Context};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;
use std::io::Write;
use serde::{Serialize, Deserialize};
use infer;
use trash::delete;
use std::sync::{Arc, Mutex};
use rayon::prelude::*;
use walkdir::WalkDir;
use crate::core::resolution::{compute_limits, ResolutionLimits};
use tauri::State;
use crate::core::file_watcher::FileWatcher;
use crate::core::file_watcher::WATCHER_PAUSED;
use std::sync::atomic::Ordering;

/// 压缩包类型枚举
///
/// 通过 `detect_archive_type`（扩展名）或 `detect_archive_type_robust`（魔数优先）识别。
/// 实现了 `Serialize`/`Deserialize`，可直接作为 Tauri 命令返回值返回给前端。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArchiveType {
    /// ZIP 格式（`.zip`），包含标准 ZIP 和 tarball 系列（tar.gz, tar.bz2 等，统一交由 7z 处理）
    Zip,
    /// RAR 格式（`.rar`），使用 7z 解压
    Rar,
    /// 7-Zip 格式（`.7z`），使用 7z 解压
    SevenZip,
    /// 不支持的格式，`String` 为文件扩展名或魔数识别信息
    Unsupported(String),
}

/// 解压结果枚举
///
/// 覆盖所有可能的解压/导入结果场景，前端根据变体类型展示不同 UI。
/// 实现了 `Serialize`，可直接作为 Tauri 命令返回值序列化给前端。
#[derive(Debug, Clone, Serialize)]
pub enum ExtractResult {
    /// 解压/导入成功
    ///
    /// 包含最终安装路径、模组名称、是否含 INI 文件（有效模组标志）和文件总数。
    Success {
        /// 模组最终安装路径（已处理重名，如 `xxx_1`）
        mod_path: PathBuf,
        /// 模组名称（从压缩包文件名/目录名提取，不含扩展名）
        mod_name: String,
        /// 是否包含 INI 文件（判断是否为有效模组的关键标志）
        has_ini: bool,
        /// 文件总数（压缩包 = 解压出的文件数，目录 = 目录内文件数）
        file_count: usize,
    },
    /// 压缩包需要密码（7z 输出匹配到 "password"/"Wrong password" 等关键词）
    ///
    /// 前端收到此结果时通常会弹出密码输入框让用户重新尝试。
    PasswordRequired {
        /// 压缩包路径
        archive_path: PathBuf,
    },
    /// 不支持的压缩包格式（非 zip/rar/7z 或魔数识别失败）
    UnsupportedFormat {
        /// 文件扩展名或魔数识别信息
        ext: String,
        /// 错误提示信息（本地化友好提示）
        message: String,
    },
    /// 解压成功但未找到 INI 文件（可能不是有效模组）
    ///
    /// 模组规范要求根目录下必须有 `.ini` 文件才视为有效模组。
    NoIniFound {
        /// 解压临时目录路径（尚未清理，用户可自行检查）
        extracted_path: PathBuf,
        /// 错误提示信息
        message: String,
    },
    /// 解压/导入失败（7z 命令执行错误、IO 错误等）
    ExtractFailed {
        /// 错误信息（包含详细的技术原因）
        message: String,
    },
}

/// 批量导入请求（Tauri IPC 反序列化结构体）
///
/// 前端通过 `importItem` 命令发送，包含待导入的文件/目录列表和目标分组。
/// 实现了 `Deserialize`，通过 serde 自动从 JSON 反序列化。
#[derive(Debug, Clone, Deserialize)]
pub struct ImportItemRequest {
    /// 待导入的文件或目录路径列表（支持混合传入压缩包和已解压目录）
    pub items: Vec<String>,
    /// 目标分组目录路径（如 `.../_MANAGED_/group_1`）
    pub target_group_dir: String,
    /// 可选压缩包密码（`None` 表示尝试无密码解压，`Some("")` 表示空密码）
    pub password: Option<String>,
}

/// 根据文件扩展名检测压缩包类型
///
/// 简单查表法：将 `.zip` → `Zip`, `.rar` → `Rar`, `.7z` → `SevenZip`，其余为 `Unsupported`。
/// 注意：此函数仅检查扩展名，不读取文件内容。如需更可靠的检测（魔数优先），请使用 `detect_archive_type_robust`。
///
/// # 参数
/// - `path`: 压缩包文件路径
///
/// # 返回
/// 检测到的 `ArchiveType`
///
/// # Panics
/// 不会 panic
///
/// # Errors
/// 无错误返回，即使文件不存在也只返回 `Unsupported`
pub fn detect_archive_type(path: &Path) -> ArchiveType {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let lower = ext.to_lowercase();
            match lower.as_str() {
                "zip" => ArchiveType::Zip,
                "rar" => ArchiveType::Rar,
                "7z" => ArchiveType::SevenZip,
                other => ArchiveType::Unsupported(other.to_string()),
            }
        }
        None => ArchiveType::Unsupported("no extension".to_string()),
    }
}

/// 鲁棒的压缩包类型检测：魔数（infer）优先 → 扩展名回退的双保险策略
///
/// 识别顺序：
/// 1. `infer::get_from_path` 读取文件头魔数（前 8-16 字节）
/// 2. 匹配 `zip` / `7z` / `rar` → 直接映射对应类型
/// 3. 匹配 `tar` / `gz` / `bz2` / `xz` / `zst` / `lz` / `lzma` → 归为 `Zip` 类（由 7z 统一处理）
/// 4. 魔数识别失败或不匹配 → log debug 后回退到 `detect_archive_type` 扩展名检测
///
/// # 参数
/// - `path`: 待检测的文件路径
///
/// # 返回
/// 检测到的 `ArchiveType`
///
/// # Panics
/// 不会 panic
///
/// # Errors
/// - 文件不可读或不存在时 infer 返回 `Err`，回退到扩展名检测
pub fn detect_archive_type_robust(path: &Path) -> ArchiveType {
    match infer::get_from_path(path) {
        Ok(Some(kind)) => {
            let mime = kind.mime_type();
            let ext = kind.extension();
            log::debug!(
                "detect_archive_type_robust: infer matched mime={} ext={} for {:?}",
                mime, ext, path
            );
            match ext {
                "zip" => ArchiveType::Zip,
                "rar" => ArchiveType::Rar,
                "7z" => ArchiveType::SevenZip,
                "tar" | "gz" | "bz2" | "xz" | "zst" | "lz" | "lzma" => ArchiveType::Zip,
                _ => detect_archive_type(path),
            }
        }
        Ok(None) => {
            log::debug!("detect_archive_type_robust: infer returned None for {:?}, fallback to extension", path);
            detect_archive_type(path)
        }
        Err(e) => {
            log::debug!("detect_archive_type_robust: infer error for {:?}: {}, fallback to extension", path, e);
            detect_archive_type(path)
        }
    }
}

/// 检查是否为支持的压缩包格式（zip/rar/7z）
///
/// 使用 `detect_archive_type_robust` 进行鲁棒检测：魔数优先 + 扩展名回退。
/// 返回 `true` 当且仅当检测结果为 `Zip`、`Rar` 或 `SevenZip`。
///
/// # 参数
/// - `path`: 待检查的文件路径
pub fn is_supported_archive(path: &Path) -> bool {
    matches!(
        detect_archive_type_robust(path),
        ArchiveType::Zip | ArchiveType::Rar | ArchiveType::SevenZip
    )
}

/// 查找系统 PATH 中的 7-Zip 可执行文件路径
///
/// 仅搜索系统 PATH，不检查内置打包的 7z 二进制。
/// 用于三级回退链的第一级：用户自行安装的 7z 版本更新、性能更好。
///
/// 搜索顺序：
/// 1. 平台标准 7z 命令（`SEVEN_Z_EXECUTABLE`）
/// 2. 备选名称：`7za`, `7zr`, `7zz`, `7z`
///
/// # 返回
/// `Some(PathBuf)` 表示找到可用的系统 7z，`None` 表示未找到
fn get_system_7z_path() -> Option<PathBuf> {
    let system_candidates: &[&str] = &[
        crate::core::constants::SEVEN_Z_EXECUTABLE,
        "7za",
        "7zr",
        "7zz",
        "7z",
    ];
    for cmd in system_candidates {
        if let Ok(output) = Command::new(cmd).arg("--help").output() {
            if output.status.success() {
                log::debug!("get_system_7z_path: found system 7z at '{}'", cmd);
                return Some(PathBuf::from(cmd));
            }
        }
    }
    log::debug!("get_system_7z_path: no system 7z found in PATH");
    None
}

/// 查找内置打包的 7-Zip 可执行文件路径
///
/// 仅搜索应用 bundle 内的 7z 二进制，不检查系统 PATH。
/// 用于三级回退链的第二级：打包在应用中的 7z CLI 作为备用方案。
///
/// 搜索位置：
/// 1. 应用程序 `resources/` 目录下的 `SEVEN_Z_BUILTIN_PATH`
/// 2. 应用程序目录下的 `SEVEN_Z_BUILTIN_PATH`
/// 3. macOS 的 `../Resources` 目录下的 `SEVEN_Z_BUILTIN_PATH`
///
/// # 返回
/// `Some(PathBuf)` 表示找到内置 7z，`None` 表示未找到
fn get_bundled_7z_path() -> Option<PathBuf> {
    let builtin = crate::core::constants::SEVEN_Z_BUILTIN_PATH;
    if let Ok(exe_dir) = std::env::current_exe() {
        if let Some(exe_dir) = exe_dir.parent() {
            let candidates = [
                exe_dir.join("resources").join(builtin),
                exe_dir.join(builtin),
                exe_dir.join("../Resources").join(builtin),
            ];
            for candidate in &candidates {
                if candidate.exists() {
                    log::debug!("get_bundled_7z_path: found builtin 7z at {:?}", candidate);
                    return Some(candidate.clone());
                }
            }
        }
    }
    log::debug!("get_bundled_7z_path: no builtin 7z found");
    None
}

/// 查找 7-Zip 可执行文件路径（系统优先 + 内置回退）
///
/// 组合 `get_system_7z_path` 和 `get_bundled_7z_path`，系统 7z 优先。
/// 保留此函数用于向后兼容和不需要分级回退的场景。
///
/// # 返回
/// 找到的第一个可用 7z 可执行文件路径
///
/// # Errors
/// - 所有搜索位置均未找到 7z 可执行文件，返回 `anyhow!("7z CLI not found...")`
#[allow(dead_code)]
fn get_7z_path() -> Result<PathBuf> {
    if let Some(path) = get_system_7z_path() {
        return Ok(path);
    }
    if let Some(path) = get_bundled_7z_path() {
        return Ok(path);
    }
    Err(anyhow!("7z CLI not found in system PATH or application bundle."))
}

/// 判断两个路径是否在同一磁盘（卷）上
///
/// 用于决定目录导入策略：同盘使用 `move_dir`（快速重命名），跨盘使用 `copy` + `trash::delete`。
///
/// Windows：比较路径的 `Prefix` 组件（盘符/卷GUID/UNC 路径），忽略大小写
/// 非 Windows：保守返回 `false`，调用方将使用 copy + trash 策略
///
/// # 参数
/// - `a`: 第一个路径
/// - `b`: 第二个路径
///
/// # 返回
/// `true` 表示在同一磁盘上
pub fn paths_on_same_disk(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Prefix;
        use std::os::windows::ffi::OsStrExt;
        fn get_prefix(path: &Path) -> Option<String> {
            let comp = path.components().next()?;
            if let std::path::Component::Prefix(prefix) = comp {
                let kind = prefix.kind();
                match kind {
                    Prefix::Disk(disk) => {
                        return Some(format!("disk_{}", (disk as char).to_ascii_lowercase()));
                    }
                    Prefix::VerbatimDisk(disk) => {
                        return Some(format!("vdisk_{}", (disk as char).to_ascii_lowercase()));
                    }
                    Prefix::UNC(server, share) => {
                        let s: String = server.encode_wide()
                            .chain(share.encode_wide())
                            .map(|c| (c as u32).to_string())
                            .collect();
                        return Some(format!("unc_{}", s.to_ascii_lowercase()));
                    }
                    Prefix::VerbatimUNC(server, share) => {
                        let s: String = server.encode_wide()
                            .chain(share.encode_wide())
                            .map(|c| (c as u32).to_string())
                            .collect();
                        return Some(format!("vunc_{}", s.to_ascii_lowercase()));
                    }
                    Prefix::Verbatim(_) | Prefix::DeviceNS(_) => return None,
                }
            }
            None
        }
        let pa = get_prefix(a);
        let pb = get_prefix(b);
        pa.is_some() && pa == pb
    }
    #[cfg(not(windows))]
    {
        let _ = (a, b);
        false
    }
}

/// 使用 7z CLI 解压压缩包到临时目录
///
/// 提取 `extract_archive` 中 7z CLI 调用逻辑为独立函数，供三级回退链复用。
/// 仅负责调用 7z 命令并返回临时目录路径，不执行后续的扁平化、统计、移动等操作。
///
/// # 参数
/// - `seven_zip_path`: 7z 可执行文件路径（系统 PATH 中的命令名或内置二进制完整路径）
/// - `archive_path`: 压缩包路径
/// - `target_dir`: 目标父目录（临时目录将创建在此目录下）
/// - `password`: 可选压缩包密码，`None` 表示尝试无密码解压
///
/// # 返回
/// 成功时返回临时目录 `PathBuf`，失败时返回错误
///
/// # Errors
/// - 临时目录创建失败
/// - 7z 命令执行失败（密码错误返回 `PasswordRequired` 变体错误消息）
/// - 7z 命令返回非零退出码
fn extract_with_7z_cli(
    seven_zip_path: &Path,
    archive_path: &Path,
    target_dir: &Path,
    password: Option<&str>,
) -> Result<PathBuf> {
    let temp_dir = target_dir.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    let mut cmd = Command::new(seven_zip_path);
    cmd.arg("x")
       .arg("-y")
       .arg(format!("-o{}", temp_dir.to_string_lossy()));

    if let Some(pw) = password {
        let sanitized: String = pw.chars().filter(|c| !c.is_control()).collect();
        if sanitized.is_empty() {
            cmd.arg("-p");
        } else {
            cmd.arg(format!("-p{}", sanitized));
        }
    } else {
        cmd.arg("-p");
    }

    cmd.arg(archive_path.to_string_lossy().to_string());

    let output = cmd.output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, stderr);

    if !output.status.success() {
        if combined.contains("password") || combined.contains("Wrong password") || combined.contains("Encrypted") {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(anyhow!("PASSWORD_REQUIRED"));
        }
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(anyhow!("7z CLI extraction failed: {}", combined));
    }

    Ok(temp_dir)
}

/// 使用 `sevenz-rust` crate 自实现解压 7z 压缩包到临时目录
///
/// 三级回退链的第三级（7z 格式）：纯 Rust 实现的 7z 解压，不依赖外部 CLI。
/// 支持密码保护的压缩包。
///
/// # 参数
/// - `archive_path`: 7z 压缩包路径
/// - `target_dir`: 目标父目录（临时目录将创建在此目录下）
/// - `password`: 可选压缩包密码
///
/// # 返回
/// 成功时返回临时目录 `PathBuf`
///
/// # Errors
/// - 临时目录创建失败
/// - `sevenz_rust::decompress_file` 解压失败
fn extract_7z_internal(
    archive_path: &Path,
    target_dir: &Path,
    password: Option<&str>,
) -> Result<PathBuf> {
    let temp_dir = target_dir.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    log::info!(
        "extract_7z_internal: falling back to sevenz-rust for {:?}",
        archive_path
    );

    if password.is_some() {
        // sevenz-rust 0.6 不直接支持密码解压，密码保护的 7z 应在前两级 7z CLI 中处理
        return Err(anyhow!("Password-protected 7z not supported by internal extractor"));
    }

    sevenz_rust::decompress_file(archive_path, &temp_dir)
        .context("sevenz-rust decompress_file failed")?;

    Ok(temp_dir)
}

/// 使用 `zip` crate 自实现解压 ZIP 压缩包到临时目录
///
/// 三级回退链的第三级（ZIP 格式）：纯 Rust 实现的 ZIP 解压，不依赖外部 CLI。
/// 注意：自实现 ZIP 解压不支持密码保护（密码保护的 ZIP 应在前两级 7z CLI 中处理）。
///
/// # 参数
/// - `archive_path`: ZIP 压缩包路径
/// - `target_dir`: 目标父目录（临时目录将创建在此目录下）
///
/// # 返回
/// 成功时返回临时目录 `PathBuf`
///
/// # Errors
/// - 临时目录创建失败
/// - 文件打开失败
/// - `zip::ZipArchive::extract` 解压失败
fn extract_zip_internal(
    archive_path: &Path,
    target_dir: &Path,
) -> Result<PathBuf> {
    let temp_dir = target_dir.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    log::info!(
        "extract_zip_internal: falling back to zip crate for {:?}",
        archive_path
    );

    let file = fs::File::open(archive_path)
        .context("extract_zip_internal: failed to open archive")?;
    let mut archive = zip::ZipArchive::new(file)
        .context("extract_zip_internal: failed to read ZIP archive")?;
    archive.extract(&temp_dir)
        .context("extract_zip_internal: ZIP extraction failed")?;

    Ok(temp_dir)
}

/// 使用 `unrar` crate 自实现解压 RAR 压缩包到临时目录
///
/// 三级回退链的第三级（RAR 格式）：基于 UnRAR C 库的 RAR 解压，不依赖外部 CLI。
/// 注意：`unrar` crate 仅支持解压，不支持创建 RAR 压缩包。
///
/// # 参数
/// - `archive_path`: RAR 压缩包路径
/// - `target_dir`: 目标父目录（临时目录将创建在此目录下）
/// - `password`: 可选压缩包密码
///
/// # 返回
/// 成功时返回临时目录 `PathBuf`
///
/// # Errors
/// - 临时目录创建失败
/// - `unrar::Archive` 打开或解压失败
fn extract_rar_internal(
    archive_path: &Path,
    target_dir: &Path,
    password: Option<&str>,
) -> Result<PathBuf> {
    let temp_dir = target_dir.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    log::info!(
        "extract_rar_internal: falling back to unrar crate for {:?}",
        archive_path
    );

    let mut archive = if let Some(pw) = password {
        unrar::Archive::with_password(archive_path, pw)
            .open_for_processing()
            .context("extract_rar_internal: failed to open RAR archive with password")?
    } else {
        unrar::Archive::new(archive_path)
            .open_for_processing()
            .context("extract_rar_internal: failed to open RAR archive")?
    };

    // 使用状态机模式逐文件解压
    while let Some(header) = archive.read_header()
        .context("extract_rar_internal: failed to read RAR header")?
    {
        archive = if header.entry().is_file() {
            header.extract_with_base(&temp_dir)
                .context("extract_rar_internal: failed to extract RAR entry")?
        } else {
            header.skip()
                .context("extract_rar_internal: failed to skip RAR entry")?
        };
    }

    Ok(temp_dir)
}

/// 解压压缩包到目标目录（三级回退链）
///
/// # 三级回退链设计
///
/// 按优先级依次尝试以下解压方式，任一成功即返回：
///
/// **第一级：系统 7z CLI**（最高优先级）
/// - 搜索系统 PATH 中的 7z 命令（`7z.exe`/`7zz`/`7za`/`7zr`/`7z`）
/// - 用户自行安装的 7z 版本更新、性能更好
///
/// **第二级：内置打包 7z CLI**（中级回退）
/// - 使用应用 bundle 内打包的 7z 二进制（`resources/7z/...`）
/// - 当用户系统无 7z 或版本过低导致解压失败时使用
///
/// **第三级：自实现解压**（最终回退）
/// - 7z 格式 → `sevenz-rust` crate（纯 Rust 实现）
/// - ZIP 格式 → `zip` crate（纯 Rust 实现）
/// - RAR 格式 → `unrar` crate（基于 UnRAR C 库，仅解压）
///
/// # 解压后处理
///
/// 无论使用哪一级解压，成功后均执行以下步骤：
/// 1. **展平单层包裹目录**：`unwrap_single_folder_nesting` 处理压缩包内单根目录情况（循环展平 + 排除元数据文件）
/// 2. **统计文件数 & 检查 INI**：`visit_files` 递归遍历
/// 3. **移动到最终位置**：`unique_path` 处理重名，`move_directory_contents` 合并移动
/// 4. **清理临时目录**：`remove_dir_all`
///
/// # 参数
/// - `archive_path`: 压缩包路径
/// - `target_dir`: 目标父目录（解压后在此目录下创建模组目录）
/// - `password`: 可选压缩包密码，`None` 表示尝试无密码解压
///
/// # 返回
/// `ExtractResult` 枚举，包含成功或各类失败详情
///
/// # Panics
/// 不会 panic
///
/// # Errors
/// - 所有三级解压均失败时返回 `ExtractFailed`
/// - 密码错误时返回 `PasswordRequired`
pub fn extract_archive(archive_path: &Path, target_dir: &Path, password: Option<&str>) -> Result<ExtractResult> {
    let archive_type = detect_archive_type_robust(archive_path);

    if let ArchiveType::Unsupported(ext) = archive_type {
        return Ok(ExtractResult::UnsupportedFormat {
            ext,
            message: "Unsupported archive format. Please manually extract .zip/.rar/.7z files.".to_string(),
        });
    }

    let mut last_error: Option<String> = None;

    // --- 第一级：系统 7z CLI ---
    if let Some(sys_7z) = get_system_7z_path() {
        match extract_with_7z_cli(&sys_7z, archive_path, target_dir, password) {
            Ok(temp_dir) => {
                log::info!("extract_archive: system 7z succeeded for {:?}", archive_path);
                return finalize_extraction(archive_path, target_dir, &temp_dir);
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("PASSWORD_REQUIRED") {
                    return Ok(ExtractResult::PasswordRequired {
                        archive_path: archive_path.to_path_buf(),
                    });
                }
                log::warn!("extract_archive: system 7z failed: {}", msg);
                last_error = Some(format!("system 7z: {}", msg));
            }
        }
    }

    // --- 第二级：内置打包 7z CLI ---
    if let Some(bundled_7z) = get_bundled_7z_path() {
        match extract_with_7z_cli(&bundled_7z, archive_path, target_dir, password) {
            Ok(temp_dir) => {
                log::info!("extract_archive: bundled 7z succeeded for {:?}", archive_path);
                return finalize_extraction(archive_path, target_dir, &temp_dir);
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("PASSWORD_REQUIRED") {
                    return Ok(ExtractResult::PasswordRequired {
                        archive_path: archive_path.to_path_buf(),
                    });
                }
                log::warn!("extract_archive: bundled 7z failed: {}", msg);
                last_error = Some(format!("bundled 7z: {}", msg));
            }
        }
    }

    // --- 第三级：自实现解压（按格式分派） ---
    let internal_result = match archive_type {
        ArchiveType::SevenZip => extract_7z_internal(archive_path, target_dir, password),
        ArchiveType::Zip => {
            if password.is_some() {
                // 自实现 ZIP 不支持密码，直接返回错误
                Err(anyhow!("ZIP password not supported by internal extractor"))
            } else {
                extract_zip_internal(archive_path, target_dir)
            }
        }
        ArchiveType::Rar => extract_rar_internal(archive_path, target_dir, password),
        _ => unreachable!(),
    };

    match internal_result {
        Ok(temp_dir) => {
            log::info!("extract_archive: internal extractor succeeded for {:?}", archive_path);
            finalize_extraction(archive_path, target_dir, &temp_dir)
        }
        Err(e) => {
            let msg = e.to_string();
            log::error!("extract_archive: all tiers failed for {:?}: {}", archive_path, msg);
            let combined = if let Some(prev) = last_error {
                format!("{}; internal: {}", prev, msg)
            } else {
                format!("internal: {}", msg)
            };
            Ok(ExtractResult::ExtractFailed {
                message: format!("All extraction methods failed: {}", combined),
            })
        }
    }
}

/// 解压后处理：扁平化、统计、移动、清理
///
/// 将 `extract_with_7z_cli` 或 internal 函数返回的临时目录内容处理后移动到最终位置。
///
/// # 参数
/// - `archive_path`: 原始压缩包路径（用于提取模组名称）
/// - `target_dir`: 目标父目录
/// - `temp_dir`: 解压后的临时目录
///
/// # 返回
/// `ExtractResult::Success` 包含最终安装路径、模组名称、INI 检查结果和文件数
fn finalize_extraction(
    archive_path: &Path,
    target_dir: &Path,
    temp_dir: &Path,
) -> Result<ExtractResult> {
    // 展平在最终模组目录上执行（见下方 unwrap_single_folder_nesting），此处直接使用临时目录
    let final_dir = temp_dir.to_path_buf();

    let mut has_ini = false;
    let mut file_count = 0usize;
    let mut counter = |path: &Path| {
        file_count += 1;
        if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("ini") {
                has_ini = true;
            }
        }
    };
    visit_files(&final_dir, &mut counter)?;

    let mod_name = archive_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let target_mod_path = target_dir.join(&mod_name);
    let target_mod_path = unique_path(&target_mod_path);

    move_directory_contents(&final_dir, &target_mod_path)?;

    // 展平单层包裹目录（对齐 Dart unwrapSingleFolderNesting）：压缩包内若仅包裹单一根目录，
    // 将其内容提升为模组目录直接内容
    if let Err(e) = unwrap_single_folder_nesting(&target_mod_path) {
        log::warn!("finalize_extraction: 展平单层目录失败 {:?}: {}", target_mod_path, e);
    }

    // 清理临时目录
    let _ = fs::remove_dir_all(temp_dir);

    Ok(ExtractResult::Success {
        mod_path: target_mod_path,
        mod_name,
        has_ini,
        file_count,
    })
}

/// 展平单层包裹目录（对齐 Dart `unwrapSingleFolderNesting`）
///
/// 反复检查 `dir`：若其「相关条目」（排除特定元数据/图标/缓存文件后）恰好只有一个，
/// 且该条目为目录，则将该子目录的内容上移到 `dir` 自身，并删除已清空的包裹目录。
/// 循环执行直到无法继续展平。
///
/// 排除的文件名（与 Dart 一致）：`modname` / `modforced` / `modsyntaxerrorremoved` /
/// `modunoptimized` / `modnamespaced` / `modlink` / `fav` / `.nahidamd` / 图标文件 /
/// `jasm_*` / `.jasm*` / `.imm*` / `*.txt` / `*.json`。
///
/// 子目录上移时使用 `unique_path` 处理命名冲突（对齐 Dart `getSafeTarget`），
/// 同盘优先 rename，跨盘回退 copy + 删除。
///
/// # 参数
/// - `dir`: 待展平的目录路径（通常是解压/导入后的最终模组目录）
///
/// # Errors
/// - `read_dir` 或条目移动失败时返回 IO 错误
pub fn unwrap_single_folder_nesting(dir: &Path) -> Result<()> {
    loop {
        let entries: Vec<_> = match fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(Result::ok).collect(),
            Err(_) => break,
        };

        // 过滤掉 Dart 约定的排除项
        let relevant: Vec<_> = entries
            .iter()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                !is_excluded_for_unwrap(&name)
            })
            .collect();

        // 多于 1 个相关条目，或没有 → 停止展平
        if relevant.len() != 1 {
            break;
        }

        let single = &relevant[0];
        let single_path = single.path();
        // 单个相关条目是文件（非目录）→ 停止
        if !single_path.is_dir() {
            break;
        }

        // 将子目录内容逐一上移到 dir 自身
        let children: Vec<_> = match fs::read_dir(&single_path) {
            Ok(rd) => rd.filter_map(Result::ok).collect(),
            Err(_) => break,
        };
        for child in children {
            let child_path = child.path();
            let target = dir.join(child.file_name());
            let target = unique_path(&target);
            move_entry_up(&child_path, &target)?;
        }

        // 删除已清空的包裹目录
        let _ = fs::remove_dir(&single_path);
    }
    Ok(())
}

/// 将条目移动到目标：同盘优先 `rename`，跨盘回退 `copy` + 删除源。
/// 目标由调用方保证不冲突（`unique_path`），此处仅做防御性清理。
fn move_entry_up(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        if dst.is_dir() {
            fs::remove_dir_all(dst)?;
        } else {
            fs::remove_file(dst)?;
        }
    }
    if paths_on_same_disk(src, dst)
        && fs::rename(src, dst).is_ok() {
            return Ok(());
        }
    // 跨盘回退：复制后删除源
    if src.is_dir() {
        copy_dir_deep(src, dst)?;
        let _ = fs::remove_dir_all(src);
    } else {
        fs::copy(src, dst)?;
        let _ = fs::remove_file(src);
    }
    Ok(())
}

/// 判断文件名是否应在展平时忽略（对齐 Dart `unwrapSingleFolderNesting` 的排除集）
fn is_excluded_for_unwrap(name: &str) -> bool {
    let lower = name.to_lowercase();
    const EXACT: &[&str] = &[
        "modname",
        "modforced",
        "modsyntaxerrorremoved",
        "modunoptimized",
        "modnamespaced",
        "modlink",
        "fav",
        ".nahidamd",
    ];
    if EXACT.contains(&lower.as_str()) {
        return true;
    }
    if lower.starts_with("jasm_") || lower.starts_with(".jasm") || lower.starts_with(".imm") {
        return true;
    }
    if lower.ends_with(".txt") || lower.ends_with(".json") {
        return true;
    }
    // 图标文件（对应 Dart ConstantVar.modIconFilenames）
    if crate::core::constants::ICON_EXTENSIONS.iter().any(|ext| lower.ends_with(*ext)) {
        return true;
    }
    false
}

/// 递归遍历目录下所有文件，对每个文件调用回调
///
/// 用于统计文件数量和检查是否包含 INI 文件。
/// 注意：仅遍历文件（`is_file`），不包含目录本身。
///
/// # 参数
/// - `dir`: 根目录路径
/// - `callback`: 每个文件路径的处理回调
///
/// # Errors
/// - `read_dir` 失败或权限不足时返回 IO 错误
fn visit_files(dir: &Path, callback: &mut dyn FnMut(&Path)) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            visit_files(&path, callback)?;
        } else {
            callback(&path);
        }
    }
    Ok(())
}

/// 生成唯一路径：如果路径已存在，追加 `_1`, `_2` 等数字后缀
///
/// 避免模组导入时因重名覆盖已有文件。
/// 处理方式：`path` → 检查存在 → 存在则 `path_stem_1.ext` → 存在则 `path_stem_2.ext` → ...
///
/// # 参数
/// - `path`: 期望的原始路径
///
/// # 返回
/// 不存在的唯一路径
fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    let mut counter = 1;
    loop {
        let candidate = if ext.is_empty() {
            parent.join(format!("{}_{}", stem, counter))
        } else {
            parent.join(format!("{}_{}.{}", stem, counter, ext))
        };
        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

/// 递归移动目录内容（处理合并情况）
///
/// 将 `src` 目录下的所有内容（文件 + 子目录）移动到 `dst` 目录下。
/// 如果 `dst` 中已存在同名文件/目录，则：
/// - 文件：覆盖目标文件（先删除再 rename）
/// - 目录：递归合并（`move_directory_contents` 后再尝试删除空源目录）
///
/// # 参数
/// - `src`: 源目录
/// - `dst`: 目标目录（会自动创建）
///
/// # Errors
/// - `create_dir_all` 失败
/// - `rename` 或 `remove_dir` 失败等 IO 错误
fn move_directory_contents(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let target = dst.join(name);
        if path.is_dir() {
            if target.exists() {
                move_directory_contents(&path, &target)?;
                let _ = fs::remove_dir(&path);
            } else {
                fs::rename(&path, &target)?;
            }
        } else {
            if target.exists() {
                let _ = fs::remove_file(&target);
            }
            fs::rename(&path, &target)?;
        }
    }
    Ok(())
}

/// 导入目录（已解压的模组目录）到分组——深层遍历复制（对齐 NRMM 多层文件夹导入）
///
/// # 行为
/// - `dir_name` = 源目录名，使用 `unique_path` 避免重名（对齐 Dart `getSafeTarget` 的 `_N` 后缀）
/// - 获取源目录相对路径，在 `target_group_dir/dir_name` 下重建完整目录结构
/// - 逐文件流式复制：同盘优先 `fs::rename`（快速路径），失败回退 `fs::copy`
///   （注：为保证「任意文件复制失败 → 仅删除复制目录、保留原目录」的回滚不变式，
///   跨盘场景始终为复制；同盘场景优先 rename 以获得最佳性能，失败时仅删除目标目录）
/// - 符号链接：直接复制链接本体（文件链接拷贝目标内容、目录链接重建链接），
///   不解析、不递归，避免死循环遍历
/// - 任意文件复制失败 → 返回错误并删除已复制的目标目录（取消本次导入）
/// - 全部成功 → 通过回收站回收（trash）原目录
///
/// # 参数
/// - `src`: 源目录路径
/// - `target_group_dir`: 目标分组目录路径
///
/// # 返回
/// 最终安装到的 `PathBuf`（已处理重名）
///
/// # Errors
/// - 源不存在或不是目录
/// - 目录名为空
/// - 目标目录创建失败
/// - 复制操作失败（权限不足、磁盘空间不足等）
pub fn import_directory(src: &Path, target_group_dir: &Path) -> Result<PathBuf> {
    if !src.is_dir() {
        return Err(anyhow!("import_directory: src is not a directory: {:?}", src));
    }

    let dir_name = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    if dir_name.is_empty() {
        return Err(anyhow!("import_directory: src directory name is empty: {:?}", src));
    }

    fs::create_dir_all(target_group_dir)?;

    let target = target_group_dir.join(&dir_name);
    let target = unique_path(&target);

    // 深层遍历复制；失败则回滚（删除已复制目录），原目录保持不变
    if let Err(e) = copy_dir_deep(src, &target) {
        let _ = fs::remove_dir_all(&target);
        return Err(anyhow!("import_directory: 复制失败已回滚: {}", e));
    }

    // 展平单层包裹目录（对齐 Dart unwrapSingleFolderNesting）
    if let Err(e) = unwrap_single_folder_nesting(&target) {
        log::warn!("import_directory: 展平单层目录失败 {:?}: {}", target, e);
    }

    // 成功：回收站回收原目录（不阻断流程，失败仅告警）
    match delete(src) {
        Ok(_) => {}
        Err(e) => log::warn!(
            "import_directory: 回收原目录失败 {:?}: {}，原目录保留",
            src, e
        ),
    }

    Ok(target)
}

/// 深层遍历复制目录树：在 `dst` 下重建与 `src` 相同的目录结构并流式复制每个文件。
///
/// - 同盘文件优先 `fs::rename`（快速路径），否则 `fs::copy`
/// - 符号链接直接复制（不解析、不递归）
/// - 任一股文件复制失败立即返回 `Err`（调用方负责删除 `dst`）
fn copy_dir_deep(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;

    let mut jobs: Vec<(PathBuf, PathBuf)> = Vec::new();
    for entry in WalkDir::new(src).follow_links(false).min_depth(1) {
        let entry = entry?;
        let path = entry.path();
        let rel = match path.strip_prefix(src) {
            Ok(r) => r,
            Err(_) => continue,
        };
        jobs.push((path.to_path_buf(), dst.join(rel)));
    }

    // 并行复制（std::fs + rayon 线程池回退）
    let failures: Vec<String> = jobs
        .par_iter()
        .filter_map(|(s, d)| match copy_one(s, d) {
            Ok(_) => None,
            Err(e) => Some(format!("{}: {}", s.display(), e)),
        })
        .collect();

    if !failures.is_empty() {
        return Err(anyhow!(
            "copy_dir_deep: {} 个文件复制失败\n{}",
            failures.len(),
            failures.join("\n")
        ));
    }
    Ok(())
}

/// 复制单个条目（文件/目录/符号链接）到目标。
fn copy_one(src: &Path, dst: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(src)?;

    // 目录：仅创建（内容由其子项 jobs 处理）
    if meta.is_dir() {
        fs::create_dir_all(dst)?;
        return Ok(());
    }

    // 确保父目录存在
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    // 符号链接：直接复制本体，不解析、不递归（避免死循环）
    if meta.is_symlink() {
        let _ = fs::remove_file(dst);
        copy_symlink_as_is(src, dst)?;
        return Ok(());
    }

    // 普通文件：同盘优先 rename，否则 copy
    let _ = fs::remove_file(dst);
    if paths_on_same_disk(src, dst)
        && fs::rename(src, dst).is_ok() {
            return Ok(());
        }
    fs::copy(src, dst).map(|_| ())?;
    Ok(())
}

/// 复制符号链接本体。
///
/// - 文件符号链接：`fs::copy` 复制其指向的文件内容（安全，不递归）
/// - 目录符号链接：跨平台重建一个指向相同目标的符号链接（保留结构、不遍历）
fn copy_symlink_as_is(src: &Path, dst: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{read_link, symlink};
        match read_link(src) {
            Ok(target) => {
                let _ = fs::remove_file(dst);
                symlink(&target, dst)?;
                Ok(())
            }
            Err(_) => {
                fs::copy(src, dst).map(|_| ())?;
                Ok(())
            }
        }
    }
    #[cfg(windows)]
    {
        match fs::copy(src, dst) {
            Ok(_) => Ok(()),
            Err(_) => {
                use std::os::windows::fs::symlink_dir;
                match fs::read_link(src) {
                    Ok(target) => {
                        let _ = fs::remove_file(dst);
                        symlink_dir(&target, dst)?;
                        Ok(())
                    }
                    Err(_) => Err(anyhow!("copy_symlink_as_is: 无法复制符号链接 {:?}", src)),
                }
            }
        }
    }
}

/// 导入模组到指定分组：统一入口，支持目录和压缩包
///
/// 根据 `fs::metadata` 判断：
/// - `is_dir` → 调用 `import_directory`（成功后包装成 `ExtractResult::Success`）
/// - `is_file` → 调用 `import_mod`（即 `extract_archive`）
/// - 其他（如符号链接、socket 等）→ `Err`
///
/// # 参数
/// - `src_path`: 源路径（文件或目录）
/// - `target_group_dir`: 目标分组目录路径
/// - `password`: 可选压缩包密码
///
/// # 返回
/// `ExtractResult` 枚举（成功或各类失败详情）
///
/// # Panics
/// 不会 panic
///
/// # Errors
/// - `src_path` 既不是文件也不是目录
/// - `metadata` 获取失败（路径不存在、权限不足等）
pub fn import_item(src_path: &Path, target_group_dir: &Path, password: Option<&str>) -> Result<ExtractResult> {
    let meta = fs::metadata(src_path)
        .map_err(|e| anyhow!("import_item: failed to stat {:?}: {}", src_path, e))?;

    if meta.is_dir() {
        let final_path = import_directory(src_path, target_group_dir)?;
        let mod_name = final_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let mut has_ini = false;
        let mut file_count = 0usize;
        let mut counter = |path: &Path| {
            file_count += 1;
            if let Some(ext) = path.extension() {
                if ext.eq_ignore_ascii_case("ini") {
                    has_ini = true;
                }
            }
        };
        let _ = visit_files(&final_path, &mut counter);
        Ok(ExtractResult::Success {
            mod_path: final_path,
            mod_name,
            has_ini,
            file_count,
        })
    } else if meta.is_file() {
        import_mod(src_path, target_group_dir, password)
    } else {
        Err(anyhow!(
            "import_item: {:?} is neither file nor directory",
            src_path
        ))
    }
}

/// 导入模组压缩包到指定分组
///
/// 解压 `archive_path` 压缩包到 `group_dir` 目录下。
/// 实际委托给 `extract_archive` 处理完整流程。
///
/// # 参数
/// - `archive_path`: 压缩包路径
/// - `group_dir`: 目标分组目录路径
/// - `password`: 可选压缩包密码
///
/// # 返回
/// `ExtractResult` 枚举
///
/// # Panics
/// 不会 panic
///
/// # Errors
/// 同 `extract_archive`：7z 未找到、IO 错误、解压失败等
pub fn import_mod(archive_path: &Path, group_dir: &Path, password: Option<&str>) -> Result<ExtractResult> {
    extract_archive(archive_path, group_dir, password)
}

/// 自动导入模组到第一个存在的分组
///
/// 从 `group_1` 开始扫描，找到第一个已存在的分组目录作为目标。
/// 如果所有分组目录（1-100）都不存在，创建 `group_1` 并导入。
/// 上限 100 个分组，超过则默认使用 `group_1`。
///
/// # 参数
/// - `archive_path`: 压缩包路径
/// - `mods_path`: 游戏 Mods 目录路径（会自动拼接 `_MANAGED_`）
/// - `password`: 可选压缩包密码
///
/// # 返回
/// `ExtractResult` 枚举
///
/// # Panics
/// 不会 panic
///
/// # Errors
/// 同 `import_mod`：7z 未找到、IO 错误、解压失败等
pub fn import_mod_auto(archive_path: &Path, mods_path: &Path, password: Option<&str>) -> Result<ExtractResult> {
    use crate::core::constants;
    let managed_dir = mods_path.join(constants::MANAGED_FOLDER);
    if !managed_dir.exists() {
        fs::create_dir_all(&managed_dir)?;
    }

    let limits = compute_limits();
    let mut group_num = 1u32;
    let group_dir = loop {
        let name = format!("group_{}", group_num);
        let path = managed_dir.join(&name);
        if path.exists() {
            break path;
        }
        group_num += 1;
        if group_num > limits.max_groups {
            let path = managed_dir.join("group_1");
            fs::create_dir_all(&path)?;
            break path;
        }
    };

    import_mod(archive_path, &group_dir, password)
}

/// 导出模组目录为压缩包（三级回退：用户 7z CLI → 打包 7z CLI → 自维护压缩）
///
/// 对齐用户要求：优先调用用户级 7z CLI 进行压缩，其次打包的 7z CLI，
/// 最后采用自维护压缩逻辑（`.zip` 用 `zip` crate，`.7z` 用 `sevenz-rust`）。
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
    cmd.arg("a").arg("-y");
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

/// 自维护压缩（第三级回退）
///
/// - `.zip`：使用 `zip` crate 创建 ZIP 归档
/// - `.7z`：使用 `sevenz-rust` 创建 7z 归档
/// - 其他格式：返回错误，提示安装 7z CLI
fn export_internal(mod_dir: &Path, archive_path: &Path, ext: &str) -> Result<()> {
    match ext {
        "zip" => zip_directory(mod_dir, archive_path),
        "7z" => sevenz_rust::compress_to_path(mod_dir, archive_path)
            .map(|_| ())
            .map_err(|e| anyhow!("sevenz-rust 自维护压缩失败: {:?}", e)),
        _ => Err(anyhow!(
            "自维护压缩仅支持 .zip / .7z；导出 .{} 格式请安装 7z CLI",
            ext
        )),
    }
}

/// 使用 `zip` crate 将目录压缩为 ZIP 归档（自维护压缩，第三级回退之一）
///
/// 归档内以模组目录名（`dir_name/`）为根，保留目录结构。
fn zip_directory(dir: &Path, archive_path: &Path) -> Result<()> {
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(archive_path)?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // 以 dir 的父目录为根，使归档内包含 `dir_name/...`
    let root = dir.parent().unwrap_or(dir);
    for entry in WalkDir::new(dir).follow_links(false).min_depth(1) {
        let entry = entry?;
        let path = entry.path();
        let rel = match path.strip_prefix(root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if entry.file_type().is_dir() {
            writer.add_directory(rel_str, options)?;
        } else {
            writer.start_file(rel_str, options)?;
            let data = fs::read(path)?;
            writer.write_all(&data)?;
        }
    }
    writer.finish()?;
    Ok(())
}

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
pub async fn import_mod_cmd(archive_path: String, group_dir: String, password: Option<String>) -> Result<ExtractResult, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<ExtractResult, String> {
        import_mod(
            Path::new(&archive_path),
            Path::new(&group_dir),
            password.as_deref(),
        ).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Tauri 命令：自动导入模组压缩包（自动选择第一个存在的分组）
///
/// 前端调用 `importModAuto` 触发。
/// 使用 `spawn_blocking` 在后台线程解压。
#[tauri::command]
pub async fn import_mod_auto_cmd(archive_path: String, mods_path: String, password: Option<String>) -> Result<ExtractResult, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<ExtractResult, String> {
        import_mod_auto(
            Path::new(&archive_path),
            Path::new(&mods_path),
            password.as_deref(),
        ).map_err(|e| e.to_string())
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
    // 暂停文件监控
    {
        let w = watcher.lock().map_err(|e| e.to_string())?;
        w.pause();
    }
    WATCHER_PAUSED.store(true, Ordering::SeqCst);

    let result = tauri::async_runtime::spawn_blocking(move || -> Vec<ExtractResult> {
        let target = PathBuf::from(&req.target_group_dir);
        let mut outs = Vec::with_capacity(req.items.len());
        for item in req.items {
            let p = PathBuf::from(&item);
            let res = import_item(&p, &target, req.password.as_deref());
            match res {
                Ok(r) => outs.push(r),
                Err(e) => outs.push(ExtractResult::ExtractFailed {
                    message: format!("{}: {}", item, e),
                }),
            }
        }
        outs
    })
    .await
    .map_err(|e| e.to_string());

    // 恢复文件监控
    WATCHER_PAUSED.store(false, Ordering::SeqCst);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    /// 在 `root` 下建一个基础模组目录（含嵌套文件与子目录）
    fn make_mod_dir(root: &Path) -> PathBuf {
        let mod_dir = root.join("MyMod");
        fs::create_dir_all(mod_dir.join("Textures")).unwrap();
        fs::write(mod_dir.join("mod.ini"), "[Section]\nhandled = 1\n").unwrap();
        fs::write(mod_dir.join("Textures").join("tex1.png"), b"PNGDATA").unwrap();
        fs::write(mod_dir.join("Textures").join("tex2.png"), b"PNGDATA2").unwrap();
        mod_dir
    }

    // ---------------- unwrap_single_folder_nesting ----------------

    #[test]
    fn unwrap_single_wrapper_flattens_into_target() {
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("mod_top");
        fs::create_dir_all(top.join("MyArchive")).unwrap();
        fs::write(top.join("MyArchive").join("mod.ini"), b"[x]").unwrap();
        fs::write(top.join("MyArchive").join("tex.png"), b"PNG").unwrap();

        unwrap_single_folder_nesting(&top).unwrap();

        // 包裹目录应被展平，文件上移到 top
        assert!(top.join("mod.ini").exists());
        assert!(top.join("tex.png").exists());
        assert!(!top.join("MyArchive").exists());
    }

    #[test]
    fn unwrap_excluded_metadata_does_not_block_flatten() {
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("mod_top");
        fs::create_dir_all(top.join("MyArchive")).unwrap();
        fs::write(top.join("MyArchive").join("sub.txt"), b"hello").unwrap();
        // modname 是排除项：与其共存时，唯一“相关”条目仍是 MyArchive，应继续展平
        fs::write(top.join("modname"), b"My Mod Name").unwrap();

        unwrap_single_folder_nesting(&top).unwrap();

        assert!(top.join("sub.txt").exists());
        assert!(top.join("modname").exists());
        assert!(!top.join("MyArchive").exists());
    }

    #[test]
    fn unwrap_loops_through_nested_single_wrappers() {
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("mod_top");
        fs::create_dir_all(top.join("A").join("B").join("C")).unwrap();
        fs::write(top.join("A").join("B").join("C").join("deep.txt"), b"x").unwrap();

        unwrap_single_folder_nesting(&top).unwrap();

        // 三层嵌套单层包裹应被循环展平到顶层
        assert!(top.join("deep.txt").exists());
        assert!(!top.join("A").exists());
    }

    #[test]
    fn unwrap_no_flatten_when_multiple_relevant_entries() {
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("mod_top");
        fs::create_dir_all(top.join("FolderA")).unwrap();
        fs::create_dir_all(top.join("FolderB")).unwrap();
        fs::write(top.join("FolderA").join("a.txt"), b"a").unwrap();
        fs::write(top.join("FolderB").join("b.txt"), b"b").unwrap();

        unwrap_single_folder_nesting(&top).unwrap();

        // 两个相关目录 → 不展平
        assert!(top.join("FolderA").join("a.txt").exists());
        assert!(top.join("FolderB").join("b.txt").exists());
    }

    #[test]
    fn unwrap_no_flatten_when_single_entry_is_file() {
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("mod_top");
        fs::create_dir_all(&top).unwrap();
        // 非排除项的单个文件（非目录）→ 停止，不“展平”
        fs::write(top.join("game.exe"), b"BINARY").unwrap();

        unwrap_single_folder_nesting(&top).unwrap();

        assert!(top.join("game.exe").exists());
    }

    // ---------------- export_internal（第三级回退，确定性） ----------------

    #[test]
    fn export_internal_zip_creates_valid_zip() {
        let tmp = TempDir::new().unwrap();
        let mod_dir = make_mod_dir(tmp.path());
        let zip_path = tmp.path().join("MyMod.zip");

        export_internal(&mod_dir, &zip_path, "zip").unwrap();

        assert!(zip_path.exists());
        let mut ar = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
        let names: Vec<String> = (0..ar.len())
            .map(|i| ar.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.iter().any(|n| n.starts_with("MyMod/mod.ini")), "names={:?}", names);
        assert!(names.iter().any(|n| n.contains("Textures/tex1.png")), "names={:?}", names);

        // 验证文件内容可读
        let mut f = ar.by_name("MyMod/mod.ini").unwrap();
        let mut buf = String::new();
        f.read_to_string(&mut buf).unwrap();
        assert!(buf.contains("handled = 1"));
    }

    #[test]
    fn export_internal_7z_creates_valid_7z() {
        let tmp = TempDir::new().unwrap();
        let mod_dir = make_mod_dir(tmp.path());
        let sevenz_path = tmp.path().join("MyMod.7z");

        export_internal(&mod_dir, &sevenz_path, "7z").unwrap();

        assert!(sevenz_path.exists());
        // 7z 文件签名：37 7A BC AF 27 1C
        let mut fd = fs::File::open(&sevenz_path).unwrap();
        let mut sig = [0u8; 6];
        fd.read_exact(&mut sig).unwrap();
        assert_eq!(&sig, &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C], "非法的 7z 签名");
    }

    #[test]
    fn export_internal_unsupported_format_errors() {
        let tmp = TempDir::new().unwrap();
        let mod_dir = make_mod_dir(tmp.path());
        let rar_path = tmp.path().join("MyMod.rar");

        let res = export_internal(&mod_dir, &rar_path, "rar");
        assert!(res.is_err());
    }

    // ---------------- export_mod（端到端契约，不依赖具体层级） ----------------

    #[test]
    fn export_mod_produces_non_empty_archive() {
        let tmp = TempDir::new().unwrap();
        let mod_dir = make_mod_dir(tmp.path());
        let out = tmp.path().join("Exported.zip");

        export_mod(&mod_dir, &out).unwrap();

        let meta = fs::metadata(&out).unwrap();
        assert!(meta.len() > 0, "导出归档不应为空");
    }
}
