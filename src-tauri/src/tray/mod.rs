use tauri::{
    AppHandle, Emitter,
    tray::{TrayIconBuilder, TrayIconEvent, MouseButton, MouseButtonState},
    menu::{Menu, MenuItem, PredefinedMenuItem},
};

pub fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::new(app, "Show NRMM", true, None::<&str>)?;
    let refresh = MenuItem::new(app, "Refresh Mods", true, None::<&str>)?;
    let quit = MenuItem::new(app, "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(app, &[&show, &refresh, &separator, &quit])?;

    let _tray = TrayIconBuilder::new()
        .tooltip("NRMM - No Reload Mod Manager")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            match event.id().0.as_str() {
                "Show NRMM" => {
                    crate::window::show_main_window(app);
                }
                "Refresh Mods" => {
                    let _ = app.emit("tray-refresh", ());
                    crate::window::show_main_window(app);
                }
                "Quit" => {
                    app.exit(0);
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
