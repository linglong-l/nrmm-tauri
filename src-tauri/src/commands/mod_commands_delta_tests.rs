//! 命令级 Delta 组装（R1）单元测试（由 mod_commands.rs 以 `#[path] mod delta_tests;` 挂载）
//!
//! 承载 toggle / remove_group / rename_mutex 三个场景的局部重扫 +
//! subtree_replace/subtree_remove 合并测试。迁移自 mod_commands.rs
//! （2026-09-19 Phase 6-3），被调用的共享辅助
//! `build_group_delta` / `build_removed_group_delta` 已迁至
//! mod_commands_util.rs，此处经完整路径引用。

use crate::commands::mod_commands_util::{build_group_delta, build_removed_group_delta};
use crate::core::constants;
use crate::models::enums::TargetGame;
use std::fs;

// =====================================================================
// 命令级 Delta 组装（R1）：局部重扫 + subtree_replace/subtree_remove 合并
//
// 注意：以下测试直接调用模块内组装函数（不经过 Tauri 命令层），
// 使用全局 MOD_CACHE（各测试临时目录路径唯一，缓存键互不冲突）；
// 故意不在结束时 invalidate_all——并行测试下清空全局缓存会破坏
// 其他正在进行的 delta 测试的前置数据。全局缓存写者（含 file_watcher
// R2 测试）共 4 个 > 容量上限 3，须持 CACHE_WRITER_TEST_LOCK 串行，
// 防 set() 的 LRU 淘汰在他测窗口内淘汰其条目（详见锁文档）。
// =====================================================================

