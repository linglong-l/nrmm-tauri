use super::*;
use std::io::Write;
use tempfile::NamedTempFile;

fn write_temp_ini(content: &str) -> NamedTempFile {
    let mut f = NamedTempFile::with_suffix(".ini").unwrap();
    write!(f, "{}", content).unwrap();
    f.flush().unwrap();
    f
}

#[test]
fn comment_errored_conditional_lines_comments_malformed_else_if() {
    // 基线证据：`else if $X = = N`（` = = ` 畸形等式分隔，keyValue 形态）应被 `;-;` 注释
    let ini_content = "[TextureOverrideA]\n\
         hash = x\n\
         if $Hair == 0\n\
         \x20   drawindexed = 1, 2, 0\n\
         \x20   else if $Hair = = 1\n\
         \x20   drawindexed = 3, 4, 0\n\
         endif\n";
    let f = write_temp_ini(ini_content.trim_start());
    let mut ini = IniFile::parse(f.path()).unwrap();
    let commented = ini.comment_errored_conditional_lines();
    assert_eq!(commented.len(), 1, "畸形 else if 条件行应被注释");
    let rendered: Vec<String> = ini.sections[0]
        .lines
        .iter()
        .map(|l| l.to_string())
        .collect();
    assert!(
        rendered
            .iter()
            .any(|l| l.trim_start().starts_with(";-;else if $Hair = = 1")),
        "注释行应保留原行并以 ;-; 前缀：{:?}",
        rendered
    );
    assert!(
        rendered
            .iter()
            .any(|l| l.trim_start() == "drawindexed = 1, 2, 0"),
        "合法行不应被注释"
    );
}

#[test]
fn comment_errored_conditional_lines_keeps_valid_else_if() {
    // 合法 `else if $X == N`（`==` 正常）不应被注释
    let ini_content = "[TextureOverrideA]\n\
         if $Hair == 0\n\
         \x20   else if $Hair == 1\n\
         endif\n";
    let f = write_temp_ini(ini_content.trim_start());
    let mut ini = IniFile::parse(f.path()).unwrap();
    let commented = ini.comment_errored_conditional_lines();
    assert!(commented.is_empty(), "合法 else if 不应被注释");
}

#[test]
fn comment_errored_conditional_lines_legacy_slot_guard_in_present() {
    // 基线证据：槽位可注入段 [Present] 的 legacy `$\1\...\managed_slot_id` 守卫被 `;-;` 注释
    let ini_content = "[Present]\n\
         if $\\1\\master_1\\managed_slot_id == $\\modmanageragl\\group_3\\active_slot\n\
         \x20   post $active = 0\n\
         endif\n";
    let f = write_temp_ini(ini_content.trim_start());
    let mut ini = IniFile::parse(f.path()).unwrap();
    let commented = ini.comment_errored_conditional_lines();
    assert_eq!(commented.len(), 1, "legacy 槽位守卫应被注释");
}

#[test]
fn test_remove_empty_if_blocks() {
    let ini_content = "[Test]\nif $x == 1\n\nendif\ndrawindexed\n";
    let f = write_temp_ini(ini_content);
    let mut ini = IniFile::parse(f.path()).unwrap();
    ini.remove_empty_if_blocks();
    let lines = &ini.sections[0].lines;
    assert!(!lines.iter().any(|l| matches!(l, IniLine::IfStart { .. })));
    assert!(!lines.iter().any(|l| matches!(l, IniLine::EndIf { .. })));
}

#[test]
fn test_comment_crash_lines() {
    let ini_content =
        "[Constants]\nx = 1\ndrawindexed\n\n[TextureOverrideSafe]\nhash = 0x1\ndrawindexed\n";
    let f = write_temp_ini(ini_content);
    let mut ini = IniFile::parse(f.path()).unwrap();
    let commented = ini.comment_crash_lines();
    assert!(!commented.is_empty());
    let has_commented_draw = ini.sections[0].lines.iter().any(|l| {
        matches!(l, IniLine::Comment(c) if c.contains("DISABLED_BY_NRMM") && c.contains("drawindexed"))
    });
    assert!(has_commented_draw);
}

#[test]
fn test_remove_old_managed_content_cleans_comment_headers() {
    // 旧版 NRMM 头注释（`;` 前缀 → 解析为 Comment）与 PreambleLine 均须被清理，
    // 否则会与 prepend_header_comment 新头并排累积（对齐 Dart _parseIniSections）。
    let ini_content = "; No Reload Mod Manager v1.0.1\n\
; \";-;\" are errored conditional lines.\n\
namespace = TestMod\n\
[Constants]\n\
global $managed_slot_id = 0\n\
$other = 1\n\
[TextureOverrideX]\n\
if $managed_slot_id == $\\modmanageragl\\group_1\\active_slot\n\
  drawindexed\n\
endif\n";
    let f = write_temp_ini(ini_content);
    let mut ini = IniFile::parse(f.path()).unwrap();
    ini.remove_old_managed_content();

    // 旧 NRMM 头注释（Comment 型）应被移除
    assert!(!ini
        .preamble
        .iter()
        .any(|l| matches!(l, IniLine::Comment(c) if c.contains("No Reload Mod Manager"))));
    assert!(!ini
        .preamble
        .iter()
        .any(|l| matches!(l, IniLine::Comment(c) if c.contains("errored conditional"))));
    // Constants 段的 $managed_slot_id 应被移除
    let constants = ini
        .sections
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case("Constants"))
        .expect("Constants 段应存在");
    assert!(!constants
        .lines
        .iter()
        .any(|l| matches!(l, IniLine::KeyValue { key, .. } if key.contains("$managed_slot_id"))));
    // 注：manager if 守卫的清理属已知限制（与幂等性语义耦合），此处不断言其移除；
    // 仅断言头注释与 Constants 清理生效（见 remove_old_managed_content 内注释说明）。
}
