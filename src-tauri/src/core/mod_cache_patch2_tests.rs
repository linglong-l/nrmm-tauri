use super::*;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

/// C-3b 测试辅助：group_1/{TestMod, ModB} fixture，返回分组目录路径。
fn setup_two_mod_group(dir: &tempfile::TempDir) -> PathBuf {
    let group1 = dir.path().join("_MANAGED_").join("group_1");
    fs::create_dir_all(group1.join("TestMod")).unwrap();
    fs::write(group1.join("TestMod").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    fs::create_dir_all(group1.join("ModB")).unwrap();
    fs::write(group1.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    group1
}

/// C-3b 测试辅助：取组内指定模组的 Arc 引用。
fn group_mod_arc<'a>(scan: &'a ScanResult, group_name: &str, mod_name: &str) -> &'a Arc<ModData> {
    scan.groups
        .iter()
        .find(|g| g.group_name == group_name)
        .unwrap_or_else(|| panic!("应存在分组 {group_name}"))
        .mods
        .iter()
        .find(|m| m.name == mod_name)
        .unwrap_or_else(|| panic!("组 {group_name} 应含模组 {mod_name}"))
}

/// C-3b 测试辅助：取扁平列表指定模组的 Arc 引用。
fn flat_mod_arc<'a>(scan: &'a ScanResult, mod_name: &str) -> &'a Arc<ModData> {
    scan.mods
        .iter()
        .find(|m| m.name == mod_name)
        .unwrap_or_else(|| panic!("扁平列表应含模组 {mod_name}"))
}

