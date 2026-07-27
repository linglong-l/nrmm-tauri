use std::path::{Path, PathBuf};
use std::io::{Write, BufWriter};
use anyhow::{Result, Context, bail};
use std::fs;

use crate::models::mod_data::ErroredLines;

#[derive(Debug, Clone, PartialEq)]
pub enum IniLine {
    Empty,
    Comment(String),
    DisabledKeyValue { key: String, value: String, comment: Option<String> },
    KeyValue { key: String, value: String, disabled: bool, comment: Option<String> },
    IfStart { condition: String, indent: usize },
    Elif { condition: String, indent: usize },
    Else { indent: usize },
    EndIf { indent: usize },
    Command(String),
    SectionHeader(String),
    Include(String),
    PreambleLine(String),
}

#[derive(Debug, Clone)]
pub struct IniSection {
    pub name: String,
    pub lines: Vec<IniLine>,
    pub is_conditional: bool,
}

#[derive(Debug, Clone)]
pub struct IniFile {
    pub path: PathBuf,
    pub preamble: Vec<IniLine>,
    pub sections: Vec<IniSection>,
}

pub fn is_conditional_section(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with("key") || lower.starts_with("keypress")
        || lower.starts_with("textureoverride") || lower.starts_with("shaderoverride")
        || lower.starts_with("commandlist")
}

pub fn is_non_executable_section(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "present" || lower.starts_with("customshader") || lower == "string"
}

fn trim_trailing_whitespace(s: &str) -> &str {
    s.trim_end_matches(|c| c == '\r' || c == '\n' || c == ' ' || c == '\t')
}

fn parse_key_value(line: &str) -> Option<(String, String, Option<String>)> {
    let equal_pos = line.find('=')?;
    let key = line[..equal_pos].trim();
    let rest = &line[equal_pos + 1..];

    let (value, comment) = if rest.trim_start().starts_with('"') {
        let trimmed_rest = rest.trim_start();
        let offset = rest.len() - trimmed_rest.len();
        if let Some(close_quote) = trimmed_rest[1..].find('"') {
            let value_part = &rest[..offset + close_quote + 2];
            let after_quote = &rest[offset + close_quote + 2..];
            let comment = if let Some(semi_pos) = after_quote.find(';') {
                Some(after_quote[semi_pos + 1..].trim().to_string())
            } else {
                None
            };
            (value_part.trim().to_string(), comment)
        } else {
            (rest.trim().to_string(), None)
        }
    } else {
        match rest.find(';') {
            Some(semi_pos) => {
                let value_part = rest[..semi_pos].trim().to_string();
                let comment_part = rest[semi_pos + 1..].trim().to_string();
                (value_part, Some(comment_part))
            }
            None => (rest.trim().to_string(), None)
        }
    };

    if key.is_empty() {
        None
    } else {
        Some((key.to_string(), value, comment))
    }
}

impl IniLine {
    fn to_string(&self) -> String {
        match self {
            IniLine::Empty => String::new(),
            IniLine::Comment(text) => text.clone(),
            IniLine::DisabledKeyValue { key, value, comment } => {
                let mut s = format!(";-;{} = {}", key, value);
                if let Some(c) = comment {
                    s.push_str(&format!(" ; {}", c));
                }
                s
            }
            IniLine::KeyValue { key, value, disabled, comment } => {
                let mut s = String::new();
                if *disabled {
                    s.push_str(";-;");
                }
                s.push_str(&format!("{} = {}", key, value));
                if let Some(c) = comment {
                    s.push_str(&format!(" ; {}", c));
                }
                s
            }
            IniLine::IfStart { condition, indent } => {
                format!("{}if {}", " ".repeat(*indent), condition)
            }
            IniLine::Elif { condition, indent } => {
                format!("{}elif {}", " ".repeat(*indent), condition)
            }
            IniLine::Else { indent } => {
                format!("{}else", " ".repeat(*indent))
            }
            IniLine::EndIf { indent } => {
                format!("{}endif", " ".repeat(*indent))
            }
            IniLine::Command(text) => text.clone(),
            IniLine::SectionHeader(name) => format!("[{}]", name),
            IniLine::Include(path) => format!("include = {}", path),
            IniLine::PreambleLine(text) => text.clone(),
        }
    }
}

