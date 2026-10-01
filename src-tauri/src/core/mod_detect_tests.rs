use super::*;
use std::collections::HashSet;
use std::fs;
use tempfile::TempDir;

/// 创建带 _MANAGED_ 子目录的临时根目录
fn setup_test_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();
    dir
}

/// 在分组目录下创建带 INI 文件的模组目录
fn create_mod_with_ini(group_path: &Path, mod_name: &str, ini_content: &str) -> PathBuf {
    let mod_path = group_path.join(mod_name);
    fs::create_dir_all(&mod_path).unwrap();
    let ini_path = mod_path.join("mod.ini");
    fs::write(&ini_path, ini_content).unwrap();
    mod_path
}

/// 在根目录创建 d3dx.ini
fn create_d3dx_ini(base: &Path) {
    fs::write(base.join("d3dx.ini"), "; test\n").unwrap();
}

/// 测试：known_libraries 并行收集（collect_libraries_parallel）与串行逐个收集结果集合相等
#[test]
fn test_known_libraries_parallel_equals_serial() {
    let dir = TempDir::new().unwrap();
    let ini_a = dir.path().join("a.ini");
    fs::write(&ini_a, "[ResourceLibA]\nhash = 0x1\n").unwrap();
    let ini_b = dir.path().join("b.ini");
    fs::write(&ini_b, "[CommandListLibB]\nps-t0 = ResourceLibA\n").unwrap();
    let ini_c = dir.path().join("c.ini");
    fs::write(&ini_c, "[Constants]\nx = 1\n").unwrap();

    let paths = vec![ini_a, ini_b, ini_c];
    let parallel = collect_libraries_parallel(&paths);

    // 串行基准：逐个 get_or_parse_ini 收集 defined_libraries
    let mut serial = HashSet::new();
    for p in &paths {
        if let Ok(ini) = get_or_parse_ini(p) {
            for lib in ini.defined_libraries() {
                serial.insert(lib);
            }
        }
    }

    assert_eq!(parallel, serial, "并行收集与串行收集集合应相等");
    assert!(parallel.contains("ResourceLibA"));
    assert!(parallel.contains("CommandListLibB"));
    assert_eq!(parallel.len(), 2);
}

/// 测试：detect_hash_conflicts 应扫描 MutexGroup（非group目录）下的模组
///
/// 验证：非group目录下的模组参与 hash 冲突检测
/// - 创建两个非group目录，各含一个使用相同 hash 的模组
/// - 调用 detect_hash_conflicts 应检测到冲突
#[test]
fn test_hash_conflict_detects_mutex_group_mods() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    // 非group目录1: #MutexA/ModA（使用 hash=0x12345678）
    let mutex_a = dir.path().join("_MANAGED_").join("#MutexA");
    fs::create_dir_all(&mutex_a).unwrap();
    create_mod_with_ini(
        &mutex_a,
        "ModA",
        "[TextureOverrideTexA]\nhash = 0x12345678\n",
    );

    // 非group目录2: #MutexB/ModB（使用相同 hash=0x12345678，应冲突）
    let mutex_b = dir.path().join("_MANAGED_").join("#MutexB");
    fs::create_dir_all(&mutex_b).unwrap();
    create_mod_with_ini(
        &mutex_b,
        "ModB",
        "[TextureOverrideTexB]\nhash = 0x12345678\n",
    );

    let result = detect_hash_conflicts(dir.path()).unwrap();
    // 应检测到至少 1 个冲突（hash=0x12345678 被 ModA 和 ModB 同时使用）
    assert!(
        !result.conflicts.is_empty(),
        "非group目录下的模组应参与 hash 冲突检测，但未检测到冲突"
    );
    // 验证冲突涉及两个模组
    let first_conflict = &result.conflicts[0];
    assert_eq!(first_conflict.entries.len(), 2);
    let mod_names: Vec<&str> = first_conflict
        .entries
        .iter()
        .map(|e| e.mod_name.as_str())
        .collect();
    assert!(mod_names.contains(&"ModA"));
    assert!(mod_names.contains(&"ModB"));
    // 验证 entries 已填充且 ini_vec 指向 .ini 文件
    assert!(!first_conflict.entries.is_empty(), "entries 应填充详情");
    for entry in &first_conflict.entries {
        assert!(!entry.ini_vec.is_empty(), "ini_vec 不应为空");
        assert!(
            entry.ini_vec[0].ends_with(".ini"),
            "ini_vec 应指向 .ini 文件"
        );
    }
}

