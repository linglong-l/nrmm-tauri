//! 主窗口管理模块。
//!
//! 封装对 Tauri 主窗口的所有操作：显示/隐藏/切换、置顶、尺寸、位置、
//! 最小尺寸、状态保存与恢复等。
//!
//! ## 坐标系
//! 所有尺寸与位置均使用 **逻辑像素**（`LogicalSize` / `LogicalPosition`），
//! 通过 `scale_factor` 在物理像素与逻辑像素之间换算，保证在高 DPI 屏幕上的一致性。
//!
//! ## 防抖策略
//! `Moved` / `Resized` 事件会在拖动/缩放过程中高频触发。为避免每次事件都触发
//! 设置写入，`lib.rs` 中的事件监听使用 `std::thread::spawn` + `sleep 500ms` 做简单防抖，
//! 而本模块的 [`WindowManager::save_window_state`] 还会进一步比较新旧状态，
//! 仅在状态真正变化时才更新设置字段（避免无意义的写锁竞争）。

use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::settings::Settings;

/// 主窗口的 Tauri 标签（与 `tauri.conf.json` 中的 window label 对应）。
const MAIN_WINDOW_LABEL: &str = "main";

/// 默认窗口宽度（逻辑像素）。
const DEFAULT_WIDTH: f64 = 800.0;

/// 默认窗口高度（逻辑像素）。
const DEFAULT_HEIGHT: f64 = 600.0;

/// 默认最小窗口宽度（逻辑像素），防止窗口被缩到不可用。
const DEFAULT_MIN_WIDTH: f64 = 600.0;

/// 默认最小窗口高度（逻辑像素）。
const DEFAULT_MIN_HEIGHT: f64 = 400.0;

/// 窗口管理器（无状态）。
///
/// 所有方法均通过 `&AppHandle` 操作主窗口，本结构本身不持有数据。
pub struct WindowManager;

impl WindowManager {
    /// 构造空实例。
    pub fn new() -> Self {
        Self
    }

    /// 获取主窗口句柄。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄。
    ///
    /// # 返回值
    /// 找到返回 [`tauri::WebviewWindow`]；未找到返回错误（带标签信息）。
    fn get_main_window(app: &AppHandle) -> Result<tauri::WebviewWindow> {
        app.get_webview_window(MAIN_WINDOW_LABEL)
            .with_context(|| format!("Main window '{}' not found", MAIN_WINDOW_LABEL))
    }

    /// 显示主窗口并获取焦点。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`，显示或聚焦失败返回封装后的错误。
    pub fn show_window(app: &AppHandle) -> Result<()> {
        log::debug!("[WindowManager] Showing main window");
        let window = Self::get_main_window(app)?;
        window.show().context("Failed to show window")?;
        window.set_focus().context("Failed to focus window")?;
        let payload = json!({ "visible": true, "source": "command" });
        log::debug!("[WindowManager] Emitting window-shown: {}", payload);
        let _ = app.emit("window-shown", payload);
        log::debug!("[WindowManager] Main window shown and focused");
        Ok(())
    }

    /// 隐藏主窗口。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`，失败返回封装后的错误。
    pub fn hide_window(app: &AppHandle) -> Result<()> {
        log::debug!("[WindowManager] Hiding main window");
        let window = Self::get_main_window(app)?;
        window.hide().context("Failed to hide window")?;
        let payload = json!({ "visible": false, "source": "command" });
        log::debug!("[WindowManager] Emitting window-hidden: {}", payload);
        let _ = app.emit("window-hidden", payload);
        log::debug!("[WindowManager] Main window hidden");
        Ok(())
    }

    /// 切换主窗口可见性（可见→隐藏 / 隐藏→可见）。
    ///
    /// # 业务逻辑
    /// - 当前可见：隐藏并返回 `Ok(false)`；
    /// - 当前隐藏：显示、聚焦并返回 `Ok(true)`。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄。
    ///
    /// # 返回值
    /// 返回切换后的可见状态（`true` 表示当前可见）。
    pub fn toggle_window(app: &AppHandle) -> Result<bool> {
        let window = Self::get_main_window(app)?;
        let is_visible = window.is_visible().context("Failed to check window visibility")?;
        log::debug!("[WindowManager] Toggling window visibility, currently visible: {}", is_visible);
        let result = if is_visible {
            window.hide().context("Failed to hide window")?;
            false
        } else {
            window.show().context("Failed to show window")?;
            window.set_focus().context("Failed to focus window")?;
            true
        };
        let (event_name, payload) = if result {
            (
                "window-shown",
                json!({ "visible": true, "source": "toggle" }),
            )
        } else {
            (
                "window-hidden",
                json!({ "visible": false, "source": "toggle" }),
            )
        };
        log::debug!(
            "[WindowManager] Emitting {}: {}",
            event_name,
            payload
        );
        let _ = app.emit(event_name, payload);
        log::debug!("[WindowManager] Window toggled, now visible: {}", result);
        Ok(result)
    }

