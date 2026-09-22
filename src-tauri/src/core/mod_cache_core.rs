//! 模组数据缓存核心：结构 + 纯生命周期 + 全局单例
//!
//! 提供内存缓存功能，避免重复扫描文件系统。
//! 缓存键为 (TargetGame, 规范化路径字符串)，值为轻量扫描结果 ScanResult。
//! 使用 parking_lot::RwLock 保证线程安全的并发读写。
//! 路径比较/组判定域见 [`crate::core::mod_cache_path`]，子树与模组级增量 patch 见
//! [`crate::core::mod_cache_patch`]，旧分组状态收集见 [`crate::core::mod_cache_delta`]。

use crate::core::mod_scan_light::ScanResult;
use crate::models::enums::TargetGame;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// 常驻缓存条目上限（LRU 淘汰阈值）。
///
/// 缓存仅保留最近交互的 3 个（游戏, 路径）结果；`set()` 插入后若超过该上限，
/// 淘汰 `_timestamp` 最旧的条目，以限制多游戏扫描结果在常驻内存中的累积量。
/// 被淘汰条目再次进入该游戏/路径时需触发一次全量重扫（~3-4s），内存优先。
const MAX_CACHE_ENTRIES: usize = 3;

/// 零拷贝保护的最小小库阈值（扁平模组数）。
///
/// [`ModCache::try_get_owned_result`] 仅在树规模 ≥ 本值时才于 Arc 被外部持有期间降级：
/// - **大树**（万级模组，实测深拷贝数秒～数十秒）：必须降级，否则 UI 卡顿 3~15s；
/// - **小库**（新用户/测试 fixture，几十个模组）：深拷贝仅毫秒级，降级反而要走组级
///   重扫（更慢），故仍走 `Arc::make_mut` 就地改。
pub(crate) const COW_GUARD_MIN_MODS: usize = 1000;

/// 测试专用：写全局 [`MOD_CACHE`] 的 delta 组装类测试的串行锁。
///
/// 全局缓存上限 [`MAX_CACHE_ENTRIES`] = 3，而使用全局缓存的测试共 9 个
/// （commands 的 3 个 R1 delta 测试 + watcher_incremental 的 6 个合并/路由测试）。
/// 并行执行时第 4 个 `set()` 触发 LRU 淘汰，可能在他测「set → 读旧分组状态」
/// 窗口内淘汰其条目，导致 old_mod_paths 随机为空的偶发断言失败；
/// 相关测试持此锁串行（顺序执行下各自的读紧跟自己的 set，无跨测试淘汰）。
/// 新增写全局缓存的测试时必须同样持锁（见 watcher_incremental_tests 的用法）。
#[cfg(test)]
pub static CACHE_WRITER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 缓存键类型，由目标游戏和规范化后的 Mods 目录路径字符串组成的二元组。
///
/// - `TargetGame`：区分不同游戏的缓存条目。
/// - `String`：`canonicalize` 规范化后的路径字符串，确保不同表示形式的同一路径命中同一缓存。
pub(crate) type CacheKey = (TargetGame, String);

/// 缓存条目，包含扫描结果和时间戳。
#[derive(Debug, Clone)]
pub(crate) struct CachedEntry {
    /// 轻量扫描结果（`Arc<ScanResult>`，缓存为唯一长期持有者）。
    ///
    /// 使用 `Arc` 使 `get()` 通过 `Arc::clone` 零拷贝共享给调用方（序列化/访问），
    /// 避免对整树做深克隆。`subtree_replace` 需要对子字段就地改时用
    /// `Arc::make_mut`：缓存仅唯一持有时原地可变（零拷贝），若调用方克隆了
    /// Arc 则写时拷贝（语义安全）。
    pub(crate) result: Arc<ScanResult>,
    /// 缓存插入时间戳，用于 LRU 淘汰时选择最旧条目剔除。
    _timestamp: Instant,
}

