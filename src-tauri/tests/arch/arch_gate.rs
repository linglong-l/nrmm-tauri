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
        // 日志时区钩子（长期放行，待 file_logger 归位后再评估）
        ("config/settings_store.rs", "crate::core::file_logger"),
        // 设置持久化的 atomically 写盘（utils 原子写，基线存量；重估 config→utils 归属后收紧）
        ("config/settings_store.rs", "crate::utils"),
    ];

    let mut files = Vec::new();
    collect_files(&src_root(), &["rs"], &mut files);
    let mut violations = Vec::new();

    for f in &files {
        let Some(module) = top_module(f) else {
            continue;
        };
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
                    rel == *file
                        && occurrence.starts_with(allowed)
                        && occurrence.len() > allowed.len()
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
        "F1 分层违规（豁免表见 tests/arch/arch_gate.rs F1_EXEMPTIONS）:\n{}",
        violations.join("\n")
    );
}

/// F2：`#[tauri::command]` 归属白名单（commands/ 适配层 + 壳模块 + 存量豁免）。
///
/// 行级精确匹配（trim 后等于属性串），文档注释中的提及不会误报。
#[test]
fn f2_tauri_command_ownership() {
    // 存量豁免：core 直挂命令（Phase 6 迁入 commands/ 后已清零，2026-09-19）
    const LEGACY: &[&str] = &[];

    let mut files = Vec::new();
    collect_files(&src_root(), &["rs"], &mut files);
    let mut offenders = Vec::new();
    let mut total = 0usize;
    for f in &files {
        let content = read(f);
        let count = content
            .lines()
            .filter(|l| l.trim() == "#[tauri::command]")
            .count();
        if count == 0 {
            continue;
        }
        total += count;
        let rel = rel_to_src(f);
        let allowed = rel.starts_with("commands/")
            || matches!(
                top_module(f).as_deref(),
                Some("window") | Some("hotkey") | Some("updater") | Some("platform")
            )
            || rel == "lib.rs"
            || LEGACY.contains(&rel.as_str());
        if !allowed {
            offenders.push(format!("{}（{} 处）", rel, count));
        }
    }
    assert!(
        offenders.is_empty(),
        "F2 命令归属违规（新增命令一律落 commands/）:\n{}",
        offenders.join("\n")
    );
    // 基线锚点：当前 82 个命令（新增/删除命令时同步更新此值，防止静默漂移）
    assert_eq!(
        total, 82,
        "命令总数基线漂移（当前 {}），确认后更新锚点",
        total
    );
}

/// F4：Rust 文件行数棘轮——新文件 ≤800 行；存量巨石以登记值为上限（只减不增）。
///
/// 每完成一次拆分，将对应条目下调至拆分后行数；文件删除时移除条目。
#[test]
fn f4_rust_file_line_ratchet() {
    const MAX_NEW_FILE_LINES: usize = 800;
    // 存量豁免（上限=2026-09-18 实测行数；随拆分逐域下调）
    const LEGACY_CAPS: &[(&str, usize)] = &[
        ("commands/mod_commands.rs", 1329),
        ("core/mod_cache.rs", 2681),
        ("core/archive_handler.rs", 1839),
        ("core/file_watcher.rs", 1790),
        ("platform/windows.rs", 868),
    ];

    let mut files = Vec::new();
    collect_files(&src_root(), &["rs"], &mut files);
    let mut violations = Vec::new();
    for f in &files {
        let rel = rel_to_src(f);
        let lines = read(f).lines().count();
        let cap = LEGACY_CAPS
            .iter()
            .find(|(p, _)| *p == rel)
            .map(|(_, c)| *c)
            .unwrap_or(MAX_NEW_FILE_LINES);
        if lines > cap {
            violations.push(format!("{}：{} 行 > 上限 {}", rel, lines, cap));
        }
    }
    assert!(
        violations.is_empty(),
        "F4 行数棘轮违规:\n{}",
        violations.join("\n")
    );
}

/// F5-1：emit 调用禁止裸字符串字面量（必须引用 core::events 常量）。
#[test]
fn f5_emit_calls_use_event_constants() {
    let mut files = Vec::new();
    collect_files(&src_root(), &["rs"], &mut files);
    let mut offenders = Vec::new();
    for f in &files {
        let content = read(f);
        let mut rest = content.as_str();
        while let Some(idx) = rest.find(".emit") {
            let after = &rest[idx + 5..];
            let after = after.strip_prefix("_to").unwrap_or(after);
            let after = after
                .strip_prefix('(')
                .map(str::trim_start)
                .unwrap_or(after);
            if let Some(literal) = after.strip_prefix('"') {
                if let Some(end) = literal.find('"') {
                    offenders.push(format!(
                        "{}: emit 裸字面量 \"{}\"",
                        rel_to_src(f),
                        &literal[..end]
                    ));
                }
            }
            rest = &rest[idx + 5..];
        }
    }
    assert!(
        offenders.is_empty(),
        "F5 emit 裸字符串（改用 core::events 常量）:\n{}",
        offenders.join("\n")
    );
}

/// 从 events.rs 源码提取全部 `pub const X: &str = "...";` 的字符串值（文本解析，独立 crate 可编译）。
fn rust_event_values(src: &str) -> Vec<String> {
    src.lines()
        .filter_map(|l| {
            let t = l.trim();
            if !t.starts_with("pub const") || !t.contains(": &str = ") {
                return None;
            }
            let start = t.find('"')?;
            let end = t[start + 1..].find('"')? + start + 1;
            Some(t[start + 1..end].to_string())
        })
        .collect()
}

/// F5-2：Rust 事件常量集合与前端 src/events.ts 登记值严格相等（双向，文本解析）。
#[test]
fn f5_event_registry_matches_frontend() {
    let rust_src = read(&src_root().join("core/events.rs"));
    let rs_values = rust_event_values(&rust_src);
    assert!(
        !rs_values.is_empty(),
        "events.rs 未解析到任何 `pub const ...: &str` 事件常量"
    );

    let ts = read(&frontend_root().join("events.ts"));
    // 提取 events.ts 中全部单引号字符串（登记值；文件内不得出现其他单引号字面量）
    let mut ts_values: Vec<String> = Vec::new();
    let mut rest = ts.as_str();
    while let Some(idx) = rest.find('\'') {
        let Some(end) = rest[idx + 1..].find('\'') else {
            break;
        };
        ts_values.push(rest[idx + 1..idx + 1 + end].to_string());
        rest = &rest[idx + 1 + end + 1..];
    }

    for v in &ts_values {
        assert!(rs_values.contains(v), "前端登记但 Rust 未登记: {}", v);
    }
    for c in &rs_values {
        assert!(ts_values.contains(c), "Rust 登记但前端未登记: {}", c);
    }
}
