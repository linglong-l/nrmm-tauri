use super::*;

use std::fs;
use tempfile::TempDir;

/// 测试：collect_group_mod_paths 递归收集互斥组 children 内模组、排除 None 空槽位。
#[test]
fn collect_group_mod_paths_recurses_children_and_excludes_none() {
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

    // 普通分组：None 空槽位被排除，仅收集真实模组
    let g1 = find_group_node(&scan, &managed.join("group_1")).unwrap();
    assert!(
        g1.mods.iter().any(|m| m.name == "None"),
        "前置条件：普通分组 light 扫描含 None 空槽位"
    );
    let g1_paths = collect_group_mod_paths(g1);
    assert_eq!(g1_paths.len(), 1, "None 空槽位不应计入");
    assert!(
        g1_paths[0].ends_with("ModA"),
        "应收集真实模组 mod_path，实际 {}",
        g1_paths[0]
    );

    // 互斥组：递归收集嵌套 children（Sub/ModX）内的模组
    let mutex = find_group_node(&scan, &managed.join("#Mutex")).unwrap();
    let mutex_paths = collect_group_mod_paths(mutex);
    assert_eq!(mutex_paths.len(), 2, "根下 + 嵌套子分组内模组均应收集");
    assert!(mutex_paths.iter().any(|p| p.ends_with("ModY")));
    assert!(mutex_paths.iter().any(|p| p.ends_with("ModX")));
}
