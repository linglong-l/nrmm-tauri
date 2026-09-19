use super::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// 测试辅助：创建带 _MANAGED_ 子目录的临时根目录
fn setup_test_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();
    dir
}

/// 测试辅助：在 _MANAGED_ 下创建分组目录
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

/// 测试辅助：在根目录创建 d3dx.ini
fn create_d3dx_ini(base: &Path) {
    fs::write(base.join("d3dx.ini"), "; test").unwrap();
}

/// 空 target_subpath 应降级为全量扫描，返回所有分组
#[test]
fn test_partial_empty_subpath_falls_back_to_full_scan() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let g1 = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");
    let g2 = create_group_dir(dir.path(), "group_2");
    create_mod_with_ini(&g2, "Mod2", "[TextureOverride2]\nhash=2\n");

    // 空路径降级
    let result = scan_partial_path(dir.path(), Path::new("")).unwrap();
    assert_eq!(
        result.groups.len(),
        2,
        "空 target_subpath 应降级为全量扫描，返回 2 个分组"
    );
}

/// target_subpath 指向 _MANAGED_ 根目录应降级为全量扫描
#[test]
fn test_partial_managed_root_falls_back_to_full_scan() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let g1 = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");
    let g2 = create_group_dir(dir.path(), "group_2");
    create_mod_with_ini(&g2, "Mod2", "[TextureOverride2]\nhash=2\n");

    let result = scan_partial_path(dir.path(), Path::new("_MANAGED_")).unwrap();
    assert_eq!(
        result.groups.len(),
        2,
        "target_subpath=_MANAGED_ 应降级为全量扫描"
    );
}

/// target_subpath 不存在时应降级为全量扫描
#[test]
fn test_partial_nonexistent_path_falls_back_to_full_scan() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let g1 = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");

    // 指向不存在的分组
    let result = scan_partial_path(dir.path(), Path::new("_MANAGED_/group_999")).unwrap();
    assert_eq!(
        result.groups.len(),
        1,
        "目标路径不存在时应降级为全量扫描，返回已存在的 1 个分组"
    );
}

/// target_subpath 指向 NormalGroup（group_xx）时仅扫描该分组
#[test]
fn test_partial_normal_group_scans_only_target_group() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let g1 = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");
    let g2 = create_group_dir(dir.path(), "group_2");
    create_mod_with_ini(&g2, "Mod2", "[TextureOverride2]\nhash=2\n");
    let g10 = create_group_dir(dir.path(), "group_10");
    create_mod_with_ini(&g10, "Mod10", "[TextureOverride10]\nhash=10\n");

    // 仅扫描 group_2
    let result = scan_partial_path(dir.path(), Path::new("_MANAGED_/group_2")).unwrap();

    // 仅返回 group_2，不包含 group_1 和 group_10
    assert_eq!(result.groups.len(), 1, "应仅返回目标分组");
    assert_eq!(result.groups[0].group_name, "group_2");
    // 模组仅包含 Mod2（轻量扫描会插入 None 槽位，故长度为 2）
    assert_eq!(result.mods.len(), 2, "应包含 None 槽位 + Mod2");
    assert_eq!(result.mods.iter().filter(|m| m.name == "Mod2").count(), 1);
    // total_mods_count 不计入 None
    assert_eq!(result.total_mods_count, 1);
}

/// target_subpath 指向 MutexGroup（#xxx）时仅扫描该分组子树
#[test]
fn test_partial_mutex_group_scans_only_target_group() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    // 创建一个 MutexGroup：#CustomGroup 下含 ModA
    let mutex_group = dir.path().join("_MANAGED_").join("#CustomGroup");
    fs::create_dir_all(&mutex_group).unwrap();
    let mod_a = mutex_group.join("ModA");
    fs::create_dir_all(&mod_a).unwrap();
    fs::write(mod_a.join("ModA.ini"), "[ShaderOverride]\n").unwrap();

    // 另一个分组不应被扫描
    let g1 = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");

    // 仅扫描 #CustomGroup
    let result = scan_partial_path(dir.path(), Path::new("_MANAGED_/#CustomGroup")).unwrap();

    // 仅包含 ModA，不包含 Mod1
    assert_eq!(
        result.mods.iter().filter(|m| m.name == "ModA").count(),
        1,
        "应扫描到 ModA"
    );
    assert_eq!(
        result.mods.iter().filter(|m| m.name == "Mod1").count(),
        0,
        "不应扫描到 group_1 下的 Mod1"
    );
}

