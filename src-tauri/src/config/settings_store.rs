use crate::models::settings::AppSettings;
use crate::config::app_paths;
use anyhow::Result;
use std::fs;
use std::path::Path;
use parking_lot::RwLock;
use once_cell::sync::Lazy;

static SETTINGS: Lazy<RwLock<Option<AppSettings>>> = Lazy::new(|| RwLock::new(None));

pub fn init_settings() -> Result<AppSettings> {
    let path = app_paths::settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let settings = if path.exists() {
        match load_from_file(&path) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Failed to load settings, using defaults: {}", e);
                let default = AppSettings::default();
                save_to_file(&path, &default)?;
                default
            }
        }
    } else {
        let default = AppSettings::default();
        save_to_file(&path, &default)?;
        default
    };
    *SETTINGS.write() = Some(settings.clone());
    Ok(settings)
}

fn load_from_file(path: &Path) -> Result<AppSettings> {
    let content = fs::read_to_string(path)?;
    let settings: AppSettings = serde_json::from_str(&content)?;
    Ok(settings)
}

fn save_to_file(path: &Path, settings: &AppSettings) -> Result<()> {
    let json = serde_json::to_string_pretty(settings)?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, json)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

pub fn get_settings() -> AppSettings {
    SETTINGS.read()
        .as_ref()
        .cloned()
        .unwrap_or_default()
}

pub fn save_settings(settings: &AppSettings) -> Result<()> {
    let path = app_paths::settings_path();
    save_to_file(&path, settings)?;
    *SETTINGS.write() = Some(settings.clone());
    Ok(())
}

pub fn update_settings<F>(updater: F) -> Result<AppSettings>
where F: FnOnce(&mut AppSettings)
{
    let mut settings = get_settings();
    updater(&mut settings);
    save_settings(&settings)?;
    Ok(settings)
}

pub fn reset_settings() -> Result<AppSettings> {
    let default = AppSettings::default();
    save_settings(&default)?;
    Ok(default)
}

pub fn export_settings(path: &Path) -> Result<()> {
    let settings = get_settings();
    let json = serde_json::to_string_pretty(&settings)?;
    fs::write(path, json)?;
    Ok(())
}

pub fn import_settings(path: &Path) -> Result<AppSettings> {
    let content = fs::read_to_string(path)?;
    let settings: AppSettings = serde_json::from_str(&content)?;
    save_settings(&settings)?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_save_load_consistency() {
        let dir = tempdir().unwrap();
        let test_path = dir.path().join("test_settings.json");
        
        let mut settings = AppSettings::default();
        settings.language = "zh-CN".to_string();
        settings.interface_scale = 1.5;
        settings.dark_mode = false;
        
        save_to_file(&test_path, &settings).unwrap();
        let loaded = load_from_file(&test_path).unwrap();
        
        assert_eq!(loaded.language, "zh-CN");
        assert_eq!(loaded.interface_scale, 1.5);
        assert_eq!(loaded.dark_mode, false);
    }

    #[test]
    fn test_default_settings() {
        let default = AppSettings::default();
        assert_eq!(default.language, "en");
        assert_eq!(default.interface_scale, 1.0);
        assert_eq!(default.bg_transparency, 0.7);
        assert_eq!(default.dark_mode, true);
        assert_eq!(default.dynamic_background, true);
    }

    #[test]
    fn test_atomic_write() {
        let dir = tempdir().unwrap();
        let test_path = dir.path().join("atomic_test.json");
        let tmp_path = test_path.with_extension("json.tmp");
        
        let settings = AppSettings::default();
        save_to_file(&test_path, &settings).unwrap();
        
        assert!(test_path.exists());
        assert!(!tmp_path.exists());
        
        let loaded = load_from_file(&test_path).unwrap();
        assert_eq!(loaded.language, "en");
    }

    #[test]
    fn test_export_import_file_operations() {
        let dir = tempdir().unwrap();
        let export_path = dir.path().join("export.json");
        
        let mut settings = AppSettings::default();
        settings.language = "de".to_string();
        settings.bg_transparency = 0.5;
        settings.dynamic_background = false;
        
        save_to_file(&export_path, &settings).unwrap();
        
        let imported = load_from_file(&export_path).unwrap();
        
        assert_eq!(imported.language, "de");
        assert_eq!(imported.bg_transparency, 0.5);
        assert_eq!(imported.dynamic_background, false);
    }

    #[test]
    fn test_create_parent_directories() {
        let dir = tempdir().unwrap();
        let nested_path = dir.path().join("nested").join("dir").join("settings.json");
        
        let settings = AppSettings::default();
        
        if let Some(parent) = nested_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        save_to_file(&nested_path, &settings).unwrap();
        
        assert!(nested_path.exists());
    }
}
