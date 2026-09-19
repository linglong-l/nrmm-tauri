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

/// remove_mod_entry：组树 + 扁平列表同步剔除并重算计数；二次移除返回 false。
#[test]
fn test_remove_mod_entry_removes_from_tree_and_flat() {
    let (_dir, mods_path) = setup_test_mods();
    let result = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    let full = result
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .and_then(|g| g.mods.iter().find(|m| m.mod_name == "TestMod"))
        .map(|m| Path::new(m.full_path.as_ref()).to_path_buf())
        .expect("存在 TestMod 模组");

    let mut cache = ModCache::new();
    cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());
    assert!(cache.remove_mod_entry(TargetGame::GenshinImpact, &mods_path, &full));

    let cached = cache
        .get(TargetGame::GenshinImpact, &mods_path)
        .expect("缓存存在");
    assert_eq!(cached.total_mods_count, 0, "总模组数应清零");
    assert!(
        !cached.mods.iter().any(|m| m.mod_name == "TestMod"),
        "扁平列表应剔除该模组"
    );
    let group = cached
        .groups
        .iter()
        .find(|g| g.group_name == "group_1")
        .expect("分组仍存在");
    assert!(
        !group.mods.iter().any(|m| m.mod_name == "TestMod"),
        "组树内应剔除该模组"
    );
    assert_eq!(group.mod_count, 0, "组内模组计数应清零");

    // 二次移除（已无此模组）返回 false
    assert!(!cache.remove_mod_entry(TargetGame::GenshinImpact, &mods_path, &full));
}

