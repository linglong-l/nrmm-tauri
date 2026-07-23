//! INI 错误检测模块
//!
//! 该模块负责扫描 INI 文件并检测各类语法错误与潜在问题，包括：
//! - 流程控制错误（if/elif/else/endif 不匹配、空条件、重复 else 等）
//! - 崩溃行（未闭合的方括号、空段名、空 if/elif 条件等会导致 3DMigoto 崩溃的行）
//! - 重复的模组库命名空间
//! - 引用了不存在的模组库
//! - 过长的文件路径（超过 Windows MAX_PATH 限制）
//!
//! 检测结果通过 `ErroredLinesReport` 结构体汇总返回，供前端展示与告警。

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::{
    detect_flow_control, get_section_type, is_comment_line, parse_file, FlowControlType, IniFile,
    IniSection, SectionType,
};
use crate::utils::DirWalker;

#[cfg(test)]
use super::parse_content;

/// 单条 INI 语法错误描述。
///
/// 每个错误包含所属文件路径、行号、原始行文本及错误原因。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IniSyntaxError {
    /// 错误所在文件路径。
    pub file_path: String,
    /// 错误所在行号（从 0 开始）。
    pub line_index: usize,
    /// 错误行的原始文本（已去除首尾空白）。
    pub trimmed_line: String,
    /// 错误原因描述（如 `"Missing \"endif\""`、`"CRASH LINE: ..."`）。
    pub reason: String,
}

/// 错误检测汇总报告。
///
/// 按错误类型分类存储，便于前端分类展示。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ErroredLinesReport {
    /// 重复的模组库命名空间：命名空间 -> 出现该命名空间的文件路径列表。
    pub duplicate_libs: HashMap<String, Vec<String>>,
    /// 引用了不存在的模组库：命名空间 -> 首次引用该命名空间的文件路径。
    pub non_existent_libs: HashMap<String, String>,
    /// 会导致 3DMigoto 崩溃的行：文件路径 -> 错误列表（reason 以 `"CRASH LINE"` 开头）。
    pub crash_lines: HashMap<String, Vec<IniSyntaxError>>,
    /// 其他语法错误：文件路径 -> 错误列表。
    pub other_errors: HashMap<String, Vec<IniSyntaxError>>,
    /// 缺少 endif 的错误：文件路径 -> 错误列表（reason 为 `"Missing \"endif\""`）。
    pub missing_endif_errors: HashMap<String, Vec<IniSyntaxError>>,
    /// 路径过长的文件列表（长度超过 255 字符）。
    pub long_path_errors: Vec<String>,
}

/// 判断指定段类型是否支持流程控制（if/elif/else/endif）。
///
/// 仅 `CommandList`、`CommandListPost` 和 `Key` 段会进行流程控制检测，
/// 其他段（如 `Constants`）中的 `if` 等关键字不会被视为流程控制。
fn is_flow_control_section(section_type: SectionType) -> bool {
    matches!(
        section_type,
        SectionType::CommandList | SectionType::CommandListPost | SectionType::Key
    )
}

