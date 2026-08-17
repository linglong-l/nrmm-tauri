//! INI 相关错误的「规范化」层：把底层技术错误（IO / 编码 / 解析）与结构化校验错误
//! （`ErroredLines`）统一转化为**非专业人员可直观理解**的中文友好提示，
//! 避免在用户界面暴露文件路径、堆栈、操作系统错误码等技术细节。
//!
//! 设计原则：
//! - 只暴露「发生了什么、用户能做什么」，不暴露「底层为什么失败」的技术细节；
//! - 错误码（`code`）供前端做条件化展示/埋点，但不展示给用户；
//! - 友好文本中不出现绝对路径、`os error N`、`panic`、`unwrap` 等技术字样。

use std::io;

use anyhow::Error;
use serde::Serialize;

/// 规范化后的用户友好错误。
///
/// 通过 Tauri 命令序列化后直接交付前端；`code` 仅供前端做条件展示/埋点。
#[derive(Debug, Clone, Serialize)]
pub struct FriendlyError {
    /// 机器可读错误码（不展示给用户）。
    pub code: &'static str,
    /// 简短标题（一句话概括问题类型）。
    pub title: String,
    /// 面向用户的说明（告知现象与可采取的行动）。
    pub message: String,
}

impl FriendlyError {
    /// 便捷取用纯文本消息（命令边界常只需 message）。
    pub fn into_message(self) -> String {
        self.message
    }
}

/// 命令边界便捷函数：把 `anyhow::Error` 转为前端友好的 `String`（仅 message 部分）。
pub fn err_to_ui(e: Error) -> String {
    normalize(&e).message
}

/// 后台任务（`spawn_blocking`）因 panic / 取消而失败时的统一友好提示（不暴露 panic 内容）。
///
/// 这类错误属于「内部任务意外中断」，并非用户可操作的文件/编码问题；真实原因应由调用方
/// 记入日志，此处仅返回中性文案，避免把 panic 栈/内部错误码直接交付前端。
pub fn join_error_to_ui() -> String {
    "后台处理任务意外中断，请稍后重试；若问题持续，请记录操作时间并联系支持。".to_string()
}

/// 把任意 `anyhow::Error` 规范化为 [`FriendlyError`]。
///
/// 判定顺序：IO 错误（按 `ErrorKind` 细分） → 编码错误 → 其它统一为「未知错误」。
/// 绝不把原始 `e.to_string()`（含路径/错误码/堆栈）直接透传给用户。
pub fn normalize(e: &Error) -> FriendlyError {
    // 1) IO 错误：按类型给出针对性提示
    if let Some(io_err) = e.downcast_ref::<io::Error>() {
        return from_io(io_err);
    }
    // 2) 编码错误（理论上 INI 走有损解码不会触发，但其它路径可能）
    if e.downcast_ref::<std::str::Utf8Error>().is_some()
        || e.downcast_ref::<std::string::FromUtf8Error>().is_some()
    {
        return FriendlyError {
            code: "encoding_error",
            title: "文件编码异常".to_string(),
            message: "文件不是有效的文本（编码异常），请将其另存为 UTF-8 编码后重试。".to_string(),
        };
    }
    // 3) 其它：统一为「未知错误」，不暴露技术细节
    FriendlyError {
        code: "internal_error",
        title: "操作未完成".to_string(),
        message: "处理过程中发生未知错误，请重试；若问题持续，请记录操作时间并联系支持。"
            .to_string(),
    }
}

/// 把标准库 IO 错误按 `ErrorKind` 映射为友好提示。
fn from_io(err: &io::Error) -> FriendlyError {
    let (code, title, message) = match err.kind() {
        io::ErrorKind::NotFound => (
            "file_not_found",
            "找不到文件",
            "找不到指定的文件或文件夹，请确认路径是否正确，或文件是否已被移动、重命名或删除。",
        ),
        io::ErrorKind::PermissionDenied => (
            "permission_denied",
            "没有访问权限",
            "没有权限读取该文件，请检查文件是否被其他程序占用，或以管理员身份运行后重试。",
        ),
        io::ErrorKind::AlreadyExists => (
            "already_exists",
            "文件已存在",
            "目标文件已存在，请确认是否重复执行了相同操作。",
        ),
        io::ErrorKind::InvalidInput => (
            "invalid_path",
            "路径无效",
            "文件路径包含无效字符，请检查路径格式后重试。",
        ),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => (
            "io_timeout",
            "读取超时",
            "读取文件超时，请检查磁盘或网络存储是否连接正常后重试。",
        ),
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => (
            "disk_full",
            "磁盘空间不足",
            "磁盘空间不足，无法完成操作，请清理空间后重试。",
        ),
        io::ErrorKind::Interrupted => ("interrupted", "操作被中断", "操作被系统中断，请重试。"),
        io::ErrorKind::IsADirectory => (
            "is_a_directory",
            "路径类型错误",
            "预期为文件，但实际是一个文件夹，请检查配置路径。",
        ),
        io::ErrorKind::NotADirectory => (
            "not_a_directory",
            "路径类型错误",
            "预期为文件夹，但实际是一个文件，请检查配置路径。",
        ),
        _ => (
            "io_error",
            "文件读取失败",
            "读取或处理文件时发生未知错误，请重试；若问题持续，请记录操作时间并联系支持。",
        ),
    };
    FriendlyError {
        code,
        title: title.to_string(),
        message: message.to_string(),
    }
}

