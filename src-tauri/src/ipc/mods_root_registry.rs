//! 已登记 Mods 根目录表（IPC 路径校验的锚点）
//!
//! # 为什么单独成表
//! 原实现的锚点是 `settings.game_mods_path`，而 `save_settings` 命令接受**完整**
//! `AppSettings` 对象——同一次调用既能把 `game_mods_path` 改成 `C:\`，又依赖它做
//! 边界判定，校验者可被自己校验的对象改写。本表把锚点从「前端可写字段」中拆出来：
//! 只有 [`register`] / [`sync_from`] 这两条后端通道能改变集合，命令入参一律只读。
//!
//! # fail-closed
//! 集合为空时 [`is_within_any_root`] 恒为 `false`。未配置游戏、或所有配置项都因
//! 不存在/不是目录而被剔除时，所有 `ManagedPath` 解析失败——绝不静默放行。
//!
//! # 分层
//! 本模块不依赖任何项目内上层模块；调用方把根路径以迭代器传入。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use anyhow::{bail, Context as _, Result};

/// 根集合。写锁仅保护集合本身，不持锁做 IO（`canonicalize` 在加锁前完成）
static ROOTS: LazyLock<RwLock<HashSet<PathBuf>>> = LazyLock::new(|| RwLock::new(HashSet::new()));

/// 取回锁的内值：锁中毒只意味着某次写操作 panic，集合本身仍可用，
/// 与仓库既有惯例（`unwrap_or_else(|p| p.into_inner())`）一致
fn roots_read() -> Vec<PathBuf> {
    ROOTS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .cloned()
        .collect()
}

/// 登记一个 Mods 根目录。
///
/// 路径必须存在且为目录；返回规范化后的形态（后续比较一律用此形态）。
///
/// # Errors
/// - `path` 不存在或不是目录
/// - `canonicalize` 失败
///
/// # Panics
/// 不会 panic（锁中毒时取内值）。
pub fn register(path: &Path) -> Result<PathBuf> {
    let canon = fs_canonicalize(path)?;
    if !canon.is_dir() {
        bail!("mods root is not a directory: {:?}", canon);
    }
    ROOTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(canon.clone());
    log::info!("[mods_root_registry] registered root: {:?}", canon);
    Ok(canon)
}

/// 注销一个 Mods 根目录（按规范化形态比较）。
///
/// # 返回
/// 确实移除时返回 `true`，原本不在表内返回 `false`
///
/// # Panics
/// 不会 panic（锁中毒时取内值）。
pub fn unregister(path: &Path) -> bool {
    let Ok(canon) = fs_canonicalize(path) else {
        return false;
    };
    let removed = ROOTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&canon);
    if removed {
        log::info!("[mods_root_registry] unregistered root: {:?}", canon);
    }
    removed
}

/// 用给定的根路径集合整体替换当前表。
///
/// 供设置持久化后同步（`settings_store` 的副作用钩子）与启动期初始化使用。
/// 无法规范化或不是目录的条目被**剔除并告警**，不纳入表——宁可拒绝命令，
/// 也不可把指向未知位置的条目当作可信锚点。
///
/// # 参数
/// - `roots`: 原始根路径（来自 `AppSettings::game_mods_path` 的值集合）
///
/// # 返回
/// 实际登记的条目数量
///
/// # Panics
/// 不会 panic（锁中毒时取内值）。
pub fn sync_from<P: AsRef<Path>, I: IntoIterator<Item = P>>(roots: I) -> usize {
    let mut next: HashSet<PathBuf> = HashSet::new();
    for raw in roots {
        let p = raw.as_ref();
        match fs_canonicalize(p) {
            Ok(canon) if canon.is_dir() => {
                next.insert(canon);
            }
            Ok(canon) => log::warn!("[mods_root_registry] 跳过非目录根：{:?}", canon),
            Err(e) => log::warn!("[mods_root_registry] 跳过不可解析根 {:?}: {e}", p),
        }
    }
    let count = next.len();
    *ROOTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    count
}

/// 当前已登记的根（规范化形态，顺序不保证）
///
/// # Panics
/// 不会 panic（锁中毒时取内值）。
pub fn roots() -> Vec<PathBuf> {
    roots_read()
}

