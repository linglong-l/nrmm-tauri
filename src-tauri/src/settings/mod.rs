//! 用户设置持久化模块。
//!
//! 定义 [`Settings`] 结构及其加载/保存逻辑。设置以 JSON 格式持久化在
//! 应用数据目录下的 `settings.json` 中。为避免写入过程中因崩溃导致文件损坏，
//! 保存时采用“写入临时文件 → 原子重命名”的策略。
//!
//! ## 字段默认值
//! 所有字段均通过 `#[serde(default = "...")]` 绑定独立的默认值函数，确保：
//! - 文件缺失时整体回退到默认值；
//! - 文件中字段缺失时单字段回退到默认值，向后兼容旧配置。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::{error, warn};
use serde::{Deserialize, Serialize};

use crate::process::TargetGame;

/// 设置文件名（最终落盘文件）。
const SETTINGS_FILE_NAME: &str = "settings.json";

/// 设置临时文件名。
///
/// 保存流程：先写入此临时文件，再原子重命名为 [`SETTINGS_FILE_NAME`]，
/// 避免写入中途崩溃导致主文件损坏。
const SETTINGS_TMP_FILE_NAME: &str = "settings.json.tmp";

/// 用户设置集合。
///
/// 通过 serde 序列化为 JSON 持久化。每个字段都配置了独立的默认值函数，
/// 在反序列化时若字段缺失会自动回填默认值。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// 键盘热键标识（如 `"altW"`）。`"none"` 表示禁用。
    #[serde(default = "default_hotkey_keyboard")]
    pub hotkey_keyboard: String,

    /// 手柄热键标识（如 `"none"`）。当前预留，默认禁用。
    #[serde(default = "default_hotkey_gamepad")]
    pub hotkey_gamepad: String,

    /// 分组搜索快捷键（窗口内绑定，前端 keydown 监听）。默认 "altG"。
    #[serde(default = "default_group_search_hotkey")]
    pub group_search_hotkey: String,

    /// 模组搜索快捷键（窗口内绑定，前端 keydown 监听）。默认 "altF"。
    #[serde(default = "default_mod_search_hotkey")]
    pub mod_search_hotkey: String,

    /// 鸣潮（Wuthering Waves）目标进程名，用于进程匹配。
    #[serde(default = "default_target_process_wuwa")]
    pub target_process_wuwa: String,

    /// 原神（Genshin Impact）目标进程名。
    #[serde(default = "default_target_process_genshin")]
    pub target_process_genshin: String,

    /// 崩坏：星穹铁道（Honkai: Star Rail）目标进程名。
    #[serde(default = "default_target_process_hsr")]
    pub target_process_hsr: String,

    /// 绝区零（Zenless Zone Zero）目标进程名。
    #[serde(default = "default_target_process_zzz")]
    pub target_process_zzz: String,

    /// 明日方舟：终末地（Arknights: Endfield）目标进程名。
    #[serde(default = "default_target_process_endfield")]
    pub target_process_endfield: String,

    /// 鸣潮 Mods 目录路径。空字符串表示未配置。
    #[serde(default = "default_mods_path_wuwa")]
    pub mods_path_wuwa: String,

    /// 原神 Mods 目录路径。
    #[serde(default = "default_mods_path_genshin")]
    pub mods_path_genshin: String,

    /// 星穹铁道 Mods 目录路径。
    #[serde(default = "default_mods_path_hsr")]
    pub mods_path_hsr: String,

    /// 绝区零 Mods 目录路径。
    #[serde(default = "default_mods_path_zzz")]
    pub mods_path_zzz: String,

    /// 终末地 Mods 目录路径。
    #[serde(default = "default_mods_path_endfield")]
    pub mods_path_endfield: String,

    /// UI 整体缩放比例（1.0 = 100%）。
    #[serde(default = "default_overall_scale")]
    pub overall_scale: f64,

    /// 背景透明度（0.0 ~ 1.0，越小越透明）。
    #[serde(default = "default_bg_transparency")]
    pub bg_transparency: f64,

    /// 布局模式编号（由前端定义具体含义）。
    #[serde(default = "default_layout_mode")]
    pub layout_mode: i32,

    /// 界面语言代码（如 `"en"`、`"zh"`）。
    #[serde(default = "default_language")]
    pub language: String,

    /// 主题名称（如 `"light"`、`"dark"`）。
    #[serde(default = "default_theme")]
    pub theme: String,

    /// 是否自动为分组生成图标。
    #[serde(default = "default_is_auto_generate_folder_icon")]
    pub is_auto_generate_folder_icon: bool,

    /// 是否在显示窗口时自动置顶。
    #[serde(default = "default_is_auto_pin_window")]
    pub is_auto_pin_window: bool,

    /// 在游戏外按下热键时是否仍然显示菜单。
    /// `true` 时游戏外热键仍然触发窗口切换。
    #[serde(default = "default_show_menu_when_toggling_outside_game")]
    pub show_menu_when_toggling_outside_game: bool,

    /// 是否启用按键模拟绑定（用于在游戏内模拟按键）。
    #[serde(default = "default_keybind_simulate_keypress")]
    pub keybind_simulate_keypress: bool,

    /// 分组排序方法编号（由前端定义具体含义）。
    #[serde(default = "default_sort_group_method")]
    pub sort_group_method: i32,

    /// 上次保存的窗口宽度（逻辑像素）。
    #[serde(default = "default_saved_window_width")]
    pub saved_window_width: i32,

    /// 上次保存的窗口高度（逻辑像素）。
    #[serde(default = "default_saved_window_height")]
    pub saved_window_height: i32,

    /// 上次保存的窗口左上角 X 坐标（逻辑像素）。
    /// `None` 表示从未保存过，启动时窗口将居中。
    #[serde(default)]
    pub saved_window_x: Option<i32>,

    /// 上次保存的窗口左上角 Y 坐标（逻辑像素）。
    /// `None` 表示从未保存过，启动时窗口将居中。
    #[serde(default)]
    pub saved_window_y: Option<i32>,

    /// 当前选中的目标游戏。用于决定加载哪个游戏的 Mods 与配置。
    #[serde(default = "default_target_game")]
    pub target_game: TargetGame,
}

