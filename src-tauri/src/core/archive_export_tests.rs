//! 导出域测试（archive_handler.rs 测试按域分发 PA-3）

use super::*;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// 在 `root` 下建一个基础模组目录（含嵌套文件与子目录）
fn make_mod_dir(root: &Path) -> PathBuf {
    let mod_dir = root.join("MyMod");
    fs::create_dir_all(mod_dir.join("Textures")).unwrap();
    fs::write(mod_dir.join("mod.ini"), "[Section]\nhandled = 1\n").unwrap();
    fs::write(mod_dir.join("Textures").join("tex1.png"), b"PNGDATA").unwrap();
    fs::write(mod_dir.join("Textures").join("tex2.png"), b"PNGDATA2").unwrap();
    mod_dir
}

// ---------------- export_internal（第三级回退，确定性） ----------------

#[test]
fn export_internal_zip_compression_rejected() {
    let tmp = TempDir::new().unwrap();
    let mod_dir = make_mod_dir(tmp.path());
    let zip_path = tmp.path().join("MyMod.zip");

    // 自维护压缩仅支持 .7z；zip 压缩须在 7z CLI 层完成，此处应被拒绝且不产生文件
    let res = export_internal(&mod_dir, &zip_path, "zip");
    assert!(res.is_err(), "自维护 zip 压缩应被拒绝");
    assert!(!zip_path.exists(), "不应生成 zip 文件");
}

#[test]
fn export_internal_7z_creates_valid_7z() {
    let tmp = TempDir::new().unwrap();
    let mod_dir = make_mod_dir(tmp.path());
    let sevenz_path = tmp.path().join("MyMod.7z");

    export_internal(&mod_dir, &sevenz_path, "7z").unwrap();

    assert!(sevenz_path.exists());
    // 7z 文件签名：37 7A BC AF 27 1C
    let mut fd = fs::File::open(&sevenz_path).unwrap();
    let mut sig = [0u8; 6];
    fd.read_exact(&mut sig).unwrap();
    assert_eq!(
        &sig,
        &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C],
        "非法的 7z 签名"
    );
}

#[test]
fn export_internal_unsupported_format_errors() {
    let tmp = TempDir::new().unwrap();
    let mod_dir = make_mod_dir(tmp.path());
    let rar_path = tmp.path().join("MyMod.rar");

    let res = export_internal(&mod_dir, &rar_path, "rar");
    assert!(res.is_err());
}

// ---------------- export_mod（端到端契约，不依赖具体层级） ----------------

#[test]
fn export_mod_produces_non_empty_archive() {
    let tmp = TempDir::new().unwrap();
    let mod_dir = make_mod_dir(tmp.path());
    let out = tmp.path().join("Exported.zip");

    export_mod(&mod_dir, &out).unwrap();

    let meta = fs::metadata(&out).unwrap();
    assert!(meta.len() > 0, "导出归档不应为空");
}