    /// 设置主窗口是否始终置顶。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `on_top`：是否置顶。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`，失败返回封装后的错误。
    pub fn set_always_on_top(app: &AppHandle, on_top: bool) -> Result<()> {
        let window = Self::get_main_window(app)?;
        window
            .set_always_on_top(on_top)
            .context("Failed to set always on top")?;
        Ok(())
    }

    /// 查询主窗口是否处于置顶状态。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄。
    ///
    /// # 返回值
    /// 返回置顶状态；查询失败返回错误。
    pub fn is_always_on_top(app: &AppHandle) -> Result<bool> {
        let window = Self::get_main_window(app)?;
        window
            .is_always_on_top()
            .context("Failed to check always on top")
    }

    /// 设置主窗口尺寸（逻辑像素）。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `width`：目标宽度；
    /// - `height`：目标高度。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`，失败返回封装后的错误。
    pub fn set_size(app: &AppHandle, width: f64, height: f64) -> Result<()> {
        let window = Self::get_main_window(app)?;
        window
            .set_size(tauri::LogicalSize::new(width, height))
            .context("Failed to set window size")?;
        Ok(())
    }

    /// 获取主窗口当前尺寸（逻辑像素）。
    ///
    /// # 业务逻辑
    /// 通过 `scale_factor` 将物理像素转换为逻辑像素后返回。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄。
    ///
    /// # 返回值
    /// 返回 `(width, height)`；任一步骤失败返回错误。
    pub fn get_size(app: &AppHandle) -> Result<(f64, f64)> {
        let window = Self::get_main_window(app)?;
        let webview: &tauri::Webview = window.as_ref();
        let scale_factor = webview
            .window()
            .scale_factor()
            .context("Failed to get scale factor")?;
        let size = webview
            .size()
            .context("Failed to get window size")?
            .to_logical::<f64>(scale_factor);
        Ok((size.width, size.height))
    }

    /// 设置主窗口位置（逻辑像素坐标）。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `x`：左上角 X 坐标；
    /// - `y`：左上角 Y 坐标。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`，失败返回封装后的错误。
    pub fn set_position(app: &AppHandle, x: f64, y: f64) -> Result<()> {
        let window = Self::get_main_window(app)?;
        window
            .set_position(tauri::LogicalPosition::new(x, y))
            .context("Failed to set window position")?;
        Ok(())
    }

    /// 获取主窗口当前位置（逻辑像素坐标）。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄。
    ///
    /// # 返回值
    /// 返回 `(x, y)`；任一步骤失败返回错误。
    #[allow(dead_code)]
    pub fn get_position(app: &AppHandle) -> Result<(f64, f64)> {
        let window = Self::get_main_window(app)?;
        let webview: &tauri::Webview = window.as_ref();
        let scale_factor = webview
            .window()
            .scale_factor()
            .context("Failed to get scale factor")?;
        let pos = webview
            .position()
            .context("Failed to get window position")?
            .to_logical::<f64>(scale_factor);
        Ok((pos.x, pos.y))
    }

    /// 将主窗口居中显示。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`，失败返回封装后的错误。
    #[allow(dead_code)]
    pub fn center_window(app: &AppHandle) -> Result<()> {
        let window = Self::get_main_window(app)?;
        window.center().context("Failed to center window")?;
        Ok(())
    }

