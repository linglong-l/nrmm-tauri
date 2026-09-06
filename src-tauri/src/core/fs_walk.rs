//! 文件系统遍历工具集（防循环 + UTF-8 路径合法化 + 非法条目传输）
//!
//! 提供三类能力，供所有涉及目录遍历的模块（mod_scanner、namespace_handler、
//! archive_handler、mod_manager 等）统一消费：
//!
//! 1. [`PathPool`]：symlink 防循环路径池。同时登记「链接路径」与「解析后的真实
//!    绝对路径」，任何形态（链接、verbatim、大小写差异）都不引发重复遍历或循环。
//!    路径池工作在 OsStr 字节层，与 UTF-8 校验互不干扰——无论路径是否合法，
//!    只要字节相同即被视为同一目录。
//! 2. [`TraversalStats`]：UTF-8 校验统计收集器。提供线程安全的跳过计数与
//!    [`SkippedEntry`] 明细列表（上限保护），供 rayon 并行扫描各任务共享写入。
//! 3. [`os_str_transport_bytes`]：将非法 UTF-8 的 `OsStr` 转为平台编码的保真
//!    字节（base64）传输给前端，前端按前缀解码展示，避免往返丢失。

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// base64 字母表（RFC 4648，带 `+/` 填充字符）
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 无依赖 base64 编码（RFC 4648，标准填充 `=`）
///
/// 手写实现避免为单点功能引入 base64 crate 依赖（遵循「减少对 Tauri 插件库依赖」约定）。
///
/// # 参数
/// - `input`: 待编码的原始字节
///
/// # 返回
/// base64 字符串（含 `=` 填充）
pub(crate) fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(BASE64_ALPHABET[(n >> 18) as usize & 0x3F] as char);
        out.push(BASE64_ALPHABET[(n >> 12) as usize & 0x3F] as char);
        if chunk.len() > 1 {
            out.push(BASE64_ALPHABET[(n >> 6) as usize & 0x3F] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_ALPHABET[n as usize & 0x3F] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// 非 UTF-8 文件名的保真传输格式：`{前缀}:{base64}`
///
/// - Windows：`u16le:` + UTF-16LE 字节（`encode_wide` 产物），可无损还原孤代理项
/// - Linux/macOS：`raw:` + OsStr 原始字节（`as_bytes` 产物）
///
/// 前端按前缀选择解码策略（Windows 用 `String.fromCharCode(...u16)` 保真展示，
/// 其余按字节映射展示）。
///
/// # 参数
/// - `os`: 待传输的 `OsStr`（通常为非法 UTF-8 的文件名）
///
/// # 返回
/// 带前缀的保真字节字符串
pub fn os_str_transport_bytes(os: &OsStr) -> String {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let units: Vec<u16> = os.encode_wide().collect();
        let mut buf = Vec::with_capacity(units.len() * 2);
        for u in units {
            buf.extend_from_slice(&u.to_le_bytes());
        }
        format!("u16le:{}", base64_encode(&buf))
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::OsStrExt;
        format!("raw:{}", base64_encode(os.as_bytes()))
    }
}

/// 被跳过的非法 UTF-8 路径条目（供前端提示与定位）
///
/// # 字段
/// - `parent_display`: 所在父目录的显示路径。该路径在遍历中逐层校验，恒为合法
///   UTF-8（能到达此层说明祖先段均已通过校验），前端可直接展示。
/// - `name_bytes`: 非法文件名的保真字节（见 [`os_str_transport_bytes`]），
///   前端解码后可定位具体是哪个文件。
/// - `kind`: 条目类型：0=目录 1=文件 2=图片候选 3=符号链接
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedEntry {
    /// 所在父目录的显示路径（恒为合法 UTF-8）
    pub parent_display: String,
    /// 非法文件名的保真字节（base64，前缀标识平台编码）
    pub name_bytes: String,
    /// 条目类型：0=目录 1=文件 2=图片候选 3=符号链接
    pub kind: u8,
}

/// 条目类型常量（与 `SkippedEntry.kind` 对齐）
pub mod kind {
    /// 目录
    pub const DIR: u8 = 0;
    /// 文件
    pub const FILE: u8 = 1;
    /// 图片候选（图标/预览图）
    pub const IMAGE: u8 = 2;
    /// 符号链接
    pub const SYMLINK: u8 = 3;
}

/// UTF-8 校验统计收集器（线程安全，供 rayon 并行扫描共享写入）
///
/// # 设计
/// - 计数用 `AtomicUsize`，明细用 `Mutex<Vec>`：并行任务各自 `record` 无冲突
/// - 明细列表设上限 [`CAP_ENTRIES`]（30 条），防止海量非法文件名撑爆 IPC 载荷；
///   计数不受限，始终准确
#[derive(Default)]
pub struct TraversalStats {
    /// 非法 UTF-8 条目总计数（不受上限约束）
    count: AtomicUsize,
    /// 非法条目明细（受 CAP_ENTRIES 上限约束）
    entries: Mutex<Vec<SkippedEntry>>,
}

/// 明细列表上限：超过后只计数、不再追加明细（防止 IPC 载荷爆炸）
pub const CAP_ENTRIES: usize = 30;

impl TraversalStats {
    /// 记录一个被跳过的非法 UTF-8 条目。
    ///
    /// # 参数
    /// - `parent_display`: 父目录显示路径（合法 UTF-8）
    /// - `name`: 非法文件名的 `OsStr`
    /// - `kind`: 条目类型（`kind::*` 常量）
    pub fn record(&self, parent_display: impl Into<String>, name: &OsStr, kind: u8) {
        self.count.fetch_add(1, Ordering::Relaxed);
        let mut entries = match self.entries.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if entries.len() < CAP_ENTRIES {
            entries.push(SkippedEntry {
                parent_display: parent_display.into(),
                name_bytes: os_str_transport_bytes(name),
                kind,
            });
        }
    }

    /// 取回统计结果：`(跳过总数, 明细列表)`。
    ///
    /// 消费完自身（读取计数后清零），供扫描主流程汇总进 `ScanResult`/`UpdateResult`。
    pub fn into_parts(self) -> (usize, Vec<SkippedEntry>) {
        let count = self.count.load(Ordering::Relaxed);
        let entries = self.entries.into_inner().unwrap_or_default();
        (count, entries)
    }
}

/// 防 symlink 循环路径池。
///
/// 同时存储「链接路径」与「解析后的真实绝对路径」两组信息：
/// - `visited`: 解析后的真实路径集合。`try_visit` 对目标 `canonicalize`（失败回退
///   原文）后插入，已存在即拒绝——无论原始形态如何（symlink、verbatim 前缀、
///   大小写），同一真实目录只遍历一次，杜绝循环与重复。
/// - `symlinks`: 链接路径 → 解析路径映射。登记后供 [`visible_path`](Self::visible_path)
///   将遍历到的链接路径归一为池内真实路径，保证读写/比较使用同一视角。
///
/// `visited`/`symlinks` 均以 `PathBuf`（OsStr 字节）为键，**不依赖 UTF-8 合法性**——
/// 非法文件名的目录同样入池判重，不会因此循环。
#[derive(Default)]
pub struct PathPool {
    /// 解析后的真实绝对路径集合（防循环/重复）
    visited: HashSet<PathBuf>,
    /// 链接路径 → 解析后真实路径
    symlinks: HashMap<PathBuf, PathBuf>,
}

impl PathPool {
    /// 创建空路径池（等价于 `Default`，语义更明确）
    pub fn new() -> Self {
        Self::default()
    }

    /// 尝试访问一个目录：解析后判重，首次见到插入并返回 `true`，重复返回 `false`。
    ///
    /// 也可登记普通文件路径（解析失败时回退原文，文件只入池判重不遍历）。
    ///
    /// # 参数
    /// - `p`: 待访问的路径（目录或文件）
    ///
    /// # 返回
    /// `true` = 首次访问（应继续遍历），`false` = 已访问/循环/重复（应跳过）
    pub fn try_visit(&mut self, p: &Path) -> bool {
        let real = canonicalize_fallback(p);
        self.visited.insert(real)
    }

    /// 登记符号链接映射：`link → real`（链接路径 → 解析后真实路径）。
    ///
    /// 与 [`try_visit`](Self::try_visit) 配合：链接目录遍历时先登记映射再判重，
    /// 使后续经 `visible_path` 归一出的路径与池内一致。
    ///
    /// # 参数
    /// - `link`: 符号链接/磁链接路径
    ///
    /// # 返回
    /// 解析后的真实绝对路径（与登记一致）
    pub fn record_symlink(&mut self, link: &Path) -> PathBuf {
        let real = canonicalize_fallback(link);
        self.symlinks.insert(link.to_path_buf(), real.clone());
        real
    }

    /// 将链接路径归一为池内已登记的真实路径；未登记时返回 `None`。
    ///
    /// # 参数
    /// - `link`: 之前经 [`record_symlink`](Self::record_symlink) 登记的链接路径
    ///
    /// # 返回
    /// 池内真实路径的引用（无效链接/未登记返回 `None`）
    pub fn visible_path(&self, link: &Path) -> Option<&Path> {
        self.symlinks.get(link).map(PathBuf::as_path)
    }
}

/// `canonicalize` 失败时回退原路径（不因解析失败阻塞遍历）
fn canonicalize_fallback(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// 校验 `DirEntry` 文件名是否为合法 UTF-8；合法返回 `Some(name)`，非法返回 `None` 且
/// 记录跳过统计。
///
/// 供遍历收集点统一使用：结构性路径（mod/group/ini 目录）严格校验，非法跳过并记录。
///
/// # 参数
/// - `entry`: 目录项
/// - `parent_display`: 父目录显示路径
/// - `kind`: 条目类型
/// - `stats`: 跳过统计收集器
///
/// # 返回
/// 合法 UTF-8 文件名（`OsString` 转为 `String`）；非法返回 `None`
pub fn valid_entry_name(
    entry: &fs::DirEntry,
    parent_display: &str,
    kind: u8,
    stats: &TraversalStats,
) -> Option<String> {
    let name = entry.file_name();
    match name.to_str() {
        Some(s) => Some(s.to_string()),
        None => {
            log::warn!(
                "[fs_walk] 跳过非法 UTF-8 名称条目 | parent={} kind={} path={:?}",
                parent_display,
                kind,
                entry.path()
            );
            stats.record(parent_display, &name, kind);
            None
        }
    }
}

/// 校验合法的 `String` 路径段是否为合法 UTF-8（`to_str` 语义，供非 `DirEntry` 场景）。
///
/// 实际 `String` 恒为合法 UTF-8，本函数主要供与 `OsStr` 混合的场景保持统一入口，
/// 直接返回 `Some(s)`。
///
/// # 参数
/// - `s`: 路径段字符串
///
/// # 返回
/// `Some(s)`（`String` 恒合法）
pub fn valid_utf8_str(s: &str) -> Option<&str> {
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// base64 编码正确性（RFC 4648 标准用例）
    #[test]
    fn base64_encode_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    /// base64 对任意二进制输入保持长度守恒（3n → 4n）
    #[test]
    fn base64_encode_random_lengths() {
        for len in 0usize..=32 {
            let input: Vec<u8> = (0..len).map(|i| (i * 7 + 13) as u8).collect();
            let encoded = base64_encode(&input);
            assert_eq!(encoded.len(), len.div_ceil(3) * 4, "len={len}");
            // 解码回归：仅验证非空（无外部解码器依赖）
            assert!(!encoded.contains('\u{FFFD}'));
        }
    }

    /// 创建指向目录的符号链接（平台无关辅助）
    #[cfg(unix)]
    fn symlink_dir_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    /// 创建指向目录的符号链接（平台无关辅助）
    #[cfg(windows)]
    fn symlink_dir_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    /// 路径池：symlink 循环目录只遍历一次（无权限创建链接时跳过——Windows 需管理员）
    #[test]
    fn path_pool_deduplicates_symlink_cycle() -> std::io::Result<()> {
        let tmp = tempfile::tempdir()?;
        let real_dir = tmp.path().join("real");
        fs::create_dir_all(&real_dir)?;
        // 链接 A -> real 指向同一真实目录（canonicalize 后判重）
        let link_a = tmp.path().join("link_a");
        if let Err(e) = symlink_dir_link(&real_dir, &link_a) {
            eprintln!("跳过 symlink 测试（无权限）: {e}");
            return Ok(());
        }
        fs::create_dir_all(real_dir.join("sub"))?;

        let mut pool = PathPool::new();
        // 根目录首次访问
        assert!(pool.try_visit(&real_dir), "根目录应首次访问");
        // 链接 A 解析后与 real_dir 相同 → 判重拒绝（防循环的关键）
        assert!(!pool.try_visit(&link_a), "链接同名目录应判重");
        // 普通子目录仍可访问
        assert!(pool.try_visit(&real_dir.join("sub")), "真实子目录应可访问");
        Ok(())
    }

    /// 路径池：canonicalize 失败时回退原文，仍可判重
    #[test]
    fn path_pool_fallback_on_canonicalize_failure() {
        let mut pool = PathPool::new();
        let ghost = PathBuf::from("C:/this/does/not/exist/任何路径");
        assert!(
            pool.try_visit(&ghost),
            "不存在的路径应首次可访问（回退原文）"
        );
        assert!(!pool.try_visit(&ghost), "同一路径应判重");
    }

    /// 路径池：symlink 映射登记与可见路径归一（无权限创建链接时跳过——Windows 需管理员）
    #[test]
    fn path_pool_symlink_mapping() -> std::io::Result<()> {
        let tmp = tempfile::tempdir()?;
        let real = tmp.path().join("real_target");
        fs::create_dir_all(&real)?;
        let link = tmp.path().join("alias");
        if let Err(e) = symlink_dir_link(&real, &link) {
            eprintln!("跳过 symlink 映射测试（无权限）: {e}");
            return Ok(());
        }

        let mut pool = PathPool::new();
        let resolved = pool.record_symlink(&link);
        assert!(pool.visible_path(&link).is_some(), "链接应已登记");
        assert_eq!(
            resolved.as_os_str(),
            pool.visible_path(&link).unwrap().as_os_str()
        );
        // 未登记的路径返回 None
        assert!(pool.visible_path(Path::new("C:/unregistered")).is_none());
        Ok(())
    }

    /// 统计收集器：计数准确、明细有上限
    #[test]
    fn traversal_stats_caps_entries() {
        use std::ffi::OsString;
        #[cfg(not(windows))]
        use std::os::unix::ffi::OsStringExt;
        #[cfg(windows)]
        use std::os::windows::ffi::OsStringExt;

        let stats = TraversalStats::default();
        for i in 0u16..100 {
            // Windows：孤代理项（非法 UTF-8）；Unix：单字节 0xFF（非法 UTF-8）
            #[cfg(windows)]
            let name = OsString::from_wide(&[0xD800u16, i]);
            #[cfg(not(windows))]
            let name = OsString::from_vec(vec![0xFF, i as u8]);
            stats.record("C:/Mods/_MANAGED_", &name, 0);
        }
        let (count, entries) = stats.into_parts();
        assert_eq!(count, 100, "计数不应受明细上限约束");
        assert_eq!(entries.len(), CAP_ENTRIES, "明细应在上限内");
        #[cfg(windows)]
        assert_eq!(
            entries[0].name_bytes.chars().next(),
            Some('u'),
            "Windows 前缀应为 u16le:"
        );
        #[cfg(not(windows))]
        assert_eq!(
            entries[0].name_bytes.chars().next(),
            Some('r'),
            "Unix 前缀应为 raw:"
        );
    }
}
