//! 压缩包处理模块
//!
//! 负责处理模组压缩包的导入，支持格式：ZIP、RAR、7Z。
//! 核心功能：
//! - 自动检测压缩包格式
//! - 调用系统 7-Zip 命令行工具解压
//! - 自动扁平化单层目录（压缩包内只有一个根目录时提升内容）
//! - 检测密码保护的压缩包
//! - 检查解压后是否包含 INI 文件（有效模组标志）
//! - 自动处理重名（追加数字后缀）
//! - 支持指定分组导入和自动选择分组导入

use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;
use serde::{Serialize, Deserialize};

/// 压缩包类型枚举
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArchiveType {
    /// ZIP 格式 (.zip)
    Zip,
    /// RAR 格式 (.rar)
    Rar,
    /// 7-Zip 格式 (.7z)
    SevenZip,
    /// 不支持的格式，包含扩展名
    Unsupported(String),
}

/// 解压结果枚举
#[derive(Debug, Clone, Serialize)]
pub enum ExtractResult {
    /// 解压成功
    Success {
        /// 模组最终安装路径
        mod_path: PathBuf,
        /// 模组名称（从压缩包文件名提取）
        mod_name: String,
        /// 是否包含 INI 文件（判断是否为有效模组）
        has_ini: bool,
        /// 解压的文件总数
        file_count: usize,
    },
    /// 压缩包需要密码
    PasswordRequired {
        /// 压缩包路径
        archive_path: PathBuf,
    },
    /// 不支持的压缩包格式
    UnsupportedFormat {
        /// 文件扩展名
        ext: String,
        /// 错误提示信息
        message: String,
    },
    /// 解压后未找到 INI 文件
    NoIniFound {
        /// 解压临时目录路径
        extracted_path: PathBuf,
        /// 错误提示信息
        message: String,
    },
    /// 解压失败
    ExtractFailed {
        /// 错误信息
        message: String,
    },
}

/// 根据文件扩展名检测压缩包类型
///
/// # 参数
/// - `path`: 压缩包文件路径
///
/// # 返回
/// 检测到的 ArchiveType
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

/// 检查是否为支持的压缩包格式（zip/rar/7z）
pub fn is_supported_archive(path: &Path) -> bool {
    matches!(detect_archive_type(path), ArchiveType::Zip | ArchiveType::Rar | ArchiveType::SevenZip)
}

/// 查找 7-Zip 可执行文件路径
///
/// 搜索顺序：
/// 1. 应用程序所在目录（exe 同级、resources 目录）
/// 2. macOS 的 ../Resources 目录
/// 3. 系统 PATH 中的 7z 命令
/// 4. 备选名称：7za, 7zr, 7zz 等
///
/// # 返回
/// 找到的 7z 可执行文件路径
fn get_7z_path() -> Result<PathBuf> {
    let seven_z_exe = crate::core::constants::SEVEN_Z_EXECUTABLE;
    
    // 优先查找应用程序目录下的 7z
    if let Ok(exe_dir) = std::env::current_exe() {
        if let Some(exe_dir) = exe_dir.parent() {
            let candidates = [
                exe_dir.join(seven_z_exe),
                exe_dir.join("resources").join(seven_z_exe),
                exe_dir.join("../Resources").join(seven_z_exe),
            ];
            for candidate in &candidates {
                if candidate.exists() {
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

/// 解压压缩包到目标目录
///
/// # 实现流程
/// 1. 检测压缩包格式
/// 2. 查找 7z 可执行文件
/// 3. 创建带 UUID 的临时目录（避免冲突）
/// 4. 调用 7z x 命令解压（-y 自动确认，-p 密码）
/// 5. 检测是否密码错误
/// 6. 扁平化单层目录
/// 7. 统计文件数和检查 INI
/// 8. 移动到最终目标位置（处理重名）
/// 9. 清理临时目录
///
/// # 参数
/// - `archive_path`: 压缩包路径
/// - `target_dir`: 目标父目录
/// - `password`: 可选密码
pub fn extract_archive(archive_path: &Path, target_dir: &Path, password: Option<&str>) -> Result<ExtractResult> {
    let archive_type = detect_archive_type(archive_path);
    
    match archive_type {
        ArchiveType::Unsupported(ext) => {
            return Ok(ExtractResult::UnsupportedFormat {
                ext,
                message: format!("Unsupported archive format. Please manually extract .zip/.rar/.7z files."),
            });
        }
        _ => {}
    }
    
    let seven_zip = get_7z_path()?;
    
    // 创建唯一临时目录，避免并发解压冲突
    let temp_dir = target_dir.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;
    
    let mut cmd = Command::new(&seven_zip);
    cmd.arg("x")  // 解压命令（完整路径）
       .arg("-y")  // 自动覆盖确认
       .arg(format!("-o{}", temp_dir.to_string_lossy()));  // 输出目录
    
    if let Some(pw) = password {
        cmd.arg(format!("-p{}", pw));
    } else {
        cmd.arg("-p");  // 无密码
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
/// 这用于处理压缩包内所有内容都在一个根目录下的常见情况。
///
/// 例如：压缩包 -> Mod/mod.ini, Mod/icon.png -> 直接返回 Mod/ 目录
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

/// 生成唯一路径：如果路径已存在，追加 _1, _2 等数字后缀
///
/// # 示例
/// - "Mod" 已存在 -> "Mod_1"
/// - "Mod_1" 已存在 -> "Mod_2"
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
/// - 文件：直接重命名，目标已存在则覆盖
/// - 目录：目标已存在则递归合并，否则直接重命名
fn move_directory_contents(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let target = dst.join(name);
        if path.is_dir() {
            if target.exists() {
                // 目标目录已存在，递归合并
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

/// 导入模组到指定分组目录
///
/// # 参数
/// - `archive_path`: 压缩包路径
/// - `group_dir`: 目标分组目录
/// - `password`: 可选密码
pub fn import_mod(archive_path: &Path, group_dir: &Path, password: Option<&str>) -> Result<ExtractResult> {
    extract_archive(archive_path, group_dir, password)
}

/// 自动导入模组到第一个存在的分组
///
/// 查找逻辑：从 group_1 开始查找第一个存在的分组。
/// 如果前 100 个分组都不存在，创建 group_1。
///
/// # 参数
/// - `archive_path`: 压缩包路径
/// - `mods_path`: 游戏 Mods 根目录
/// - `password`: 可选密码
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

/// Tauri 命令：检查是否支持的压缩包格式
#[tauri::command]
pub fn is_supported_archive_cmd(path: String) -> bool {
    is_supported_archive(Path::new(&path))
}

/// Tauri 命令：导入模组到指定分组
///
/// 7z 解压和文件移动是重度 IO 操作，使用 spawn_blocking 避免阻塞 async 运行时。
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

/// Tauri 命令：自动导入模组（自动选择分组）
///
/// 7z 解压和文件移动是重度 IO 操作，使用 spawn_blocking。
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
