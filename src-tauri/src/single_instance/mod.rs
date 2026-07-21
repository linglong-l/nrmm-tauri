use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use sysinfo::System;

const SINGLE_INSTANCE_PORT: u16 = 49152;

pub fn check_single_instance() -> (bool, Option<String>) {
    let current_exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            log::warn!("Failed to get current executable path: {}", e);
            return (false, None);
        }
    };

    let current_name = current_exe.file_name().unwrap().to_string_lossy().to_string();
    let system = System::new_all();

    for (_, process) in system.processes() {
        if process.pid().as_u32() == std::process::id() {
            continue;
        }

        let process_name = process.name().to_string_lossy().to_string();
        if process_name != current_name {
            continue;
        }

        let process_exe = match process.exe() {
            Some(path) => path,
            None => {
                log::warn!("Found process with same name but cannot get exe path, exiting");
                return (true, None);
            }
        };

        let current_canonical = Path::new(&current_exe).canonicalize().unwrap_or(current_exe.clone());
        let process_canonical = Path::new(process_exe).canonicalize().unwrap_or(process_exe.to_path_buf());

        if current_canonical == process_canonical {
            log::info!("Found running instance with same path: {}", current_canonical.display());
            return (true, Some(current_canonical.to_string_lossy().to_string()));
        } else {
            log::info!(
                "Found process with same name but different path: current={}, other={}",
                current_canonical.display(),
                process_canonical.display()
            );
            return (true, None);
        }
    }

    (false, None)
}

pub fn send_show_signal() -> bool {
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Failed to bind UDP socket: {}", e);
            return false;
        }
    };

    let addr: SocketAddr = format!("127.0.0.1:{}", SINGLE_INSTANCE_PORT).parse().unwrap();

    for i in 0..3 {
        match socket.send_to(b"SHOW", addr) {
            Ok(_) => {
                log::info!("Sent show signal to existing instance");
                return true;
            }
            Err(e) => {
                log::warn!("Failed to send show signal (attempt {}): {}", i + 1, e);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }

    false
}

pub fn start_listener(app_handle: tauri::AppHandle) {
    let socket = match UdpSocket::bind(format!("127.0.0.1:{}", SINGLE_INSTANCE_PORT)) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Failed to bind listener socket: {}", e);
            return;
        }
    };

    let mut buf = [0u8; 64];
    std::thread::spawn(move || {
        log::info!("Single instance listener started on port {}", SINGLE_INSTANCE_PORT);
        loop {
            match socket.recv_from(&mut buf) {
                Ok((len, _)) => {
                    let msg = String::from_utf8_lossy(&buf[..len]);
                    if msg == "SHOW" {
                        log::info!("Received show signal, activating window");
                        let _ = crate::window_manager::WindowManager::show_window(&app_handle);
                    }
                }
                Err(e) => {
                    log::warn!("UDP listener error: {}", e);
                    break;
                }
            }
        }
    });
}