/// 分析单个段内的 if/elif/else/endif 流程控制结构，返回所有语法错误。
///
/// 检测项：
/// - `if` 条件为空
/// - `elif` 出现在 `if` 之外，或出现在 `else` 之后
/// - `else` 出现在 `if` 之外，或重复出现
/// - `endif` 出现在 `if` 之外
/// - `if` 块未闭合（缺少 `endif`）
///
/// 参数：
/// - `section`: 要分析的 INI 段引用。
/// - `file_path`: 所属文件路径（用于错误报告）。
///
/// 返回：该段内所有流程控制错误列表。若该段类型不支持流程控制则返回空列表。
///
/// 当前未消费，保留用于后续按段细粒度流程控制分析。
#[allow(dead_code)]
pub fn analyze_flow_control_section(section: &IniSection, file_path: &str) -> Vec<IniSyntaxError> {
    let mut errors = Vec::new();

    let section_type = get_section_type(&section.name);
    if !is_flow_control_section(section_type) {
        return errors;
    }

    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut else_found = false;

    for (i, raw_line) in section.raw_lines.iter().enumerate() {
        let trimmed = raw_line.trim();
        if is_comment_line(trimmed) || trimmed.is_empty() {
            continue;
        }

        let line_index = section.line_index + 1 + i;

        if let Some(flow_type) = detect_flow_control(trimmed) {
            match flow_type {
                FlowControlType::If => {
                    let condition = trimmed
                        .trim_start_matches(|c: char| c.is_whitespace() || c == 'i' || c == 'f')
                        .trim();
                    if condition.is_empty() {
                        errors.push(IniSyntaxError {
                            file_path: file_path.to_string(),
                            line_index,
                            trimmed_line: trimmed.to_string(),
                            reason: "Empty condition".to_string(),
                        });
                    }
                    stack.push((line_index, trimmed.to_string()));
                    else_found = false;
                }
                FlowControlType::ElseIf => {
                    if stack.is_empty() {
                        errors.push(IniSyntaxError {
                            file_path: file_path.to_string(),
                            line_index,
                            trimmed_line: trimmed.to_string(),
                            reason: "Unexpected \"elif\"".to_string(),
                        });
                    } else if else_found {
                        errors.push(IniSyntaxError {
                            file_path: file_path.to_string(),
                            line_index,
                            trimmed_line: trimmed.to_string(),
                            reason: "Unexpected \"elif\" after \"else\"".to_string(),
                        });
                    } else {
                        let condition = trimmed
                            .trim_start_matches(|c: char| {
                                c.is_whitespace()
                                    || c == 'e'
                                    || c == 'l'
                                    || c == 'i'
                                    || c == 'f'
                            })
                            .trim();
                        if condition.is_empty() {
                            errors.push(IniSyntaxError {
                                file_path: file_path.to_string(),
                                line_index,
                                trimmed_line: trimmed.to_string(),
                                reason: "Empty condition".to_string(),
                            });
                        }
                    }
                }
                FlowControlType::Else => {
                    if stack.is_empty() {
                        errors.push(IniSyntaxError {
                            file_path: file_path.to_string(),
                            line_index,
                            trimmed_line: trimmed.to_string(),
                            reason: "Unexpected \"else\"".to_string(),
                        });
                    } else if else_found {
                        errors.push(IniSyntaxError {
                            file_path: file_path.to_string(),
                            line_index,
                            trimmed_line: trimmed.to_string(),
                            reason: "Duplicate \"else\"".to_string(),
                        });
                    } else {
                        else_found = true;
                    }
                }
                FlowControlType::EndIf => {
                    if stack.is_empty() {
                        errors.push(IniSyntaxError {
                            file_path: file_path.to_string(),
                            line_index,
                            trimmed_line: trimmed.to_string(),
                            reason: "Unexpected \"endif\"".to_string(),
                        });
                    } else {
                        stack.pop();
                        else_found = false;
                    }
                }
            }
        }
    }

    for (line_idx, line_text) in &stack {
        errors.push(IniSyntaxError {
            file_path: file_path.to_string(),
            line_index: *line_idx,
            trimmed_line: line_text.clone(),
            reason: "Missing \"endif\"".to_string(),
        });
    }

    errors
}

