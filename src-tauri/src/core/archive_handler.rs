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

use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;
use serde::{Serialize, Deserialize};
use infer;
use fs_extra::dir::{CopyOptions, move_dir, copy};
use trash::delete;
use std::sync::{Arc, Mutex};
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

/// 查找 7-Zip 可执行文件路径
///
/// 搜索优先级（内置 > 系统 PATH）：
/// 1. 应用程序所在目录下的 `constants::SEVEN_Z_BUILTIN_PATH`
/// 2. 应用程序所在目录 `resources/` 下的 `SEVEN_Z_BUILTIN_PATH`
/// 3. macOS 的 `../Resources` 目录下的 `SEVEN_Z_BUILTIN_PATH`
/// 4. 系统 PATH 中的 `7z` 命令（`SEVEN_Z_EXECUTABLE`）
/// 5. 备选名称依次尝试：`7za`, `7zr`, `7zz`, `7z.exe`, `7z`
///
/// # 返回
/// 找到的第一个可用 7z 可执行文件路径
///
/// # Errors
/// - 所有搜索位置均未找到 7z 可执行文件，返回 `anyhow!("7z CLI not found...")`
fn get_7z_path() -> Result<PathBuf> {
    let builtin = crate::core::constants::SEVEN_Z_BUILTIN_PATH;
    let seven_z_exe = crate::core::constants::SEVEN_Z_EXECUTABLE;

    // 优先查找应用程序目录下的内置 7z 路径
    if let Ok(exe_dir) = std::env::current_exe() {
        if let Some(exe_dir) = exe_dir.parent() {
            let candidates = [
                exe_dir.join(builtin),
                exe_dir.join("resources").join(builtin),
                exe_dir.join("../Resources").join(builtin),
            ];
            for candidate in &candidates {
                if candidate.exists() {
                    log::debug!("get_7z_path: found builtin 7z at {:?}", candidate);
                    return Ok(candidate.clone());
                }
            }
        }
    }

    // 检查系统 PATH 中的 7z
    let which = seven_z_exe;
    if let Ok(output) = Command::new(which).arg("--help").output() {
        if output.status.success() {
            return Ok(PathBuf::from(which));
        }
    }

    // 尝试其他常见的 7z 可执行文件名
    for cmd in &["7za", "7zr", "7zz", "7z.exe", "7z"] {
        if Command::new(cmd).arg("--help").output().is_ok() {
            return Ok(PathBuf::from(cmd));
        }
    }

    Err(anyhow!("7z CLI not found. Please install 7-Zip or ensure 7z binary is in the application directory."))
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
        false
    }
}

/// 解压压缩包到目标目录
///
/// # 实现流程
/// 1. **检测压缩包格式**：使用 `detect_archive_type_robust`（魔数优先 + 扩展名回退）
/// 2. **查找 7z**：调用 `get_7z_path`（内置优先，系统 PATH 备选）
/// 3. **创建临时目录**：`target_dir/.extract_tmp_{uuid}` 避免并发解压冲突
/// 4. **调用 7z x 命令**：`-y` 自动确认，`-p{password}` 或 `-p`（空密码尝试）
/// 5. **检测密码错误**：匹配 stdout/stderr 中的 "password"/"Wrong password"/"Encrypted"
/// 6. **扁平化单层目录**：`flatten_single_directory` 处理压缩包内单根目录情况
/// 7. **统计文件数 & 检查 INI**：`visit_files` 递归遍历
/// 8. **移动到最终位置**：`unique_path` 处理重名，`move_directory_contents` 合并移动
/// 9. **清理临时目录**：`remove_dir_all`
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
/// - 7z 可执行文件未找到（`get_7z_path` 失败）
/// - 临时目录创建失败（IO 错误）
/// - 7z 命令执行失败（非密码错误的其他原因）
pub fn extract_archive(archive_path: &Path, target_dir: &Path, password: Option<&str>) -> Result<ExtractResult> {
    let archive_type = detect_archive_type_robust(archive_path);

    if let ArchiveType::Unsupported(ext) = archive_type {
        return Ok(ExtractResult::UnsupportedFormat {
            ext,
            message: "Unsupported archive format. Please manually extract .zip/.rar/.7z files.".to_string(),
        });
    }

    let seven_zip = get_7z_path()?;

    // 创建唯一临时目录，避免并发解压冲突
    let temp_dir = target_dir.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    let mut cmd = Command::new(&seven_zip);
    cmd.arg("x")
       .arg("-y")
       .arg(format!("-o{}", temp_dir.to_string_lossy()));

    if let Some(pw) = password {
        cmd.arg(format!("-p{}", pw));
    } else {
        cmd.arg("-p");
    }

    cmd.arg(archive_path.to_string_lossy().to_string());

    let output = cmd.output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);

        let combined = format!("{}{}", stdout, stderr);
        // 检测是否密码错误
        if combined.contains("password") || combined.contains("Wrong password") || combined.contains("Encrypted") {
            let _ = fs::remove_dir_all(&temp_dir);
            return Ok(ExtractResult::PasswordRequired {
                archive_path: archive_path.to_path_buf(),
            });
        }

        let _ = fs::remove_dir_all(&temp_dir);
        return Ok(ExtractResult::ExtractFailed {
            message: format!("7z extraction failed: {}", combined),
        });
    }

    // 扁平化：如果临时目录内只有一个子目录，将其内容提升一级
    let final_dir = flatten_single_directory(&temp_dir)?;

    let mut has_ini = false;
    let mut file_count = 0usize;
    // 遍历文件统计数量并检查 INI
    let mut counter = |path: &Path| {
        file_count += 1;
        if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("ini") {
                has_ini = true;
            }
        }
    };
    visit_files(&final_dir, &mut counter)?;

    // 使用压缩包文件名（不含扩展名）作为模组名
    let mod_name = archive_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let target_mod_path = target_dir.join(&mod_name);
    // 处理重名：如果已存在则追加 _1, _2 等后缀
    let target_mod_path = unique_path(&target_mod_path);

    // 将临时目录内容移动到最终位置
    move_directory_contents(&final_dir, &target_mod_path)?;

    // 清理临时目录
    let _ = fs::remove_dir_all(&temp_dir);

    Ok(ExtractResult::Success {
        mod_path: target_mod_path,
        mod_name,
        has_ini,
        file_count,
    })
}

