use super::*;
use crate::core::constants;
use crate::core::mod_scan_marker::TEST_WINDOW_LOCK;
use crate::core::mod_scan_partial::scan_partial_path;
use std::fs;
use tempfile::TempDir;

/// 测试辅助：创建临时目录，在根目录下创建 `_MANAGED_` 子目录
fn setup_test_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();
    dir
}

/// 测试辅助：在 `_MANAGED_` 下创建分组目录
fn create_group_dir(base: &Path, group_name: &str) -> PathBuf {
    let group_path = base.join("_MANAGED_").join(group_name);
    fs::create_dir_all(&group_path).unwrap();
    group_path
}

/// 测试辅助：在分组目录下创建带 INI 文件的模组目录
fn create_mod_with_ini(group_path: &Path, mod_name: &str, ini_content: &str) -> PathBuf {
    let mod_path = group_path.join(mod_name);
    fs::create_dir_all(&mod_path).unwrap();
    let ini_path = mod_path.join("mod.ini");
    fs::write(&ini_path, ini_content).unwrap();
    mod_path
}

/// 测试辅助：在根目录下创建主 INI 文件（d3dx.ini）
fn create_d3dx_ini(base: &Path) {
    fs::write(base.join("d3dx.ini"), "; test").unwrap();
}

// ========== 轻量扫描测试（scan_mods_light） ==========

/// 测试：轻量扫描 NormalGroup 会自动创建 None 空槽位
#[test]
fn test_light_scan_none_slot() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&group_path, "TestMod", "[Section]\n");

    let result = scan_mods_light(dir.path()).unwrap();
    assert_eq!(result.groups.len(), 1);
    let group = &result.groups[0];
    assert!(group.mods.iter().any(|m| m.name == "None"));
    assert!(group.mods.iter().any(|m| m.name == "TestMod"));
}

/// 测试：轻量扫描 NormalGroup 不递归子目录
#[test]
fn test_light_scan_no_recursion_normal_group() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");

    let subdir = group_path.join("SubDir");
    fs::create_dir_all(&subdir).unwrap();
    let nested_mod = subdir.join("NestedMod");
    fs::create_dir_all(&nested_mod).unwrap();
    fs::write(nested_mod.join("mod.ini"), "[Section]\n").unwrap();

    let result = scan_mods_light(dir.path()).unwrap();
    let group = &result.groups[0];
    assert!(!group.mods.iter().any(|m| m.name == "NestedMod"));
    assert!(group.mods.iter().any(|m| m.name == "SubDir"));
    assert_eq!(group.mods.len(), 2); // None + SubDir
}

/// 测试：轻量扫描会自动创建缺失的标记文件（groupname、selectedindex、modname）
#[test]
fn test_light_scan_creates_marker_files() {
    let _window_lock = TEST_WINDOW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");
    let mod_path = group_path.join("TestMod");
    fs::create_dir_all(&mod_path).unwrap();

    assert!(!group_path.join("groupname").exists());
    assert!(!group_path.join("selectedindex").exists());
    assert!(!mod_path.join("modname").exists());

    let _result = scan_mods_light(dir.path()).unwrap();

    assert!(group_path.join("groupname").exists());
    assert!(group_path.join("selectedindex").exists());
    assert!(mod_path.join("modname").exists());
}

/// 测试：已存在的 groupname 仍被读取作为显示名（只读语义保留读取路径）
#[test]
fn test_light_scan_reads_existing_groupname() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");
    fs::write(group_path.join("groupname"), "My Custom Group\n").unwrap();

    let result = scan_mods_light(dir.path()).unwrap();
    assert_eq!(result.groups[0].name, "My Custom Group");
    assert_eq!(
        fs::read_to_string(group_path.join("groupname")).unwrap(),
        "My Custom Group\n"
    );
}

/// 测试：轻量扫描 MutexGroup 不会创建标记文件（与 NormalGroup 不同）
#[test]
fn test_light_scan_mutex_group_no_markers() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let mutex_path = dir.path().join("_MANAGED_").join("#MutexMods");
    fs::create_dir_all(&mutex_path).unwrap();
    let mod_path = mutex_path.join("MutexMod");
    fs::create_dir_all(&mod_path).unwrap();
    fs::write(mod_path.join("mod.ini"), "[Section]\n").unwrap();

    let _result = scan_mods_light(dir.path()).unwrap();

    assert!(!mutex_path.join("groupname").exists());
    assert!(!mutex_path.join("selectedindex").exists());
    assert!(!mod_path.join("modname").exists());
}