/// 检查 INI 文件中所有支持流程控制的段内的 if/elif/else/endif 匹配性。
///
/// 检测项：
/// - `if` 条件为空
/// - `elif` 出现在 `if` 之外，或出现在 `else` 之后
/// - `else` 出现在 `if` 之外，或重复出现
/// - `endif` 出现在 `if` 之外
/// - `if` 块未闭合（缺少 `endif`）
///
/// 参数：
/// - `ini_file`: 已解析的 INI 文件结构。
///
/// 返回：所有流程控制错误列表。
pub fn check_flow_control(ini_file: &IniFile) -> Vec<IniSyntaxError> {
    let mut errors = Vec::new();

    for section in &ini_file.sections {
        let section_type = get_section_type(&section.name);
        // 仅对支持流程控制的段进行检查
        if !is_flow_control_section(section_type) {
            continue;
        }

        // 用栈记录未闭合的 if 块：(行号, 行文本)
        let mut stack: Vec<(usize, String)> = Vec::new();
        let mut else_found = false;

        for (i, raw_line) in section.raw_lines.iter().enumerate() {
            let trimmed = raw_line.trim();
            // 跳过注释行和空行
            if is_comment_line(trimmed) || trimmed.is_empty() {
                continue;
            }

            let line_index = section.line_index + 1 + i;

            if let Some(flow_type) = detect_flow_control(trimmed) {
                match flow_type {
                    FlowControlType::If => {
                        // 提取 if 后的条件文本（去除 "if" 与空白）
                        let condition = trimmed
                            .trim_start_matches(|c: char| c.is_whitespace() || c == 'i' || c == 'f')
                            .trim();
                        if condition.is_empty() {
                            errors.push(IniSyntaxError {
                                file_path: ini_file.path.clone(),
                                line_index,
                                trimmed_line: trimmed.to_string(),
                                reason: "Empty condition".to_string(),
                            });
                        }
                        stack.push((line_index, trimmed.to_string()));
                        // 新的 if 块重置 else 标记
                        else_found = false;
                    }
                    FlowControlType::ElseIf => {
                        if stack.is_empty() {
                            // elif 出现在 if 之外
                            errors.push(IniSyntaxError {
                                file_path: ini_file.path.clone(),
                                line_index,
                                trimmed_line: trimmed.to_string(),
                                reason: "Unexpected \"elif\"".to_string(),
                            });
                        } else if else_found {
                            // elif 出现在 else 之后
                            errors.push(IniSyntaxError {
                                file_path: ini_file.path.clone(),
                                line_index,
                                trimmed_line: trimmed.to_string(),
                                reason: "Unexpected \"elif\" after \"else\"".to_string(),
                            });
                        } else {
                            // 检查 elif 条件是否为空
                            let condition = trimmed
                                .trim_start_matches(|c: char| {
                                    c.is_whitespace()
                                        || c == 'e'
                                        || c == 'l'
                                        || c == 'i'
                                        || c == 'f'
                                })
                                .trim();
                            if condition.is_empty() {
                                errors.push(IniSyntaxError {
                                    file_path: ini_file.path.clone(),
                                    line_index,
                                    trimmed_line: trimmed.to_string(),
                                    reason: "Empty condition".to_string(),
                                });
                            }
                        }
                    }
                    FlowControlType::Else => {
                        if stack.is_empty() {
                            // else 出现在 if 之外
                            errors.push(IniSyntaxError {
                                file_path: ini_file.path.clone(),
                                line_index,
                                trimmed_line: trimmed.to_string(),
                                reason: "Unexpected \"else\"".to_string(),
                            });
                        } else if else_found {
                            // 重复的 else
                            errors.push(IniSyntaxError {
                                file_path: ini_file.path.clone(),
                                line_index,
                                trimmed_line: trimmed.to_string(),
                                reason: "Duplicate \"else\"".to_string(),
                            });
                        } else {
                            else_found = true;
                        }
                    }
                    FlowControlType::EndIf => {
                        if stack.is_empty() {
                            // endif 出现在 if 之外
                            errors.push(IniSyntaxError {
                                file_path: ini_file.path.clone(),
                                line_index,
                                trimmed_line: trimmed.to_string(),
                                reason: "Unexpected \"endif\"".to_string(),
                            });
                        } else {
                            // 闭合栈顶的 if 块
                            stack.pop();
                            else_found = false;
                        }
                    }
                }
            }
        }

        // 栈中剩余的 if 块均为未闭合
        for (line_idx, line_text) in &stack {
            errors.push(IniSyntaxError {
                file_path: ini_file.path.clone(),
                line_index: *line_idx,
                trimmed_line: line_text.clone(),
                reason: "Missing \"endif\"".to_string(),
            });
        }
    }

    errors
}

