use super::*;
use crate::core::{mod_cache_core, mod_cache_delta, mod_cache_path};
use tempfile::TempDir;

/// 分层分类规则 1：脏目录全部位于同一 group → 仅该分组全量
#[test]
fn classify_single_group() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let managed = Path::new("/mods/_MANAGED_");
    let dirty = vec![
        managed.join("group_1/ModA"),
        managed.join("group_1/ModB/sub"),
    ];
    let roots = classify_overflow_roots(&dirty, managed);
    assert_eq!(roots, vec![managed.join("group_1")]);
}

/// 分层分类规则 2：跨多个 group、无 mutex → 各涉及分组全量
#[test]
fn classify_multiple_groups() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let managed = Path::new("/mods/_MANAGED_");
    let dirty = vec![
        managed.join("group_1/ModA"),
        managed.join("group_2/ModB"),
        managed.join("group_1/ModC"),
    ];
    let mut roots = classify_overflow_roots(&dirty, managed);
    roots.sort();
    assert_eq!(
        roots,
        vec![managed.join("group_1"), managed.join("group_2")]
    );
}

/// 分层分类规则 3：仅 mutex 目录 → 各 mutex 全量
#[test]
fn classify_mutex_only() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let managed = Path::new("/mods/_MANAGED_");
    let dirty = vec![managed.join("#Mutex1/ModA"), managed.join("#Mutex2/ModB")];
    let mut roots = classify_overflow_roots(&dirty, managed);
    roots.sort();
    assert_eq!(
        roots,
        vec![managed.join("#Mutex1"), managed.join("#Mutex2")]
    );
}

/// 分层分类规则 4：group/mutex 混合或 _MANAGED_ 之外 → 空（全量兜底）
#[test]
fn classify_mixed_or_external() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let managed = Path::new("/mods/_MANAGED_");
    // 混合
    let dirty = vec![managed.join("group_1/ModA"), managed.join("#M/ModB")];
    assert!(classify_overflow_roots(&dirty, managed).is_empty());
    // _MANAGED_ 之外
    let dirty2 = vec![PathBuf::from("/mods/loose/ModA")];
    assert!(classify_overflow_roots(&dirty2, managed).is_empty());
    // _MANAGED_ 根自身
    let dirty3 = vec![managed.to_path_buf()];
    assert!(classify_overflow_roots(&dirty3, managed).is_empty());
}

