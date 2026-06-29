//! 系统托盘管理模块。
//!
//! 提供：
//! - 托盘菜单（每个游戏一个“显示”项 + 重置位置/隐藏/退出）；
//! - 托盘图标与提示文本；
//! - 菜单点击事件分发；
//! - 托盘图标交互（左键单击切换窗口显示）。
//!
//! ## 注意事项
//! 涉及设置落盘的操作（如 [`TrayManager::switch_game_and_show`]）使用
//! `std::thread::spawn` 而非 `tokio::spawn`，原因：
//! - 托盘事件回调运行在 Tauri 主线程的同步上下文中，可能没有活跃的 tokio 运行时；
//! - 使用独立 OS 线程执行阻塞 IO 可避免“Cannot drop a runtime in a context where blocking is disallowed” 等问题。

use std::path::Path;

use anyhow::{Context, Result};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

use crate::process::TargetGame;
use crate::window_manager::WindowManager;

/// 托盘图标唯一标识，用于后续通过 `app.tray_by_id` 检索。
const TRAY_ID: &str = "main-tray";

/// 托盘默认提示文本（鼠标悬停时显示）。
const TRAY_TOOLTIP: &str = "No Reload Mod Manager";

/// 托盘管理器（无状态）。
///
/// 所有方法均通过 `&AppHandle` 操作托盘资源，本结构本身不持有数据。
pub struct TrayManager;

impl TrayManager {
    /// 构造空实例。
    pub fn new() -> Self {
        Self
    }

    /// 创建并注册系统托盘（菜单 + 图标 + 提示文本）。
    ///
    /// # 菜单结构
    /// ```text
    /// Show (WuWa)
    /// Show (Genshin)
    /// Show (HSR)
    /// Show (ZZZ)
    /// Show (Endfield)
    /// ─────────────
    /// Reset Position
    /// Hide
    /// ─────────────
    /// Exit
    /// ```
    ///
    /// # 业务逻辑
    /// 1. 为每个菜单项创建带 id 的 [`MenuItem`]；
    /// 2. 用分隔符组织菜单结构；
    /// 3. 使用应用默认窗口图标作为托盘图标；
    /// 4. `show_menu_on_left_click(false)`：左键单击不弹出菜单（改为触发图标事件）。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄。
    ///
    /// # 返回值
    /// 任一步骤失败返回封装后的错误。
    pub fn setup_tray(app: &AppHandle) -> Result<()> {
        // 各游戏的“显示”菜单项
        let show_wuwa = MenuItem::with_id(app, "show-wuwa", "Show (WuWa)", true, None::<&str>)?;
        let show_genshin = MenuItem::with_id(app, "show-genshin", "Show (Genshin)", true, None::<&str>)?;
        let show_hsr = MenuItem::with_id(app, "show-hsr", "Show (HSR)", true, None::<&str>)?;
        let show_zzz = MenuItem::with_id(app, "show-zzz", "Show (ZZZ)", true, None::<&str>)?;
        let show_endfield = MenuItem::with_id(app, "show-endfield", "Show (Endfield)", true, None::<&str>)?;
        // 窗口操作菜单项
        let reset_position = MenuItem::with_id(app, "reset-position", "Reset Position", true, None::<&str>)?;
        let hide = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
        // 退出菜单项
        let exit = MenuItem::with_id(app, "exit", "Exit", true, None::<&str>)?;

        // 组装菜单：5 个游戏项 + 分隔符 + 重置/隐藏 + 分隔符 + 退出
        let menu = Menu::with_items(
            app,
            &[
                &show_wuwa,
                &show_genshin,
                &show_hsr,
                &show_zzz,
                &show_endfield,
                &tauri::menu::PredefinedMenuItem::separator(app)?,
                &reset_position,
                &hide,
                &tauri::menu::PredefinedMenuItem::separator(app)?,
                &exit,
            ],
        )?;

        // 复用应用默认窗口图标作为托盘图标
        let default_icon = app.default_window_icon().cloned();

        TrayIconBuilder::with_id(TRAY_ID)
            .tooltip(TRAY_TOOLTIP)
            .icon(default_icon.context("Default window icon not found")?)
            .menu(&menu)
            // 左键单击不弹菜单，留给图标事件处理（切换窗口显示）
            .show_menu_on_left_click(false)
            .build(app)?;

        Ok(())
    }

