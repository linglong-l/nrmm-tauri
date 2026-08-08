//! 文件日志双输出模块
//!
//! 提供 `DualWriter`，作为 `env_logger::Target::Pipe` 的输出目标，
//! 使所有 `log::xxx!` 调用同时写入 stderr 和每日日志文件。
//!
//! # 日志路径
//! `<data_local_dir>/nrmm-rust/logs/YYYY-MM-DD.log`
//!
//! # 设计要点
//! - 不新增 crate 依赖，仅使用 `std::fs`、`dirs`、`chrono`（均为现有依赖）
//! - `env_logger` 内部通过 `Mutex` 序列化写入，`DualWriter` 的 `Write` 实现无需额外同步
//! - 目录或文件创建失败时降级为仅 stderr（`file = None`），不 panic
//! - 日志文件按日期命名，应用启动时打开当天文件，追加写入

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

/// 双输出写入器：同时写入 stderr 和每日日志文件
///
/// 作为 `env_logger::Target::Pipe` 的目标，使所有 `log::xxx!` 调用
/// 同时输出到控制台和文件。`env_logger` 内部已通过 `Mutex` 序列化写入，
/// 因此 `DualWriter` 的 `Write` 实现无需额外同步原语。
///
/// # 字段
/// - `file`: 日志文件句柄，`None` 表示文件不可用（降级为仅 stderr）
pub struct DualWriter {
    /// 日志文件句柄，None 表示文件不可用（降级为仅 stderr）
    file: Option<File>,
}

impl DualWriter {
    /// 创建双输出写入器
    ///
    /// 日志路径: `<data_local_dir>/nrmm-rust/logs/YYYY-MM-DD.log`
    ///
    /// # 降级策略
    /// 目录或文件创建失败时 `file = None`，仅输出到 stderr，不影响主流程
    ///
    /// # 返回
    /// 初始化完成的 `DualWriter` 实例
    pub fn new() -> Self {
        let file = Self::open_daily_log();
        Self { file }
    }

    /// 打开当日日志文件
    ///
    /// 构建路径 `<data_local_dir>/nrmm-rust/logs/YYYY-MM-DD.log`，
    /// 创建目录（若不存在），以追加模式打开文件。
    ///
    /// # 返回
    /// - `Ok(file)`: 文件打开成功
    /// - `Err(())`: 路径获取、目录创建或文件打开失败
    fn open_daily_log() -> Option<File> {
        // 获取用户本地数据目录
        let data_dir = dirs::data_local_dir()?;
        let log_dir: PathBuf = data_dir
            .join("nrmm-rust")
            .join("logs");

        // 创建日志目录（若不存在）
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            eprintln!("[file_logger] Failed to create log dir {:?}: {}", log_dir, e);
            return None;
        }

        // 按日期命名日志文件
        let date_str = chrono::Local::now().format("%Y-%m-%d").to_string();
        let log_path = log_dir.join(format!("{}.log", date_str));

        // 以追加模式打开文件
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(mut f) => {
                // 写入分隔标记，标识新的一次启动
                let _ = writeln!(f, "\n--- NRMM 启动 {} ---", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
                Some(f)
            }
            Err(e) => {
                eprintln!("[file_logger] Failed to open log file {:?}: {}", log_path, e);
                None
            }
        }
    }
}

impl Default for DualWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for DualWriter {
    /// 将数据同时写入 stderr 和日志文件
    ///
    /// # 参数
    /// - `buf`: 待写入的字节缓冲区
    ///
    /// # 返回
    /// - `Ok(len)`: 成功写入的字节数（始终为 `buf.len()`）
    /// - `Err(e)`: stderr 写入失败时返回错误（文件写入失败仅记录，不返回错误）
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // 先写入 stderr
        io::stderr().write_all(buf)?;

        // 再写入文件（失败不影响主流程）
        if let Some(file) = self.file.as_mut() {
            let _ = file.write_all(buf);
        }

        Ok(buf.len())
    }

    /// 刷新 stderr 和日志文件的缓冲区
    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()?;
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 DualWriter 在无文件时仍可写入 stderr
    #[test]
    fn test_dual_writer_with_none_file() {
        let mut writer = DualWriter { file: None };
        let result = writer.write_all(b"test message\n");
        assert!(result.is_ok());
        let result = writer.flush();
        assert!(result.is_ok());
    }

    /// 测试 DualWriter::new() 不 panic
    ///
    /// 即使日志目录创建失败，DualWriter 也应正常构造（file=None）
    #[test]
    fn test_dual_writer_new_does_not_panic() {
        let writer = DualWriter::new();
        // 不断言 file 是否 Some，因为取决于运行环境
        let _ = writer;
    }

    /// 测试 DualWriter 实现了 Write + Send
    ///
    /// env_logger 的 Target::Pipe 要求 Box<dyn Write + Send>
    #[test]
    fn test_dual_writer_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<DualWriter>();
    }
}
