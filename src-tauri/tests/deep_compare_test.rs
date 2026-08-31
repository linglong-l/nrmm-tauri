//! Deep comparison test after running update_mod_data on NRMM-Rust-test data.
//!
//! Flow:
//! 1. Copy NRMM-Rust-test data to temp dir (with d3dx.ini at game_root)
//! 2. Run update_mod_data
//! 3. Compare output with NRMM-test baseline file-by-file (bytes + INI line diff)
//! 4. Output structured diagnostic report

// 同 lib.rs：关闭主观的 pedantic / nursery 两组，保留 `-D warnings` 把关真实问题。
#![allow(clippy::pedantic, clippy::nursery)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use xxmi_nrmm_lib::core::mod_manager;
use xxmi_nrmm_lib::models::enums::TargetGame;
use xxmi_nrmm_lib::models::settings::AppSettings;

// Path helpers
fn input_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("NRMM-Rust-test")
}
fn baseline_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("NRMM-test")
}

// File content helpers
fn read_bytes(p: &Path) -> Option<Vec<u8>> {
    std::fs::read(p).ok()
}
fn read_utf8(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn normalize_path(s: &str) -> String {
    s.replace('\\', "/")
}

fn normalize_ini_content(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let lower = t.to_lowercase();
        if lower.starts_with("ini_path_absolute") {
            continue;
        }
        if lower.starts_with("global $managed_slot_id") && t.contains('=') {
            out.push_str("global $managed_slot_id = <SLOT>\n");
            continue;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.replace("\r\n", "\n")
}

// Recursive file collection
fn collect_files(root: &Path) -> BTreeMap<String, PathBuf> {
    let mut map = BTreeMap::new();
    fn walk(dir: &Path, base: &Path, map: &mut BTreeMap<String, PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                let rel = p
                    .strip_prefix(base)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                if p.is_dir() {
                    walk(&p, base, map);
                } else {
                    map.insert(rel, p);
                }
            }
        }
    }
    walk(root, root, &mut map);
    map
}

// Copy dataset from NRMM-Rust-test to temp dir
//
// 镜像 NRMM-test 基线布局：游戏根目录（temp/）下放置 d3dx.ini 与所有原有
// 游戏文件/目录（Core / d3d11.dll / d3dcompiler_47.dll / d3dx_user.ini /
// ShaderCache / ShaderFixes 等），Mods 目录（temp/Mods）仅放置模组（含 _MANAGED_）。
// 这样 update_mod_data 运行时 game_root = temp/，d3d11.dll 等位于游戏根，
// 与 NRMM 真实运行环境及 NRMM-test 基线一致；避免将游戏根文件误置于 Mods/ 下。
fn restore_env(src: &Path, temp: &Path) -> PathBuf {
    let mods_dir = temp.join("Mods");
    std::fs::create_dir_all(&mods_dir).unwrap();

    for entry in std::fs::read_dir(src).unwrap() {
        let e = entry.unwrap();
        let name = e.file_name();
        let sp = e.path();

        if name == "Mods" {
            // 游戏 Mods 目录：仅复制其下的 _MANAGED_（其余模组由 update_mod_data 处理）
            let mgr = sp.join("_MANAGED_");
            if mgr.exists() {
                copy_dir(&mgr, &mods_dir.join("_MANAGED_"));
            }
        } else if name == "d3dx.ini" {
            // 主 INI 位于游戏根目录（Mods 的父目录）
            std::fs::copy(&sp, temp.join("d3dx.ini")).unwrap();
        } else {
            // 其余均为游戏根目录下的原有文件/目录（Core / d3d11.dll /
            // d3dcompiler_47.dll / d3dx_user.ini / ShaderCache / ShaderFixes 等），
            // 必须置于游戏根（temp/）而非 Mods/ 下，以镜像 NRMM-test 基线布局。
            if sp.is_dir() {
                copy_dir(&sp, &temp.join(&name));
            } else {
                std::fs::copy(&sp, temp.join(&name)).unwrap();
            }
        }
    }
    mods_dir
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap().flatten() {
        let sp = e.path();
        let dp = dst.join(e.file_name());
        if sp.is_dir() {
            copy_dir(&sp, &dp);
        } else {
            std::fs::copy(&sp, &dp).unwrap();
        }
    }
}

fn is_ini_file(name: &str) -> bool {
    let l = name.to_lowercase();
    (l.ends_with(".ini") || l.ends_with(".ini_managed_backup")) && !l.ends_with("desktop.ini")
}

