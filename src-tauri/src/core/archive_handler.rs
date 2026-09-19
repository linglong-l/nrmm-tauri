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

use crate::core::file_watcher::WatcherGuard;
use crate::core::operation_registry::OperationGuard;
use crate::core::resolution::compute_limits;
use anyhow::{anyhow, Context, Result};
use infer;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use trash::delete;
use walkdir::WalkDir;

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
                mime,
                ext,
                path
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
            log::debug!(
                "detect_archive_type_robust: infer returned None for {:?}, fallback to extension",
                path
            );
            detect_archive_type(path)
        }
        Err(e) => {
            log::debug!(
                "detect_archive_type_robust: infer error for {:?}: {}, fallback to extension",
                path,
                e
            );
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
/// 所有安装包默认将 7z 落在 `exe_dir/7z/` 下（portable / NSIS 用户安装 / deb/rpm 系统安装皆同），
/// 因此第一优先探测 `exe_dir/7z/<EXE>`。其余位置覆盖各 bundle 形态：
/// 1. `exe_dir/7z/<EXE>`            —— portable、NSIS（用户目录）、deb/rpm（`exe_dir`=/usr/lib/<productName>）
/// 2. `exe_dir/resources/7z/<EXE>`  —— 防御性：资源若落入 `resources/` 子目录
/// 3. `exe_dir/../Resources/7z/<EXE>` —— macOS
///
/// 注：本项目已取消 AppImage 打包（Tauri 强依赖 WebKit/glibc 等无法静态链接的 C/C++ 库，
/// AppImage 便携优势不成立，且打包器需联网拉取 runtime）。故不再探测 AppImage 专属路径（$APPDIR 等）。
///
/// # 返回
/// `Some(PathBuf)` 表示找到内置 7z，`None` 表示未找到
fn get_bundled_7z_path() -> Option<PathBuf> {
    let builtin = crate::core::constants::SEVEN_Z_BUILTIN_PATH;
    if let Ok(exe_dir) = std::env::current_exe() {
        if let Some(exe_dir) = exe_dir.parent() {
            let candidates = vec![
                exe_dir.join(builtin),
                exe_dir.join("resources").join(builtin),
                exe_dir.join("../Resources").join(builtin),
            ];
            // 注：已取消 AppImage 打包，不再探测 $APPDIR 等 AppImage 专属路径。
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
    Err(anyhow!(
        "7z CLI not found in system PATH or application bundle."
    ))
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
        use std::os::windows::ffi::OsStrExt;
        use std::path::Prefix;
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
                        let s: String = server
                            .encode_wide()
                            .chain(share.encode_wide())
                            .map(|c| (c as u32).to_string())
                            .collect();
                        return Some(format!("unc_{}", s.to_ascii_lowercase()));
                    }
                    Prefix::VerbatimUNC(server, share) => {
                        let s: String = server
                            .encode_wide()
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
/// - `tmp_parent`: 临时目录的父目录（解压临时区，如 `<Mods>/.tmp`）
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
    tmp_parent: &Path,
    password: Option<&str>,
) -> Result<PathBuf> {
    let temp_dir = tmp_parent.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
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
        if combined.contains("password")
            || combined.contains("Wrong password")
            || combined.contains("Encrypted")
        {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(anyhow!("PASSWORD_REQUIRED"));
        }
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(anyhow!("7z CLI extraction failed: {}", combined));
    }

    Ok(temp_dir)
}

/// 安全拼接 7z 条目名到目标目录，防止路径穿越（zip-slip / 7z-slip，RUSTSEC-2026-0245）
///
/// 拒绝含 `..`、绝对路径前缀（`/`、`C:`）、反斜杠逃逸的条目名。
/// 反斜杠统一按分隔符处理（Windows 兼容），仅保留 `Normal` 组件。
fn safe_join_entry(base: &Path, name: &str) -> std::result::Result<PathBuf, sevenz_rust::Error> {
    if name.is_empty() {
        return Err(sevenz_rust::Error::Other("empty 7z entry name".into()));
    }
    // 反斜杠统一为正斜杠，避免 Windows 下 `\..` 被 components 当作普通文件名段
    let normalized = name.replace('\\', "/");
    let p = Path::new(&normalized);
    let mut safe = base.to_path_buf();
    for comp in p.components() {
        match comp {
            std::path::Component::Normal(s) => safe = safe.join(s),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(sevenz_rust::Error::Other(
                    format!(
                        "unsafe 7z entry name rejected (traversal/absolute/prefix): {}",
                        name
                    )
                    .into(),
                ));
            }
        }
    }
    Ok(safe)
}

/// 使用 `sevenz-rust` crate 自实现解压 7z 压缩包到临时目录
///
/// 三级回退链的第三级（7z 格式）：纯 Rust 实现的 7z 解压，不依赖外部 CLI。
/// 支持密码保护的压缩包。
///
/// # 安全
/// 使用自定义 extract 回调 + `safe_join_entry` 对每个条目名做路径穿越净化，
/// 修复 RUSTSEC-2026-0245（sevenz-rust 0.6.1 `decompress_file` 内部
/// `dest.join(entry.name())` 无净化，恶意条目可覆盖任意可写文件）。
///
/// # 参数
/// - `archive_path`: 7z 压缩包路径
/// - `tmp_parent`: 临时目录的父目录（解压临时区，如 `<Mods>/.tmp`）
/// - `password`: 可选压缩包密码
///
/// # 返回
/// 成功时返回临时目录 `PathBuf`
///
/// # Errors
/// - 临时目录创建失败
/// - `sevenz_rust::decompress_file_with_extract_fn` 解压失败
fn extract_7z_internal(
    archive_path: &Path,
    tmp_parent: &Path,
    password: Option<&str>,
) -> Result<PathBuf> {
    let temp_dir = tmp_parent.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    log::info!(
        "extract_7z_internal: falling back to sevenz-rust for {:?}",
        archive_path
    );

    if password.is_some() {
        // sevenz-rust2 未启用 aes256 feature：密码保护的 7z 应在前两级 7z CLI 中处理
        return Err(anyhow!(
            "Password-protected 7z not supported by internal extractor"
        ));
    }

    let dest_base = temp_dir.clone();
    sevenz_rust::decompress_file_with_extract_fn(
        archive_path,
        &temp_dir,
        |entry, reader, _unsafe_path| {
            // 跳过空名条目：sevenz-rust 自维护压缩产物可能含空名根/目录占位，
            // 无法安全映射路径且无实际文件数据，跳过不丢失内容、无穿越风险。
            if entry.name().is_empty() {
                return Ok(true);
            }
            // 安全修复（RUSTSEC-2026-0245）：忽略 decompress_impl 内部已 join 的
            // _unsafe_path（其用 dest.join(entry.name())，对含 `..` 的条目名会穿越），
            // 改用 entry.name() 重新做安全 join，仅保留 Normal 组件。
            let safe_path = safe_join_entry(&dest_base, entry.name())?;
            if entry.is_directory() {
                if !safe_path.exists() {
                    fs::create_dir_all(&safe_path)
                        .map_err(|e| sevenz_rust::Error::Io(e, "".into()))?;
                }
            } else {
                if let Some(parent) = safe_path.parent() {
                    if !parent.exists() {
                        fs::create_dir_all(parent)
                            .map_err(|e| sevenz_rust::Error::Io(e, "".into()))?;
                    }
                }
                let mut file = fs::File::create(&safe_path).map_err(|e| {
                    sevenz_rust::Error::Io(
                        e,
                        format!("create file: {}", safe_path.display()).into(),
                    )
                })?;
                std::io::copy(reader, &mut file)
                    .map_err(|e| sevenz_rust::Error::Io(e, "".into()))?;
            }
            Ok(true)
        },
    )
    .context("sevenz-rust decompress_file failed (with path traversal protection)")?;

    Ok(temp_dir)
}

/// 使用 `zip` crate 自实现解压 ZIP 压缩包到临时目录
///
/// 三级回退链的第三级（ZIP 格式）：纯 Rust 实现的 ZIP 解压，不依赖外部 CLI。
/// 注意：自实现 ZIP 解压不支持密码保护（密码保护的 ZIP 应在前两级 7z CLI 中处理）。
///
/// # 参数
/// - `archive_path`: ZIP 压缩包路径
/// - `tmp_parent`: 临时目录的父目录（解压临时区，如 `<Mods>/.tmp`）
///
/// # 返回
/// 成功时返回临时目录 `PathBuf`
///
/// # Errors
/// - 临时目录创建失败
/// - 文件打开失败
/// - `zip::ZipArchive::extract` 解压失败
fn extract_zip_internal(archive_path: &Path, tmp_parent: &Path) -> Result<PathBuf> {
    let temp_dir = tmp_parent.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    log::info!(
        "extract_zip_internal: falling back to zip crate for {:?}",
        archive_path
    );

    let file =
        fs::File::open(archive_path).context("extract_zip_internal: failed to open archive")?;
    let mut archive =
        zip::ZipArchive::new(file).context("extract_zip_internal: failed to read ZIP archive")?;
    archive
        .extract(&temp_dir)
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
/// - `tmp_parent`: 临时目录的父目录（解压临时区，如 `<Mods>/.tmp`）
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
    tmp_parent: &Path,
    password: Option<&str>,
) -> Result<PathBuf> {
    let temp_dir = tmp_parent.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
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
    while let Some(header) = archive
        .read_header()
        .context("extract_rar_internal: failed to read RAR header")?
    {
        archive = if header.entry().is_file() {
            header
                .extract_with_base(&temp_dir)
                .context("extract_rar_internal: failed to extract RAR entry")?
        } else {
            header
                .skip()
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
/// 3. **移动到最终位置**：`unique_mod_dir_name` 处理重名，`move_temp_to_target` 同盘原子移动
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
pub fn extract_archive(
    archive_path: &Path,
    target_dir: &Path,
    password: Option<&str>,
) -> Result<ExtractResult> {
    let archive_type = detect_archive_type_robust(archive_path);

    if let ArchiveType::Unsupported(ext) = archive_type {
        return Ok(ExtractResult::UnsupportedFormat {
            ext,
            message: "Unsupported archive format. Please manually extract .zip/.rar/.7z files."
                .to_string(),
        });
    }

    // 解压临时区：`<Mods>/.tmp`（隐藏 dotdir，扫描器自动跳过），与分组目录同处一卷，
    // 保证后续「移动到分组目录」为同盘原子 rename（高效且原子，避免解压中断污染分组目录）。
    let tmp_parent = extract_tmp_dir(target_dir);

    let mut last_error: Option<String> = None;

    // --- 第一级：系统 7z CLI ---
    if let Some(sys_7z) = get_system_7z_path() {
        match extract_with_7z_cli(&sys_7z, archive_path, &tmp_parent, password) {
            Ok(temp_dir) => {
                log::info!(
                    "extract_archive: system 7z succeeded for {:?}",
                    archive_path
                );
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
        match extract_with_7z_cli(&bundled_7z, archive_path, &tmp_parent, password) {
            Ok(temp_dir) => {
                log::info!(
                    "extract_archive: bundled 7z succeeded for {:?}",
                    archive_path
                );
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
        ArchiveType::SevenZip => extract_7z_internal(archive_path, &tmp_parent, password),
        ArchiveType::Zip => {
            if password.is_some() {
                // 自实现 ZIP 不支持密码，直接返回错误
                Err(anyhow!("ZIP password not supported by internal extractor"))
            } else {
                extract_zip_internal(archive_path, &tmp_parent)
            }
        }
        ArchiveType::Rar => extract_rar_internal(archive_path, &tmp_parent, password),
        _ => unreachable!(),
    };

    match internal_result {
        Ok(temp_dir) => {
            log::info!(
                "extract_archive: internal extractor succeeded for {:?}",
                archive_path
            );
            finalize_extraction(archive_path, target_dir, &temp_dir)
        }
        Err(e) => {
            let msg = e.to_string();
            log::error!(
                "extract_archive: all tiers failed for {:?}: {}",
                archive_path,
                msg
            );
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

/// 解析目标分组目录对应的「游戏 Mods 根目录」（用于放置 `.tmp` 临时解压区）。
///
/// `target_dir` 形如 `<Mods>/_MANAGED_/group_X` → Mods 根为其祖父目录；
/// 若层级不足则回退为 `target_dir` 自身。
fn mods_root_of(target_dir: &Path) -> PathBuf {
    target_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| target_dir.to_path_buf())
}

/// 解压临时区目录：`<Mods>/.tmp`（隐藏 dotdir，扫描器自动跳过），
/// 与分组目录同处 Mods 卷，保证后续移动到分组目录为同盘原子 rename。
fn extract_tmp_dir(target_dir: &Path) -> PathBuf {
    mods_root_of(target_dir).join(".tmp")
}

/// 生成唯一模组目录名（Windows 资源管理器风格）：若 `base` 不存在返回 `base`，
/// 否则依次尝试 `base (1)`、`base (2)` ... 直达找到不存在的名称。
///
/// 用于导入解压后移动到分组目录时的重名处理（对齐 `{modname} (1)` 语义）：
/// 若目标分组目录下已存在同名模组，则对移动后的模组重命名以避免覆盖。
fn unique_mod_dir_name(parent: &Path, base: &str) -> String {
    if !parent.join(base).exists() {
        return base.to_string();
    }
    let mut counter = 1u32;
    loop {
        let name = format!("{} ({})", base, counter);
        if !parent.join(&name).exists() {
            return name;
        }
        counter += 1;
    }
}

/// 将解压临时目录整体移动到最终模组目录：同盘优先 `rename`（原子、高效），
/// 失败回退 `copy_dir_deep` + 删除源。目标由调用方保证不冲突（unique_mod_dir_name）。
fn move_temp_to_target(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        // 防御性兜底：unique_mod_dir_name 已保证不冲突，此处仅清理残留
        if dst.is_dir() {
            fs::remove_dir_all(dst)?;
        } else {
            fs::remove_file(dst)?;
        }
    }
    if paths_on_same_disk(src, dst) && fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    if src.is_dir() {
        copy_dir_deep(src, dst)?;
        let _ = fs::remove_dir_all(src);
    } else {
        fs::copy(src, dst)?;
        let _ = fs::remove_file(src);
    }
    Ok(())
}

/// 清理解压临时区：仅当 `.tmp` 目录为空时删除，避免误删并发解压的其它临时目录。
fn cleanup_extract_tmp(tmp_parent: Option<&Path>) {
    if let Some(p) = tmp_parent {
        if let Ok(mut entries) = fs::read_dir(p) {
            if entries.next().is_none() {
                let _ = fs::remove_dir(p);
            }
        }
    }
}

/// 解压后处理：统计、原子移动到最终位置、展平、清理
///
/// 将 `extract_*` 返回的临时目录（位于 `<Mods>/.tmp`）整体同盘移动到分组目录下的
/// 唯一模组目录（重名时 `{modname} (N)`），再展平单层包裹目录。
///
/// 移动发生在「解压完成之后」，且源（`.tmp`）与目的地（分组目录）同处 Mods 卷，
/// 因此 rename 为同盘原子操作——即便移动瞬间进程崩溃，分组目录也只可能处于
/// 「尚未移动」（干净）或「已完整移动」两种状态，不会出现半解压污染分组目录。
///
/// # 参数
/// - `archive_path`: 原始压缩包路径（用于提取模组名称）
/// - `target_dir`: 目标分组目录（`.../Mods/_MANAGED_/group_X`）
/// - `temp_dir`: 解压后的临时目录（位于 `<Mods>/.tmp`）
///
/// # 返回
/// `ExtractResult::Success` 包含最终安装路径、模组名称、INI 检查结果和文件数
fn finalize_extraction(
    archive_path: &Path,
    target_dir: &Path,
    temp_dir: &Path,
) -> Result<ExtractResult> {
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

    // 展平单层包裹目录（对齐 Dart unwrapSingleFolderNesting）：压缩包内若仅包裹单一根目录，
    // 将其内容提升为模组目录直接内容。
    //
    // ⚠️ 必须在「移动到最终位置之前」于 `.tmp` 暂存目录上执行：`.tmp` 隐藏目录被文件监视器
    // 过滤（file_watcher 回调中的 `.tmp` 排除规则），不会触发轻量扫描写入 `modname` 等标记文件；
    // 若改在移动到分组目录之后执行，监视器会在 300ms 防抖窗口内写入 `modname`，使顶层「相关条目」
    // 不再是单一目录，导致展平判定失败、智能解压退化为常规解压并额外消耗 CPU（B 修复）。
    if let Err(e) = unwrap_single_folder_nesting(&final_dir) {
        log::warn!(
            "finalize_extraction: 展平单层目录失败 {:?}: {}",
            final_dir,
            e
        );
    }

    // 重名处理：目标分组目录下已存在同名模组时，重命名为 `{modname} (N)`
    let final_name = unique_mod_dir_name(target_dir, &mod_name);
    let target_mod_path = target_dir.join(&final_name);

    // 同盘原子移动到最终位置（解压完成后才移动，避免中断污染分组目录）
    move_temp_to_target(&final_dir, &target_mod_path)?;

    // 清理：temp_dir 已被 rename 移走；尝试删除空的 `.tmp` 父目录
    cleanup_extract_tmp(temp_dir.parent());

    Ok(ExtractResult::Success {
        mod_path: target_mod_path,
        mod_name: final_name,
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
    if paths_on_same_disk(src, dst) && fs::rename(src, dst).is_ok() {
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
    if crate::core::constants::ICON_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(*ext))
    {
        return true;
    }
    false
}

/// 遍历目录下所有文件（BFS + 路径池防 symlink 循环），对每个文件调用回调
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
    use std::collections::VecDeque;
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    // 路径池（visited + symlink 登记）：防 symlink 目录循环/重复遍历
    let mut pool = crate::core::fs_walk::PathPool::new();
    queue.push_back(dir.to_path_buf());
    pool.try_visit(dir);
    // 非法 UTF-8 名称计数（仅日志汇总，不上报前端）
    let stats = crate::core::fs_walk::TraversalStats::default();

    while let Some(current) = queue.pop_front() {
        let entries = fs::read_dir(&current)?;
        let current_display = current.to_string_lossy().to_string();
        for entry in entries {
            let entry = entry?;
            let ft = entry.file_type()?;
            let path = entry.path();
            // UTF-8 合法化：非法名称条目跳过（结构性路径）
            if crate::core::fs_walk::valid_entry_name(
                &entry,
                &current_display,
                crate::core::fs_walk::kind::FILE,
                &stats,
            )
            .is_none()
            {
                continue;
            }
            if is_dir_like(&entry, &ft) {
                // 目录（含 symlink 指向目录）：路径池判重后入队
                if pool.try_visit(&path) {
                    queue.push_back(path);
                }
            } else if ft.is_file() {
                callback(&path);
            }
        }
    }
    let (skipped, _entries) = stats.into_parts();
    if skipped > 0 {
        log::warn!("[visit_files] 跳过 {} 个非法 UTF-8 名称条目", skipped);
    }
    Ok(())
}

/// 判断目录项是否为「真实目录或指向目录的 symlink」（symlink 目录跟随，路径池防环）
fn is_dir_like(entry: &std::fs::DirEntry, ft: &std::fs::FileType) -> bool {
    if ft.is_dir() {
        return true;
    }
    ft.is_symlink()
        && entry
            .path()
            .metadata()
            .map(|md| md.is_dir())
            .unwrap_or(false)
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
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
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
        return Err(anyhow!(
            "import_directory: src is not a directory: {:?}",
            src
        ));
    }

    let dir_name = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    if dir_name.is_empty() {
        return Err(anyhow!(
            "import_directory: src directory name is empty: {:?}",
            src
        ));
    }

    // 暂停文件监视器：深层复制期间高密度写入目标会触发监视器扫描并生成 `modname` 标记，
    // 导致后续单层展平判定失败（B 修复）。守卫引用计数，函数退出自动恢复。
    let _watch_guard = WatcherGuard::new();

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
            src,
            e
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

    // 符号链接 / junction：跳过，不重建、不跟随（阻断导入逃逸到 Mods 根外，R3）
    // 正常 mod 不含符号链接；保留它会把指向 Mods 外的 junction 带进导入目录，
    // 后续扫描 / 还原遍历该 junction 会在外部创建 / 改写 INI。
    if meta.is_symlink() {
        log::warn!(
            "copy_one: skip symlink/junction (not imported to prevent escape): {:?}",
            src
        );
        return Ok(());
    }

    // 普通文件：同盘优先 rename，否则 copy
    let _ = fs::remove_file(dst);
    if paths_on_same_disk(src, dst) && fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    fs::copy(src, dst).map(|_| ())?;
    Ok(())
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
pub fn import_item(
    src_path: &Path,
    target_group_dir: &Path,
    password: Option<&str>,
) -> Result<ExtractResult> {
    // 跨进程建议锁：确保同一游戏 Mods 目录仅一个 NRMM 进程执行导入，避免文件竞争
    // （与 update_mod_data / 备份共用同一锁文件，互斥覆盖整个游戏 Mods 根）。
    let mods_root = mods_root_of(target_group_dir);
    let _proc_lock = match crate::core::file_lock::AdvisoryLock::acquire_blocking(
        &mods_root,
        Duration::from_secs(30),
    ) {
        Ok(Some(l)) => l,
        Ok(None) => return Err(anyhow!("另一个 NRMM 实例正在操作该游戏目录，请稍后再试")),
        Err(e) => return Err(e),
    };

    // 重量级后台操作登记：供用户主动退出时检查「是否仍在导入」，若有则取消退出。
    let _op_guard = OperationGuard::new("import_mod");

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
pub fn import_mod(
    archive_path: &Path,
    group_dir: &Path,
    password: Option<&str>,
) -> Result<ExtractResult> {
    // 暂停文件监视器：解压会把大量文件写入分组目录，监视器若实时扫描会写入 `modname` 等标记、
    // 触发增量扫描管线，既浪费 CPU 也是在「先于移动执行展平」之前导致智能解压退化的诱因
    // （B 修复）。守卫为引用计数，函数退出（含错误路径）自动恢复监听。
    let _watch_guard = WatcherGuard::new();
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
pub fn import_mod_auto(
    archive_path: &Path,
    mods_path: &Path,
    password: Option<&str>,
) -> Result<ExtractResult> {
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

    // 跨进程建议锁：与 import_item / update_mod_data / 备份共用同一锁文件
    let mods_root = mods_root_of(&group_dir);
    let _proc_lock = match crate::core::file_lock::AdvisoryLock::acquire_blocking(
        &mods_root,
        Duration::from_secs(30),
    ) {
        Ok(Some(l)) => l,
        Ok(None) => return Err(anyhow!("另一个 NRMM 实例正在操作该游戏目录，请稍后再试")),
        Err(e) => return Err(e),
    };

    // 重量级后台操作登记：供用户主动退出时检查「是否仍在导入」，若有则取消退出。
    let _op_guard = OperationGuard::new("import_mod");

    import_mod(archive_path, &group_dir, password)
}

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
fn export_internal(mod_dir: &Path, archive_path: &Path, ext: &str) -> Result<()> {
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
    fn export_internal_zip_compression_rejected() {
        let tmp = TempDir::new().unwrap();
        let mod_dir = make_mod_dir(tmp.path());
        let zip_path = tmp.path().join("MyMod.zip");

        // 自维护压缩仅支持 .7z；zip 压缩须在 7z CLI 层完成，此处应被拒绝且不产生文件
        let res = export_internal(&mod_dir, &zip_path, "zip");
        assert!(res.is_err(), "自维护 zip 压缩应被拒绝");
        assert!(!zip_path.exists(), "不应生成 zip 文件");
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
        assert_eq!(
            &sig,
            &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C],
            "非法的 7z 签名"
        );
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

    // ---------------- 智能解压（需求 2/3）：两条路径均实现单层包裹展平 ----------------

    /// 构造用于导入测试的源模组（顶层单一目录包裹，模拟 Bandizip 式冗余顶层目录）
    fn make_wrapped_archive_src(root: &Path) -> PathBuf {
        let src = root.join("SrcMod");
        fs::create_dir_all(src.join("MyMod").join("Textures")).unwrap();
        fs::write(
            src.join("MyMod").join("mod.ini"),
            b"[Section]\nhandled = 1\n",
        )
        .unwrap();
        fs::write(
            src.join("MyMod").join("Textures").join("tex1.png"),
            b"PNGDATA",
        )
        .unwrap();
        src
    }

    /// 断言解压结果已展平：内容在 mod_path 直接可见，且无冗余 MyMod 包裹目录
    fn assert_flattened(mod_path: &Path) {
        assert!(mod_path.join("mod.ini").exists(), "应展平到直接内容");
        assert!(
            mod_path.join("Textures").join("tex1.png").exists(),
            "子目录内容应上移"
        );
        assert!(!mod_path.join("MyMod").exists(), "冗余顶层目录应被去除");
    }

    #[test]
    fn smart_extract_via_system_7z_cli_flatten_single_top_dir() {
        // 仅当系统/打包 7z 可用时才执行端到端 CLI 路径；否则跳过（不视为失败）
        let tmp = TempDir::new().unwrap();
        let managed = tmp.path().join("Mods").join("_MANAGED_").join("group_1");
        fs::create_dir_all(&managed).unwrap();

        let src = make_wrapped_archive_src(tmp.path());
        // 用 export_internal(.7z) 自维护压缩生成一个「顶层单目录包裹」的 7z（CLI 无密码路径可控）
        let archive = tmp.path().join("SrcMod.7z");
        export_internal(&src, &archive, "7z").unwrap();

        let seven_zip = get_system_7z_path()
            .or_else(get_bundled_7z_path)
            .expect("需要 7z CLI 才能跑 CLI 端到端智能解压测试");
        let _ = seven_zip; // 确认存在后再走 import_mod（其内部会再次查找）

        let result = import_mod(&archive, &managed, None).unwrap();
        match result {
            ExtractResult::Success { mod_path, .. } => assert_flattened(&mod_path),
            other => panic!("预期成功，得到 {:?}", other),
        }
    }

    #[test]
    fn smart_extract_via_internal_7z_flatten_single_top_dir() {
        // 直接走自维护 7z 路径（不依赖系统 7z CLI），验证 finalize_extraction 展平逻辑
        let tmp = TempDir::new().unwrap();
        let managed = tmp.path().join("Mods").join("_MANAGED_").join("group_1");
        fs::create_dir_all(&managed).unwrap();

        let src = make_wrapped_archive_src(tmp.path());
        let archive = tmp.path().join("SrcMod.7z");
        export_internal(&src, &archive, "7z").unwrap();

        // 直接使用自维护 7z 解压到 .tmp 临时区，再 finalize（与 extract_archive 第三级一致）
        let tmp_parent = extract_tmp_dir(&managed);
        let temp_dir = extract_7z_internal(&archive, &tmp_parent, None).unwrap();
        let result = finalize_extraction(&archive, &managed, &temp_dir).unwrap();
        match result {
            ExtractResult::Success { mod_path, .. } => assert_flattened(&mod_path),
            other => panic!("预期成功，得到 {:?}", other),
        }
    }

    #[test]
    fn smart_extract_keeps_multiple_top_entries_unflattened() {
        // 顶层含多个目录 → 保持原结构，不展平
        let tmp = TempDir::new().unwrap();
        let managed = tmp.path().join("Mods").join("_MANAGED_").join("group_1");
        fs::create_dir_all(&managed).unwrap();

        let src = tmp.path().join("SrcMod");
        fs::create_dir_all(src.join("FolderA")).unwrap();
        fs::create_dir_all(src.join("FolderB")).unwrap();
        fs::write(src.join("FolderA").join("a.txt"), b"a").unwrap();
        fs::write(src.join("FolderB").join("b.txt"), b"b").unwrap();
        let archive = tmp.path().join("SrcMod.7z");
        export_internal(&src, &archive, "7z").unwrap();

        let tmp_parent = extract_tmp_dir(&managed);
        let temp_dir = extract_7z_internal(&archive, &tmp_parent, None).unwrap();
        let result = finalize_extraction(&archive, &managed, &temp_dir).unwrap();
        match result {
            ExtractResult::Success { mod_path, .. } => {
                assert!(mod_path.join("FolderA").join("a.txt").exists());
                assert!(mod_path.join("FolderB").join("b.txt").exists());
                // 未展平：保留 FolderA / FolderB 两层
                assert!(mod_path.join("FolderA").exists());
                assert!(mod_path.join("FolderB").exists());
            }
            other => panic!("预期成功，得到 {:?}", other),
        }
    }
}
