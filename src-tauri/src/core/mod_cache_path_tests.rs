use super::*;

use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// 测试：find_group_node 按完整路径在顶层分组中定位节点（含互斥组）。
#[test]
fn find_group_node_matches_by_full_path() {
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join("_MANAGED_");
    fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
    fs::create_dir_all(managed.join("group_1").join("ModA")).unwrap();
    fs::write(
        managed.join("group_1").join("ModA").join("mod.ini"),
        "[Section]\nx=1\n",
    )
    .unwrap();
    // 互斥组根：根下直接一个模组 + 嵌套子分组一个模组
    fs::create_dir_all(managed.join("#Mutex").join("ModY")).unwrap();
    fs::write(
        managed.join("#Mutex").join("ModY").join("mod.ini"),
        "[Section]\nx=1\n",
    )
    .unwrap();
    fs::create_dir_all(managed.join("#Mutex").join("Sub").join("ModX")).unwrap();
    fs::write(
        managed
            .join("#Mutex")
            .join("Sub")
            .join("ModX")
            .join("mod.ini"),
        "[Section]\nx=1\n",
    )
    .unwrap();

    let scan = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();

    let g1 = find_group_node(&scan, &managed.join("group_1"));
    assert!(g1.is_some(), "应能定位 group_1 节点");
    let mutex = find_group_node(&scan, &managed.join("#Mutex"));
    assert!(mutex.is_some(), "应能定位互斥组根节点");
    assert_eq!(
        mutex.unwrap().group_type,
        crate::models::enums::GroupType::MutexGroup
    );
    assert!(
        find_group_node(&scan, &managed.join("group_9")).is_none(),
        "不存在的分组应返回 None"
    );
}

/// 测试：find_group_node 在 Windows 下容忍 verbatim 前缀（`\\?\`）与大小写差异。
///
/// 命令层传入的路径经 canonicalize 为 verbatim 形态，而缓存中 full_path
/// 为 get_mods 时前端传入的普通形态，两者必须仍能匹配。
#[cfg(windows)]
#[test]
fn find_group_node_tolerates_verbatim_and_case_diff() {
    let dir = TempDir::new().unwrap();
    let mods_path = dir.path().to_path_buf();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(managed.join("group_1").join("ModA")).unwrap();
    fs::write(
        managed.join("group_1").join("ModA").join("mod.ini"),
        "[Section]\nx=1\n",
    )
    .unwrap();

    let scan = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
    let g1 = managed.join("group_1");
    // verbatim 前缀形态（canonicalize 产物）
    let verbatim = PathBuf::from(format!("\\\\?\\{}", g1.to_string_lossy()));
    assert!(
        find_group_node(&scan, &verbatim).is_some(),
        "verbatim 前缀形态应能匹配缓存节点"
    );
    // 大小写差异形态
    let upper = PathBuf::from(g1.to_string_lossy().to_uppercase());
    assert!(
        find_group_node(&scan, &upper).is_some(),
        "大小写差异形态应能匹配缓存节点"
    );
}
