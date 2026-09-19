//! 模组扫描模块
//!
//! 提供两种扫描模式：
//! - **轻量扫描 (scan_mods_light)**: 不解析 INI 内容，仅检查目录结构和标记文件，速度快，用于 UI 列表展示
//! - **深度扫描 (scan_mods_deep)**: 完整解析所有 INI 文件，统计段数量、检测错误、提取 namespace，用于 apply 时的 INI 注入
//!
//! # 分组类型
//! - **NormalGroup (group_xx)**: 普通分组，一级子目录为模组，不递归。每组同一时间只能启用一个模组（互斥槽位）
//! - **MutexGroup (非 group_xx 目录)**: 互斥组，支持任意深度嵌套，同级目录下的模组互斥
//!
//! # 关键设计
//! - **栈式非递归 DFS**: MutexGroup 使用 `Vec<DfsStackItem>` 模拟栈进行非递归深度优先遍历，避免递归调用栈溢出
//! - **rayon 并行遍历**: NormalGroup 和 MutexGroup 根目录使用 `par_iter()` 并行扫描，充分利用多核 CPU
//! - **HashSet 规范化路径去重**: 使用 `Path::canonicalize()` 获取绝对路径后存入 `HashSet`，防止符号链接/硬链接导致无限循环或重复扫描
//! - **`entry.file_type()` 替代 `metadata()`**: 遍历目录时使用 `DirEntry::file_type()` 而非 `fs::metadata()`，减少系统调用次数，提高性能
//! - 轻量扫描不递归 NormalGroup，避免遍历 vendor 等大目录
//! - 排序规则：启用优先 > 收藏优先 > 最新收藏 > 自然排序
//! - 每个 NormalGroup 自动添加 "None" 空槽位（索引 0），表示不选任何模组

#[cfg(test)]
use crate::core::constants;
#[cfg(test)]
pub(crate) use crate::core::mod_scan_deep::scan_mods_deep;
#[cfg(test)]
use crate::models::enums::{GroupType, ModsPathStatus, TargetGame};
#[cfg(test)]
use std::path::{Path, PathBuf};

