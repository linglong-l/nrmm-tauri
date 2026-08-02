//! d3dx.ini 解析结果缓存，避免每次扫描重复解析 Mods/d3dx.ini。
//!
//! 核心思路：以 canonicalize 后的路径作为键，将 `IniFile` 解析结果缓存到内存中，
//! 配合文件修改时间（`modified_at`）判断缓存是否失效。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use crate::core::ini_handler::IniFile;
use anyhow::Result;

/// 缓存的 INI 条目，包含解析结果和文件修改时间戳。
#[derive(Debug, Clone)]
struct CachedIni {
    /// `IniFile` 解析结果，供 `get_or_parse` 返回克隆。
    parsed: IniFile,
    /// 文件上次修改时间，用于判断缓存是否过期。
    ///
    /// 与 `std::fs::metadata` 返回的 `modified()` 比较，
    /// 不一致时触发重新解析。
    modified_at: SystemTime,
}

/// d3dx.ini 解析结果缓存，以 `HashMap<PathBuf, CachedIni>` 实现。
///
/// 所有路径均经过 `canonicalize` 规范化后作为键存储，
/// 避免因路径表示形式不同（如 `./d3dx.ini` vs `../Mods/d3dx.ini`）导致缓存未命中。
#[derive(Debug)]
pub struct D3dxIniCache {
    /// 内部缓存映射表，键为规范化后的 `PathBuf`，值为 `CachedIni`。
    inner: HashMap<PathBuf, CachedIni>,
}

impl Default for D3dxIniCache {
    /// 创建空缓存实例（等价于 `new()`）。
    fn default() -> Self { Self::new() }
}

impl D3dxIniCache {
    /// 创建一个新的空 d3dx.ini 缓存。
    pub fn new() -> Self { Self { inner: HashMap::new() } }

    /// 获取或解析指定路径的 INI 文件。
    ///
    /// # 缓存命中逻辑
    ///
    /// 1. 对 `path` 调用 `canonicalize` 得到规范化路径 `norm`。
    /// 2. 查询 `inner` 中是否存在 `norm` 对应的缓存条目。
    /// 3. 若命中，比较缓存条目的 `modified_at` 与文件当前修改时间 `fs_modified`：
    ///    - 一致 → 直接返回 `parsed` 的克隆。
    ///    - 不一致 → 视为失效，继续执行解析。
    /// 4. 未命中或失效时，调用 `IniFile::parse` 重新解析，插入缓存后返回结果。
    ///
    /// # 参数
    /// - `path`: 待解析的 d3dx.ini 文件路径。
    ///
    /// # 返回
    ///
    /// `Ok(IniFile)` 解析成功。
    ///
    /// # Errors
    ///
    /// - 文件不存在：`canonicalize` 失败或 `IniFile::parse` 返回 IO 错误。
    /// - INI 解析失败：`IniFile::parse` 返回语法错误。
    pub fn get_or_parse(&mut self, path: &Path) -> Result<IniFile> {
        let norm = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mt = fs_modified(&norm);
        if let Some(cached) = self.inner.get(&norm) {
            if Some(cached.modified_at) == mt {
                return Ok(cached.parsed.clone());
            }
        }
        let parsed = IniFile::parse(&norm)?;
        let mt_val = mt.unwrap_or_else(SystemTime::now);
        self.inner.insert(norm, CachedIni { parsed: parsed.clone(), modified_at: mt_val });
        Ok(parsed)
    }

    /// 按路径移除缓存的 INI 条目。
    ///
    /// 对 `path` 执行 `canonicalize` 规范化后，从 `inner` 中删除对应条目。
    ///
    /// # 参数
    /// - `path`: 要失效的 d3dx.ini 文件路径。
    pub fn invalidate(&mut self, path: &Path) {
        let norm = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.inner.remove(&norm);
    }
}

/// 读取文件的修改时间，不存在的文件返回 `None`。
///
/// 底层调用 `std::fs::metadata(p)?.modified()`，失败时静默返回 `None`。
///
/// # Panics
///
/// 此函数不会 panic。
///
/// # Errors
///
/// 此函数不会返回错误；文件不存在或元数据不可读时返回 `None`。
fn fs_modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok().and_then(|m| m.modified().ok())
}

/// 全局 d3dx.ini 缓存单例。
///
/// 使用 `once_cell::sync::Lazy` 实现首次访问时自动初始化，
/// 以 `parking_lot::RwLock` 包装支持并发读和独占写。
pub static D3DX_INI_CACHE: Lazy<RwLock<D3dxIniCache>> =
    Lazy::new(|| RwLock::new(D3dxIniCache::new()));