/// C-3a 测试辅助：构造互斥树 fixture `#M/{Sub1/ModX, ModW, Sub2/ModY}`，返回各目录。
fn setup_mutex_tree(dir: &tempfile::TempDir) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    let managed = dir.path().join("_MANAGED_");
    let root = managed.join("#M");
    let sub1 = root.join("Sub1");
    let sub2 = root.join("Sub2");
    fs::create_dir_all(sub1.join("ModX")).unwrap();
    fs::write(sub1.join("ModX").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    fs::create_dir_all(root.join("ModW")).unwrap();
    fs::write(root.join("ModW").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    fs::create_dir_all(sub2.join("ModY")).unwrap();
    fs::write(sub2.join("ModY").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    let mod_x = sub1.join("ModX");
    (managed, root, sub1, sub2, mod_x)
}

/// patch_mod_entry：目录重命名（禁用）后组树 + 扁平列表路径与禁用标志同步更新，零扫描。
#[test]
fn test_patch_mod_entry_updates_path_and_disabled() {
    let (_dir, mods_path) = setup_test_mods();
    let result = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    let old_full = result
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .and_then(|g| g.mods.iter().find(|m| m.mod_name == "TestMod"))
        .map(|m| Path::new(m.full_path.as_ref()).to_path_buf())
        .expect("存在 TestMod 模组");

    let new_full = old_full
        .parent()
        .expect("模组有父目录")
        .join("DISABLEDTestMod");

    let mut cache = ModCache::new();
    cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());
    assert!(cache.patch_mod_entry(
        TargetGame::GenshinImpact,
        &mods_path,
        &old_full,
        &new_full,
        true
    ));

    let cached = cache
        .get(TargetGame::GenshinImpact, &mods_path)
        .expect("缓存存在");
    // 组树内：路径更新 + 禁用标志 + 名称不变
    let entry = cached
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .and_then(|g| g.mods.iter().find(|m| m.mod_name == "TestMod"))
        .expect("组内仍存在该模组");
    assert!(
        path_equivalent(Path::new(entry.full_path.as_ref()), &new_full),
        "组内 full_path 应更新为 DISABLED 路径"
    );
    assert!(entry.mod_disabled, "mod_disabled 应置 true");
    assert!(
        entry.disabled,
        "兼容字段 disabled 应与 mod_disabled 同步置 true（防字段分叉）"
    );
    assert_eq!(entry.mod_name, "TestMod", "mod_name 应保持不变");
    // 扁平列表同步（共享 Arc，路径一致）
    assert!(
        cached
            .mods
            .iter()
            .any(|m| path_equivalent(Path::new(m.full_path.as_ref()), &new_full)),
        "扁平列表应同步更新路径"
    );
    let flat = cached
        .mods
        .iter()
        .find(|m| path_equivalent(Path::new(m.full_path.as_ref()), &new_full))
        .expect("扁平列表含 DISABLED 路径条目");
    assert!(
        flat.disabled,
        "扁平条目 disabled 字段同步声明（与组内共享同一 Arc）"
    );
    // 计数重算：禁用 1，启用 0
    assert_eq!(cached.total_mods_count, 1);
    assert_eq!(cached.enabled_mods_count, 0);
    assert_eq!(cached.disabled_mods_count, 1);

    // 再次 patch（启用还原）同样命中
    assert!(cache.patch_mod_entry(
        TargetGame::GenshinImpact,
        &mods_path,
        &new_full,
        &old_full,
        false
    ));
    let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let entry = cached
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .and_then(|g| g.mods.iter().find(|m| m.mod_name == "TestMod"))
        .unwrap();
    assert!(!entry.mod_disabled, "再次 patch 后应恢复启用");
    assert!(!entry.disabled, "兼容字段 disabled 应同步恢复启用态");
    assert_eq!(cached.enabled_mods_count, 1);
}

/// patch_mod_entry：命中不存在的模组路径返回 false（调用方据此降级重扫）。
#[test]
fn test_patch_mod_entry_miss_returns_false() {
    let (_dir, mods_path) = setup_test_mods();
    let result = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    let mut cache = ModCache::new();
    cache.set(TargetGame::GenshinImpact, &mods_path, result);

    let miss = mods_path.join("_MANAGED_/group_1/NoSuchMod");
    let new = mods_path.join("_MANAGED_/group_1/DISABLEDNoSuchMod");
    assert!(!cache.patch_mod_entry(TargetGame::GenshinImpact, &mods_path, &miss, &new, true));

    // 缓存未命中（无该游戏条目）也返回 false
    assert!(!cache.patch_mod_entry(
        TargetGame::GenshinImpact,
        Path::new("/no/mods"),
        &miss,
        &new,
        true
    ));
}

/// C-3a：替换——位置匹配替换、mod_index 保留旧值、兄弟 Arc 复用、
/// 扁平列表同步（共享同一实例）、组级派生与计数重算。
#[test]
fn patch_single_mod_replaces_preserving_mod_index_and_siblings() {
    use crate::core::mod_scan_light::scan_single_mod;
    let (dir, mods_path) = setup_test_mods();
    let group1 = setup_two_mod_group(&dir);

    let mut cache = ModCache::new();
    cache.set(
        TargetGame::GenshinImpact,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );
    // 旧实例快照（持有共享 Arc → patch 的 make_mut 走写时拷贝路径，指针断言有效）
    let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let old_test = group_mod_arc(&old, "group_1", "TestMod");
    let old_index = old_test.mod_index;

    // 磁盘变化：TestMod 加 fav（深部变更场景）
    fs::write(group1.join("TestMod").join("fav"), "").unwrap();
    let (arc, _) = scan_single_mod(&mods_path, &group1.join("TestMod")).unwrap();
    assert!(arc.is_favorite, "前置：新扫描应反映 fav 状态");

    let patched = cache.patch_single_mod(
        TargetGame::GenshinImpact,
        &mods_path,
        &group1.join("TestMod"),
        Some(arc),
    );
    let group_node = patched.expect("patch 应返回受影响组节点");
    assert_eq!(group_node.group_name, "group_1");

    let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let g1 = updated
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .unwrap();
    // 位置不变 + mod_index 保留 + 新状态生效
    assert_eq!(g1.mods.len(), 3, "None + TestMod + ModB");
    let pos = g1
        .mods
        .iter()
        .position(|m| m.name == "TestMod")
        .expect("TestMod 应在组内");
    assert_eq!(
        g1.mods[pos].mod_index, old_index,
        "替换时 mod_index 保留旧实例值"
    );
    assert!(g1.mods[pos].is_favorite, "组内新实例应反映 fav");
    assert!(!Arc::ptr_eq(&g1.mods[pos], old_test), "被替换条目为新实例");
    // 兄弟 Arc 复用（零重扫零深拷贝）
    assert!(
        Arc::ptr_eq(
            group_mod_arc(&updated, "group_1", "ModB"),
            group_mod_arc(&old, "group_1", "ModB")
        ),
        "未变兄弟 ModB 组内条目 Arc 复用"
    );
    // 扁平列表同步：同位置替换且与组内共享同一 Arc 实例
    let flat_test = flat_mod_arc(&updated, "TestMod");
    assert!(
        Arc::ptr_eq(flat_test, &g1.mods[pos]),
        "扁平条目与组内条目共享同一 Arc"
    );
    assert!(flat_test.is_favorite);
    assert_eq!(updated.mods.len(), old.mods.len(), "扁平列表长度不变");
    assert_eq!(updated.total_mods_count, 2);
}

/// C-3a：新增——无匹配条目时追加组内末位、mod_index 为末位、计数/扁平同步。
#[test]
fn patch_single_mod_inserts_at_end() {
    use crate::core::mod_scan_light::scan_single_mod;
    let (dir, mods_path) = setup_test_mods();
    let group1 = setup_two_mod_group(&dir);

    let mut cache = ModCache::new();
    cache.set(
        TargetGame::GenshinImpact,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );

    // 磁盘新增 ModC
    let mod_c = group1.join("ModC");
    fs::create_dir_all(&mod_c).unwrap();
    fs::write(mod_c.join("mod.ini"), "[Section]\nx=1\n").unwrap();
    let (arc, _) = scan_single_mod(&mods_path, &mod_c).unwrap();

    let group_node = cache
        .patch_single_mod(TargetGame::GenshinImpact, &mods_path, &mod_c, Some(arc))
        .expect("新增 patch 应成功");

    assert_eq!(
        group_node.mod_count, 3,
        "mod_count 重算（TestMod + ModB + ModC）"
    );
    let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let g1 = updated
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .unwrap();
    assert_eq!(g1.mods.len(), 4, "None + TestMod + ModB + ModC");
    let last = g1.mods.last().expect("组内应有条目");
    assert_eq!(
        last.name, "ModC",
        "新增模组置于组内末位（排序位由下次组级扫描校正）"
    );
    assert_eq!(last.mod_index, 3, "新增条目 mod_index 为组内末位");
    assert_eq!(updated.total_mods_count, 3, "扁平计数重算");
    assert!(
        updated.mods.iter().any(|m| m.name == "ModC"),
        "扁平列表应含新增模组"
    );
}

/// C-3a：删除——组内 + 扁平移除、计数重算、兄弟条目不动。
#[test]
fn patch_single_mod_removes_entry() {
    let (dir, mods_path) = setup_test_mods();
    let group1 = setup_two_mod_group(&dir);

    let mut cache = ModCache::new();
    cache.set(
        TargetGame::GenshinImpact,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );
    let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let old_test = group_mod_arc(&old, "group_1", "TestMod");

    let group_node = cache
        .patch_single_mod(
            TargetGame::GenshinImpact,
            &mods_path,
            &group1.join("ModB"),
            None,
        )
        .expect("删除 patch 应成功");
    assert_eq!(group_node.mod_count, 1, "mod_count 重算");

    let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let g1 = updated
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .unwrap();
    assert!(g1.mods.iter().all(|m| m.name != "ModB"), "组内应移除 ModB");
    assert!(
        updated.mods.iter().all(|m| m.name != "ModB"),
        "扁平列表应移除 ModB"
    );
    assert_eq!(updated.total_mods_count, 1, "计数重算");
    assert_eq!(updated.mods.len(), 1, "扁平列表长度同步");
    // 兄弟条目不动（Arc 复用）
    assert!(
        Arc::ptr_eq(group_mod_arc(&updated, "group_1", "TestMod"), old_test),
        "删除的兄弟条目（TestMod）Arc 复用不动"
    );
}

/// C-3a：深层 mutex 树定位——2 层嵌套下 patch 链上节点重建、链外兄弟子树
/// Arc 指针不变（make_mut 语义验证）。
#[test]
fn patch_single_mod_deep_mutex_chain_rebuild_and_sibling_reuse() {
    use crate::core::mod_scan_light::scan_single_mod;
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    let (_managed, _root, _sub1, _sub2, mod_x) = setup_mutex_tree(&dir);

    let mut cache = ModCache::new();
    cache.set(
        TargetGame::GenshinImpact,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );
    let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let old_mutex = old
        .groups
        .iter()
        .find(|g| g.group_name == "#M")
        .expect("应含互斥组根节点");
    let old_sub2 = old_mutex
        .children
        .iter()
        .find(|c| c.group_name == "Sub2")
        .expect("应含 Sub2 子分组");
    let old_mod_w = old_mutex
        .mods
        .iter()
        .find(|m| m.name == "ModW")
        .expect("根下应含 ModW");

    // 磁盘变化：ModX 加 fav
    fs::write(mod_x.join("fav"), "").unwrap();
    let (arc, _) = scan_single_mod(&mods_path, &mod_x).unwrap();

    let group_node = cache
        .patch_single_mod(TargetGame::GenshinImpact, &mods_path, &mod_x, Some(arc))
        .expect("深层 patch 应成功");
    assert_eq!(group_node.group_name, "#M", "返回顶层组节点（delta 形状）");

    // 结构断言辅助：定位子分组（嵌套 fn 规避闭包生命周期推断）
    fn find_child<'a>(node: &'a ModGroupData, name: &str) -> &'a Arc<ModGroupData> {
        node.children
            .iter()
            .find(|c| c.group_name == name)
            .unwrap_or_else(|| panic!("应含子分组 {name}"))
    }
    let new_sub1 = find_child(&group_node, "Sub1");
    let new_sub2 = find_child(&group_node, "Sub2");
    let patched_mod_x = new_sub1
        .mods
        .iter()
        .find(|m| m.name == "ModX")
        .expect("Sub1 内应含 ModX");
    assert!(patched_mod_x.is_favorite, "深部 patch 生效（fav 反映）");

    // 缓存内验证：链外兄弟 Sub2 子树 Arc 指针不变（复用）；根下 ModW 条目复用
    let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let new_mutex = updated
        .groups
        .iter()
        .find(|g| g.group_name == "#M")
        .unwrap();
    let cached_sub2 = new_mutex
        .children
        .iter()
        .find(|c| c.group_name == "Sub2")
        .expect("缓存应保留 Sub2");
    assert!(
        Arc::ptr_eq(cached_sub2, old_sub2),
        "链外兄弟子树（Sub2）Arc 指针不变——make_mut 仅重建定位链"
    );
    let cached_mod_w = new_mutex
        .mods
        .iter()
        .find(|m| m.name == "ModW")
        .expect("根下应含 ModW");
    assert!(
        Arc::ptr_eq(cached_mod_w, old_mod_w),
        "同组内未 patch 的兄弟条目（ModW）Arc 复用"
    );
    // 扁平同步
    assert!(
        updated
            .mods
            .iter()
            .any(|m| m.name == "ModX" && m.is_favorite),
        "扁平列表同步深层 patch"
    );
    assert_eq!(updated.total_mods_count, 3);
    assert_eq!(new_sub2.mods.len(), 1, "Sub2 子树内容不变");
}

