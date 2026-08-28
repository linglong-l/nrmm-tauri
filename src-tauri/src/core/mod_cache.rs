//! 模组数据缓存模块
//!
//! 提供内存缓存功能，避免重复扫描文件系统。
//! 缓存键为 (TargetGame, 规范化路径字符串)，值为轻量扫描结果 ScanResult。
//! 使用 parking_lot::RwLock 保证线程安全的并发读写。

use crate::core::mod_scanner::{group_order_cmp, recount_mods, ScanResult};
use crate::models::enums::TargetGame;
use crate::models::mod_data::ModData;
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

/// 缓存键类型，由目标游戏和规范化后的 Mods 目录路径字符串组成的二元组。
///
/// - `TargetGame`：区分不同游戏的缓存条目。
/// - `String`：`canonicalize` 规范化后的路径字符串，确保不同表示形式的同一路径命中同一缓存。
type CacheKey = (TargetGame, String);

/// 缓存条目，包含扫描结果和时间戳。
#[derive(Debug, Clone)]
struct CachedEntry {
    /// 轻量扫描结果（`Arc<ScanResult>`，缓存为唯一长期持有者）。
    ///
    /// 使用 `Arc` 使 `get()` 通过 `Arc::clone` 零拷贝共享给调用方（序列化/访问），
    /// 避免对整树做深克隆。`subtree_replace` 需要对子字段就地改时用
    /// `Arc::make_mut`：缓存仅唯一持有时原地可变（零拷贝），若调用方克隆了
    /// Arc 则写时拷贝（语义安全）。
    result: Arc<ScanResult>,
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
    cache: HashMap<CacheKey, CachedEntry>,
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

    /// 按路径前缀批量失效缓存条目，用于文件监控。
    ///
    /// 遍历所有缓存键，如果路径字符串（大小写不敏感）包含 `managed_path` 前缀则移除。
    /// 用于文件监控检测到 `_MANAGED_` 目录变化时，失效受影响游戏的缓存。
    ///
    /// 同时扫描 `cache` 中剩余的所有游戏，将它们加入 `invalidated_games` 集合，
    /// 确保 `is_valid()` 能感知到部分条目可能已被删除。
    ///
    /// # 参数
    /// - `managed_path`: `_MANAGED_` 目录的路径，将被规范化为字符串进行前缀匹配。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn invalidate_by_prefix(&mut self, managed_path: &Path) {
        let prefix = match managed_path.canonicalize() {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => managed_path.to_string_lossy().to_string(),
        };
        let prefix_lower = prefix.to_lowercase();

        // 先收集受影响的游戏（路径包含前缀的条目），再移除条目
        // 避免误标所有剩余游戏为失效
        let affected_games: HashSet<TargetGame> = self
            .cache
            .keys()
            .filter(|(_, path_str)| path_str.to_lowercase().contains(&prefix_lower))
            .map(|(g, _)| *g)
            .collect();

        self.cache
            .retain(|(_, path_str), _| !path_str.to_lowercase().contains(&prefix_lower));