/// 检测 INI 文件内容中会导致 3DMigoto 崩溃的行。
///
/// 检测项：
/// - 未闭合的方括号（如单独的 `[`）
/// - 空段名（`[]`）
/// - 段名中包含 `[` 或 `]`
/// - `if` / `elif` 条件为空
///
/// 与 `check_flow_control` 的区别：本函数直接对原始文本逐行扫描，
/// 不依赖段解析结果，因此能捕获段解析阶段遗漏的崩溃行。
///
/// 参数：
/// - `content`: INI 文件的原始文本内容。
/// - `file_path`: 文件路径（用于错误报告）。
///
/// 返回：所有崩溃行错误列表。
pub fn detect_crash_lines(content: &str, file_path: &str) -> Vec<IniSyntaxError> {
    let mut errors = Vec::new();

    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        // 跳过空行和注释行
        if trimmed.is_empty() || is_comment_line(trimmed) {
            continue;
        }

        let mut is_crash = false;
        let mut reason = String::new();

        // 检测方括号相关错误
        if trimmed == "[" || trimmed.starts_with('[') && !trimmed.contains(']') {
            // 单独的 [ 或 [ 后无闭合 ]
            is_crash = true;
            reason = "CRASH LINE: Unclosed section bracket".to_string();
        } else if trimmed.starts_with('[') && trimmed.ends_with(']') {
            // 完整的 [xxx] 形式，检查段名合法性
            let section_name = &trimmed[1..trimmed.len() - 1];
            if section_name.is_empty() {
                // 空段名 []
                is_crash = true;
                reason = "CRASH LINE: Empty section name".to_string();
            } else if section_name.contains('[') || section_name.contains(']') {
                // 段名中包含方括号
                is_crash = true;
                reason = "CRASH LINE: Invalid characters in section name".to_string();
            }
        }

        // 检测 if/elif 条件为空
        if let Some(flow_type) = detect_flow_control(trimmed) {
            match flow_type {
                FlowControlType::If => {
                    let after_if = trimmed
                        .trim_start_matches(|c: char| c.is_whitespace() || c == 'i' || c == 'f')
                        .trim();
                    if after_if.is_empty() {
                        is_crash = true;
                        reason = "CRASH LINE: Empty if condition".to_string();
                    }
                }
                FlowControlType::ElseIf => {
                    let after_elif = trimmed
                        .trim_start_matches(|c: char| {
                            c.is_whitespace() || c == 'e' || c == 'l' || c == 'i' || c == 'f'
                        })
                        .trim();
                    if after_elif.is_empty() {
                        is_crash = true;
                        reason = "CRASH LINE: Empty elif condition".to_string();
                    }
                }
                _ => {}
            }
        }

        if is_crash {
            errors.push(IniSyntaxError {
                file_path: file_path.to_string(),
                line_index,
                trimmed_line: trimmed.to_string(),
                reason,
            });
        }
    }

    errors
}

/// 检查模组库（命名空间）的重复与引用问题。
///
/// 检测项：
/// 1. **重复库**：同一命名空间在多个 INI 文件中被定义。
/// 2. **不存在的库**：IN I 文件中引用了已知库命名空间，但该命名空间未在任何文件中定义。
///
/// 参数：
/// - `ini_files`: 所有已解析的 INI 文件列表。
/// - `known_lib_namespaces`: 已知的模组库命名空间列表（用于检测引用了不存在的库）。
/// - `_base_path`: 基础路径（当前未使用）。
///
/// 返回：`(重复库映射, 不存在库映射)`。
pub fn check_mod_libraries(
    ini_files: &[IniFile],
    known_lib_namespaces: &[String],
    _base_path: &str,
) -> (HashMap<String, Vec<String>>, HashMap<String, String>) {
    let mut duplicate_libs: HashMap<String, Vec<String>> = HashMap::new();
    let mut non_existent_libs: HashMap<String, String> = HashMap::new();
    // 命名空间 -> 定义该命名空间的文件路径列表
    let mut found_namespaces: HashMap<String, Vec<String>> = HashMap::new();

    // 收集所有文件中出现的命名空间
    for ini_file in ini_files {
        for ns in &ini_file.namespaces {
            let ns_lower = ns.to_lowercase();
            found_namespaces
                .entry(ns_lower)
                .or_default()
                .push(ini_file.path.clone());
        }
    }

    // 找出在多个文件中定义的命名空间（重复）
    for (ns, paths) in &found_namespaces {
        if paths.len() > 1 {
            duplicate_libs.insert(ns.clone(), paths.clone());
        }
    }

    // 检测引用了不存在的库
    for ini_file in ini_files {
        for section in &ini_file.sections {
            for line in &section.lines {
                let value_lower = line.value.to_lowercase();
                // 仅检查包含 CommandList/TextureOverride/ShaderOverride 引用的行
                if value_lower.contains("commandlist")
                    || value_lower.contains("textureoverride")
                    || value_lower.contains("shaderoverride")
                {
                    for known_ns in known_lib_namespaces {
                        let ns_lower = known_ns.to_lowercase();
                        // 若引用了已知库命名空间但该命名空间未在任何文件中定义
                        if value_lower.contains(&format!("{}.", ns_lower))
                            && !found_namespaces.contains_key(&ns_lower)
                        {
                            non_existent_libs
                                .entry(known_ns.clone())
                                .or_insert_with(|| ini_file.path.clone());
                        }
                    }
                }
            }
        }
    }

    (duplicate_libs, non_existent_libs)
}

