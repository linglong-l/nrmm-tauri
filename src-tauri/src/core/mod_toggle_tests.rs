
use super::*;
use tempfile::TempDir;

fn create_test_mod(parent: &Path, name: &str) -> PathBuf {
    let mod_path = parent.join(name);
    fs::create_dir_all(&mod_path).unwrap();
    fs::write(mod_path.join("mod.ini"), "").unwrap();
    mod_path
}

#[test]
fn test_toggle_mod_enable_disable() {
    let dir = TempDir::new().unwrap();
    let enabled_path = dir.path().join("MyMod");
    fs::create_dir_all(&enabled_path).unwrap();

    toggle_mod(&enabled_path, false).unwrap();
    let disabled_name = enabled_path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let new_path = dir.path().join(format!("DISABLED{}", disabled_name));
    assert!(new_path.exists());
    assert!(!enabled_path.exists());

    toggle_mod(&new_path, true).unwrap();
    assert!(enabled_path.exists());
    assert!(!new_path.exists());
}

#[test]
fn test_toggle_mod_already_in_state() {
    let dir = TempDir::new().unwrap();

    let enabled_path = dir.path().join("EnabledMod");
    fs::create_dir_all(&enabled_path).unwrap();
    toggle_mod(&enabled_path, true).unwrap();
    assert!(enabled_path.exists());

    let disabled_path = dir.path().join("DISABLEDDisabledMod");
    fs::create_dir_all(&disabled_path).unwrap();
    toggle_mod(&disabled_path, false).unwrap();
    assert!(disabled_path.exists());
}

#[test]
fn test_enable_mutex_mod_disables_siblings() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join("MutexGroup");
    fs::create_dir_all(&parent).unwrap();

    let mod1 = create_test_mod(&parent, "DISABLEDModA");
    let mod2 = create_test_mod(&parent, "ModB");
    let mod3 = create_test_mod(&parent, "ModC");

    let target_mod = parent.join("ModA");
    enable_mutex_mod(&mod1).unwrap();

    assert!(target_mod.exists());
    assert!(!mod1.exists());
    assert!(!mod2.exists());
    assert!(parent.join("DISABLEDModB").exists());
    assert!(!mod3.exists());
    assert!(parent.join("DISABLEDModC").exists());
}

#[test]
fn test_enable_mutex_idempotent() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join("MutexGroup");
    fs::create_dir_all(&parent).unwrap();

    let mod1 = create_test_mod(&parent, "ModA");
    let _mod2 = create_test_mod(&parent, "DISABLEDModB");
    let _mod3 = create_test_mod(&parent, "DISABLEDModC");

    enable_mutex_mod(&mod1).unwrap();

    assert!(mod1.exists());
    assert!(parent.join("DISABLEDModB").exists());
    assert!(parent.join("DISABLEDModC").exists());

    enable_mutex_mod(&mod1).unwrap();

    assert!(mod1.exists());
    assert!(parent.join("DISABLEDModB").exists());
    assert!(parent.join("DISABLEDModC").exists());
}

#[test]
fn test_disable_mutex_mod() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join("MutexGroup");
    fs::create_dir_all(&parent).unwrap();

    let mod_path = create_test_mod(&parent, "MyMod");
    assert!(mod_path.exists());

    disable_mutex_mod(&mod_path).unwrap();

    assert!(!mod_path.exists());
    assert!(parent.join("DISABLEDMyMod").exists());
}

#[test]
fn test_disable_mutex_idempotent() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join("MutexGroup");
    fs::create_dir_all(&parent).unwrap();

    let disabled_path = create_test_mod(&parent, "DISABLEDMyMod");
    assert!(disabled_path.exists());

    disable_mutex_mod(&disabled_path).unwrap();

    assert!(disabled_path.exists());
}

#[test]
fn test_mutex_does_not_affect_subgroups() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join("MutexGroup");
    fs::create_dir_all(&parent).unwrap();

    let mod1 = create_test_mod(&parent, "ModA");
    let mod2 = create_test_mod(&parent, "ModB");

    let subgroup = parent.join("SubGroup");
    fs::create_dir_all(&subgroup).unwrap();
    let submod = create_test_mod(&subgroup, "SubMod1");
    let submod2 = create_test_mod(&subgroup, "SubMod2");

    enable_mutex_mod(&mod1).unwrap();

    assert!(mod1.exists());
    assert!(!mod2.exists());
    assert!(parent.join("DISABLEDModB").exists());
    assert!(submod.exists());
    assert!(submod2.exists());
}

