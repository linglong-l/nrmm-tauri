//! 前端日志转发命令
//!
//! 提供 `frontend_log`：前端（WebView）日志经 IPC 转发到后端统一日志系统，
//! 与后端日志一同写入每日日志文件（`logs/YYYY-MM-DD.log`），便于 dev 排查
//! 前后端链路异常（前端无需终端、后端日志时序合一）。
//!
//! 设计约定：
//! - **fire-and-forget**：命令无返回值（`Result<(), String>` 恒成功），
//!   前端调用方不 await、不感知写入成功/失败——日志丢失不阻断业务
//! - 级别过滤由后端 env_logger 的 `LevelFilter` 统一执行（dev=Debug / prod=Info），
//!   前端只需按自身策略转发，无需重复过滤
//! - 命令体为纯 `log::xxx!` 调用（env_logger 同步写文件 + stderr 异步排水），
//!   单次调用微秒级，无需 async/spawn_blocking

use log::Level;

/// `module` 字段最大长度（超出截断并标记）
const MODULE_MAX_LEN: usize = 64;
/// `msg` 字段最大长度（超出截断并标记）
const MSG_MAX_LEN: usize = 2000;

/// 清洗来自 WebView 的日志字段：剥除换行/回车/制表/NUL 等控制字符，超长截断。
///
/// 保留可见字符与原意（不做转义还原），因为日志只做人读与检索。
fn sanitize_log_field(raw: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(raw.len().min(max_chars));
    for ch in raw.chars() {
        if ch.is_control() {
            out.push(' ');
            continue;
        }
        out.push(ch);
        if out.chars().count() >= max_chars {
            out.push_str("…[truncated]");
            break;
        }
    }
    out
}

/// 将前端日志消息按级别写入后端日志系统（fire-and-forget，恒成功）。
///
/// # 参数
/// - `level`: 日志级别（"DEBUG" | "INFO" | "WARN" | "ERROR"；未知值按 DEBUG 处理）
/// - `module`: 前端模块名（如 "ModsStore"、"App"）
/// - `msg`: 格式化后的日志消息（前端已安全提取 Error del message）
///
/// # 返回
/// `()`（前端不依赖返回值；写入异常由后端日志管道自行降级/红字提示）
#[tauri::command]
pub fn frontend_log(level: String, module: String, msg: String) {
    let target = level.parse::<Level>().unwrap_or(Level::Debug);
    // 统一前缀 [FE]，与后端日志在同一文件中对齐；秒级定位前后端时序
    //
    // module/msg 全部来自 WebView（不可信输入）：原样落盘会让一条含换行的消息
    // 伪造出多行日志（log forging），掩盖时序或植入假条目。故剥除控制字符并截断。
    let line = format!(
        "[FE][{}] {}",
        sanitize_log_field(&module, MODULE_MAX_LEN),
        sanitize_log_field(&msg, MSG_MAX_LEN)
    );
    match target {
        Level::Error => log::error!("{}", line),
        Level::Warn => log::warn!("{}", line),
        Level::Info => log::info!("{}", line),
        _ => log::debug!("{}", line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// level 字符串解析（未知值降级 DEBUG）——供日志级别映射的纯逻辑回归。
    #[test]
    fn level_parse_mapping() {
        assert_eq!("ERROR".parse::<Level>().unwrap(), Level::Error);
        assert_eq!("WARN".parse::<Level>().unwrap(), Level::Warn);
        assert_eq!("INFO".parse::<Level>().unwrap(), Level::Info);
        assert_eq!("DEBUG".parse::<Level>().unwrap(), Level::Debug);
        // 未知级别回退 DEBUG（前端误传不吞错，但以最低严重度记录）
        assert_eq!(
            Level::Debug,
            "verbose".parse::<Level>().unwrap_or(Level::Debug)
        );
        assert_eq!(Level::Debug, "".parse::<Level>().unwrap_or(Level::Debug));
    }
}