/// 检测路径长度超过指定限制的文件。
///
/// Windows 默认路径长度限制为 260 字符（MAX_PATH），3DMigoto 在处理超长路径时可能失败。
///
/// 参数：
/// - `file_paths`: 文件路径列表。
/// - `max_length`: 最大允许长度（默认 255）。
///
/// 返回：长度超过限制的文件路径列表。
pub fn check_long_paths(file_paths: &[String], max_length: usize) -> Vec<String> {
    file_paths
        .iter()
        .filter(|path| path.len() > max_length)
        .cloned()
        .collect()
}

/// 使用 DirWalker BFS 遍历目录树，匹配扩展名为 `.ini` 或 `.INI` 的文件。
///
/// 不跟随符号链接（follow_symlinks=false），通过 VisitedPathPool 防止循环，
/// 深度限制使用 DirWalker 默认值（DEFAULT_MAX_TRAVERSAL_DEPTH=64）。
///
/// 参数：
/// - `base_path`: 起始目录路径。
///
/// 返回：INI 文件路径列表。路径不存在或非目录时返回空 Vec。
fn collect_ini_files(base_path: &str) -> Vec<String> {
    let base = Path::new(base_path);

    if !base.exists() || !base.is_dir() {
        return Vec::new();
    }

    let entries = DirWalker::new()
        .follow_symlinks(false)
        .file_ext("ini")
        .include_dirs(false)
        .skip_hidden(false)
        .walk_bfs(base);

    entries
        .into_iter()
        .filter_map(|e| e.path.to_str().map(|s| s.to_string()))
        .collect()
}

/// 将错误列表按类型分类到三个映射中。
///
/// 分类规则：
/// - `reason` 以 `"CRASH LINE"` 开头 → `crash_lines`
/// - `reason` 为 `"Missing \"endif\""` → `missing_endif_errors`
/// - 其他 → `other_errors`
///
/// 返回：`(crash_lines, other_errors, missing_endif_errors)`。
#[allow(clippy::type_complexity)]
fn classify_errors(errors: Vec<IniSyntaxError>) -> (
    HashMap<String, Vec<IniSyntaxError>>,
    HashMap<String, Vec<IniSyntaxError>>,
    HashMap<String, Vec<IniSyntaxError>>,
) {
    let mut crash_lines: HashMap<String, Vec<IniSyntaxError>> = HashMap::new();
    let mut other_errors: HashMap<String, Vec<IniSyntaxError>> = HashMap::new();
    let mut missing_endif_errors: HashMap<String, Vec<IniSyntaxError>> = HashMap::new();

    for error in errors {
        let file_path = error.file_path.clone();
        if error.reason.starts_with("CRASH LINE") {
            crash_lines.entry(file_path).or_default().push(error);
        } else if error.reason == "Missing \"endif\"" {
            missing_endif_errors
                .entry(file_path)
                .or_default()
                .push(error);
        } else {
            other_errors.entry(file_path).or_default().push(error);
        }
    }

    (crash_lines, other_errors, missing_endif_errors)
}

