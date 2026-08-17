//! 应用路径配置模块
//!
//! 提供应用所需的各类标准路径：
//! - 配置目录：存储 settings.json 等配置文件
//! - 缓存目录：存储云端数据缓存等临时数据
//! - 数据目录：存储应用数据
//! - 临时解压目录：压缩包导入时的临时工作目录
//!
//! 路径获取失败时回退到当前目录（"."），保证应用不会崩溃

use dirs::{cache_dir, config_dir, data_local_dir};
use std::path::PathBuf;

/// 获取应用配置目录
/// Windows: %APPDATA%\nrmm-tauri
/// macOS: ~/Library/Application Support/nrmm-tauri
/// Linux: ~/.config/nrmm-tauri
pub fn app_config_dir() -> PathBuf {
    config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nrmm-tauri")
}

/// 获取应用缓存目录
/// Windows: %LOCALAPPDATA%\nrmm-tauri
/// macOS: ~/Library/Caches/nrmm-tauri
/// Linux: ~/.cache/nrmm-tauri
pub fn app_cache_dir() -> PathBuf {
    cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nrmm-tauri")
}

/// 获取应用本地数据目录
pub fn app_data_dir() -> PathBuf {
    data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nrmm-tauri")
}

/// 获取设置文件路径（settings.json）
pub fn settings_path() -> PathBuf {
    app_config_dir().join("settings.json")
}

/// 获取云端数据缓存目录
pub fn cloud_cache_dir() -> PathBuf {
    app_cache_dir().join("cloud")
}

/// 获取压缩包临时解压目录
pub fn temp_extract_dir() -> PathBuf {
    std::env::temp_dir().join("nrmm_extract")
}

/// 获取 7z 临时工作目录
pub fn sevenz_temp_dir() -> PathBuf {
    std::env::temp_dir().join("nrmm_7z")
}
