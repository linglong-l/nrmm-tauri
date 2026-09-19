use super::*;

use std::fs;
use std::path::{Path, PathBuf};
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
    let result = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();

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
    let result = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();

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
    let result = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();

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
    let result = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();

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
    let result = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();

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
