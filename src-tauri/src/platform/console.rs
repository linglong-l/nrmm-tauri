//! 控制台管理模块（GUI 子系统下的控制台归属决策）
//!
//! release 构建使用 Windows GUI 子系统（见 main.rs 顶部的 windows_subsystem 属性），
//! 进程默认没有控制台窗口。本模块在日志系统初始化之前完成控制台决策：
//! - 默认（showConsoleOnStart = false）：从终端启动时附加父进程控制台（AttachConsole），
//!   双击启动（无父控制台）时静默忽略，应用无控制台运行
//! - 可选（showConsoleOnStart = true）：调用 AllocConsole 自建控制台窗口查看日志
//!
//! 决策成功后打开 CONOUT$ 存入全局 sink，供 write_console_err 在 GUI 子系统下
//! 将日志写入可见控制台；所有失败路径一律静默降级，绝不 panic。

use std::fs::File;
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

/// 全局控制台输出 sink（CONOUT$ 文件句柄）
///
/// init_console() 成功附加/自建控制台后写入一次；
/// 未设置时 write_console_err 降级写 stderr。Mutex 保证多线程写入串行化。
static CONOUT: OnceLock<Mutex<File>> = OnceLock::new();

/// 初始化控制台（必须在 env_logger 初始化之前调用）
///
/// 功能：
/// 1. 调用 read_show_console_flag() 轻量预读 settings.json（不依赖设置存储初始化）
/// 2. 标志为 true 时调用 AllocConsole 自建控制台；失败（如已拥有控制台）则忽略
/// 3. 标志为 false 时调用 AttachConsole(ATTACH_PARENT_PROCESS) 附加父控制台；
///    失败（双击启动无父控制台）则静默忽略
/// 4. 步骤 2 或 3 成功后，以只写模式打开 CONOUT$ 存入全局 sink
///
/// # 参数
/// 无
///
/// # 返回
/// 无；任何失败均不 panic、不返回错误（仅 debug 构建下通过 eprintln 输出诊断）
///
/// # 注意
/// - 非 Windows 平台为空实现
/// - 必须在 env_logger 初始化之前调用，否则标准流对日志系统不可见
pub fn init_console() {
    #[cfg(target_os = "windows")]
    {
        use std::fs::OpenOptions;
        use windows::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};

        // 控制台归属由用户开关（settings.showConsoleOnStart）决定，dev 与 release 同口径：
        // 启用 → AllocConsole 自建控制台并把日志输出到它；未启用 → 仅尝试附加父控制台
        // （从终端启动时日志仍可见，双击启动则静默无控制台）。
        let show_console: bool = read_show_console_flag();

        // 控制台归属决策：自建或附加，两类失败均属预期场景，静默忽略
        let console_ready = if show_console {
            match unsafe { AllocConsole() } {
                Ok(()) => true,
                Err(_e) => {
                    // 常见失败：进程已拥有控制台
                    #[cfg(debug_assertions)]
                    eprintln!("[console] AllocConsole 失败: {:?}", _e);
                    false
                }
            }
        } else {
            match unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } {
                Ok(()) => true,
                Err(_e) => {
                    // 常见失败：双击启动，无父控制台可附加
                    #[cfg(debug_assertions)]
                    eprintln!("[console] AttachConsole 失败: {:?}", _e);
                    false
                }
            }
        };

        // 打开 CONOUT$ 作为全局输出 sink（仅写模式）
        if console_ready {
            match OpenOptions::new().read(false).write(true).open("CONOUT$") {
                Ok(file) => {
                    let _ = CONOUT.set(Mutex::new(file));
                }
                Err(_e) => {
                    #[cfg(debug_assertions)]
                    eprintln!("[console] 打开 CONOUT$ 失败: {}", _e);
                }
            }
        }
    }

    // 非 Windows 平台：无控制台管理需求，空实现
}