impl IniFile {
    pub fn parse(path: &Path) -> Result<Self> {
        let content = Self::force_read_as_utf8(path)?;
        let mut preamble: Vec<IniLine> = Vec::new();
        let mut sections: Vec<IniSection> = Vec::new();
        let mut current_section: Option<usize> = None;

        for raw_line in content.lines() {
            let line = trim_trailing_whitespace(raw_line);

            if line.is_empty() {
                match current_section {
                    Some(idx) => sections[idx].lines.push(IniLine::Empty),
                    None => preamble.push(IniLine::Empty),
                }
                continue;
            }

            if line.starts_with(";-;") {
                let after_prefix = line[3..].trim_start();
                match parse_key_value(after_prefix) {
                    Some((key, value, comment)) => {
                        let kv = IniLine::DisabledKeyValue { key, value, comment };
                        match current_section {
                            Some(idx) => sections[idx].lines.push(kv),
                            None => preamble.push(kv),
                        }
                    }
                    None => {
                        let c = IniLine::Comment(line.to_string());
                        match current_section {
                            Some(idx) => sections[idx].lines.push(c),
                            None => preamble.push(c),
                        }
                    }
                }
                continue;
            }

            if line.starts_with(';') {
                let c = IniLine::Comment(line.to_string());
                match current_section {
                    Some(idx) => sections[idx].lines.push(c),
                    None => preamble.push(c),
                }
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                let section_name = line[1..line.len() - 1].trim().to_string();
                let is_cond = is_conditional_section(&section_name);
                sections.push(IniSection {
                    name: section_name,
                    lines: Vec::new(),
                    is_conditional: is_cond,
                });
                current_section = Some(sections.len() - 1);
                continue;
            }

            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();

            if trimmed.starts_with("if ") {
                let condition = trimmed[3..].trim().to_string();
                let l = IniLine::IfStart { condition, indent };
                match current_section {
                    Some(idx) => sections[idx].lines.push(l),
                    None => preamble.push(IniLine::PreambleLine(l.to_string())),
                }
                continue;
            }

            if trimmed.starts_with("elif ") {
                let condition = trimmed[5..].trim().to_string();
                let l = IniLine::Elif { condition, indent };
                match current_section {
                    Some(idx) => sections[idx].lines.push(l),
                    None => preamble.push(IniLine::PreambleLine(l.to_string())),
                }
                continue;
            }

            let after_else = if trimmed.starts_with("else") {
                Some(&trimmed[4..])
            } else {
                None
            };
            if let Some(rest) = after_else {
                if rest.trim_start().is_empty() || rest.trim_start().starts_with(';') {
                    let l = IniLine::Else { indent };
                    match current_section {
                        Some(idx) => sections[idx].lines.push(l),
                        None => preamble.push(IniLine::PreambleLine(l.to_string())),
                    }
                    continue;
                }
            }

            if trimmed.starts_with("endif") {
                let l = IniLine::EndIf { indent };
                match current_section {
                    Some(idx) => sections[idx].lines.push(l),
                    None => preamble.push(IniLine::PreambleLine(l.to_string())),
                }
                continue;
            }

            if trimmed.starts_with("include") {
                if let Some(eq_pos) = trimmed.find('=') {
                    let value = trimmed[eq_pos + 1..].trim().to_string();
                    let l = IniLine::Include(value);
                    match current_section {
                        Some(idx) => sections[idx].lines.push(l),
                        None => preamble.push(l),
                    }
                    continue;
                }
            }

            if line.contains('=') {
                match parse_key_value(line) {
                    Some((key, value, comment)) => {
                        let lower_key = key.to_lowercase();
                        if lower_key == "include" {
                            let l = IniLine::Include(value);
                            match current_section {
                                Some(idx) => sections[idx].lines.push(l),
                                None => preamble.push(l),
                            }
                        } else {
                            let l = IniLine::KeyValue { key, value, disabled: false, comment };
                            match current_section {
                                Some(idx) => sections[idx].lines.push(l),
                                None => preamble.push(IniLine::PreambleLine(l.to_string())),
                            }
                        }
                    }
                    None => {
                        let l = IniLine::Command(line.to_string());
                        match current_section {
                            Some(idx) => sections[idx].lines.push(l),
                            None => preamble.push(IniLine::PreambleLine(line.to_string())),
                        }
                    }
                }
            } else {
                let l = IniLine::Command(line.to_string());
                match current_section {
                    Some(idx) => sections[idx].lines.push(l),
                    None => preamble.push(IniLine::PreambleLine(line.to_string())),
                }
            }
        }

        Ok(IniFile {
            path: path.to_path_buf(),
            preamble,
            sections,
        })
    }