/// 增量分支 delta 组装：局部重扫产物 → 读锁收集旧状态 → 写锁合并 → GroupDelta 列表。
///
/// 对齐 mod_commands R1 冒烟测试模式（临时目录 fixture + 全局 MOD_CACHE 唯一路径键，
/// 故意不 invalidate_all 以免破坏并行测试前置数据）。
#[test]
fn merge_partials_and_build_deltas_assembly() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // 写全局 MOD_CACHE 的测试串行（容量 3 + 4 个写者，防 LRU 跨测试淘汰）
    let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join(constants::MANAGED_FOLDER);
    std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    let group1 = managed.join("group_1");
    let group2 = managed.join("group_2");
    for (m, g) in [("ModA", &group1), ("ModB", &group1), ("ModC", &group2)] {
        std::fs::create_dir_all(g.join(m)).unwrap();
        std::fs::write(g.join(m).join("mod.ini"), "[Section]\nx=1\n").unwrap();
    }

    // 预填充缓存（模拟 get_mods 已加载该游戏）
    let game = crate::models::enums::TargetGame::GenshinImpact;
    let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    mod_cache_core::MOD_CACHE
        .write()
        .set(game, &mods_path, full);

    // 模拟外部变更：group_1 新增 ModD、group_2 新增 ModE（各自局部重扫产物）
    std::fs::create_dir_all(group1.join("ModD")).unwrap();
    std::fs::write(group1.join("ModD").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    std::fs::create_dir_all(group2.join("ModE")).unwrap();
    std::fs::write(group2.join("ModE").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    let scan_results = vec![
        (
            group1.clone(),
            crate::core::mod_scan_partial::scan_partial_path(&mods_path, &group1).unwrap(),
        ),
        (
            group2.clone(),
            crate::core::mod_scan_partial::scan_partial_path(&mods_path, &group2).unwrap(),
        ),
    ];

    let deltas = merge_partials_and_build_deltas(game, &mods_path, &scan_results);

    // 多 root 批次 → 每个受影响分组各产出 1 个 delta
    assert_eq!(deltas.len(), 2, "两个受影响分组应各产出一个 delta");
    let d1 = deltas
        .iter()
        .find(|d| d.old_group_path.ends_with("group_1"))
        .expect("应含 group_1 delta");
    assert_eq!(
        d1.old_group_path,
        group1.to_string_lossy().to_string(),
        "old_group_path 应为缓存内该组的分组路径"
    );
    // old_mod_paths 为合并前旧状态（ModA/ModB，不含新增 ModD）
    assert_eq!(d1.old_mod_paths.len(), 2, "旧状态应为合并前的 2 个模组");
    assert!(
        d1.old_mod_paths.iter().all(|p| !p.ends_with("ModD")),
        "old_mod_paths 不应含新增 ModD"
    );
    // new_group 为重扫后完整组节点（含新增 ModD）
    let ng = d1.new_group.as_ref().expect("new_group 不应为 None");
    assert_eq!(
        mod_cache_delta::collect_group_mod_paths(ng).len(),
        3,
        "新分组应含 3 个真实模组（ModA/ModB/ModD）"
    );

    // 缓存已合并（后端缓存与 delta 一致）
    let cached = mod_cache_core::MOD_CACHE
        .read()
        .get(game, &mods_path)
        .expect("缓存应命中");
    let g1 = mod_cache_path::find_group_node(&cached, &group1).expect("缓存应含 group_1");
    assert_eq!(
        mod_cache_delta::collect_group_mod_paths(g1).len(),
        3,
        "缓存 group_1 应已含新增 ModD"
    );
    assert_eq!(cached.total_mods_count, 5, "合并后计数应重算：A+B+C+D+E");

    // 同组重复 partial（多 root 收敛到同一组的场景）：第二个 partial 与
    // 已合并缓存完全一致 → P1 差异过滤直接跳过 delta emit（前端零打扰；
    // 「连续 apply 产生扁平重复条目」的问题随过滤不复存在）。缓存侧
    // subtree_replace 照常幂等合并，终态不变。
    let repeat = vec![(
        group1.clone(),
        crate::core::mod_scan_partial::scan_partial_path(&mods_path, &group1).unwrap(),
    )];
    let deltas2 = merge_partials_and_build_deltas(game, &mods_path, &repeat);
    assert!(
        deltas2.is_empty(),
        "与缓存一致的重复批次应被差异过滤，不 emit delta: {:?}",
        deltas2
    );
}

/// C4 世代校验：批次世代与当前世代一致则非过期，否则过期静默丢弃
///
/// 同一监听内（未再调用 `start_watching`）世代保持一致 → 不丢弃；
/// 切换游戏触发 `start_watching`（世代递增）后，旧监听批次捕获的快照过时 → 丢弃。
#[test]
fn generation_stale_check() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // 同一监听内：世代一致 → 不过期
    assert!(!is_generation_stale(1, 1));
    assert!(!is_generation_stale(5, 5));
    // 旧监听批次：当前世代已推进（start_watching 再次递增）→ 过期
    assert!(is_generation_stale(1, 2));
    assert!(is_generation_stale(5, 6));
    // 世代回退不应发生，但判定同样视为不一致（防御）
    assert!(is_generation_stale(6, 5));
    // 初始世代越界场景（不可能出现，逻辑防御）
    assert!(is_generation_stale(0, 1));
}

// =====================================================================
// C-3a：双粒度路由（is_mod_level_root / route_incremental_batch / 批次聚合）
// =====================================================================

/// C-3a 粒度路由判定：组级根（group_xx / _MANAGED_ 一级子目录）与模组级根分类
#[test]
fn is_mod_level_root_classification() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let managed = Path::new("/mods/_MANAGED_");
    // 组级：group_xx 目录
    assert!(!is_mod_level_root(&managed.join("group_1"), managed));
    // 组级：_MANAGED_ 一级子目录（mutex 顶层根，含根即模组的形态）
    assert!(!is_mod_level_root(&managed.join("#Mutex"), managed));
    assert!(!is_mod_level_root(&managed.join("StandaloneMod"), managed));
    // 组级：_MANAGED_ 根自身 / 之外
    assert!(!is_mod_level_root(managed, managed));
    assert!(!is_mod_level_root(
        &PathBuf::from("/mods/loose/ModA"),
        managed
    ));
    // 模组级：普通组下的模组目录
    assert!(is_mod_level_root(
        &managed.join("group_1").join("ModA"),
        managed
    ));
    // 模组级：mutex 根下模组 / 深层嵌套模组
    assert!(is_mod_level_root(
        &managed.join("#Mutex").join("ModX"),
        managed
    ));
    assert!(is_mod_level_root(
        &managed.join("#Mutex").join("Sub").join("ModX"),
        managed
    ));
    // 组级：mutex 树内 group_xx 命名的子分组（snap 会停在 group_xx 边界）
    assert!(!is_mod_level_root(
        &managed.join("#Mutex").join("group_5"),
        managed
    ));
    // 形态容忍：Windows verbatim（consolidate 的 canonicalize 产物形态）
    let verbatim = PathBuf::from(format!(
        "\\\\?\\{}",
        managed.join("group_1").join("ModA").to_string_lossy()
    ));
    assert!(
        is_mod_level_root(&verbatim, managed),
        "verbatim 形态的模组级根应被识别"
    );
}