/// 测试：轻量扫描 MutexGroup 的栈式 DFS 能正确遍历嵌套目录
#[test]
fn test_light_scan_mutex_group_dfs() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let root = dir.path().join("_MANAGED_").join("#MutexRoot");
    fs::create_dir_all(&root).unwrap();

    let cat1 = root.join("Category1");
    fs::create_dir_all(&cat1).unwrap();
    let mod_a = cat1.join("ModA");
    fs::create_dir_all(&mod_a).unwrap();
    fs::write(mod_a.join("mod.ini"), "[A]\n").unwrap();

    let sub_cat = cat1.join("SubCategory");
    fs::create_dir_all(&sub_cat).unwrap();
    let mod_b = sub_cat.join("ModB");
    fs::create_dir_all(&mod_b).unwrap();
    fs::write(mod_b.join("mod.ini"), "[B]\n").unwrap();

    let mod_c = root.join("ModC");
    fs::create_dir_all(&mod_c).unwrap();
    fs::write(mod_c.join("mod.ini"), "[C]\n").unwrap();

    let result = scan_mods_light(dir.path()).unwrap();

    assert_eq!(result.groups.len(), 1);
    let root_group = &result.groups[0];
    assert_eq!(root_group.group_type, GroupType::MutexGroup);

    let mod_names: Vec<&str> = result.mods.iter().map(|m| m.name.as_str()).collect();
    assert!(mod_names.contains(&"ModA"), "Should find ModA");
    assert!(mod_names.contains(&"ModB"), "Should find ModB");
    assert!(mod_names.contains(&"ModC"), "Should find ModC");

    for m in &result.mods {
        assert!(m.is_mutex, "Mutex mod should have is_mutex=true");
    }
}

/// 测试：轻量扫描 DFS 遇到含 INI 的目录后停止向下遍历
#[test]
fn test_light_scan_stops_at_ini_dir() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let root = dir.path().join("_MANAGED_").join("#MutexRoot");
    fs::create_dir_all(&root).unwrap();

    let mod_dir = root.join("ModWithIni");
    fs::create_dir_all(&mod_dir).unwrap();
    fs::write(mod_dir.join("mod.ini"), "[Section]\n").unwrap();

    let sub_under_mod = mod_dir.join("SubModUnder");
    fs::create_dir_all(&sub_under_mod).unwrap();
    fs::write(sub_under_mod.join("another.ini"), "[Another]\n").unwrap();

    let result = scan_mods_light(dir.path()).unwrap();
    let mod_names: Vec<&str> = result.mods.iter().map(|m| m.name.as_str()).collect();
    assert!(mod_names.contains(&"ModWithIni"));
    assert!(
        !mod_names.contains(&"SubModUnder"),
        "Should not recurse into mod dir with ini"
    );
}

/// 测试：轻量扫描正确识别 NormalGroup 和 MutexGroup 中的禁用模组
#[test]
fn test_light_scan_disabled_both_types() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let group_path = create_group_dir(dir.path(), "group_1");
    let mod_enabled = group_path.join("EnabledMod");
    fs::create_dir_all(&mod_enabled).unwrap();
    let mod_disabled = group_path.join("DISABLEDDisabledMod");
    fs::create_dir_all(&mod_disabled).unwrap();

    let mutex_path = dir.path().join("_MANAGED_").join("#Mutex");
    fs::create_dir_all(&mutex_path).unwrap();
    let m_enabled = mutex_path.join("MutexEnabled");
    fs::create_dir_all(&m_enabled).unwrap();
    fs::write(m_enabled.join("m.ini"), "").unwrap();
    let m_disabled = mutex_path.join("disabled_MutexDisabled");
    fs::create_dir_all(&m_disabled).unwrap();
    fs::write(m_disabled.join("m.ini"), "").unwrap();

    let result = scan_mods_light(dir.path()).unwrap();

    for m in &result.mods {
        if m.name.contains("Disabled") {
            assert!(
                m.disabled || m.name == "None",
                "{} should be disabled",
                m.name
            );
        } else if m.name != "None" {
            assert!(!m.disabled, "{} should be enabled", m.name);
        }
    }
}

