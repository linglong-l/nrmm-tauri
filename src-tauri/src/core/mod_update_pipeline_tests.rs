use super::*;
use std::fs;
use std::time::SystemTime;
use tempfile::TempDir;
use walkdir::WalkDir;

fn setup_test_env() -> TempDir {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();
    dir
}

fn setup_test_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();
    dir
}

fn create_group_dir(base: &Path, group_name: &str) -> PathBuf {
    let group_path = base.join("_MANAGED_").join(group_name);
    fs::create_dir_all(&group_path).unwrap();
    group_path
}

fn create_mod_with_ini(group_path: &Path, mod_name: &str, ini_content: &str) -> PathBuf {
    let mod_path = group_path.join(mod_name);
    fs::create_dir_all(&mod_path).unwrap();
    let ini_path = mod_path.join("mod.ini");
    fs::write(&ini_path, ini_content).unwrap();
    mod_path
}

fn create_main_ini(base: &Path, game: TargetGame) {
    let ini_name = game.d3dx_ini_name();
    // 包含 [Include] 段的 include_recursive，对齐标准 XXMI 环境
    // 使用实际目录名确保 detect_include_recursive 返回 true，不注入 ;NRMM_INI_START 块
    let dir_name = base.file_name().and_then(|n| n.to_str()).unwrap_or("Mods");
    let content = format!(
        "; original main ini\n[Include]\ninclude_recursive = {}\n",
        dir_name
    );
    fs::write(base.join(ini_name), content).unwrap();
}

fn create_d3dx_ini(base: &Path) {
    fs::write(base.join("d3dx.ini"), "; test\n").unwrap();
}

/// 递归收集目录下所有 .ini 文件内容（路径 → 内容，BTreeMap 保证迭代顺序稳定）
fn snapshot_ini_files(root: &Path) -> std::collections::BTreeMap<PathBuf, String> {
    let mut map = std::collections::BTreeMap::new();
    for entry in WalkDir::new(root).into_iter().flatten() {
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("ini") {
            map.insert(p.to_path_buf(), fs::read_to_string(p).unwrap_or_default());
        }
    }
    map
}

/// 测试：update_mod_data 对 # 目录（MutexGroup）零触碰（不读、不写、不遍历）
///
/// 构造 _MANAGED_/#MyMutex/MyMod/lib.ini（含库定义与崩溃行混合内容，记录
/// 写入时间戳），同时在 # 目录放置哨兵文件；运行 update_mod_data 后断言：
/// 1. # 目录内所有文件内容与 mtime 完全不变；
/// 2. # 目录内未新增任何文件（无标记文件、无备份、无 selectedindex）。
/// 验证 2026-08-18 用户决策：非 group_<int> 一级目录不参与数据更新。
#[test]
fn test_update_mod_data_does_not_touch_mutex_dirs() {
    let dir = TempDir::new().unwrap();
    let mods = dir.path().join("Mods");
    let managed = mods.join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();

    // # 互斥分组目录：模组 INI 含库定义 + 崩溃行（诱导注释写入）+ 哨兵文件
    let mutex_mod_dir = managed.join("#MyMutex").join("MyMod");
    fs::create_dir_all(&mutex_mod_dir).unwrap();
    let mutex_ini = mutex_mod_dir.join("lib.ini");
    let mutex_content = "[ResourceMyLib]\nhash = abc123\n[SomeSection]\ndrawindexed = crashval\n";
    fs::write(&mutex_ini, mutex_content).unwrap();
    let sentinel = mutex_mod_dir.join("sentinel.txt");
    fs::write(&sentinel, "keep").unwrap();

    // 同目录树的 group_1 正常分组（含一个启用模组），保证 update 有实际工作量
    let group_dir = managed.join("group_1");
    fs::create_dir_all(&group_dir).unwrap();
    fs::write(
        group_dir.join("group_1.ini"),
        ";template\n[Constants]\nglobal $group_id = 1\n",
    )
    .unwrap();
    let mod_dir = group_dir.join("EnabledMod");
    fs::create_dir_all(&mod_dir).unwrap();
    fs::write(
        mod_dir.join("mod.ini"),
        "[TextureOverride]\nhash = 0xAAAA\n",
    )
    .unwrap();

    // 记录 # 目录快照（路径集合 + 内容 + mtime）
    let snapshot: std::collections::BTreeMap<PathBuf, (String, SystemTime)> = {
        let mut m = std::collections::BTreeMap::new();
        for entry in WalkDir::new(&mutex_mod_dir).into_iter().flatten() {
            let p = entry.path();
            if p.is_file() {
                let content = fs::read_to_string(p).unwrap_or_default();
                let mtime = fs::metadata(p).and_then(|md| md.modified()).ok().unwrap();
                m.insert(p.to_path_buf(), (content, mtime));
            }
        }
        m
    };
    assert!(!snapshot.is_empty(), "快照应包含 # 目录下文件");

    let settings = AppSettings::default();
    update_mod_data(TargetGame::GenshinImpact, &mods, &settings).expect("update_mod_data 失败");

    // 断言 1：内容与 mtime 不变
    for (p, (content, mtime)) in &snapshot {
        let after = fs::read_to_string(p).unwrap_or_default();
        assert_eq!(&after, content, "# 目录文件内容不得被修改: {:?}", p);
        let after_mtime = fs::metadata(p).and_then(|md| md.modified()).ok().unwrap();
        assert_eq!(
            after_mtime, *mtime,
            "# 目录文件 mtime 不得变化（不得被读取后重写）: {:?}",
            p
        );
    }
    // 断言 2：未新增文件（无标记/备份/selectedindex 生成进 # 目录）
    let after_count = WalkDir::new(&mutex_mod_dir)
        .into_iter()
        .flatten()
        .filter(|e| e.path().is_file())
        .count();
    assert_eq!(
        after_count,
        snapshot.len(),
        "# 目录文件数量不得变化（不得新增任何文件）"
    );
}