// ===== 各字段默认值函数 =====
// 独立函数而非闭包，便于在 serde 属性与 Default impl 中复用。

/// 默认键盘热键：`Alt+D`。
/// 注意：此默认值仅影响新创建的配置文件；已有配置文件不受影响（serde default 仅在字段缺失时调用）。
fn default_hotkey_keyboard() -> String {
    "altD".to_string()
}

/// 默认手柄热键：禁用。
fn default_hotkey_gamepad() -> String {
    "none".to_string()
}

/// 分组搜索快捷键默认值。
fn default_group_search_hotkey() -> String {
    "altG".to_string()
}

/// 模组搜索快捷键默认值。
fn default_mod_search_hotkey() -> String {
    "altF".to_string()
}

/// 默认鸣潮进程名。
///
/// 注意：Linux 用户通过 Wine/Proton 运行游戏时进程名与 Windows 不同，
/// 需在设置中手动配置目标进程名。
fn default_target_process_wuwa() -> String {
    "Wuthering Waves.exe".to_string()
}

/// 默认原神进程名。
///
/// 注意：Linux 用户通过 Wine/Proton 运行游戏时进程名与 Windows 不同，
/// 需在设置中手动配置目标进程名。
fn default_target_process_genshin() -> String {
    "GenshinImpact.exe".to_string()
}

/// 默认星铁进程名。
///
/// 注意：Linux 用户通过 Wine/Proton 运行游戏时进程名与 Windows 不同，
/// 需在设置中手动配置目标进程名。
fn default_target_process_hsr() -> String {
    "StarRail.exe".to_string()
}

/// 默认绝区零进程名。
///
/// 注意：Linux 用户通过 Wine/Proton 运行游戏时进程名与 Windows 不同，
/// 需在设置中手动配置目标进程名。
fn default_target_process_zzz() -> String {
    "ZenlessZoneZero.exe".to_string()
}

/// 默认终末地进程名。
///
/// 注意：Linux 用户通过 Wine/Proton 运行游戏时进程名与 Windows 不同，
/// 需在设置中手动配置目标进程名。
fn default_target_process_endfield() -> String {
    "Endfield-Win64-Shipping.exe".to_string()
}

/// 默认鸣潮 Mods 路径：空字符串（未配置）。
fn default_mods_path_wuwa() -> String {
    String::new()
}

/// 默认原神 Mods 路径：空字符串。
fn default_mods_path_genshin() -> String {
    String::new()
}

/// 默认星铁 Mods 路径：空字符串。
fn default_mods_path_hsr() -> String {
    String::new()
}

/// 默认绝区零 Mods 路径：空字符串。
fn default_mods_path_zzz() -> String {
    String::new()
}

/// 默认终末地 Mods 路径：空字符串。
fn default_mods_path_endfield() -> String {
    String::new()
}

/// 默认整体缩放：1.0（100%）。
fn default_overall_scale() -> f64 {
    1.0
}

/// 默认背景透明度：0.85。
fn default_bg_transparency() -> f64 {
    0.85
}

