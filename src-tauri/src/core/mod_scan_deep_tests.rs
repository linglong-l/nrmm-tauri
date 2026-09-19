use super::*;
use crate::core::mod_scan_light::check_mods_path;
use crate::models::enums::{ModsPathStatus, TargetGame};
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

/// 测试：深度扫描对禁用模组跳过内容解析（2026-08-18 性能优化）
#[test]
fn test_scan_deep_disabled_mod_skips_parse() {
    let dir = setup_test_dir();
    let group = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(
        &group,
        "EnabledMod",
        "[TextureOverride]\nhash = 0xAAAA\n[CommandListX]\ny = 1\n",
    );
    create_mod_with_ini(
        &group,
        "DISABLED_DisabledMod",
        "[TextureOverride]\nhash = 0xBBBB\n[CommandListY]\nz = 1\n",
    );

    let result = scan_mods_deep(dir.path()).expect("scan_mods_deep 失败");
    assert_eq!(result.mods.len(), 2, "应发现启用与禁用两个模组");

    let enabled = result
        .mods
        .iter()
        .find(|m| !m.disabled)
        .expect("应存在启用模组");
    assert_eq!(
        enabled.mod_ini_data[0].section_count, 2,
        "启用模组应正常解析（2 个段）"
    );

    let disabled = result
        .mods
        .iter()
        .find(|m| m.disabled)
        .expect("应存在禁用模组");
    assert!(
        !disabled.mod_ini_data.is_empty() && !disabled.mod_ini_data[0].ini_path.is_empty(),
        "禁用模组 INI 路径应保留"
    );
    assert_eq!(
        disabled.mod_ini_data[0].section_count, 0,
        "禁用模组应跳过内容解析（section 计数为 0）"
    );
    assert!(
        disabled.errored_lines.is_empty(),
        "禁用模组不应产生解析错误条目"
    );
}

/// 测试：深扫两阶段库收集，跨 INI 引用已声明库不误报（error_type=5）
#[test]
fn test_deep_cross_ini_library_reference_resolution() {
    let dir = setup_test_dir();
    let group = create_group_dir(dir.path(), "group_1");
    let mod_path = group.join("MyMod");
    fs::create_dir_all(&mod_path).unwrap();
    fs::write(
            mod_path.join("a.ini"),
            "[ShaderOverrideShaderA]\nhash = 0xAAA\nrun = ShaderOverrideShaderB\nrun = ShaderOverrideMissingLib\n",
        )
        .unwrap();
    fs::write(
        mod_path.join("b.ini"),
        "[ShaderOverrideShaderB]\nhash = 0xBBB\n",
    )
    .unwrap();

    let result = scan_mods_deep(dir.path()).expect("scan_mods_deep 失败");
    assert_eq!(result.mods.len(), 1, "应发现一个模组");
    let mod_data = &result.mods[0];

    let type5_errors: Vec<&String> = mod_data
        .errored_lines
        .iter()
        .filter(|e| e.error_type == 5)
        .map(|e| &e.error_message)
        .collect();

    assert_eq!(
        type5_errors.len(),
        1,
        "仅未声明库应产出 NON EXISTENT LIB，实际: {type5_errors:?}"
    );
    assert!(
        type5_errors[0].contains("ShaderOverrideMissingLib"),
        "应仅报未声明库，实际: {type5_errors:?}"
    );
}

// ========== 深度扫描测试（原 scan_mods 改名为 scan_mods_deep） ==========

/// 测试：空 `_MANAGED_` 目录的深度扫描应返回空结果
#[test]
fn test_scan_empty_managed_folder() {
    let dir = setup_test_dir();
    let result = scan_mods_deep(dir.path()).unwrap();
    assert!(result.groups.is_empty());
    assert!(result.mods.is_empty());
    assert_eq!(result.total_mods_count, 0);
}

/// 测试：深度扫描单个 NormalGroup 中的单个模组
#[test]
fn test_scan_single_mod() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(
        &group_path,
        "TestMod",
        "[TextureOverrideTest]\nhash = 0x123\n",
    );

    let result = scan_mods_deep(dir.path()).unwrap();
    assert_eq!(result.groups.len(), 1);
    assert_eq!(result.mods.len(), 1);
    assert_eq!(result.total_mods_count, 1);
    assert_eq!(result.mods[0].name, "TestMod");
    assert!(!result.mods[0].disabled);
}

