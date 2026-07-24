//! 系统托盘管理模块。
//!
//! 提供：
//! - 托盘菜单（每个游戏一个“显示”项 + 重置位置/隐藏/退出）；
//! - 托盘图标与提示文本；
//! - 菜单点击事件分发；
//! - 托盘图标交互（左键单击切换窗口显示）；
//! - 国际化支持：菜单文本根据当前语言设置自动切换。
//!
//! ## 注意事项
//! 涉及设置落盘的操作（如 [`TrayManager::switch_game_and_show`]）使用
//! `tauri::async_runtime::spawn` 复用主运行时，避免在回调中创建独立 tokio 运行时。

use std::path::Path;

use anyhow::{Context, Result};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

use crate::process::TargetGame;
use crate::window_manager::WindowManager;

/// 托盘图标唯一标识，用于后续通过 `app.tray_by_id` 检索。
const TRAY_ID: &str = "main-tray";

/// 托盘默认提示文本（鼠标悬停时显示）。
const TRAY_TOOLTIP: &str = "No Reload Mod Manager - Tauri";

/// 托盘管理器（无状态）。
///
/// 所有方法均通过 `&AppHandle` 操作托盘资源，本结构本身不持有数据。
pub struct TrayManager;

impl TrayManager {
    /// 构造空实例。
    pub fn new() -> Self {
        Self
    }

    /// 根据语言代码获取本地化字符串。
    ///
    /// # 语言优先级
    /// 1. 配置文件中指定的语言；
    /// 2. 系统语言；
    /// 3. 默认英文（fallback）。
    ///
    /// # 参数
    /// - `locale`：语言代码（如 "zh-CN"、"en"、"zh-TW"、"id"、"ru"）；
    /// - `key`：字符串键（如 "show-wuwa"、"reset-position"）。
    ///
    /// # 返回值
    /// 本地化后的字符串，未找到匹配时返回英文默认值。
    fn get_localized_string(locale: &str, key: &str) -> String {
        match (locale, key) {
            // 简体中文
            ("zh-CN", "show-wuwa") => "显示 (鸣潮)".to_string(),
            ("zh-CN", "show-genshin") => "显示 (原神)".to_string(),
            ("zh-CN", "show-hsr") => "显示 (崩坏：星穹铁道)".to_string(),
            ("zh-CN", "show-zzz") => "显示 (绝区零)".to_string(),
            ("zh-CN", "show-endfield") => "显示 (明日方舟：终末地)".to_string(),
            ("zh-CN", "reset-position") => "重置位置".to_string(),
            ("zh-CN", "hide") => "隐藏窗口".to_string(),
            ("zh-CN", "exit") => "退出".to_string(),
            // 繁体中文
            ("zh-TW", "show-wuwa") => "顯示 (鳴潮)".to_string(),
            ("zh-TW", "show-genshin") => "顯示 (原神)".to_string(),
            ("zh-TW", "show-hsr") => "顯示 (崩壞：星穹鐵道)".to_string(),
            ("zh-TW", "show-zzz") => "顯示 (絕區零)".to_string(),
            ("zh-TW", "show-endfield") => "顯示 (明日方舟：終末地)".to_string(),
            ("zh-TW", "reset-position") => "重置位置".to_string(),
            ("zh-TW", "hide") => "隱藏視窗".to_string(),
            ("zh-TW", "exit") => "退出".to_string(),
            // 印尼语
            ("id", "show-wuwa") => "Tampilkan (WuWa)".to_string(),
            ("id", "show-genshin") => "Tampilkan (Genshin)".to_string(),
            ("id", "show-hsr") => "Tampilkan (HSR)".to_string(),
            ("id", "show-zzz") => "Tampilkan (ZZZ)".to_string(),
            ("id", "show-endfield") => "Tampilkan (Endfield)".to_string(),
            ("id", "reset-position") => "Reset Posisi".to_string(),
            ("id", "hide") => "Sembunyikan".to_string(),
            ("id", "exit") => "Keluar".to_string(),
            // 俄语
            ("ru", "show-wuwa") => "Показать (WuWa)".to_string(),
            ("ru", "show-genshin") => "Показать (Genshin)".to_string(),
            ("ru", "show-hsr") => "Показать (HSR)".to_string(),
            ("ru", "show-zzz") => "Показать (ZZZ)".to_string(),
            ("ru", "show-endfield") => "Показать (Endfield)".to_string(),
            ("ru", "reset-position") => "Сбросить позицию".to_string(),
            ("ru", "hide") => "Скрыть".to_string(),
            ("ru", "exit") => "Выход".to_string(),
            // 默认英文
            _ => match key {
                "show-wuwa" => "Show (WuWa)".to_string(),
                "show-genshin" => "Show (Genshin)".to_string(),
                "show-hsr" => "Show (HSR)".to_string(),
                "show-zzz" => "Show (ZZZ)".to_string(),
                "show-endfield" => "Show (Endfield)".to_string(),
                "reset-position" => "Reset Position".to_string(),
                "hide" => "Hide".to_string(),
                "exit" => "Exit".to_string(),
                _ => key.to_string(),
            },
        }
    }