/// 对指定基础路径下的所有 INI 文件执行全面错误检测。
///
/// 流程：
/// 1. 递归收集所有 `.ini` 文件路径。
/// 2. 检测路径长度超限的文件。
/// 3. 并行处理每个文件：读取内容 → 检测崩溃行 → 解析 INI → 检测流程控制错误。
/// 4. 汇总所有错误并按类型分类。
/// 5. 检测模组库的重复与引用问题。
///
/// 参数：
/// - `base_path`: 基础目录路径（通常为 `_MANAGED_` 目录）。
/// - `known_lib_namespaces`: 已知的模组库命名空间列表。
///
/// 返回：`ErroredLinesReport` 汇总报告。
pub fn check_all_errors(
    base_path: &str,
    known_lib_namespaces: &[String],
) -> Result<ErroredLinesReport> {
    let file_paths = collect_ini_files(base_path);

    // 检测路径过长的文件
    let long_path_errors = check_long_paths(&file_paths, 255);

    // 并行处理每个 INI 文件
    let results: Vec<Result<(Vec<IniSyntaxError>, IniFile)>> = file_paths
        .par_iter()
        .map(|path| {
            let content = fs::read_to_string(path)
                .with_context(|| format!("Failed to read file: {}", path))?;

            // 检测崩溃行（基于原始文本）
            let crash_errors = detect_crash_lines(&content, path);

            // 解析 INI 并检测流程控制错误
            let ini_file = parse_file(path)?;
            let flow_errors = check_flow_control(&ini_file);

            // 合并该文件的所有错误
            let mut all_errors = crash_errors;
            all_errors.extend(flow_errors);

            Ok((all_errors, ini_file))
        })
        .collect();

    let mut all_errors = Vec::new();
    let mut all_ini_files = Vec::new();

    // 汇总所有文件的结果，处理失败的文件
    for result in results {
        match result {
            Ok((errors, ini_file)) => {
                all_errors.extend(errors);
                all_ini_files.push(ini_file);
            }
            Err(e) => {
                // 单个文件处理失败不影响整体流程，仅记录警告
                log::warn!("Error processing file: {}", e);
            }
        }
    }

    // 按类型分类错误
    let (crash_lines, other_errors, missing_endif_errors) = classify_errors(all_errors);

    // 检测模组库问题
    let (duplicate_libs, non_existent_libs) =
        check_mod_libraries(&all_ini_files, known_lib_namespaces, base_path);

    Ok(ErroredLinesReport {
        duplicate_libs,
        non_existent_libs,
        crash_lines,
        other_errors,
        missing_endif_errors,
        long_path_errors,
    })
}

