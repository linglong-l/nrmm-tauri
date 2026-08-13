use anyhow::Result;
use std::path::{Path, PathBuf};
use std::collections::HashSet;
use std::fs;
use regex::RegexBuilder;
use crate::core::ini_handler::{IniFile, IniLine};
use crate::core::constants;

fn extract_namespace_from_kv_string(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if let Some(eq_pos) = trimmed.find('=') {
        let key = trimmed[..eq_pos].trim();
        let value = trimmed[eq_pos + 1..].trim();
        if key.eq_ignore_ascii_case("namespace") && !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

pub fn extract_namespace(ini: &IniFile) -> Option<String> {
    for line in &ini.preamble {
        match line {
            IniLine::KeyValue { key, value, .. } => {
                if key.trim().eq_ignore_ascii_case("namespace") {
                    return Some(value.trim().to_string());
                }
            }
            IniLine::DisabledKeyValue { key, value, .. } => {
                if key.trim().eq_ignore_ascii_case("namespace") {
                    return Some(value.trim().to_string());
                }
            }
            IniLine::PreambleLine(text) => {
                if let Some(ns) = extract_namespace_from_kv_string(text) {
                    return Some(ns);
                }
            }
            _ => {}
        }
    }
    if let Some(first_section) = ini.sections.first() {
        for line in &first_section.lines {
            if let IniLine::KeyValue { key, value, .. } = line {
                if key.trim().eq_ignore_ascii_case("namespace") {
                    return Some(value.trim().to_string());
                }
            }
            if let IniLine::DisabledKeyValue { key, value, .. } = line {
                if key.trim().eq_ignore_ascii_case("namespace") {
                    return Some(value.trim().to_string());
                }
            }
        }
    }
    None
}

pub fn has_namespace(ini: &IniFile) -> bool {
    extract_namespace(ini).is_some()
}

pub fn expand_variable(var_name: &str, namespace: &str) -> String {
    let trimmed = var_name.trim();
    if trimmed.starts_with("$\\") {
        trimmed.to_string()
    } else if let Some(stripped) = trimmed.strip_prefix('$') {
        format!("$\\{}\\{}", namespace, stripped)
    } else {
        trimmed.to_string()
    }
}

pub fn expand_variables_in_value(value: &str, namespace: &str) -> String {
    let mut result = String::with_capacity(value.len() + namespace.len() + 10);
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '$' && i + 1 < chars.len() {
            let next = chars[i + 1];
            if next == '\\' {
                result.push(c);
                result.push(next);
                i += 2;
                while i < chars.len() && chars[i] != '\\' {
                    result.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() {
                    result.push(chars[i]);
                    i += 1;
                    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                        result.push(chars[i]);
                        i += 1;
                    }
                }
                continue;
            } else if next.is_alphanumeric() || next == '_' {
                result.push_str("$\\");
                result.push_str(namespace);
                result.push('\\');
                i += 1;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    result.push(chars[i]);
                    i += 1;
                }
                continue;
            }
        }
        result.push(c);
        i += 1;
    }
    result
}

pub fn expand_ini_variables(ini: &mut IniFile, namespace: &str) {
    for line in &mut ini.preamble {
        expand_line_variables(line, namespace);
    }
    for section in &mut ini.sections {
        for line in &mut section.lines {
            expand_line_variables(line, namespace);
        }
    }
}

fn expand_line_variables(line: &mut IniLine, namespace: &str) {
    match line {
        IniLine::KeyValue { key, value, disabled: _, comment: _, indent: _ } => {
            if key.starts_with('$') && !key.starts_with("$\\") {
                *key = expand_variable(key, namespace);
            }
            *value = expand_variables_in_value(value, namespace);
        }
        IniLine::DisabledKeyValue { key, value, comment: _ } => {
            if key.starts_with('$') && !key.starts_with("$\\") {
                *key = expand_variable(key, namespace);
            }
            *value = expand_variables_in_value(value, namespace);
        }
        IniLine::IfStart { condition, indent: _ } => {
            *condition = expand_variables_in_value(condition, namespace);
        }
        IniLine::Elif { condition, indent: _ } => {
            *condition = expand_variables_in_value(condition, namespace);
        }
        IniLine::Command(text) => {
            *text = expand_variables_in_value(text, namespace);
        }
        IniLine::PreambleLine(text) => {
            *text = expand_variables_in_value(text, namespace);
        }
        _ => {}
    }
}

pub fn collect_existing_namespaces(mod_dir: &Path) -> Result<HashSet<String>> {
    let mut namespaces = HashSet::new();
    if !mod_dir.exists() {
        return Ok(namespaces);
    }
    collect_namespaces_recursive(mod_dir, mod_dir, &mut namespaces)?;
    Ok(namespaces)
}

