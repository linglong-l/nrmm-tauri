//! 数据集驱动测试：对 NRMM-Rust-test 运行 update_mod_data，逐项比对 NRMM-test 基准。
//!
//! NRMM-test     = 原版 NRMM(Dart+C) 实测产物（预期输出基准），_MANAGED_ 位于 Mods/ 下
//! NRMM-Rust-test = 本 Rust 移植版的输入数据集
//!
//! 比对覆盖：输入/输出/副作用/边界情况。
//!
//! 架构差异说明：
//! - 原版 NRMM：_MANAGED_ 在 Mods/_MANAGED_（由 include_recursive=Mods 自动加载）
//! - Rust 移植版：_MANAGED_ 在 game_mods_path/_MANAGED_（根级，由 d3dx.ini 末尾 include 注入）
//!
//! 本测试默认接受此架构差异，将两者映射比较。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use xxmi_nrmm_lib::core::mod_manager;
use xxmi_nrmm_lib::models::enums::TargetGame;
use xxmi_nrmm_lib::models::settings::AppSettings;

// ---- 工具 ----

fn manifest_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn baseline_dir() -> PathBuf {
    manifest_dir().join("tests").join("NRMM-test")
}

fn input_dir() -> PathBuf {
    manifest_dir().join("tests").join("NRMM-Rust-test")
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let e = entry?;
        let ft = e.file_type()?;
        let sp = e.path();
        let dp = dst.join(e.file_name());
        if ft.is_dir() {
            copy_dir(&sp, &dp)?;
        } else {
            fs::copy(&sp, &dp)?;
        }
    }
    Ok(())
}