/// 对单个 INI 文件执行错误检测。
///
/// 与 `check_all_errors` 的区别：仅检测指定文件，不涉及模组库的重复/引用检测。
///
/// 参数：
/// - `path`: INI 文件路径。
///
/// 返回：`ErroredLinesReport`（`duplicate_libs` 与 `non_existent_libs` 为空）。
pub fn check_single_file(path: &str) -> Result<ErroredLinesReport> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read file: {}", path))?;

    let crash_errors = detect_crash_lines(&content, path);

    let ini_file = parse_file(path)?;
    let flow_errors = check_flow_control(&ini_file);

    let mut all_errors = crash_errors;
    all_errors.extend(flow_errors);

    let (crash_lines, other_errors, missing_endif_errors) = classify_errors(all_errors);

    let long_path_errors = check_long_paths(&[path.to_string()], 255);

    Ok(ErroredLinesReport {
        duplicate_libs: HashMap::new(),
        non_existent_libs: HashMap::new(),
        crash_lines,
        other_errors,
        missing_endif_errors,
        long_path_errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flow_control_normal() {
        let content = r#"
[CommandList.Test]
if $x > 0
    y = 1
elif $x < 0
    y = -1
else
    y = 0
endif
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_flow_control_missing_endif() {
        let content = r#"
[CommandList.Test]
if $x > 0
    y = 1
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].reason, "Missing \"endif\"");
    }

    #[test]
    fn test_flow_control_unexpected_endif() {
        let content = r#"
[CommandList.Test]
x = 1
endif
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].reason, "Unexpected \"endif\"");
    }

    #[test]
    fn test_flow_control_unexpected_else() {
        let content = r#"
[CommandList.Test]
else
    x = 1
endif
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert!(errors.iter().any(|e| e.reason == "Unexpected \"else\""));
    }

    #[test]
    fn test_flow_control_unexpected_elif() {
        let content = r#"
[CommandList.Test]
elif $x > 0
    y = 1
endif
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert!(errors.iter().any(|e| e.reason == "Unexpected \"elif\""));
    }

    #[test]
    fn test_flow_control_nested() {
        let content = r#"
[CommandList.Test]
if $x > 0
    if $y > 0
        z = 1
    else
        z = 0
    endif
else
    z = -1
endif
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_flow_control_nested_missing_endif() {
        let content = r#"
[CommandList.Test]
if $x > 0
    if $y > 0
        z = 1
endif
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].reason, "Missing \"endif\"");
    }

    #[test]
    fn test_crash_lines_single_bracket() {
        let content = r#"
[GoodSection]
x = 1

[
y = 2
"#;
        let errors = detect_crash_lines(content, "test.ini");
        assert!(errors.iter().any(|e| e.reason.starts_with("CRASH LINE")));
        assert!(errors.iter().any(|e| e.trimmed_line == "["));
    }

    #[test]
    fn test_crash_lines_empty_section() {
        let content = r#"
[GoodSection]
x = 1

[[]]
y = 2
"#;
        let errors = detect_crash_lines(content, "test.ini");
        assert!(errors.iter().any(|e| e.reason.starts_with("CRASH LINE")));
    }

    #[test]
    fn test_crash_lines_empty_if_condition() {
        let content = r#"
[CommandList.Test]
if 
    x = 1
endif
"#;
        let errors = detect_crash_lines(content, "test.ini");
        assert!(errors
            .iter()
            .any(|e| e.reason == "CRASH LINE: Empty if condition"));
    }

    #[test]
    fn test_crash_lines_empty_elif_condition() {
        let content = r#"
[CommandList.Test]
if $x > 0
    y = 1
elif 
    y = -1
endif
"#;
        let errors = detect_crash_lines(content, "test.ini");
        assert!(errors
            .iter()
            .any(|e| e.reason == "CRASH LINE: Empty elif condition"));
    }

    #[test]
    fn test_empty_condition_in_flow_check() {
        let content = r#"
[CommandList.Test]
if 
    y = 1
endif
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert!(errors.iter().any(|e| e.reason == "Empty condition"));
    }

    #[test]
    fn test_flow_control_else_after_else() {
        let content = r#"
[CommandList.Test]
if $x > 0
    y = 1
else
    y = 0
else
    y = -1
endif
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert!(errors.iter().any(|e| e.reason == "Duplicate \"else\""));
    }

    #[test]
    fn test_flow_control_elif_after_else() {
        let content = r#"
[CommandList.Test]
if $x > 0
    y = 1
else
    y = 0
elif $x < -5
    y = -1
endif
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert!(errors
            .iter()
            .any(|e| e.reason == "Unexpected \"elif\" after \"else\""));
    }

    #[test]
    fn test_long_paths() {
        let short = "short.ini".to_string();
        let long = "a".repeat(260) + ".ini";
        let paths = vec![short.clone(), long.clone()];
        let result = check_long_paths(&paths, 255);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], long);
    }

    #[test]
    fn test_key_section_flow_control() {
        let content = r#"
[Key.Test]
key = VK_F1
if $toggle
    back = 1
else
    back = 0
endif
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_non_flow_section_ignored() {
        let content = r#"
[Constants]
if this is not checked
    x = 1
"#;
        let mut ini_file = parse_content(content).unwrap();
        ini_file.path = "test.ini".to_string();
        let errors = check_flow_control(&ini_file);
        assert!(errors.is_empty());
    }
}