fn collect_namespaces_recursive(_base: &Path, dir: &Path, namespaces: &mut HashSet<String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with('.') || name == "_MANAGED_" {
                continue;
            }
            collect_namespaces_recursive(_base, &path, namespaces)?;
        } else if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("ini") {
                match crate::core::mod_ini_cache::get_or_parse_ini(&path) {
                    Ok(ini) => {
                        if let Some(ns) = extract_namespace(&ini) {
                            namespaces.insert(ns);
                        }
                    }
                    Err(e) => {
                        // 解析失败不再静默吞掉：记录日志，跳过该文件但保留目录其余收集结果
                        log::warn!(
                            "[namespace_handler] 解析 INI 失败，跳过该文件命名空间收集: {:?}: {}",
                            path,
                            e
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn unique_namespace(ns: &str, existing: &HashSet<String>) -> String {
    if !existing.contains(ns) {
        return ns.to_string();
    }
    let mut counter = 1;
    loop {
        let candidate = format!("{}_{}", ns, counter);
        if !existing.contains(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

/// 在单个 INI 文件文本中，将旧 namespace 的所有引用改写为新 namespace。
///
/// 逐行忠实移植原版 Dart `_generateModifiedLinesNamespace`：
/// - 注释行（以 `;` 开头）原样保留，不改写（避免误改注释中的同名文本）。
/// - `namespace = <value>` 声明行：仅当 value 与 `old_ns` 精确相等（大小写不敏感）时，
///   替换该 value 为 `new_ns`（不使用正则，避免把声明行里的 ns 也加定界符）。
/// - 其余非注释行：将所有 `$old_ns$`（大小写不敏感）替换为 `$new_ns$`。
///   3Dmigoto/NRMM 中 namespace 引用以 `$ns$` 定界（如 `$MyMod$SomeVar`）；
///   后续 `expand_ini_variables` 会基于改名后的 `namespace=` 声明把裸 `$var` 展开为
///   `$\new_ns\var`，从而保证跨引用一致。
///
/// 返回 `(是否发生改动, 新文本)`。
pub fn rewrite_namespace_references(content: &str, old_ns: &str, new_ns: &str) -> (bool, String) {
    // 构造匹配 `$old_ns$` 的正则：转义 old_ns 内的正则元字符，大小写不敏感（对齐原版 caseSensitive:false）。
    let pattern = format!("${}$", old_ns);
    let re = match RegexBuilder::new(&regex::escape(&pattern))
        .case_insensitive(true)
        .build()
    {
        Ok(r) => r,
        Err(_) => return (false, content.to_string()),
    };
    let replacement = format!("${}$", new_ns);

    let mut changed = false;
    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
        let trimmed = line.trim();
        // 1) 注释行原样保留
        if trimmed.starts_with(';') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // 2) namespace= 声明行：精确匹配 value（不使用正则）
        let lower_no_space = trimmed.replace(' ', "").to_lowercase();
        if lower_no_space.starts_with("namespace=") {
            if let Some(eq) = line.find('=') {
                let value_start = eq + 1;
                let original_value = &line[value_start..];
                let trimmed_value = original_value.trim();
                if trimmed_value.eq_ignore_ascii_case(old_ns) {
                    let lead_ws = original_value.len() - original_value.trim_start().len();
                    let value_end = value_start + lead_ws + trimmed_value.len();
                    out.push_str(&format!(
                        "{}{}{}",
                        &line[..value_start + lead_ws],
                        new_ns,
                        &line[value_end..]
                    ));
                    out.push('\n');
                    changed = true;
                    continue;
                }
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // 3) 其余非注释行：正则替换 `$old_ns$`
        let new_line: String = re
            .replace_all(line, |_: &regex::Captures| replacement.clone())
            .into_owned();
        if new_line != line {
            changed = true;
        }
        out.push_str(&new_line);
        out.push('\n');
    }
    // 保留原始结尾换行（若原文本以 \n 结尾，上面的循环已补上；否则不强行追加）
    if content.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    (changed, out)
}

/// 对一个模组的所有 INI 文件执行 namespace 重命名（文本级），采用与原版 `replaceNamespace`
/// 一致的三阶段原子提交：
/// 1) 备份发生改动的源文件为 `<name>.baknamespace`；
/// 2) 将改写后的内容写入 `<name>.tmp` 再 rename 覆盖原文件；
/// 3) 全部成功则删除备份；任一步失败则从备份回滚并清理临时文件，返回 Err。
///
/// 返回 `Ok(true)` 表示至少改写了一个文件，`Ok(false)` 表示无需改写。
pub fn replace_namespace_in_mod(
    mod_ini_paths: &[PathBuf],
    old_ns: &str,
    new_ns: &str,
) -> anyhow::Result<bool> {
    // 阶段 0：读取并计算改写结果
    let mut changes: Vec<(PathBuf, String)> = Vec::new();
    for p in mod_ini_paths {
        let content = IniFile::force_read_as_utf8(p)?;
        let (changed, new_content) = rewrite_namespace_references(&content, old_ns, new_ns);
        if changed {
            changes.push((p.clone(), new_content));
        }
    }
    if changes.is_empty() {
        return Ok(false);
    }

    // 阶段 1：备份发生改动的源文件
    let mut backups: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (p, _) in &changes {
        let bak = p.with_file_name(format!(
            "{}.{}",
            p.file_name().unwrap_or_default().to_string_lossy(),
            constants::NAMESPACE_BACKUP_SUFFIX
        ));
        fs::copy(p, &bak)
            .map_err(|e| anyhow::anyhow!("namespace 备份失败 {:?}: {}", p, e))?;
        backups.push((p.clone(), bak));
    }

    // 阶段 2+3：写 tmp 并 rename；失败则回滚
    let commit = (|| -> anyhow::Result<()> {
        for (p, new_content) in &changes {
            let tmp = p.with_file_name(format!(
                "{}.tmp",
                p.file_name().unwrap_or_default().to_string_lossy()
            ));
            fs::write(&tmp, new_content)
                .map_err(|e| anyhow::anyhow!("namespace 写入临时文件失败 {:?}: {}", tmp, e))?;
            fs::rename(&tmp, p)
                .map_err(|e| anyhow::anyhow!("namespace 重命名失败 {:?}: {}", p, e))?;
        }
        Ok(())
    })();

    match commit {
        Ok(()) => {
            for (_, bak) in &backups {
                let _ = fs::remove_file(bak);
            }
            Ok(true)
        }
        Err(e) => {
            // 回滚：用备份覆盖原文件，并清理临时文件与备份
            for (p, bak) in &backups {
                let _ = fs::copy(bak, p);
            }
            for (p, _) in &changes {
                let _ = fs::remove_file(p.with_file_name(format!(
                    "{}.tmp",
                    p.file_name().unwrap_or_default().to_string_lossy()
                )));
            }
            for (_, bak) in &backups {
                let _ = fs::remove_file(bak);
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use std::fs;

    fn write_temp_ini(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::with_suffix(".ini").unwrap();
        write!(f, "{}", content).unwrap();
        f.flush().unwrap();
        f
    }

    fn make_test_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn test_extract_namespace() {
        let ini_content = r#"; Header comment
namespace = MyMod

[Constants]
global $var1 = 0
"#;
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        assert_eq!(extract_namespace(&ini), Some("MyMod".to_string()));
    }

    #[test]
    fn test_no_namespace() {
        let ini_content = r#"[Constants]
$var1 = 0
"#;
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        assert_eq!(extract_namespace(&ini), None);
    }

    #[test]
    fn test_expand_variable_short() {
        assert_eq!(expand_variable("$var", "ns"), "$\\ns\\var");
        assert_eq!(expand_variable("$myVar", "MyMod"), "$\\MyMod\\myVar");
    }

    #[test]
    fn test_expand_variable_already_qualified() {
        assert_eq!(expand_variable("$\\other\\var", "ns"), "$\\other\\var");
        assert_eq!(expand_variable("$\\ns\\var", "other"), "$\\ns\\var");
    }

    #[test]
    fn test_expand_variable_non_var() {
        assert_eq!(expand_variable("key", "ns"), "key");
        assert_eq!(expand_variable("hash", "ns"), "hash");
    }

    #[test]
    fn test_expand_variables_in_value() {
        let result = expand_variables_in_value("$a + $b", "ns");
        assert_eq!(result, "$\\ns\\a + $\\ns\\b");
    }

    #[test]
    fn test_expand_variables_mixed() {
        let result = expand_variables_in_value("$a > $\\other\\b", "ns");
        assert_eq!(result, "$\\ns\\a > $\\other\\b");
    }

    #[test]
    fn test_expand_ini_variables() {
        let ini_content = r#"namespace = Test
[Constants]
$myvar = $other + 1
[Section]
if $myvar == 1
  draw = $val
endif
"#;
        let f = write_temp_ini(ini_content);
        let mut ini = IniFile::parse(f.path()).unwrap();
        let ns = extract_namespace(&ini).unwrap();
        expand_ini_variables(&mut ini, &ns);

        let constants = &ini.sections[0];
        if let IniLine::KeyValue { key, value, .. } = &constants.lines[0] {
            assert_eq!(key, "$\\Test\\myvar");
            assert!(value.contains("$\\Test\\other"));
        } else {
            panic!("Expected KeyValue line");
        }
    }

    #[test]
    fn test_unique_namespace() {
        let mut existing = HashSet::new();
        existing.insert("MyMod".to_string());
        existing.insert("MyMod_1".to_string());
        assert_eq!(unique_namespace("MyMod", &existing), "MyMod_2");
    }

    #[test]
    fn test_unique_namespace_no_conflict() {
        let existing = HashSet::new();
        assert_eq!(unique_namespace("MyMod", &existing), "MyMod");
    }

    #[test]
    fn test_expand_variable_with_underscore() {
        assert_eq!(expand_variable("$my_var", "ns"), "$\\ns\\my_var");
        assert_eq!(expand_variable("$var123", "ns"), "$\\ns\\var123");
    }

    #[test]
    fn test_collect_existing_namespaces() {
        let dir = make_test_dir();
        let ini1 = dir.path().join("mod1.ini");
        fs::write(&ini1, "namespace = Mod1\n[Section]\n").unwrap();
        let ini2 = dir.path().join("subdir");
        fs::create_dir(&ini2).unwrap();
        let ini2 = ini2.join("mod2.ini");
        fs::write(&ini2, "namespace = Mod2\n[Section]\n").unwrap();

        let namespaces = collect_existing_namespaces(dir.path()).unwrap();
        assert!(namespaces.contains("Mod1"));
        assert!(namespaces.contains("Mod2"));
    }

    #[test]
    fn test_rewrite_namespace_declaration_and_refs() {
        // 声明行精确替换 + `$ns$` 引用正则替换（大小写不敏感）+ 注释行不改写
        let content = "namespace = MyMod\n\
[Constants]\n\
; $MyMod$commented should NOT change\n\
$myvar = $MyMod$SomeVar + 1\n\
[TextureOverrideX]\n\
if $mymod$Other == 1\n\
  draw = $MyMod$Ref\n\
endif\n";
        let (changed, out) = rewrite_namespace_references(content, "MyMod", "MyMod_1");
        assert!(changed);
        assert!(out.contains("namespace = MyMod_1"), "声明行应被替换:\n{}", out);
        // 引用被替换
        assert!(out.contains("$MyMod_1$SomeVar"), "引用应被替换:\n{}", out);
        assert!(out.contains("$MyMod_1$Other"), "引用应大小写不敏感替换:\n{}", out);
        assert!(out.contains("$MyMod_1$Ref"), "引用应被替换:\n{}", out);
        // 注释行保持原样（不被替换）
        assert!(
            out.contains("; $MyMod$commented should NOT change"),
            "注释行不应被改写:\n{}",
            out
        );
    }

    #[test]
    fn test_rewrite_namespace_no_change() {
        let content = "namespace = OtherMod\n[Constants]\n$x = 1\n";
        let (changed, out) = rewrite_namespace_references(content, "MyMod", "MyMod_1");
        assert!(!changed);
        assert_eq!(out, content);
    }

    #[test]
    fn test_rewrite_namespace_declaration_exact_only() {
        // 声明行 value 仅为前缀匹配（如 MyModX）不应被误改
        let content = "namespace = MyModX\n[Constants]\n$y = $MyMod$Ref\n";
        let (changed, out) = rewrite_namespace_references(content, "MyMod", "MyMod_1");
        assert!(changed);
        assert!(out.contains("namespace = MyModX"), "声明不应被前缀误改:\n{}", out);
        assert!(out.contains("$MyMod_1$Ref"), "独立引用仍应替换:\n{}", out);
    }

    #[test]
    fn test_replace_namespace_in_mod_atomic_success() {
        let dir = make_test_dir();
        let ini = dir.path().join("mod.ini");
        let original = "namespace = Shared\n[Constants]\n$v = $Shared$Val\n";
        fs::write(&ini, original).unwrap();

        let changed =
            replace_namespace_in_mod(std::slice::from_ref(&ini), "Shared", "Shared_1").unwrap();
        assert!(changed);
        let after = fs::read_to_string(&ini).unwrap();
        assert!(after.contains("namespace = Shared_1"));
        assert!(after.contains("$Shared_1$Val"));
        // 成功后不应残留 .baknamespace / .tmp
        assert!(!dir.path().join("mod.ini.baknamespace").exists());
        assert!(!dir.path().join("mod.ini.tmp").exists());

        // 幂等：再次重命名（Shared 已不存在）应无改动
        let changed2 =
            replace_namespace_in_mod(std::slice::from_ref(&ini), "Shared", "Shared_1").unwrap();
        assert!(!changed2);
    }

    #[test]
    fn test_replace_namespace_in_mod_no_match() {
        let dir = make_test_dir();
        let ini = dir.path().join("mod.ini");
        fs::write(&ini, "namespace = Other\n[Constants]\n$x = 1\n").unwrap();
        let changed =
            replace_namespace_in_mod(std::slice::from_ref(&ini), "Shared", "Shared_1").unwrap();
        assert!(!changed);
    }
}