#[test]
fn test_update_mod_data_empty() {
    let dir = setup_test_env();
    create_main_ini(dir.path(), TargetGame::GenshinImpact);
    let settings = AppSettings::default();

    let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
    assert_eq!(result.total_mods, 0);
    assert_eq!(result.enabled_mods, 0);
    assert_eq!(result.processed_mods, 0);
    assert!(result.errors.is_empty());

    let main_ini = dir.path().join("d3dx.ini");
    let content = fs::read_to_string(&main_ini).unwrap();
    // 对齐 NRMM 原版：d3dx.ini 不再注入 include 标记，依赖 include_recursive 自动加载
    assert!(!content.contains(";NRMM_INI_START"));
    assert!(!content.contains(";NRMM_INI_END"));
    assert!(!content.contains("nrmm_include.ini"));

    // 验证管理文件已创建
    let managed = dir.path().join("_MANAGED_");
    assert!(managed.join("nrmm_keypress.txt").exists());
    assert!(managed.join("nrmm_include.ini").exists());
    assert!(managed.join("manager_group.ini").exists());
}

#[test]
fn test_update_mod_data_with_mods() {
    let dir = setup_test_env();
    create_main_ini(dir.path(), TargetGame::GenshinImpact);
    let settings = AppSettings::default();

    let group_path = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(
        &group_path,
        "TestMod",
        "[TextureOverrideTest]\nhash = 0x12345678\nps-t0 = ResourceTest\ndrawindexed = auto\n",
    );

    let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
    assert_eq!(result.total_mods, 1);
    assert_eq!(result.enabled_mods, 1);
    assert_eq!(result.processed_mods, 1);

    // 验证 group_1.ini 已创建（对齐 NRMM：仅包含组模板，不包含显式 include）
    let group_ini = group_path.join("group_1.ini");
    assert!(group_ini.exists());
    let group_content = fs::read_to_string(&group_ini).unwrap();
    assert!(group_content.contains("group_1"));
    assert!(group_content.contains("$group_id = 1"));
    // NRMM 不生成显式 include 指令，模组由 include_recursive=Mods 自动加载
    assert!(!group_content.contains("; === NRMM Managed Includes ==="));

    // 验证 nrmm_include.ini 仅包含 IncludeKeypress（对齐 NRMM）
    let include_content =
        fs::read_to_string(dir.path().join("_MANAGED_/nrmm_include.ini")).unwrap();
    assert!(include_content.contains("[IncludeKeypress]"));
    assert!(include_content.contains("nrmm_keypress.txt"));
    assert!(!include_content.contains("manager_group.ini"));
    assert!(!include_content.contains("group_1/group_1.ini"));

    let mod_ini_path = group_path.join("TestMod/mod.ini");
    let backup_path = mod_ini_path.with_extension(constants::BACKUP_EXTENSION);
    assert!(backup_path.exists());
}