/// 测试：detect_hash_conflicts 同时扫描 NormalGroup 和 MutexGroup
///
/// 验证：group_xx 目录和非group目录下的模组都参与 hash 冲突检测
#[test]
fn test_hash_conflict_scans_both_group_types() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    // group_1 下的启用模组（需要在 selectedindex 中标记为选中）
    let group1 = dir.path().join("_MANAGED_").join("group_1");
    fs::create_dir_all(&group1).unwrap();
    fs::write(group1.join("selectedindex"), "1").unwrap();
    create_mod_with_ini(
        &group1,
        "NormalMod",
        "[TextureOverrideNormal]\nhash = 0xdeadbeef\n",
    );

    // 非group目录下的启用模组
    let mutex_group = dir.path().join("_MANAGED_").join("#Mutex");
    fs::create_dir_all(&mutex_group).unwrap();
    create_mod_with_ini(
        &mutex_group,
        "MutexMod",
        "[TextureOverrideMutex]\nhash = 0xdeadbeef\n",
    );

    let result = detect_hash_conflicts(dir.path()).unwrap();
    // 应检测到冲突：NormalMod 和 MutexMod 使用相同 hash
    assert!(
        !result.conflicts.is_empty(),
        "应检测到 NormalGroup 和 MutexGroup 之间的 hash 冲突"
    );
    let conflict = &result.conflicts[0];
    assert_eq!(conflict.entries.len(), 2);
    let mod_names: Vec<&str> = conflict
        .entries
        .iter()
        .map(|e| e.mod_name.as_str())
        .collect();
    assert!(mod_names.contains(&"NormalMod"));
    assert!(mod_names.contains(&"MutexMod"));
    // 验证 entries 已填充且 ini_vec 指向 .ini 文件
    assert!(!conflict.entries.is_empty(), "entries 应填充详情");
    for entry in &conflict.entries {
        assert!(!entry.ini_vec.is_empty(), "ini_vec 不应为空");
        assert!(
            entry.ini_vec[0].ends_with(".ini"),
            "ini_vec 应指向 .ini 文件"
        );
    }
}

/// 测试：detect_hash_conflicts 应跳过被 NRMM 禁用的 hash 行（;-;hash = ...）
///
/// 验证：当模组 A 的 hash 行被 `;-;` 禁用（DisabledKeyValue），
/// 模组 B 正常使用同一 hash 时，不报告冲突。
#[test]
fn test_hash_conflict_skips_disabled_hash_lines() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    // 非group目录1: ModA 的 hash 行被 ;-; 禁用
    let mutex_a = dir.path().join("_MANAGED_").join("#MutexA");
    fs::create_dir_all(&mutex_a).unwrap();
    create_mod_with_ini(
        &mutex_a,
        "ModA",
        "[TextureOverrideTexA]\n;-;hash = 0x12345678\n",
    );

    // 非group目录2: ModB 正常使用同一 hash
    let mutex_b = dir.path().join("_MANAGED_").join("#MutexB");
    fs::create_dir_all(&mutex_b).unwrap();
    create_mod_with_ini(
        &mutex_b,
        "ModB",
        "[TextureOverrideTexB]\nhash = 0x12345678\n",
    );

    let result = detect_hash_conflicts(dir.path()).unwrap();
    // ModA 的 hash 被禁用，仅 ModB 有该 hash → 不构成冲突
    assert!(
        result.conflicts.is_empty(),
        "被 ;-; 禁用的 hash 行不应参与冲突检测，但检测到了冲突"
    );
}

/// 测试：detect_hash_conflicts 应跳过被禁用分组（DISABLED_ 前缀）下的模组
///
/// 验证：当 MutexGroup 子分组目录有 DISABLED_ 前缀（group_disabled=true），
/// 但其内模组目录无前缀时，该模组不参与冲突检测。
#[test]
fn test_hash_conflict_skips_disabled_group_mods() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    // MutexGroup 根分组（未禁用），内含一个被禁用的子分组
    let mutex_root = dir.path().join("_MANAGED_").join("#MutexRoot");
    fs::create_dir_all(&mutex_root).unwrap();
    // DISABLED_SubGroup 无 .ini 文件 → 被视为子分组节点，group_disabled=true
    let disabled_sub = mutex_root.join("DISABLED_SubGroup");
    fs::create_dir_all(&disabled_sub).unwrap();
    // SubGroup 内的模组（无 DISABLED_ 前缀，但父分组被禁用）
    create_mod_with_ini(
        &disabled_sub,
        "ModA",
        "[TextureOverrideTexA]\nhash = 0xdeadbeef\n",
    );

    // 另一个正常 MutexGroup，含使用相同 hash 的模组
    let another = dir.path().join("_MANAGED_").join("#Another");
    fs::create_dir_all(&another).unwrap();
    create_mod_with_ini(
        &another,
        "ModB",
        "[TextureOverrideTexB]\nhash = 0xdeadbeef\n",
    );

    let result = detect_hash_conflicts(dir.path()).unwrap();
    // ModA 在被禁用分组下，应被跳过；仅 ModB 有该 hash → 不构成冲突
    assert!(
        result.conflicts.is_empty(),
        "被禁用分组（DISABLED_ 前缀）下的模组不应参与冲突检测，但检测到了冲突"
    );
}