    /// 更新托盘提示文本。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `tooltip`：新的提示文本。
    ///
    /// # 返回值
    /// 托盘未找到时静默返回 `Ok(())`；设置失败返回错误。
    pub fn update_tray_tooltip(app: &AppHandle, tooltip: &str) -> Result<()> {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            tray.set_tooltip(Some(tooltip))?;
        }
        Ok(())
    }

    /// 设置托盘图标。
    ///
    /// # 业务逻辑
    /// - `Some(path)`：从文件加载图片，转为 RGBA 后构造 [`tauri::image::Image`] 并应用；
    /// - `None`：恢复为应用默认窗口图标。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `icon_path`：图标文件路径，`None` 表示恢复默认。
    ///
    /// # 返回值
    /// 托盘未找到时静默返回 `Ok(())`；图像加载或设置失败返回错误。
    #[allow(dead_code)]
    pub fn set_tray_icon(app: &AppHandle, icon_path: Option<&Path>) -> Result<()> {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            match icon_path {
                Some(path) => {
                    // 加载图像并转换为 RGBA8，便于构造 Tauri Image
                    let img = image::open(path)
                        .with_context(|| format!("Failed to open icon from path: {:?}", path))?
                        .to_rgba8();
                    let width = img.width();
                    let height = img.height();
                    let rgba = img.into_raw();
                    let icon = tauri::image::Image::new_owned(rgba, width, height);
                    tray.set_icon(Some(icon))?;
                }
                None => {
                    // 恢复默认图标
                    let default_icon = app.default_window_icon().cloned();
                    tray.set_icon(default_icon)?;
                }
            }
        }
        Ok(())
    }

    /// 处理托盘菜单点击事件。
    ///
    /// # 业务逻辑（按菜单 id 分发）
    /// - `show-*`：切换到对应游戏并显示窗口（[`Self::switch_game_and_show`]）；
    /// - `reset-position`：重置主窗口尺寸并居中；
    /// - `hide`：隐藏主窗口；
    /// - `exit`：退出应用（退出码 0）；
    /// - 其他 id：忽略。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `id`：触发菜单项的字符串 id。
    ///
    /// # 返回值
    /// 子操作失败返回封装后的错误。
    pub fn handle_menu_event(app: &AppHandle, id: &str) -> Result<()> {
        match id {
            "show-wuwa" => {
                Self::switch_game_and_show(app, TargetGame::WutheringWaves)?;
            }
            "show-genshin" => {
                Self::switch_game_and_show(app, TargetGame::GenshinImpact)?;
            }
            "show-hsr" => {
                Self::switch_game_and_show(app, TargetGame::HonkaiStarRail)?;
            }
            "show-zzz" => {
                Self::switch_game_and_show(app, TargetGame::ZenlessZoneZero)?;
            }
            "show-endfield" => {
                Self::switch_game_and_show(app, TargetGame::ArknightsEndfield)?;
            }
            "reset-position" => {
                WindowManager::reset_position(app)?;
            }
            "hide" => {
                WindowManager::hide_window(app)?;
            }
            "exit" => {
                app.exit(0);
            }
            _ => {}
        }
        Ok(())
    }

    /// 处理托盘图标交互事件。
    ///
    /// # 业务逻辑
    /// 仅响应 **左键单击抬起**（`MouseButton::Left` + `MouseButtonState::Up`），
    /// 触发主窗口可见性切换。其他按键/状态忽略。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `event`：托盘图标事件引用。
    ///
    /// # 返回值
    /// 始终返回 `Ok(())`（窗口切换失败仅记录在内部）。
    pub fn handle_tray_icon_event(app: &AppHandle, event: &TrayIconEvent) -> Result<()> {
        if let TrayIconEvent::Click {
            button,
            button_state,
            ..
        } = event
        {
            if *button == MouseButton::Left && *button_state == MouseButtonState::Up {
                let _ = WindowManager::toggle_window(app);
            }
        }
        Ok(())
    }

    /// 切换当前目标游戏并显示主窗口。
    ///
    /// # 业务逻辑
    /// 1. 获取 `AppState`，写锁更新 `settings.target_game` 为指定游戏；
    /// 2. 在独立 OS 线程中将新设置落盘（避免阻塞托盘事件回调，且规避 tokio 上下文限制）：
    ///    - 线程内先短时持有读锁克隆设置，再释放锁后执行同步 `save`；
    /// 3. 调用 [`WindowManager::show_window`] 显示主窗口。
    ///
    /// # 为什么用 `std::thread::spawn` 而非 `tokio::spawn`
    /// 托盘事件回调运行在同步上下文，可能不在 tokio 运行时内；直接 `tokio::spawn`
    /// 会触发运行时相关错误。使用独立 OS 线程执行阻塞 IO 是最稳妥的方式。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `game`：要切换到的目标游戏。
    ///
    /// # 返回值
    /// 显示窗口失败返回错误；落盘失败仅在线程内记录 error，不影响返回值。
    fn switch_game_and_show(app: &AppHandle, game: TargetGame) -> Result<()> {
        let state = app.state::<crate::state::AppState>();
        // 短时持有写锁以更新目标游戏，随后立即释放
        {
            let mut settings = state.settings.write();
            settings.target_game = game;
        }

        // 在独立线程中持久化设置：避免在托盘回调中执行阻塞 IO
        if let Ok(app_data_dir) = app.path().app_data_dir() {
            let settings_arc = state.settings.clone();
            let app_data_dir = app_data_dir.clone();
            std::thread::spawn(move || {
                // 仅短时持有读锁以克隆设置，随后立即释放，避免阻塞其他读者
                let settings_clone = {
                    let settings = settings_arc.read();
                    settings.clone()
                };
                if let Err(e) = settings_clone.save(&app_data_dir) {
                    log::error!("Failed to save settings after game switch: {}", e);
                }
            });
        }

        // 切换完成后显示主窗口
        WindowManager::show_window(app)?;
        Ok(())
    }
}

impl Default for TrayManager {
    /// 默认实现等价于 [`TrayManager::new`]。
    fn default() -> Self {
        Self::new()
    }
}