/// 测试：轻量扫描正确识别 NormalGroup 和 MutexGroup 中的收藏模组
#[test]
fn test_light_scan_fav_both_types() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let group_path = create_group_dir(dir.path(), "group_1");
    let mod_fav = group_path.join("FavMod");
    fs::create_dir_all(&mod_fav).unwrap();
    fs::write(mod_fav.join("fav"), "").unwrap();
    let mod_normal = group_path.join("NormalMod");
    fs::create_dir_all(&mod_normal).unwrap();

    let mutex_path = dir.path().join("_MANAGED_").join("#Mutex");
    fs::create_dir_all(&mutex_path).unwrap();
    let m_fav = mutex_path.join("MutexFav");
    fs::create_dir_all(&m_fav).unwrap();
    fs::write(m_fav.join("m.ini"), "").unwrap();
    fs::write(m_fav.join("fav"), "").unwrap();
    let m_normal = mutex_path.join("MutexNormal");
    fs::create_dir_all(&m_normal).unwrap();
    fs::write(m_normal.join("m.ini"), "").unwrap();

    let result = scan_mods_light(dir.path()).unwrap();

    for m in &result.mods {
        if m.name.contains("Fav") {
            assert!(m.is_favorite, "{} should be favorite", m.name);
        } else if m.name != "None" {
            assert!(!m.is_favorite, "{} should not be favorite", m.name);
        }
    }
}

/// 测试：轻量扫描图标查找的优先级顺序
#[test]
fn test_light_scan_icon_priority() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");

    let mod_path = group_path.join("IconTest");
    fs::create_dir_all(&mod_path).unwrap();
    fs::write(mod_path.join("preview.png"), b"p").unwrap();
    fs::write(mod_path.join("other.png"), b"o").unwrap();
    fs::write(mod_path.join("icon.png"), b"i").unwrap();

    let result = scan_mods_light(dir.path()).unwrap();
    let m = result.mods.iter().find(|m| m.name == "IconTest").unwrap();
    assert!(m.preview_image_path.is_some());
    assert!(m.preview_image_path.as_ref().unwrap().ends_with("icon.png"));
}

/// 测试：轻量扫描混合 NormalGroup 和 MutexGroup 并存
#[test]
fn test_light_scan_mixed_groups() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let g1 = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&g1, "NormalMod", "[S]\n");

    let mutex_path = dir.path().join("_MANAGED_").join("#MutexMods");
    fs::create_dir_all(&mutex_path).unwrap();
    create_mod_with_ini(&mutex_path, "MutexMod", "[S]\n");

    let result = scan_mods_light(dir.path()).unwrap();

    let group_types: Vec<GroupType> = result.groups.iter().map(|g| g.group_type).collect();
    assert!(group_types.contains(&GroupType::NormalGroup));
    assert!(group_types.contains(&GroupType::MutexGroup));
    assert_eq!(result.groups.len(), 2);
}

/// 测试：轻量扫描严格拒绝 `group_0` 和 `group_01` 作为 NormalGroup
#[test]
fn test_light_scan_group_regex_strict() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let g0 = dir.path().join("_MANAGED_").join("group_0");
    fs::create_dir_all(&g0).unwrap();
    create_mod_with_ini(&g0, "Mod0", "[S]\n");

    let g01 = dir.path().join("_MANAGED_").join("group_01");
    fs::create_dir_all(&g01).unwrap();
    create_mod_with_ini(&g01, "Mod01", "[S]\n");

    let g1 = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&g1, "Mod1", "[S]\n");

    let result = scan_mods_light(dir.path()).unwrap();
    let group_names: Vec<&str> = result
        .groups
        .iter()
        .filter(|g| g.group_type == GroupType::NormalGroup)
        .map(|g| g.group_name.as_str())
        .collect();

    assert!(
        group_names.contains(&"group_1"),
        "group_1 should be NormalGroup"
    );
    let mutex_names: Vec<&str> = result
        .groups
        .iter()
        .filter(|g| g.group_type == GroupType::MutexGroup)
        .map(|g| g.group_name.as_str())
        .collect();
    assert!(
        mutex_names.contains(&"group_0"),
        "group_0 should be MutexGroup"
    );
    assert!(
        mutex_names.contains(&"group_01"),
        "group_01 should be MutexGroup"
    );
}