/// C-3a：缓存 miss / 无定位（路径实为子分组节点）/ 删除无匹配 → None。
#[test]
fn patch_single_mod_miss_and_no_match_return_none() {
    use crate::core::mod_scan_light::scan_single_mod;
    // 缓存 miss
    let mut empty_cache = ModCache::new();
    let fake = PathBuf::from("/nonexistent");
    assert!(empty_cache
        .patch_single_mod(TargetGame::GenshinImpact, &fake, &fake.join("x"), None)
        .is_none());

    // 有缓存但目标为子分组节点自身（非模组目录）→ 无定位
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    let (_managed, root, _sub1, _sub2, _mod_x) = setup_mutex_tree(&dir);
    let mut cache = ModCache::new();
    cache.set(
        TargetGame::GenshinImpact,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );
    assert!(
        cache
            .patch_single_mod(
                TargetGame::GenshinImpact,
                &mods_path,
                &root.join("Sub1"),
                None
            )
            .is_none(),
        "子分组节点路径（删除语义）在组内 mods 无匹配 → None 降级"
    );

    // 删除缓存中不存在的模组路径 → 无匹配
    assert!(cache
        .patch_single_mod(
            TargetGame::GenshinImpact,
            &mods_path,
            &root.join("Sub1").join("Ghost"),
            None
        )
        .is_none());

    // 合法模组 patch 不受影响（对照）
    let (_arc, _) = scan_single_mod(&mods_path, &root.join("Sub1").join("ModX")).unwrap();
    assert!(cache
        .patch_single_mod(
            TargetGame::GenshinImpact,
            &mods_path,
            &root.join("Sub1").join("ModX"),
            Some(_arc)
        )
        .is_some());
}

