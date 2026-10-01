use super::*;
use crate::core::mod_update_pipeline::update_mod_data;
use crate::models::enums::TargetGame;
use crate::models::settings::AppSettings;
use tempfile::TempDir;

/// 还原安全窗口全局状态互斥 + RAII 清除。
///
/// 调用 `restore_all_inis` / `restore_managed_mod`（内部开启 60s 全局还原安全窗口，
/// `mod_scanner::mark_restore_safe_window`）的测试必须持有 `mod_scanner::TEST_WINDOW_LOCK`，
/// 并在退出 / panic 时通过本 guard 清除窗口——否则窗口会泄漏到并发执行的
/// mod_scanner 标记创建断言测试（窗口期内标记不被创建而误报失败）。
struct RestoreWindowGuard;
impl Drop for RestoreWindowGuard {
    fn drop(&mut self) {
        crate::core::mod_scan_marker::test_clear_restore_safe_window();
    }
}

fn setup_test_env() -> TempDir {
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

#[test]
fn test_restore_all_inis() {
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _w = RestoreWindowGuard;
    // d3dx.ini 在游戏根（Mods 父目录）；_MANAGED_ 在 Mods/ 下
    let dir = tempfile::TempDir::new().unwrap();
    let mods_path = dir.path().join("Mods");
    let managed = mods_path.join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();
    create_main_ini(dir.path(), TargetGame::GenshinImpact);
    // 覆盖 include_recursive 值为 "Mods"，使 detect_include_recursive 匹配
    fs::write(
        dir.path().join("d3dx.ini"),
        "; original main ini\n[Include]\ninclude_recursive = Mods\n",
    )
    .unwrap();
    let settings = AppSettings::default();

    let group_path = managed.join("group_1");
    fs::create_dir_all(&group_path).unwrap();
    let _mod_path = {
        let p = group_path.join("RestoreMod");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("mod.ini"), "[TextureOverrideTest]\nhash = 0x1\n").unwrap();
        p
    };

    update_mod_data(TargetGame::GenshinImpact, &mods_path, &settings).unwrap();

    let main_ini_path = dir.path().join("d3dx.ini");
    let modified_content = fs::read_to_string(&main_ini_path).unwrap();
    // 对齐 NRMM 原版：d3dx.ini 有 include_recursive=Mods 时不注入 ;NRMM_INI_START 块
    assert!(!modified_content.contains(";NRMM_INI_START"));

    let result = restore_all_inis(&mods_path).unwrap();
    assert!(result.restored >= 1);
    assert_eq!(result.failed, 0);

    let restored_content = fs::read_to_string(&main_ini_path).unwrap();
    assert!(!restored_content.contains(";NRMM_INI_START"));

    let backup_path = main_ini_path.with_extension(constants::BACKUP_EXTENSION);
    assert!(!backup_path.exists());

    // 验证管理文件在还原后保留（对齐 NRMM restoreManagedMod：不动管理器文件）
    assert!(managed.join("nrmm_include.ini").exists());
}

#[test]
fn test_restore_managed_mod_single_ini_file() {
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _w = RestoreWindowGuard;
    let dir = setup_test_env();
    // 构造一个含 NRMM 管理注入的单个 .ini 文件
    let ini_path = dir.path().join("mod.ini");
    fs::write(
        &ini_path,
        "[TextureOverrideTest]\nhash = 0x1\n; no reload mod manager\n",
    )
    .unwrap();

    // 单 .ini 文件应被还原
    let (restored, failed) = restore_managed_mod(&ini_path);
    assert_eq!(restored, 1);
    assert_eq!(failed, 0);
    let content = fs::read_to_string(&ini_path).unwrap();
    assert!(!content.contains("no reload mod manager"));
}

#[test]
fn test_restore_managed_mod_skips_non_ini_file() {
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _w = RestoreWindowGuard;
    let dir = setup_test_env();
    // 非 .ini 文件应被忽略（pass），不报错也不处理
    let txt_path = dir.path().join("readme.txt");
    fs::write(&txt_path, "hello").unwrap();

    let (restored, failed) = restore_managed_mod(&txt_path);
    assert_eq!(restored, 0);
    assert_eq!(failed, 0);
}

#[test]
fn test_restore_managed_mod_directory_recursive() {
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _w = RestoreWindowGuard;
    let dir = setup_test_env();
    let group_path = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(
        &group_path,
        "ModA",
        "[TextureOverrideA]\nhash = 0x1\n; no reload mod manager\n",
    );
    create_mod_with_ini(
        &group_path,
        "ModB",
        "[TextureOverrideB]\nhash = 0x2\nglobal $managed_slot_id = 0\n",
    );

    // 目录应递归还原其下所有 .ini 文件
    let (restored, failed) = restore_managed_mod(&group_path);
    assert_eq!(restored, 2);
    assert_eq!(failed, 0);
    let a = fs::read_to_string(group_path.join("ModA").join("mod.ini")).unwrap();
    assert!(!a.contains("no reload mod manager"));
    let b = fs::read_to_string(group_path.join("ModB").join("mod.ini")).unwrap();
    assert!(!b.contains("global $managed_slot_id"));
}