/// 测试：轻量扫描排序规则（启用优先 > 收藏优先 > 自然排序）
#[test]
fn test_light_scan_sorting() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");

    let m10 = group_path.join("mod10");
    fs::create_dir_all(&m10).unwrap();
    let m2 = group_path.join("mod2");
    fs::create_dir_all(&m2).unwrap();
    let m_disabled = group_path.join("DISABLEDbad");
    fs::create_dir_all(&m_disabled).unwrap();
    let m_fav = group_path.join("fav_mod");
    fs::create_dir_all(&m_fav).unwrap();
    fs::write(m_fav.join("fav"), "").unwrap();

    let result = scan_mods_light(dir.path()).unwrap();
    let group = &result.groups[0];

    assert_eq!(group.mods[0].name, "None");
    assert_eq!(group.mods[1].name, "fav_mod");
    assert_eq!(group.mods[2].name, "mod2");
    assert_eq!(group.mods[3].name, "mod10");
    assert_eq!(group.mods[4].name, "bad");
    assert_eq!(group.mods.len(), 5);
}

/// 测试（R3/C-3b）：轻量扫描为模组填充磁盘签名。
#[test]
fn test_light_scan_disk_signature_sources() {
    let _window_lock = TEST_WINDOW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&group_path, "ModA", "[Section]\n");
    create_mod_with_ini(&group_path, "FavMod", "[Section]\n");
    let mutex_root = dir.path().join("_MANAGED_").join("#Mutex");
    fs::create_dir_all(mutex_root.join("ModX")).unwrap();
    fs::write(mutex_root.join("ModX").join("m.ini"), "[Section]\n").unwrap();

    let find_sig = |r: &ScanResult, name: &str| {
        r.mods
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("应存在模组 {name}"))
            .disk_signature
    };

    let r1 = scan_mods_light(dir.path()).unwrap();
    let r2 = scan_mods_light(dir.path()).unwrap();
    assert_ne!(find_sig(&r2, "ModA"), 0, "NormalGroup 模组签名应非零");
    assert_ne!(find_sig(&r2, "ModX"), 0, "MutexGroup 模组签名应非零");
    assert_eq!(
        find_sig(&r1, "ModA"),
        find_sig(&r2, "ModA"),
        "无磁盘变化时两次扫描签名应相等"
    );
    assert_eq!(find_sig(&r1, "ModX"), find_sig(&r2, "ModX"));

    fs::write(group_path.join("FavMod").join("fav"), "").unwrap();
    let r3 = scan_mods_light(dir.path()).unwrap();
    assert_ne!(
        find_sig(&r2, "FavMod"),
        find_sig(&r3, "FavMod"),
        "fav 创建（mtime 源 0→t）应使签名失效"
    );
    assert_eq!(
        find_sig(&r2, "ModA"),
        find_sig(&r3, "ModA"),
        "未动模组签名应保持稳定"
    );

    fs::write(group_path.join("selectedindex"), "1").unwrap();
    let r4 = scan_mods_light(dir.path()).unwrap();
    assert_ne!(
        find_sig(&r3, "ModA"),
        find_sig(&r4, "ModA"),
        "selectedindex mtime 源应参与签名（覆盖 is_active 派生源）"
    );
    assert_eq!(find_sig(&r3, "ModX"), find_sig(&r4, "ModX"));

    let deep = scan_mods_deep(dir.path()).unwrap();
    assert!(
        deep.mods.iter().all(|m| m.disk_signature == 0),
        "深扫产物签名应为 0（无签名 → 永不复用）"
    );
}

