//! 扁平化 / 导入域测试（archive_handler.rs 测试按域分发 PA-3）

use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn unwrap_single_wrapper_flattens_into_target() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("mod_top");
    fs::create_dir_all(top.join("MyArchive")).unwrap();
    fs::write(top.join("MyArchive").join("mod.ini"), b"[x]").unwrap();
    fs::write(top.join("MyArchive").join("tex.png"), b"PNG").unwrap();

    unwrap_single_folder_nesting(&top).unwrap();

    // 包裹目录应被展平，文件上移到 top
    assert!(top.join("mod.ini").exists());
    assert!(top.join("tex.png").exists());
    assert!(!top.join("MyArchive").exists());
}

#[test]
fn unwrap_excluded_metadata_does_not_block_flatten() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("mod_top");
    fs::create_dir_all(top.join("MyArchive")).unwrap();
    fs::write(top.join("MyArchive").join("sub.txt"), b"hello").unwrap();
    // modname 是排除项：与其共存时，唯一“相关”条目仍是 MyArchive，应继续展平
    fs::write(top.join("modname"), b"My Mod Name").unwrap();

    unwrap_single_folder_nesting(&top).unwrap();

    assert!(top.join("sub.txt").exists());
    assert!(top.join("modname").exists());
    assert!(!top.join("MyArchive").exists());
}

#[test]
fn unwrap_loops_through_nested_single_wrappers() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("mod_top");
    fs::create_dir_all(top.join("A").join("B").join("C")).unwrap();
    fs::write(top.join("A").join("B").join("C").join("deep.txt"), b"x").unwrap();

    unwrap_single_folder_nesting(&top).unwrap();

    // 三层嵌套单层包裹应被循环展平到顶层
    assert!(top.join("deep.txt").exists());
    assert!(!top.join("A").exists());
}

#[test]
fn unwrap_no_flatten_when_multiple_relevant_entries() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("mod_top");
    fs::create_dir_all(top.join("FolderA")).unwrap();
    fs::create_dir_all(top.join("FolderB")).unwrap();
    fs::write(top.join("FolderA").join("a.txt"), b"a").unwrap();
    fs::write(top.join("FolderB").join("b.txt"), b"b").unwrap();

    unwrap_single_folder_nesting(&top).unwrap();

    // 两个相关目录 → 不展平
    assert!(top.join("FolderA").join("a.txt").exists());
    assert!(top.join("FolderB").join("b.txt").exists());
}

#[test]
fn unwrap_no_flatten_when_single_entry_is_file() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("mod_top");
    fs::create_dir_all(&top).unwrap();
    // 非排除项的单个文件（非目录）→ 停止，不“展平”
    fs::write(top.join("game.exe"), b"BINARY").unwrap();

    unwrap_single_folder_nesting(&top).unwrap();

    assert!(top.join("game.exe").exists());
}
