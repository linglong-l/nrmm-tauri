use super::*;
use crate::core::mod_update_pipeline::update_mod_data;
use crate::models::settings::AppSettings;
use tempfile::TempDir;

fn setup_test_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();
    dir
}

fn create_mod_with_ini(group_path: &Path, mod_name: &str, ini_content: &str) -> PathBuf {
    let mod_path = group_path.join(mod_name);
    fs::create_dir_all(&mod_path).unwrap();
    let ini_path = mod_path.join("mod.ini");
    fs::write(&ini_path, ini_content).unwrap();
    mod_path
}

fn create_d3dx_ini(base: &Path) {
    fs::write(base.join("d3dx.ini"), "; test\n").unwrap();
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
fn test_strip_nrmm_content() {
    let content = r#"; original header
[Constants]
x = 1
;NRMM_INI_START
managed stuff here
include = mod1.ini
;NRMM_INI_END
; trailing content
y = 2
"#;
    let result = strip_nrmm_injected_content(content);
    assert!(!result.contains("managed stuff"));
    assert!(!result.contains("include = mod1.ini"));
    assert!(result.contains("original header"));
    assert!(result.contains("x = 1"));
    assert!(result.contains("y = 2"));
    assert!(!result.contains("NRMM_INI_START"));
    assert!(!result.contains("NRMM_INI_END"));
}

#[test]
fn test_strip_nrmm_content_no_injection() {
    let content = "; no injection here\n[Section]\nkey=val\n";
    let result = strip_nrmm_injected_content(content);
    assert_eq!(result.trim(), content.trim());
}

#[test]
fn test_detect_include_recursive_present() {
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().join("Mods");
    fs::create_dir_all(&mods_path).unwrap();
    let ini_path = dir.path().join("d3dx.ini");
    fs::write(
        &ini_path,
        "[Include]\ninclude_recursive = Mods\n\n[Constants]\nglobal $test = 0\n",
    )
    .unwrap();
    assert!(detect_include_recursive(&ini_path, &mods_path));
}

#[test]
fn test_detect_include_recursive_missing() {
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().join("Mods");
    fs::create_dir_all(&mods_path).unwrap();
    let ini_path = dir.path().join("d3dx.ini");
    fs::write(&ini_path, "[Constants]\nglobal $test = 0\n").unwrap();
    assert!(!detect_include_recursive(&ini_path, &mods_path));
}

#[test]
fn test_detect_include_recursive_wrong_section() {
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().join("Mods");
    fs::create_dir_all(&mods_path).unwrap();
    let ini_path = dir.path().join("d3dx.ini");
    // include_recursive 放在 [Constants] 段内，3Dmigoto 不处理此位置
    fs::write(&ini_path, "[Constants]\ninclude_recursive = Mods\n").unwrap();
    assert!(!detect_include_recursive(&ini_path, &mods_path));
}

#[test]
fn test_detect_include_recursive_case_insensitive() {
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().join("Mods");
    fs::create_dir_all(&mods_path).unwrap();
    let ini_path = dir.path().join("d3dx.ini");
    fs::write(&ini_path, "[INCLUDE]\ninclude_recursive = Mods\n").unwrap();
    assert!(detect_include_recursive(&ini_path, &mods_path));
}

#[test]
fn test_generate_injected_content() {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();
    let include_path = managed.join("nrmm_include.ini");
    fs::write(&include_path, "").unwrap();

    let result = generate_nrmm_injected_content(&include_path, dir.path()).unwrap();

    assert!(result.contains(";NRMM_INI_START"));
    assert!(result.contains(";NRMM_INI_END"));
    assert!(result.contains("[Constants]"));
    assert!(result.contains("$managed_slot_id"));
    assert!(result.contains("include = _MANAGED_/nrmm_include.ini"));
    assert!(!result.contains("\\"));
}

#[test]
fn test_prepare_managed_folder() {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();

    let need_reload = prepare_managed_folder(&managed, TargetGame::GenshinImpact).unwrap();
    assert!(need_reload); // 首次创建需要手动重载

    assert!(managed.join(constants::KEYPRESS_FILENAME).exists());
    assert!(managed.join(constants::INCLUDE_FILENAME).exists());
    assert!(managed.join("manager_group.ini").exists());

    // 第二次调用应该不需要手动重载
    let need_reload2 = prepare_managed_folder(&managed, TargetGame::GenshinImpact).unwrap();
    assert!(!need_reload2);
}

#[test]
fn test_create_group_ini() {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    let group_dir = managed.join("group_1");
    fs::create_dir_all(&group_dir).unwrap();

    let mod1 = group_dir.join("Mod1/mod.ini");
    fs::create_dir_all(mod1.parent().unwrap()).unwrap();
    fs::write(&mod1, "").unwrap();

    let mod2 = group_dir.join("Mod2/config.ini");
    fs::create_dir_all(mod2.parent().unwrap()).unwrap();
    fs::write(&mod2, "").unwrap();

    let ini_paths = vec![mod1, mod2];
    let result = create_group_ini(&group_dir, 1, &ini_paths).unwrap();
    assert!(result.is_some());

    let group_ini = result.unwrap();
    assert!(group_ini.exists());
    let content = fs::read_to_string(&group_ini).unwrap();
    assert!(content.contains("group_1"));
    assert!(content.contains("$group_id = 1"));
    // 对齐 NRMM：group_X.ini 仅包含组模板，不包含显式 include 指令
    assert!(!content.contains("include = "));
}

/// 测试：create_group_ini 对空分组返回 None，不生成 group_X.ini
#[test]
fn test_create_group_ini_skips_empty() {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    let group_dir = managed.join("group_84");
    fs::create_dir_all(&group_dir).unwrap();

    let ini_paths: Vec<PathBuf> = vec![];
    let result = create_group_ini(&group_dir, 84, &ini_paths).unwrap();
    assert!(result.is_none(), "空分组应返回 None");

    // 验证未生成 group_84.ini 文件
    let group_ini = group_dir.join("group_84.ini");
    assert!(!group_ini.exists(), "空分组不应生成 group_X.ini 文件");
}

/// 测试：fix_non_managed_crash_lines 复用 MOD_INI_CACHE 且缓存新鲜度正确
///
/// 验证三点：
/// 1. 首次调用注释崩溃行（`;-;` 前缀）；
/// 2. 第二次调用同一文件未变化 → 缓存命中，检测无崩溃行，不重复写（内容不变）；
/// 3. 文件被修改（mtime/len 变化）→ 缓存自动未命中重新解析，新增崩溃行被注释，
///    证明缓存不会返回旧内容。
///
/// 同时验证 `_MANAGED_` 内文件与 `#` 目录不受触碰（沿用现有过滤逻辑）。
#[test]
fn test_fix_non_managed_crash_lines_cache_freshness() {
    let dir = TempDir::new().unwrap();
    let mods = dir.path().join("Mods");
    let managed = mods.join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();

    // 非托管 INI（Mods 根下、_MANAGED_ 外）：drawindexed 取非法值构成崩溃行
    let loose = mods.join("loose_mod.ini");
    fs::write(&loose, "[SomeSection]\ndrawindexed = crashval\n").unwrap();

    // _MANAGED_ 内与 # 目录下的文件：不应被 fix_non_managed 触碰
    let managed_ini = managed.join("managed_mod.ini");
    fs::write(&managed_ini, "[SomeSection]\ndrawindexed = crashval\n").unwrap();

    let known = HashSet::new();
    fix_non_managed_crash_lines(&mods, &managed, &known);

    let after1 = fs::read_to_string(&loose).unwrap();
    assert!(
        after1.contains(";-;drawindexed = crashval"),
        "首次调用应注释非托管崩溃行"
    );
    assert!(
        !fs::read_to_string(&managed_ini).unwrap().contains(";-;"),
        "_MANAGED_ 内文件不应被 fix_non_managed 修改"
    );

    // 第二次调用：文件未变化 → 缓存命中（不重复解析），无崩溃行 → 不写
    fix_non_managed_crash_lines(&mods, &managed, &known);
    let after2 = fs::read_to_string(&loose).unwrap();
    assert_eq!(after1, after2, "未变化文件二次调用不应重复解析后被改写");

    // 修改文件追加新崩溃行：mtime/len 变化 → 缓存失效重新解析 → 新行被注释
    fs::write(
        &loose,
        "[SomeSection]\n;-;drawindexed = crashval\ndraw = anothercrash\n",
    )
    .unwrap();
    fix_non_managed_crash_lines(&mods, &managed, &known);
    let after3 = fs::read_to_string(&loose).unwrap();
    assert!(
        after3.contains(";-;draw = anothercrash"),
        "文件变化后缓存不得返回旧内容，新崩溃行应被注释"
    );
}

/// 测试：prepare_managed_folder 正确创建根级 selectedindex 文件
#[test]
fn test_prepare_managed_folder_creates_root_selectedindex() {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();

    let root_selectedindex = managed.join(constants::SELECTED_INDEX_FILE);
    assert!(!root_selectedindex.exists());

    prepare_managed_folder(&managed, TargetGame::GenshinImpact).unwrap();

    // 验证根级 selectedindex 已创建
    assert!(
        root_selectedindex.exists(),
        "根级 selectedindex 文件应被创建"
    );
    let content = fs::read_to_string(&root_selectedindex).unwrap();
    assert_eq!(content, "0", "根级 selectedindex 默认值应为 0");

    // 再次调用不应覆盖已有内容
    fs::write(&root_selectedindex, "5").unwrap();
    prepare_managed_folder(&managed, TargetGame::GenshinImpact).unwrap();
    let content2 = fs::read_to_string(&root_selectedindex).unwrap();
    assert_eq!(content2, "5", "已有 selectedindex 不应被覆盖");
}

#[test]
fn test_delete_group_ini_files() {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();

    let old_ini = managed.join("group_1.ini");
    fs::write(&old_ini, "").unwrap();
    let old_modfolder = managed.join("ModFolder.ini");
    fs::write(&old_modfolder, "").unwrap();
    let other_file = managed.join("other.txt");
    fs::write(&other_file, "").unwrap();

    assert!(old_ini.exists());
    delete_group_ini_files(&managed).unwrap();
    assert!(!old_ini.exists());
    assert!(!old_modfolder.exists());
    assert!(other_file.exists());
}

#[test]
fn test_strip_nrmm_multiline() {
    let content = "before\n;NRMM_INI_START\na\nb\nc\n;NRMM_INI_END\nafter";
    let result = strip_nrmm_injected_content(content);
    assert_eq!(result.trim(), "before\nafter");
}

#[test]
fn test_save_customizations() {
    // d3dx.ini 在游戏根（Mods 父目录）
    let dir = tempfile::TempDir::new().unwrap();
    let mods_path = dir.path().join("Mods");
    fs::create_dir_all(mods_path.join("_MANAGED_")).unwrap();
    create_main_ini(dir.path(), TargetGame::GenshinImpact);
    let settings = AppSettings::default();

    // 先执行一次 update_mod_data 添加 NRMM 管理段
    update_mod_data(TargetGame::GenshinImpact, &mods_path, &settings).unwrap();

    // 保存自定义设置
    let result = save_customizations(&mods_path, TargetGame::GenshinImpact).unwrap();
    assert!(result.success);

    let user_ini = dir.path().join("d3dx_user.ini");
    assert!(user_ini.exists());
    let content = fs::read_to_string(&user_ini).unwrap();
    assert!(content.contains("Custom settings saved by NRMM"));
    assert!(!content.contains(";NRMM_INI_START"));
}

#[test]
fn test_atomic_write_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.txt");
    atomic_write_file(&path, b"hello world").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "hello world");
    assert!(!path.with_extension("tmp").exists());
}