/// patch_selected_index：两启用模组（多启用回退 selectedindex）场景下，
/// patch 结果与「落盘 selectedindex 后全量轻扫」完全一致（组级字段 +
/// 组内 is_active + 扁平列表 is_active），幂等重复写收敛。
#[test]
fn patch_selected_index_matches_light_scan_semantics() {
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join("_MANAGED_");
    fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    let group1 = managed.join("group_1");
    fs::create_dir_all(group1.join("ModA")).unwrap();
    fs::write(group1.join("ModA").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    fs::create_dir_all(group1.join("ModB")).unwrap();
    fs::write(group1.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();

    let mut cache = ModCache::new();
    let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    cache.set(TargetGame::GenshinImpact, &mods_path, full);
    // 前置：首扫 selectedindex=0（多启用回退）→ 无模组激活
    {
        let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let g = cached
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .unwrap();
        assert!(!g.mods.iter().any(|m| m.name != "None" && m.is_active));
    }

    // patch selectedindex=1（组内排序 [None, ModA, ModB] → 选中 ModA）
    assert!(cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 1));

    // 与全量轻扫等价性校验：落盘同一 selectedindex 后重扫对比
    fs::write(group1.join("selectedindex"), "1").unwrap();
    let rescan = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    let rescan_g = rescan
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .unwrap();

    let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let g = cached
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .unwrap();
    assert_eq!(g.active_mod_index, rescan_g.active_mod_index);
    assert_eq!(g.is_active, rescan_g.is_active);
    for (cm, rm) in g.mods.iter().zip(rescan_g.mods.iter()) {
        assert_eq!(
            cm.is_active, rm.is_active,
            "组内 {} is_active 应与重扫一致",
            cm.name
        );
    }
    assert_eq!(g.active_mod_index, 1, "selectedindex=1 → ModA 激活");
    assert!(g.mods.iter().find(|m| m.name == "ModA").unwrap().is_active);
    assert!(!g.mods.iter().find(|m| m.name == "ModB").unwrap().is_active);
    // 扁平列表同步
    let flat_a = flat_mod_arc(&cached, "ModA");
    assert!(flat_a.is_active, "扁平列表 ModA is_active 应同步");
    assert!(!flat_mod_arc(&cached, "ModB").is_active);
    // 释放对缓存 Arc 的外部持有（否则下一次 patch 触发快路径守卫返回 false）
    drop(cached);

    // 幂等重复写：切到 2 → ModB 激活、ModA 失活，再次收敛一致
    assert!(cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 2));
    let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let g = cached
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .unwrap();
    assert_eq!(g.active_mod_index, 2);
    assert!(!g.mods.iter().find(|m| m.name == "ModA").unwrap().is_active);
    assert!(g.mods.iter().find(|m| m.name == "ModB").unwrap().is_active);
    assert!(flat_mod_arc(&cached, "ModB").is_active);
}