        for g in affected_games {
            self.invalidated_games.insert(g);
        }
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
    fn make_key(game: TargetGame, mods_path: &Path) -> CacheKey {
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

    /// 增量更新子树替换：将局部扫描结果合并/覆盖进已有缓存中。
    ///
    /// 当增量更新器检测到局部文件变更后，调用此方法将 `partial_result` 的
    /// groups 和 mods 合并到该游戏+路径的现有缓存条目中。
    ///
    /// # 合并策略
    ///
    /// - **groups**：就地合并，按 `group_path`（路径等价、大小写不敏感）原位替换；
    ///   不存在则追加。合并后按全量扫描一致的规则排序（NormalGroup 数值序 / MutexGroup 名称序）。
    /// - **mods**：扁平列表仅保留真实模组（跳过 None 空槽位），按 `full_path`（路径等价、
    ///   大小写不敏感）原位替换；partial 分组子树下未出现的旧 mod 视为删除（删除传播）。
    /// - 合并后按全量轻量扫描同口径重算 `total_mods_count` / `enabled_mods_count` / `disabled_mods_count`
    ///   （排除 None；enabled = `!disabled && !mod_disabled`）。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径。
    /// - `partial_result`: 局部扫描结果，仅包含变更的 groups 和 mods。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn subtree_replace(
        &mut self,
        game: TargetGame,
        mods_path: &Path,
        partial_result: ScanResult,
    ) {
        let key = Self::make_key(game, mods_path);
        let existing = match self.cache.get_mut(&key) {
            Some(e) => e,
            None => {
                // 缓存未命中时跳过：partial_result 仅包含变更的 groups/mods，
                // 直接创建缓存条目会导致残缺数据。下次 get_mods 会全量扫描填充完整缓存。
                log::debug!("[mod_cache] subtree_replace: cache miss for {:?}, skipping (will full-scan on next get_mods)", game);
                return;
            }
        };

        // 记录 partial 各分组子树根（用于删除传播；必须在消费 partial_result.groups 之前计算）
        let partial_roots: Vec<std::path::PathBuf> = partial_result
            .groups
            .iter()
            .map(|g| std::path::PathBuf::from(&g.full_path))
            .collect();

        // Arc::make_mut：缓存是 Arc 的唯一长期持有者，通常原地可变（零拷贝）；
        // 若调用方已通过 get() 克隆了该 Arc，则此处写时拷贝一份再改（语义安全）。
        let owned = Arc::make_mut(&mut existing.result);

        // 将现有 groups 就地合并：按 group_path（路径等价、大小写不敏感）原位替换，
        // 避免 BTreeMap 按字典序破坏分组数值序、或大小写路径差异产生重复分组。
        for pg in partial_result.groups {
            match owned
                .groups
                .iter()
                .position(|g| path_string_eq(&g.group_path, &pg.group_path))
            {
                Some(i) => owned.groups[i] = pg,
                None => owned.groups.push(pg),
            }
        }
        // 合并后按与全量轻量扫描一致的规则排序（NormalGroup 置顶数值序 / MutexGroup 名称序）
        owned.groups.sort_by(group_order_cmp);

        // 扁平 mods 就地合并：None 空槽位仅存于 groups[i].mods 内用于槽位展示，不进扁平列表；
        // 其余 mod 按 full_path（路径等价、大小写不敏感）原位替换，否则追加。
        let partial_mods: Vec<ModData> = partial_result
            .mods
            .into_iter()
            .filter(|m| m.name != "None" && m.mod_path != "None")
            .collect();
        // 删除传播：partial 扫描粒度到分组，partial 中出现的分组代表该子树当前完整状态，
        // 其子树下未出现在 partial 中的旧 mod 视为已被删除（文件/目录被移除）。
        for gp in &partial_roots {
            owned
                .mods
                .retain(|m| !Path::new(m.full_path.as_ref()).starts_with(gp));
        }
        // 合并/覆盖 partial 的真实模组
        for pm in partial_mods {
            match owned
                .mods
                .iter()
                .position(|m| path_string_eq(m.full_path.as_ref(), pm.full_path.as_ref()))
            {
                Some(i) => owned.mods[i] = pm,
                None => owned.mods.push(pm),
            }
        }

        // 重算计数（与全量轻量扫描同口径：排除 None；enabled = !disabled && !mod_disabled）
        let (total, enabled, disabled) = recount_mods(&owned.mods);
        owned.total_mods_count = total;
        owned.enabled_mods_count = enabled;
        owned.disabled_mods_count = disabled;

        log::debug!(
            "[mod_cache] subtree_replace 合并守卫 | groups={} mods={} total={} enabled={} disabled={}",
            owned.groups.len(),
            owned.mods.len(),
            owned.total_mods_count,
            owned.enabled_mods_count,
            owned.disabled_mods_count,
        );
    }
}