/// 测试：深度扫描 `DISABLED` 前缀模组应被正确标记为禁用状态
#[test]
fn test_scan_disabled_mod() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(
        &group_path,
        "DISABLEDMyMod",
        "[TextureOverrideTest]\nhash = 0x456\n",
    );

    let result = scan_mods_deep(dir.path()).unwrap();
    assert_eq!(result.mods.len(), 1);
    assert!(result.mods[0].disabled);
    assert!(result.mods[0].mod_disabled);
    assert!(result.mods[0].name.contains("MyMod"));
    assert_eq!(result.disabled_mods_count, 1);
    assert_eq!(result.enabled_mods_count, 0);
}

/// 测试：深度扫描忽略非 `group_xx` 目录（MutexGroup 和无关目录）
#[test]
fn test_scan_ignores_non_group_dirs() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let mutex_path = dir.path().join("_MANAGED_").join("#MutexMods");
    fs::create_dir_all(&mutex_path).unwrap();
    create_mod_with_ini(
        &mutex_path,
        "MutexMod",
        "[TextureOverrideTest]\nhash = 0x789\n",
    );

    let other_path = dir.path().join("_MANAGED_").join("OtherFolder");
    fs::create_dir_all(&other_path).unwrap();
    create_mod_with_ini(
        &other_path,
        "OtherMod",
        "[TextureOverrideTest]\nhash = 0xABC\n",
    );

    let group_path = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(
        &group_path,
        "ValidMod",
        "[TextureOverrideTest]\nhash = 0xDEF\n",
    );

    let result = scan_mods_deep(dir.path()).unwrap();
    assert_eq!(result.groups.len(), 1);
    assert_eq!(result.mods.len(), 1);
    assert_eq!(result.mods[0].name, "ValidMod");
}

/// 测试：深度扫描能递归找到嵌套子目录中的模组（BFS 遍历）
#[test]
fn test_scan_nested_mods_deep() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");

    let subdir = group_path.join("SubCategory");
    fs::create_dir_all(&subdir).unwrap();
    let mod_path = subdir.join("NestedMod");
    fs::create_dir_all(&mod_path).unwrap();
    fs::write(mod_path.join("mod.ini"), "[KeyTest]\nkey = VkA\n").unwrap();

    let result = scan_mods_deep(dir.path()).unwrap();
    assert_eq!(result.mods.len(), 1);
    assert_eq!(result.mods[0].name, "NestedMod");
}

/// 测试：深度扫描模组包含图标文件时可正确识别
#[test]
fn test_scan_mod_with_icon() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");

    let mod_path = group_path.join("IconMod");
    fs::create_dir_all(&mod_path).unwrap();
    fs::write(mod_path.join("mod.ini"), "[Constants]\nx=1\n").unwrap();
    fs::write(mod_path.join("icon.png"), b"fake png").unwrap();

    let result = scan_mods_deep(dir.path()).unwrap();
    assert_eq!(result.mods.len(), 1);
    assert!(result.mods[0].preview_image_path.is_some());
    assert!(result.mods[0]
        .preview_image_path
        .as_ref()
        .unwrap()
        .ends_with("icon.png"));
}

/// 测试：`check_mods_path` 对有效路径返回 `Valid`
#[test]
fn test_check_mods_path_valid() {
    let dir = setup_test_dir();
    let mods_path = dir.path().join("Mods");
    fs::create_dir_all(mods_path.join("_MANAGED_")).unwrap();
    create_d3dx_ini(dir.path());
    let status = check_mods_path(TargetGame::GenshinImpact, &mods_path);
    assert_eq!(status, ModsPathStatus::Valid);
}

/// 测试：`check_mods_path` 对缺少 `_MANAGED_` 目录的路径返回 `ManagedFolderNotFound`
#[test]
fn test_check_mods_path_missing_managed() {
    let dir = TempDir::new().unwrap();
    create_d3dx_ini(dir.path());
    let status = check_mods_path(TargetGame::GenshinImpact, dir.path());
    assert_eq!(status, ModsPathStatus::ManagedFolderNotFound);
}

