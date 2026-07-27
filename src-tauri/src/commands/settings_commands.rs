use crate::config::settings_store;
use crate::models::settings::AppSettings;

#[tauri::command]
pub fn get_settings() -> AppSettings {
    settings_store::get_settings()
}

#[tauri::command]
pub fn save_settings(settings: AppSettings) -> Result<(), String> {
    settings_store::save_settings(&settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reset_settings() -> Result<AppSettings, String> {
    settings_store::reset_settings().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_settings(path: String) -> Result<(), String> {
    settings_store::export_settings(std::path::Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn import_settings(path: String) -> Result<AppSettings, String> {
    settings_store::import_settings(std::path::Path::new(&path)).map_err(|e| e.to_string())
}