/// 路径字符串等价比较（Windows 下大小写不敏感，其他平台精确比较）。
///
/// 用于增量合并时按路径原位识别分组/模组，避免大小写路径差异产生重复条目。
/// 语义对齐 `mod_manager::path_eq_ignore_case`。
fn path_string_eq(a: &str, b: &str) -> bool {
    #[cfg(windows)]
    {
        a.to_lowercase() == b.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        a == b
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

/// Tauri IPC 命令：检查指定游戏的模组缓存是否有效。
///
/// 供前端通过 `checkModCacheValid` 调用，返回 `bool` 表示缓存是否可用。
///
/// # 参数
/// - `game`: 游戏名称的字符串，将调用 `parse_game` 解析为 `TargetGame`。
/// - `mods_path`: Mods 目录路径的字符串。
///
/// # 返回
///
/// - `true`: 缓存有效，可直接使用。
/// - `false`: 缓存已失效或不存在。
///
/// # 注意
///
/// 若 `parse_game` 解析失败（未知游戏名称），使用 `unwrap_or(TargetGame::GenshinImpact)` 作为兜底。
#[tauri::command]
pub fn check_mod_cache_valid(game: String, mods_path: String) -> bool {
    use crate::models::enums::TargetGame;
    let parsed = crate::hotkey::parse_game(&game).unwrap_or(TargetGame::GenshinImpact);
    let cache = MOD_CACHE.read();
    cache.is_valid(parsed, Path::new(&mods_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mod_scanner;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// 创建测试用的临时 Mods 目录结构：
    /// - `_MANAGED_/group_1/TestMod/mod.ini`
    /// - `d3dx.ini`
    fn setup_test_mods() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        // 创建一个group_1目录和一个mod
        let group1 = managed.join("group_1");
        fs::create_dir_all(&group1).unwrap();
        let mod_dir = group1.join("TestMod");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("mod.ini"), "[Section]\nx=1\n").unwrap();
        // 创建d3dx.ini
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        (dir, mods_path)
    }

    /// 测试 `set` 后能通过 `get` 正确获取缓存结果。
    #[test]
    fn test_cache_set_and_get() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());

        let cached = cache.get(TargetGame::GenshinImpact, &mods_path);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().total_mods_count, result.total_mods_count);
    }

    /// 测试缓存未命中：不存在的路径返回 `None`。
    #[test]
    fn test_cache_miss() {
        let cache = ModCache::new();
        let fake_path = PathBuf::from("/nonexistent/path");
        assert!(cache.get(TargetGame::GenshinImpact, &fake_path).is_none());
    }

    /// 测试精确失效：`invalidate` 后 `get` 返回 `None`。
    #[test]
    fn test_cache_invalidate() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result);
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_some());

        cache.invalidate(TargetGame::GenshinImpact, &mods_path);
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_none());
    }

    /// 测试按游戏失效：`invalidate_game` 只移除指定游戏的缓存，其他游戏不受影响。
    #[test]
    fn test_cache_invalidate_game() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());
        cache.set(TargetGame::Wuwa, &mods_path, result);

        cache.invalidate_game(TargetGame::GenshinImpact);
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_none());
        assert!(cache.get(TargetGame::Wuwa, &mods_path).is_some());
    }

    /// 测试全量清空：`invalidate_all` 后所有缓存条目消失。
    #[test]
    fn test_cache_invalidate_all() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());
        cache.set(TargetGame::Wuwa, &mods_path, result);

        cache.invalidate_all();
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_none());
        assert!(cache.get(TargetGame::Wuwa, &mods_path).is_none());
    }

    /// 测试不同游戏的缓存隔离：设置 GenshinImpact 后，Wuwa 查询不应命中。
    #[test]
    fn test_cache_different_games_isolated() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result);

        assert!(cache.get(TargetGame::Wuwa, &mods_path).is_none());
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_some());
    }

    /// 测试全局单例 `MOD_CACHE` 可正常访问且初始状态为空。
    #[test]
    fn test_global_cache_singleton() {
        // 测试全局单例可正常访问
        let cache = MOD_CACHE.read();
        assert!(cache
            .get(TargetGame::GenshinImpact, Path::new("/nonexistent"))
            .is_none());
    }

    /// 测试删除传播：局部扫描合并后，磁盘上已删除的模组应从缓存移除。
    ///
    /// 回归场景：用户删除 group_1 下某模组目录，文件监听器触发局部重扫 →
    /// `subtree_replace` 合并 partial 结果。原实现只增改不删，导致已删除模组
    /// 在 UI 中残留至全量重扫；修复后应即时消失。
    #[test]
    fn test_subtree_replace_propagates_deletion() {
        use crate::core::mod_scanner::scan_partial_path;
        let (dir, mods_path) = setup_test_mods();
        let group1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_gone = group1.join("ModGone");
        fs::create_dir_all(&mod_gone).unwrap();
        fs::write(mod_gone.join("mod.ini"), "[Section]\nx=1\n").unwrap();

        let mut cache = ModCache::new();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();
        cache.set(TargetGame::GenshinImpact, &mods_path, result);
        assert!(
            cache
                .get(TargetGame::GenshinImpact, &mods_path)
                .unwrap()
                .mods
                .iter()
                .any(|m| m.mod_name == "ModGone"),
            "前置条件：ModGone 应已进入缓存"
        );

        // 删除磁盘上的 ModGone 目录后，对 group_1 做局部扫描并合并
        fs::remove_dir_all(&mod_gone).unwrap();
        let partial = scan_partial_path(&mods_path, &group1).unwrap();
        cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        assert!(
            !updated.mods.iter().any(|m| m.mod_name == "ModGone"),
            "已删除的模组不应残留在缓存中"
        );
        assert!(
            updated.mods.iter().any(|m| m.mod_name == "TestMod"),
            "未删除的模组应保留"
        );
    }

    /// 测试：subtree_replace 合并后计数与扁平列表需与全量轻量扫描口径一致。
    ///
    /// 回归场景：原实现 groups 用 BTreeMap 按字典序合并破坏数值分组顺序、扁平 mods 含 None
    /// 空槽位、enabled 用 is_active 冒充 disabled 语义 → 计数/排序/扁平列表与全量扫描不一致。
    #[test]
    fn subtree_replace_matches_light_scan_semantics() {
        use crate::core::mod_scanner::scan_partial_path;
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(managed.join("group_2").join("EnabledMod")).unwrap();
        fs::write(
            managed.join("group_2").join("EnabledMod").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        fs::create_dir_all(managed.join("group_2").join("DISABLED_Off")).unwrap();
        fs::write(
            managed.join("group_2").join("DISABLED_Off").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        fs::create_dir_all(managed.join("group_10").join("ModB")).unwrap();
        fs::write(
            managed.join("group_10").join("ModB").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();

        let mut cache = ModCache::new();
        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        // 前置：全量扁平 mods 不含 None，计数口径（total/enabled/disabled）
        assert!(full.mods.iter().all(|m| m.name != "None"));
        assert_eq!(
            (
                full.total_mods_count,
                full.enabled_mods_count,
                full.disabled_mods_count
            ),
            (3, 2, 1)
        );
        cache.set(TargetGame::GenshinImpact, &mods_path, full);

        // 局部重扫 group_10 并合并
        let g10 = managed.join("group_10");
        let partial = scan_partial_path(&mods_path, &g10).unwrap();
        cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        // 计数与全量口径一致
        assert_eq!(
            (
                updated.total_mods_count,
                updated.enabled_mods_count,
                updated.disabled_mods_count
            ),
            (3, 2, 1)
        );
        // 扁平 mods 无 None，且仅为三个真实模组
        assert!(updated.mods.iter().all(|m| m.name != "None"));
        assert_eq!(updated.mods.len(), 3);
        // 分组顺序为数值序：group_2（索引 2）在 group_10（索引 10）前
        let idx: Vec<u32> = updated.groups.iter().map(|g| g.group_index).collect();
        assert_eq!(idx, vec![2, 10]);
    }

    /// 测试：subtree_replace 遇到仅大小写不同的 group_path 时原位替换，不产生重复分组。
    ///
    /// 回归场景：原实现按 group_path 字典序 key 去重，大小写路径差异被当作不同分组追加，
    /// 产生重复分组。修复后按路径等价（大小写不敏感）比较原位替换。
    #[test]
    fn subtree_replace_no_duplicate_group_on_case_diff() {
        use crate::core::mod_scanner::scan_partial_path;
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let g2 = dir.path().join("_MANAGED_").join("group_2");
        fs::create_dir_all(g2.join("ModB")).unwrap();
        fs::write(g2.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();

        let mut cache = ModCache::new();
        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        cache.set(TargetGame::GenshinImpact, &mods_path, full);

        // 构造仅大小写不同的 partial（group_path 大写化，模拟 Windows 大小写不敏感路径差异）
        let mut partial = scan_partial_path(&mods_path, &g2).unwrap();
        for g in partial.groups.iter_mut() {
            g.group_path = g.group_path.to_uppercase();
        }
        cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        // 不应因大小写差异产生重复分组
        assert_eq!(updated.groups.len(), 1, "大小写路径差异不应产生重复分组");
        // 原位替换后 group_path 保持大写形态（仅一处，未被追加为第二项）
        assert_eq!(
            updated.groups[0].group_path,
            updated.groups[0].group_path.to_uppercase()
        );
    }
}