    pub fn write_atomic(&self, path: &Path) -> Result<()> {
        let tmp_path = path.with_extension("ini.tmp");
        let file = fs::File::create(&tmp_path)
            .with_context(|| format!("Failed to create temp file: {:?}", tmp_path))?;
        let mut writer = BufWriter::new(file);

        for line in &self.preamble {
            writeln!(writer, "{}", line.to_string())?;
        }

        let mut first_section = true;
        for section in &self.sections {
            if !first_section {
                writeln!(writer)?;
            }
            first_section = false;

            writeln!(writer, "[{}]", section.name)?;
            for line in &section.lines {
                writeln!(writer, "{}", line.to_string())?;
            }
        }

        writer.flush()?;
        drop(writer);

        fs::rename(&tmp_path, path)
            .with_context(|| format!("Failed to rename temp file to {:?}", path))?;

        Ok(())
    }

    pub fn backup(path: &Path) -> Result<PathBuf> {
        let backup_path = path.with_extension("ini_managed_backup");
        if backup_path.exists() {
            return Ok(backup_path);
        }
        fs::copy(path, &backup_path)
            .with_context(|| format!("Failed to backup {:?} to {:?}", path, backup_path))?;
        Ok(backup_path)
    }

    pub fn restore_from_backup(path: &Path) -> Result<()> {
        let backup_path = path.with_extension("ini_managed_backup");
        if !backup_path.exists() {
            bail!("Backup file not found: {:?}", backup_path);
        }
        fs::copy(&backup_path, path)
            .with_context(|| format!("Failed to restore from backup {:?}", backup_path))?;
        fs::remove_file(&backup_path)
            .with_context(|| format!("Failed to delete backup {:?}", backup_path))?;
        Ok(())
    }

    pub fn force_read_as_utf8(path: &Path) -> Result<String> {
        let bytes = fs::read(path)
            .with_context(|| format!("Failed to read file: {:?}", path))?;

        let bytes = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            &bytes[3..]
        } else {
            &bytes[..]
        };

