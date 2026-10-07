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
///
/// 断言依赖 Windows 文件系统语义，故仅在 Windows 运行；类 Unix 宿主的对应行为见
/// `test_enable_mutex_mod_case_sensitive_target_rejected`。
#[cfg(windows)]
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
    let is_disabled = crate::core::mod_scan_util::is_disabled_dir(
        &mod_path.file_name().unwrap().to_string_lossy(),
    );
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

/// 读取目录下全部条目名并按字节序排序（不依赖目录枚举顺序）
fn child_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

/// 缺陷回归（核心断言）：目标未命中时报错，且兄弟目录一个都不许改名
///
/// 修复前：`mod_path` 指向已消失/拼错的目录时，循环内无任何条目命中目标，
/// 父目录下所有含 .ini 的兄弟会被成批加 `DISABLED` 前缀并沉默返回 `Ok(())`。
#[test]
fn test_enable_mutex_mod_missing_target_errors_and_keeps_siblings() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join("MutexGroup");
    fs::create_dir_all(&parent).unwrap();

    let _mod_a = create_test_mod(&parent, "ModA");
    let _mod_b = create_test_mod(&parent, "ModB");
    let _mod_c = create_test_mod(&parent, "DISABLEDModC");

    // 目标目录不存在
    let target = parent.join("ModGone");
    let err = enable_mutex_mod(&target).expect_err("目标未命中应返回 Err 而非沉默成功");

    assert!(
        err.to_string().starts_with("[PATH_NOT_FOUND]"),
        "错误串应带前端可识别前缀，实际: {err}"
    );
    assert_eq!(
        child_names(&parent),
        vec!["DISABLEDModC", "ModA", "ModB"],
        "目标未命中时不得发生任何改名"
    );
}

/// 缺陷回归：目标存在但无 .ini（分组节点）同样视为未命中，兄弟不受影响
#[test]
fn test_enable_mutex_mod_target_without_ini_is_rejected() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join("MutexGroup");
    fs::create_dir_all(&parent).unwrap();

    let _mod_a = create_test_mod(&parent, "ModA");
    let _mod_b = create_test_mod(&parent, "ModB");
    // 分组节点：是目录、非隐藏，但不含 .ini
    let group_node = parent.join("SubGroup");
    fs::create_dir_all(&group_node).unwrap();

    let err = enable_mutex_mod(&group_node).expect_err("无 .ini 的分组节点不得被当作互斥目标");

    assert!(
        err.to_string().starts_with("[PATH_NOT_FOUND]"),
        "错误串应带前端可识别前缀，实际: {err}"
    );
    assert_eq!(
        child_names(&parent),
        vec!["ModA", "ModB", "SubGroup"],
        "目标未命中时不得发生任何改名"
    );
}

/// 类 Unix 宿主语义：仅大小写不同的目标不是同一个目录 → 未命中报错、兄弟不动
///
/// Windows 侧「大小写视为同一目录并正确命中」的行为见
/// `test_enable_mutex_mod_case_insensitive_target_match`。
#[cfg(not(windows))]
#[test]
fn test_enable_mutex_mod_case_sensitive_target_rejected() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path();

    let _mod_a = create_test_mod(parent, "ModA");
    let _mod_b = create_test_mod(parent, "ModB");

    let err = enable_mutex_mod(&parent.join("moda"))
        .expect_err("大小写敏感宿主上，moda 与 ModA 不是同一目录");

    assert!(
        err.to_string().starts_with("[PATH_NOT_FOUND]"),
        "错误串应带前端可识别前缀，实际: {err}"
    );
    assert_eq!(
        child_names(parent),
        vec!["ModA", "ModB"],
        "未命中时不得成批禁用兄弟"
    );
}

/// 既有行为保留：改名后的目标名已被占用 → 只 warn 跳过、不改名，整体仍返回 Ok
///
/// 场景中 `DISABLEDModA`（目标）去前缀得 `ModA` 已存在，`ModA`（兄弟）加前缀得
/// `DISABLEDModA` 亦已存在——两条都跳过，磁盘状态不变。
#[test]
fn test_enable_mutex_mod_name_taken_skips_without_rename() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join("MutexGroup");
    fs::create_dir_all(&parent).unwrap();

    let target = create_test_mod(&parent, "DISABLEDModA");
    let sibling = create_test_mod(&parent, "ModA");

    enable_mutex_mod(&target).unwrap();

    assert!(target.exists(), "目标保持原名（去前缀会与兄弟重名）");
    assert!(sibling.exists(), "兄弟保持原名（加前缀会与目标重名）");
    assert_eq!(child_names(&parent), vec!["DISABLEDModA", "ModA"]);
}

/// 缺陷回归（原子性）：提交中途失败 → 逆序回滚已成功项，磁盘回到调用前状态
///
/// 以「第二条动作的源目录不存在」稳定模拟中途失败（权限/磁盘满/名字冲突同理），
/// 宿主无关。
#[test]
fn test_commit_mutex_plan_rolls_back_on_failure() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path();

    let mod_a = create_test_mod(parent, "ModA");
    let mod_b = create_test_mod(parent, "ModB");

    let plan = vec![
        RenameOp {
            from: mod_a.clone(),
            to: parent.join("DISABLEDModA"),
            desc: "test rename",
        },
        RenameOp {
            from: parent.join("GhostMod"),
            to: parent.join("DISABLEDGhostMod"),
            desc: "test rename",
        },
    ];

    let err = commit_mutex_plan(&plan).expect_err("第二条 rename 应失败");
    assert!(
        err.to_string().contains("GhostMod"),
        "应返回原始错误而非回滚错误: {err}"
    );

    assert!(
        !parent.join("DISABLEDModA").exists(),
        "回滚应撤销第一条已成功的改名"
    );
    assert!(mod_a.exists(), "ModA 应回到调用前状态");
    assert!(mod_b.exists(), "未纳入计划的条目不受影响");
}

/// 预检：计划内目标名重复、或目标名命中另一动作的现名 → 整体拒绝执行
#[test]
fn test_precheck_mutex_plan_rejects_target_conflicts() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path();
    let shared_target = parent.join("DISABLEDModX");

    let duplicated = vec![
        RenameOp {
            from: parent.join("ModA"),
            to: shared_target.clone(),
            desc: "test",
        },
        RenameOp {
            from: parent.join("ModB"),
            to: shared_target,
            desc: "test",
        },
    ];
    assert!(
        precheck_mutex_plan(&duplicated).is_err(),
        "两个动作争抢同一目标名必须被拒绝"
    );

    let overwrite = vec![
        RenameOp {
            from: parent.join("ModA"),
            to: parent.join("ModB"),
            desc: "test",
        },
        RenameOp {
            from: parent.join("ModB"),
            to: parent.join("ModC"),
            desc: "test",
        },
    ];
    assert!(
        precheck_mutex_plan(&overwrite).is_err(),
        "目标名命中另一动作的现名（结果依赖执行顺序）必须被拒绝"
    );
}

/// 前缀约定：core 侧字面量必须与 commands 层 `[PATH_NOT_FOUND]` 一致
///
/// core 不得反向依赖 commands（arch_gate F1），无法共享常量，故在此锁定字面量防漂移。
#[test]
fn test_err_path_not_found_prefix_matches_convention() {
    assert_eq!(ERR_PATH_NOT_FOUND, "[PATH_NOT_FOUND]");
}
