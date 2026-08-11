//! 模组数据缓存模块
//! 
//! 提供内存缓存功能，避免重复扫描文件系统。
//! 缓存键为 (TargetGame, 规范化路径字符串)，值为轻量扫描结果 ScanResult。
//! 使用 parking_lot::RwLock 保证线程安全的并发读写。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::Instant;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use crate::models::enums::TargetGame;
use crate::models::mod_data::{ModGroupData, ModData};
use crate::core::mod_scanner::ScanResult;

/// 缓存键类型，由目标游戏和规范化后的 Mods 目录路径字符串组成的二元组。
///
/// - `TargetGame`：区分不同游戏的缓存条目。
/// - `String`：`canonicalize` 规范化后的路径字符串，确保不同表示形式的同一路径命中同一缓存。
type CacheKey = (TargetGame, String);

/// 缓存条目，包含扫描结果和时间戳。
#[derive(Debug, Clone)]
struct CachedEntry {
    /// 轻量扫描结果（`ScanResult` 结构体）。
    result: ScanResult,
    /// 缓存插入时间戳，当前仅用于调试或未来实现 LRU 淘汰策略时使用。
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

    /// 查询缓存，命中返回克隆的 `ScanResult`。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径（将被规范化）。
    ///
    /// # 返回
    ///
    /// - `Some(ScanResult)`: 缓存命中，返回结果的克隆。
    /// - `None`: 缓存未命中。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn get(&self, game: TargetGame, mods_path: &Path) -> Option<ScanResult> {
        let key = Self::make_key(game, mods_path);
        let result = self.cache.get(&key).map(|entry| entry.result.clone());
        log::debug!("[core::mod_cache] [get] game={:?} hit={}", game, result.is_some());
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
        self.cache.insert(key, CachedEntry {
            result,
            _timestamp: Instant::now(),
        });
        // 清除该游戏的失效标记，表示缓存已更新
        self.invalidated_games.remove(&game);
        log::debug!("[core::mod_cache] [set] game={:?} mods={}", game, mods_count);
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
        log::debug!("[core::mod_cache] [invalidate] game={:?} removed={}", game, removed.is_some());
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
        let affected_games: HashSet<TargetGame> = self.cache
            .keys()
            .filter(|(_, path_str)| path_str.to_lowercase().contains(&prefix_lower))
            .map(|(g, _)| *g)
            .collect();

        self.cache.retain(|(_, path_str), _| {
            !path_str.to_lowercase().contains(&prefix_lower)
        });

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
        let normalized = mods_path.canonicalize()
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
    /// - **groups**：通过 `BTreeMap` 按 `group_path` 去重。
    ///   已存在相同 `groupPath` 的组则整体替换，不存在则追加。
    /// - **mods**：通过 `BTreeMap` 按 `mod_path` 去重。
    ///   已存在相同 `modPath` 的 mod 则替换，否则追加。
    /// - 合并后重算 `total_mods_count` / `enabled_mods_count` / `disabled_mods_count`。
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

        // 将现有 groups 转为 BTreeMap（按 group_path 去重）
        let mut existing_groups: BTreeMap<String, ModGroupData> = existing
            .result
            .groups
            .drain(..)
            .map(|g| (g.group_path.clone(), g))
            .collect();
        // 合并/覆盖 partial_result 的 groups
        for g in partial_result.groups {
            existing_groups.insert(g.group_path.clone(), g);
        }
        existing.result.groups = existing_groups.into_values().collect();

        // 将现有 mods 转为 BTreeMap（按 mod_path 去重）
        let mut existing_mods: BTreeMap<String, ModData> = existing
            .result
            .mods
            .drain(..)
            .map(|m| (m.mod_path.clone(), m))
            .collect();
        // 合并/覆盖 partial_result 的 mods
        for m in partial_result.mods {
            existing_mods.insert(m.mod_path.clone(), m);
        }
        existing.result.mods = existing_mods.into_values().collect();

        // 重算计数
        existing.result.total_mods_count = existing.result.mods.len();
        existing.result.enabled_mods_count =
            existing.result.mods.iter().filter(|m| m.is_active).count();
        existing.result.disabled_mods_count =
            existing.result.mods.iter().filter(|m| !m.is_active).count();
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
pub static MOD_CACHE: Lazy<RwLock<ModCache>> = Lazy::new(|| {
    RwLock::new(ModCache::new())
});

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
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use crate::core::mod_scanner;

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
        cache.set(TargetGame::Wuwa, &mods_path, result.clone());
        
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
        cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());
        
        assert!(cache.get(TargetGame::Wuwa, &mods_path).is_none());
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_some());
    }

    /// 测试全局单例 `MOD_CACHE` 可正常访问且初始状态为空。
    #[test]
    fn test_global_cache_singleton() {
        // 测试全局单例可正常访问
        let cache = MOD_CACHE.read();
        assert!(cache.get(TargetGame::GenshinImpact, Path::new("/nonexistent")).is_none());
    }
}