/// 默认布局模式：0。
fn default_layout_mode() -> i32 {
    0
}

/// 默认语言：英语。
fn default_language() -> String {
    "en".to_string()
}

/// 默认主题：浅色。
fn default_theme() -> String {
    "light".to_string()
}

/// 默认开启自动生成分组图标。
fn default_is_auto_generate_folder_icon() -> bool {
    true
}

/// 默认关闭自动置顶窗口（普通优先级）。
fn default_is_auto_pin_window() -> bool {
    false
}

/// 默认在游戏外按热键时显示菜单。
fn default_show_menu_when_toggling_outside_game() -> bool {
    true
}

/// 默认关闭按键模拟绑定。
fn default_keybind_simulate_keypress() -> bool {
    false
}

/// 默认分组排序方法：0。
fn default_sort_group_method() -> i32 {
    0
}

/// 默认窗口宽度：800 逻辑像素。
fn default_saved_window_width() -> i32 {
    800
}

/// 默认窗口高度：600 逻辑像素。
fn default_saved_window_height() -> i32 {
    600
}

/// 默认目标游戏：鸣潮。
fn default_target_game() -> TargetGame {
    TargetGame::WutheringWaves
}

impl Settings {
    /// 构造默认设置（等价于 [`Settings::default`]）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 校验并修复设置中的非法值。
    ///
    /// 对超出合理范围的数值自动钳位到有效范围；
    /// 对空字符串的关键字段回填默认值。
    pub fn validate_and_fix(&mut self) {
        self.overall_scale = self.overall_scale.clamp(0.5, 2.0);
        self.bg_transparency = self.bg_transparency.clamp(0.0, 1.0);
        self.layout_mode = self.layout_mode.clamp(0, 2);
        self.sort_group_method = self.sort_group_method.clamp(0, 1);
        if self.theme.is_empty() {
            self.theme = default_theme();
        }
        if self.language.is_empty() {
            self.language = default_language();
        }
        self.saved_window_width = self.saved_window_width.max(400);
        self.saved_window_height = self.saved_window_height.max(300);
    }

    /// 拼接设置文件的完整路径。
    ///
    /// # 参数
    /// - `app_data_dir`：应用数据目录。
    ///
    /// # 返回值
    /// 返回 `<app_data_dir>/settings.json` 的路径。
    pub fn settings_file_path(app_data_dir: &Path) -> PathBuf {
        app_data_dir.join(SETTINGS_FILE_NAME)
    }

    /// 从应用数据目录同步加载设置。
    ///
    /// # 业务逻辑
    /// 1. 若设置文件不存在，直接返回默认值（首次启动场景）；
    /// 2. 若读取成功但解析失败（配置损坏），记录警告并回退到默认值；
    /// 3. 若读取失败（IO 错误），记录错误并回退到默认值。
    ///
    /// # 参数
    /// - `app_data_dir`：应用数据目录。
    ///
    /// # 返回值
    /// 始终返回一个有效的 [`Settings`]（可能是默认值），不会返回错误。
    pub fn load(app_data_dir: &Path) -> Self {
        let path = Self::settings_file_path(app_data_dir);

        // 文件不存在视为首次启动，直接使用默认值
        if !path.exists() {
            log::info!("Settings file not found at {:?}, using defaults", path);
            return Self::default();
        }

        match fs::read_to_string(&path) {
            Ok(content) => {
                let trimmed = content.trim();
                if trimmed.is_empty() {
                    warn!("Settings file {:?} is empty, using defaults", path);
                    return Self::default();
                }
                match serde_json::from_str::<Settings>(&content) {
                    Ok(mut settings) => {
                        settings.validate_and_fix();
                        log::info!("Settings loaded successfully from {:?}", path);
                        settings
                    }
                    Err(e) => {
                        warn!("Failed to parse settings file {:?}: {}. Using defaults.", path, e);
                        Self::default()
                    }
                }
            }
            Err(e) => {
                error!("Failed to read settings file {:?}: {}. Using defaults.", path, e);
                Self::default()
            }
        }
    }

