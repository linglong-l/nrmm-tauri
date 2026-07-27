use std::path::PathBuf;
use dirs::{config_dir, cache_dir, data_local_dir};

pub fn app_config_dir() -> PathBuf {
    config_dir().unwrap_or_else(|| PathBuf::from(".")).join("nrmm-tauri")
}

pub fn app_cache_dir() -> PathBuf {
    cache_dir().unwrap_or_else(|| PathBuf::from(".")).join("nrmm-tauri")
}

pub fn app_data_dir() -> PathBuf {
    data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("nrmm-tauri")
}

pub fn settings_path() -> PathBuf {
    app_config_dir().join("settings.json")
}

pub fn cloud_cache_dir() -> PathBuf {
    app_cache_dir().join("cloud")
}

pub fn temp_extract_dir() -> PathBuf {
    std::env::temp_dir().join("nrmm_extract")
}

pub fn sevenz_temp_dir() -> PathBuf {
    std::env::temp_dir().join("nrmm_7z")
}