#[test]
fn test_update_mod_data_namespace_dedup() {
    // 引入原版语义：同组两个模组共享同一 namespace 时，其中一个应被重命名为唯一值
    let dir = setup_test_env();
    create_main_ini(dir.path(), TargetGame::GenshinImpact);
    let settings = AppSettings::default();

    let group_path = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(
        &group_path,
        "ModA",
        "namespace = Shared\n[Constants]\n$v = $Shared$Val\n",
    );
    create_mod_with_ini(
        &group_path,
        "ModB",
        "namespace = Shared\n[Constants]\n$w = $Shared$Other\n",
    );

    let _result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();

    let a_path = group_path.join("ModA/mod.ini");
    let b_path = group_path.join("ModB/mod.ini");
    let a = fs::read_to_string(&a_path).unwrap();
    let b = fs::read_to_string(&b_path).unwrap();

    let ns_of = |content: &str| -> Option<String> {
        for line in content.lines() {
            let t = line.trim();
            if t.to_lowercase().replace(' ', "").starts_with("namespace=") {
                if let Some(eq) = t.find('=') {
                    return Some(t[eq + 1..].trim().to_string());
                }
            }
        }
        None
    };
    let ns_a = ns_of(&a).unwrap();
    let ns_b = ns_of(&b).unwrap();

    // 去重后应为 {Shared, Shared_1}（顺序不定）
    let mut pair = [ns_a, ns_b];
    pair.sort();
    assert_eq!(pair, ["Shared", "Shared_1"]);

    // 每个模组内部的 namespace 引用应与自身声明一致（被 expand_ini_variables 展开为 $\ns\ 形式）
    for (content, ns) in [(&a, &pair[0]), (&b, &pair[1])] {
        let own_ref = format!("$\\{}\\", ns);
        let other_ns = if ns == "Shared" { "Shared_1" } else { "Shared" };
        let other_ref = format!("$\\{}\\", other_ns);
        assert!(
            content.contains(&own_ref),
            "模组 namespace={} 应包含自身引用 {}",
            ns,
            own_ref
        );
        assert!(
            !content.contains(&other_ref),
            "模组 namespace={} 不应残留对方引用 {}",
            ns,
            other_ref
        );
    }

    // 成功后不应残留 .baknamespace / .tmp
    assert!(!a_path.with_file_name("mod.ini.baknamespace").exists());
    assert!(!b_path.with_file_name("mod.ini.baknamespace").exists());
}

#[test]
fn test_update_mod_data_creates_default_ini() {
    // 对齐原版 NRMM：d3dx.ini 不在 Mods/ 内创建，位于游戏根目录（Mods 父目录）。
    // 无 d3dx.ini 时 update_mod_data 应优雅跳过主 INI 处理，不报错、不在 Mods/ 创建。
    let dir = setup_test_env();
    let settings = AppSettings::default();

    assert!(!dir.path().join("d3dx.ini").exists());
    let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
    assert_eq!(result.total_mods, 0);
    // d3dx.ini 不应在 Mods 目录内被创建
    assert!(!dir.path().join("d3dx.ini").exists());
    // 主 INI 缺失必须显式上报（而非静默跳过）
    assert_eq!(result.main_ini_status, MainIniStatus::MissingMainIni);
}