// ============================================================================
// 还原逻辑对齐测试（NRMM restoreManagedMod / restoreAllInis）
// 验证「备份式还原（Restore All）」无损回放，以及「就地清理式还原（还原区/移除）」
// 正确清除管理器注入的所有产物（manager if/endif、global $managed_slot_id、
// condition 槽位段、4 空格缩进、管理器头注释），并保留非管理器内容。
// ============================================================================

/// 备份式还原（Restore All）必须是 update_mod_data 的「无损逆操作」：
/// 每个被管理的 mod INI 都应从 .ini_managed_backup 精确恢复到更新前字节。
#[test]
fn test_restore_all_inis_lossless_roundtrip() {
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _w = RestoreWindowGuard;
    // 结构与真实游戏一致：game_root/Mods/_MANAGED_/group_X/Mod/mod.ini
    // 且 game_root/d3dx.ini 位于 Mods 父目录。
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().join("Mods");
    let managed = mods_path.join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();
    create_main_ini(dir.path(), TargetGame::GenshinImpact);
    let settings = AppSettings::default();

    let group_path = managed.join("group_1");
    fs::create_dir_all(&group_path).unwrap();
    let original = "\
[TextureOverrideBody]
hash = 0x1
drawindexed = auto
condition = orig_cond

[KeySection]
condition = (orig_cond) && other

[Resource]
hash = 0x2
";
    let mod_dir = create_mod_with_ini(&group_path, "ModA", original);
    let mod_ini = mod_dir.join("mod.ini");

    // 更新（注入管理器内容 + 创建备份）
    let result = update_mod_data(TargetGame::GenshinImpact, &mods_path, &settings).unwrap();
    assert_eq!(result.processed_mods, 1);
    // 更新后文件应已被注入（与原始不同）
    let updated = fs::read_to_string(&mod_ini).unwrap();
    assert_ne!(updated, original, "更新后应注入管理器内容");
    // 备份必须存在且为更新前字节
    let backup = mod_ini.with_extension(constants::BACKUP_EXTENSION);
    assert!(backup.exists());
    assert_eq!(fs::read_to_string(&backup).unwrap(), original);

    // 还原（Restore All）
    let rc = restore_all_inis(&mods_path).unwrap();
    assert_eq!(rc.failed, 0);

    // 还原后 mod INI 必须精确等于更新前字节（无损回放）
    let restored = fs::read_to_string(&mod_ini).unwrap();
    assert_eq!(restored, original, "备份式还原必须无损恢复到更新前状态");

    // 备份应被删除
    assert!(!backup.exists());
    // 管理器生成文件应保留（对齐 NRMM restoreManagedMod：还原不动管理器文件；
    // group_N.ini 生成于组目录内，非 _MANAGED_ 根）
    assert!(managed.join("nrmm_include.ini").exists());
    assert!(managed.join("manager_group.ini").exists());
    assert!(managed.join("group_1").join("group_1.ini").exists());
}

