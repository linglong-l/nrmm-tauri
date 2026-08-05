use std::path::{Path, PathBuf};
use std::io::{Write, BufWriter};
use anyhow::{Result, Context, bail};
use std::fs;
use regex::Regex;

use crate::core::constants;
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

/// 判断段名是否属于"按键触发类条件段"（大小写不敏感前缀匹配）
///
/// 严格映射 constants::CONDITIONAL_SECTION_PREFIXES 常量列表，避免与常量定义重复。
pub fn is_conditional_section(name: &str) -> bool {
    let lower = name.to_lowercase();
    constants::CONDITIONAL_SECTION_PREFIXES
        .iter()
        .any(|p| lower.starts_with(&p.to_lowercase()))
}

/// 判断段名是否为 Key 触发段（大小写不敏感前缀匹配 "key"）
///
/// 对应 NRMM 的 `_isKeySection`（`sectionName.toLowerCase().startsWith("key")`）。
/// Key 段在模组数据更新时使用 condition 追加方式注入槽位条件（而非 if 包裹）。
pub fn is_key_section(name: &str) -> bool {
    name.to_lowercase().starts_with("key")
}

/// 判断表达式是否被一对匹配的括号整体包裹（外层括号闭合于末尾）
///
/// 对应 NRMM 的 `_isWrappedInMatchingParens`。用于决定 condition 追加时
/// 是否需要为原表达式补括号：已整体包裹则直接 `&&`，否则用 `(...) &&`。
fn is_wrapped_in_matching_parens(expr: &str) -> bool {
    let expr = expr.trim();
    if !expr.starts_with('(') || !expr.ends_with(')') {
        return false;
    }
    let mut depth = 0usize;
    for (i, c) in expr.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                // 若最外层括号在末尾前已闭合，说明并非整体包裹（如 "(a)(b)"）
                if depth == 0 && i != expr.len() - 1 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// 从 condition 表达式中移除本管理器注入的槽位条件，避免重复更新导致嵌套
///
/// 对应 NRMM 的 `_sanitizeKeyConditionExpressionFromModManager`：
/// 1. 正则移除 `$managed_slot_id == $\modmanageragl\group_X\<token>` 段
/// 2. 清理移除后遗留的空括号、悬空 `&&`/`||`、多余逻辑连接符
/// 3. 若整段被外层括号包裹，则解包
///
/// 返回清理后的表达式（可能为空字符串，表示该行原本仅含管理器条件）。
pub fn sanitize_condition_expression_public(expression: &str) -> String {
    sanitize_condition_expression(expression)
}

fn sanitize_condition_expression(expression: &str) -> String {
    // 匹配 §managed_slot_id == $\\modmanageragl\\group_<数字>\\<token>，
    // token 可为 active_slot（NRMM）或数字编号（本项目历史注入）。
    let manager_re = Regex::new(
        r"\$managed_slot_id\s*==\s*\$\\modmanageragl\\group_\d+\\[A-Za-z0-9_]+",
    )
    .unwrap();
    if !manager_re.is_match(expression) {
        return expression.to_string();
    }

    let mut expr = manager_re.replace_all(expression, "").trim().to_string();

    // "(&& x)" -> "(x)"
    expr = Regex::new(r"\(\s*&&\s*").unwrap().replace_all(&expr, "(").trim().to_string();
    // "(|| x)" -> "(x)"
    expr = Regex::new(r"\(\s*\|\|\s*").unwrap().replace_all(&expr, "(").trim().to_string();
    // "(x && )" -> "(x)"
    expr = Regex::new(r"\s*&&\s*\)").unwrap().replace_all(&expr, ")").trim().to_string();
    // "(x || )" -> "(x)"
    expr = Regex::new(r"\s*\|\|\s*\)").unwrap().replace_all(&expr, ")").trim().to_string();
    // "()" -> ""
    expr = Regex::new(r"\(\s*\)").unwrap().replace_all(&expr, "").trim().to_string();
    // 尾部悬空 "&&"（连续移除，避免残留）
    while expr.trim_end().ends_with("&&") {
        expr = expr.trim_end()[..expr.trim_end().len() - 2].trim().to_string();
    }
    // 尾部悬空 "||"
    while expr.trim_end().ends_with("||") {
        expr = expr.trim_end()[..expr.trim_end().len() - 2].trim().to_string();
    }
    // 头部悬空 "&&"
    if expr.starts_with("&&") {
        expr = expr.replacen("&&", "", 1).trim().to_string();
    }
    // 头部悬空 "||"
    if expr.starts_with("||") {
        expr = expr.replacen("||", "", 1).trim().to_string();
    }
    // "&& &&" -> "&&"
    expr = Regex::new(r"&&\s*&&").unwrap().replace_all(&expr, "&&").trim().to_string();
    // "&& ||" -> "||"
    expr = Regex::new(r"&&\s*\|\|").unwrap().replace_all(&expr, "||").trim().to_string();
    // "|| ||" -> "||"
    expr = Regex::new(r"\|\|\s*\|\|").unwrap().replace_all(&expr, "||").trim().to_string();
    // "|| &&" -> "&&"
    expr = Regex::new(r"\|\|\s*&&").unwrap().replace_all(&expr, "&&").trim().to_string();

    // 若管理器表达式整体被外层括号包裹，解包
    if is_wrapped_in_matching_parens(&expr) {
        expr = expr[1..expr.len() - 1].trim().to_string();
    }
    expr
}

fn trim_trailing_whitespace(s: &str) -> &str {
    s.trim_end_matches(|c: char| ['\r', '\n', ' ', '\t'].contains(&c))
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
            let comment = after_quote.find(';').map(|semi_pos| after_quote[semi_pos + 1..].trim().to_string());
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

impl std::fmt::Display for IniLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IniLine::Empty => Ok(()),
            IniLine::Comment(text) => write!(f, "{}", text),
            IniLine::DisabledKeyValue { key, value, comment } => {
                write!(f, ";-;{} = {}", key, value)?;
                if let Some(c) = comment {
                    write!(f, " ; {}", c)?;
                }
                Ok(())
            }
            IniLine::KeyValue { key, value, disabled, comment } => {
                if *disabled {
                    write!(f, ";-;")?;
                }
                write!(f, "{} = {}", key, value)?;
                if let Some(c) = comment {
                    write!(f, " ; {}", c)?;
                }
                Ok(())
            }
            IniLine::IfStart { condition, indent } => {
                write!(f, "{}if {}", " ".repeat(*indent), condition)
            }
            IniLine::Elif { condition, indent } => {
                write!(f, "{}elif {}", " ".repeat(*indent), condition)
            }
            IniLine::Else { indent } => {
                write!(f, "{}else", " ".repeat(*indent))
            }
            IniLine::EndIf { indent } => {
                write!(f, "{}endif", " ".repeat(*indent))
            }
            IniLine::Command(text) => write!(f, "{}", text),
            IniLine::SectionHeader(name) => write!(f, "[{}]", name),
            IniLine::Include(path) => write!(f, "include = {}", path),
            IniLine::PreambleLine(text) => write!(f, "{}", text),
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
                let after_prefix = line.strip_prefix(";-;").unwrap_or(line).trim_start();
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
                let condition = trimmed.strip_prefix("if ").unwrap_or(trimmed).trim().to_string();
                let l = IniLine::IfStart { condition, indent };
                match current_section {
                    Some(idx) => sections[idx].lines.push(l),
                    None => preamble.push(IniLine::PreambleLine(l.to_string())),
                }
                continue;
            }

            if trimmed.starts_with("elif ") {
                let condition = trimmed.strip_prefix("elif ").unwrap_or(trimmed).trim().to_string();
                let l = IniLine::Elif { condition, indent };
                match current_section {
                    Some(idx) => sections[idx].lines.push(l),
                    None => preamble.push(IniLine::PreambleLine(l.to_string())),
                }
                continue;
            }

            let after_else = trimmed.strip_prefix("else");
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

    /// 在文件头部添加 NRMM 标准注释头（对齐 NRMM 输出格式）
    pub fn prepend_header_comment(&mut self) {
        let header_lines = [
            "; \";-;\" are errored conditional lines.",
            "; \";+;\" are disabled keys.",
            "; Errored conditional blocks (if/else/elif/endif) are handled correctly (newer syntax may require further testing), including namespaced variables.",
            "; If certain syntax is only available in newer XXMI versions, make sure to use the latest XXMI.",
        ];
        for line in header_lines.iter().rev() {
            self.preamble.insert(0, IniLine::Comment(line.to_string()));
        }
    }

    pub fn write_atomic(&self, path: &Path) -> Result<()> {
        let tmp_path = path.with_extension("ini.tmp");
        let file = fs::File::create(&tmp_path)
            .with_context(|| format!("Failed to create temp file: {:?}", tmp_path))?;
        let mut writer = BufWriter::new(file);

        for line in &self.preamble {
            writeln!(writer, "{}", line)?;
        }

        let mut first_section = true;
        for section in &self.sections {
            if !first_section {
                writeln!(writer)?;
            }
            first_section = false;

            writeln!(writer, "[{}]", section.name)?;
            for line in &section.lines {
                writeln!(writer, "{}", line)?;
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
                        if let Some(suffix) = key_str.strip_prefix(rp) {
                            if suffix.is_empty() || suffix.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
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

    pub fn inject_slot_conditions(&mut self, group_id: u32) {
        let condition_var = format!(
            "$managed_slot_id == $\\modmanageragl\\group_{}\\active_slot",
            group_id
        );

        for section in &mut self.sections {
            // Key 段：与 NRMM 一致，使用 condition 追加方式注入槽位条件，
            // 并清理旧的管理器表达式，避免重复更新导致嵌套。
            if is_key_section(&section.name) {
                Self::inject_key_condition(section, &condition_var);
                continue;
            }

            // NRMM 对齐：仅对按键触发段（历史逻辑）+ 注入白名单段（Present/CustomShader/
            // TextureOverride/ShaderOverride/CommandList/Resource/InputLayout/Draw*/Dispatch 等）
            // 包裹 VariableGroup 条件；其余段（如 String 等）不做处理。
            let should_wrap = is_conditional_section(&section.name)
                || constants::is_injectable_section(&section.name);

            if should_wrap {
                let has_existing_condition = section.lines.iter().any(|line| {
                    matches!(line, IniLine::IfStart { condition, .. } if condition.contains("managed_slot_id"))
                });

                if has_existing_condition {
                    continue;
                }

                let first_idx = Self::first_command_line_index(&section.lines);
                let last_idx = Self::last_command_line_index(&section.lines);

                if let (Some(first), Some(last)) = (first_idx, last_idx) {
                    // NRMM 对齐：先插入 if，再插入 match_priority=0（match_priority 在 if 之前）
                    section.lines.insert(first, IniLine::IfStart {
                        condition: condition_var.clone(),
                        indent: 0,
                    });
                    section.lines.insert(first, IniLine::KeyValue {
                        key: "match_priority".to_string(),
                        value: "0".to_string(),
                        disabled: false,
                        comment: None,
                    });

                    // 2 个元素插入在 last 之前，last 偏移 +2，endif 在 last+3
                    let insert_end = last + 3;
                    section.lines.insert(insert_end.min(section.lines.len()), IniLine::EndIf { indent: 0 });
                }
            }
        }
    }

    /// 对 Key 段注入槽位条件（condition 追加方式，对齐 NRMM）
    ///
    /// 处理流程：
    /// 1. 若该段已被旧版 if 包裹（含 managed_slot_id 的 IfStart），保持幂等直接跳过；
    /// 2. 清理所有 `condition`/disabled condition 行中遗留的旧管理器表达式；
    /// 3. 若无有效 condition 行，在段首插入 `condition = <manager_expr>`；
    /// 4. 否则对每个 condition 行追加 `&& <manager_expr>`（必要时用括号包裹原表达式）。
    ///
    /// # 参数
    /// - `section`: 待处理的 Key 段
    /// - `condition_var`: 本管理器注入的槽位条件表达式（如 `$managed_slot_id == $...\group_1\2`）
    fn inject_key_condition(section: &mut IniSection, condition_var: &str) {
        // 若已用旧版 if 包裹（含 managed_slot_id 的 IfStart），保持幂等，跳过注入
        if section.lines.iter().any(|l| {
            matches!(l, IniLine::IfStart { condition, .. } if condition.contains("managed_slot_id"))
        }) {
            return;
        }

        // 阶段1：清理所有 condition 行的旧管理器表达式
        struct CleanedCond {
            index: usize,
            disabled: bool,
            value: String,
        }
        let mut cleaned: Vec<CleanedCond> = Vec::new();
        for (i, line) in section.lines.iter().enumerate() {
            if let IniLine::KeyValue { key, value, disabled, .. } = line {
                if key.eq_ignore_ascii_case("condition") {
                    cleaned.push(CleanedCond {
                        index: i,
                        disabled: *disabled,
                        value: sanitize_condition_expression(value),
                    });
                }
            }
        }

        if cleaned.is_empty() {
            // 无 condition 行，插入新条件
            section.lines.insert(0, IniLine::KeyValue {
                key: "condition".to_string(),
                value: condition_var.to_string(),
                disabled: false,
                comment: None,
            });
        } else {
            // 有 condition 行，逐条追加（清理后为空则直接替换为管理器表达式）
            for cond in &cleaned {
                let rhs = cond.value.trim();
                let new_value = if rhs.is_empty() {
                    condition_var.to_string()
                } else if is_wrapped_in_matching_parens(rhs) {
                    format!("{} && {}", rhs, condition_var)
                } else {
                    format!("({}) && {}", rhs, condition_var)
                };
                section.lines[cond.index] = IniLine::KeyValue {
                    key: "condition".to_string(),
                    value: new_value,
                    disabled: cond.disabled,
                    comment: None,
                };
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
        ini.inject_slot_conditions(1);

        // TextureOverride 段：match_priority=0 在 if 之前，无 allow_duplicate_hash
        let to_lines = &ini.sections[1].lines;
        assert!(matches!(to_lines[0], IniLine::KeyValue { ref key, ref value, .. } if key == "match_priority" && value == "0"));
        assert!(matches!(to_lines[1], IniLine::IfStart { ref condition, .. } if condition.contains("active_slot")));
        assert!(to_lines.iter().any(|l| matches!(l, IniLine::EndIf { .. })));
        assert!(!to_lines.iter().any(|l| matches!(l, IniLine::KeyValue { ref key, .. } if key == "allow_duplicate_hash")));

        // Constants 段：NRMM 不包裹，变量原样保留
        let constants_lines = &ini.sections[0].lines;
        assert!(!constants_lines.iter().any(|l| matches!(l, IniLine::IfStart { .. })));
        assert!(!constants_lines.iter().any(|l| matches!(l, IniLine::EndIf { .. })));
    }

    #[test]
    fn test_inject_key_condition_no_existing() {
        // Key 段无 condition 行时，应插入一条 condition = manager_expr
        let ini_content = "[KeyDefault]\nkey = a\n";
        let f = write_temp_ini(ini_content);
        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.inject_slot_conditions(1);

        let lines = &ini.sections[0].lines;
        assert!(matches!(lines[0], IniLine::KeyValue { ref key, ref value, .. }
            if key.eq_ignore_ascii_case("condition") && value.contains("$managed_slot_id")));
        // 不应产生 if 包裹
        assert!(!lines.iter().any(|l| matches!(l, IniLine::IfStart { .. })));
    }

    #[test]
    fn test_inject_key_condition_append() {
        // Key 段已有 condition 行时，应追加 && manager_expr
        let ini_content = "[KeyDefault]\ncondition = $Key1 == 1\n";
        let f = write_temp_ini(ini_content);
        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.inject_slot_conditions(1);

        let lines = &ini.sections[0].lines;
        let cond = lines.iter().find_map(|l| match l {
            IniLine::KeyValue { key, value, .. } if key.eq_ignore_ascii_case("condition") => Some(value.clone()),
            _ => None,
        });
        let cond = cond.expect("condition line should exist");
        assert!(cond.contains("$Key1 == 1"));
        assert!(cond.contains("$managed_slot_id"));
        assert!(cond.contains("&&"));
    }

    #[test]
    fn test_inject_key_condition_idempotent() {
        // 重复注入不应累积 manager 表达式（幂等）
        let ini_content = "[KeyDefault]\ncondition = $Key1 == 1\n";
        let f = write_temp_ini(ini_content);
        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.inject_slot_conditions(1);
        ini.inject_slot_conditions(1);
        ini.inject_slot_conditions(1);

        let lines = &ini.sections[0].lines;
        let cond = lines.iter().find_map(|l| match l {
            IniLine::KeyValue { key, value, .. } if key.eq_ignore_ascii_case("condition") => Some(value.clone()),
            _ => None,
        });
        let cond = cond.expect("condition line should exist");
        // 仅应出现一次 $managed_slot_id
        assert_eq!(cond.matches("$managed_slot_id").count(), 1);
    }

    #[test]
    fn test_sanitize_condition_expression() {
        // 清理 NRMM 格式（active_slot）的旧表达式
        let expr = "$Key1 == 1 && $managed_slot_id == $\\modmanageragl\\group_1\\active_slot";
        let cleaned = sanitize_condition_expression(expr);
        assert_eq!(cleaned.trim(), "$Key1 == 1");
        assert!(!cleaned.contains("$managed_slot_id"));

        // 清理本项目历史格式（数字编号）的旧表达式
        let expr2 = "$Key1 == 1 && $managed_slot_id == $\\modmanageragl\\group_1\\2";
        let cleaned2 = sanitize_condition_expression(expr2);
        assert_eq!(cleaned2.trim(), "$Key1 == 1");

        // 无管理器表达式时原样返回
        let expr3 = "$Key1 == 1";
        assert_eq!(sanitize_condition_expression(expr3), expr3);

        // 仅含管理器表达式时清理为空
        let expr4 = "$managed_slot_id == $\\modmanageragl\\group_1\\active_slot";
        assert_eq!(sanitize_condition_expression(expr4).trim(), "");
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

    // ========== is_conditional_section（由 constants::CONDITIONAL_SECTION_PREFIXES 驱动） ==========
    #[test]
    fn test_is_conditional_section_aligned_with_constants_prefixes() {
        use crate::core::constants::CONDITIONAL_SECTION_PREFIXES;

        // 每个前缀的原样/大写/驼峰形式都应匹配（大小写不敏感）
        for prefix in CONDITIONAL_SECTION_PREFIXES {
            let sample = format!("{}Example", prefix);
            assert!(
                is_conditional_section(&sample),
                "prefix sample {} should be conditional",
                sample
            );
            let sample_upper = format!("{}EXAMPLE", prefix.to_uppercase());
            assert!(
                is_conditional_section(&sample_upper),
                "uppercase prefix sample {} should be conditional",
                sample_upper
            );
        }

        // 已知真实 INI 段名：按键触发类 / 覆盖类 / 命令列表
        assert!(is_conditional_section("Key1"));
        assert!(is_conditional_section("keypress"));
        assert!(is_conditional_section("TextureOverride_Ningguang_Dress"));
        assert!(is_conditional_section("shaderoverride_vs"));
        assert!(is_conditional_section("CommandList_Post"));
    }

    #[test]
    fn test_is_conditional_section_rejects_non_conditional() {
        // Present / CustomShader 现在属于 INJECTABLE（由 is_injectable_section 驱动），
        // 但不是 CONDITIONAL_SECTION_PREFIXES 列表，故 is_conditional_section 返回 false
        assert!(!is_conditional_section("Present"));
        assert!(!is_conditional_section("CustomShader01"));
        // Constants / String 段：二者均非条件段
        assert!(!is_conditional_section("Constants"));
        assert!(!is_conditional_section("String"));
        // 空字符串
        assert!(!is_conditional_section(""));
    }
}