#[test]
fn test_update_mod_data_hsr_ini() {
    // 对齐原版 NRMM：全游戏统一 d3dx.ini（无 HSR RatioShot 特判），d3dx.ini 不在 Mods/ 内创建
    let dir = setup_test_env();
    let settings = AppSettings::default();

    let result = update_mod_data(TargetGame::HonkaiStarRail, dir.path(), &settings).unwrap();
    assert_eq!(result.total_mods, 0);
    assert!(!dir.path().join("d3dx.ini").exists());
    assert_eq!(result.main_ini_status, MainIniStatus::MissingMainIni);
}

// ============================================================================
// 步骤 5 按分组并行化（rayon）单元测试
// ============================================================================

/// 测试：多分组各自产生错误时，errors 合并顺序按 group_id 升序稳定（多次运行一致）
#[test]
fn test_parallel_merge_error_order_deterministic() {
    let dir = setup_test_env();
    create_main_ini(dir.path(), TargetGame::GenshinImpact);
    let settings = AppSettings::default();

    // 3 个分组，各含一个引用未知库的模组（触发 NON EXISTENT LIB，error_type=5）
    for gid in [1u32, 2, 3] {
        let group_path = create_group_dir(dir.path(), &format!("group_{}", gid));
        create_mod_with_ini(
            &group_path,
            "ModWithMissingLib",
            &format!(
                "[TextureOverrideTest]\nhash = 0x{0}\nrun = MissingLibG{0}\ndrawindexed = auto\n",
                gid
            ),
        );
    }

    let fmt_errors = |r: &UpdateResult| -> Vec<(u8, u32, String, String)> {
        r.errors
            .iter()
            .map(|e| {
                (
                    e.error_type,
                    e.line_number,
                    e.line.clone(),
                    e.error_message.clone(),
                )
            })
            .collect()
    };

    // 第一次运行完成内容注入；后续运行进入稳态，errors 应逐条一致（顺序确定）
    let _ = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
    let steady =
        fmt_errors(&update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap());
    let steady2 =
        fmt_errors(&update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap());
    assert_eq!(steady, steady2, "多次运行 errors 顺序应稳定");

    // 三组各产生一条 NON EXISTENT LIB 错误
    let nonexistent_count = steady.iter().filter(|(t, ..)| *t == 5).count();
    assert_eq!(nonexistent_count, 3, "每组应各产生一条缺失库错误");

    // 合并顺序按 group_id 升序：G1 的错误先于 G2，G2 先于 G3
    let pos = |name: &str| {
        steady
            .iter()
            .position(|(_, _, _, msg)| msg.contains(name))
            .unwrap_or_else(|| panic!("缺少包含 {} 的错误条目", name))
    };
    assert!(
        pos("MissingLibG1") < pos("MissingLibG2"),
        "group_1 错误应先于 group_2"
    );
    assert!(
        pos("MissingLibG2") < pos("MissingLibG3"),
        "group_2 错误应先于 group_3"
    );
}

/// 测试：单模组写入失败被隔离为错误条目，整体返回 Ok，同组其他模组正常处理且 group INI 生成。
///
/// 失败构造方式：占位 write_atomic 的临时文件路径（mod.ini → mod.ini.tmp）为目录，
/// 使 fs::File::create 必然失败。相比 Windows 只读属性（tmp+rename 策略可能绕过），
/// 该方式在 Windows/Linux 上均可可靠复现写入失败。
#[test]
fn test_write_failure_isolated_per_mod() {
    let dir = setup_test_env();
    create_main_ini(dir.path(), TargetGame::GenshinImpact);
    let settings = AppSettings::default();

    let group_path = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(
        &group_path,
        "GoodMod",
        "[TextureOverrideGood]\nhash = 0x1\ndrawindexed = auto\n",
    );
    let bad_mod_dir = create_mod_with_ini(
        &group_path,
        "BadMod",
        "[TextureOverrideBad]\nhash = 0x2\ndrawindexed = auto\n",
    );
    fs::create_dir_all(bad_mod_dir.join("mod.ini.tmp")).unwrap();

    let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();

    // 整体 Ok，仅 GoodMod 计入 processed
    assert_eq!(result.processed_mods, 1);
    // BadMod 写入失败被收集为错误条目（error_type=3，消息含路径）
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.error_type == 3 && e.error_message.contains("BadMod")),
        "应包含 BadMod 写入失败错误条目: {:?}",
        result.errors
    );
    // 同组其他 ini 正常处理（已注入 NRMM 头注释）且 group INI 生成
    let good = fs::read_to_string(group_path.join("GoodMod").join("mod.ini")).unwrap();
    assert!(good.contains("; \";-;\" are errored conditional lines."));
    assert!(group_path.join("group_1.ini").exists());
    // 失败 INI 未被写入（无 NRMM 头注释）
    let bad = fs::read_to_string(group_path.join("BadMod").join("mod.ini")).unwrap();
    assert!(!bad.contains("; \";-;\" are errored conditional lines."));
}

