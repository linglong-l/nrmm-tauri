//! 模组数据缓存模块
//! 
//! 提供内存缓存功能，避免重复扫描文件系统。
//! 缓存键为 (TargetGame, 规范化路径字符串)，值为轻量扫描结果 ScanResult。
//! 使用 parking_lot::RwLock 保证线程安全的并发读写。

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use crate::models::enums::TargetGame;
use crate::core::mod_scanner::ScanResult;

/// 缓存键：(游戏, 规范化mods路径)
type CacheKey = (TargetGame, String);

/// 缓存条目，包含扫描结果和时间戳
#[derive(Debug, Clone)]
struct CachedEntry {
    /// 轻量扫描结果
    result: ScanResult,
    /// 缓存插入时间
    _timestamp: Instant,
}

/// 模组数据缓存管理器
/// 
/// 使用 RwLock 包裹 HashMap，支持多读单写。
/// 全局单例通过 MOD_CACHE 访问。
#[derive(Debug)]
pub struct ModCache {
    /// 内部缓存存储
    cache: HashMap<CacheKey, CachedEntry>,
}

impl ModCache {
    /// 创建新的空缓存实例
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// 查询缓存，命中返回克隆的 ScanResult
    /// 
    /// # 参数
    /// - `game`: 目标游戏
    /// - `mods_path`: 游戏Mods目录路径（将被规范化）
    /// 
    /// # 返回
    /// - `Some(ScanResult)`: 缓存命中
    /// - `None`: 缓存未命中
    pub fn get(&self, game: TargetGame, mods_path: &Path) -> Option<ScanResult> {
        let key = Self::make_key(game, mods_path);
        self.cache.get(&key).map(|entry| entry.result.clone())
    }

    /// 写入缓存
    /// 
    /// # 参数
    /// - `game`: 目标游戏
    /// - `mods_path`: 游戏Mods目录路径
    /// - `result`: 要缓存的轻量扫描结果
    pub fn set(&mut self, game: TargetGame, mods_path: &Path, result: ScanResult) {
        let key = Self::make_key(game, mods_path);
        self.cache.insert(key, CachedEntry {
            result,
            _timestamp: Instant::now(),
        });
    }

    /// 失效指定游戏+路径的缓存
    pub fn invalidate(&mut self, game: TargetGame, mods_path: &Path) {
        let key = Self::make_key(game, mods_path);
        self.cache.remove(&key);
    }

    /// 失效指定游戏的所有缓存
    pub fn invalidate_game(&mut self, game: TargetGame) {
        self.cache.retain(|(g, _), _| *g != game);
    }

    /// 按路径前缀批量失效缓存（用于文件监控）
    /// 
    /// 遍历所有缓存键，如果路径以给定managed_path前缀开头则移除。
    /// 用于文件监控检测到_MANAGED_目录变化时，失效对应游戏的缓存。
    /// 
    /// # 参数
    /// - `managed_path`: _MANAGED_目录的路径，将被规范化为字符串进行前缀匹配
    pub fn invalidate_by_prefix(&mut self, managed_path: &Path) {
        let prefix = match managed_path.canonicalize() {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => managed_path.to_string_lossy().to_string(),
        };
        let prefix_lower = prefix.to_lowercase();
        self.cache.retain(|(_, path_str), _| {
            !path_str.to_lowercase().contains(&prefix_lower)
        });
    }

    /// 清空所有缓存
    pub fn invalidate_all(&mut self) {
        self.cache.clear();
    }

    /// 生成缓存键（规范化路径）
    fn make_key(game: TargetGame, mods_path: &Path) -> CacheKey {
        let normalized = mods_path.canonicalize()
            .unwrap_or_else(|_| mods_path.to_path_buf())
            .to_string_lossy()
            .to_string();
        (game, normalized)
    }
}

impl Default for ModCache {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局模组缓存单例
/// 
/// 首次访问时自动初始化为空缓存。
/// 使用 parking_lot::RwLock 支持并发读和独占写。
pub static MOD_CACHE: Lazy<RwLock<ModCache>> = Lazy::new(|| {
    RwLock::new(ModCache::new())
});

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use crate::core::mod_scanner;

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

    #[test]
    fn test_cache_miss() {
        let cache = ModCache::new();
        let fake_path = PathBuf::from("/nonexistent/path");
        assert!(cache.get(TargetGame::GenshinImpact, &fake_path).is_none());
    }

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

    #[test]
    fn test_cache_different_games_isolated() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();
        
        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());
        
        assert!(cache.get(TargetGame::Wuwa, &mods_path).is_none());
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_some());
    }

    #[test]
    fn test_global_cache_singleton() {
        // 测试全局单例可正常访问
        let cache = MOD_CACHE.read();
        assert!(cache.get(TargetGame::GenshinImpact, Path::new("/nonexistent")).is_none());
    }
}
