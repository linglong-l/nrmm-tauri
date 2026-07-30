//! 设置存储模块
//!
//! 实现应用设置的内存缓存和磁盘持久化：
//! - 使用 parking_lot::RwLock 实现线程安全的内存缓存
//! - 启动时从磁盘加载，之后优先从内存读取
//! - 写入使用临时文件+rename 原子操作，保证配置文件不会损坏
//! - 支持导入/导出 JSON 格式的设置文件
//!
//! # 原子写入流程
//! 1. 将 JSON 写入 .tmp 临时文件
//! 2. 使用 fs::rename 原子替换目标文件
//! 这样即使中途断电/崩溃，也只会留下临时文件，不会损坏原配置

use crate::models::settings::AppSettings;
use crate::models::enums::TargetGame;
use crate::config::app_paths;
use anyhow::Result;
use std::fs;
use std::path::Path;
use parking_lot::RwLock;
use once_cell::sync::Lazy;

/// 设置内存缓存（全局单例）
/// Lazy 保证首次访问时初始化，RwLock 保证多线程读写安全
/// Option 表示是否已初始化（init_settings 后变为 Some）
static SETTINGS: Lazy<RwLock<Option<AppSettings>>> = Lazy::new(|| RwLock::new(None));

/// 初始化设置（应用启动时调用）
///
/// 流程：
/// 1. 确保配置目录存在
/// 2. 如果 settings.json 存在则加载，否则创建默认设置
/// 3. 加载失败时记录警告并回退到默认设置
/// 4. 调用 fill_defaults 填充缺失的默认值（兼容旧配置）
/// 5. 将设置写入内存缓存
pub fn init_settings() -> Result<AppSettings> {
    let path = app_paths::settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut settings = if path.exists() {
        match load_from_file(&path) {
            Ok(mut s) => {
                // 填充默认值（兼容旧版本配置缺少新字段的情况）
                fill_defaults(&mut s);
                s
            }
            Err(e) => {
                log::warn!("Failed to load settings, using defaults: {}", e);
                let mut default = AppSettings::default();
                fill_defaults(&mut default);
                save_to_file(&path, &default)?;
                default
            }
        }
    } else {
        let mut default = AppSettings::default();
        fill_defaults(&mut default);
        save_to_file(&path, &default)?;
        default
    };
    // 再次确保填充了默认值（防止反序列化缺少字段）
    fill_defaults(&mut settings);
    *SETTINGS.write() = Some(settings.clone());
    Ok(settings)
}

/// 从文件加载设置（内部函数）
fn load_from_file(path: &Path) -> Result<AppSettings> {
    let content = fs::read_to_string(path)?;
    let settings: AppSettings = serde_json::from_str(&content)?;
    Ok(settings)
}

/// 原子写入设置到文件（内部函数）
///
/// 使用临时文件+rename 保证原子性：
/// - 先写入 .json.tmp
/// - 再 rename 覆盖目标文件
/// rename 在同一文件系统上是原子操作
fn save_to_file(path: &Path, settings: &AppSettings) -> Result<()> {
    let json = serde_json::to_string_pretty(settings)?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, json)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

/// 从内存缓存获取设置
///
/// 无 IO 操作，线程安全，读取不阻塞其他读者
pub fn get_settings() -> AppSettings {
    SETTINGS.read()
        .as_ref()
        .cloned()
        .unwrap_or_default()
}

/// 保存设置到磁盘并更新内存缓存
pub fn save_settings(settings: &AppSettings) -> Result<()> {
    let path = app_paths::settings_path();
    save_to_file(&path, settings)?;
    *SETTINGS.write() = Some(settings.clone());
    Ok(())
}

/// 使用更新函数修改设置并保存
///
/// 便捷方法：读取 → 修改 → 保存 原子操作
pub fn update_settings<F>(updater: F) -> Result<AppSettings>
where F: FnOnce(&mut AppSettings)
{
    let mut settings = get_settings();
    updater(&mut settings);
    save_settings(&settings)?;
    Ok(settings)
}

/// 重置为默认设置
pub fn reset_settings() -> Result<AppSettings> {
    let default = AppSettings::default();
    save_settings(&default)?;
    Ok(default)
}

/// 导出设置到指定路径（JSON 格式）
pub fn export_settings(path: &Path) -> Result<()> {
    let settings = get_settings();
    let json = serde_json::to_string_pretty(&settings)?;
    fs::write(path, json)?;
    Ok(())
}

/// 从指定路径导入设置
///
/// 导入成功后自动保存到配置文件并更新内存缓存
pub fn import_settings(path: &Path) -> Result<AppSettings> {
    let content = fs::read_to_string(path)?;
    let settings: AppSettings = serde_json::from_str(&content)?;
    save_settings(&settings)?;
    Ok(settings)
}

/// 切换目标游戏
pub fn set_target_game(game: crate::models::enums::TargetGame) -> Result<AppSettings> {
    update_settings(|s| {
        s.target_game = game;
    })
}

/// 获取当前语言设置
pub fn get_language() -> String {
    get_settings().language
}

