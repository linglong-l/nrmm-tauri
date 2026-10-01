use super::*;
use std::fs;
use tempfile::TempDir;

fn create_mod_dir(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{}.ini", name.trim_start_matches("DISABLED"))),
        "[ShaderOverride]",
    )
    .unwrap();
    dir
}

/// 正常场景：启用3个禁用模组，跳过1个启用模组，跳过子分组
#[test]
fn test_enable_all_basic() {
    let tmp = TempDir::new().unwrap();
    let group = tmp.path().join("group_1");
    fs::create_dir_all(&group).unwrap();

    create_mod_dir(&group, "DISABLEDModA"); // 需启用
    create_mod_dir(&group, "ModB"); // 已启用，跳过
    create_mod_dir(&group, "DISABLEDModC"); // 需启用
                                            // 子分组目录（无 .ini），跳过
    let subgroup = group.join("SubGroup");
    fs::create_dir_all(&subgroup).unwrap();
    create_mod_dir(&group, "DISABLEDModD"); // 需启用

    let count = enable_all_mods_in_group(&group).unwrap().len() as u32;
    assert_eq!(count, 3); // ModA, ModC, ModD

    let entries: Vec<String> = fs::read_dir(&group)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(entries.iter().any(|n| n == "ModA"), "ModA 应被启用");
    assert!(entries.iter().any(|n| n == "ModB"), "ModB 保持已启用");
    assert!(entries.iter().any(|n| n == "ModC"), "ModC 应被启用");
    assert!(entries.iter().any(|n| n == "ModD"), "ModD 应被启用");
    assert!(entries.iter().any(|n| n == "SubGroup"), "子分组不变");
}

/// 幂等性：全部已启用计数为0
#[test]
fn test_enable_all_idempotent() {
    let tmp = TempDir::new().unwrap();
    let group = tmp.path().join("group_1");
    fs::create_dir_all(&group).unwrap();
    create_mod_dir(&group, "ModA");
    create_mod_dir(&group, "ModB");

    let count = enable_all_mods_in_group(&group).unwrap().len() as u32;
    assert_eq!(count, 0);
}

/// 前缀后跟分隔符场景：DISABLED_MyMod 应为 MyMod
#[test]
fn test_enable_all_prefix_with_separator() {
    let tmp = TempDir::new().unwrap();
    let group = tmp.path().join("group_1");
    fs::create_dir_all(&group).unwrap();
    create_mod_dir(&group, "DISABLED_MyMod"); // 下划线分隔
    create_mod_dir(&group, "DISABLED-OtherMod"); // 横杠分隔
    create_mod_dir(&group, "DISABLED Third"); // 空格分隔

    let count = enable_all_mods_in_group(&group).unwrap().len() as u32;
    assert_eq!(count, 3);

    let entries: Vec<String> = fs::read_dir(&group)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(entries.iter().any(|n| n == "MyMod"));
    assert!(entries.iter().any(|n| n == "OtherMod"));
    assert!(entries.iter().any(|n| n == "Third"));
}

/// 组目录含 `groupname` 标记文件时，批量启用不得误将其当模组名处理
///
/// 与 `tests_disable_all::test_disable_all_keeps_groupname_marker` 对称：
/// 启用侧同样必须保留组内标记文件（groupname/selectedindex/modname），
/// 不得把它们当作 `DISABLED*` 模组目录去剥前缀或改名。
#[test]
fn test_enable_all_keeps_groupname_marker() {
    let tmp = TempDir::new().unwrap();
    let group = tmp.path().join("group_1");
    fs::create_dir_all(&group).unwrap();

    // 标记文件 + 已禁用模组并存（真实组目录形态）
    fs::write(group.join("groupname"), "My Custom Group\n").unwrap();
    fs::write(group.join("selectedindex"), "1\n").unwrap();
    create_mod_dir(&group, "DISABLEDModA");

    let count = enable_all_mods_in_group(&group).unwrap().len() as u32;
    assert_eq!(count, 1, "仅 DISABLEDModA 应被启用");

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
    assert!(group.join("ModA").exists(), "ModA 应被启用");
}