/// C-3a 顶层组目录推导：模组级根 → `_MANAGED_` 一级子目录（含 verbatim/大小写容忍）
#[test]
fn top_level_group_dir_derivation() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let managed = Path::new("/mods/_MANAGED_");
    assert_eq!(
        top_level_group_dir(&managed.join("group_1").join("ModA"), managed),
        Some(managed.join("group_1"))
    );
    assert_eq!(
        top_level_group_dir(&managed.join("#M").join("Sub").join("ModX"), managed),
        Some(managed.join("#M"))
    );
    // 组外路径 → None
    assert_eq!(
        top_level_group_dir(&PathBuf::from("/mods/loose/ModA"), managed),
        None
    );
}

/// C-3a 路由行为证明（一）：模组级根走单模组 patch 而非整组重扫——
/// 组内**未被指向的兄弟**的陈旧状态保留（整组重扫会以磁盘为准重扫并传播删除）。
#[test]
fn route_incremental_batch_mod_level_not_group_rescan() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join(constants::MANAGED_FOLDER);
    std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    let group1 = managed.join("group_1");
    for m in ["ModA", "ModB"] {
        std::fs::create_dir_all(group1.join(m)).unwrap();
        std::fs::write(group1.join(m).join("mod.ini"), "[Section]\nx=1\n").unwrap();
    }

    let game = crate::models::enums::TargetGame::GenshinImpact;
    mod_cache_core::MOD_CACHE.write().set(
        game,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );

    // 磁盘侧「删除 ModB + 深部变更 ModA」，但收敛根仅指向 ModA：
    // 真实事件流中删除自身会收敛出组级根（整组重扫），此处模拟纯模组级批次
    std::fs::remove_dir_all(group1.join("ModB")).unwrap();
    std::fs::write(group1.join("ModA").join("fav"), "").unwrap();

    let deltas = route_incremental_batch(game, &mods_path, &managed, &[group1.join("ModA")]);

    // 产出 group_1 的单个 delta（模组级 patch 形状：整组节点）
    assert_eq!(deltas.len(), 1, "单模组级根应产出一个组 delta");
    let d = &deltas[0];
    assert!(d.old_group_path.ends_with("group_1"));
    let ng = d.new_group.as_ref().expect("new_group 不应为 None");
    assert!(
        ng.mods.iter().any(|m| m.name == "ModA" && m.is_favorite),
        "深部 fav 变更生效"
    );
    // 行为证明：ModB 未被该批次触及（陈旧保留）——若走整组重扫，
    // scan_partial_path 会以磁盘为准重扫且删除传播会剔除 ModB
    assert!(
        ng.mods.iter().any(|m| m.name == "ModB"),
        "模组级 patch 不应触碰兄弟条目（证明未走整组重扫路径）"
    );
    // 缓存与 delta 一致
    let cached = mod_cache_core::MOD_CACHE
        .read()
        .get(game, &mods_path)
        .unwrap();
    let g1 = mod_cache_path::find_group_node(&cached, &group1).unwrap();
    assert!(
        g1.mods.iter().any(|m| m.name == "ModB"),
        "缓存同样保留兄弟（等其自身批次收敛）"
    );
}