    /// 同步保存设置到应用数据目录。
    ///
    /// # 业务逻辑（原子写入）
    /// 1. 确保应用数据目录存在（不存在则创建）；
    /// 2. 序列化为美化后的 JSON；
    /// 3. 写入临时文件 `settings.json.tmp`；
    /// 4. 将临时文件重命名为 `settings.json`（在大多数文件系统上是原子操作）。
    ///
    /// 这样即使写入过程中崩溃，主设置文件也不会被部分写入破坏。
    ///
    /// # 参数
    /// - `app_data_dir`：应用数据目录。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`，任一步骤失败返回封装后的错误。
    pub fn save(&self, app_data_dir: &Path) -> Result<()> {
        // 确保目录存在，避免后续写入失败
        fs::create_dir_all(app_data_dir)
            .with_context(|| format!("Failed to create app data dir: {:?}", app_data_dir))?;

        let path = app_data_dir.join(SETTINGS_FILE_NAME);
        let tmp_path = app_data_dir.join(SETTINGS_TMP_FILE_NAME);

        // 使用 pretty 格式以便用户阅读与手动编辑
        let json = serde_json::to_string_pretty(self)
            .context("Failed to serialize settings")?;

        // 先写临时文件，确保内容完整后再替换主文件
        fs::write(&tmp_path, json)
            .with_context(|| format!("Failed to write temporary settings file: {:?}", tmp_path))?;

        // 原子重命名：替换主文件
        fs::rename(&tmp_path, &path)
            .with_context(|| format!("Failed to rename temp file to settings file: {:?}", path))?;

        log::debug!("Settings saved to {:?}", path);
        Ok(())
    }

    /// 将当前设置重置为默认值（原地替换）。
    ///
    /// 当前未暴露到 UI，保留用于后续「恢复默认设置」功能。
    #[allow(dead_code)]
    pub fn reset_to_default(&mut self) {
        *self = Self::default();
    }

    /// 异步加载设置。
    ///
    /// 通过 `tokio::task::spawn_blocking` 在阻塞线程池中执行同步 [`Settings::load`]，
    /// 避免阻塞异步运行时。
    ///
    /// # 参数
    /// - `app_data_dir`：应用数据目录。
    ///
    /// # 返回值
    /// 返回加载后的 [`Settings`]；若线程 join 失败则返回默认值。
    ///
    /// 当前启动流程使用同步 [`Settings::load`]，本函数保留用于后续异步初始化场景。
    #[allow(dead_code)]
    pub async fn load_async(app_data_dir: &Path) -> Self {
        let app_data_dir = app_data_dir.to_path_buf();
        tokio::task::spawn_blocking(move || Self::load(&app_data_dir))
            .await
            .unwrap_or_default()
    }

    /// 异步保存设置。
    ///
    /// 通过 `tokio::task::spawn_blocking` 在阻塞线程池中执行同步 [`Settings::save`]，
    /// 避免阻塞异步运行时。
    ///
    /// # 参数
    /// - `app_data_dir`：应用数据目录。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`；若线程 join 失败，将 join 错误转换为 `Err`。
    pub async fn save_async(&self, app_data_dir: &Path) -> Result<()> {
        let self_clone = self.clone();
        let app_data_dir = app_data_dir.to_path_buf();
        tokio::task::spawn_blocking(move || self_clone.save(&app_data_dir))
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("Task join error: {}", e)))
    }
}