/// patch_selected_index：单启用模组场景（派生口径=该启用模组激活，
/// selectedindex 不改变激活位——与轻量扫描一致）。
#[test]
fn patch_selected_index_single_enabled_keeps_scan_semantics() {
    let (_dir, mods_path) = setup_test_mods();
    // setup_test_mods：group_1/TestMod 单启用模组
    let group1 = mods_path.join("_MANAGED_").join("group_1");

    let mut cache = ModCache::new();
    cache.set(
        TargetGame::GenshinImpact,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );

    // 单启用 → 无论 selectedindex 写何值，激活位恒为该启用模组（索引 1）
    assert!(cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 1));
    let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let g = cached
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .unwrap();
    assert_eq!(g.active_mod_index, 1);
    assert!(
        g.mods
            .iter()
            .find(|m| m.mod_name == "TestMod")
            .unwrap()
            .is_active
    );
}

/// patch_selected_index：缓存未命中 / 分组不在缓存 → false（不 panic、不失效）。
#[test]
fn patch_selected_index_miss_returns_false() {
    let (_dir, mods_path) = setup_test_mods();
    let group1 = mods_path.join("_MANAGED_").join("group_1");

    // 缓存未命中（空缓存）
    let mut cache = ModCache::new();
    assert!(!cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 1));

    // 有缓存但分组不在缓存（group_2 不存在）
    cache.set(
        TargetGame::GenshinImpact,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );
    assert!(!cache.patch_selected_index(
        TargetGame::GenshinImpact,
        &mods_path,
        &mods_path.join("_MANAGED_").join("group_2"),
        1
    ));
}

/// patch_selected_index：缓存 Arc 被外部持有（strong_count>1）时不走整树 COW，
/// 直接降级返回 false（由 watcher 增量兜底）；释放外部引用后恢复快路径生效。
///
/// 回归场景（2026-09-06）：get_mods 序列化在途 clone 缓存 Arc 时，`Arc::make_mut`
/// 深拷贝整棵万级模组树（实测数十秒），选择命令整体 24s+。快路径守卫阻止该行为。
#[test]
fn patch_selected_index_skips_cow_when_arc_externally_held() {
    let (_dir, mods_path) = setup_test_mods();
    let group1 = mods_path.join("_MANAGED_").join("group_1");

    let mut cache = ModCache::new();
    cache.set(
        TargetGame::GenshinImpact,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );

    // 模拟外部持有：get() 克隆 Arc 并保活（strong_count>1）
    {
        let _held = cache
            .get(TargetGame::GenshinImpact, &mods_path)
            .expect("缓存存在");
        // 外部 Arc 存活期间 patch → 拒绝深拷贝，返回 false
        assert!(!cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 1));
    }
    // 外部引用释放后恢复快路径
    assert!(cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 1));
    let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let g = cached
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .unwrap();
    assert_eq!(g.active_mod_index, 1, "快路径恢复后派生生效");
}
