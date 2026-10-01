use super::*;
use std::fs;
use tempfile::TempDir;

/// 创建测试辅助：在指定目录下创建含 .ini 的模组目录
fn create_mod_dir(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    fs::create_dir_all(&dir).unwrap();
    // 创建一个 .ini 文件使该目录被识别为叶子模组节点
    fs::write(dir.join(format!("{}.ini", name)), "[ShaderOverride]").unwrap();
    dir
}

/// 正常场景：禁用3个启用模组，跳过1个已禁用模组，跳过无子分组目录
#[test]
fn test_disable_all_basic() {
    let tmp = TempDir::new().unwrap();
    let group = tmp.path().join("group_1");
    fs::create_dir_all(&group).unwrap();

    let _mod_a = create_mod_dir(&group, "ModA"); // 启用
    let _mod_b = create_mod_dir(&group, "DISABLEDModB"); // 已禁用
    let _mod_c = create_mod_dir(&group, "ModC"); // 启用
                                                 // 子分组目录（无 .ini），应跳过
    let subgroup = group.join("SubGroup");
    fs::create_dir_all(&subgroup).unwrap();
    let _mod_d = create_mod_dir(&group, "ModD"); // 启用

    let count = disable_all_mods_in_group(&group).unwrap().len() as u32;
    assert_eq!(count, 3); // ModA, ModC, ModD

    // 验证目录状态
    let entries: Vec<String> = fs::read_dir(&group)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(entries.iter().any(|n| n == "DISABLEDModA"), "ModA 应被禁用");
    assert!(
        entries.iter().any(|n| n == "DISABLEDModB"),
        "DisabledModB 应保持不变"
    );
    assert!(entries.iter().any(|n| n == "DISABLEDModC"), "ModC 应被禁用");
    assert!(entries.iter().any(|n| n == "DISABLEDModD"), "ModD 应被禁用");
    assert!(
        entries.iter().any(|n| n == "SubGroup"),
        "子分组目录应保持不变"
    );
    assert_eq!(entries.len(), 5, "目录数量不变（重命名不改变数量）");
}

/// 幂等性：对已全部禁用的目录不产生任何变化
#[test]
fn test_disable_all_idempotent() {
    let tmp = TempDir::new().unwrap();
    let group = tmp.path().join("group_1");
    fs::create_dir_all(&group).unwrap();

    create_mod_dir(&group, "DISABLEDModA");
    create_mod_dir(&group, "DISABLEDModB");

    let count = disable_all_mods_in_group(&group).unwrap().len() as u32;
    assert_eq!(count, 0, "全部已禁用时计数为0");
}

/// 幂等性：对已含 DISABLED 前缀的目录不重复加前缀（仍为 DISABLEDxxx 而非 DISABLEDDISABLEDxxx）
#[test]
fn test_disable_all_no_duplicate_prefix_on_disabled() {
    let tmp = TempDir::new().unwrap();
    let group = tmp.path().join("group_1");
    fs::create_dir_all(&group).unwrap();

    create_mod_dir(&group, "DISABLEDModA");
    create_mod_dir(&group, "DISABLEDModB");
    create_mod_dir(&group, "ModC");

    let count = disable_all_mods_in_group(&group).unwrap().len() as u32;
    assert_eq!(count, 1, "仅未禁用的 ModC 应被禁用");

    let entries: Vec<String> = fs::read_dir(&group)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        entries.iter().any(|n| n == "DISABLEDModA"),
        "DISABLEDModA 应保持不变"
    );
    assert!(
        entries.iter().any(|n| n == "DISABLEDModB"),
        "DISABLEDModB 应保持不变"
    );
    assert!(
        entries.iter().all(|n| !n.starts_with("DISABLEDDISABLED")),
        "不应产生 DISABLEDDISABLEDxxx 重复前缀目录"
    );
}

/// 无模组场景：仅有子分组或空目录
#[test]
fn test_disable_all_no_mods() {
    let tmp = TempDir::new().unwrap();
    let group = tmp.path().join("group_1");
    fs::create_dir_all(&group).unwrap();

    let subgroup = group.join("SubGroup");
    fs::create_dir_all(&subgroup).unwrap();

    let count = disable_all_mods_in_group(&group).unwrap().len() as u32;
    assert_eq!(count, 0);
}

/// 组目录含 `groupname` 标记文件时，批量禁用不得将其重命名为 `DISABLEDgroupname`
///
/// 真实磁盘上 group_xx 目录必有 `groupname` 标记文件（与模组目录并存）。
/// 批量禁用仅处理「目录 + 含 .ini」的叶子模组节点；标记文件（groupname/
/// selectedindex/modname）虽位于组目录下，但为普通文件，必须原样保留——
/// 若逻辑误按条目名加 DISABLED 前缀，会产生 `DISABLEDgroupname` 垃圾文件。
#[test]
fn test_disable_all_keeps_groupname_marker() {
    let tmp = TempDir::new().unwrap();
    let group = tmp.path().join("group_1");
    fs::create_dir_all(&group).unwrap();

    // 模拟真实组目录：标记文件 + 模组子目录并存
    fs::write(group.join("groupname"), "My Custom Group\n").unwrap();
    fs::write(group.join("selectedindex"), "1\n").unwrap();
    create_mod_dir(&group, "ModA");

    let count = disable_all_mods_in_group(&group).unwrap().len() as u32;
    assert_eq!(count, 1, "仅 ModA 应被禁用");

    assert!(
        fs::read_to_string(group.join("groupname")).unwrap() == "My Custom Group\n",
        "groupname 标记文件内容应原样保留"
    );
    assert!(
        group.join("selectedindex").exists(),
        "selectedindex 标记文件应原样保留"
    );
    assert!(
        !group.join("DISABLEDgroupname").exists(),
        "不得把 groupname 标记文件重命名为 DISABLEDgroupname"
    );
    assert!(group.join("DISABLEDModA").exists(), "ModA 应被禁用");
}