fn is_managed_file(name: &str) -> bool {
    matches!(
        name,
        "nrmm_include.ini" | "nrmm_keypress.txt" | "manager_group.ini" | "selectedindex"
    )
}

#[test]
fn deep_compare_after_update_mod_data() {
    println!("\n============================================================");
    println!("  Deep Compare: NRMM-Rust-test -> update_mod_data -> NRMM-test");
    println!("============================================================");

    // Step 1: Restore environment from NRMM-Rust-test
    let temp = tempfile::TempDir::new().unwrap();
    let src = input_dir();
    println!("\n[1] Restoring from: {:?}", src);
    let mods_dir = restore_env(&src, temp.path());

    // Step 2: Run update_mod_data
    println!("\n[2] Running update_mod_data...");
    let result = mod_manager::update_mod_data(
        TargetGame::GenshinImpact,
        &mods_dir,
        &AppSettings::default(),
    )
    .expect("update_mod_data failed");
    println!(
        "    enabled={} disabled={} processed={} errors={} groups={}",
        result.enabled_mods,
        result.disabled_mods,
        result.processed_mods,
        result.errors.len(),
        result.total_groups
    );

    // 错误明细（按 error_type 分组统计 + 逐条打印缺失库类报错），
    // 用于在测试中直接观察是否生成与 NRMM 一致的「缺少 RabbitFx 等依赖库」警告。
    {
        let mut by_type: BTreeMap<u8, usize> = BTreeMap::new();
        for e in &result.errors {
            *by_type.entry(e.error_type).or_default() += 1;
        }
        println!("    error_type 分布: {:?}", by_type);
        let lib_missing = result
            .errors
            .iter()
            .filter(|e| e.error_type == 5)
            .collect::<Vec<_>>();
        println!(
            "    NON EXISTENT LIB 明细 (error_type=5, 共 {} 条):",
            lib_missing.len()
        );
        for e in lib_missing {
            println!(
                "      [{}:{}] {} | {:?}",
                e.line_number, e.line, e.error_message, e.friendly_message
            );
        }
    }

    // Step 3: Collect files from both sides
    let actual = collect_files(temp.path());
    let baseline = collect_files(&baseline_dir());
    println!(
        "\n[3] File counts: actual={} baseline={}",
        actual.len(),
        baseline.len()
    );

    // Step 4: Compare
    let mut all_keys: Vec<&str> = actual
        .keys()
        .chain(baseline.keys())
        .map(|s| s.as_str())
        .collect();
    all_keys.sort();
    all_keys.dedup();

    let mut missing = Vec::new(); // in baseline, not in actual
    let mut extra = Vec::new(); // in actual, not in baseline
                                // 白名单 extra（Rust 有意保留、基线不存在的文件）：
                                // - d3dx.ini_managed_backup: update_mod_data 为主 INI 原子备份（NRMM 不保留；备份是安全特性，故忽略）
    let mut extra_whitelisted: Vec<(String, PathBuf)> = Vec::new();
    const WHITELISTED_EXTRA: &[&str] = &["d3dx.ini_managed_backup"];
    let whitelisted_extra = |rel: &str| -> bool {
        WHITELISTED_EXTRA
            .iter()
            .any(|w| rel.eq_ignore_ascii_case(w))
    };
    let mut bytes_ok = 0usize;
    let mut bytes_diff = Vec::new(); // bytes differ

    for rel in &all_keys {
        let a = actual.get(*rel);
        let b = baseline.get(*rel);
        match (a, b) {
            (None, Some(bp)) => missing.push((rel.to_string(), bp.clone())),
            (Some(ap), None) => {
                if whitelisted_extra(rel) {
                    extra_whitelisted.push((rel.to_string(), ap.clone()));
                } else {
                    extra.push((rel.to_string(), ap.clone()));
                }
            }
            (Some(ap), Some(bp)) => {
                if read_bytes(ap) == read_bytes(bp) {
                    bytes_ok += 1;
                } else {
                    bytes_diff.push((rel.to_string(), ap.clone(), bp.clone()));
                }
            }
            _ => {}
        }
    }

    // Step 5: INI line-level diff for byte-different files
    let mut ini_diffs = Vec::new();
    // 语义级逐行 diff 明细（最多保留前 10 个文件）：normalize 后的行向量（输出侧, 基线侧）
    let mut detail_diffs: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    for (rel, ap, bp) in &bytes_diff {
        let fname = std::path::Path::new(rel)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !is_ini_file(&fname) {
            continue;
        }
        let na = normalize_ini_content(&normalize_path(&read_utf8(ap)));
        let nb = normalize_ini_content(&normalize_path(&read_utf8(bp)));
        if na != nb {
            ini_diffs.push((rel.clone(), na.lines().count(), nb.lines().count()));
            if detail_diffs.len() < 10 {
                detail_diffs.push((
                    rel.clone(),
                    na.lines().map(str::to_string).collect(),
                    nb.lines().map(str::to_string).collect(),
                ));
            }
        }
    }

    // Step 6: Classify and report
    let missing_real: Vec<_> = missing
        .iter()
        .filter(|(p, _)| {
            let name = std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            !is_managed_file(&name)
        })
        .collect();

    let missing_mgr: Vec<_> = missing
        .iter()
        .filter(|(p, _)| {
            let name = std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            is_managed_file(&name)
        })
        .collect();

    let ini_filtered: Vec<_> = ini_diffs
        .iter()
        .filter(|(r, _, _)| !r.to_lowercase().contains("desktop.ini"))
        .collect();

    println!("\n=================================================");
    println!("  Report");
    println!("=================================================");

    println!("\n--- Missing files (baseline has, actual doesn't) ---");
    if missing_real.is_empty() {
        println!("  OK: none");
    } else {
        for (p, _) in &missing_real {
            println!("  MISSING: {}", p);
        }
    }

    println!("\n--- NRMM managed files (expected missing) ---");
    if missing_mgr.is_empty() {
        println!("  OK: none");
    } else {
        for (p, _) in &missing_mgr {
            println!("  EXPECTED: {}", p);
        }
    }

    println!("\n--- Extra files (actual has, baseline doesn't) ---");
    if extra.is_empty() {
        println!("  OK: none");
    } else {
        for (p, _) in &extra {
            println!("  EXTRA: {}", p);
        }
    }

    println!("\n--- Whitelisted extras (expected) ---");
    if extra_whitelisted.is_empty() {
        println!("  OK: none");
    } else {
        for (p, _) in &extra_whitelisted {
            println!("  WHITELISTED: {}", p);
        }
    }

    println!("\n--- Bytes comparison ---");
    println!("  MATCH: {}", bytes_ok);
    println!("  DIFF:  {}", bytes_diff.len());
    for (rel, _ap, _bp) in &bytes_diff {
        let tag = if rel.to_lowercase().contains("desktop.ini") {
            " [whitelist-ignore]"
        } else {
            ""
        };
        println!("    DIFF: {}{}", rel, tag);
    }

    println!("\n--- INI line-level diff ---");
    if ini_filtered.is_empty() {
        println!("  OK: all INI contents match");
    } else {
        for (rel, a_len, b_len) in &ini_filtered {
            println!(
                "  DIFF: {} (actual {} lines vs baseline {} lines)",
                rel, a_len, b_len
            );
        }
    }

    println!(
        "\n--- INI 逐行 diff（normalize 后，{} 个文件，每文件前 30 处；`< 输出`=Rust，`< 基线`=NRMM）---",
        detail_diffs.len()
    );
    for (idx, (rel, na, nb)) in detail_diffs.iter().enumerate() {
        println!("    --- INI_DIFF #{}: {} ---", idx + 1, rel);
        let mut shown = 0usize;
        let max_len = na.len().max(nb.len());
        for i in 0..max_len {
            if shown >= 30 {
                println!("      ...（其余差异省略）");
                break;
            }
            let a = na.get(i).map(String::as_str).unwrap_or("<EOF>");
            let b = nb.get(i).map(String::as_str).unwrap_or("<EOF>");
            if a != b {
                println!("      L{:<4} < 输出 | {}", i + 1, a);
                println!("      L{:<4} < 基线 | {}", i + 1, b);
                shown += 1;
            }
        }
    }

    println!("\n--- `;-;` 注释行集合对比（_MANAGED_ 下 .ini） ---");
    let semicolon_lines = |root: &Path| -> BTreeMap<String, Vec<String>> {
        let mut map = BTreeMap::new();
        for (rel, p) in collect_files(root) {
            if rel.to_lowercase().ends_with(".ini") {
                let hits: Vec<String> = read_utf8(&p)
                    .lines()
                    .filter(|l| l.trim_start().starts_with(";-;"))
                    .map(|l| l.trim_start().to_string())
                    .collect();
                if !hits.is_empty() {
                    map.insert(rel, hits);
                }
            }
        }
        map
    };
    let semi_actual = semicolon_lines(&temp.path().join("Mods").join("_MANAGED_"));
    let semi_base = semicolon_lines(&baseline_dir().join("Mods").join("_MANAGED_"));
    for rel in semi_base.keys().chain(semi_actual.keys()) {
        let a = semi_actual.get(rel);
        let b = semi_base.get(rel);
        match (a, b) {
            (None, Some(v)) => println!("    [;-;][基线有-输出无] {} ({} 行)", rel, v.len()),
            (Some(v), None) => println!("    [;-;][输出有-基线无] {} ({} 行)", rel, v.len()),
            (Some(va), Some(vb)) if va != vb => {
                for miss in vb.iter().filter(|x| !va.contains(x)) {
                    println!("    [;-;][基线有-输出无] {} | {}", rel, miss);
                }
                for extra in va.iter().filter(|x| !vb.contains(x)) {
                    println!("    [;-;][输出有-基线无] {} | {}", rel, extra);
                }
            }
            _ => {}
        }
    }

    // 保留输出树（排查用）：DEEP_COMPARE_KEEP_TEMP=1 时复制到 tests/.tmp_deep_compare_out。
    // 置于最终判定之前，保证 FAIL（panic）路径也能保留现场。
    if std::env::var("DEEP_COMPARE_KEEP_TEMP").as_deref() == Ok("1") {
        let keep_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join(".tmp_deep_compare_out");
        if keep_dir.exists() {
            std::fs::remove_dir_all(&keep_dir).unwrap();
        }
        copy_dir(temp.path(), &keep_dir);
        println!("    [keep] Rust 输出已保留至: {:?}", keep_dir);
    }

    // Step 7: Final verdict
    //
    // #6 对齐说明：restore_env 现已镜像 NRMM 游戏根布局（Core / d3d11.dll /
    // d3dcompiler_47.dll / d3dx_user.ini / ShaderCache / ShaderFixes / d3dx.ini
    // 均位于游戏根 temp/，而非 Mods/ 下）。本测试断言「布局正确」。
    //
    // 分歧忽略已按用户指令禁用（2026-08-12）：不再把 Mods/_MANAGED_ 内部差异当作
    // 信息性输出静默放过。Rust 用「重命名模组目录」表示禁用（DISABLED_<dir>），
    // Dart/NRMM 用「为 INI 文件加 _DISABLED_ 前缀」并保留 _X.ini / _DISABLED_X.ini
    // 双副本 —— 这是一项真实 parity 分歧，现在会被如实暴露为失败，强制可定位、可修复，
    // 而非隐藏。布局违规（游戏根资源误置于 Mods/）仍独立判定失败。
    println!("\n=================================================");
    let layout_violation: Vec<&&str> = all_keys
        .iter()
        .filter(|rel| {
            let r = rel.to_lowercase();
            r.starts_with("mods/core")
                || r.starts_with("mods/d3d11.dll")
                || r.starts_with("mods/d3dcompiler_47.dll")
                || r.starts_with("mods/d3dx_user.ini")
                || r.starts_with("mods/shadercache")
                || r.starts_with("mods/shaderfixes")
                || r.starts_with("mods/d3dx.ini")
        })
        .collect();

    if !layout_violation.is_empty() {
        println!("  FAIL: 游戏根资源被错误置于 Mods/ 下（#6 布局回归）:");
        for rel in &layout_violation {
            println!("    LAYOUT_VIOLATION: {}", rel);
        }
        println!("=================================================\n");
        panic!("deep_compare layout regression - game-root assets under Mods/");
    }

    // 分歧忽略已禁用：对 _MANAGED_ 内部（及全部）差异执行严格 parity，
    // 任一非空即判定失败，把刻意的设计分歧暴露为真实、可定位的失败。
    let any_divergence = !missing_real.is_empty() || !extra.is_empty() || !ini_filtered.is_empty();
    if !any_divergence {
        println!("  PASS: 输出与 NRMM-test 基线完全匹配（含布局与 _MANAGED_ 内部）");
        println!("=================================================\n");
    } else {
        println!("  FAIL: 与 NRMM-test 基线存在真实分歧（分歧忽略已禁用，不再静默放过）：");
        for (p, _) in &missing_real {
            println!("    MISSING:  {}", p);
        }
        for (p, _) in &extra {
            println!("    EXTRA:    {}", p);
        }
        for (r, _, _) in &ini_filtered {
            println!("    INI_DIFF: {}", r);
        }
        println!("=================================================\n");
        panic!("deep_compare parity divergence detected (divergence-ignore disabled) - see report above");
    }

    let _ = temp;
}