/// 就地清理式还原：对一份「已经过 update_mod_data」的 INI，验证 restore_managed_mod
/// 清除全部管理器注入产物，但保留用户原始内容与合法悬空 endif。
#[test]
fn test_restore_managed_mod_cleans_injected_artifacts() {
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _w = RestoreWindowGuard;
    let dir = setup_test_env();
    let ini = dir.path().join("mod.ini");
    // 模拟 update_mod_data 输出的真实形态（头注释 + Constants 全局 + 包裹段 + 增强 condition + 悬空 endif）
    let managed = "\
; \";-;\" are errored conditional lines.
; \";+;\" are disabled keys.
; Errored conditional blocks (if/else/elif/endif) are handled correctly (newer syntax may require further testing), including namespaced variables.
; If certain syntax is only available in newer XXMI versions, make sure to use the latest XXMI.
[Constants]
global $managed_slot_id = 0
[TextureOverrideBody]
hash = 0x1
match_priority = 0
if $managed_slot_id == $\\modmanageragl\\group_1\\active_slot
    drawindexed = auto
    ; inner note
endif
[KeySection]
condition = (orig_cond) && $managed_slot_id == $\\modmanageragl\\group_1\\active_slot
[Resource]
hash = 0x2
endif
";
    fs::write(&ini, managed).unwrap();

    let (total, failed) = restore_managed_mod(&ini);
    assert_eq!(total, 1);
    assert_eq!(failed, 0);

    let out = fs::read_to_string(&ini).unwrap();

    // 1) 管理器头注释被移除
    assert!(!out.contains("are errored"), "管理器头注释应被移除");
    assert!(!out.contains("are disabled keys"), "管理器头注释应被移除");
    // 2) global $managed_slot_id 被移除
    assert!(
        !out.contains("global $managed_slot_id"),
        "global 声明应被移除"
    );
    // 3) manager if 与该段 endif 被移除，且不含任何管理器残留
    assert!(!out.contains("managed_slot_id"), "管理器槽位残留应被清除");
    assert!(!out.contains("active_slot"), "管理器槽位残留应被清除");
    // 4) 4 空格缩进被移除（段体行去缩进）
    assert!(
        out.contains("drawindexed = auto"),
        "缩进后的命令应去缩进保留"
    );
    assert!(!out.contains("    drawindexed"), "不应残留 4 空格缩进");
    // 5) 非管理器注释保留
    assert!(out.contains("; inner note"), "非管理器注释应保留");
    // 6) 增强 condition 中管理器段被清除，仅留用户原条件（圆括号为 sanitize 对称解包，详见报告）
    assert!(out.contains("orig_cond"), "用户原 condition 应保留");
    // 7) 合法悬空 endif 必须保留（对齐 Dart：不删除 INPUT 遗留悬空 endif）
    assert!(out.contains("endif"), "合法悬空 endif 应被保留");
    // 8) match_priority=0 由 update 注入，restore 不移除（非管理器专属标记；
    //    作为独立 3Dmigoto 指令在脱管后依然合法。此行为是否为 NRMM 严格一致项待 Dart 基线确认。）
    assert!(
        out.contains("match_priority = 0"),
        "match_priority 注入项当前保留（对齐待确认）"
    );
}

/// 真实管线回放：update_mod_data 产出后，对 mod 目录执行 restore_managed_mod，
/// 确认管理器 if/endif/global/condition 全部清除。
#[test]
fn test_restore_managed_mod_after_real_update() {
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _w = RestoreWindowGuard;
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().join("Mods");
    let managed = mods_path.join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();
    create_main_ini(dir.path(), TargetGame::GenshinImpact);
    let settings = AppSettings::default();

    let group_path = managed.join("group_1");
    fs::create_dir_all(&group_path).unwrap();
    let mod_dir = create_mod_with_ini(
        &group_path,
        "ModA",
        "[TextureOverrideBody]\nhash = 0x1\ndrawindexed = auto\n",
    );

    update_mod_data(TargetGame::GenshinImpact, &mods_path, &settings).unwrap();

    let (total, failed) = restore_managed_mod(&mod_dir);
    assert_eq!(total, 1);
    assert_eq!(failed, 0);

    let out = fs::read_to_string(mod_dir.join("mod.ini")).unwrap();
    assert!(
        !out.contains("managed_slot_id"),
        "真实管线还原后不应残留管理器槽位"
    );
    assert!(
        !out.contains("active_slot"),
        "真实管线还原后不应残留管理器槽位"
    );
    // 段体命令保留且去缩进
    assert!(out.contains("drawindexed = auto"));
}

/// 测试：collect_mod_ini_files 正确收集模组目录下的 .ini 文件
///
/// 验证：
/// - 收集直接子 .ini 文件
/// - 不递归子目录
/// - 排除 desktop.ini
/// - 排除 .ini_managed_backup 备份文件
#[test]
fn test_collect_mod_ini_files() {
    let dir = TempDir::new().unwrap();
    let mod_dir = dir.path().join("TestMod");
    fs::create_dir_all(&mod_dir).unwrap();

    // 正常 .ini 文件
    fs::write(mod_dir.join("mod.ini"), "[Section]\n").unwrap();
    fs::write(mod_dir.join("config.ini"), "[Other]\n").unwrap();
    // desktop.ini 应被排除
    fs::write(mod_dir.join("desktop.ini"), "[System]\n").unwrap();
    // .ini_managed_backup 备份文件应被排除
    fs::write(mod_dir.join("mod.ini_managed_backup"), "[Backup]\n").unwrap();
    // 子目录中的 .ini 应被排除（不递归）
    let subdir = mod_dir.join("SubDir");
    fs::create_dir_all(&subdir).unwrap();
    fs::write(subdir.join("nested.ini"), "[Nested]\n").unwrap();
    // 非 .ini 文件应被排除
    fs::write(mod_dir.join("readme.txt"), "text\n").unwrap();

    let ini_files = collect_mod_ini_files(&mod_dir);
    // 应收集到 mod.ini 和 config.ini（2 个文件）
    assert_eq!(
        ini_files.len(),
        2,
        "应仅收集直接 .ini 文件，排除 desktop/backup/nested"
    );
    let file_names: Vec<String> = ini_files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(file_names.contains(&"mod.ini".to_string()));
    assert!(file_names.contains(&"config.ini".to_string()));
}
