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

/// Phase 4 不变量：同一物理 Mods 根的多种字符串形态必须折叠为同一个缓存键。
///
/// 规范化键被 `make_key` 记忆表加速后，必须与直算结果完全一致——否则同一游戏会以
/// 两个键同时存在于 `cache` 里，表现为「改了却没生效 / 反复全量重扫」。
/// 记忆表清空后重新直算，结论也必须不变。
#[test]
fn make_key_folds_equivalent_spellings_across_memo_states() {
    use std::path::PathBuf;

    let dir = tempfile::TempDir::new().unwrap();
    let mods = dir.path().join("Mods");
    std::fs::create_dir_all(&mods).unwrap();
    let game = crate::models::enums::TargetGame::GenshinImpact;

    // 尾分隔符与冗余 `.` 段在 canonicalize 后同形（跨宿主成立的两种形态）
    let forms = [
        mods.clone(),
        PathBuf::from(format!("{}/", mods.display())),
        mods.join("."),
    ];

    for clear_before in [false, true] {
        if clear_before {
            crate::core::mod_cache_core::clear_canonical_key_memo();
        }
        let base = ModCache::make_key(game, &forms[0]).1;
        for f in &forms[1..] {
            assert_eq!(
                ModCache::make_key(game, f).1,
                base,
                "形态 {:?} 未折叠到同一键（clear_before={clear_before}）",
                f
            );
        }
        // 同形两次取值必须稳定（记忆表命中路径与未命中路径一致）
        assert_eq!(ModCache::make_key(game, &forms[0]).1, base);
    }
    crate::core::mod_cache_core::clear_canonical_key_memo();
}