// 过渡门面：外部消费方经 mod_scanner::X 保持可达（收口 P5-7 统一切换新域路径）
pub use crate::core::mod_scan_light::{
    check_mods_path, scan_mods, scan_mods_light, scan_single_mod, ScanResult,
};
pub use crate::core::mod_scan_marker::mark_restore_safe_window;
#[cfg(test)]
pub(crate) use crate::core::mod_scan_marker::{test_clear_restore_safe_window, TEST_WINDOW_LOCK};
pub use crate::core::mod_scan_partial::scan_partial_path;
pub use crate::core::mod_scan_util::{
    derive_group_active_state, dir_has_ini_file, is_disabled_dir, is_group_xx_dir,
    is_normal_group_dir, strip_disabled_prefix, GroupModSlot,
};
pub(crate) use crate::core::mod_scan_util::{group_order_cmp, recount_mods};

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
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
    ///
    /// 构造启用 + 禁用（DISABLED_ 前缀）各一个模组，运行 scan_mods_deep 后断言：
    /// 1. 禁用模组仍被发现（disabled=true，ini_path 保留——update 需要路径做状态展示）；
    /// 2. 禁用模组的 section 计数为 0（未执行内容解析）；
    /// 3. 启用模组的 section 计数正常（解析未受影响）。
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
    ///
    /// 构造同一模组目录下的两个 INI：a.ini 通过 `run` 引用 b.ini 声明的
    /// ShaderOverride 库，同时再引用一个完全未声明的库。断言：
    /// 1. 引用已声明库（应合计为全局 known_libraries）不产生 NON EXISTENT LIB（error_type=5）；
    /// 2. 引用未声明库仍产生 error_type=5。
    #[test]
    fn test_deep_cross_ini_library_reference_resolution() {
        let dir = setup_test_dir();
        let group = create_group_dir(dir.path(), "group_1");
        let mod_path = group.join("MyMod");
        fs::create_dir_all(&mod_path).unwrap();
        // a.ini：引用 b.ini 声明的库 + 一个未声明库
        fs::write(
            mod_path.join("a.ini"),
            "[ShaderOverrideShaderA]\nhash = 0xAAA\nrun = ShaderOverrideShaderB\nrun = ShaderOverrideMissingLib\n",
        )
        .unwrap();
        // b.ini：声明 ShaderOverrideShaderB 库
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
        // 深度扫描下应该能找到 NestedMod
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
        // d3dx.ini 位于游戏根（Mods 父目录），不在 Mods/ 内
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

    // ========== 轻量扫描测试（scan_mods_light） ==========

    /// 测试：轻量扫描 NormalGroup 会自动创建 None 空槽位
    ///
    /// 验证：每个 NormalGroup 的第一个模组始终是索引为 0 的 None 槽位
    #[test]
    fn test_light_scan_none_slot() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&group_path, "TestMod", "[Section]\n");

        let result = scan_mods_light(dir.path()).unwrap();
        assert_eq!(result.groups.len(), 1);
        // None + TestMod
        let group = &result.groups[0];
        assert!(group.mods.iter().any(|m| m.name == "None"));
        assert!(group.mods.iter().any(|m| m.name == "TestMod"));
    }

    /// 测试：轻量扫描 NormalGroup 不递归子目录
    ///
    /// 验证：`SubDir` 是一级子目录被扫描为模组，但 `SubDir/NestedMod` 是二级子目录不被扫描
    #[test]
    fn test_light_scan_no_recursion_normal_group() {
        // 轻量扫描下 NormalGroup 绝对不递归：SubDir 是一级子目录会被扫描为模组，
        // 但 SubDir/NestedMod 是二级子目录，不会被扫描到
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
        // NestedMod 在子目录中，不应该被扫描到
        assert!(!group.mods.iter().any(|m| m.name == "NestedMod"));
        // SubDir 是一级子目录，会被扫描为模组；加上 None 槽位，共 2 个
        assert!(group.mods.iter().any(|m| m.name == "SubDir"));
        assert_eq!(group.mods.len(), 2); // None + SubDir
    }

    /// 测试：轻量扫描会自动创建缺失的标记文件（groupname、selectedindex、modname）
    ///
    /// 正常扫描语义（还原安全窗口外）：标记缺失照常新建。
    /// 还原窗口内的只读行为由 [`test_restore_safe_window_markers_readonly`] 覆盖。
    #[test]
    fn test_light_scan_creates_marker_files() {
        // 与还原安全窗口测试互斥（窗口生效时不创建标记）
        let _window_lock = super::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");
        let mod_path = group_path.join("TestMod");
        fs::create_dir_all(&mod_path).unwrap();

        // 标记文件不存在
        assert!(!group_path.join("groupname").exists());
        assert!(!group_path.join("selectedindex").exists());
        assert!(!mod_path.join("modname").exists());

        let _result = scan_mods_light(dir.path()).unwrap();

        // 标记文件应该被创建
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
        // 已存在的文件内容不被改写（mtime 之外的直接验证：内容原样保留）
        assert_eq!(
            fs::read_to_string(group_path.join("groupname")).unwrap(),
            "My Custom Group\n"
        );
    }

    /// 测试：轻量扫描 MutexGroup 不会创建标记文件（与 NormalGroup 不同）
    ///
    /// 验证：MutexGroup 目录下不会出现 groupname、selectedindex、modname 等标记文件
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

        // MutexGroup 不应该创建 groupname/selectedindex/modname
        assert!(!mutex_path.join("groupname").exists());
        assert!(!mutex_path.join("selectedindex").exists());
        assert!(!mod_path.join("modname").exists());
    }

    /// 测试：轻量扫描 MutexGroup 的栈式 DFS 能正确遍历嵌套目录
    ///
    /// 验证：嵌套结构 `#MutexRoot/Category1/ModA`、`#MutexRoot/Category1/SubCategory/ModB`、`#MutexRoot/ModC` 全部被扫描到
    #[test]
    fn test_light_scan_mutex_group_dfs() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // 创建嵌套 MutexGroup 结构：
        // #MutexRoot/
        //   Category1/
        //     ModA/ (有 ini)
        //     SubCategory/
        //       ModB/ (有 ini)
        //   ModC/ (有 ini)
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

        // 应该有一个分组（#MutexRoot）
        assert_eq!(result.groups.len(), 1);
        let root_group = &result.groups[0];
        assert_eq!(root_group.group_type, GroupType::MutexGroup);

        // 检查 mods 是否包含所有模组（通过 all_mods 收集）
        let mod_names: Vec<&str> = result.mods.iter().map(|m| m.name.as_str()).collect();
        assert!(mod_names.contains(&"ModA"), "Should find ModA");
        assert!(mod_names.contains(&"ModB"), "Should find ModB");
        assert!(mod_names.contains(&"ModC"), "Should find ModC");

        // 检查 is_mutex 标记
        for m in &result.mods {
            assert!(m.is_mutex, "Mutex mod should have is_mutex=true");
        }
    }

    /// 测试：轻量扫描 DFS 遇到含 INI 的目录后停止向下遍历
    ///
    /// 验证：`ModWithIni` 目录下有 INI，其子目录 `SubModUnder` 不被扫描
    #[test]
    fn test_light_scan_stops_at_ini_dir() {
        // 有 .ini 的目录不遍历其子目录
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let root = dir.path().join("_MANAGED_").join("#MutexRoot");
        fs::create_dir_all(&root).unwrap();

        let mod_dir = root.join("ModWithIni");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("mod.ini"), "[Section]\n").unwrap();

        // 模组目录下有子目录，应该不被扫描
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

        // NormalGroup 禁用模组
        let group_path = create_group_dir(dir.path(), "group_1");
        let mod_enabled = group_path.join("EnabledMod");
        fs::create_dir_all(&mod_enabled).unwrap();
        let mod_disabled = group_path.join("DISABLEDDisabledMod");
        fs::create_dir_all(&mod_disabled).unwrap();

        // MutexGroup 禁用模组
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

        // NormalGroup fav
        let group_path = create_group_dir(dir.path(), "group_1");
        let mod_fav = group_path.join("FavMod");
        fs::create_dir_all(&mod_fav).unwrap();
        fs::write(mod_fav.join("fav"), "").unwrap();
        let mod_normal = group_path.join("NormalMod");
        fs::create_dir_all(&mod_normal).unwrap();

        // MutexGroup fav
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
    ///
    /// 验证：`icon.png` 优先于 `preview.png` 和 `other.png`
    #[test]
    fn test_light_scan_icon_priority() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let mod_path = group_path.join("IconTest");
        fs::create_dir_all(&mod_path).unwrap();
        // 有 preview.png 和 other.png，但 icon.png 优先
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

        // NormalGroup
        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "NormalMod", "[S]\n");

        // MutexGroup
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
    ///
    /// 验证：`group_0`（零值）和 `group_01`（前导零）被作为 MutexGroup 处理，`group_1` 作为 NormalGroup
    #[test]
    fn test_light_scan_group_regex_strict() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        // group_0 应该被拒绝（零值）
        let g0 = dir.path().join("_MANAGED_").join("group_0");
        fs::create_dir_all(&g0).unwrap();
        create_mod_with_ini(&g0, "Mod0", "[S]\n");

        // group_01 应该被拒绝（前导零）
        let g01 = dir.path().join("_MANAGED_").join("group_01");
        fs::create_dir_all(&g01).unwrap();
        create_mod_with_ini(&g01, "Mod01", "[S]\n");

        // group_1 应该被接受
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
        // group_0 和 group_01 应该作为 MutexGroup 存在
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
    ///
    /// 验证：None 固定第一，然后是 fav_mod（收藏），mod2 < mod10（自然排序），禁用模组最后
    #[test]
    fn test_light_scan_sorting() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        // 创建 mod10, mod2, disabled_mod, fav_mod
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

        // 第一个应该是 None（固定位置，不参与排序）
        assert_eq!(group.mods[0].name, "None");
        // 然后是 fav_mod（收藏优先）
        assert_eq!(group.mods[1].name, "fav_mod");
        // 然后是 mod2（自然排序 mod2 < mod10）
        assert_eq!(group.mods[2].name, "mod2");
        assert_eq!(group.mods[3].name, "mod10");
        // 禁用的在最后（bad 是 DISABLEDbad 去掉前缀后的名字）
        assert_eq!(group.mods[4].name, "bad");
        assert_eq!(group.mods.len(), 5);
    }

    /// 测试（R3/C-3b）：轻量扫描为模组填充磁盘签名。
    ///
    /// 签名 = mod 目录 mtime ^ fav mtime ^ 组 selectedindex mtime 三源混合：
    /// - NormalGroup 与 MutexGroup 模组均非零；
    /// - 无磁盘变化时两次扫描签名相等（稳定，复用判定前提）；
    /// - fav 创建（fav 源 0→t）→ 该模组签名变化，未动兄弟不变；
    /// - 组 selectedindex 改写 → 组内全部模组签名变化（覆盖 is_active 派生源）；
    /// - 深度扫描不填充（0 = 无签名，永不参与增量复用）。
    #[test]
    fn test_light_scan_disk_signature_sources() {
        // 首扫依赖标记文件创建（modname/selectedindex 影响 mtime 签名），
        // 与还原安全窗口测试互斥
        let _window_lock = super::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&group_path, "ModA", "[Section]\n");
        create_mod_with_ini(&group_path, "FavMod", "[Section]\n");
        // 互斥组模组
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

        // 首扫会创建 modname/selectedindex 标记文件；二次扫描读已有文件（mtime 稳定）
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

        // fav 源：FavMod 创建 fav → 其签名变化，未动的 ModA 签名不变
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

        // 组 selectedindex 源：改写组目录 selectedindex → 组内全部模组签名变化
        fs::write(group_path.join("selectedindex"), "1").unwrap();
        let r4 = scan_mods_light(dir.path()).unwrap();
        assert_ne!(
            find_sig(&r3, "ModA"),
            find_sig(&r4, "ModA"),
            "selectedindex mtime 源应参与签名（覆盖 is_active 派生源）"
        );
        // 互斥组不受该组 selectedindex 影响（各自组源独立）
        assert_eq!(find_sig(&r3, "ModX"), find_sig(&r4, "ModX"));

        // 深度扫描不填充签名（其产物不参与增量复用）
        let deep = scan_mods_deep(dir.path()).unwrap();
        assert!(
            deep.mods.iter().all(|m| m.disk_signature == 0),
            "深扫产物签名应为 0（无签名 → 永不复用）"
        );
    }
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
        // 非group目录
        let non_group = mods_root.join("_MANAGED_").join("#CustomGroup");
        fs::create_dir_all(&non_group).unwrap();
        let mod_dir = non_group.join("ModA");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("ModA.ini"), "[ShaderOverride]\n").unwrap();

        // 执行扫描（使用轻量扫描，只有它会扫描 MutexGroup 并创建标记文件）
        let result = scan_mods_light(mods_root).expect("scan should succeed");

        // 检查不应存在的标记文件
        assert!(
            !non_group.join("groupname").exists(),
            "非group不应创建 groupname 文件"
        );
        assert!(
            !mod_dir.join("modname").exists(),
            "非group下模组不应创建 modname 文件"
        );

        // 但模组仍应被正确识别
        let mod_names: Vec<&str> = result.mods.iter().map(|m| m.name.as_str()).collect();
        assert!(mod_names.contains(&"ModA"), "ModA 应被扫描到，名称为 ModA");
    }

    /// 扫描 group_1：正常语义下 groupname / modname 标记文件均创建
    #[test]
    fn test_scan_group_xx_still_creates_markers() {
        // 与还原安全窗口测试互斥（窗口生效时不创建标记）
        let _window_lock = super::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join(constants::MANAGED_FOLDER);
        let group1 = managed.join("group_1");
        fs::create_dir_all(&group1).unwrap();
        let mod_dir = group1.join("ModA");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("ModA.ini"), "[ShaderOverride]\n").unwrap();

        // 使用轻量扫描：正常语义（还原安全窗口外）缺失标记照常新建
        let _result = scan_mods_light(mods_root).expect("scan should succeed");

        // group_xx 目录应创建标记文件
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

    /// 单模组扫描产出与组级扫描同口径的磁盘派生字段：
    /// 名称（modname 标记优先）、DISABLED 解析、fav、is_active（唯一启用者）、组定位。
    #[test]
    fn scan_single_mod_normal_group_fields() {
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "ModA");
        let mod_b = make_mod(&g1, "DISABLED_ModB");
        // modname 标记优先于目录名
        fs::write(mod_a.join("modname"), "My Display Name\n").unwrap();
        // fav 标记
        fs::write(mod_b.join(constants::FAV_MARKER), "").unwrap();

        // ModA：唯一启用模组 → is_active = true
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

        // ModB：禁用 + 收藏 + 非 active
        let (arc_b, _) = scan_single_mod(dir.path(), &mod_b).unwrap();
        assert!(arc_b.disabled && arc_b.mod_disabled, "DISABLED_ 前缀解析");
        assert_eq!(arc_b.name, "ModB", "禁用模组展示名剥离前缀");
        assert!(arc_b.is_favorite, "fav 标记存在");
        assert!(!arc_b.is_active, "禁用模组不 active");
    }

    /// modname 标记不存在时创建并回写目录名（NRMM 对齐）
    #[test]
    fn scan_single_mod_creates_modname_marker() {
        // 与还原安全窗口测试互斥（窗口生效时不创建标记）
        let _window_lock = super::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = setup();
        let g1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_a = make_mod(&g1, "FreshMod");
        assert!(!mod_a.join("modname").exists());

        let (arc, _) = scan_single_mod(dir.path(), &mod_a).unwrap();
        assert_eq!(arc.name, "FreshMod", "无 modname 时以目录名为展示名");
        assert!(
            mod_a.join("modname").exists(),
            "scan_single_mod 应创建 modname 标记（对齐组扫描语义）"
        );
    }

    /// is_active 组语义近似：多个启用模组（默认 selectedindex=0 → None 槽位 active）
    /// 时所有真实模组均非 active，与整组重扫派生一致
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

        // 对照：整组重扫同口径（多启用回退 selectedindex=0 → None active）
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

        // 不含 .ini 的目录
        let no_ini = g1.join("NoIni");
        fs::create_dir_all(&no_ini).unwrap();
        assert!(scan_single_mod(dir.path(), &no_ini).is_err());

        // _MANAGED_ 根自身
        assert!(scan_single_mod(dir.path(), &dir.path().join("_MANAGED_")).is_err());

        // 组外的任意路径（不在 _MANAGED_ 下）
        let outside = dir.path().join("Outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("x.ini"), "[S]\n").unwrap();
        assert!(scan_single_mod(dir.path(), &outside).is_err());

        // 合法模组目录不应报错
        assert!(scan_single_mod(dir.path(), &mod_a).is_ok());
    }

    /// 单模组扫描的磁盘派生字段与整组扫描产出一致（除 mod_index/is_active 占位外）：
    /// name/disabled/favorite/is_active/签名三源一致（正确性金标准的字段级子集）
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
        // 唯一启用者 → 两路径 is_active 均为 true
        assert_eq!(single.is_active, group_mod_a.is_active);
        // 签名一致（三源 mtime 相同）
        assert_eq!(single.disk_signature, group_mod_a.disk_signature);
    }
}
