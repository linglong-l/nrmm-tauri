//! INI 编解码域：读取与编码转换（GBK/BOM/UTF-8 探测、原子写出）
//!
//! 从 ini_handler.rs 拆出（2026-09-18 架构拆分试点 3.1）。
//! 零内部域依赖；线程级读缓冲池随域迁移。

use anyhow::{Context, Result};
use std::cell::RefCell;
use std::fs;
use std::io::{BufReader, Read};
use std::path::Path;

/// 文件读取缓冲容量：足够覆盖绝大多数 INI 单行，同时避免一次性把整个文件读入内存。
pub(crate) const READ_BUFFER_CAPACITY: usize = 256 * 1024;

/// 单次读取数据量上限（20MB）：INI 读取策略的分界点。
///
/// - 文件大小 ≤ 此值：直接读取（[`fs::read`]/[`read_to_end`]），开销最低；
/// - 文件大小 > 此值：强制采用流式读取，按此容量分块 [`read`] 拼接，确保任意单次
///   读操作都不超过 20MB，避免超大 INI（如数 GB 的合并日志型 ini）一次性占满内存。
///
/// 注：[`crate::core::ini_model::IniFile::parse`] 本就采用 `BufReader` 逐行
/// （[`BufRead::read_line`]）流式解析，单行缓冲固定为 `READ_BUFFER_CAPACITY`，
/// 因此解析路径天然有界、不整文件驻留；本常量主要约束 [`read_file_bytes`]
/// （整体读取型路径，如 `force_read_as_utf8`）。
pub(crate) const MAX_DIRECT_READ_BYTES: u64 = 20 * 1024 * 1024;

// 线程级读取缓冲池：跨多次流式读取复用（`LINE_BUF`/`DECODED_BUF` 留待 3.3 随 model 迁移）。
thread_local! {
    pub(crate) static READ_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(64 * 1024));
}

/// 以「缓冲池 + 流式分块」读取整个文件字节，复用线程级 `READ_BUF` 避免重复分配。
///
/// 读取策略（统一流式，单次读 ≤ [`MAX_DIRECT_READ_BYTES`]）：
/// - 文件大小 ≤ 20MB：直接 `read_to_end`（语义等价于 `fs::read`），开销最低；
/// - 文件大小 > 20MB：按 20MB 分块循环 `read` 拼接，确保任意单次读操作都不超过 20MB，
///   避免超大 INI 一次性占满内存（仍最终返回完整字节，供 `force_read_as_utf8` 解码）。
///
/// 返回文件原始字节；仅读取方式（缓冲/分块/复用）不同，语义与 `fs::read` 一致。
pub(crate) fn read_file_bytes(path: &Path) -> Result<Vec<u8>> {
    let meta = fs::metadata(path).with_context(|| format!("Failed to stat file: {:?}", path))?;
    let file = fs::File::open(path).with_context(|| format!("Failed to open file: {:?}", path))?;

    // 小文件（≤ 20MB）：直接读取，最快路径
    if meta.len() <= MAX_DIRECT_READ_BYTES {
        let mut reader = BufReader::with_capacity(READ_BUFFER_CAPACITY, file);
        return READ_BUF.with(|b| {
            let mut buf = b.borrow_mut();
            buf.clear();
            reader
                .read_to_end(&mut buf)
                .with_context(|| format!("Failed to read file: {:?}", path))?;
            Ok(buf.clone())
        });
    }

    // 大文件（> 20MB）：流式分块读取，单次读严格不超过 MAX_DIRECT_READ_BYTES
    log::debug!(
        "[ini_handler] 大文件（{} 字节）采用流式分块读取（单块上限 {}MB）: {:?}",
        meta.len(),
        MAX_DIRECT_READ_BYTES / (1024 * 1024),
        path
    );
    let mut reader: BufReader<fs::File> = BufReader::with_capacity(READ_BUFFER_CAPACITY, file);
    let mut out: Vec<u8> = Vec::with_capacity(MAX_DIRECT_READ_BYTES as usize);
    let mut chunk: Vec<u8> = vec![0u8; MAX_DIRECT_READ_BYTES as usize];
    loop {
        let mut total = 0usize;
        // 单次 read 至多填充一个完整 chunk（≤ 20MB），避免一次性读取超大字节量
        while total < chunk.len() {
            let n = reader
                .read(&mut chunk[total..])
                .with_context(|| format!("Failed to read file: {:?}", path))?;
            if n == 0 {
                break;
            }
            total += n;
        }
        if total == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..total]);
    }
    Ok(out)
}

/// 源文件编码（写回策略依据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceEncoding {
    /// 文件本身是合法 UTF-8（含或不含 BOM）。
    Utf8,
    /// 文件非 UTF-8，已按 GB18030（覆盖 GBK/GB2312）无损解码为 Unicode。
    Gbk,
}

/// 统一 INI 读取管线的源编码信息，供 [`write_utf8_atomic`] 决定写回策略。
#[derive(Debug, Clone, Copy)]
pub struct SourceEncodingInfo {
    /// 源文件实际编码。
    pub encoding: SourceEncoding,
    /// 源文件是否带 UTF-8 BOM（`EF BB BF`）。
    pub had_bom: bool,
}