/// 模组数据缓存管理器。
///
/// 使用 `RwLock` 包裹 `HashMap`，支持多读单写。
/// 全局单例通过 `MOD_CACHE` 访问。
///
/// `cache` 保留所有缓存条目，`invalidated_games` 作为快速失效标记，
/// 避免在 `is_valid()` 中全量扫描 `cache` 来判断游戏是否失效。
#[derive(Debug)]
pub struct ModCache {
    /// 内部缓存存储，键为 `CacheKey`，值为 `CachedEntry`。
    pub(crate) cache: HashMap<CacheKey, CachedEntry>,
    /// 已被文件监控标记失效的游戏集合。
    ///
    /// `window-shown` 时查此集合快速判断是否可直接用缓存，
    /// 无需遍历 `cache` 中该游戏的所有条目。
    invalidated_games: HashSet<TargetGame>,
}

impl ModCache {
    /// 创建新的空缓存实例。
    ///
    /// 初始化空的 `HashMap` 和 `HashSet`，不分配任何缓存条目。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            invalidated_games: HashSet::new(),
        }
    }

    /// 查询缓存，命中返回共享的 `Arc<ScanResult>`（零拷贝，共享内部数据）。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径（将被规范化）。
    ///
    /// # 返回
    ///
    /// - `Some(Arc<ScanResult>)`: 缓存命中，返回结果的共享引用。
    /// - `None`: 缓存未命中。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn get(&self, game: TargetGame, mods_path: &Path) -> Option<Arc<ScanResult>> {
        let key = Self::make_key(game, mods_path);
        let result = self.cache.get(&key).map(|entry| Arc::clone(&entry.result));
        log::debug!(
            "[core::mod_cache] [get] game={:?} hit={}",
            game,
            result.is_some()
        );
        result
    }

    /// 写入缓存条目。
    ///
    /// 将 `result` 以 `(game, 规范化路径)` 为键存入 `cache`，
    /// 同时从 `invalidated_games` 中移除该游戏的失效标记，
    /// 表示该游戏的缓存已重新生效。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径。
    /// - `result`: 要缓存的轻量扫描结果。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn set(&mut self, game: TargetGame, mods_path: &Path, result: ScanResult) {
        let key = Self::make_key(game, mods_path);
        let mods_count = result.mods.len();
        self.cache.insert(
            key,
            CachedEntry {
                result: Arc::new(result),
                _timestamp: Instant::now(),
            },
        );
        // LRU 淘汰：超过上限时，按 _timestamp 淘汰最旧的溢出条目（逐次取全局最旧）。
        // 仅影响 set 时的内存上限；invalidate_* 显式失效语义不变。
        if self.cache.len() > MAX_CACHE_ENTRIES {
            let overflow = self.cache.len() - MAX_CACHE_ENTRIES;
            for _ in 0..overflow {
                let oldest_key = self
                    .cache
                    .iter()
                    .min_by_key(|(_, entry)| entry._timestamp)
                    .map(|(k, _)| k.clone());
                if let Some(k) = oldest_key {
                    self.cache.remove(&k);
                }
            }
        }
        // 清除该游戏的失效标记，表示缓存已更新
        self.invalidated_games.remove(&game);
        log::debug!(
            "[core::mod_cache] [set] game={:?} mods={}",
            game,
            mods_count
        );
    }

    /// 精确移除指定游戏 + 路径的一个缓存条目。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn invalidate(&mut self, game: TargetGame, mods_path: &Path) {
        let key = Self::make_key(game, mods_path);
        let removed = self.cache.remove(&key);
        log::debug!(
            "[core::mod_cache] [invalidate] game={:?} removed={}",
            game,
            removed.is_some()
        );
    }

    /// 失效指定游戏的所有缓存条目。
    ///
    /// 使用 `retain` 过滤掉所有 `CacheKey` 中 `game` 匹配的条目，
    /// 同时将该游戏加入 `invalidated_games` 集合，
    /// 以便后续 `is_valid()` 快速判断。
    ///
    /// # 参数
    /// - `game`: 要失效的目标游戏。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn invalidate_game(&mut self, game: TargetGame) {
        self.cache.retain(|(g, _), _| *g != game);
        self.invalidated_games.insert(game);
    }

    /// 取缓存结果的**独占**可变引用用于零拷贝就地改；不可独占时返回 `None`（调用方降级）。
    ///
    /// 缓存以 `Arc<ScanResult>` 共享，`Arc::make_mut` 在 Arc 被外部持有（如 `get_mods`
    /// 序列化在途 clone）时会**深拷贝整棵万级模组树**（实测数秒～数十秒，表现为 UI
    /// 3~15s 卡顿 + 命令排队后通知齐发）。故仅当唯一持有、或树规模小于
    /// [`COW_GUARD_MIN_MODS`]（深拷贝代价可忽略）时才返回 `Some`；否则 `None`，
    /// 由调用方降级（返回 miss 值 / 标记失效 / 走增量重扫）。
    ///
    /// # 参数
    /// - `game`: 目标游戏（缓存键）
    /// - `mods_path`: Mods 根目录（缓存键）
    /// - `op`: 调用方标识（降级度量日志用，如 `"patch_mod_entry"`）
    ///
    /// # 返回
    /// - `Some(&mut ScanResult)`：可安全就地改（唯一持有零拷贝，或小库深拷贝可忽略）
    /// - `None`：缓存未命中，或大树被外部持有 → 调用方降级
    pub(crate) fn try_get_owned_result(
        &mut self,
        game: TargetGame,
        mods_path: &Path,
        op: &str,
    ) -> Option<&mut ScanResult> {
        let key = Self::make_key(game, mods_path);
        let Some(existing) = self.cache.get_mut(&key) else {
            log::debug!("[mod_cache] {} 跳过：缓存未命中 (game={:?})", op, game);
            return None;
        };
        let strong = Arc::strong_count(&existing.result);
        let mods = existing.result.mods.len();
        if strong > 1 && mods >= COW_GUARD_MIN_MODS {
            // 度量点（P2）：COW 保护降级**发生率**。P0/P1 后单次降级已降至微秒级
            // （跳过 patch + 标失效，不做同步重扫），但频繁降级仍意味着 Arc 被外部
            // 长期持有（如 get_mods 的 IPC 序列化在途）。用 info 级便于统计与检索。
            log::info!(
                "[mod_cache] COW 保护降级 | op={} game={} mods={} strong_count={}",
                op,
                game.as_str(),
                mods,
                strong
            );
            return None;
        }
        Some(Arc::make_mut(&mut existing.result))
    }

    /// 清空所有缓存条目和失效标记。
    ///
    /// 清空 `cache` 和 `invalidated_games`，恢复到初始空状态。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn invalidate_all(&mut self) {
        self.cache.clear();
        self.invalidated_games.clear();
    }

    /// 生成缓存键（规范化路径）。
    ///
    /// 先调用 `canonicalize` 规范化路径，失败时回退到原始路径，
    /// 再将路径转换为 `String` 作为 `CacheKey` 的第二元素。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 待规范化的 Mods 目录路径。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub(crate) fn make_key(game: TargetGame, mods_path: &Path) -> CacheKey {
        let normalized = mods_path
            .canonicalize()
            .unwrap_or_else(|_| mods_path.to_path_buf())
            .to_string_lossy()
            .to_string();
        (game, normalized)
    }

    /// 检查指定游戏的缓存是否有效。
    ///
    /// 先检查 `invalidated_games` 中是否包含该游戏（快速失效标记），
    /// 若包含则直接返回 `false`；否则再查 `cache` 中是否存在对应条目。
    ///
    /// 用于 `window-shown` 时快速判断能否直接使用缓存，
    /// 避免不必要的全量重新扫描。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径。
    ///
    /// # 返回
    ///
    /// - `true`: 缓存有效，可直接使用。
    /// - `false`: 缓存已失效，需要重新扫描。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn is_valid(&self, game: TargetGame, mods_path: &Path) -> bool {
        if self.invalidated_games.contains(&game) {
            return false;
        }
        let key = Self::make_key(game, mods_path);
        self.cache.contains_key(&key)
    }
}

impl Default for ModCache {
    /// 创建默认的空缓存实例（等价于 `new()`）。
    fn default() -> Self {
        Self::new()
    }
}

/// 全局模组缓存单例。
///
/// 使用 `once_cell::sync::Lazy` 实现首次访问时自动初始化为空缓存，
/// 以 `parking_lot::RwLock` 包装支持并发读和独占写。
pub static MOD_CACHE: Lazy<RwLock<ModCache>> = Lazy::new(|| RwLock::new(ModCache::new()));

#[cfg(test)]
#[path = "mod_cache_core_tests.rs"]
mod tests;