/// 测试：detect_hash_conflicts 应扫描启用分组内嵌套子分组中的启用模组（回归测试）
///
/// 验证：当 MutexGroup 子分组未禁用（无 DISABLED_ 前缀）时，
/// 其内嵌套的模组正常参与冲突检测，确保 group_disabled 传播不影响正常场景。
#[test]
fn test_hash_conflict_scans_enabled_mod_in_enabled_group() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    // MutexGroup 根分组（未禁用），内含一个正常子分组
    let mutex_root = dir.path().join("_MANAGED_").join("#GroupA");
    fs::create_dir_all(&mutex_root).unwrap();
    // SubGroup 无 .ini 文件 → 被视为子分组节点，group_disabled=false
    let enabled_sub = mutex_root.join("SubGroup");
    fs::create_dir_all(&enabled_sub).unwrap();
    // 子分组内的模组
    create_mod_with_ini(
        &enabled_sub,
        "ModA",
        "[TextureOverrideTexA]\nhash = 0xcafebabe\n",
    );

    // 另一个正常 MutexGroup，含使用相同 hash 的模组
    let another = dir.path().join("_MANAGED_").join("#GroupB");
    fs::create_dir_all(&another).unwrap();
    create_mod_with_ini(
        &another,
        "ModB",
        "[TextureOverrideTexB]\nhash = 0xcafebabe\n",
    );

    let result = detect_hash_conflicts(dir.path()).unwrap();
    // 两个启用模组使用相同 hash → 应检测到冲突
    assert!(
        !result.conflicts.is_empty(),
        "启用分组内嵌套子分组中的启用模组应参与冲突检测，但未检测到冲突"
    );
    let conflict = &result.conflicts[0];
    assert_eq!(conflict.entries.len(), 2);
    let mod_names: Vec<&str> = conflict
        .entries
        .iter()
        .map(|e| e.mod_name.as_str())
        .collect();
    assert!(mod_names.contains(&"ModA"));
    assert!(mod_names.contains(&"ModB"));
}

/// 测试：同一模组在多个 INI 文件中使用相同 hash 时，聚合到该模组的 ini_vec
///
/// 验证边界场景：
/// - 模组 ModA 有两个 INI 文件（a1.ini、a2.ini），都使用 hash=0xcafebabe
/// - 模组 ModB 有一个 INI 文件，也使用 hash=0xcafebabe
/// - entries 应包含 2 条记录（ModA、ModB 各一条）
/// - ModA 的 ini_vec 应有 2 个元素（a1.ini、a2.ini），ModB 的 ini_vec 应有 1 个元素
#[test]
fn test_hash_conflict_entries_tracks_ini_path() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    // ModA：两个 INI 文件都使用 hash=0xcafebabe
    let mutex_a = dir.path().join("_MANAGED_").join("#MutexA");
    fs::create_dir_all(&mutex_a).unwrap();
    let mod_a_dir = mutex_a.join("ModA");
    fs::create_dir_all(&mod_a_dir).unwrap();
    fs::write(
        mod_a_dir.join("a1.ini"),
        "[TextureOverrideT1]\nhash = 0xcafebabe\n",
    )
    .unwrap();
    fs::write(
        mod_a_dir.join("a2.ini"),
        "[TextureOverrideT2]\nhash = 0xcafebabe\n",
    )
    .unwrap();

    // ModB：一个 INI 文件使用相同 hash
    let mutex_b = dir.path().join("_MANAGED_").join("#MutexB");
    fs::create_dir_all(&mutex_b).unwrap();
    create_mod_with_ini(&mutex_b, "ModB", "[TextureOverrideT3]\nhash = 0xcafebabe\n");

    let result = detect_hash_conflicts(dir.path()).unwrap();
    assert!(!result.conflicts.is_empty(), "应检测到 hash 冲突");

    let conflict = &result.conflicts[0];
    assert_eq!(conflict.hash, "0xcafebabe");

    // entries 按模组聚合：ModA、ModB 各一条
    assert_eq!(
        conflict.entries.len(),
        2,
        "entries 应包含 2 条记录（ModA、ModB 各一条），实际：{:?}",
        conflict
            .entries
            .iter()
            .map(|e| &e.mod_name)
            .collect::<Vec<_>>()
    );

    // 验证 ModA 聚合了两条 INI 路径
    let mod_a_entry = conflict
        .entries
        .iter()
        .find(|e| e.mod_name == "ModA")
        .expect("应存在 ModA 的 entry");
    assert_eq!(
        mod_a_entry.ini_vec.len(),
        2,
        "ModA 的 ini_vec 应有 2 个元素"
    );
    assert!(
        mod_a_entry.ini_vec.iter().any(|p| p.ends_with("a1.ini")),
        "应包含 a1.ini 路径"
    );
    assert!(
        mod_a_entry.ini_vec.iter().any(|p| p.ends_with("a2.ini")),
        "应包含 a2.ini 路径"
    );

    // 验证 ModB 聚合了一条 INI 路径
    let mod_b_entry = conflict
        .entries
        .iter()
        .find(|e| e.mod_name == "ModB")
        .expect("应存在 ModB 的 entry");
    assert_eq!(
        mod_b_entry.ini_vec.len(),
        1,
        "ModB 的 ini_vec 应有 1 个元素"
    );
}
