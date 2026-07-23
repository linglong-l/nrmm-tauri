// 全局 panic hook 模块。
//
// 作用：
//  - debug 构建：记录 panic 到日志文件，调用默认 hook 打印到 stderr，然后正常 abort。
//  - release 构建：记录 panic 到日志文件，弹出原生错误对话框，用户确认后重启应用。
//
// 注意事项：
//  - 使用 AtomicBool 防止递归 panic（如日志系统本身出问题）。
//  - release 下不输出任何控制台内容。

// dev 构建下 AtomicBool 仅在 #[cfg(not(debug_assertions))] 中使用，clippy 会误报未使用
#[cfg_attr(debug_assertions, allow(unused_imports))]
use std::sync::atomic::{AtomicBool, Ordering};

/// 安装全局 panic hook。
///
/// 应在 `lib.rs` 的 `setup` 阶段尽早调用，确保所有 panic 都能被捕获。
pub fn install_panic_hook() {
    #[cfg(debug_assertions)]
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        // 1. 记录 panic 到日志文件
        log::error!("========== UNRECOVERABLE PANIC ==========");
        log::error!("Message: {}", info);
        if let Some(location) = info.location() {
            log::error!("Location: {}:{}:{}", location.file(), location.line(), location.column());
        }
        if let Some(s) = info.payload().downcast_ref::<&str>() {
            log::error!("Payload: {}", s);
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            log::error!("Payload: {}", s);
        }
        log::error!("=========================================");

        // 2. debug 构建：调用默认 hook 打印到 stderr，正常 abort
        #[cfg(debug_assertions)]
        {
            default_hook(info);
        }

        // 3. release 构建：原生对话框 → 重启
        #[cfg(not(debug_assertions))]
        {
            show_error_dialog_and_restart();
        }
    }));
}

/// release 模式下显示错误对话框并重启应用。
#[cfg(not(debug_assertions))]
fn show_error_dialog_and_restart() {
    // 防止递归 panic（如果日志系统本身出问题）
    static ALREADY_PANICKING: AtomicBool = AtomicBool::new(false);
    if ALREADY_PANICKING.swap(true, Ordering::SeqCst) {
        return; // 递归 panic，直接返回让进程 abort
    }

    show_native_error_dialog();

    // 重启应用
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }

    std::process::exit(1);
}

/// 使用平台原生 API 显示错误对话框。
#[cfg(not(debug_assertions))]
#[cfg(target_os = "windows")]
fn show_native_error_dialog() {
    // 使用 windows crate 的 MessageBoxW 显示原生中文对话框
    // windows = { version = "0.62", features = ["Win32_UI_WindowsAndMessaging"] }
    // 已在 Cargo.toml 中配置
    unsafe {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONERROR};

        let message = "软件发生未知异常，将在您点击确认后重启应用，避免损坏您的模组。\n\n错误详情已记录至日志文件，可联系开发者获取支持。";
        let title = "nrmm-tauri - 未知异常";

        let wide_msg: Vec<u16> = OsStr::new(message)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let wide_title: Vec<u16> = OsStr::new(title)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        MessageBoxW(
            None,
            windows::core::PCWSTR(wide_msg.as_ptr()),
            windows::core::PCWSTR(wide_title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(debug_assertions))]
#[cfg(target_os = "linux")]
fn show_native_error_dialog() {
    let message = "软件发生未知异常，将在您点击确认后重启应用，避免损坏您的模组。\n\n错误详情已记录至日志文件。";
    let result = std::process::Command::new("zenity")
        .args(&[
            "--error",
            "--title=nrmm-tauri - 未知异常",
            "--text",
            message,
            "--ok-label=确认",
        ])
        .status();
    if result.is_err() {
        let _ = std::process::Command::new("xmessage")
            .args(&["-title", "nrmm-tauri - 未知异常", message])
            .status();
    }
}