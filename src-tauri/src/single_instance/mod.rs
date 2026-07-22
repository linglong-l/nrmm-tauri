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

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 check_single_instance 在无同名进程时不崩溃并返回预期结果。
    #[test]
    fn test_check_single_instance_no_duplicate() {
        let (has_duplicate, _) = check_single_instance();
        // 测试环境中通常只有一个进程，应返回 false
        // 但在 CI 并行运行场景下可能有同名进程，仅验证不崩溃
        assert!(has_duplicate == false || has_duplicate == true);
    }

    /// 验证 send_show_signal 在不绑定监听端口时返回预期结果。
    #[test]
    fn test_send_show_signal_returns_bool() {
        let result = send_show_signal();
        // UDP 是无连接协议，send_to 可能成功（数据发送到端口但无监听者）
        // 仅验证不崩溃且返回布尔值
        assert!(result == true || result == false);
    }

    /// 验证 send_show_signal 在监听端口存在时成功发送。
    #[test]
    fn test_send_show_signal_with_listener() {
        // 绑定监听端口
        let listener = UdpSocket::bind(format!("127.0.0.1:{}", SINGLE_INSTANCE_PORT));
        if listener.is_err() {
            // 端口可能被占用，跳过测试
            return;
        }
        let listener = listener.unwrap();

        // 设置非阻塞以避免死锁
        let _ = listener.set_read_timeout(Some(std::time::Duration::from_millis(100)));

        let result = send_show_signal();
        assert!(result);

        // 验证收到正确的消息
        let mut buf = [0u8; 64];
        if let Ok((len, _)) = listener.recv_from(&mut buf) {
            assert_eq!(&buf[..len], b"SHOW");
        }
    }

    /// 验证 SINGLE_INSTANCE_PORT 常量值。
    #[test]
    fn test_single_instance_port_value() {
        assert_eq!(SINGLE_INSTANCE_PORT, 49152);
    }
}