/// 填充设置默认值
///
/// 为未设置的字段填充合理默认值：
/// - 各游戏默认目标进程名（GenshinImpact.exe、StarRail.exe等）
/// - 若默认Mods路径存在则自动填充（%APPDATA%\XXMI Launcher\{launcher}\Mods）
/// - 已存在的用户设置不会被覆盖
///
/// # 参数
/// * `settings` - 待填充默认值的设置对象（可变引用）
pub fn fill_defaults(settings: &mut AppSettings) {
    // 填充快捷键默认值（防止旧配置为空字符串）
    if settings.window_hotkey.is_empty() {
        settings.window_hotkey = "Alt+D".to_string();
    }
    if settings.gamepad_hotkey_toggle.is_empty() {
        settings.gamepad_hotkey_toggle = "LB+RB".to_string();
    }
    if settings.search_hotkey.is_empty() {
        settings.search_hotkey = "Alt+S".to_string();
    }

    // 默认进程名映射
    let default_processes: Vec<(TargetGame, &str)> = vec![
        (TargetGame::GenshinImpact, "GenshinImpact.exe"),
        (TargetGame::HonkaiStarRail, "StarRail.exe"),
        (TargetGame::Wuwa, "Client-Win64-Shipping.exe"),
        (TargetGame::ZZZ, "ZenlessZoneZero.exe"),
        (TargetGame::HonkaiImpact3rd, "BH3.exe"),
        (TargetGame::ArknightsEndfield, "Endfield.exe"),
    ];

    // 填充默认进程名（仅当用户未设置时）
    for (game, proc_name) in default_processes {
        settings.target_process_per_game
            .entry(game)
            .or_insert_with(|| proc_name.to_string());
    }

    // XXMI Launcher 默认Mods路径（Windows %APPDATA%）
    // 格式：%APPDATA%\XXMI Launcher\{LauncherFolder}\Mods
    let default_mods_paths: Vec<(TargetGame, &str)> = vec![
        (TargetGame::GenshinImpact, "GIMI"),
        (TargetGame::HonkaiStarRail, "SRMI"),
        (TargetGame::Wuwa, "WWMI"),
        (TargetGame::ZZZ, "ZZMI"),
        (TargetGame::ArknightsEndfield, "EFMI"),
        // HonkaiImpact3rd 暂无XXMI支持，不设置默认路径
    ];

    // 填充默认Mods路径（仅当路径存在时）
    for (game, launcher_dir) in default_mods_paths {
        if settings.game_mods_path.contains_key(&game) {
            continue; // 用户已设置，跳过
        }
        // 尝试构建默认路径
        if let Some(appdata) = dirs::config_dir() {
            let mods_path = appdata
                .join("XXMI Launcher")
                .join(launcher_dir)
                .join("Mods");
            if mods_path.exists() {
                settings.game_mods_path.insert(
                    game,
                    mods_path.to_string_lossy().to_string()
                );
            }
        }
    }
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
        assert!(default.target_process_per_game.is_empty());
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

    #[test]
    fn test_fill_defaults_process_names() {
        let mut settings = AppSettings::default();
        fill_defaults(&mut settings);
        
        // 所有游戏都应该有默认进程名
        for game in TargetGame::all().iter() {
            let proc_name = settings.target_process_per_game.get(game);
            assert!(proc_name.is_some(), "Missing default process for {:?}", game);
            let name = proc_name.unwrap();
            assert!(!name.is_empty(), "Empty process name for {:?}", game);
        }
        
        // 验证具体进程名
        assert_eq!(
            settings.target_process_per_game.get(&TargetGame::HonkaiStarRail),
            Some(&"StarRail.exe".to_string())
        );
        assert_eq!(
            settings.target_process_per_game.get(&TargetGame::ZZZ),
            Some(&"ZenlessZoneZero.exe".to_string())
        );
    }

    #[test]
    fn test_fill_defaults_preserves_user_settings() {
        let mut settings = AppSettings::default();
        // 用户自定义了星铁进程名
        settings.target_process_per_game.insert(
            TargetGame::HonkaiStarRail,
            "CustomProcess.exe".to_string()
        );
        settings.language = "zh-CN".to_string();
        
        fill_defaults(&mut settings);
        
        // 用户自定义值不应被覆盖
        assert_eq!(
            settings.target_process_per_game.get(&TargetGame::HonkaiStarRail),
            Some(&"CustomProcess.exe".to_string())
        );
        // 其他未设置的游戏仍应获得默认值
        assert!(settings.target_process_per_game.get(&TargetGame::ZZZ).is_some());
        // 其他设置字段不受影响
        assert_eq!(settings.language, "zh-CN");
    }

    #[test]
    fn test_default_process_names_match_spec() {
        let mut settings = AppSettings::default();
        fill_defaults(&mut settings);
        
        let expected: Vec<(TargetGame, &str)> = vec![
            (TargetGame::GenshinImpact, "GenshinImpact.exe"),
            (TargetGame::HonkaiStarRail, "StarRail.exe"),
            (TargetGame::Wuwa, "Client-Win64-Shipping.exe"),
            (TargetGame::ZZZ, "ZenlessZoneZero.exe"),
            (TargetGame::HonkaiImpact3rd, "BH3.exe"),
            (TargetGame::ArknightsEndfield, "Endfield.exe"),
        ];
        
        for (game, expected_name) in expected {
            assert_eq!(
                settings.target_process_per_game.get(&game),
                Some(&expected_name.to_string()),
                "Process name mismatch for {:?}", game
            );
        }
    }
}
