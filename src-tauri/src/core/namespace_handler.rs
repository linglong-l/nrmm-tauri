use anyhow::Result;
use std::path::Path;
use std::collections::HashSet;
use std::fs;
use crate::core::ini_handler::{IniFile, IniLine};

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
    } else if trimmed.starts_with('$') {
        format!("$\\{}\\{}", namespace, &trimmed[1..])
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
        if c == '$' {
            if i + 1 < chars.len() {
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
        IniLine::KeyValue { key, value, disabled: _, comment: _ } => {
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

fn collect_namespaces_recursive(base: &Path, dir: &Path, namespaces: &mut HashSet<String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_string_lossy();
            if name.starts_with('.') || name == "_MANAGED_" {
                continue;
            }
            collect_namespaces_recursive(base, &path, namespaces)?;
        } else if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("ini") {
                if let Ok(ini) = IniFile::parse(&path) {
                    if let Some(ns) = extract_namespace(&ini) {
                        namespaces.insert(ns);
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
}