    /// 重置主窗口为默认尺寸并居中。
    ///
    /// # 业务逻辑
    /// 1. 将尺寸重置为 [`DEFAULT_WIDTH`] × [`DEFAULT_HEIGHT`]；
    /// 2. 调用系统 API 将窗口居中。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄。
    ///
    /// # 返回值
    /// 任一步骤失败返回封装后的错误。
    pub fn reset_position(app: &AppHandle) -> Result<()> {
        let window = Self::get_main_window(app)?;
        window
            .set_size(tauri::LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT))
            .context("Failed to set default window size")?;
        window.center().context("Failed to center window")?;
        Ok(())
    }

    /// 保存当前窗口状态到设置（防抖式写入）。
    ///
    /// # 业务逻辑
    /// 1. 读取当前窗口的尺寸、位置（逻辑像素）与置顶状态；
    /// 2. 与设置中已保存的值逐字段比较；
    /// 3. **仅当任一字段发生变化时**才获取写锁并更新字段，并记录 debug 日志。
    ///
    /// 该“比较后写”策略可避免高频事件（如 `Moved`/`Resized`）下产生大量无意义的
    /// 写锁竞争与日志噪声。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `settings`：通过 `Arc<RwLock<Settings>>` 共享的设置。
    ///
    /// # 返回值
    /// 读取窗口状态失败返回错误；无变化时返回 `Ok(())` 不写入。
    pub fn save_window_state(app: &AppHandle, settings: &Arc<RwLock<Settings>>) -> Result<()> {
        let window = Self::get_main_window(app)?;
        let webview: &tauri::Webview = window.as_ref();
        let scale_factor = webview
            .window()
            .scale_factor()
            .context("Failed to get scale factor")?;
        let size = webview
            .size()
            .context("Failed to get window size")?
            .to_logical::<f64>(scale_factor);
        let pos = webview
            .position()
            .context("Failed to get window position")?
            .to_logical::<f64>(scale_factor);

        let mut settings = settings.write();

        // 只在状态实际变化时记录日志和更新设置
        // 比较 4 个关键字段：宽、高、X、Y
        // 注意：is_auto_pin_window 是用户偏好设置，不应被窗口状态覆盖
        let has_changes = settings.saved_window_width != size.width as i32
            || settings.saved_window_height != size.height as i32
            || settings.saved_window_x != Some(pos.x as i32)
            || settings.saved_window_y != Some(pos.y as i32);

        if has_changes {
            settings.saved_window_width = size.width as i32;
            settings.saved_window_height = size.height as i32;
            settings.saved_window_x = Some(pos.x as i32);
            settings.saved_window_y = Some(pos.y as i32);

            log::debug!(
                "Window state saved: size=({}, {}), position=({:?}, {:?})",
                settings.saved_window_width,
                settings.saved_window_height,
                settings.saved_window_x,
                settings.saved_window_y
            );
        }

        Ok(())
    }

    /// 从设置恢复主窗口状态。
    ///
    /// # 业务逻辑
    /// 1. 应用保存的尺寸；
    /// 2. 若保存了位置则应用，否则居中；
    /// 3. 若 `is_auto_pin_window` 为真则恢复置顶。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `settings`：通过 `Arc<RwLock<Settings>>` 共享的设置（仅读取）。
    ///
    /// # 返回值
    /// 任一步骤失败返回封装后的错误。
    pub fn restore_window_state(app: &AppHandle, settings: &Arc<RwLock<Settings>>) -> Result<()> {
        let window = Self::get_main_window(app)?;
        let settings = settings.read();

        let width = settings.saved_window_width as f64;
        let height = settings.saved_window_height as f64;
        window
            .set_size(tauri::LogicalSize::new(width, height))
            .context("Failed to restore window size")?;

        // 位置同时存在时直接应用，否则居中
        if let (Some(x), Some(y)) = (settings.saved_window_x, settings.saved_window_y) {
            window
                .set_position(tauri::LogicalPosition::new(x as f64, y as f64))
                .context("Failed to restore window position")?;
        } else {
            window.center().context("Failed to center window")?;
        }

        // 仅在配置为自动置顶时恢复置顶状态
        if settings.is_auto_pin_window {
            window
                .set_always_on_top(true)
                .context("Failed to restore always on top")?;
        }

        log::debug!(
            "Window state restored: size=({}, {}), position=({:?}, {:?}), on_top={}",
            settings.saved_window_width,
            settings.saved_window_height,
            settings.saved_window_x,
            settings.saved_window_y,
            settings.is_auto_pin_window
        );

        Ok(())
    }

    /// 设置主窗口的最小尺寸（逻辑像素）。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `width`：最小宽度；
    /// - `height`：最小高度。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`，失败返回封装后的错误。
    pub fn set_min_size(app: &AppHandle, width: f64, height: f64) -> Result<()> {
        let window = Self::get_main_window(app)?;
        window
            .set_min_size(Some(tauri::LogicalSize::new(width, height)))
            .context("Failed to set min window size")?;
        Ok(())
    }

    /// 根据当前平台应用窗口兼容性调整。
    ///
    /// # 平台差异说明
    /// - **Windows**：无边框 + 透明 + 无阴影，完全使用自定义标题栏；
    /// - **macOS**：无边框 + 透明（保留 `titleBarStyle:Overlay` 效果），
    ///   自定义标题栏提供完整控制；
    /// - **Linux**：
    ///   - X11 环境下透明窗口需要合成器（picom/compton 等），
    ///     若无合成器则透明区域会显示为黑色。
    ///   - 部分窗口管理器（GNOME/KDE 除外）对无边框窗口的拖拽支持有限，
    ///     `data-tauri-drag-region` 可解决大部分情况。
    ///   - Wayland 环境下客户端装饰（CSD）支持较好。
    ///
    /// 该函数当前记录平台信息日志，预留后续细调入口。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄。
    ///
    /// # 返回值
    /// 成功返回 `Ok(())`。
    pub fn apply_platform_window_config(app: &AppHandle) -> Result<()> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let window = Self::get_main_window(app)?;

        #[cfg(target_os = "windows")]
        {
            log::debug!("Applying Windows-specific window configuration");
            let _ = app;
        }

        #[cfg(target_os = "macos")]
        {
            log::debug!("Applying macOS-specific window configuration");
            // macOS 上可考虑设置 titleBarStyle 为 overlay，
            // 让窗口内容延伸到标题栏区域（需配合 contentProtected 等）
            let _ = window.set_decorations(false);
        }

        #[cfg(target_os = "linux")]
        {
            log::debug!("Applying Linux-specific window configuration");
            // Linux 下检测运行环境（X11/Wayland/WSLg），
            // 对于无合成器的 X11 环境，透明窗口可能显示为黑色背景。
            // 这里保留配置，由用户的桌面环境决定是否启用合成。
            // 若后续发现兼容性问题，可在此处动态调整 decorations/transparent。
            let wayland = std::env::var("WAYLAND_DISPLAY").is_ok();
            let xdg_session = std::env::var("XDG_SESSION_TYPE")
                .unwrap_or_else(|_| "unknown".to_string());
            let wslg = std::env::var("WSLENV").is_ok()
                || std::env::var("WSL_DISTRO_NAME").is_ok();

            log::info!(
                "Linux window environment: WAYLAND={}, XDG_SESSION_TYPE={}, WSLg={}",
                wayland,
                xdg_session,
                wslg
            );

            // WSLg 环境下可能存在渲染问题，记录日志便于排查
            if wslg {
                log::warn!("Running under WSLg - transparent window may have rendering issues");
            }

            let _ = window;
        }

        Ok(())
    }

    /// 应用启动时的初始窗口装配。
    ///
    /// # 业务逻辑
    /// 1. 调用 [`Self::apply_platform_window_config`] 应用平台特定配置；
    /// 2. 设置最小尺寸（[`DEFAULT_MIN_WIDTH`] × [`DEFAULT_MIN_HEIGHT`]），防止窗口过小；
    /// 3. 调用 [`Self::restore_window_state`] 恢复上次保存的尺寸/位置/置顶；
    /// 4. 调用 [`Self::show_window`] 确保窗口可见，即使上次退出时处于隐藏状态。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `settings`：通过 `Arc<RwLock<Settings>>` 共享的设置。
    ///
    /// # 返回值
    /// 任一步骤失败返回封装后的错误。
    pub fn setup_initial_window(app: &AppHandle, settings: &Arc<RwLock<Settings>>) -> Result<()> {
        // 先应用平台特定的窗口配置
        if let Err(e) = Self::apply_platform_window_config(app) {
            log::warn!("Failed to apply platform window config: {}", e);
        }
        Self::set_min_size(app, DEFAULT_MIN_WIDTH, DEFAULT_MIN_HEIGHT)?;
        Self::restore_window_state(app, settings)?;
        // 启动时确保窗口可见，即使上次退出时通过关闭按钮隐藏到托盘
        if let Err(e) = Self::show_window(app) {
            log::warn!("Failed to show window on startup: {}", e);
        }
        Ok(())
    }
}

impl Default for WindowManager {
    /// 默认实现等价于 [`WindowManager::new`]。
    fn default() -> Self {
        Self::new()
    }
}