/// 冒烟测试：toggle 禁用后 delta.new_group 与磁盘一致、old_mod_paths 正确、缓存已合并。
#[test]
fn toggle_group_delta_assembly_smoke() {
    use crate::core::{mod_cache_core, mod_cache_delta, mod_cache_path};

    use tempfile::TempDir;

    // 写全局 MOD_CACHE 的测试串行（容量 3 + 4 个写者，防 LRU 跨测试淘汰，
    // 见 mod_cache_core::CACHE_WRITER_TEST_LOCK 文档）
    let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join(constants::MANAGED_FOLDER);
    fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    let group1 = managed.join("group_1");
    fs::create_dir_all(group1.join("ModA")).unwrap();
    fs::write(group1.join("ModA").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    fs::create_dir_all(group1.join("ModB")).unwrap();
    fs::write(group1.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    // 另一组不应受影响
    fs::create_dir_all(managed.join("group_2").join("ModC")).unwrap();
    fs::write(
        managed.join("group_2").join("ModC").join("mod.ini"),
        "[Section]\nx=1\n",
    )
    .unwrap();

    // 预填充缓存（模拟 get_mods 已加载该游戏）
    let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    mod_cache_core::MOD_CACHE
        .write()
        .set(TargetGame::GenshinImpact, &mods_path, full);

    // 执行禁用（目录重命名：ModA → DISABLED 前缀形态）
    let mod_a = group1.join("ModA");
    crate::core::mod_toggle::toggle_mod(&mod_a, false).expect("toggle_mod 失败");

    // delta 组装：toggle 场景对 parent（分组目录）局部重扫
    let delta = build_group_delta(
        TargetGame::GenshinImpact,
        &mods_path,
        &group1,
        mod_a.parent().expect("mod 目录应有 parent"),
    )
    .expect("delta 组装失败");

    // old_group_path 为缓存中该组的 group_path（前端树定位键）
    assert_eq!(
        delta.old_group_path,
        group1.to_string_lossy().to_string(),
        "old_group_path 应为缓存内该组的分组路径"
    );
    // old_mod_paths：磁盘操作前该组全部真实模组（排除 None 槽位）
    assert_eq!(delta.old_mod_paths.len(), 2, "该组应有 2 个旧模组条目");
    assert!(
        delta.old_mod_paths.iter().any(|p| p.ends_with("ModA")),
        "old_mod_paths 应含旧 ModA 路径"
    );
    assert!(
        delta.old_mod_paths.iter().any(|p| p.ends_with("ModB")),
        "old_mod_paths 应含 ModB 路径"
    );

    // new_group 与磁盘一致：含禁用后的 ModA 与未受影响的 ModB
    let new_group = delta.new_group.as_ref().expect("new_group 不应为 None");
    assert_eq!(new_group.group_name, "group_1");
    assert_eq!(
        mod_cache_delta::collect_group_mod_paths(new_group).len(),
        2,
        "新分组应含 2 个真实模组"
    );
    let disabled = new_group
        .mods
        .iter()
        .find(|m| m.mod_name.ends_with("ModA"))
        .expect("新分组应含重命名后的 ModA");
    assert!(
        disabled.mod_disabled && disabled.disabled,
        "禁用后的模组应带 disabled 标记"
    );
    assert!(
        new_group.mods.iter().any(|m| m.mod_name == "ModB"),
        "未受影响的 ModB 应保留"
    );

    // 缓存已合并：group_1 节点反映禁用后状态，扁平列表剔除旧路径、计数重算
    let cached = mod_cache_core::MOD_CACHE
        .read()
        .get(TargetGame::GenshinImpact, &mods_path)
        .expect("缓存应命中");
    let g1 = mod_cache_path::find_group_node(&cached, &group1).expect("缓存应含 group_1");
    assert!(
        g1.mods
            .iter()
            .any(|m| m.mod_name.ends_with("ModA") && m.mod_disabled),
        "缓存内 group_1 应已反映禁用状态"
    );
    assert!(
        !cached
            .mods
            .iter()
            .any(|m| m.mod_path.ends_with("\\ModA") || m.mod_path.ends_with("/ModA")),
        "缓存扁平列表不应残留旧启用态 ModA 路径（应替换为 DISABLED 前缀路径）"
    );
    assert!(
        cached.mods.iter().any(|m| m.mod_name == "ModB"),
        "缓存扁平列表应保留未受影响的 ModB（删除传播后由 partial 重新合并）"
    );
    assert!(
        cached
            .mods
            .iter()
            .any(|m| m.mod_disabled && m.mod_path.contains("DISABLED")),
        "缓存扁平列表应含禁用后的 ModA 新条目"
    );
    assert_eq!(
        (
            cached.total_mods_count,
            cached.enabled_mods_count,
            cached.disabled_mods_count
        ),
        (3, 2, 1),
        "合并后计数应重算：3 总 / 2 启用（ModB+ModC）/ 1 禁用"
    );
    // group_2 未受影响
    assert!(
        cached.mods.iter().any(|m| m.mod_name == "ModC"),
        "未受影响分组的模组应保留"
    );
    println!("[PASS] toggle_group_delta_assembly_smoke");
}

/// 冒烟测试：remove_group 场景 delta（new_group=None，缓存子树被裁剪）。
#[test]
fn remove_group_delta_clears_cache_subtree() {
    use crate::core::{mod_cache_core, mod_cache_path};

    use tempfile::TempDir;

    // 写全局 MOD_CACHE 的测试串行（容量 3 + 4 个写者，防 LRU 跨测试淘汰，
    // 见 mod_cache_core::CACHE_WRITER_TEST_LOCK 文档）
    let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join(constants::MANAGED_FOLDER);
    fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    let group1 = managed.join("group_1");
    fs::create_dir_all(group1.join("ModA")).unwrap();
    fs::write(group1.join("ModA").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    fs::create_dir_all(group1.join("ModB")).unwrap();
    fs::write(group1.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    fs::create_dir_all(managed.join("group_2").join("ModC")).unwrap();
    fs::write(
        managed.join("group_2").join("ModC").join("mod.ini"),
        "[Section]\nx=1\n",
    )
    .unwrap();

    let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    mod_cache_core::MOD_CACHE
        .write()
        .set(TargetGame::GenshinImpact, &mods_path, full);

    // 磁盘删除 group_1（模拟 trash_delete 的效果）
    fs::remove_dir_all(&group1).unwrap();

    let delta = build_removed_group_delta(TargetGame::GenshinImpact, &mods_path, &group1);

    assert!(
        delta.new_group.is_none(),
        "remove_group 场景 new_group 应为 None"
    );
    assert_eq!(delta.old_group_path, group1.to_string_lossy().to_string());
    assert_eq!(
        delta.old_mod_paths.len(),
        2,
        "应收集被移除组的 2 个模组路径"
    );

    let cached = mod_cache_core::MOD_CACHE
        .read()
        .get(TargetGame::GenshinImpact, &mods_path)
        .expect("缓存应命中");
    assert!(
        mod_cache_path::find_group_node(&cached, &group1).is_none(),
        "缓存中 group_1 节点应被移除"
    );
    assert!(
        cached
            .mods
            .iter()
            .all(|m| m.mod_name != "ModA" && m.mod_name != "ModB"),
        "缓存扁平列表不应残留 group_1 模组"
    );
    assert!(
        cached.mods.iter().any(|m| m.mod_name == "ModC"),
        "其他分组应保留"
    );
    assert_eq!(
        (
            cached.total_mods_count,
            cached.enabled_mods_count,
            cached.disabled_mods_count
        ),
        (1, 1, 0),
        "裁剪后计数应重算"
    );
    println!("[PASS] remove_group_delta_clears_cache_subtree");
}

/// 冒烟测试：互斥组模组重命名场景，以重命名后的新目录局部重扫。
#[test]
fn rename_mutex_mod_delta_uses_new_path() {
    use crate::core::{mod_cache_core, mod_cache_delta};

    use tempfile::TempDir;

    // 写全局 MOD_CACHE 的测试串行（容量 3 + 4 个写者，防 LRU 跨测试淘汰，
    // 见 mod_cache_core::CACHE_WRITER_TEST_LOCK 文档）
    let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join(constants::MANAGED_FOLDER);
    fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    let mutex_root = managed.join("#Mutex");
    fs::create_dir_all(mutex_root.join("ModY")).unwrap();
    fs::write(mutex_root.join("ModY").join("mod.ini"), "[Section]\nx=1\n").unwrap();

    let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    mod_cache_core::MOD_CACHE
        .write()
        .set(TargetGame::GenshinImpact, &mods_path, full);

    // 重命名 mod 目录（mutex 分支：目录重命名）
    let old_path = mutex_root.join("ModY");
    let new_path = mutex_root.join("NewY");
    fs::rename(&old_path, &new_path).expect("重命名失败");

    let delta = build_group_delta(
        TargetGame::GenshinImpact,
        &mods_path,
        &mutex_root,
        &new_path,
    )
    .expect("delta 组装失败");

    // new_group 为 #Mutex 整组（局部重扫自动扩展到所属分组），含新目录模组
    let new_group = delta.new_group.as_ref().expect("new_group 不应为 None");
    assert_eq!(new_group.group_name, "#Mutex");
    let new_paths = mod_cache_delta::collect_group_mod_paths(new_group);
    assert!(
        new_paths.iter().any(|p| p.ends_with("NewY")),
        "新分组应含重命名后的 NewY"
    );
    assert!(
        !new_paths.iter().any(|p| p.ends_with("ModY")),
        "新分组不应残留旧 ModY"
    );
    // old_mod_paths 含旧路径（供前端剔除）
    assert!(
        delta.old_mod_paths.iter().any(|p| p.ends_with("ModY")),
        "old_mod_paths 应含旧 ModY 路径"
    );
    // 缓存合并后同样无旧条目
    let cached = mod_cache_core::MOD_CACHE
        .read()
        .get(TargetGame::GenshinImpact, &mods_path)
        .expect("缓存应命中");
    assert!(
        cached.mods.iter().all(|m| m.mod_name != "ModY"),
        "缓存扁平列表不应残留旧 ModY"
    );
    assert!(
        cached.mods.iter().any(|m| m.mod_name == "NewY"),
        "缓存扁平列表应含 NewY"
    );
    println!("[PASS] rename_mutex_mod_delta_uses_new_path");
}