/// 测试：`check_mods_path` 对不存在的路径返回 `NotFound`
#[test]
fn test_check_mods_path_not_found() {
    let dir = TempDir::new().unwrap();
    let nonexistent = dir.path().join("nonexistent");
    let status = check_mods_path(TargetGame::GenshinImpact, &nonexistent);
    assert_eq!(status, ModsPathStatus::NotFound);
}

/// 测试：`check_mods_path` 对缺少主 INI 文件的路径返回 `D3dxIniNotFound`
#[test]
fn test_check_mods_path_missing_d3dx() {
    let dir = setup_test_dir();
    let status = check_mods_path(TargetGame::GenshinImpact, dir.path());
    assert_eq!(status, ModsPathStatus::D3dxIniNotFound);
}

/// 测试：BFS 遇到模组目录（含 INI）后停止向下遍历，不扫描子目录
#[test]
fn test_bfs_stops_at_mod_directory() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");

    let mod_path = group_path.join("StoppingMod");
    fs::create_dir_all(&mod_path).unwrap();
    fs::write(mod_path.join("mod.ini"), "[TextureOverrideA]\nhash=1\n").unwrap();

    let subdir_under_mod = mod_path.join("ShouldNotBeScanned");
    fs::create_dir_all(&subdir_under_mod).unwrap();
    fs::write(
        subdir_under_mod.join("ignored.ini"),
        "[TextureOverrideB]\nhash=2\n",
    )
    .unwrap();

    let result = scan_mods_deep(dir.path()).unwrap();
    assert_eq!(result.mods.len(), 1);
    assert_eq!(result.mods[0].name, "StoppingMod");
}

/// 测试：深度扫描多个 NormalGroup 并按 `group_index` 排序
#[test]
fn test_multiple_groups() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let g1 = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");

    let g2 = create_group_dir(dir.path(), "group_2");
    create_mod_with_ini(&g2, "Mod2", "[TextureOverride2]\nhash=2\n");

    let g10 = create_group_dir(dir.path(), "group_10");
    create_mod_with_ini(&g10, "Mod10", "[TextureOverride10]\nhash=10\n");

    let result = scan_mods_deep(dir.path()).unwrap();
    assert_eq!(result.groups.len(), 3);
    assert_eq!(result.mods.len(), 3);
    assert_eq!(result.groups[0].group_name, "group_1");
    assert_eq!(result.groups[1].group_name, "group_2");
    assert_eq!(result.groups[2].group_name, "group_10");
}

/// 测试：深度扫描正确统计 INI 中各类型段的数量
#[test]
fn test_ini_section_counting() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");

    let ini_content = r#"
[Constants]
x = 1

[KeyTest]
key = VkA

[KeyPressB]
key = VkB

[TextureOverrideTex1]
hash = 0x1

[TextureOverrideTex2]
hash = 0x2

[ShaderOverrideS1]
hash = 0x3

[CommandListCL1]
x = 1

[ResourceR1]
filename = test.dds
"#;
    create_mod_with_ini(&group_path, "CountingMod", ini_content);

    let result = scan_mods_deep(dir.path()).unwrap();
    let m = &result.mods[0];
    assert_eq!(m.key_sections, 2);
    assert_eq!(m.texture_override_sections, 2);
    assert_eq!(m.shader_override_sections, 1);
    assert_eq!(m.command_list_sections, 1);
    assert_eq!(m.resource_sections, 1);
    assert_eq!(m.total_section_count, 8);
}

/// 测试：深度扫描正确检测 INI 中的 `include` 指令
#[test]
fn test_has_include_detection() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");

    let ini_content = "include = common.ini\n[Section]\nx=1\n";
    create_mod_with_ini(&group_path, "IncludeMod", ini_content);

    let result = scan_mods_deep(dir.path()).unwrap();
    let ini_data = &result.mods[0].mod_ini_data[0];
    assert!(ini_data.has_include);
}

/// 测试：深度扫描正确提取 INI 中的 `namespace` 变量
#[test]
fn test_namespace_extraction() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");

    let ini_content = "namespace = MyTestMod\n[TextureOverrideT]\nhash=1\n";
    create_mod_with_ini(&group_path, "NsMod", ini_content);

    let result = scan_mods_deep(dir.path()).unwrap();
    assert!(result.mods[0].is_namespaced);
    assert_eq!(result.mods[0].namespace, Some("MyTestMod".to_string()));
}
