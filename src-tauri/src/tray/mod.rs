//! 系统托盘模块
//!
//! 负责创建和管理系统托盘图标及菜单：
//! - 多语言菜单支持（中文/英文）
//! - 快捷游戏切换
//! - 窗口显示/隐藏
//! - 重置窗口位置
//! - 左键单击托盘图标切换窗口可见性
//! - 退出时自动注销全局快捷键

use crate::config::settings_store;
use crate::hotkey::HotkeyManager;
use crate::models::enums::TargetGame;
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

/// 托盘菜单 i18n 翻译表（中文）
const TRAY_ZH: &[(&str, &str)] = &[
    ("tooltip", "NRMM - 抽卡游戏模组管理器"),
    ("showGameWuwa", "显示(鸣潮)"),
    ("showGameGenshin", "显示(原神)"),
    ("showGameHSR", "显示(崩坏: 星穹铁道)"),
    ("showGameZZZ", "显示(绝区零)"),
    ("showGameHi3", "显示(崩坏三)"),
    ("showGameEndfield", "显示(终末地)"),
    ("resetWindowPosition", "重置窗口位置"),
    ("toggleVisibility", "显示/隐藏窗口"),
    ("quit", "退出"),
];

/// 托盘菜单 i18n 翻译表（英文）
const TRAY_EN: &[(&str, &str)] = &[
    ("tooltip", "NRMM - Mod Manager for Gacha Games"),
    ("showGameWuwa", "Show (WuWa)"),
    ("showGameGenshin", "Show (Genshin)"),
    ("showGameHSR", "Show (HSR)"),
    ("showGameZZZ", "Show (ZZZ)"),
    ("showGameHi3", "Show (Hi3)"),
    ("showGameEndfield", "Show (Endfield)"),
    ("resetWindowPosition", "Reset Window Position"),
    ("toggleVisibility", "Show/Hide Window"),
    ("quit", "Quit"),
];

/// 根据语言代码获取托盘菜单翻译字符串
///
/// 优先读取配置文件中的语言设置，若未设置或无法识别则使用系统语言，
/// 系统语言也无法识别时默认使用英文。
///
/// # 参数
///
/// * `lang` - 语言代码（如 "zh-CN"、"en"）
/// * `key` - 翻译键名
///
/// # 返回值
///
/// 返回对应语言的翻译字符串，找不到则返回英文翻译作为兜底
fn tray_lang_str(lang: &str, key: &str) -> String {
    let is_zh = lang.starts_with("zh") || lang.eq_ignore_ascii_case("zh-CN");
    let table = if is_zh { TRAY_ZH } else { TRAY_EN };
    table
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| {
            TRAY_EN
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
                .unwrap_or_else(|| key.to_string())
        })
}

/// 处理游戏切换操作
///
/// 切换目标游戏设置，发送事件通知前端刷新模组列表，
/// 同时显示主窗口方便用户操作。
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
/// * `game` - 目标游戏枚举值
fn handle_game_switch(app: &AppHandle, game: TargetGame) {
    if let Err(e) = settings_store::set_target_game(game) {
        log::warn!("Failed to switch target game: {}", e);
    }
    let _ = app.emit("target-game-switched", game);
    crate::window::show_main_window(app);
}

/// 处理应用退出操作
///
/// 先注销所有全局快捷键，释放系统热键资源，
/// 然后直接调用 exit 退出进程，绕过窗口关闭拦截。
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
fn handle_quit(app: &AppHandle) {
    if let Some(hotkey_mgr) = app.try_state::<Arc<HotkeyManager>>() {
        hotkey_mgr.unregister_all();
    }
    app.exit(0);
}