/// C-3a 路由行为证明（二）：新增模组 patch 置组内**末位**（整组重扫会按排序规则
/// 插入自然位），以此证明模组级路径被命中。
#[test]
fn route_incremental_batch_inserts_at_end_not_sorted_position() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join(constants::MANAGED_FOLDER);
    std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    let group1 = managed.join("group_1");
    std::fs::create_dir_all(group1.join("ZZZMod")).unwrap();
    std::fs::write(group1.join("ZZZMod").join("mod.ini"), "[Section]\nx=1\n").unwrap();

    let game = crate::models::enums::TargetGame::GenshinImpact;
    mod_cache_core::MOD_CACHE.write().set(
        game,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );

    // 新增 AAAMod（自然序在 ZZZMod 之前；整组重扫会将其排到组首真实模组位）
    std::fs::create_dir_all(group1.join("AAAMod")).unwrap();
    std::fs::write(group1.join("AAAMod").join("mod.ini"), "[Section]\nx=1\n").unwrap();

    route_incremental_batch(game, &mods_path, &managed, &[group1.join("AAAMod")]);

    let cached = mod_cache_core::MOD_CACHE
        .read()
        .get(game, &mods_path)
        .unwrap();
    let g1 = mod_cache_path::find_group_node(&cached, &group1).unwrap();
    let names: Vec<&str> = g1.mods.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["None", "ZZZMod", "AAAMod"],
        "新增模组置于末位（模组级 patch 语义；排序位由下次组级扫描校正）"
    );
}

/// C-3a 混合批次聚合：组级根 + 模组级根各自产出 delta；
/// 同组多个模组级根合并为**一次**组 delta。
#[test]
fn route_incremental_batch_mixed_and_aggregation() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join(constants::MANAGED_FOLDER);
    std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    // group_1：ModA + ModB；group_2：ModC（组级根用）
    for (g, m) in [
        ("group_1", "ModA"),
        ("group_1", "ModB"),
        ("group_2", "ModC"),
    ] {
        std::fs::create_dir_all(managed.join(g).join(m)).unwrap();
        std::fs::write(managed.join(g).join(m).join("mod.ini"), "[Section]\nx=1\n").unwrap();
    }

    let game = crate::models::enums::TargetGame::GenshinImpact;
    mod_cache_core::MOD_CACHE.write().set(
        game,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );

    // 混合批次：group_2（组级）+ group_1/ModA + group_1/ModB（同组两个模组级）
    std::fs::write(managed.join("group_1").join("ModA").join("fav"), "").unwrap();
    std::fs::write(managed.join("group_1").join("ModB").join("fav"), "").unwrap();
    std::fs::write(managed.join("group_2").join("ModC").join("fav"), "").unwrap();

    let deltas = route_incremental_batch(
        game,
        &mods_path,
        &managed,
        &[
            managed.join("group_2"),
            managed.join("group_1").join("ModA"),
            managed.join("group_1").join("ModB"),
        ],
    );

    // group_1 两个模组级根 → 合并为 1 个 delta；group_2 组级根 → 1 个 delta
    assert_eq!(deltas.len(), 2, "混合批次应聚合为两个组 delta");
    let g1_deltas = deltas
        .iter()
        .filter(|d| d.old_group_path.ends_with("group_1"))
        .count();
    assert_eq!(g1_deltas, 1, "同组两个模组级根应合并为一次组 delta");
    let d1 = deltas
        .iter()
        .find(|d| d.old_group_path.ends_with("group_1"))
        .unwrap();
    let ng = d1.new_group.as_ref().unwrap();
    assert!(
        ng.mods.iter().any(|m| m.name == "ModA" && m.is_favorite)
            && ng.mods.iter().any(|m| m.name == "ModB" && m.is_favorite),
        "同组批次的两个模组变更均生效于合并后的单个 delta"
    );
    assert_eq!(
        d1.old_mod_paths.len(),
        2,
        "旧状态为合并前（两模组均未 fav）"
    );
    // 缓存终态一致
    let cached = mod_cache_core::MOD_CACHE
        .read()
        .get(game, &mods_path)
        .unwrap();
    assert_eq!(cached.total_mods_count, 3);
    assert!(
        cached.mods.iter().filter(|m| m.is_favorite).count() == 3,
        "三个模组的 fav 状态均合并入缓存"
    );
}