    /// 创建并注册系统托盘（菜单 + 图标 + 提示文本）。
    ///
    /// # 菜单结构
    /// ```text
    /// Show (WuWa) / 显示 (鸣潮)
    /// Show (Genshin) / 显示 (原神)
    /// Show (HSR) / 显示 (崩坏：星穹铁道)
    /// Show (ZZZ) / 显示 (绝区零)
    /// Show (Endfield) / 显示 (明日方舟：终末地)
    /// ─────────────
    /// Reset Position / 重置位置
    /// Hide / 隐藏窗口
    /// ─────────────
    /// Exit / 退出
    /// ```
    ///
    /// # 国际化支持
    /// 菜单文本根据 `locale` 参数自动切换语言。支持的语言：
    /// - `zh-CN`：简体中文
    /// - `zh-TW`：繁体中文
    /// - `id`：印尼语
    /// - `ru`：俄语
    /// - 其他：回退到英文
    ///
    /// # 业务逻辑
    /// 1. 根据语言设置获取本地化菜单文本；
    /// 2. 为每个菜单项创建带 id 的 [`MenuItem`]；
    /// 3. 用分隔符组织菜单结构；
    /// 4. 使用应用默认窗口图标作为托盘图标；
    /// 5. `show_menu_on_left_click(false)`：左键单击不弹出菜单（改为触发图标事件）。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `locale`：当前语言代码（如 "zh-CN"）。
    ///
    /// # 返回值
    /// 任一步骤失败返回封装后的错误。
    pub fn setup_tray(app: &AppHandle, locale: &str) -> Result<()> {
        // 根据语言设置获取本地化菜单文本
        let show_wuwa_text = Self::get_localized_string(locale, "show-wuwa");
        let show_genshin_text = Self::get_localized_string(locale, "show-genshin");
        let show_hsr_text = Self::get_localized_string(locale, "show-hsr");
        let show_zzz_text = Self::get_localized_string(locale, "show-zzz");
        let show_endfield_text = Self::get_localized_string(locale, "show-endfield");
        let reset_position_text = Self::get_localized_string(locale, "reset-position");
        let hide_text = Self::get_localized_string(locale, "hide");
        let exit_text = Self::get_localized_string(locale, "exit");

        // 各游戏的“显示”菜单项
        let show_wuwa = MenuItem::with_id(app, "show-wuwa", show_wuwa_text, true, None::<&str>)?;
        let show_genshin =
            MenuItem::with_id(app, "show-genshin", show_genshin_text, true, None::<&str>)?;
        let show_hsr = MenuItem::with_id(app, "show-hsr", show_hsr_text, true, None::<&str>)?;
        let show_zzz = MenuItem::with_id(app, "show-zzz", show_zzz_text, true, None::<&str>)?;
        let show_endfield =
            MenuItem::with_id(app, "show-endfield", show_endfield_text, true, None::<&str>)?;
        // 窗口操作菜单项
        let reset_position = MenuItem::with_id(
            app,
            "reset-position",
            reset_position_text,
            true,
            None::<&str>,
        )?;
        let hide = MenuItem::with_id(app, "hide", hide_text, true, None::<&str>)?;
        // 退出菜单项
        let exit = MenuItem::with_id(app, "exit", exit_text, true, None::<&str>)?;

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

    /// 切换当前目标游戏并显示主窗口（完整交互流程）。
    ///
    /// # 业务逻辑（操作事务管理）
    /// 1. **游戏类型切换**：获取 `AppState`，写锁更新 `settings.target_game` 为指定游戏；
    /// 2. **设置持久化**：在独立 OS 线程中将新设置落盘（避免阻塞托盘事件回调）；
    /// 3. **模组列表重新加载**：在独立线程中触发模组数据重新扫描（通过 tokio 运行时）；
    /// 4. **窗口显示**：调用 [`WindowManager::show_window`] 显示主窗口。
    ///
    /// # 异常处理
    /// - 设置落盘失败：记录 error 日志，不影响窗口显示；
    /// - 模组加载失败：记录 error 日志，不影响窗口显示；
    /// - 窗口显示失败：返回错误（这是用户可见的核心操作）。
    ///
    /// # 为什么用 `tauri::async_runtime::spawn`
    /// 复用 Tauri 主运行时，避免在回调中创建独立 tokio 运行时（反模式）。
    ///
    /// # 参数
    /// - `app`：Tauri 应用句柄；
    /// - `game`：要切换到的目标游戏。
    ///
    /// # 返回值
    /// 显示窗口失败返回错误；其他步骤失败仅记录日志。
    fn switch_game_and_show(app: &AppHandle, game: TargetGame) -> Result<()> {
        let state = app.state::<crate::state::AppState>();

        // 步骤 1：游戏类型切换 - 更新内存中的目标游戏设置
        {
            let mut settings = state.settings.write();
            settings.target_game = game;
        }

        // 步骤 2+3：在异步运行时中执行设置持久化和模组重新加载
        // 使用 tauri::async_runtime::spawn 复用主 runtime，避免创建独立 tokio 运行时
        if let Some(app_data_dir) = crate::get_app_data_dir() {
            let settings_arc = state.settings.clone();
            let mod_manager = state.mod_manager.clone();
            let app_data_dir = app_data_dir.clone();
            let app = app.clone();
            let target_game = game;

            tauri::async_runtime::spawn(async move {
                // 子步骤 2a：短时持有读锁克隆设置
                let settings_clone = {
                    let settings = settings_arc.read();
                    settings.clone()
                };

                // 子步骤 2b：持久化设置到磁盘
                if let Err(e) = settings_clone.save(&app_data_dir) {
                    log::error!("Failed to save settings after game switch: {}", e);
                }

                // 子步骤 3：重新加载模组数据
                match mod_manager.load_mods(&settings_clone).await {
                    Ok(_) => log::info!("Mods reloaded successfully for game: {:?}", target_game),
                    Err(e) => log::error!("Failed to reload mods after game switch: {}", e),
                }

                // 通知前端游戏已切换
                let _ = app.emit("game-switched", serde_json::json!({ "game": target_game }));
            });
        }

        // 步骤 4：显示主窗口（模组管理页面自动打开）
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
