//! 架构健身函数（F1/F2/F4/F5）：以集成测试形态冻结分层纪律。
//!
//! - F1 依赖方向：core/platform/models/config/utils 不得引用上层模块（全文内联路径扫描）
//! - F2 命令归属：`#[tauri::command]` 仅允许出现在 commands/ 与壳模块（存量豁免表随 Phase 6 清零）
//! - F4 行数棘轮：新文件 ≤800 行；存量巨石以登记值为上限（只减不增，拆分后下调）
//! - F5 事件契约：emit 调用禁止裸字符串；Rust/前端事件登记集合严格相等
//!
//! 设计文档：docs/superpowers/specs/2026-09-18-arch-fitness-and-monolith-split-design.md

use std::fs;
use std::path::{Path, PathBuf};

fn src_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// 未在当前 Task 使用（F5 事件集合校验的 Task 5 使用），避免 clippy -D warnings 触发 dead_code
#[allow(dead_code)]
fn frontend_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../src")
}

fn collect_files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_files(&p, exts, out);
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
        {
            out.push(p);
        }
    }
}

/// 文件所属顶层模块（src/core/x.rs → Some("core")；src/lib.rs → None）
fn top_module(path: &Path) -> Option<String> {
    let rel = path.strip_prefix(src_root()).ok()?;
    rel.parent()?
        .to_str()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

fn rel_to_src(path: &Path) -> String {
    path.strip_prefix(src_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("读取失败 {:?}: {}", path, e))
}

/// F1：底层模块禁止引用上层模块（扫描全文内联路径，非仅 use 行）。
#[test]
fn f1_layering_direction_frozen() {
    // (模块, 禁止引用的上层模块路径前缀)
    const FORBIDDEN: &[(&str, &[&str])] = &[
        (
            "core",
            &[
                "crate::commands",
                "crate::window",
                "crate::tray",
                "crate::hotkey",
                "crate::updater",
                "crate::debug",
                "crate::platform",
            ],
        ),
        (
            "platform",
            &[
                "crate::core",
                "crate::commands",
                "crate::window",
                "crate::tray",
                "crate::hotkey",
                "crate::updater",
                "crate::debug",
            ],
        ),
        (
            "models",
            &[
                "crate::core",
                "crate::commands",
                "crate::window",
                "crate::tray",
                "crate::hotkey",
                "crate::updater",
                "crate::debug",
                "crate::platform",
                "crate::config",
                "crate::utils",
            ],
        ),
        (
            "config",
            &[
                "crate::core",
                "crate::commands",
                "crate::window",
                "crate::tray",
                "crate::hotkey",
                "crate::updater",
                "crate::debug",
                "crate::platform",
                "crate::utils",
            ],
        ),
        (
            "utils",
            &[
                "crate::core",
                "crate::commands",
                "crate::window",
                "crate::tray",
                "crate::hotkey",
                "crate::updater",
                "crate::debug",
                "crate::platform",
                "crate::config",
                "crate::models",
            ],
        ),
    ];
    // 存量豁免：(文件, 该文件内允许出现的上层前缀)；Phase 6 命令迁移后收缩
    const F1_EXEMPTIONS: &[(&str, &str)] = &[
        // 控制台基础设施：日志双输出直连（长期放行）
        ("core/file_logger.rs", "crate::platform::console"),
        // check_mod_cache_valid 命令体引用 parse_game（Phase 6 迁 commands/ 后删除）
        ("core/mod_cache.rs", "crate::hotkey"),
        // archive import 命令体的 BusyOpGuard（Phase 6 迁移后删除）
        ("core/archive_handler.rs", "crate::window"),
        // 日志时区钩子（长期放行，待 file_logger 归位后再评估）
        ("config/settings_store.rs", "crate::core::file_logger"),
        // 设置持久化的 atomically 写盘（utils 原子写，基线存量；重估 config→utils 归属后收紧）
        ("config/settings_store.rs", "crate::utils"),
    ];

    let mut files = Vec::new();
    collect_files(&src_root(), &["rs"], &mut files);
    let mut violations = Vec::new();

    for f in &files {
        let Some(module) = top_module(f) else { continue };
        let Some((_, banned)) = FORBIDDEN.iter().find(|(m, _)| *m == module) else {
            continue;
        };
        let content = read(f);
        let rel = rel_to_src(f);
        for b in *banned {
            let mut search = content.as_str();
            while let Some(idx) = search.find(b) {
                let occurrence = &search[idx..];
                let exempted = F1_EXEMPTIONS.iter().any(|(file, allowed)| {
                    rel == *file && occurrence.starts_with(allowed) && occurrence.len() > allowed.len()
                });
                if !exempted {
                    violations.push(format!("{} 引用 {}", rel, b));
                }
                search = &search[idx + b.len()..];
            }
        }
    }
    assert!(
        violations.is_empty(),
        "F1 分层违规（豁免表见 tests/arch_gate.rs F1_EXEMPTIONS）:\n{}",
        violations.join("\n")
    );
}