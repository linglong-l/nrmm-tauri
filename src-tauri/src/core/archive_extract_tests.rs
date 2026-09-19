//! 解压域测试（archive_handler.rs 测试按域分发 PA-3）
//!
//! 智能解压（需求 2/3）：两条路径均实现单层包裹展平。

use super::*;
use crate::core::archive_export::export_internal;
use crate::core::archive_import::import_mod;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// 构造用于导入测试的源模组（顶层单一目录包裹，模拟 Bandizip 式冗余顶层目录）
fn make_wrapped_archive_src(root: &Path) -> PathBuf {
    let src = root.join("SrcMod");
    fs::create_dir_all(src.join("MyMod").join("Textures")).unwrap();
    fs::write(
        src.join("MyMod").join("mod.ini"),
        b"[Section]\nhandled = 1\n",
    )
    .unwrap();
    fs::write(
        src.join("MyMod").join("Textures").join("tex1.png"),
        b"PNGDATA",
    )
    .unwrap();
    src
}

/// 断言解压结果已展平：内容在 mod_path 直接可见，且无冗余 MyMod 包裹目录
fn assert_flattened(mod_path: &Path) {
    assert!(mod_path.join("mod.ini").exists(), "应展平到直接内容");
    assert!(
        mod_path.join("Textures").join("tex1.png").exists(),
        "子目录内容应上移"
    );
    assert!(!mod_path.join("MyMod").exists(), "冗余顶层目录应被去除");
}

#[test]
fn smart_extract_via_system_7z_cli_flatten_single_top_dir() {
    // 仅当系统/打包 7z 可用时才执行端到端 CLI 路径；否则跳过（不视为失败）
    let tmp = TempDir::new().unwrap();
    let managed = tmp.path().join("Mods").join("_MANAGED_").join("group_1");
    fs::create_dir_all(&managed).unwrap();

    let src = make_wrapped_archive_src(tmp.path());
    // 用 export_internal(.7z) 自维护压缩生成一个「顶层单目录包裹」的 7z（CLI 无密码路径可控）
    let archive = tmp.path().join("SrcMod.7z");
    export_internal(&src, &archive, "7z").unwrap();

    let seven_zip = get_system_7z_path()
        .or_else(get_bundled_7z_path)
        .expect("需要 7z CLI 才能跑 CLI 端到端智能解压测试");
    let _ = seven_zip; // 确认存在后再走 import_mod（其内部会再次查找）

    let result = import_mod(&archive, &managed, None).unwrap();
    match result {
        ExtractResult::Success { mod_path, .. } => assert_flattened(&mod_path),
        other => panic!("预期成功，得到 {:?}", other),
    }
}

#[test]
fn smart_extract_via_internal_7z_flatten_single_top_dir() {
    // 直接走自维护 7z 路径（不依赖系统 7z CLI），验证 finalize_extraction 展平逻辑
    let tmp = TempDir::new().unwrap();
    let managed = tmp.path().join("Mods").join("_MANAGED_").join("group_1");
    fs::create_dir_all(&managed).unwrap();

    let src = make_wrapped_archive_src(tmp.path());
    let archive = tmp.path().join("SrcMod.7z");
    export_internal(&src, &archive, "7z").unwrap();

    // 直接使用自维护 7z 解压到 .tmp 临时区，再 finalize（与 extract_archive 第三级一致）
    let tmp_parent = extract_tmp_dir(&managed);
    let temp_dir = extract_7z_internal(&archive, &tmp_parent, None).unwrap();
    let result = finalize_extraction(&archive, &managed, &temp_dir).unwrap();
    match result {
        ExtractResult::Success { mod_path, .. } => assert_flattened(&mod_path),
        other => panic!("预期成功，得到 {:?}", other),
    }
}

#[test]
fn smart_extract_keeps_multiple_top_entries_unflattened() {
    // 顶层含多个目录 → 保持原结构，不展平
    let tmp = TempDir::new().unwrap();
    let managed = tmp.path().join("Mods").join("_MANAGED_").join("group_1");
    fs::create_dir_all(&managed).unwrap();

    let src = tmp.path().join("SrcMod");
    fs::create_dir_all(src.join("FolderA")).unwrap();
    fs::create_dir_all(src.join("FolderB")).unwrap();
    fs::write(src.join("FolderA").join("a.txt"), b"a").unwrap();
    fs::write(src.join("FolderB").join("b.txt"), b"b").unwrap();
    let archive = tmp.path().join("SrcMod.7z");
    export_internal(&src, &archive, "7z").unwrap();

    let tmp_parent = extract_tmp_dir(&managed);
    let temp_dir = extract_7z_internal(&archive, &tmp_parent, None).unwrap();
    let result = finalize_extraction(&archive, &managed, &temp_dir).unwrap();
    match result {
        ExtractResult::Success { mod_path, .. } => {
            assert!(mod_path.join("FolderA").join("a.txt").exists());
            assert!(mod_path.join("FolderB").join("b.txt").exists());
            // 未展平：保留 FolderA / FolderB 两层
            assert!(mod_path.join("FolderA").exists());
            assert!(mod_path.join("FolderB").exists());
        }
        other => panic!("预期成功，得到 {:?}", other),
    }
}