/// 统一 INI 读取管线（C6）：先判断后读取。
///
/// 1. 合法 UTF-8（含或不含 BOM）→ 直通，`encoding == Utf8`，字节不被改写；
/// 2. 非 UTF-8 → 按 GB18030（覆盖 GBK/GB2312）解码为 Unicode 再编码为 UTF-8，
///    `encoding == Gbk`。既非 UTF-8 也无法按 GB18030 解码的字节以 U+FFFD 替换并
///    `log::warn`（全数据集统一 UTF-8 决策，不拒绝写回）。
///
/// 返回值中的字符串**不含** UTF-8 BOM 前缀；`had_bom` 交由 [`write_utf8_atomic`]
/// 决定写回是否补回 BOM。
///
/// # Errors
/// 文件无法读取时返回对应 `std::io::Error`。
pub fn read_ini_safe(path: &Path) -> std::io::Result<(String, SourceEncodingInfo)> {
    let bytes = fs::read(path)?;
    let (had_bom, body) = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (true, &bytes[3..])
    } else {
        (false, &bytes[..])
    };
    match std::str::from_utf8(body) {
        Ok(s) => Ok((
            s.to_string(),
            SourceEncodingInfo {
                encoding: SourceEncoding::Utf8,
                had_bom,
            },
        )),
        Err(_) => {
            let (decoded, _, had_errors) = encoding_rs::GB18030.decode(body);
            if had_errors {
                log::warn!(
                    "[ini_handler] read_ini_safe: 文件含既非 UTF-8 亦无法按 GB18030 解码的字节，已按 U+FFFD 替换：{:?}",
                    path
                );
            } else {
                // 埋点 #6：GBK→Unicode→UTF-8 无损转换记录（全数据集统一 UTF-8 决策可审计）
                log::info!(
                    "[ini_handler] read_ini_safe: 非 UTF-8 文件无损转换（GB18030→UTF-8，{} 字节，BOM={}）：{:?}",
                    body.len(),
                    had_bom,
                    path
                );
            }
            Ok((
                decoded.into_owned(),
                SourceEncodingInfo {
                    encoding: SourceEncoding::Gbk,
                    had_bom,
                },
            ))
        }
    }
}

/// 统一 UTF-8 写回（C6）：保留源 BOM 策略 + 原子写。
///
/// `content` 为已就绪的 UTF-8 文本；`info.encoding == Gbk` 表示本次是转换写回，
/// `info.had_bom` 为真时在内容前置 UTF-8 BOM 前缀。通过 `crate::utils::atomic_write`
/// 原子落盘（同目录临时文件 + rename），进程任一时刻退出都不产生半成品目标文件。
///
/// # Errors
/// 创建临时文件、写入或替换失败时返回对应 `std::io::Error`。
pub fn write_utf8_atomic(
    path: &Path,
    content: &str,
    info: SourceEncodingInfo,
) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(content.len() + 3);
    if info.had_bom {
        buf.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    buf.extend_from_slice(content.as_bytes());
    crate::utils::atomic_write(path, &buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ========== C6 统一编码管线（read_ini_safe / write_utf8_atomic） ==========
    #[test]
    fn test_read_ini_safe_gbk_roundtrip() {
        // GBK 字节 [0xC4, 0xE3, 0xBA, 0xC3] 即 "你好"
        let f = tempfile::NamedTempFile::with_suffix(".ini").unwrap();
        fs::write(f.path(), [0xC4, 0xE3, 0xBA, 0xC3]).unwrap();
        let (content, info) = read_ini_safe(f.path()).unwrap();
        assert_eq!(content, "你好");
        assert_eq!(info.encoding, SourceEncoding::Gbk);
        assert!(!info.had_bom);
    }

    #[test]
    fn test_read_ini_safe_utf8_bom_variants() {
        // 带 BOM：had_bom = true，内容直通（不含 BOM 前缀）
        let f1 = tempfile::NamedTempFile::with_suffix(".ini").unwrap();
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice("你好".as_bytes());
        fs::write(f1.path(), &with_bom).unwrap();
        let (c1, i1) = read_ini_safe(f1.path()).unwrap();
        assert_eq!(c1, "你好");
        assert_eq!(i1.encoding, SourceEncoding::Utf8);
        assert!(i1.had_bom);

        // 无 BOM：had_bom = false，内容直通
        let f2 = tempfile::NamedTempFile::with_suffix(".ini").unwrap();
        fs::write(f2.path(), b"hello\n").unwrap();
        let (c2, i2) = read_ini_safe(f2.path()).unwrap();
        assert_eq!(c2, "hello\n");
        assert_eq!(i2.encoding, SourceEncoding::Utf8);
        assert!(!i2.had_bom);
    }

    #[test]
    fn test_write_utf8_atomic_preserves_bom_and_cleans_tmp() {
        let f = tempfile::NamedTempFile::with_suffix(".ini").unwrap();
        let info = SourceEncodingInfo {
            encoding: SourceEncoding::Gbk,
            had_bom: true,
        };
        write_utf8_atomic(f.path(), "你好", info).unwrap();
        let bytes = fs::read(f.path()).unwrap();
        assert!(
            bytes.starts_with(&[0xEF, 0xBB, 0xBF]),
            "had_bom=true 应写出 UTF-8 BOM"
        );
        assert_eq!(&bytes[3..], "你好".as_bytes(), "主体内容应为 UTF-8");

        // 原子写：同目录临时文件应被清理，无残留
        let file_name = f
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .expect("temp file name");
        let tmp = f.path().with_file_name(format!("{file_name}.tmp_write"));
        assert!(!tmp.exists(), "原子写后临时文件应被清理");
    }

    #[test]
    fn test_write_utf8_atomic_without_bom() {
        let f = tempfile::NamedTempFile::with_suffix(".ini").unwrap();
        let info = SourceEncodingInfo {
            encoding: SourceEncoding::Utf8,
            had_bom: false,
        };
        write_utf8_atomic(f.path(), "hello", info).unwrap();
        let bytes = fs::read(f.path()).unwrap();
        assert!(
            !bytes.starts_with(&[0xEF, 0xBB, 0xBF]),
            "had_bom=false 不应写 BOM"
        );
        assert_eq!(bytes, b"hello");
    }
}