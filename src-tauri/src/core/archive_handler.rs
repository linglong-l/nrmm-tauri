use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArchiveType {
    Zip,
    Rar,
    SevenZip,
    Unsupported(String),
}

#[derive(Debug, Clone, Serialize)]
pub enum ExtractResult {
    Success {
        mod_path: PathBuf,
        mod_name: String,
        has_ini: bool,
        file_count: usize,
    },
    PasswordRequired {
        archive_path: PathBuf,
    },
    UnsupportedFormat {
        ext: String,
        message: String,
    },
    NoIniFound {
        extracted_path: PathBuf,
        message: String,
    },
    ExtractFailed {
        message: String,
    },
}

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

pub fn is_supported_archive(path: &Path) -> bool {
    matches!(detect_archive_type(path), ArchiveType::Zip | ArchiveType::Rar | ArchiveType::SevenZip)
}

fn get_7z_path() -> Result<PathBuf> {
    let seven_z_exe = crate::core::constants::SEVEN_Z_EXECUTABLE;
    
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
    
    let which = seven_z_exe;
    if let Ok(output) = Command::new(which).arg("--help").output() {
        if output.status.success() {
            return Ok(PathBuf::from(which));
        }
    }
    
    for cmd in &["7za", "7zr", "7zz", "7z.exe", "7z"] {
        if Command::new(cmd).arg("--help").output().is_ok() {
            return Ok(PathBuf::from(cmd));
        }
    }
    
    Err(anyhow!("7z CLI not found. Please install 7-Zip or ensure 7z binary is in the application directory."))
}

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
    
    let final_dir = flatten_single_directory(&temp_dir)?;
    
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
    
    let _ = fs::remove_dir_all(&temp_dir);
    
    Ok(ExtractResult::Success {
        mod_path: target_mod_path,
        mod_name,
        has_ini,
        file_count,
    })
}

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

pub fn import_mod(archive_path: &Path, group_dir: &Path, password: Option<&str>) -> Result<ExtractResult> {
    extract_archive(archive_path, group_dir, password)
}

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

#[tauri::command]
pub fn is_supported_archive_cmd(path: String) -> bool {
    is_supported_archive(Path::new(&path))
}

#[tauri::command]
pub fn import_mod_cmd(archive_path: String, group_dir: String, password: Option<String>) -> Result<ExtractResult, String> {
    import_mod(
        Path::new(&archive_path),
        Path::new(&group_dir),
        password.as_deref(),
    ).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn import_mod_auto_cmd(archive_path: String, mods_path: String, password: Option<String>) -> Result<ExtractResult, String> {
    import_mod_auto(
        Path::new(&archive_path),
        Path::new(&mods_path),
        password.as_deref(),
    ).map_err(|e| e.to_string())
}