/// 轻量预读 settings.json 中的 showConsoleOnStart 标志
///
/// 功能：直接读取磁盘上的 settings.json 文本（路径取 config::app_paths::settings_path()），
/// 不依赖设置存储初始化——本函数在 init_settings() 之前调用。
///
/// # 参数
/// 无
///
/// # 返回
/// true 表示启动时需自建控制台；文件不存在/读取失败一律返回 false
///
/// # 注意
/// 任何 IO 失败均保守返回 false，绝不 panic。
///
/// dev 与 release 同口径读取该标志（此前 debug 构建恒强制启用，已按用户需求移除）。
#[cfg(target_os = "windows")]
fn read_show_console_flag() -> bool {
    match std::fs::read_to_string(crate::config::app_paths::settings_path()) {
        Ok(content) => parse_show_console_flag(&content),
        Err(_) => false,
    }
}

/// 解析 settings.json 文本中的 showConsoleOnStart 布尔标志（纯函数）
///
/// 功能：用 serde_json 将文本解析为通用 JSON 值后读取 showConsoleOnStart 字段，
/// 从 read_show_console_flag 中抽出以便表驱动单元测试。
///
/// # 参数
/// - `content`: settings.json 文件文本内容
///
/// # 返回
/// 字段存在且为布尔 true 时返回 true；
/// 字段为 false / 缺失 / 值非布尔 / JSON 非法 / 空串一律返回 false
#[cfg(any(target_os = "windows", test))]
fn parse_show_console_flag(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|v| v.get("showConsoleOnStart").and_then(|f| f.as_bool()))
        .unwrap_or(false)
}

/// 写入控制台错误流（供日志排水线程调用）
///
/// 功能：若全局 CONOUT$ sink 存在则写入之；否则降级写标准错误流。
/// GUI 子系统下未成功附加/自建控制台时 stderr 不可见，属预期行为。
///
/// # 参数
/// - `buf`: 待写入的字节缓冲区
///
/// # 返回
/// 无；写入失败静默忽略，绝不 panic
pub fn write_console_err(buf: &[u8]) {
    match CONOUT.get() {
        Some(sink) => {
            if let Ok(mut file) = sink.lock() {
                let _ = file.write_all(buf);
            }
        }
        None => {
            let _ = io::stderr().write_all(buf);
        }
    }
}

/// ANSI 红色转义包裹（纯函数，供测试与 `write_console_red` 复用）。
///
/// 输出 `\x1b[31m{buf}\x1b[0m`；Windows 10 1909+ 终端（含 VS Code 集成终端 /
/// Windows Terminal）默认支持 VT 序列。作为「日志写入异常」等醒目告警的输出。
pub fn render_red(buf: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len() + 11);
    out.extend_from_slice(b"\x1b[31m");
    out.extend_from_slice(buf);
    out.extend_from_slice(b"\x1b[0m");
    out
}

/// 以红色写入控制台（醒目告警：日志文件写入异常等）。
///
/// 走与 `write_console_err` 相同的 CONOUT/stderr 降级链；失败静默。不 panic。
pub fn write_console_red(buf: &[u8]) {
    write_console_err(&render_red(buf));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// render_red：ANSI 红色包裹（\x1b[31m...\x1b[0m），用于日志写入异常醒目告警。
    #[test]
    fn test_render_red_wraps_ansi() {
        let out = render_red(b"write failed");
        assert_eq!(out, b"\x1b[31mwrite failed\x1b[0m");
    }

    /// 表驱动测试：parse_show_console_flag 对各种输入的解析行为
    ///
    /// 覆盖：字段存在 true/false、字段缺失（空对象/其他字段）、字段非布尔、非法 JSON、空串
    #[test]
    fn test_parse_show_console_flag() {
        let cases: Vec<(&str, &str, bool)> = vec![
            ("字段存在 true", r#"{"showConsoleOnStart": true}"#, true),
            ("字段存在 false", r#"{"showConsoleOnStart": false}"#, false),
            ("字段缺失（空对象）", "{}", false),
            ("字段缺失（其他字段）", r#"{"language": "en"}"#, false),
            (
                "字段存在但非布尔",
                r#"{"showConsoleOnStart": "yes"}"#,
                false,
            ),
            ("非法 JSON", "not a json", false),
            ("空串", "", false),
        ];

        for (name, input, expected) in cases {
            assert_eq!(
                parse_show_console_flag(input),
                expected,
                "测试用例 [{}] 未达预期",
                name
            );
        }
    }
}