#[cfg(test)]
mod tests_non_group_no_markers {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// 扫描 mutexGroup（非group）不产生 groupname/modname 文件
    #[test]
    fn test_scan_mutex_no_markers_created() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let non_group = mods_root.join("_MANAGED_").join("#CustomGroup");
        fs::create_dir_all(&non_group).unwrap();
        let mod_dir = non_group.join("ModA");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("ModA.ini"), "[ShaderOverride]\n").unwrap();

        let result = scan_mods_light(mods_root).expect("scan should succeed");

        assert!(
            !non_group.join("groupname").exists(),
            "非group不应创建 groupname 文件"
        );
        assert!(
            !mod_dir.join("modname").exists(),
            "非group下模组不应创建 modname 文件"
        );

        let mod_names: Vec<&str> = result.mods.iter().map(|m| m.name.as_str()).collect();
        assert!(mod_names.contains(&"ModA"), "ModA 应被扫描到，名称为 ModA");
    }

    /// 扫描 group_1：正常语义下 groupname / modname 标记文件均创建
    #[test]
    fn test_scan_group_xx_still_creates_markers() {
        let _window_lock = TEST_WINDOW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join(constants::MANAGED_FOLDER);
        let group1 = managed.join("group_1");
        fs::create_dir_all(&group1).unwrap();
        let mod_dir = group1.join("ModA");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("ModA.ini"), "[ShaderOverride]\n").unwrap();

        let _result = scan_mods_light(mods_root).expect("scan should succeed");

        assert!(
            group1.join("groupname").exists(),
            "group_xx 应创建 groupname 文件"
        );
        assert!(
            mod_dir.join("modname").exists(),
            "group_xx 下模组应创建 modname 文件"
        );
    }
}

// ============================================================================
// scan_single_mod 单元测试（C-3a 单模组扫描）
// ============================================================================