/// 测试：幂等性——update_mod_data 连续多次运行后，_MANAGED_ 下 group INI 与模组 ini
/// 内容进入稳态且不再变化（比较第 2 次后与第 3 次后）。
///
/// 说明：首次运行注入的守卫包裹整段；第 2 次运行起 reorder_by_ini_key_priority 会将
/// hash / match_priority 等优先键提升到守卫之前（且已存在守卫时跳过重复注入），
/// 故第 1 → 2 次存在一次性的注入归一化，第 2 次后进入稳态。该行为为预存在的
/// 管线语义（与串行版一致），非并行化引入。
#[test]
fn test_update_mod_data_idempotent() {
    let dir = setup_test_env();
    create_main_ini(dir.path(), TargetGame::GenshinImpact);
    let settings = AppSettings::default();

    let group1 = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(
        &group1,
        "ModA",
        "[TextureOverrideBody]\nhash = 0x1\ndrawindexed = auto\n",
    );
    create_mod_with_ini(
        &group1,
        "ModB",
        "[TextureOverrideBody]\nhash = 0x2\ndrawindexed = auto\n",
    );
    let group2 = create_group_dir(dir.path(), "group_2");
    create_mod_with_ini(&group2, "ModC", "[Constants]\n$v = 1\n");

    let managed = dir.path().join("_MANAGED_");
    update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
    assert!(group1.join("group_1.ini").exists());
    assert!(group2.join("group_2.ini").exists());

    // 第 2 次运行后进入稳态
    update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
    let snapshot2 = snapshot_ini_files(&managed);
    assert!(
        snapshot2.len() >= 5,
        "应至少包含 3 个模组 ini 与 2 个 group ini"
    );

    // 第 3 次运行后内容应与稳态完全一致（幂等）
    update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
    let snapshot3 = snapshot_ini_files(&managed);

    assert_eq!(
        snapshot2, snapshot3,
        "进入稳态后再次更新，_MANAGED_ 下 ini 内容应保持完全一致"
    );
}

/// 测试：update_mod_data 不涉及非group目录的标记文件生成
///
/// 验证：调用 update_mod_data 后，非group目录下不会创建 groupname/selectedindex/modname
#[test]
fn test_update_mod_data_no_markers_in_non_group() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let settings = AppSettings::default();

    // group_1 下的模组
    let group1 = dir.path().join("_MANAGED_").join("group_1");
    fs::create_dir_all(&group1).unwrap();
    create_mod_with_ini(&group1, "NormalMod", "[TextureOverride]\nhash=0x1\n");

    // 非group目录下的模组
    let mutex_group = dir.path().join("_MANAGED_").join("#Mutex");
    fs::create_dir_all(&mutex_group).unwrap();
    let mutex_mod = create_mod_with_ini(&mutex_group, "MutexMod", "[TextureOverride]\nhash=0x2\n");

    let _result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();

    // 验证非group目录下未创建标记文件
    assert!(
        !mutex_group.join("groupname").exists(),
        "update_mod_data 不应在非group目录创建 groupname 文件"
    );
    assert!(
        !mutex_group.join(constants::SELECTED_INDEX_FILE).exists(),
        "update_mod_data 不应在非group目录创建 selectedindex 文件"
    );
    assert!(
        !mutex_mod.join("modname").exists(),
        "update_mod_data 不应在非group目录下模组创建 modname 文件"
    );
}