        match std::str::from_utf8(bytes) {
            Ok(s) => Ok(s.to_string()),
            Err(_) => Ok(String::from_utf8_lossy(bytes).into_owned()),
        }
    }

    fn first_command_line_index(lines: &[IniLine]) -> Option<usize> {
        for (i, line) in lines.iter().enumerate() {
            match line {
                IniLine::Empty | IniLine::Comment(_) => continue,
                _ => return Some(i),
            }
        }
        None
    }

    fn last_command_line_index(lines: &[IniLine]) -> Option<usize> {
        for (i, line) in lines.iter().enumerate().rev() {
            match line {
                IniLine::Empty | IniLine::Comment(_) => continue,
                _ => return Some(i),
            }
        }
        None
    }

    fn calculate_match_priority(lines: &[IniLine]) -> u32 {
        let mut priority: u32 = 0;
        let mut found_resources = std::collections::HashSet::new();

        for line in lines {
            match line {
                IniLine::KeyValue { key, .. } | IniLine::DisabledKeyValue { key, .. } => {
                    let lower_key = key.to_lowercase();
                    let key_str = lower_key.as_str();

                    if key_str == "drawindexed" || key_str == "draw" {
                        priority += 50;
                    }
                    if key_str == "ib" {
                        priority += 30;
                    }
                    if key_str.starts_with("vb") && key_str.len() >= 3 && key_str[2..3].chars().all(|c| c.is_ascii_digit()) {
                        priority += 20;
                    }

                    let resource_prefixes = [
                        "ps-t", "vs-t", "ps-", "vs-", "cs-", "o", "u",
                    ];
                    for rp in &resource_prefixes {
                        if key_str.starts_with(rp) {
                            let suffix = &key_str[rp.len()..];
                            if suffix.is_empty() || suffix.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                                found_resources.insert(key_str.to_string());
                            }
                        }
                    }
                }
                IniLine::Command(cmd) => {
                    let lower_cmd = cmd.trim_start().to_lowercase();
                    if lower_cmd.starts_with("drawindexed") || lower_cmd.starts_with("draw") {
                        priority += 50;
                    }
                }
                _ => {}
            }
        }

        priority + found_resources.len() as u32
    }

    pub fn inject_slot_conditions(&mut self, group_id: u32, mod_index: u32) {
        let condition_var = format!(
            "$managed_slot_id == $\\modmanageragl\\group_{}\\{}",
            group_id, mod_index
        );

        for section in &mut self.sections {
            let section_name_lower = section.name.to_lowercase();

            if is_non_executable_section(&section.name) {
                continue;
            }

            if is_conditional_section(&section.name) {
                let has_existing_condition = section.lines.iter().any(|line| {
                    matches!(line, IniLine::IfStart { condition, .. } if condition.contains("managed_slot_id"))
                });

                if has_existing_condition {
                    continue;
                }

                let first_idx = Self::first_command_line_index(&section.lines);
                let last_idx = Self::last_command_line_index(&section.lines);

                if let (Some(first), Some(last)) = (first_idx, last_idx) {
                    let match_priority = Self::calculate_match_priority(&section.lines);

                    section.lines.insert(first, IniLine::KeyValue {
                        key: "match_priority".to_string(),
                        value: match_priority.to_string(),
                        disabled: false,
                        comment: None,
                    });
                    section.lines.insert(first, IniLine::KeyValue {
                        key: "allow_duplicate_hash".to_string(),
                        value: "true".to_string(),
                        disabled: false,
                        comment: None,
                    });
                    section.lines.insert(first, IniLine::IfStart {
                        condition: condition_var.clone(),
                        indent: 0,
                    });

                    let insert_end = last + 4;
                    section.lines.insert(insert_end.min(section.lines.len()), IniLine::EndIf { indent: 0 });
                }
            } else if section_name_lower == "constants" {
                let mut new_lines: Vec<IniLine> = Vec::new();
                for line in &section.lines {
                    match line {
                        IniLine::KeyValue { key, value, disabled, comment } if key.starts_with('$') => {
                            new_lines.push(IniLine::IfStart {
                                condition: condition_var.clone(),
                                indent: 0,
                            });
                            new_lines.push(IniLine::KeyValue {
                                key: key.clone(),
                                value: value.clone(),
                                disabled: *disabled,
                                comment: comment.clone(),
                            });
                            new_lines.push(IniLine::EndIf { indent: 0 });
                        }
                        other => new_lines.push(other.clone()),
                    }
                }
                section.lines = new_lines;
            }
        }
    }

    pub fn remove_empty_if_blocks(&mut self) {
        for section in &mut self.sections {
            let mut result: Vec<IniLine> = Vec::new();
            let mut stack: Vec<usize> = Vec::new();

            for line in &section.lines {
                match line {
                    IniLine::IfStart { .. } => {
                        stack.push(result.len());
                        result.push(line.clone());
                    }
                    IniLine::EndIf { .. } => {
                        if let Some(start_idx) = stack.pop() {
                            let block_content = &result[start_idx + 1..];
                            let is_empty = block_content.iter().all(|l| {
                                matches!(l, IniLine::Empty | IniLine::Comment(_))
                            });
                            if is_empty {
                                result.truncate(start_idx);
                                continue;
                            }
                        }
                        result.push(line.clone());
                    }
                    _ => result.push(line.clone()),
                }
            }
            section.lines = result;
        }
    }

    pub fn apply_indentation(&mut self) {
        let indent_size = 2;
        for section in &mut self.sections {
            let mut current_indent = 0usize;
            let mut result: Vec<IniLine> = Vec::new();

            for line in &section.lines {
                match line {
                    IniLine::IfStart { condition, .. } => {
                        result.push(IniLine::IfStart {
                            condition: condition.clone(),
                            indent: current_indent * indent_size,
                        });
                        current_indent += 1;
                    }
                    IniLine::Elif { condition, .. } => {
                        let indent = current_indent.saturating_sub(1) * indent_size;
                        result.push(IniLine::Elif {
                            condition: condition.clone(),
                            indent,
                        });
                    }
                    IniLine::Else { .. } => {
                        let indent = current_indent.saturating_sub(1) * indent_size;
                        result.push(IniLine::Else { indent });
                    }
                    IniLine::EndIf { .. } => {
                        current_indent = current_indent.saturating_sub(1);
                        result.push(IniLine::EndIf {
                            indent: current_indent * indent_size,
                        });
                    }
                    other => result.push(other.clone()),
                }
            }
            section.lines = result;
        }
    }

    pub fn comment_crash_lines(&mut self) -> Vec<u32> {
        let mut commented_lines: Vec<u32> = Vec::new();
        let mut global_line_num = 1u32;

        for _ in &self.preamble {
            global_line_num += 1;
        }

        for section in &mut self.sections {
            global_line_num += 1;
            let section_name_lower = section.name.to_lowercase();
            let in_texture_override = section_name_lower.starts_with("textureoverride");
            let mut in_conditional_block = 0i32;

            let mut new_lines: Vec<IniLine> = Vec::new();
            for line in &section.lines {
                match line {
                    IniLine::IfStart { .. } => {
                        in_conditional_block += 1;
                        new_lines.push(line.clone());
                    }
                    IniLine::EndIf { .. } => {
                        in_conditional_block = in_conditional_block.saturating_sub(1);
                        new_lines.push(line.clone());
                    }
                    IniLine::Else { .. } | IniLine::Elif { .. } => {
                        new_lines.push(line.clone());
                    }
                    IniLine::Command(text) => {
                        let lower_text = text.trim_start().to_lowercase();
                        let should_comment = if in_texture_override || in_conditional_block > 0 {
                            false
                        } else {
                            lower_text.starts_with("drawindexed")
                                || lower_text.starts_with("draw ")
                                || lower_text == "draw"
                                || (lower_text.starts_with("ib") && lower_text.contains('='))
                        };

                        if should_comment {
                            commented_lines.push(global_line_num);
                            new_lines.push(IniLine::Comment(format!(";-; DISABLED_BY_NRMM {}", text)));
                        } else {
                            new_lines.push(line.clone());
                        }
                    }
                    IniLine::KeyValue { key, value, disabled: false, comment } => {
                        let lower_key = key.to_lowercase();
                        let should_comment = if in_texture_override || in_conditional_block > 0 {
                            false
                        } else {
                            lower_key == "drawindexed"
                                || lower_key == "draw"
                                || (lower_key == "ib" && !value.to_lowercase().starts_with("resource"))
                        };

                        if should_comment {
                            commented_lines.push(global_line_num);
                            let mut orig = format!("{} = {}", key, value);
                            if let Some(c) = comment {
                                orig.push_str(&format!(" ; {}", c));
                            }
                            new_lines.push(IniLine::Comment(format!(";-; DISABLED_BY_NRMM {}", orig)));
                        } else {
                            new_lines.push(line.clone());
                        }
                    }
                    other => new_lines.push(other.clone()),
                }
                global_line_num += 1;
            }
            section.lines = new_lines;
        }

        commented_lines
    }

    pub fn detect_errors(&self, mod_path: &Path, known_libraries: &std::collections::HashSet<String>) -> Vec<ErroredLines> {
        let mut errors: Vec<ErroredLines> = Vec::new();

        let mut defined_libs = std::collections::HashSet::new();
        let mut lib_sections: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();
        let mut line_num = 1u32;

        for _ in &self.preamble {
            line_num += 1;
        }

        for section in &self.sections {
            line_num += 1;
            let name_lower = section.name.to_lowercase();
            if name_lower.starts_with("resource")
                || name_lower.starts_with("commandlist")
                || name_lower.starts_with("shaderoverride")
            {
                defined_libs.insert(section.name.clone());
                lib_sections.entry(section.name.clone()).or_default().push(line_num - 1);
            }
        }

        for (lib_name, lines) in &lib_sections {
            if lines.len() > 1 {
                for &ln in lines {
                    errors.push(ErroredLines {
                        line_number: ln,
                        line: format!("[{}]", lib_name),
                        error_type: 0,
                        ..Default::default()
                    });
                }
            }
        }

        let mut if_stack: Vec<(u32, String)> = Vec::new();
        line_num = 1;

        for line in &self.preamble {
            if let IniLine::PreambleLine(text) = line {
                let trimmed = text.trim_start();
                if trimmed.starts_with("if ") {
                    if_stack.push((line_num, text.clone()));
                } else if trimmed.starts_with("endif") {
                    if_stack.pop();
                }
            }
            line_num += 1;
        }

        for section in &self.sections {
            line_num += 1;
            for line in &section.lines {
                match line {
                    IniLine::IfStart { condition, .. } => {
                        if_stack.push((line_num, format!("if {}", condition)));
                    }
                    IniLine::EndIf { .. } => {
                        if if_stack.pop().is_none() {
                            errors.push(ErroredLines {
                                line_number: line_num,
                                line: "endif".to_string(),
                                error_type: 3,
                                ..Default::default()
                            });
                        }
                    }
                    IniLine::KeyValue { key, value, .. } | IniLine::DisabledKeyValue { key, value, .. } => {
                        let lower_key = key.to_lowercase();
                        let is_ref_key = lower_key == "run"
                            || (lower_key.starts_with("vb") && lower_key.len() >= 3)
                            || lower_key == "ib"
                            || lower_key.starts_with("ps-t")
                            || lower_key.starts_with("vs-t")
                            || lower_key.starts_with("ps-")
                            || lower_key.starts_with("vs-")
                            || lower_key.starts_with("cs-")
                            || (lower_key.starts_with('o') && lower_key.len() >= 2)
                            || (lower_key.starts_with('u') && lower_key.len() >= 2);

                        if is_ref_key && !value.is_empty() {
                            let ref_name = value.trim();
                            if !ref_name.is_empty()
                                && !known_libraries.contains(ref_name)
                                && !defined_libs.contains(ref_name)
                                && !ref_name.starts_with("Resource")
                                && !ref_name.is_empty()
                            {
                                errors.push(ErroredLines {
                                    line_number: line_num,
                                    line: format!("{} = {}", key, value),
                                    error_type: 1,
                                    ..Default::default()
                                });
                            }
                        }
                    }
                    _ => {}
                }
                line_num += 1;
            }
        }

        for (ln, _) in if_stack {
            errors.push(ErroredLines {
                line_number: ln,
                line: "if (missing endif)".to_string(),
                error_type: 3,
                ..Default::default()
            });
        }

        let path_str = mod_path.to_string_lossy();
        if path_str.len() > 260 {
            errors.push(ErroredLines {
                line_number: 0,
                line: format!("Path too long: {} ({} chars)", path_str, path_str.len()),
                error_type: 4,
                ..Default::default()
            });
        }

        errors
    }

    pub fn all_section_names(&self) -> Vec<&str> {
        self.sections.iter().map(|s| s.name.as_str()).collect()
    }

    pub fn defined_libraries(&self) -> std::collections::HashSet<String> {
        let mut libs = std::collections::HashSet::new();
        for section in &self.sections {
            let name_lower = section.name.to_lowercase();
            if name_lower.starts_with("resource")
                || name_lower.starts_with("commandlist")
                || name_lower.starts_with("shaderoverride")
            {
                libs.insert(section.name.clone());
            }
        }
        libs
    }

    pub fn has_include(&self) -> bool {
        for line in &self.preamble {
            if matches!(line, IniLine::Include(_)) {
                return true;
            }
            if let IniLine::PreambleLine(t) = line {
                if t.trim_start().starts_with("include") {
                    return true;
                }
            }
        }
        for section in &self.sections {
            for line in &section.lines {
                if matches!(line, IniLine::Include(_)) {
                    return true;
                }
            }
        }
        false
    }

    pub fn line_count(&self) -> usize {
        let mut count = self.preamble.len();
        for section in &self.sections {
            count += 1 + section.lines.len();
        }
        count
    }
}