/// C-3a 降级兜底：普通组内嵌套目录（轻扫不递归 NormalGroup——`Sub` 在缓存中是
/// group_1 的模组条目而非分组，深层 `Sub/ModX` 无模组定位）的模组级根 →
/// patch 未命中 → 降级组级重扫，与变更前行为一致（缓存不漏更新、不损坏）。
#[test]
fn route_incremental_batch_nested_normal_group_falls_back_to_group_rescan() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join(constants::MANAGED_FOLDER);
    std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    // group_1/Sub/ModX：Sub 无 ini（轻扫中是 group_1 的模组条目），ModX 含 ini
    let group1 = managed.join("group_1");
    let mod_x = group1.join("Sub").join("ModX");
    std::fs::create_dir_all(&mod_x).unwrap();
    std::fs::write(mod_x.join("mod.ini"), "[Section]\nx=1\n").unwrap();

    let game = crate::models::enums::TargetGame::GenshinImpact;
    mod_cache_core::MOD_CACHE.write().set(
        game,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );
    let before = mod_cache_core::MOD_CACHE
        .read()
        .get(game, &mods_path)
        .unwrap();
    assert!(
        before.groups.iter().any(|g| g.group_name == "group_1"),
        "前置：group_1 已入缓存"
    );

    // 深部变更（ModX 加 fav；模组级根指向 ModX）
    std::fs::write(mod_x.join("fav"), "").unwrap();
    let deltas = route_incremental_batch(game, &mods_path, &managed, &[mod_x]);

    // P1 差异过滤：嵌套深层变更（ModX/fav）在轻扫粒度下对缓存与 UI 不可见
    //（group_1/Sub 条目不受影响）→ 降级组级重扫产物与缓存一致 → 差异过滤
    // 拦截，不 emit delta（watcher 对嵌套路径「空转不惊动前端」的实证）。
    // 缓存侧照常合并（幂等），组节点健壮性断言保留。
    assert!(
        deltas.is_empty(),
        "嵌套深层变更的降级组级重扫与缓存一致，应被差异过滤: {:?}",
        deltas
    );
    let cached = mod_cache_core::MOD_CACHE
        .read()
        .get(game, &mods_path)
        .unwrap();
    let g1 = mod_cache_path::find_group_node(&cached, &group1).expect("缓存组节点应健在");
    assert!(
        g1.mods.iter().any(|m| m.name == "Sub"),
        "Sub 条目保留（轻扫粒度：嵌套内容对其不可见，与变更前行为一致）"
    );
    assert_eq!(cached.total_mods_count, 1, "计数不受影响");
}

/// C-3a 集成对齐（正确性金标准）：模组级 patch 后的缓存数据与整组重扫结果一致。
///
/// fixture 覆盖普通组（一启用一禁用 + 唯一启用者 active）与互斥树（根模组 +
/// 嵌套子分组模组）；深部变更（fav）走模组级路由后，缓存序列化产物应与
/// 对同一磁盘状态的全量轻量扫描完全一致。
#[test]
fn route_incremental_batch_mod_level_parity_with_full_scan() {
    // 还原安全窗口互斥：扫描会读/建 modname 标记，与 restore 系测试的 60s 全局窗口串行
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join(constants::MANAGED_FOLDER);
    std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();

    // 普通组：ModA（唯一启用 → active）、DISABLED_ModB
    let group1 = managed.join("group_1");
    for m in ["ModA", "DISABLED_ModB"] {
        std::fs::create_dir_all(group1.join(m)).unwrap();
        std::fs::write(group1.join(m).join("mod.ini"), "[Section]\nx=1\n").unwrap();
    }
    // 互斥树：#M/{ModW, Sub/{ModX, ModY}}
    let mutex_root = managed.join("#M");
    std::fs::create_dir_all(mutex_root.join("ModW")).unwrap();
    std::fs::write(mutex_root.join("ModW").join("mod.ini"), "[Section]\nx=1\n").unwrap();
    let sub = mutex_root.join("Sub");
    for m in ["ModX", "ModY"] {
        std::fs::create_dir_all(sub.join(m)).unwrap();
        std::fs::write(sub.join(m).join("mod.ini"), "[Section]\nx=1\n").unwrap();
    }

    let game = crate::models::enums::TargetGame::GenshinImpact;
    mod_cache_core::MOD_CACHE.write().set(
        game,
        &mods_path,
        crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
    );

    // 深部变更（外部工具写入场景）：普通组 ModA 与互斥树深层 ModX 各加 fav
    std::fs::write(group1.join("ModA").join("fav"), "").unwrap();
    std::fs::write(sub.join("ModX").join("fav"), "").unwrap();

    let deltas = route_incremental_batch(
        game,
        &mods_path,
        &managed,
        &[group1.join("ModA"), sub.join("ModX")],
    );
    assert_eq!(deltas.len(), 2, "两个顶层组各一个 delta");

    // 金标准：缓存序列化产物与同一磁盘状态的全量轻量扫描完全一致
    let cached = mod_cache_core::MOD_CACHE
        .read()
        .get(game, &mods_path)
        .unwrap();
    let fresh = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    let cached_json = serde_json::to_value(&*cached).expect("缓存序列化失败");
    let fresh_json = serde_json::to_value(&fresh).expect("全量扫描序列化失败");
    assert_eq!(
        cached_json, fresh_json,
        "mod 级 patch 后缓存数据应与整组重扫结果一致（get_mods 口径）"
    );
}