/// 编码安全测试：`comment_crash_lines_in_file` 保留 BOM / CRLF / 末尾换行，
/// 并对非 UTF-8 文件跳过修复（拒绝有损回写损坏）。
#[test]
fn test_comment_crash_lines_encoding_safe() {
    let dir = TempDir::new().unwrap();

    // 1) BOM + CRLF 文件：仅目标行被加前缀，其余字节原样保留
    let crlf_path = dir.path().join("crlf.ini");
    let crlf_content = "\u{FEFF}[Section]\r\nkey = 1\r\ndrawindexed = 999\r\n";
    fs::write(&crlf_path, crlf_content.as_bytes()).unwrap();
    let changed = comment_crash_lines_in_file(&crlf_path, &[(3, "drawindexed = 999".to_string())]);
    assert!(changed, "CRLF 文件应执行崩溃行修复");
    let bytes = fs::read(&crlf_path).unwrap();
    assert!(bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "BOM 应保留");
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.contains("\r\n;-;drawindexed = 999\r\n"),
        "CRLF 与目标行前缀应保留:\n{:?}",
        text
    );
    assert!(text.contains("\r\nkey = 1\r\n"), "未命中行应原样保留");

    // 2) 非 UTF-8 文件（含 GBK 字节）：C6 统一编码——先无损转 UTF-8 再注释崩溃行
    let gbk_path = dir.path().join("gbk.ini");
    // GBK 字节：\xb2\xe2\xca\xd4 = "测试"
    let gbk_bytes: Vec<u8> = b"[Section]\r\ndrawindexed = 5\r\n\xb2\xe2\xca\xd4\r\n".to_vec();
    fs::write(&gbk_path, &gbk_bytes).unwrap();
    let changed2 = comment_crash_lines_in_file(&gbk_path, &[(2, "drawindexed = 5".to_string())]);
    assert!(changed2, "非 UTF-8 文件应转换并修复崩溃行");
    let out_text = String::from_utf8(fs::read(&gbk_path).unwrap()).unwrap();
    assert!(
        out_text.contains(";-;drawindexed = 5") && out_text.contains("测试"),
        "GBK 内容应无损转为 UTF-8 并注释崩溃行:\n{:?}",
        out_text
    );

    // 3) LF 无 BOM 文件：正常修复且保留末尾换行
    let lf_path = dir.path().join("lf.ini");
    fs::write(&lf_path, "[Section]\nkey = 1\ndrawindexed = auto\n").unwrap();
    let changed3 = comment_crash_lines_in_file(&lf_path, &[(3, "drawindexed = auto".to_string())]);
    assert!(changed3);
    assert_eq!(
        fs::read_to_string(&lf_path).unwrap(),
        "[Section]\nkey = 1\n;-;drawindexed = auto\n",
        "LF 文件修复后应保留末尾换行"
    );
}