#[cfg(test)]
mod tests_scan_single_mod {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// 测试辅助：创建带 `_MANAGED_` 子目录的临时 Mods 根
    fn setup() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("_MANAGED_")).unwrap();
        dir
    }

    /// 测试辅助：创建带 ini 的模组目录（可选 fav/modname 标记）
    fn make_mod(parent: &Path, name: &str) -> PathBuf {
        let p = parent.join(name);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("mod.ini"), "[Section]\nx=1\n").unwrap();
        p
    }

    /// 单模组扫描产出与组级扫描同口径的磁盘派生字段
    #[test]
    fn scan_single_mod_normal_group_fields() {
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "ModA");
        let mod_b = make_mod(&g1, "DISABLED_ModB");
        fs::write(mod_a.join("modname"), "My Display Name\n").unwrap();
        fs::write(mod_b.join(constants::FAV_MARKER), "").unwrap();

        let (arc, group_dir) = scan_single_mod(dir.path(), &mod_a).unwrap();
        assert_eq!(group_dir, g1, "组目录应为 mod_dir.parent()");
        assert_eq!(arc.name, "My Display Name", "modname 标记内容优先");
        assert_eq!(arc.group_index, 1);
        assert!(!arc.is_mutex, "普通组模组 is_mutex=false");
        assert!(arc.is_active, "唯一启用模组应为 active");
        assert!(!arc.disabled && !arc.mod_disabled);
        assert!(!arc.is_favorite);
        assert!(arc.disk_signature != 0, "磁盘签名非 0（可参与复用判定）");
        assert_eq!(
            arc.mod_path,
            mod_a.to_string_lossy().to_string(),
            "full_path/mod_path 为归一（mods_path 基准）形态"
        );

        let (arc_b, _) = scan_single_mod(dir.path(), &mod_b).unwrap();
        assert!(arc_b.disabled && arc_b.mod_disabled, "DISABLED_ 前缀解析");
        assert_eq!(arc_b.name, "ModB", "禁用模组展示名剥离前缀");
        assert!(arc_b.is_favorite, "fav 标记存在");
        assert!(!arc_b.is_active, "禁用模组不 active");
    }

    /// modname 标记不存在时创建并回写目录名（NRMM 对齐）
    #[test]
    fn scan_single_mod_creates_modname_marker() {
        let _window_lock = TEST_WINDOW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "FreshMod");
        assert!(!mod_a.join("modname").exists());

        let (arc, _) = scan_single_mod(dir.path(), &mod_a).unwrap();
        assert_eq!(arc.name, "FreshMod", "无 modname 时以目录名为展示名");
        assert!(
            mod_a.join("modname").exists(),
            "scan_single_mod 应创建 modname 标记"
        );
    }

    /// is_active 组语义近似：多个启用模组（默认 selectedindex=0 → None 槽位 active）
    #[test]
    fn scan_single_mod_is_active_with_multiple_enabled() {
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "ModA");
        let mod_b = make_mod(&g1, "ModB");

        let (arc_a, _) = scan_single_mod(dir.path(), &mod_a).unwrap();
        let (arc_b, _) = scan_single_mod(dir.path(), &mod_b).unwrap();
        assert!(
            !arc_a.is_active && !arc_b.is_active,
            "多启用（默认 selectedindex=0 → None active）时真实模组均非 active"
        );

        let group_scan = scan_partial_path(dir.path(), &g1).unwrap();
        let gm = &group_scan.groups[0];
        assert_eq!(gm.active_mod_index, 0, "整组重扫：None 槽位 active");
        assert!(
            gm.mods
                .iter()
                .filter(|m| m.name != "None")
                .all(|m| !m.is_active),
            "整组重扫：真实模组均非 active"
        );
    }

    /// 互斥组模组（含深层嵌套）：is_active 恒 false、is_mutex=true
    #[test]
    fn scan_single_mod_mutex_mod_always_inactive() {
        let dir = setup();
        let mutex_root = dir.path().join("_MANAGED_").join("#Mutex");
        let sub = mutex_root.join("Sub");
        let mod_x = make_mod(&sub, "ModX");
        let mod_root_level = make_mod(&mutex_root, "ModY");

        let (arc_x, group_dir_x) = scan_single_mod(dir.path(), &mod_x).unwrap();
        assert_eq!(group_dir_x, sub, "嵌套模组的所属组目录为其直接父目录");
        assert!(arc_x.is_mutex, "互斥组模组 is_mutex=true");
        assert!(!arc_x.is_active, "互斥组模组 is_active 恒 false");

        let (arc_y, group_dir_y) = scan_single_mod(dir.path(), &mod_root_level).unwrap();
        assert_eq!(group_dir_y, mutex_root);
        assert!(arc_y.is_mutex && !arc_y.is_active);
    }

    /// 非 `_MANAGED_` 子树路径 / 无 .ini 目录 → Err（调用方降级组级重扫）
    #[test]
    fn scan_single_mod_rejects_invalid_targets() {
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "ModA");

        let no_ini = g1.join("NoIni");
        fs::create_dir_all(&no_ini).unwrap();
        assert!(scan_single_mod(dir.path(), &no_ini).is_err());

        assert!(scan_single_mod(dir.path(), &dir.path().join("_MANAGED_")).is_err());

        let outside = dir.path().join("Outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("x.ini"), "[S]\n").unwrap();
        assert!(scan_single_mod(dir.path(), &outside).is_err());

        assert!(scan_single_mod(dir.path(), &mod_a).is_ok());
    }

    /// 单模组扫描的磁盘派生字段与整组扫描产出一致（字段级子集）
    #[test]
    fn scan_single_mod_matches_group_scan_fields() {
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "ModA");
        make_mod(&g1, "DISABLED_ModB");

        let group_scan = scan_partial_path(dir.path(), &g1).unwrap();
        let group_mod_a = group_scan.groups[0]
            .mods
            .iter()
            .find(|m| m.name == "ModA")
            .unwrap()
            .clone();

        let (single, _) = scan_single_mod(dir.path(), &mod_a).unwrap();
        assert_eq!(single.name, group_mod_a.name);
        assert_eq!(single.disabled, group_mod_a.disabled);
        assert_eq!(single.mod_disabled, group_mod_a.mod_disabled);
        assert_eq!(single.is_favorite, group_mod_a.is_favorite);
        assert_eq!(single.is_namespaced, group_mod_a.is_namespaced);
        assert_eq!(single.is_mutex, group_mod_a.is_mutex);
        assert_eq!(single.group_index, group_mod_a.group_index);
        assert_eq!(
            single.full_path.as_ref(),
            group_mod_a.full_path.as_ref(),
            "full_path 与组扫描一致（缓存 patch 位置匹配前提）"
        );
        assert_eq!(single.is_active, group_mod_a.is_active);
        assert_eq!(single.disk_signature, group_mod_a.disk_signature);
    }
}
