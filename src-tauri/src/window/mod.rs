use tauri::{AppHandle, Manager, WebviewWindow, Position, LogicalPosition};
use anyhow::Result;

#[tauri::command]
pub fn show_window(app: AppHandle, window_name: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.show().map_err(|e| e.to_string())?;
        w.set_focus().map_err(|e| e.to_string())?;
        w.unminimize().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn hide_window(app: AppHandle, window_name: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn set_window_position(app: AppHandle, window_name: String, x: i32, y: i32) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.set_position(Position::Logical(LogicalPosition::new(x as f64, y as f64)))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn close_window(app: AppHandle, window_name: String) {
    if let Some(w) = app.get_webview_window(&window_name) {
        let _ = w.close();
    }
}

#[tauri::command]
pub fn minimize_window(app: AppHandle, window_name: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.minimize().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn toggle_maximize(app: AppHandle, window_name: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        if w.is_maximized().unwrap_or(false) {
            w.unmaximize().map_err(|e| e.to_string())?;
        } else {
            w.maximize().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn center_window_cmd(app: AppHandle, window_name: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.center().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn is_window_visible(app: AppHandle, window_name: String) -> bool {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.is_visible().unwrap_or(false)
    } else {
        false
    }
}

pub fn show_popup_at_cursor(app: &AppHandle, x: i32, y: i32) -> Result<()> {
    if let Some(window) = app.get_webview_window("game-select") {
        let _ = window.set_position(Position::Logical(LogicalPosition::new(x as f64, y as f64)));
        let _ = window.show();
        let _ = window.set_focus();
    }
    Ok(())
}

pub fn set_transparent_window(window: &WebviewWindow) {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let _ = window.set_decorations(false);
    }
}

pub fn get_main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

pub fn center_window(window: &WebviewWindow) -> Result<()> {
    let _ = window.center();
    Ok(())
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.unminimize();
    }
}

pub fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

pub fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}
