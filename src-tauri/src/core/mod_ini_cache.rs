//! 通用 INI 解析结果缓存，避免重复扫描时对每个模组 INI 文件重复执行 `IniFile::parse`。
//!
//! 与 `d3dxini_cache` 的区别：
//! - 本缓存面向**所有模组 INI 文件**（不只是 d3dx.ini）；
//! - 命中时返回 `IniFile` 的克隆（与 `d3dxini_cache` 一致），未命中才真正解析；
//! - 读取阶段**不在锁内执行文件 I/O**：先以读锁快速判定命中，未命中时释放锁再解析，
//!   解析完成后再以写锁回填，避免在大批量扫描时把全局锁当成 I/O 串行化瓶颈。
//!
//! 语义保证：缓存只存储与「直接调用 `IniFile::parse`」完全一致的解析结果，
//! 因此对解析产物（写入/校验/对比）无任何语义影响。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use once_cell::sync::Lazy;
use parking_lot::RwLock;

use crate::core::ini_handler::IniFile;

/// 缓存条目：解析结果（共享所有权）与失效判定信息。
#[derive(Debug, Clone)]
struct CachedIni {
    /// 解析结果，以 `Arc` 共享，命中时仅克隆 `Arc` 指针（廉价）。
    parsed: Arc<IniFile>,
    /// 文件修改时间，用于判断缓存是否过期（与 `std::fs::metadata().modified()` 比较）。
    modified_at: SystemTime,
    /// 文件字节长度，作为修改时间的辅助判定，避免同秒内的内容替换被误判为命中。
    len: u64,
}

/// 通用 INI 解析缓存，键为规范化后的 `PathBuf`。
#[derive(Debug)]
struct ModIniCache {
    inner: HashMap<PathBuf, CachedIni>,
}

impl ModIniCache {
    fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }
}

/// 全局通用 INI 缓存单例（首次访问时初始化）。
static MOD_INI_CACHE: Lazy<RwLock<ModIniCache>> =
    Lazy::new(|| RwLock::new(ModIniCache::new()));

/// 读取并解析指定 INI 文件；若缓存有效则直接返回克隆，否则解析并回填缓存。
///
/// 设计要点（避免 I/O 在锁内）：
/// 1. 以**读锁**快速检查：键存在且 `(modified_at, len)` 与当前文件一致 → 返回 `Arc` 克隆。
/// 2. 未命中则**释放读锁**，执行 `IniFile::parse`（真实文件 I/O 发生在锁外）。
/// 3. 解析成功后以**写锁**回填（二次校验，避免并发重复解析互相覆盖）。
///
/// # 参数
/// - `path`: 待解析的 INI 文件路径。
///
/// # 返回
/// `Ok(IniFile)` 解析成功（可能来自缓存或实时解析）。
///
/// # Errors
/// 文件不存在或无法读取 / 解析失败（`IniFile::parse` 透传的错误）。
pub fn get_or_parse_ini(path: &Path) -> Result<IniFile> {
    let norm = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let meta = std::fs::metadata(&norm);
    let (mt, len) = match &meta {
        Ok(m) => (m.modified().ok(), m.len()),
        Err(_) => (None, 0),
    };

    // 1) 读锁快速命中判定（不执行 I/O）
    {
        let cache = MOD_INI_CACHE.read();
        if let Some(c) = cache.inner.get(&norm) {
            if Some(c.modified_at) == mt && c.len == len {
                return Ok((*c.parsed).clone());
            }
        }
    }

    // 2) 未命中：释放读锁后在锁外解析（真实 I/O 不占用全局锁）
    let parsed = IniFile::parse(&norm)?;
    let mt_val = mt.unwrap_or_else(SystemTime::now);

    // 3) 写锁回填（二次校验，避免并发重复解析互相覆盖）
    {
        let mut cache = MOD_INI_CACHE.write();
        cache.inner.insert(
            norm,
            CachedIni {
                parsed: Arc::new(parsed.clone()),
                modified_at: mt_val,
                len,
            },
        );
    }

    Ok(parsed)
}

/// 按路径使缓存条目失效（文件被改写后调用，确保下次读取重新解析）。
///
/// 注意：缓存失效判定主要依赖 `(modified_at, len)`，正常情况下文件改写后
/// 这两项会变化并自动未命中；此函数用于需要**立即**失效的显式场景。
pub fn invalidate_ini(path: &Path) {
    let norm = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    MOD_INI_CACHE.write().inner.remove(&norm);
}

/// 清空全部缓存（如切换游戏目录、重置配置时调用）。
pub fn clear_ini_cache() {
    MOD_INI_CACHE.write().inner.clear();
}
