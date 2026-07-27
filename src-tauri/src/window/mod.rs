use anyhow::Result;
use tauri::{AppHandle, LogicalPosition, Manager, Position, WebviewWindow};

/// 检测当前操作系统是否为 Windows 11
///
/// 通过调用 Windows API `GetVersionExW` 获取系统版本信息，
/// 判断主版本号是否 >= 10 且构建号 >= 22000（Windows 11 起始构建号）
///
/// # 返回值
///
/// 返回 `true` 表示 Windows 11，否则返回 `false`
#[cfg(target_os = "windows")]
fn is_windows_11() -> bool {
    use windows::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};

    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };

    match unsafe { GetVersionExW(&mut info) } {
        Ok(()) => info.dwMajorVersion >= 10 && info.dwBuildNumber >= 22000,
        Err(err) => {
            log::warn!("Failed to detect Windows version: {:?}", err);
            false
        }
    }
}

/// 为窗口应用平台特定的视觉效果
///
/// 根据不同操作系统应用对应的窗口效果：
/// - Windows: 依次尝试 MicaDark、TabbedDark、Acrylic、Blur 效果
/// - macOS: 应用 HUD Window、Sidebar、FullScreenUI、UnderWindowBackground 效果
/// - Linux: 尝试启用模糊效果，不支持时回退到不透明窗口
///
/// 仅在 Windows 11 及以上版本启用透明效果
pub fn apply_window_effects(window: &WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        if !is_windows_11() {
            log::info!("Skipping transparent window effects on non-Windows 11 systems");
            return;
        }

        let effects = tauri::window::EffectsBuilder::new()
            .effects([
                tauri::window::Effect::MicaDark,
                tauri::window::Effect::TabbedDark,
                tauri::window::Effect::Acrylic,
                tauri::window::Effect::Blur,
            ])
            .build();

        if let Err(e) = window.set_effects(effects) {
            log::warn!("Failed to set window effects: {}", e);
        }
    }

    #[cfg(target_os = "macos")]
    {
        let effects = tauri::window::EffectsBuilder::new()
            .effects([
                tauri::window::Effect::HudWindow,
                tauri::window::Effect::Sidebar,
                tauri::window::Effect::FullScreenUI,
                tauri::window::Effect::UnderWindowBackground,
            ])
            .build();
        if let Err(e) = window.set_effects(effects) {
            log::warn!("Failed to set window effects: {}", e);
        }
    }

    #[cfg(target_os = "linux")]
    {
        match window.set_effects(
            tauri::window::EffectsBuilder::new()
                .effects([tauri::window::Effect::Blur])
                .build(),
        ) {
            Ok(_) => log::info!("Window blur effect enabled on Linux"),
            Err(e) => log::info!(
                "Window effects not supported on this Linux environment: {} (fallback to opaque)",
                e
            ),
        }
    }
}

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

/// 设置窗口为透明无边框模式
///
/// 在 Windows 和 macOS 上移除窗口装饰（标题栏、边框等）
///
/// # 参数
///
/// * `window` - 目标窗口引用
pub fn set_transparent_window(window: &WebviewWindow) {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let _ = window.set_decorations(false);
    }
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
/// 查找并显示 "main" 窗口，同时获取焦点和取消最小化状态
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
}

/// 隐藏主窗口
///
/// 查找并隐藏 "main" 窗口
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
pub fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

/// 切换主窗口显示/隐藏状态
///
/// 如果主窗口当前可见则隐藏，否则显示并获取焦点
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
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