#[cfg(test)]
mod tests {
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
    fn test_parse_basic_ini() {
        let ini_content = r#"; Header comment
namespace = TestMod

[Constants]
global persist $myvar = 0
$test = 1

[TextureOverrideTest]
hash = 0x12345678
ps-t0 = ResourceTestTex
vb0 = ResourceVb
drawindexed = auto
"#;
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        assert!(!ini.preamble.is_empty());
        assert_eq!(ini.sections.len(), 2);
        assert_eq!(ini.sections[0].name, "Constants");
        assert_eq!(ini.sections[1].name, "TextureOverrideTest");
        assert!(ini.sections[1].is_conditional);
    }

    #[test]
    fn test_parse_conditional_blocks() {
        let ini_content = r#"[CommandListTest]
if $condition == 1
  drawindexed
elif $condition == 2
  drawindexed = auto
else
  draw
endif
"#;
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        let lines = &ini.sections[0].lines;
        assert!(matches!(lines[0], IniLine::IfStart { .. }));
        assert!(matches!(lines[2], IniLine::Elif { .. }));
        assert!(matches!(lines[4], IniLine::Else { .. }));
        assert!(matches!(lines[6], IniLine::EndIf { .. }));
    }

    #[test]
    fn test_parse_disabled_lines() {
        let ini_content = r#"[Test]
key = value
;-; disabledkey = disabledval
; comment
"#;
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        let lines = &ini.sections[0].lines;
        assert!(matches!(lines[0], IniLine::KeyValue { disabled: false, .. }));
        assert!(matches!(lines[1], IniLine::DisabledKeyValue { .. }));
        assert!(matches!(lines[2], IniLine::Comment(_)));
    }

