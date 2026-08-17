//! 核心纯逻辑边界/回归集成测试（安全红线：仅新增测试文件，不修改任何源码）。
//!
//! 覆盖：
//! - `mod_scanner::strip_disabled_prefix`：DISABLED 前缀剥离（历史 Bug A 复现：过度剥离/误删大小写混合）
//! - `utils::atomic_write`：原子写入崩溃安全（历史 Bug B 复现：失败时目标不受影响 + 临时文件清理）
//! - `error_normalizer::normalize`：错误规范化（不向用户透传原始路径/技术字样）
//! - `error_normalizer::friendly_errored_line`：友好错误行（去掉 DUPLICATE LIB 等技术前缀）
//!
//! 全部使用系统临时目录，测试结束自动清理，无真实项目文件副作用。

// 同 lib.rs：关闭主观的 pedantic / nursery 两组，保留 `-D warnings` 把关真实问题。
#![allow(clippy::pedantic, clippy::nursery)]

use std::fs;
use std::io;
use std::path::PathBuf;

use xxmi_nrmm_lib::core::error_normalizer::{friendly_errored_line, normalize};
use xxmi_nrmm_lib::core::mod_scanner::strip_disabled_prefix;
use xxmi_nrmm_lib::utils::atomic_write;

/// 在系统临时目录下创建唯一子目录，返回路径；带控制变量用于并行安全与清理。
fn temp_dir(tag: &str) -> PathBuf {
    let unique = format!("nrmm_test_{}_{}", std::process::id(), tag);
    let dir = std::env::temp_dir().join(&unique);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("临时目录创建失败");
    dir
}

#[test]
fn r1_strip_accumulated_prefixed_returns_clean() {
    // █ 历史 Bug A：DISABLED 累积前缀（如 DISABLEDDISABLEDMod）须完整剥离
    assert_eq!(strip_disabled_prefix("DISABLEDDISABLEDMod"), "Mod");
    assert_eq!(strip_disabled_prefix("DISABLEDDISABLEDDISABLEDMod"), "Mod");
}

#[test]
fn r2_strip_disabled_not_prefix_position_preserved() {
    // 前缀不在串首（"MyDisabledMod" 开头非 Disabled）时原样保留，不误删
    assert_eq!(strip_disabled_prefix("MyDisabledMod"), "MyDisabledMod");
    assert_eq!(
        strip_disabled_prefix("enableDisabledStuff"),
        "enableDisabledStuff"
    );
}

#[test]
fn r3_strip_prefix_with_separator() {
    // 前缀后跟 _ / - / 空格作为分隔符，一并移除
    assert_eq!(strip_disabled_prefix("DISABLED_Mod_v2"), "Mod_v2");
    assert_eq!(strip_disabled_prefix("DISABLED-Mod"), "Mod");
    assert_eq!(strip_disabled_prefix("DISABLED Mod"), "Mod");
}

#[test]
fn r4_strip_no_prefix_cjk_untouched() {
    // 无前缀（含中文/数字）原样返回
    assert_eq!(strip_disabled_prefix("普通Mod"), "普通Mod");
    assert_eq!(strip_disabled_prefix("Mod2"), "Mod2");
}

#[test]
fn r5_atomic_write_new_file_clean() {
    let dir = temp_dir("r5");
    let path = dir.join("settings.json");
    atomic_write(&path, b"hello").expect("写入应成功");
    assert_eq!(fs::read(&path).unwrap(), b"hello");
    // 临时文件不应残留
    assert!(!dir.join("settings.json.tmp_write").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn r6_atomic_write_overwrite_clean() {
    let dir = temp_dir("r6");
    let path = dir.join("data.json");
    fs::write(&path, "old").unwrap();
    atomic_write(&path, b"new").expect("覆盖应成功");
    assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    assert!(!dir.join("data.json.tmp_write").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn r7_atomic_write_rename_failure_keeps_target() {
    // █ 历史 Bug B：失败（rename 到已存在目录）时目标不受影响，且临时文件被清理
    let dir = temp_dir("r7");
    let target = dir.join("locked"); // 目标“文件”实际是一个目录 → rename(tmp, dir) 失败
    fs::create_dir(&target).unwrap();

    let res = atomic_write(&target, b"payload");
    assert!(res.is_err(), "写入到目录应失败");
    // 目标目录仍存在（未被破坏），临时文件已清理
    assert!(target.is_dir());
    assert!(!fs::read_dir(&dir).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp_write")
    }));
    let _ = fs::remove_dir_all(&dir);
}

fn io_error(kind: io::ErrorKind, msg: &str) -> anyhow::Error {
    anyhow::Error::new(io::Error::new(kind, msg))
}

#[test]
fn r8_normalize_not_found_hides_path() {
    // 绝不可向用户透传含路径的技术信息
    let e = io_error(io::ErrorKind::NotFound, "C:/secret/user/X.ini");
    let f = normalize(&e);
    assert_eq!(f.code, "file_not_found");
    assert!(!f.message.contains("C:"), "不应泄露路径");
    assert!(!f.message.contains("secret"));
}

#[test]
fn r9_normalize_permission_denied() {
    let e = io_error(io::ErrorKind::PermissionDenied, "access denied on Y");
    let f = normalize(&e);
    assert_eq!(f.code, "permission_denied");
    assert!(f.message.contains("权限"));
}

#[test]
fn r10_normalize_unknown_error_redacts_details() {
    // 未知错误统一为 internal_error，绝不低于技术细节（历史 Bug：错误原文透传）
    let e = anyhow::anyhow!("boom: os error 1234");
    let f = normalize(&e);
    assert_eq!(f.code, "internal_error");
    assert!(!f.message.contains("boom"));
    assert!(!f.message.contains("os error"));
}

#[test]
fn r11_friendly_errored_line_strips_tech_prefix() {
    // 去除 DUPLICATE LIB / CRASH 技术字样，保留行号与库名
    let line = friendly_errored_line(0, "DUPLICATE LIB:武装模组", 3);
    assert!(line.contains("第 3 行"));
    assert!(line.contains("武装模组"));
    assert!(!line.contains("DUPLICATE LIB"), "不应暴露技术前缀");

    let crash = friendly_errored_line(1, "CRASH LINE", 5);
    assert!(crash.contains("第 5 行"));
    assert!(!crash.contains("CRASH"));

    // 路径过长（文件级，行号为 0）
    let too_long = friendly_errored_line(6, "", 0);
    assert!(too_long.contains("260"));

    // 未知类型兜底
    let fallback = friendly_errored_line(99, "x", 0);
    assert!(!fallback.is_empty());
}