/// 是否已有任何可用根。用于把「未配置游戏」与「路径越界」报成不同错误码
///
/// # Panics
/// 不会 panic（锁中毒时取内值）。
pub fn has_any_root() -> bool {
    !ROOTS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_empty()
}

/// 判断**已规范化**的路径是否落在任一根目录内（含根自身）。
///
/// 两侧均为 `canonicalize` 产物，故符号链接/目录联接指向表外时，其真实形态
/// 不匹配任何根，会被自然拦下（这也是要求调用方先规范化的原因）。
///
/// # 参数
/// - `canonical_target`: 必须已经过 `std::fs::canonicalize`
///
/// # 返回
/// 表为空时恒 `false`（fail-closed）
///
/// # Panics
/// 不会 panic（锁中毒时取内值）。
pub fn is_within_any_root(canonical_target: &Path) -> bool {
    roots_read()
        .iter()
        .any(|root| canonical_target.starts_with(root))
}

/// 规范化路径，失败时带上原始入参便于排障
fn fs_canonicalize(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).with_context(|| format!("canonicalize failed: {:?}", path))
}

/// 测试专用串行化守卫：凡触碰全局 [`ROOTS`] 的用例都必须先持有它。
///
/// 各测试模块**不得**自建各自的锁——`ROOTS` 是进程级单例，两把不同的锁等于没有锁：
/// 并行用例会互相清空登记表，产生「只在完整测试集下偶发」的假失败。
#[cfg(test)]
pub(crate) fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    /// 清空全局集合。生产代码不得调用（仅测试可见）
    fn clear_roots(_guard: &MutexGuard<'static, ()>) {
        *ROOTS
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = HashSet::new();
    }

    #[test]
    fn empty_registry_fails_closed() {
        let g = test_guard();
        clear_roots(&g);
        assert!(!has_any_root(), "未登记任何根时 has_any_root 应为 false");
        assert!(
            !is_within_any_root(Path::new("/")),
            "空表必须拒绝一切路径，不得静默放行"
        );
    }

    #[test]
    fn register_requires_existing_directory() {
        let g = test_guard();
        clear_roots(&g);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();

        assert!(register(&file).is_err(), "文件不能充当 Mods 根");
        assert_eq!(roots(), Vec::<PathBuf>::new(), "失败的登记不得留下副作用");

        let root = register(dir.path()).unwrap();
        assert!(has_any_root());
        assert_eq!(roots(), vec![root]);
    }

    #[test]
    fn sync_from_drops_unusable_entries_and_keeps_valid_ones() {
        let g = test_guard();
        clear_roots(&g);
        let ok = tempfile::tempdir().unwrap();
        let valid = ok.path().to_path_buf();
        let ghost = valid.join("does-not-exist");

        let n = sync_from(vec![
            valid.to_string_lossy().to_string(),
            ghost.to_string_lossy().to_string(),
        ]);
        assert_eq!(n, 1, "不可解析的条目应被剔除");
        assert_eq!(roots().len(), 1);

        // 整体替换语义：再次同步到空集合后回到 fail-closed
        assert_eq!(sync_from(Vec::<String>::new()), 0);
        assert!(!has_any_root());
    }

    #[test]
    fn containment_is_by_canonical_prefix() {
        let g = test_guard();
        clear_roots(&g);
        let dir = tempfile::tempdir().unwrap();
        let root = register(dir.path()).unwrap();
        let inner = dir.path().join("Mods").join("group_1");
        std::fs::create_dir_all(&inner).unwrap();
        let inner_canon = std::fs::canonicalize(&inner).unwrap();

        assert!(is_within_any_root(&inner_canon), "根内路径应命中");
        let outside = tempfile::tempdir().unwrap();
        let outside_canon = std::fs::canonicalize(outside.path()).unwrap();
        assert!(
            !is_within_any_root(&outside_canon),
            "根外路径不得命中：{:?} vs root {:?}",
            outside_canon,
            root
        );
    }

    #[test]
    fn unregister_removes_only_the_given_root() {
        let g = test_guard();
        clear_roots(&g);
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        register(a.path()).unwrap();
        register(b.path()).unwrap();
        assert_eq!(roots().len(), 2);

        assert!(unregister(a.path()));
        assert!(!unregister(a.path()), "重复注销应返回 false");
        assert_eq!(roots().len(), 1);
    }
}