/// 扁平化单层目录
///
/// 如果目录内只有一个子目录且没有直接文件，返回该子目录路径。
/// 用于处理压缩包内所有内容都在一个根目录下的常见情况（例如 `mod.zip` 解压后得到 `mod/` → 实际上是 `mod/content`）。
/// 扁平化后将 `mod/content` 提升为 `mod/` 的直接内容。
///
/// # 参数
/// - `dir`: 待检查的目录路径
///
/// # 返回
/// 扁平化后的目录路径（可能是原目录，也可能是其唯一的子目录）
///
/// # Errors
/// - `read_dir` 失败时返回 IO 错误
fn flatten_single_directory(dir: &Path) -> Result<PathBuf> {
    let entries = fs::read_dir(dir)?;
    let mut first_entry = None;
    let mut has_files = false;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            has_files = true;
            break;
        }
        if first_entry.is_none() {
            first_entry = Some(path);
        } else {
            has_files = true;
            break;
        }
    }

    if !has_files {
        if let Some(only_dir) = first_entry {
            if only_dir.is_dir() {
                return Ok(only_dir);
            }
        }
    }

    Ok(dir.to_path_buf())
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

/// 导入目录（已解压的模组目录）到分组
///
/// # 行为
/// - 非目录 → `Err(anyhow!("import_directory: src is not a directory"))`
/// - `dir_name` = 目录名，使用 `unique_path` 避免重名
/// - `same_disk` = `paths_on_same_disk(src, target)`
///   - 同盘：`fs_extra::dir::move_dir`（overwrite + copy_inside）
///   - 跨盘：`fs_extra::dir::copy` 后 `trash::delete(src)`（trash 失败仅 warn，不中断流程）
///
/// # 参数
/// - `src`: 源目录路径
/// - `target_group_dir`: 目标分组目录路径
///
/// # 返回
/// 最终安装到的 `PathBuf`（已处理重名）
///
/// # Panics
/// 不会 panic
///
/// # Errors
/// - 源不存在或不是目录
/// - 目录名为空
/// - 目标目录创建失败
/// - 移动/复制操作失败（权限不足、磁盘空间不足等）
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

    let target = target_group_dir.join(&dir_name);
    let target = unique_path(&target);
    fs::create_dir_all(target_group_dir)?;

    let same_disk = paths_on_same_disk(src, &target);
    log::debug!(
        "import_directory: same_disk={} src={:?} target={:?}",
        same_disk, src, target
    );

    let mut options = CopyOptions::new();
    options.overwrite = true;
    options.copy_inside = true;

    if same_disk {
        move_dir(src, target.parent().unwrap_or(target_group_dir), &options)
            .map_err(|e| anyhow!("import_directory move_dir failed: {}", e))?;
    } else {
        copy(src, target.parent().unwrap_or(target_group_dir), &options)
            .map_err(|e| anyhow!("import_directory copy failed: {}", e))?;
        match delete(src) {
            Ok(_) => {}
            Err(e) => log::warn!(
                "import_directory: trash delete failed for {:?}: {}, leaving source intact",
                src, e
            ),
        }
    }

    // 如果源目录名和目标唯一路径名不同（存在重名追加后缀），需要重命名
    let final_path = target_group_dir.join(&dir_name);
    if final_path != target && final_path.exists() {
        // unique_path 已经返回目标路径，但 copy_inside=true 可能会把 src
        // 按原名放到目标目录下。若存在重名则需要把刚复制过来的目录名改为 target
        let src_after_copy = target_group_dir.join(&dir_name);
        if src_after_copy.exists() && src_after_copy != target {
            fs::rename(&src_after_copy, &target)?;
        }
    }

    Ok(target)
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

    let mut group_num = 1u32;
    let group_dir = loop {
        let name = format!("group_{}", group_num);
        let path = managed_dir.join(&name);
        if path.exists() {
            break path;
        }
        group_num += 1;
        if group_num > 100 {
            let path = managed_dir.join("group_1");
            fs::create_dir_all(&path)?;
            break path;
        }
    };

    import_mod(archive_path, &group_dir, password)
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