/// target_subpath 指向分组内的模组目录时，粒度仍为分组级（扫描整个分组）
#[test]
fn test_partial_mod_path_scans_whole_group() {
    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());

    let g1 = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");
    create_mod_with_ini(&g1, "Mod2", "[TextureOverride2]\nhash=2\n");

    let g2 = create_group_dir(dir.path(), "group_2");
    create_mod_with_ini(&g2, "Mod3", "[TextureOverride3]\nhash=3\n");

    // 指向 group_1 下的 Mod1（粒度仍为分组级）
    let result = scan_partial_path(dir.path(), Path::new("_MANAGED_/group_1/Mod1")).unwrap();

    // 应扫描整个 group_1（None + Mod1 + Mod2），不包含 group_2 的 Mod3
    assert_eq!(result.groups.len(), 1, "应仅返回 group_1");
    assert_eq!(result.groups[0].group_name, "group_1");
    assert_eq!(
        result.mods.iter().filter(|m| m.name == "Mod3").count(),
        0,
        "不应扫描到 group_2 的 Mod3"
    );
    // group_1 内的 Mod1 和 Mod2 都应被扫描到
    assert_eq!(
        result.mods.iter().filter(|m| m.name == "Mod1").count(),
        1,
        "应扫描到 Mod1"
    );
    assert_eq!(
        result.mods.iter().filter(|m| m.name == "Mod2").count(),
        1,
        "应扫描到 Mod2（粒度为分组级）"
    );
}

/// normalize_subpath 应统一分隔符、去除末尾斜杠，并剥离 `_MANAGED_` 前缀
#[test]
fn test_normalize_subpath_handles_separators() {
    // canonicalize 在空 mods_path 下会失败，走策略 2（字符串匹配）
    // Windows 风格反斜杠应被转换为正斜杠，并剥离 _MANAGED_ 前缀
    let n1 = normalize_subpath(Path::new("_MANAGED_\\group_1"), Path::new(""));
    assert_eq!(n1, "group_1");

    // 末尾斜杠应被去除
    let n2 = normalize_subpath(Path::new("_MANAGED_/group_1/"), Path::new(""));
    assert_eq!(n2, "group_1");

    // 混合分隔符
    let n3 = normalize_subpath(Path::new("_MANAGED_\\group_1/Mod1\\"), Path::new(""));
    assert_eq!(n3, "group_1/Mod1");

    // 不带 _MANAGED_ 前缀的相对路径应原样返回
    let n4 = normalize_subpath(Path::new("group_1"), Path::new(""));
    assert_eq!(n4, "group_1");

    // 仅 _MANAGED_ 应返回空字符串（触发全量扫描降级）
    let n5 = normalize_subpath(Path::new("_MANAGED_"), Path::new(""));
    assert_eq!(n5, "");
}

/// normalize_subpath 应正确处理绝对路径（文件监听器 consolidate 的返回格式）
#[test]
fn test_normalize_subpath_absolute_path() {
    let dir = setup_test_dir();
    let g1 = create_group_dir(dir.path(), "group_1");
    create_mod_with_ini(&g1, "Mod1", "[Section]\n");

    // 模拟 consolidate 返回的绝对路径：mods_path/_MANAGED_/group_1
    let abs_path = dir.path().join("_MANAGED_").join("group_1");
    let result = normalize_subpath(&abs_path, dir.path());
    assert_eq!(
        result, "group_1",
        "绝对路径应被剥离为相对于 _MANAGED_ 的子路径"
    );

    // 模组级绝对路径：mods_path/_MANAGED_/group_1/Mod1
    let abs_mod_path = g1.join("Mod1");
    let result2 = normalize_subpath(&abs_mod_path, dir.path());
    assert_eq!(result2, "group_1/Mod1", "模组级绝对路径应保留分组/模组结构");
}
