use anyhow::Result;
use crate::models::enums::TargetGame;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;

#[derive(Debug, Clone, serde::Serialize)]
pub struct PlatformInfo {
    pub os: String,
    pub session_type: Option<String>,
    pub transparency_supported: bool,
    pub keypress_supported: bool,
    pub keypress_error: Option<String>,
    pub foreground_detection_supported: bool,
}

pub trait KeySimulator: Send + Sync {
    fn simulate_select_group(&self) -> Result<()>;
    fn simulate_select_mod(&self) -> Result<()>;
    fn check_support(&self) -> Result<(), String>;
}

pub trait ForegroundDetector: Send + Sync {
    fn get_foreground_process_name(&self) -> Result<String>;
    
    fn is_game_foreground(&self, game: TargetGame) -> bool {
        match self.get_foreground_process_name() {
            Ok(name) => {
                let lower = name.to_lowercase();
                game.process_names().iter().any(|pn| pn.to_lowercase() == lower)
            }
            Err(_) => false,
        }
    }
    
    fn get_cursor_position(&self) -> Result<(i32, i32)>;
}

pub fn get_key_simulator() -> Box<dyn KeySimulator> {
    #[cfg(target_os = "windows")]
    { Box::new(windows::WindowsKeySimulator) }
    #[cfg(target_os = "linux")]
    { Box::new(linux::LinuxKeySimulator::new()) }
    #[cfg(target_os = "macos")]
    { Box::new(macos::MacOSKeySimulator) }
}

pub fn get_foreground_detector() -> Box<dyn ForegroundDetector> {
    #[cfg(target_os = "windows")]
    { Box::new(windows::WindowsForegroundDetector) }
    #[cfg(target_os = "linux")]
    { Box::new(linux::LinuxForegroundDetector::new()) }
    #[cfg(target_os = "macos")]
    { Box::new(macos::MacOSForegroundDetector) }
}

#[tauri::command]
pub fn get_platform_info() -> PlatformInfo {
    #[cfg(target_os = "windows")]
    {
        PlatformInfo {
            os: "windows".to_string(),
            session_type: None,
            transparency_supported: true,
            keypress_supported: true,
            keypress_error: None,
            foreground_detection_supported: true,
        }
    }
    #[cfg(target_os = "linux")]
    { linux::get_linux_platform_info() }
    #[cfg(target_os = "macos")]
    {
        PlatformInfo {
            os: "macos".to_string(),
            session_type: None,
            transparency_supported: true,
            keypress_supported: false,
            keypress_error: Some("macOS keypress simulation requires assistive access permission".to_string()),
            foreground_detection_supported: false,
        }
    }
}