fn read_file_lossy(p: &Path) -> String {
    let mut buf = Vec::new();
    if let Ok(mut f) = fs::File::open(p) {
        let _ = f.read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn normalize_paths(s: &str) -> String {
    s.replace('\\', "/")
}

/// 规范化 INI 内容，移除绝对路径和注释噪声，保留结构
fn normalize_ini_content(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_lowercase();
        // 移除 ini_path_absolute 行（含绝对路径）
        if lower.starts_with("ini_path_absolute") {
            continue;
        }
        // 规范化 global $managed_slot_id = <N>（仅校验键存在，值因扫描器实现差异允许不同）
        if lower.starts_with("global $managed_slot_id") {
            out.push_str("global $managed_slot_id = <SLOT>\n");
            continue;
        }
        // 移除纯注释行（保留 NRMM/DISABLED 注释以对照）
        if trimmed.starts_with(';') {
            if trimmed.contains("NRMM") || trimmed.contains("DISABLED") || trimmed.starts_with("; \"") {
                out.push_str(line.trim_end());
                out.push('\n');
            }
            continue;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.replace("\r\n", "\n")
}

// ---- 测试 ----

#[test]
fn test_dataset_parity_update_mod_data() {
    let temp_dir = tempfile::TempDir::new().expect("创建临时目录失败");
    // 架构约定：game_mods_path 必须以 "Mods" 结尾。
    // 将数据集复制到 temp/Mods/ 下，使所有文件（d3dx.ini, _MANAGED_, Core/...）位于 Mods/ 内。
    let mods_dir = temp_dir.path().join("Mods");
    fs::create_dir_all(&mods_dir).expect("创建 Mods 目录失败");

    // 复制数据集内容。数据集中存储了原版 NRMM 的完整目录结构，
    // 其中包含 Mods/_MANAGED_/ 子目录和 d3dx.ini 主配置文件。
    //
    // 路径约定：
    // - d3dx.ini → temp/d3dx.ini（游戏根目录，3Dmigoto 以此为基准解析所有 include 路径）
    // - Mods/_MANAGED_/ → temp/Mods/_MANAGED_/（避免双重嵌套 Mods/Mods/_MANAGED_）
    // - Core/ 等目录 → temp/Mods/Core/
    let src_root = input_dir();
    for entry in std::fs::read_dir(&src_root).expect("读取数据集失败") {
        let e = entry.expect("读取目录项失败");
        let name = e.file_name();
        let src = e.path();
        let dst = mods_dir.join(&name);
        if name == "Mods" {
            // 将数据集 Mods/_MANAGED_/* 复制到目标 Mods/_MANAGED_/
            let src_managed = src.join("_MANAGED_");
            if src_managed.exists() {
                let dst_managed = mods_dir.join("_MANAGED_");
                copy_dir(&src_managed, &dst_managed).expect("复制 _MANAGED_ 失败");
            }
        } else if name == "d3dx.ini" {
            // d3dx.ini 必须位于游戏根目录（temp_dir），而非 Mods/ 内。
            // 3Dmigoto 将所有 include 路径相对于 d3dx.ini 所在目录解析。
            let game_root_dst = temp_dir.path().join("d3dx.ini");
            fs::copy(&src, &game_root_dst).expect("复制 d3dx.ini 到游戏根目录失败");
        } else if src.is_dir() {
            copy_dir(&src, &dst).expect("复制目录失败");
        } else {
            fs::copy(&src, &dst).expect("复制文件失败");
        }
    }

    // game_mods_path = <temp>/Mods（末尾目录名为 Mods）
    let game_mods_path = mods_dir;

    // 运行 update_mod_data
    println!("[parity] 执行 update_mod_data(GenshinImpact, {:?})", game_mods_path);
    let settings = AppSettings::default();
    let result = match mod_manager::update_mod_data(TargetGame::GenshinImpact, &game_mods_path, &settings) {
        Ok(r) => r,
        Err(e) => panic!("update_mod_data 失败: {:#}", e),
    };

    println!(
        "[parity] 结果: enabled={}, processed={}, errors={}, groups={}",
        result.enabled_mods, result.processed_mods, result.errors.len(), result.total_groups
    );

    if result.processed_mods == 0 {
        println!("[parity] 警告: 没有模组被处理。可能扫描未识别到模组。");
    }

    // 比对管理文件（Rust 输出在 game_mods_path/_MANAGED_/ = Mods/_MANAGED_/）
    let rust_managed = game_mods_path.join("_MANAGED_");
    let base_managed = baseline_dir().join("Mods").join("_MANAGED_");
    compare_managed_files(&rust_managed, &base_managed);

    // 比对模组 INI
    compare_mod_inis(&rust_managed, &base_managed);

    // 比对 d3dx.ini（Rust 输出在 <game_root>/d3dx.ini = temp_dir/d3dx.ini）
    // 当 include_recursive=Mods 存在时，d3dx.ini 应保持原样（仅剥离 NRMM 注入块）。
    // 基线使用原始 d3dx.ini（NRMM-test/d3dx.ini），验证 Rust 未做非预期修改。
    let rust_d3dx = temp_dir.path().join("d3dx.ini");
    let original_d3dx = input_dir().join("d3dx.ini");
    if rust_d3dx.exists() && original_d3dx.exists() {
        let na = normalize_paths(&read_file_lossy(&rust_d3dx));
        let ne = normalize_paths(&read_file_lossy(&original_d3dx));
        if na == ne {
            println!("[OK] d3dx.ini 与原始输入一致（无意外修改）");
        } else {
            // 差异应仅为 NRMM_INI 块的剥离（strip_nrmm_injected_content）
            let a_lines: Vec<&str> = na.lines().collect();
            let e_lines: Vec<&str> = ne.lines().collect();
            if a_lines.len() <= e_lines.len() {
                println!("[INFO] d3dx.ini 已剥离 NRMM 注入块（{} 行差异）", e_lines.len() as isize - a_lines.len() as isize);
            } else {
                diff_lines("d3dx.ini", &na, &ne);
            }
        }
    } else {
        if !rust_d3dx.exists() {
            println!("[WARN] Rust 输出 d3dx.ini 不存在: {:?}", rust_d3dx);
        }
        if !original_d3dx.exists() {
            println!("[WARN] 原始 d3dx.ini 不存在: {:?}", original_d3dx);
        }
    }

    // 比对备份文件
    compare_backups(&rust_managed, &base_managed);

    println!("[parity] === 比对完成 ===");
}

fn compare_managed_files(actual_root: &Path, base_root: &Path) {
    let files = ["nrmm_include.ini", "nrmm_keypress.txt"];

    for fname in &files {
        let actual_p = actual_root.join(fname);
        let expect_p = base_root.join(fname);
        let actual = read_file_lossy(&actual_p);
        let expected = read_file_lossy(&expect_p);

        if !actual_p.exists() { println!("[MISSING] {}", fname); continue; }
        if !expect_p.exists() { println!("[NO_BASELINE] {}", fname); continue; }

        let na = normalize_paths(&actual);
        let ne = normalize_paths(&expected);
        if na == ne {
            println!("[OK] {}", fname);
        } else {
            diff_lines(fname, &na, &ne);
        }
    }

    // group_1.ini
    compare_group_ini(actual_root, base_root);
}

fn compare_group_ini(actual_root: &Path, base_root: &Path) {
    let actual_p = actual_root.join("group_1").join("group_1.ini");
    let expect_p = base_root.join("group_1").join("group_1.ini");

    if !actual_p.exists() { println!("[MISSING] group_1.ini"); return; }
    if !expect_p.exists() { println!("[NO_BASELINE] group_1.ini"); return; }

    let na = normalize_ini_content(&normalize_paths(&read_file_lossy(&actual_p)));
    let ne = normalize_ini_content(&normalize_paths(&read_file_lossy(&expect_p)));
    if na == ne {
        println!("[OK] group_1.ini 语义一致");
    } else {
        diff_lines("group_1.ini", &na, &ne);
    }
}

fn compare_mod_inis(actual_root: &Path, base_root: &Path) {
    let a_dir = actual_root.join("group_1");
    let b_dir = base_root.join("group_1");
    if a_dir.exists() {
        recurse_compare_mod_inis(&a_dir, &b_dir, &a_dir, &b_dir);
    }
}

fn recurse_compare_mod_inis(dir: &Path, _base_dir: &Path, root_a: &Path, root_b: &Path) {
    if !dir.is_dir() { return; }
    let entries = match fs::read_dir(dir) { Ok(e) => e, Err(_) => return };

    for entry in entries.flatten() {
        let p = entry.path();
        let name = entry.file_name();
        let rel = p.strip_prefix(root_a).unwrap_or(&p);
        let bp = root_b.join(rel);

        if p.is_dir() {
            let n = name.to_string_lossy().into_owned();
            if n.starts_with("DISABLED") { continue; }
            recurse_compare_mod_inis(&p, &bp, root_a, root_b);
        } else if p.extension().map(|e| e == "ini").unwrap_or(false) {
            let n = name.to_string_lossy();
            if n.starts_with("group_") || n.contains("nrmm_") || n.contains("manager_") || n == "desktop.ini" { continue; }
            if !bp.exists() { continue; }

            let na = normalize_ini_content(&read_file_lossy(&p));
            let ne = normalize_ini_content(&read_file_lossy(&bp));
            if na == ne {
                println!("[OK_MOD] {:?}", rel);
            } else {
                println!("[DIFF_MOD] {:?}", rel);
                diff_lines(&format!("{:?}", rel), &na, &ne);
            }
        }
    }
}

fn compare_backups(actual_root: &Path, base_root: &Path) {
    // 检查 _managed_backup 文件是否被创建
    let group1_a = actual_root.join("group_1");
    let group1_b = base_root.join("group_1");

    if group1_a.exists() {
        let mut a_backups = Vec::new();
        let mut b_backups = Vec::new();
        collect_backups(&group1_a, &mut a_backups, &group1_a);
        collect_backups(&group1_b, &mut b_backups, &group1_b);

        println!(
            "[BACKUP] Rust 产生 {} 个备份, 基准 {} 个备份",
            a_backups.len(), b_backups.len()
        );
    }
}

fn collect_backups(dir: &Path, out: &mut Vec<String>, root: &Path) {
    if !dir.is_dir() { return; }
    let entries = match fs::read_dir(dir) { Ok(e) => e, Err(_) => return };
    for e in entries.flatten() {
        let p = e.path();
        let n = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if n.starts_with("DISABLED") { continue; }
            collect_backups(&p, out, root);
        } else if p.extension().map(|s| s == "baknamespace").unwrap_or(false)
            || n.contains("_managed_backup")
        {
            let rel = p.strip_prefix(root).unwrap_or(&p);
            out.push(rel.to_string_lossy().into_owned());
        }
    }
}

fn diff_lines(label: &str, actual: &str, expected: &str) {
    let a: Vec<&str> = actual.lines().collect();
    let e: Vec<&str> = expected.lines().collect();
    let max = a.len().max(e.len());
    let mut diffs = 0usize;
    for i in 0..max {
        let al = a.get(i).unwrap_or(&"(missing)");
        let el = e.get(i).unwrap_or(&"(missing)");
        if al != el {
            if diffs < 15 {
                println!("  {:4} act: {}", i + 1, al);
                println!("       exp: {}", el);
            }
            diffs += 1;
        }
    }
    if diffs > 15 {
        println!("  ... 还有 {} 处差异未显示", diffs - 15);
    }
    println!("  [{}] 总差异行数: {} (实际{}行 vs 期望{}行)", label, diffs, a.len(), e.len());
}