#[test]
fn test_is_mutex_mod_detection() {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();

    let normal_group = managed.join("group_1");
    fs::create_dir_all(&normal_group).unwrap();
    let normal_mod = create_test_mod(&normal_group, "NormalMod");

    let mutex_group = managed.join("#MutexMods");
    fs::create_dir_all(&mutex_group).unwrap();
    let mutex_mod = create_test_mod(&mutex_group, "MutexMod");

    assert!(!is_mutex_mod(&normal_mod, &managed));
    assert!(is_mutex_mod(&mutex_mod, &managed));
}

/// 测试：toggle_mod 目标为 `DISABLEDDISABLEDMod` 时启用应一次移除全部前缀 → `Mod`
#[test]
fn test_toggle_mod_enable_removes_all_prefixes() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path();
    let mod_path = parent.join("DISABLEDDISABLEDMod");
    fs::create_dir_all(&mod_path).unwrap();
    fs::write(mod_path.join("mod.ini"), "[ShaderOverride]").unwrap();

    toggle_mod(&mod_path, true).unwrap();

    assert!(!mod_path.exists(), "原 DISABLEDDISABLEDMod 目录应被重命名");
    assert!(
        parent.join("Mod").exists(),
        "启用后应为 Mod（一次移除全部连续前缀）"
    );
}

/// 测试：toggle_mod 对已禁用目录重复禁用不重复加前缀（幂等）
#[test]
fn test_toggle_mod_disable_idempotent_no_extra_prefix() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path();
    let mod_path = parent.join("DISABLEDMod");
    fs::create_dir_all(&mod_path).unwrap();
    fs::write(mod_path.join("mod.ini"), "[ShaderOverride]").unwrap();

    // 已禁用 → 应幂等跳过，不新增前缀
    toggle_mod(&mod_path, false).unwrap();

    let entries: Vec<String> = fs::read_dir(parent)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        entries.iter().any(|n| n == "DISABLEDMod"),
        "目录仍应为 DISABLEDMod"
    );
    assert!(
        entries.iter().all(|n| !n.starts_with("DISABLEDDISABLED")),
        "不应重复加前缀产生 DISABLEDDISABLEDxxx"
    );
}

/// 测试：enable_mutex_mod 目标匹配大小写不敏感，目标正确启用、兄弟正确禁用
///
/// Windows 下路径大小写不敏感，用大小写差异的 mod_path 调用时目标应被识别并启用，
/// 而非被误判为"兄弟"禁用（缺陷 B 回归测试）。
#[test]
fn test_enable_mutex_mod_case_insensitive_target_match() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path();
    // 目标 ModA、兄弟 ModB（均含 .ini）
    let mod_a = parent.join("ModA");
    fs::create_dir_all(&mod_a).unwrap();
    fs::write(mod_a.join("mod.ini"), "[ShaderOverride]").unwrap();
    let mod_b = parent.join("ModB");
    fs::create_dir_all(&mod_b).unwrap();
    fs::write(mod_b.join("mod.ini"), "[ShaderOverride]").unwrap();

    // 以大小写差异的路径调用（Windows 下视为同一目录）
    let target = parent.join("moda");
    enable_mutex_mod(&target).unwrap();

    assert!(
        parent.join("ModA").exists(),
        "目标 ModA 应被识别并保持启用（未被改名）"
    );
    assert!(
        parent.join("DISABLEDModB").exists(),
        "兄弟 ModB 应被禁用（加 DISABLED 前缀）"
    );
}

/// 测试：rename_mod 互斥分支 final_name 前缀防重复
///
/// 复刻 mod_commands::rename_mod 的 core 逻辑（validate + spawn_blocking 为 Tauri 命令
/// 封装不便直接调用），验证 new_name 带 DISABLED 前缀时最终只保有一个前缀。
#[test]
fn test_rename_mod_final_name_no_duplicate_prefix() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path();
    let mod_path = parent.join("DISABLEDMod");
    fs::create_dir_all(&mod_path).unwrap();
    fs::write(mod_path.join("mod.ini"), "[ShaderOverride]").unwrap();

    // 危险场景：用户重命名 new_name 已带 DISABLED 前缀
    let is_disabled =
        crate::core::mod_scan_util::is_disabled_dir(&mod_path.file_name().unwrap().to_string_lossy());
    let new_name = "DISABLEDNewName".to_string();
    let final_name = if is_disabled {
        format!(
            "{}{}",
            constants::DISABLED_PREFIX,
            crate::core::mod_scan_util::strip_disabled_prefix(&new_name)
        )
    } else {
        new_name
    };

    assert_eq!(
        final_name, "DISABLEDNewName",
        "重命名结果应只含一个 DISABLED 前缀，而非 DISABLEDDISABLEDNewName"
    );
}
