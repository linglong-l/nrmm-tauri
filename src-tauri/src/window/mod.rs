//! 窗口管理模块
//!
//! 提供窗口操作的封装函数和 Tauri 命令：
//! - show/hide/toggle: 显示/隐藏/切换窗口
//! - minimize/maximize/center: 最小化/最大化/居中窗口
//! - set_position: 设置窗口位置
//! - close: 关闭窗口
//! - hard_quit: 强制退出（绕过关闭拦截，注销热键后退出）
//! - 主窗口快捷函数（show_main_window 等）

use anyhow::Result;
use tauri::{AppHandle, Emitter, LogicalPosition, Manager, Position, WebviewWindow};


/// 显示指定窗口并获取焦点
///
/// 通过窗口名称查找窗口，然后依次执行：显示窗口、获取焦点、取消最小化
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
/// * `window_name` - 窗口标识名称
///
/// # 返回值
///
/// 成功返回 `Ok(())`，失败返回错误信息字符串
#[tauri::command]
pub fn show_window(app: AppHandle, window_name: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.show().map_err(|e| e.to_string())?;
        w.set_focus().map_err(|e| e.to_string())?;
        w.unminimize().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 隐藏指定窗口
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
/// * `window_name` - 窗口标识名称
///
/// # 返回值
///
/// 成功返回 `Ok(())`，失败返回错误信息字符串
#[tauri::command]
pub fn hide_window(app: AppHandle, window_name: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 设置窗口在屏幕上的位置
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
/// * `window_name` - 窗口标识名称
/// * `x` - 新的 X 坐标（逻辑像素）
/// * `y` - 新的 Y 坐标（逻辑像素）
///
/// # 返回值
///
/// 成功返回 `Ok(())`，失败返回错误信息字符串
#[tauri::command]
pub fn set_window_position(
    app: AppHandle,
    window_name: String,
    x: i32,
    y: i32,
) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.set_position(Position::Logical(LogicalPosition::new(x as f64, y as f64)))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 关闭指定窗口
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
/// * `window_name` - 窗口标识名称
#[tauri::command]
pub fn close_window(app: AppHandle, window_name: String) {
    if let Some(w) = app.get_webview_window(&window_name) {
        let _ = w.close();
    }
}

/// 最小化指定窗口
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
/// * `window_name` - 窗口标识名称
///
/// # 返回值
///
/// 成功返回 `Ok(())`，失败返回错误信息字符串
#[tauri::command]
pub fn minimize_window(app: AppHandle, window_name: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.minimize().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 切换窗口最大化状态
///
/// 如果窗口当前已最大化，则还原；否则最大化窗口
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
/// * `window_name` - 窗口标识名称
///
/// # 返回值
///
/// 成功返回 `Ok(())`，失败返回错误信息字符串
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

/// 将窗口居中显示在屏幕上
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
/// * `window_name` - 窗口标识名称
///
/// # 返回值
///
/// 成功返回 `Ok(())`，失败返回错误信息字符串
#[tauri::command]
pub fn center_window_cmd(app: AppHandle, window_name: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.center().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 检查窗口是否可见
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
/// * `window_name` - 窗口标识名称
///
/// # 返回值
///
/// 窗口存在且可见返回 `true`，否则返回 `false`
#[tauri::command]
pub fn is_window_visible(app: AppHandle, window_name: String) -> bool {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.is_visible().unwrap_or(false)
    } else {
        false
    }
}

/// 在光标位置显示弹出窗口
///
/// 将 "game-select" 窗口移动到指定坐标并显示，常用于右键菜单等场景
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
/// * `x` - 光标的 X 坐标（逻辑像素）
/// * `y` - 光标的 Y 坐标（逻辑像素）
///
/// # 返回值
///
/// 成功返回 `Ok(())`，失败返回错误
pub fn show_popup_at_cursor(app: &AppHandle, x: i32, y: i32) -> Result<()> {
    if let Some(window) = app.get_webview_window("game-select") {
        let _ = window.set_position(Position::Logical(LogicalPosition::new(x as f64, y as f64)));
        let _ = window.show();
        let _ = window.set_focus();
    }
    Ok(())
}

/// 获取主窗口引用
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
///
/// # 返回值
///
/// 找到主窗口返回 `Some(WebviewWindow)`，否则返回 `None`
pub fn get_main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

/// 将窗口居中显示
///
/// # 参数
///
/// * `window` - 目标窗口引用
///
/// # 返回值
///
/// 成功返回 `Ok(())`，失败返回错误
pub fn center_window(window: &WebviewWindow) -> Result<()> {
    let _ = window.center();
    Ok(())
}

/// 显示主窗口
///
/// 查找并显示 "main" 窗口，同时获取焦点和取消最小化状态，
/// 显示后发出 "window-shown" 事件通知前端重新加载数据。
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.unminimize();
    }
    let _ = app.emit("window-shown", ());
}

/// 隐藏主窗口
///
/// 查找并隐藏 "main" 窗口，
/// 隐藏后发出 "window-hidden" 事件通知前端清除数据。
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
pub fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    let _ = app.emit("window-hidden", ());
}

/// 切换主窗口显示/隐藏状态
///
/// 如果主窗口当前可见则隐藏（发出 "window-hidden"），否则显示并获取焦点（发出 "window-shown"）。
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
pub fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
            let _ = app.emit("window-hidden", ());
        } else {
            let _ = window.show();
            let _ = window.set_focus();
            let _ = window.unminimize();
            let _ = app.emit("window-shown", ());
        }
    }
}

/// 重置窗口位置（居中显示）
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
/// * `window_name` - 窗口标识名称
///
/// # 返回值
///
/// 成功返回 `Ok(())`，失败返回错误信息字符串
#[tauri::command]
pub fn reset_window_position(app: AppHandle, window_name: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&window_name) {
        w.center().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 显示主窗口（作为 Command 导出）
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
#[tauri::command]
pub fn show_main_window_cmd(app: AppHandle) {
    show_main_window(&app);
}

/// 切换主窗口显示/隐藏（作为 Command 导出）
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
#[tauri::command]
pub fn toggle_main_window_cmd(app: AppHandle) {
    toggle_main_window(&app);
}

/// 强制退出应用（绕过窗口关闭拦截）
///
/// 先注销所有全局快捷键，然后直接调用 exit 退出进程，
/// 避免 on_window_event CloseRequested 中的 prevent_close 导致无法退出。
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
#[tauri::command]
pub fn hard_quit_app(app: AppHandle) {
    use crate::hotkey::HotkeyManager;
    use std::sync::Arc;
    if let Some(hotkey_mgr) = app.try_state::<Arc<HotkeyManager>>() {
        let _ = hotkey_mgr.unregister_all();
    }
    app.exit(0);
}

/// 获取当前前台窗口的进程名
///
/// 用于热键触发窗口显示时自动检测前台游戏，
/// 跨平台实现：Windows 使用 Win32 API，Linux 使用 X11/Wayland。
///
/// # 返回值
///
/// 成功返回进程名字符串（如 "StarRail.exe"），失败返回错误信息
#[tauri::command]
pub fn get_foreground_process_name() -> Result<String, String> {
    let detector = crate::platform::get_foreground_detector();
    detector.get_foreground_process_name().map_err(|e| e.to_string())
}