    #[test]
    fn test_write_atomic_roundtrip() {
        let ini_content = "; Preamble comment\n\n[Constants]\n$test = 42\n\n[TextureOverrideFoo]\nhash = 0xDEADBEEF\nps-t0 = ResourceFoo\n";
        let f_in = write_temp_ini(ini_content);
        let ini = IniFile::parse(f_in.path()).unwrap();

        let f_out = NamedTempFile::with_suffix(".ini").unwrap();
        ini.write_atomic(f_out.path()).unwrap();

        let ini2 = IniFile::parse(f_out.path()).unwrap();
        assert_eq!(ini.sections.len(), ini2.sections.len());
        assert_eq!(ini.sections[0].name, ini2.sections[0].name);
        assert_eq!(ini.sections[1].name, ini2.sections[1].name);
    }

    #[test]
    fn test_backup_restore() {
        let ini_content = "[Test]\nkey = original\n";
        let f = write_temp_ini(ini_content);
        let orig = IniFile::force_read_as_utf8(f.path()).unwrap();

        let backup = IniFile::backup(f.path()).unwrap();
        assert!(backup.exists());

        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.sections[0].lines.push(IniLine::KeyValue {
            key: "added".to_string(),
            value: "new".to_string(),
            disabled: false,
            comment: None,
        });
        ini.write_atomic(f.path()).unwrap();
        assert_ne!(IniFile::force_read_as_utf8(f.path()).unwrap(), orig);

        IniFile::restore_from_backup(f.path()).unwrap();
        assert!(!backup.exists());
        assert_eq!(IniFile::force_read_as_utf8(f.path()).unwrap(), orig);
    }

