//! 验证脚本：在 Sw999.ini 上运行当前 NRMM(Rust) 真实的更新模组核心转换。
//!
//! 该测试直接调用 update_mod_data 内部实际使用的 IniFile 转换流水线：
//!   parse -> inject_slot_conditions -> comment_crash_lines -> detect_errors
//!        -> remove_empty_if_blocks -> apply_indentation -> prepend_header_comment -> write_atomic
//! 以便对 Sw999.ini 用例产生"当前实现"的真实输出，用于与原版(Dart)逐项对比。

use std::collections::HashSet;
use std::path::Path;

use xxmi_nrmm_lib::core::ini_handler::{IniFile, IniLine};

#[test]
fn verify_sw999_new_injection() {
    let src = Path::new(r"D:\PC\TestProjects\XXMI-NRMM\tmp\Sw999.ini");
    let out_dir = Path::new(r"D:\PC\TestProjects\XXMI-NRMM\tmp");
    let out_path = out_dir.join("sw999_new_out.ini");

    let mut ini = IniFile::parse(src).expect("解析 Sw999.ini 失败");

    // 组 1，槽位 2（模拟该模组在 group_1 内的第 2 个启用槽）
    ini.inject_slot_conditions(1, 2);
    let crash_lines = ini.comment_crash_lines();
    let errors = ini.detect_errors(src, &HashSet::new());
    ini.remove_empty_if_blocks();
    ini.apply_indentation();
    ini.prepend_header_comment();
    ini.write_atomic(&out_path).expect("写入转换结果失败");

    // ---- 生成结构化报告 ----
    let mut report = String::new();
    report.push_str("# 当前实现(Rust) 对 Sw999.ini 的转换报告\n\n");
    report.push_str(&format!(
        "- 源文件: {}\n",
        src.display()
    ));
    report.push_str(&format!("- 输出文件: {}\n", out_path.display()));
    report.push_str(&format!("- 段总数(含 preamble): {}\n", ini.sections.len()));
    report.push_str(&format!("- 被注释的裸露 draw/ib 行数: {}\n", crash_lines.len()));
    report.push_str(&format!("- detect_errors 错误数: {}\n", errors.len()));
    if !errors.is_empty() {
        report.push_str("\n## 检测到的错误(error_type 分布)\n");
        let mut counts: std::collections::HashMap<u8, usize> = std::collections::HashMap::new();
        for e in &errors {
            *counts.entry(e.error_type).or_insert(0) += 1;
        }
        for (k, v) in counts.iter() {
            report.push_str(&format!("- error_type={}: {} 条\n", k, v));
        }
    }

    report.push_str("\n## 各段是否被注入 `if $managed_slot_id == $...\\group_1\\active_slot` 守卫\n\n");
    report.push_str("| 段名 | 被包裹 | 含 global $managed_slot_id | 含 match_priority |\n");
    report.push_str("|------|--------|-------------------------------|--------------------|\n");

    let mut wrapped_count = 0usize;
    let mut has_global = false;

    for sec in &ini.sections {
        let name = &sec.name;
        let guard = sec.lines.iter().any(|l| {
            matches!(l, xxmi_nrmm_lib::core::ini_handler::IniLine::IfStart { condition, .. } if condition.contains("active_slot"))
        });
        let gvar = sec.lines.iter().any(|l| {
            matches!(l, xxmi_nrmm_lib::core::ini_handler::IniLine::KeyValue { key, .. } if key.to_lowercase().contains("$managed_slot_id"))
        });
        let mp = sec.lines.iter().any(|l| {
            matches!(l, xxmi_nrmm_lib::core::ini_handler::IniLine::KeyValue { key, .. } if key.eq_ignore_ascii_case("match_priority"))
        });
        if guard {
            wrapped_count += 1;
        }
        if gvar {
            has_global = true;
        }
        let _ = name;
        report.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            name,
            if guard { "是" } else { "否" },
            if gvar { "是" } else { "否" },
            if mp { "是" } else { "否" },
        ));
    }

    report.push_str(&format!(
        "\n## 汇总\n- 被包裹段数: {}\n- 含 global $managed_slot_id 的段: {}\n",
        wrapped_count, has_global
    ));

    let report_path = out_dir.join("sw999_new_report.md");
    std::fs::write(&report_path, report).expect("写入报告失败");

    // ============ 回归断言 ============
    // P0-1：干净模组在修复后不应再产生缺失库误报（原 58 条 error_type=1）
    assert_eq!(
        errors.len(),
        0,
        "[P0-1 回归] Sw999 误报数应为 0，实际 {} 条: {:?}",
        errors.len(),
        errors
    );

    // P1-1：Resource 段不应被 if 守卫包裹（原版不包裹 resource/inputlayout/draw/dispatch）
    let wrapped_resource = ini
        .sections
        .iter()
        .filter(|s| {
            s.name.to_lowercase().starts_with("resource")
                && s.lines.iter().any(|l| {
                    matches!(l, IniLine::IfStart { condition, .. } if condition.contains("active_slot"))
                })
        })
        .count();
    assert_eq!(
        wrapped_resource, 0,
        "[P1-1 回归] Resource 段不应被包裹，实际包裹 {} 个",
        wrapped_resource
    );

    println!(
        "[sw999_verify] 已生成 {} 与 {}（误报={}, 包裹Resource={}）",
        out_path.display(),
        report_path.display(),
        errors.len(),
        wrapped_resource
    );
}