/// 创建系统托盘图标及菜单
///
/// 根据配置文件语言设置动态生成本地化菜单文本，
/// 菜单结构为：6个游戏切换项 → 分隔符 → 重置窗口位置 + 显示/隐藏窗口 → 分隔符 → 退出。
/// 显示/隐藏菜单项根据主窗口当前可见状态自动切换文本。
/// 左键单击托盘图标切换主窗口显示/隐藏，右键弹出菜单。
///
/// # 参数
///
/// * `app` - Tauri 应用句柄
///
/// # 返回值
///
/// 成功返回 `Ok(())`，失败返回 Tauri 错误
pub fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let lang = settings_store::get_language();

    let show_wuwa = MenuItem::with_id(
        app,
        "showGameWuwa",
        tray_lang_str(&lang, "showGameWuwa"),
        true,
        None::<&str>,
    )?;
    let show_genshin = MenuItem::with_id(
        app,
        "showGameGenshin",
        tray_lang_str(&lang, "showGameGenshin"),
        true,
        None::<&str>,
    )?;
    let show_hsr = MenuItem::with_id(
        app,
        "showGameHSR",
        tray_lang_str(&lang, "showGameHSR"),
        true,
        None::<&str>,
    )?;
    let show_zzz = MenuItem::with_id(
        app,
        "showGameZZZ",
        tray_lang_str(&lang, "showGameZZZ"),
        true,
        None::<&str>,
    )?;
    let show_hi3 = MenuItem::with_id(
        app,
        "showGameHi3",
        tray_lang_str(&lang, "showGameHi3"),
        true,
        None::<&str>,
    )?;
    let show_endfield = MenuItem::with_id(
        app,
        "showGameEndfield",
        tray_lang_str(&lang, "showGameEndfield"),
        true,
        None::<&str>,
    )?;
    let separator1 = PredefinedMenuItem::separator(app)?;
    let reset_position = MenuItem::with_id(
        app,
        "resetWindowPosition",
        tray_lang_str(&lang, "resetWindowPosition"),
        true,
        None::<&str>,
    )?;
    let toggle_visibility = MenuItem::with_id(
        app,
        "toggleVisibility",
        tray_lang_str(&lang, "toggleVisibility"),
        true,
        None::<&str>,
    )?;
    let separator2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        tray_lang_str(&lang, "quit"),
        true,
        None::<&str>,
    )?;

    let menu = Menu::with_items(
        app,
        &[
            &show_wuwa,
            &show_genshin,
            &show_hsr,
            &show_zzz,
            &show_hi3,
            &show_endfield,
            &separator1,
            &reset_position,
            &toggle_visibility,
            &separator2,
            &quit,
        ],
    )?;

    let _tray = TrayIconBuilder::new()
        .tooltip(tray_lang_str(&lang, "tooltip"))
        // SAFETY: The default window icon is configured in tauri.conf.json and is always present at runtime.
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            let id = event.id().0.as_str();
            match id {
                // 游戏切换涉及写设置文件（阻塞IO），在新线程中执行避免阻塞托盘回调
                "showGameWuwa" => {
                    let app_handle = app.clone();
                    crate::utils::spawn_safe(
                        "tray_game_switch_wuwa",
                        std::panic::AssertUnwindSafe(move || {
                            handle_game_switch(&app_handle, TargetGame::Wuwa)
                        }),
                    );
                }
                "showGameGenshin" => {
                    let app_handle = app.clone();
                    crate::utils::spawn_safe(
                        "tray_game_switch_genshin",
                        std::panic::AssertUnwindSafe(move || {
                            handle_game_switch(&app_handle, TargetGame::GenshinImpact)
                        }),
                    );
                }
                "showGameHSR" => {
                    let app_handle = app.clone();
                    crate::utils::spawn_safe(
                        "tray_game_switch_hsr",
                        std::panic::AssertUnwindSafe(move || {
                            handle_game_switch(&app_handle, TargetGame::HonkaiStarRail)
                        }),
                    );
                }
                "showGameZZZ" => {
                    let app_handle = app.clone();
                    crate::utils::spawn_safe(
                        "tray_game_switch_zzz",
                        std::panic::AssertUnwindSafe(move || {
                            handle_game_switch(&app_handle, TargetGame::ZZZ)
                        }),
                    );
                }
                "showGameHi3" => {
                    let app_handle = app.clone();
                    crate::utils::spawn_safe(
                        "tray_game_switch_hi3",
                        std::panic::AssertUnwindSafe(move || {
                            handle_game_switch(&app_handle, TargetGame::HonkaiImpact3rd)
                        }),
                    );
                }
                "showGameEndfield" => {
                    let app_handle = app.clone();
                    crate::utils::spawn_safe(
                        "tray_game_switch_endfield",
                        std::panic::AssertUnwindSafe(move || {
                            handle_game_switch(&app_handle, TargetGame::ArknightsEndfield)
                        }),
                    );
                }
                "resetWindowPosition" => {
                    let _ = crate::window::reset_window_position(app.clone(), "main".to_string());
                }
                "toggleVisibility" => {
                    crate::window::toggle_main_window(app);
                }
                "quit" => {
                    handle_quit(app);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                crate::window::toggle_main_window(app);
            }
        })
        .build(app)?;

    Ok(())
}