impl Default for Settings {
    /// 默认设置构造器，调用各字段的默认值函数填充字段。
    fn default() -> Self {
        Self {
            hotkey_keyboard: default_hotkey_keyboard(),
            hotkey_gamepad: default_hotkey_gamepad(),
            group_search_hotkey: default_group_search_hotkey(),
            mod_search_hotkey: default_mod_search_hotkey(),
            target_process_wuwa: default_target_process_wuwa(),
            target_process_genshin: default_target_process_genshin(),
            target_process_hsr: default_target_process_hsr(),
            target_process_zzz: default_target_process_zzz(),
            target_process_endfield: default_target_process_endfield(),
            mods_path_wuwa: default_mods_path_wuwa(),
            mods_path_genshin: default_mods_path_genshin(),
            mods_path_hsr: default_mods_path_hsr(),
            mods_path_zzz: default_mods_path_zzz(),
            mods_path_endfield: default_mods_path_endfield(),
            overall_scale: default_overall_scale(),
            bg_transparency: default_bg_transparency(),
            layout_mode: default_layout_mode(),
            language: default_language(),
            theme: default_theme(),
            is_auto_generate_folder_icon: default_is_auto_generate_folder_icon(),
            is_auto_pin_window: default_is_auto_pin_window(),
            show_menu_when_toggling_outside_game: default_show_menu_when_toggling_outside_game(),
            keybind_simulate_keypress: default_keybind_simulate_keypress(),
            sort_group_method: default_sort_group_method(),
            saved_window_width: default_saved_window_width(),
            saved_window_height: default_saved_window_height(),
            saved_window_x: None,
            saved_window_y: None,
            target_game: default_target_game(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证默认设置结构体字段的非空性。
    #[test]
    fn test_default_settings_non_empty() {
        let settings = Settings::default();
        assert!(!settings.hotkey_keyboard.is_empty());
        assert!(!settings.language.is_empty());
        assert!(!settings.theme.is_empty());
        assert!(!settings.target_process_wuwa.is_empty());
    }

    /// 验证默认设置整体缩放比例为 1.0。
    #[test]
    fn test_default_overall_scale() {
        let settings = Settings::default();
        assert_eq!(settings.overall_scale, 1.0);
    }

    /// 验证默认设置背景透明度为 0.85。
    #[test]
    fn test_default_bg_transparency() {
        let settings = Settings::default();
        assert_eq!(settings.bg_transparency, 0.85);
    }

    /// 验证默认设置目标游戏为鸣潮。
    #[test]
    fn test_default_target_game() {
        let settings = Settings::default();
        assert_eq!(settings.target_game, TargetGame::WutheringWaves);
    }

    /// 验证默认设置键盘热键。
    #[test]
    fn test_default_hotkey_keyboard() {
        let settings = Settings::default();
        assert_eq!(settings.hotkey_keyboard, "altD");
    }

    /// 验证 validate_and_fix 钳位整体缩放。
    #[test]
    fn test_validate_and_fix_clamps_overall_scale() {
        let mut settings = Settings::default();
        settings.overall_scale = 3.0;
        settings.validate_and_fix();
        assert_eq!(settings.overall_scale, 2.0);
    }

    /// 验证 validate_and_fix 钳位下限。
    #[test]
    fn test_validate_and_fix_clamps_lower_bound() {
        let mut settings = Settings::default();
        settings.overall_scale = 0.1;
        settings.validate_and_fix();
        assert_eq!(settings.overall_scale, 0.5);
    }

    /// 验证 validate_and_fix 回填空语言。
    #[test]
    fn test_validate_and_fix_empty_language() {
        let mut settings = Settings::default();
        settings.language = String::new();
        settings.validate_and_fix();
        assert!(!settings.language.is_empty());
    }

    /// 验证 validate_and_fix 回填空主题。
    #[test]
    fn test_validate_and_fix_empty_theme() {
        let mut settings = Settings::default();
        settings.theme = String::new();
        settings.validate_and_fix();
        assert!(!settings.theme.is_empty());
    }

    /// 验证 validate_and_fix 钳位窗口尺寸下限。
    #[test]
    fn test_validate_and_fix_clamps_window_size() {
        let mut settings = Settings::default();
        settings.saved_window_width = 100;
        settings.saved_window_height = 100;
        settings.validate_and_fix();
        assert!(settings.saved_window_width >= 400);
        assert!(settings.saved_window_height >= 300);
    }

    /// 验证 Settings::new() 等价于 default。
    #[test]
    fn test_settings_new_equals_default() {
        let new_settings = Settings::new();
        let default_settings = Settings::default();
        assert_eq!(new_settings.hotkey_keyboard, default_settings.hotkey_keyboard);
        assert_eq!(new_settings.language, default_settings.language);
    }

    /// 验证 settings_file_path 拼接正确。
    #[test]
    fn test_settings_file_path() {
        let path = Settings::settings_file_path(Path::new("/app/data"));
        assert_eq!(path, PathBuf::from("/app/data/settings.json"));
    }

    /// 验证 JSON 序列化使用 camelCase。
    #[test]
    fn test_settings_serialize_camelcase() {
        let settings = Settings::default();
        let json = serde_json::to_string(&settings).unwrap();
        // 字段名应使用 camelCase
        assert!(json.contains("hotkeyKeyboard"));
        assert!(json.contains("targetGame"));
        assert!(json.contains("overallScale"));
    }

    /// 验证 JSON 反序列化 camelCase。
    #[test]
    fn test_settings_deserialize_camelcase() {
        let json = r#"{"hotkeyKeyboard":"altX","language":"zh","theme":"dark"}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.hotkey_keyboard, "altX");
        assert_eq!(settings.language, "zh");
        assert_eq!(settings.theme, "dark");
    }

    /// 验证 JSON 反序列化缺失字段时回退默认值。
    #[test]
    fn test_settings_deserialize_missing_fields() {
        let json = r#"{}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        // 默认值应被填充
        assert_eq!(settings.hotkey_keyboard, "altD");
        assert_eq!(settings.language, "en");
    }

    /// 验证 reset_to_default 重置所有字段。
    #[test]
    fn test_reset_to_default() {
        let mut settings = Settings::default();
        settings.hotkey_keyboard = "altX".to_string();
        settings.language = "zh".to_string();
        settings.reset_to_default();
        assert_eq!(settings.hotkey_keyboard, "altD");
        assert_eq!(settings.language, "en");
    }
}