/// 把结构化校验错误（`ErroredLines`）转换为面向非专业人员的友好文本。
///
/// - `error_type`：`detect_errors` 定义的错误类型常量（0/1/2/3/5/6）；
/// - `error_message`：原始技术信息（如 `DUPLICATE LIB: X`），仅用于从中提取
///   与用户相关的名称，**不**直接展示；
/// - `line_number`：出错行号（0 表示文件级错误，如路径过长）。
///
/// 返回文本已去除 `DUPLICATE LIB` / `CRASH LINE` / `NON EXISTENT LIB` 等技术字样，
/// 仅保留用户需要理解的「是什么、在哪、怎么办」。
pub fn friendly_errored_line(error_type: u8, error_message: &str, line_number: u32) -> String {
    let prefix = if line_number > 0 {
        format!("第 {} 行：", line_number)
    } else {
        String::new()
    };

    match error_type {
        // 0: DUPLICATE LIB
        0 => {
            let lib = error_message
                .strip_prefix("DUPLICATE LIB:")
                .or_else(|| error_message.strip_prefix("DUPLICATE LIB"))
                .unwrap_or(error_message)
                .trim();
            format!(
                "{}模组「{}」的配置段被重复定义，请合并或删除多余的段，避免冲突。",
                prefix, lib
            )
        }
        // 1: CRASH LINE
        1 => format!(
            "{}存在可能导致程序崩溃的绘制指令取值，建议检查该值或暂时注释此行。",
            prefix
        ),
        // 2: MISSING ENDIF
        2 => format!(
            "{}if 条件块缺少对应的 endif 结束标记，请在该块的末尾补充 endif。",
            prefix
        ),
        // 3: FLOW CONTROL
        3 => format!(
            "{}流程控制结构有误（如多余的 endif），请检查 if / else / endif 是否配对正确。",
            prefix
        ),
        // 5: NON EXISTENT LIB
        5 => {
            let lib = error_message
                .strip_prefix("NON EXISTENT LIB:")
                .or_else(|| error_message.strip_prefix("NON EXISTENT LIB"))
                .unwrap_or(error_message)
                .trim();
            format!(
                "{}引用了不存在的库「{}」，请确认对应的依赖模组已启用，或检查名称拼写。",
                prefix, lib
            )
        }
        // 6: PATH TOO LONG
        6 => "配置路径过长（超过 260 字符），Windows 可能无法正常访问，请将模组移动到路径更短的位置。".to_string(),
        // 未知类型：兜底
        _ => format!("{}该配置项存在异常，请检查后重试。", prefix),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    /// 红线：友好提示中绝不可出现这些技术字样。
    const FORBIDDEN: &[&str] = &[
        "os error",
        "panic",
        "unwrap",
        "src-tauri",
        "DUPLICATE LIB",
        "CRASH LINE",
        "NON EXISTENT LIB",
        "MISSING ENDIF",
    ];

    fn assert_clean(msg: &str) {
        for f in FORBIDDEN {
            assert!(!msg.contains(f), "友好提示泄露技术字样 `{f}`：{msg}");
        }
    }

    #[test]
    fn io_not_found_is_friendly_and_clean() {
        let e = io::Error::new(io::ErrorKind::NotFound, "no such file");
        let msg = err_to_ui(anyhow::Error::new(e));
        assert_clean(&msg);
        assert!(msg.contains("找不到"));
    }

    #[test]
    fn io_permission_denied_is_friendly_and_clean() {
        let e = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let msg = err_to_ui(anyhow::Error::new(e));
        assert_clean(&msg);
        assert!(msg.contains("权限"));
    }

    #[test]
    fn io_other_is_generic_and_clean() {
        let e = io::Error::other("weird");
        let msg = err_to_ui(anyhow::Error::new(e));
        assert_clean(&msg);
    }

    #[test]
    fn internal_error_is_generic_and_clean() {
        // 非 IO 的 anyhow 错误统一为未知错误提示，不含底层信息
        let msg = err_to_ui(anyhow::anyhow!("some internal detail xyz"));
        assert_clean(&msg);
        assert!(!msg.contains("xyz"));
    }

    #[test]
    fn friendly_errored_line_strips_technical_prefixes() {
        let dup = friendly_errored_line(0, "DUPLICATE LIB: Foo", 12);
        assert_clean(&dup);
        assert!(dup.contains("Foo"));
        assert!(dup.contains("第 12 行"));

        let missing = friendly_errored_line(2, "Missing \"endif\"", 5);
        assert_clean(&missing);
        assert!(missing.contains("endif"));

        let nonexist = friendly_errored_line(5, "NON EXISTENT LIB: Bar", 7);
        assert_clean(&nonexist);
        assert!(nonexist.contains("Bar"));

        let crash = friendly_errored_line(1, "CRASH LINE", 3);
        assert_clean(&crash);
        assert!(crash.contains("第 3 行"));

        let path = friendly_errored_line(6, "PATH TOO LONG", 0);
        assert_clean(&path);
        assert!(path.contains("路径过长"));
    }

    #[test]
    fn join_error_to_ui_is_generic_and_clean() {
        let msg = join_error_to_ui();
        assert_clean(&msg);
        assert!(msg.contains("后台"));
    }
}
