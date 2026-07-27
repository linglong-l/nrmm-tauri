use crate::core::mod_manager;
use crate::core::mod_scanner;
use crate::core::constants;
use crate::models::enums::TargetGame;
use crate::config::settings_store;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::fs;

#[tauri::command]
pub fn get_mods(game: String, mods_path: String) -> Result<mod_scanner::ScanResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let _ = game;
    mod_scanner::scan_mods(&mods_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn check_mods_path_status(game: String, mods_path: String) -> Result<crate::models::enums::ModsPathStatus, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    Ok(mod_scanner::check_mods_path(game, &mods_path))
}

#[tauri::command]
pub fn refresh_mods(game: String, mods_path: String) -> Result<mod_manager::UpdateResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let settings = settings_store::get_settings();
    mod_manager::update_mod_data(game, &mods_path, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn select_mod(game: String, mods_path: String, group_index: u32, mod_index: u32) -> Result<mod_manager::UpdateResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let settings = settings_store::get_settings();
    mod_manager::switch_mod(game, &mods_path, &settings, group_index, mod_index).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn deselect_group_mod(game: String, mods_path: String, group_index: u32) -> Result<mod_manager::UpdateResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let settings = settings_store::get_settings();
    mod_manager::deselect_group_mods(game, &mods_path, &settings, group_index).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_group(game: String, mods_path: String, group_name: Option<String>) -> Result<mod_manager::UpdateResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let managed_folder = mods_path.join(constants::MANAGED_FOLDER);
    if !managed_folder.exists() {
        fs::create_dir_all(&managed_folder).map_err(|e| e.to_string())?;
    }

    // Scan existing group_xx directories to find the first available number
    let mut used_numbers: Vec<u32> = Vec::new();
    if let Ok(entries) = fs::read_dir(&managed_folder) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix("group_") {
                if let Ok(num) = rest.parse::<u32>() {
                    used_numbers.push(num);
                }
            }
        }
    }
    used_numbers.sort();
    let mut group_num = 1u32;
    for used in &used_numbers {
        if *used == group_num {
            group_num += 1;
        } else if *used > group_num {
            break;
        }
    }

    let dir_name = format!("group_{}", group_num);
    let group_path = managed_folder.join(&dir_name);
    fs::create_dir(&group_path).map_err(|e| e.to_string())?;

    // Create ModFolder.ini from template
    let template_str = String::from_utf8_lossy(crate::resources::TEMPLATE_GROUP);
    let ini_content = template_str
        .replace("{group_x}", &dir_name)
        .replace("{x}", &group_num.to_string());
    let ini_path = group_path.join("ModFolder.ini");
    fs::write(&ini_path, ini_content).map_err(|e| e.to_string())?;

    // If custom name provided, rename the directory
    if let Some(custom_name) = group_name {
        let trimmed = custom_name.trim();
        if !trimmed.is_empty() && trimmed != &dir_name {
            let new_group_path = managed_folder.join(trimmed);
            if !new_group_path.exists() {
                fs::rename(&group_path, &new_group_path).map_err(|e| e.to_string())?;
            }
        }
    }

    let settings = settings_store::get_settings();
    mod_manager::update_mod_data(game, &mods_path, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_group(group_path: String) -> Result<(), String> {
    let path = PathBuf::from(group_path);
    if !path.exists() {
        return Err("Group path does not exist".into());
    }
    trash_delete(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_mod(mod_path: String) -> Result<(), String> {
    let path = PathBuf::from(mod_path);
    if !path.exists() {
        return Err("Mod path does not exist".into());
    }
    trash_delete(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_mod(mod_path: String, new_name: String) -> Result<PathBuf, String> {
    let path = PathBuf::from(&mod_path);
    if !path.exists() {
        return Err("Mod path does not exist".into());
    }
    let parent = path.parent().ok_or("Invalid mod path")?;
    let dir_name = path.file_name().unwrap().to_string_lossy().to_string();
    let is_disabled = dir_name.to_uppercase().starts_with("DISABLED");
    let final_name = if is_disabled {
        format!("{}{}", constants::DISABLED_PREFIX, new_name)
    } else {
        new_name.clone()
    };
    let new_path = parent.join(final_name);
    fs::rename(&path, &new_path).map_err(|e| e.to_string())?;
    Ok(new_path)
}

#[tauri::command]
pub fn rename_group(group_path: String, new_name: String) -> Result<PathBuf, String> {
    let path = PathBuf::from(&group_path);
    if !path.exists() {
        return Err("Group path does not exist".into());
    }
    let parent = path.parent().ok_or("Invalid group path")?;
    let new_path = parent.join(new_name);
    fs::rename(&path, &new_path).map_err(|e| e.to_string())?;
    Ok(new_path)
}

#[tauri::command]
pub fn toggle_mod_disabled(mod_path: String, enable: bool) -> Result<(), String> {
    let path = PathBuf::from(&mod_path);
    mod_manager::toggle_mod(&path, enable).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_favorite(mod_path: String) -> Result<bool, String> {
    let path = PathBuf::from(&mod_path);
    let fav_path = path.join(constants::FAV_MARKER);
    if fav_path.exists() {
        fs::remove_file(&fav_path).map_err(|e| e.to_string())?;
        Ok(false)
    } else {
        fs::write(&fav_path, "").map_err(|e| e.to_string())?;
        Ok(true)
    }
}

#[tauri::command]
pub fn is_favorite(mod_path: String) -> bool {
    PathBuf::from(mod_path).join(constants::FAV_MARKER).exists()
}

#[tauri::command]
pub fn open_mod_folder(mod_path: String) -> Result<(), String> {
    let path = PathBuf::from(&mod_path);
    if !path.exists() {
        return Err("Path does not exist".into());
    }
    open_path_in_explorer(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_group_folder(group_path: String) -> Result<(), String> {
    let path = PathBuf::from(&group_path);
    if !path.exists() {
        return Err("Path does not exist".into());
    }
    open_path_in_explorer(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn restore_all_inis(mods_path: String) -> Result<mod_manager::RestoredCount, String> {
    let path = PathBuf::from(&mods_path);
    mod_manager::restore_all_inis(&path).map_err(|e| e.to_string())
}

fn parse_game(game: &str) -> Result<TargetGame, String> {
    match game.to_lowercase().as_str() {
        "genshinimpact" | "genshin" | "gi" => Ok(TargetGame::GenshinImpact),
        "honkaistarrail" | "starrail" | "hsr" => Ok(TargetGame::HonkaiStarRail),
        "wuwa" | "wutheringwaves" | "wuthering waves" => Ok(TargetGame::Wuwa),
        "zzz" | "zenlesszonezero" => Ok(TargetGame::ZZZ),
        "honkaiimpact3rd" | "honkaiimpact3" | "hi3" => Ok(TargetGame::HonkaiImpact3rd),
        _ => Err(format!("Unknown game: {}", game)),
    }
}

fn trash_delete(path: &Path) -> Result<()> {
    match trash::delete(path) {
        Ok(_) => Ok(()),
        Err(e) => {
            log::warn!("Failed to move to trash: {}, falling back to permanent delete", e);
            if path.is_dir() {
                fs::remove_dir_all(path)?;
            } else {
                fs::remove_file(path)?;
            }
            Ok(())
        }
    }
}

fn open_path_in_explorer(path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()?;
    }
    Ok(())
}