    #[test]
    fn test_inject_slot_conditions() {
        let ini_content = "[Constants]\n$active = 0\n$other = hello\n\n[TextureOverrideTest]\nhash = 0x123\nps-t0 = Res\ndrawindexed\n\n[Present]\nx = 1\n";
        let f = write_temp_ini(ini_content);
        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.inject_slot_conditions(1, 2);

        let to_lines = &ini.sections[1].lines;
        assert!(matches!(to_lines[0], IniLine::IfStart { .. }));
        assert!(matches!(to_lines[1], IniLine::KeyValue { ref key, .. } if key == "allow_duplicate_hash"));
        assert!(to_lines.iter().any(|l| matches!(l, IniLine::EndIf { .. })));

        let constants_lines = &ini.sections[0].lines;
        assert!(matches!(constants_lines[0], IniLine::IfStart { .. }));
        assert!(matches!(constants_lines[2], IniLine::EndIf { .. }));
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
        let ini_content = "[Constants]\nx = 1\ndrawindexed\n\n[TextureOverrideSafe]\nhash = 0x1\ndrawindexed\n";
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
    fn test_detect_duplicate_libs() {
        let ini_content = "[ResourceTest]\nfilename = test.dds\n\n[ResourceTest]\nfilename = test2.dds\n";
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        let known = std::collections::HashSet::new();
        let errors = ini.detect_errors(f.path().parent().unwrap(), &known);
        assert!(errors.iter().any(|e| e.error_type == 0));
    }

    #[test]
    fn test_detect_nonexistent_libs() {
        let ini_content = "[TextureOverrideT]\nps-t0 = NonExistentTex\n";
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        let known = std::collections::HashSet::new();
        let errors = ini.detect_errors(f.path().parent().unwrap(), &known);
        assert!(errors.iter().any(|e| e.error_type == 1));
    }

    #[test]
    fn test_force_read_utf8_bom() {
        let mut f = NamedTempFile::with_suffix(".ini").unwrap();
        f.write_all(&[0xEF, 0xBB, 0xBF]).unwrap();
        f.write_all(b"[Test]\nkey=val\n").unwrap();
        f.flush().unwrap();
        let content = IniFile::force_read_as_utf8(f.path()).unwrap();
        assert!(!content.starts_with('\u{FEFF}'));
        assert!(content.contains("[Test]"));
    }

    #[test]
    fn test_include_detection() {
        let ini_content = "include = common.ini\n\n[Test]\ninclude = other.ini\n";
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        assert!(ini.has_include());
    }
}