/// 测试删除传播：局部扫描合并后，磁盘上已删除的模组应从缓存移除。
///
/// 回归场景：用户删除 group_1 下某模组目录，文件监听器触发局部重扫 →
/// `subtree_replace` 合并 partial 结果。原实现只增改不删，导致已删除模组
/// 在 UI 中残留至全量重扫；修复后应即时消失。
#[test]
fn test_subtree_replace_propagates_deletion() {
    use crate::core::mod_scan_partial::scan_partial_path;
    let (dir, mods_path) = setup_test_mods();
    let group1 = dir.path().join("_MANAGED_").join("group_1");
    let mod_gone = group1.join("ModGone");
    fs::create_dir_all(&mod_gone).unwrap();
    fs::write(mod_gone.join("mod.ini"), "[Section]\nx=1\n").unwrap();

    let mut cache = ModCache::new();
    let result = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
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
    use crate::core::mod_scan_partial::scan_partial_path;
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
    let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
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
    use crate::core::mod_scan_partial::scan_partial_path;
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let g2 = dir.path().join("_MANAGED_").join("group_2");
    fs::create_dir_all(g2.join("ModB")).unwrap();
    fs::write(g2.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();

    let mut cache = ModCache::new();
    let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
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

/// 测试：subtree_remove 移除顶层组节点 + 其全部扁平 mods + 计数重算 + 返回移除数。
///
/// 场景：remove_group 等分组级删除命令（磁盘目录已移除）直接裁剪缓存。
#[test]
fn subtree_remove_removes_group_subtree_and_recounts() {
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join("_MANAGED_");
    fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    // group_1：一启一禁两个模组；group_2：一个启用模组
    fs::create_dir_all(managed.join("group_1").join("ModA")).unwrap();
    fs::write(
        managed.join("group_1").join("ModA").join("mod.ini"),
        "[Section]\nx=1\n",
    )
    .unwrap();
    fs::create_dir_all(managed.join("group_1").join("DISABLED_ModB")).unwrap();
    fs::write(
        managed
            .join("group_1")
            .join("DISABLED_ModB")
            .join("mod.ini"),
        "[Section]\nx=1\n",
    )
    .unwrap();
    fs::create_dir_all(managed.join("group_2").join("ModC")).unwrap();
    fs::write(
        managed.join("group_2").join("ModC").join("mod.ini"),
        "[Section]\nx=1\n",
    )
    .unwrap();

    let mut cache = ModCache::new();
    let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    cache.set(TargetGame::GenshinImpact, &mods_path, full);
    assert_eq!(
        cache
            .get(TargetGame::GenshinImpact, &mods_path)
            .unwrap()
            .mods
            .len(),
        3,
        "前置条件：三个模组均进入缓存"
    );

    let removed = cache.subtree_remove(
        TargetGame::GenshinImpact,
        &mods_path,
        &managed.join("group_1"),
    );

    assert_eq!(removed, 2, "应返回被移除的模组数");
    let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    assert!(
        updated.groups.iter().all(|g| g.group_name != "group_1"),
        "group_1 组节点应被移除"
    );
    assert!(
        updated.groups.iter().any(|g| g.group_name == "group_2"),
        "group_2 组节点应保留"
    );
    assert!(
        !updated
            .mods
            .iter()
            .any(|m| m.mod_name == "ModA" || m.mod_name == "DISABLED_ModB"),
        "group_1 子树下的模组应从扁平列表移除"
    );
    assert!(
        updated.mods.iter().any(|m| m.mod_name == "ModC"),
        "group_2 的模组应保留"
    );
    // 计数按全量轻量扫描口径重算：total=1 enabled=1 disabled=0
    assert_eq!(
        (
            updated.total_mods_count,
            updated.enabled_mods_count,
            updated.disabled_mods_count
        ),
        (1, 1, 0)
    );
}

/// 测试：subtree_remove 缓存未命中时返回 0，不 panic。
#[test]
fn subtree_remove_cache_miss_returns_zero() {
    let mut cache = ModCache::new();
    let fake_path = PathBuf::from("/nonexistent/path");
    let removed = cache.subtree_remove(
        TargetGame::GenshinImpact,
        &fake_path,
        &fake_path.join("_MANAGED_").join("group_1"),
    );
    assert_eq!(removed, 0, "缓存未命中应返回 0");
}

/// C-3b：同组 partial 重扫（磁盘无变化）合并后，未变模组复用旧 Arc 实例
///（`Arc::ptr_eq` 证明零重分配），且签名保持 → 再次合并仍命中。
#[test]
fn subtree_replace_reuses_unchanged_mods_by_signature() {
    use crate::core::mod_scan_partial::scan_partial_path;
    let (dir, mods_path) = setup_test_mods();
    let group1 = setup_two_mod_group(&dir);

    let mut cache = ModCache::new();
    let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    cache.set(TargetGame::GenshinImpact, &mods_path, full);

    // 快照旧实例（get 返回共享 Arc，subtree_replace 的 make_mut 对其 COW 保护）
    let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let old_group_test = group_mod_arc(&old, "group_1", "TestMod");
    let old_group_b = group_mod_arc(&old, "group_1", "ModB");
    let old_flat_test = flat_mod_arc(&old, "TestMod");
    assert!(
        Arc::ptr_eq(old_group_test, old_flat_test),
        "前置：set 时组内条目与扁平条目共享同一 Arc"
    );

    // 同组 partial 重扫（未修改任何文件）
    let partial = scan_partial_path(&mods_path, &group1).unwrap();
    cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

    let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let new_group_test = group_mod_arc(&updated, "group_1", "TestMod");
    let new_group_b = group_mod_arc(&updated, "group_1", "ModB");
    let new_flat_test = flat_mod_arc(&updated, "TestMod");
    let new_flat_b = flat_mod_arc(&updated, "ModB");
    assert!(
        Arc::ptr_eq(new_group_test, old_group_test),
        "未变模组 TestMod 组内条目应复用旧 Arc（零重分配）"
    );
    assert!(
        Arc::ptr_eq(new_group_b, old_group_b),
        "未变模组 ModB 组内条目应复用旧 Arc（零重分配）"
    );
    assert!(
        Arc::ptr_eq(new_flat_test, old_group_test),
        "扁平条目应复用同一旧 Arc"
    );
    assert!(
        Arc::ptr_eq(new_flat_b, old_group_b),
        "扁平条目应复用同一旧 Arc"
    );

    // 复用后签名保持（旧实例签名即命中值）→ 二次合并仍命中
    let partial2 = scan_partial_path(&mods_path, &group1).unwrap();
    cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial2);
    let updated2 = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    assert!(
        Arc::ptr_eq(
            group_mod_arc(&updated2, "group_1", "TestMod"),
            old_group_test
        ),
        "签名保持 → 二次合并仍复用旧 Arc"
    );
}

/// C-3b：fav 状态变化（mtime 源变化）→ 签名不等 → 该条目不复用（新实例
/// 反映新收藏状态），未变兄弟仍复用。
#[test]
fn subtree_replace_replaces_entry_when_fav_changes() {
    use crate::core::mod_scan_partial::scan_partial_path;
    let (dir, mods_path) = setup_test_mods();
    let group1 = setup_two_mod_group(&dir);

    let mut cache = ModCache::new();
    let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    cache.set(TargetGame::GenshinImpact, &mods_path, full);
    let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let old_group_test = group_mod_arc(&old, "group_1", "TestMod");
    let old_group_b = group_mod_arc(&old, "group_1", "ModB");

    // 变化：TestMod 创建 fav 文件
    fs::write(group1.join("TestMod").join("fav"), "").unwrap();
    let partial = scan_partial_path(&mods_path, &group1).unwrap();
    cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

    let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let new_group_test = group_mod_arc(&updated, "group_1", "TestMod");
    let new_group_b = group_mod_arc(&updated, "group_1", "ModB");
    assert!(
        !Arc::ptr_eq(new_group_test, old_group_test),
        "fav 变化的模组不得复用旧实例"
    );
    assert!(new_group_test.is_favorite, "新实例应反映 fav 状态");
    assert!(
        Arc::ptr_eq(new_group_b, old_group_b),
        "未变兄弟模组仍复用旧 Arc"
    );
}

/// C-3b 正确性边界：签名必须覆盖 is_active 派生源（组 selectedindex）。
///
/// 两启用模组场景下 is_active 回退到 selectedindex 文件派生；select 改写
/// selectedindex 后增量合并，模组必须以新实例反映新激活态——若签名漏掉
/// selectedindex 源，未动模组被复用旧实例，is_active 停留在旧值。
#[test]
fn subtree_replace_signature_covers_selectedindex_source() {
    use crate::core::mod_scan_partial::scan_partial_path;
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
    // 前置：两启用 → 多启用回退 selectedindex（首扫创建为 0）→ 无模组激活
    let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let old_a = group_mod_arc(&old, "group_1", "ModA");
    assert!(
        !old_a.is_active,
        "前置：初始 selectedindex=0 时 ModA 未激活"
    );

    // select：改写 selectedindex=1（组内排序 [None, ModA, ModB] → 选中 ModA）
    fs::write(group1.join("selectedindex"), "1").unwrap();
    let partial = scan_partial_path(&mods_path, &group1).unwrap();
    cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

    let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let new_a = group_mod_arc(&updated, "group_1", "ModA");
    let new_b = group_mod_arc(&updated, "group_1", "ModB");
    assert!(
        !Arc::ptr_eq(new_a, old_a),
        "selectedindex 源变化 → ModA 不得复用旧实例"
    );
    assert!(
        new_a.is_active,
        "selectedindex=1 → ModA 应激活（新实例派生）"
    );
    assert!(!new_b.is_active, "ModB 不应激活");
}

/// C-3b：互斥组嵌套 children 的深层组内模组同样按签名递归复用。
#[test]
fn subtree_replace_reuses_mutex_children_entries() {
    use crate::core::mod_scan_partial::scan_partial_path;
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join("_MANAGED_");
    fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    let mutex_root = managed.join("#Mutex");
    fs::create_dir_all(mutex_root.join("Sub").join("ModX")).unwrap();
    fs::write(
        mutex_root.join("Sub").join("ModX").join("mod.ini"),
        "[Section]\nx=1\n",
    )
    .unwrap();
    fs::create_dir_all(mutex_root.join("ModY")).unwrap();
    fs::write(mutex_root.join("ModY").join("mod.ini"), "[Section]\nx=1\n").unwrap();

    let mut cache = ModCache::new();
    let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    cache.set(TargetGame::GenshinImpact, &mods_path, full);
    let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let old_mutex = old
        .groups
        .iter()
        .find(|g| g.group_name == "#Mutex")
        .expect("应含 #Mutex 根节点");
    let old_sub_x = old_mutex
        .children
        .iter()
        .find(|c| c.group_name == "Sub")
        .expect("应含嵌套子分组 Sub")
        .mods
        .iter()
        .find(|m| m.name == "ModX")
        .expect("Sub 内应含 ModX");
    let old_flat_x = flat_mod_arc(&old, "ModX");
    assert!(
        Arc::ptr_eq(old_sub_x, old_flat_x),
        "前置：互斥组深层条目与扁平条目共享同一 Arc"
    );

    // #Mutex 根整组局部重扫（无任何磁盘变化）
    let partial = scan_partial_path(&mods_path, &mutex_root).unwrap();
    cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

    let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
    let new_mutex = updated
        .groups
        .iter()
        .find(|g| g.group_name == "#Mutex")
        .expect("合并后应含 #Mutex 根节点");
    let new_sub_x = new_mutex
        .children
        .iter()
        .find(|c| c.group_name == "Sub")
        .expect("合并后应含嵌套子分组 Sub")
        .mods
        .iter()
        .find(|m| m.name == "ModX")
        .expect("Sub 内应含 ModX");
    assert!(
        Arc::ptr_eq(new_sub_x, old_sub_x),
        "嵌套 children 深层组内模组应复用旧 Arc"
    );
    assert!(
        Arc::ptr_eq(flat_mod_arc(&updated, "ModX"), old_sub_x),
        "互斥组扁平条目应复用同一旧 Arc"
    );
}
