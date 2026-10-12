//! 压缩包处理 —— 类型契约 / 格式探测 / 7-Zip 解析域
//!
//! 从 archive_handler.rs 拆出（2026-09-19 剩余巨石拆分 PA-3）。
//! 承载压缩包类型枚举（`ArchiveType`）、解压/导入结果枚举（`ExtractResult`）、
//! 批量导入请求（`ImportItemRequest`）、格式探测（魔数优先 + 扩展名回退双保险），
//! 以及 7-Zip 可执行文件的系统 PATH / 内置 bundle 路径解析。
//! 解压实现/编排见 [`crate::core::archive_extract`]，扁平化/导入见
//! [`crate::core::archive_import`]，导出见 [`crate::core::archive_export`]。

use anyhow::{anyhow, Result};
use infer;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

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
pub(crate) fn get_system_7z_path() -> Option<PathBuf> {
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
///
/// 注：本项目已取消 AppImage 打包（Tauri 强依赖 WebKit/glibc 等无法静态链接的 C/C++ 库，
/// AppImage 便携优势不成立，且打包器需联网拉取 runtime）。故不再探测 AppImage 专属路径（$APPDIR 等）。
/// macOS 不在支持范围，`../Resources` 这类 bundle 路径也不再探测。
///
/// # 返回
/// `Some(PathBuf)` 表示找到内置 7z，`None` 表示未找到
pub(crate) fn get_bundled_7z_path() -> Option<PathBuf> {
    let builtin = crate::core::constants::SEVEN_Z_BUILTIN_PATH;
    if let Ok(exe_dir) = std::env::current_exe() {
        if let Some(exe_dir) = exe_dir.parent() {
            let candidates = vec![
                exe_dir.join(builtin),
                exe_dir.join("resources").join(builtin),
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
pub(crate) fn get_7z_path() -> Result<PathBuf> {
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