#[test]
fn test_validate_group_ini_consistency() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    // 创建 group_1 分组目录和模组
    let group_path = dir.path().join("_MANAGED_").join("group_1");
    fs::create_dir_all(&group_path).unwrap();
    create_mod_with_ini(&group_path, "TestMod", "[Constants]\nx=1\n");

    // 创建一致的 group_1.ini：global $group_id = 1（与目录 group_1 匹配）
    let good_ini = "[Constants]\nglobal $group_id = 1\n";
    fs::write(group_path.join("group_1.ini"), good_ini).unwrap();

    let scan = crate::core::mod_scan_deep::scan_mods_deep(dir.path()).unwrap();
    let enabled: Vec<&ModData> = scan
        .mods
        .iter()
        .filter(|m| !m.disabled)
        .map(|m| m.as_ref())
        .collect();
    let managed = dir.path().join("_MANAGED_");
    let mismatched = validate_group_ini_consistency(&managed, &enabled);
    assert!(mismatched.is_empty(), "一致的 group_ini 不应报错");

    // 创建不一致的 group_1.ini：global $group_id = 99（与目录 group_1 不匹配）
    let bad_ini = "[Constants]\nglobal $group_id = 99\n";
    fs::write(group_path.join("group_1.ini"), bad_ini).unwrap();
    let mismatched = validate_group_ini_consistency(&managed, &enabled);
    assert_eq!(mismatched, vec![1], "不一致的 group_ini 应返回 gid=1");
}
