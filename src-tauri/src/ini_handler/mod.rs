//! INI 文件处理模块
//!
//! 该模块负责解析与序列化 3DMigoto 风格的 INI 配置文件。
//! 提供对 INI 文件的结构化访问（段、键值对、命名空间），
//! 以及读取（load）/保存（save）的同步与异步封装。
//!
//! 主要特性：
//! - 支持段（`[Section]`）、键值对（`key = value`）和注释（`;` 或 `//`）。
//! - 自动识别段类型（Constants、CommandList、Key、TextureOverride 等）。
//! - 支持命名空间提取（如 `TextureOverride.MyMod` 中的 `MyMod`）。
//! - 自动处理 BOM（字节顺序标记）。
//! - 通过 `IniFileData` 等结构体支持前后端 JSON 序列化。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;

pub mod error_detection;

/// INI 文件中的单行键值对。
///
/// 每一行被解析为 `key = value` 形式，同时保留原始行文本（`raw_line`）
/// 以便在保存时无损还原未修改的内容。
#[derive(Debug, Clone)]
pub struct IniLine {
    /// 键名（等号左侧，已去除空白）。
    pub key: String,
    /// 值（等号右侧，已去除空白）。
    pub value: String,
    /// 原始行文本（保存时使用，避免格式丢失）。
    pub raw_line: String,
    /// 所属命名空间（由段名解析得到，可能为空字符串）。
    pub namespace: String,
    /// 该行在原始文件中的行号（从 0 开始）。
    pub line_index: usize,
}

/// INI 文件中的一个段（section）。
///
/// 对应 INI 文件中 `[SectionName]` 及其后续到下一段之前的所有行。
#[derive(Debug, Clone)]
pub struct IniSection {
    /// 段名（不含方括号）。
    pub name: String,
    /// 该段的命名空间（由段名解析得到，可能为空字符串）。
    pub namespace: String,
    /// 该段内所有可解析的键值对行。
    pub lines: Vec<IniLine>,
    /// 该段头在原始文件中的行号（从 0 开始）。
    pub line_index: usize,
    /// 该段内所有原始行（包括注释、空行、非键值对行），用于写回时格式无损。
    pub raw_lines: Vec<String>,
}

/// 完整的 INI 文件结构。
///
/// 包含文件路径、所有段列表以及文件中出现的所有命名空间列表。
#[derive(Debug, Clone)]
pub struct IniFile {
    /// 文件路径（解析时可为空，写入文件时使用）。
    pub path: String,
    /// 文件中所有段的列表（按出现顺序）。
    pub sections: Vec<IniSection>,
    /// 文件中出现的所有命名空间列表（去重，保留首次出现顺序）。
    pub namespaces: Vec<String>,
}

/// INI 段类型枚举。
///
/// 根据段名前缀识别 3DMigoto 中不同语义的段类型，
/// 用于错误检测与 INI 修改时的流程控制。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionType {
    /// 未知类型（不属于以下任何一种）。
    Unknown,
    /// `[Constants]` 段：常量与全局变量定义。
    Constants,
    /// `[CommandList]` 或 `[CommandListPre]` 段：命令列表。
    CommandList,
    /// `[CommandListPost]` 段：后置命令列表。
    CommandListPost,
    /// `[Key...]` 段：快捷键绑定。
    Key,
    /// `[TextureOverride...]` 段：纹理覆盖。
    TextureOverride,
    /// `[ShaderOverride...]` 段：着色器覆盖。
    ShaderOverride,
    /// `[Resource...]` 段：资源定义。
    Resource,
}

/// Key 段快捷键绑定信息。
///
/// 从 INI 文件中 `[Key.*]` 类型的段提取出的结构化快捷键信息，
/// 包含按键列表、回退键、条件和类型等字段。
///
/// 当前由前端直接解析展示，后端提取函数保留以备后续复用。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct IniKeybind {
    /// 段全名（如 "Key.Toggle"）。
    pub section_name: String,
    /// 命名空间（从段名解析，如 "Toggle"）。
    pub namespace: String,
    /// 所有 `key =` 行的值列表（支持多个 key 绑定）。
    pub keys: Vec<String>,
    /// `back =` 字段值（可选）。
    pub back: Option<String>,
    /// `condition =` 字段值（可选）。
    pub condition: Option<String>,
    /// `type =` 字段值（可选，如 "keypress", "sequence"）。
    pub type_: Option<String>,
    /// 段起始行号（从 0 开始）。
    pub line_index: usize,
}

/// 流程控制关键字类型枚举（用于错误检测中的 if/elif/else/endif 匹配）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowControlType {
    /// `if` 关键字。
    If,
    /// `elif` 或 `else if` 关键字。
    ElseIf,
    /// `else` 关键字。
    Else,
    /// `endif` 关键字。
    EndIf,
}

/// 将段名转换为小写形式（用于不区分大小写的匹配）。
pub fn section_name_to_lower(name: &str) -> String {
    name.to_lowercase()
}

/// 判断一行是否为注释行（以 `;` 或 `//` 开头，忽略前导空白）。
///
/// 注意：`;-;` 前缀是 NRMM 的"解除注释"标记，需要交给后续解析流程处理，
/// 因此不算作普通注释。
pub fn is_comment_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with(";-;") {
        return false;
    }
    trimmed.starts_with(';') || trimmed.starts_with("//")
}

/// 判断一行是否为段头（形如 `[Section]`，忽略首尾空白）。
pub fn is_section_header(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('[') && trimmed.contains(']')
}

/// 从段头行中解析段名。
///
/// 例如 `[TextureOverride.MyMod]` 解析为 `TextureOverride.MyMod`。
/// 若该行不是合法段头则返回 `None`。
pub fn parse_section_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return None;
    }
    let end = trimmed.find(']')?;
    let name = &trimmed[1..end];
    Some(name.trim().to_string())
}

/// 从一行中解析键值对。
///
/// 仅解析形如 `key = value` 的行，注释行、段头行和空行返回 `None`。
/// 等号两侧的空白会被去除。
pub fn parse_key_value(line: &str) -> Option<(String, String)> {
    if is_comment_line(line) || is_section_header(line) {
        return None;
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(eq_pos) = trimmed.find('=') {
        let key = trimmed[..eq_pos].trim().to_string();
        let value = trimmed[eq_pos + 1..].trim().to_string();
        Some((key, value))
    } else {
        None
    }
}

/// 若行以 NRMM 的 `;-;` 解除注释标记开头，则去掉该标记并返回剩余内容。
///
/// 注意返回的是原始行切片，调用方需自行决定是用于解析还是保留原始文本。
fn strip_uncomment_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    if trimmed.starts_with(";-;") {
        trimmed.strip_prefix(";-;").unwrap_or(trimmed).trim_start()
    } else {
        trimmed
    }
}

/// 从段名中检测命名空间。
///
/// 对于带命名空间的段名（如 `TextureOverride.MyMod`），
/// 返回 `.` 之后的部分（`MyMod`）。
/// 不带命名空间的段名（如 `Constants`）返回 `None`。
///
/// 支持的前缀包括：TextureOverride、CommandList、CommandListPost、
/// CommandListPre、ShaderOverride、Resource、Key、CustomShader、
/// ShaderRegex、BuiltInCommandList、BuiltInCustomShader。
pub fn detect_namespace(section_name: &str) -> Option<String> {
    let lower_name = section_name_to_lower(section_name);
    let prefixes = [
        "textureoverride",
        "commandlistpost",
        "commandlistpre",
        "commandlist",
        "shaderoverride",
        "resource",
        "key",
        "customshader",
        "shaderregex",
        "builtincommandlist",
        "builtincustomshader",
    ];

    // 同时支持 `.` 和 `\` 作为命名空间分隔符
    let separator_pos = lower_name.find('.').or_else(|| lower_name.find('\\'));

    if let Some(pos) = separator_pos {
        let prefix_part = &lower_name[..pos];
        for prefix in &prefixes {
            if prefix_part == *prefix {
                let namespace = &section_name[pos + 1..];
                return Some(namespace.to_string());
            }
        }
    }

    None
}

/// 为段名添加命名空间（若该段类型支持命名空间）。
///
/// 例如 `TextureOverrideTest` + 命名空间 `MyMod` → `TextureOverride.MyModTest`。
/// 不支持命名空间的段类型（如 `Constants`）原样返回。
#[allow(dead_code)]
pub fn get_namespaced_section_name(section: &str, namespace: &str) -> String {
    let prefixes = [
        "TextureOverride",
        "CommandList",
        "CommandListPost",
        "CommandListPre",
        "ShaderOverride",
        "Resource",
        "Key",
        "CustomShader",
        "ShaderRegex",
        "BuiltInCommandList",
        "BuiltInCustomShader",
    ];

    for prefix in &prefixes {
        if section.len() >= prefix.len()
            && section[..prefix.len()].eq_ignore_ascii_case(prefix)
        {
            return format!("{}.{}{}", prefix, namespace, &section[prefix.len()..]);
        }
    }

    section.to_string()
}

/// 根据段名识别段类型。
///
/// 先取段名中 `.` 或 `\` 之前的部分（去除命名空间），再根据前缀匹配类型。
/// 例如 `TextureOverride.MyMod` → `TextureOverride`，`CommandListPost.X` → `CommandListPost`，
/// `TextureOverride\MyMod` → `TextureOverride`。
pub fn get_section_type(section_name: &str) -> SectionType {
    let lower = section_name_to_lower(section_name);
    let name = if let Some(dot_pos) = lower.find('.') {
        &lower[..dot_pos]
    } else if let Some(backslash_pos) = lower.find('\\') {
        &lower[..backslash_pos]
    } else {
        lower.as_str()
    };

    if name == "constants" {
        SectionType::Constants
    } else if name == "commandlist" || name == "commandlistpre" {
        SectionType::CommandList
    } else if name == "commandlistpost" {
        SectionType::CommandListPost
    } else if name.starts_with("key") {
        SectionType::Key
    } else if name.starts_with("textureoverride") {
        SectionType::TextureOverride
    } else if name.starts_with("shaderoverride") {
        SectionType::ShaderOverride
    } else if name.starts_with("resource") {
        SectionType::Resource
    } else {
        SectionType::Unknown
    }
}

/// 解析 INI 文本内容为 `IniFile` 结构。
///
/// 流程：
/// 1. 去除 BOM 标记（`\u{FEFF}`）。
/// 2. 逐行遍历：识别段头并切换当前段，识别键值对并归入当前段。
/// 3. 解析完成后提取所有命名空间。
///
/// 注意：段头之前的键值对行会被丢弃（不属于任何段）。
///
/// 参数：
/// - `content`: INI 文件的文本内容。
///
/// 返回：解析后的 `IniFile`（`path` 字段为空，需调用方后续设置）。
pub fn parse_content(content: &str) -> Result<IniFile> {
    // 去除 BOM（UTF-8 字节顺序标记）
    let content = content.strip_prefix("\u{FEFF}").unwrap_or(content);
    
    let mut sections: Vec<IniSection> = Vec::new();
    let mut current_section: Option<IniSection> = None;
    let mut current_namespace = String::new();
    
    for (line_index, line) in content.lines().enumerate() {
        let raw_line = line.to_string();
        let parse_line = strip_uncomment_prefix(line);

        if is_section_header(parse_line) {
            // 遇到新段头时，将上一段存入列表
            if let Some(section) = current_section.take() {
                sections.push(section);
            }

            if let Some(section_name) = parse_section_name(parse_line) {
                // 解析新段的命名空间
                current_namespace = detect_namespace(&section_name).unwrap_or_default();
                current_section = Some(IniSection {
                    name: section_name,
                    namespace: current_namespace.clone(),
                    lines: Vec::new(),
                    line_index,
                    raw_lines: Vec::new(),
                });
            }
        } else if let Some(section) = current_section.as_mut() {
            // 保存所有原始行，确保写回时格式无损
            section.raw_lines.push(raw_line.clone());
            // 仅当处于某个段内时才解析键值对
            if let Some((key, value)) = parse_key_value(parse_line) {
                section.lines.push(IniLine {
                    key,
                    value,
                    raw_line,
                    namespace: current_namespace.clone(),
                    line_index,
                });
            }
        }
    }
    
    // 存入最后一个段
    if let Some(section) = current_section.take() {
        sections.push(section);
    }
    
    let mut ini_file = IniFile {
        path: String::new(),
        sections,
        namespaces: Vec::new(),
    };
    
    // 提取所有命名空间
    ini_file.namespaces = extract_namespaces(&ini_file);
    
    Ok(ini_file)
}

/// 从 `IniFile` 中提取所有命名空间（去重，保留首次出现顺序）。
///
/// 命名空间匹配不区分大小写（例如 `Mod1` 与 `mod1` 视为相同）。
pub fn extract_namespaces(ini_file: &IniFile) -> Vec<String> {
    let mut namespaces = Vec::new();
    for section in &ini_file.sections {
        if let Some(ns) = detect_namespace(&section.name) {
            if !namespaces.iter().any(|existing: &String| existing.eq_ignore_ascii_case(&ns)) {
                namespaces.push(ns);
            }
        }
    }
    namespaces
}

/// 从单个段提取快捷键信息（仅 Key 类型段）。
///
/// 如果该段不是 Key 类型，则返回 `None`。
/// 对于 Key 段，会收集所有 `key =` 行的值，以及可选的
/// `back`、`condition`、`type` 等字段。
///
/// 参数：
/// - `section`: 要提取的 INI 段引用。
///
/// 返回：提取到的快捷键信息，若段类型不是 Key 则为 `None`。
#[allow(dead_code)]
pub fn extract_keybind_from_section(section: &IniSection) -> Option<IniKeybind> {
    if get_section_type(&section.name) != SectionType::Key {
        return None;
    }

    let mut keys = Vec::new();
    let mut back = None;
    let mut condition = None;
    let mut type_ = None;

    for line in &section.lines {
        let key_lower = line.key.to_lowercase();
        match key_lower.as_str() {
            "key" => {
                keys.push(line.value.clone());
            }
            "back" => {
                back = Some(line.value.clone());
            }
            "condition" => {
                condition = Some(line.value.clone());
            }
            "type" => {
                type_ = Some(line.value.clone());
            }
            _ => {}
        }
    }

    Some(IniKeybind {
        section_name: section.name.clone(),
        namespace: section.namespace.clone(),
        keys,
        back,
        condition,
        type_,
        line_index: section.line_index,
    })
}

/// 从整个 INI 文件提取所有 Key 段的快捷键。
///
/// 遍历文件中所有段，对每个 Key 类型的段调用 `extract_keybind_from_section`，
/// 并将结果收集为向量返回。
///
/// 参数：
/// - `ini_file`: 要提取的 INI 文件引用。
///
/// 返回：所有 Key 段的快捷键信息列表（按段出现顺序）。
#[allow(dead_code)]
pub fn extract_keybinds(ini_file: &IniFile) -> Vec<IniKeybind> {
    ini_file
        .sections
        .iter()
        .filter_map(extract_keybind_from_section)
        .collect()
}

/// 从文件路径读取并解析 INI 文件。
///
/// 参数：
/// - `path`: INI 文件路径。
///
/// 返回：解析后的 `IniFile`（`path` 字段已设置为传入的路径）。
pub fn parse_file(path: &str) -> Result<IniFile> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read INI file: {}", path))?;
    
    let mut ini_file = parse_content(&content)?;
    ini_file.path = path.to_string();
    
    Ok(ini_file)
}

/// 将 `IniFile` 结构写回文件。
///
/// 输出格式：每个段头单独一行，其后依次输出段内所有原始行（包括注释、空行等），
/// 保证格式无损。行尾使用 CRLF（`\r\n`），符合 Windows 与 3DMigoto 的惯例。
///
/// 参数：
/// - `ini_file`: 要写入的 INI 文件结构。
/// - `path`: 目标文件路径。
pub fn write_ini_file(ini_file: &IniFile, path: &str) -> Result<()> {
    let mut lines = Vec::new();
    
    for section in &ini_file.sections {
        lines.push(format!("[{}]", section.name));
        for raw_line in &section.raw_lines {
            lines.push(raw_line.clone());
        }
    }
    
    write_lines_to_file(&lines, path)
}

/// 将多行文本写入文件（使用临时文件 + 重命名保证原子性）。
///
/// 流程：
/// 1. 写入临时文件（`<目标>.tmp`）。
/// 2. 刷新并关闭文件句柄。
/// 3. 将临时文件重命名为目标路径。
///
/// 行尾使用 CRLF（`\r\n`）。
///
/// 参数：
/// - `lines`: 要写入的行列表。
/// - `path`: 目标文件路径。
pub fn write_lines_to_file(lines: &[String], path: &str) -> Result<()> {
    let path = Path::new(path);
    let tmp_path = path.with_extension("tmp");
    
    let mut file = fs::File::create(&tmp_path)
        .with_context(|| format!("Failed to create temporary file: {:?}", tmp_path))?;
    
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            file.write_all(b"\r\n")?;
        }
        file.write_all(line.as_bytes())?;
    }
    
    file.flush()?;
    drop(file);
    
    // 原子重命名，避免写入中途崩溃导致文件损坏
    fs::rename(&tmp_path, path)
        .with_context(|| format!("Failed to rename temporary file to {:?}", path))?;
    
    Ok(())
}

/// 检测一行是否为流程控制关键字（if/elif/else/endif）。
///
/// 匹配不区分大小写，支持 `if`、`elif`、`else if`、`else`、`endif`。
/// 非流程控制行返回 `None`。
pub fn detect_flow_control(line: &str) -> Option<FlowControlType> {
    let trimmed = line.trim().to_lowercase();
    if trimmed.starts_with("if ") || trimmed == "if" {
        Some(FlowControlType::If)
    } else if trimmed.starts_with("elif ")
        || trimmed == "elif"
        || trimmed.starts_with("else if ")
        || trimmed == "else if"
    {
        Some(FlowControlType::ElseIf)
    } else if trimmed == "else" {
        Some(FlowControlType::Else)
    } else if trimmed == "endif" {
        Some(FlowControlType::EndIf)
    } else {
        None
    }
}

/// INI 行的可序列化数据结构（用于前后端 JSON 传输）。
///
/// 与 `IniLine` 字段一一对应，但通过 `serde` 支持 camelCase 序列化。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IniLineData {
    /// 键名。
    pub key: String,
    /// 值。
    pub value: String,
    /// 原始行文本。
    pub raw_line: String,
    /// 命名空间。
    pub namespace: String,
    /// 行号。
    pub line_index: usize,
}

/// INI 段的可序列化数据结构（用于前后端 JSON 传输）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IniSectionData {
    /// 段名。
    pub name: String,
    /// 命名空间。
    pub namespace: String,
    /// 段内所有行。
    pub lines: Vec<IniLineData>,
    /// 段头行号。
    pub line_index: usize,
    /// 段内所有原始行（包括注释、空行等），用于写回时格式无损。
    pub raw_lines: Vec<String>,
}

/// INI 文件的可序列化数据结构（用于前后端 JSON 传输）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IniFileData {
    /// 文件路径。
    pub path: String,
    /// 所有段。
    pub sections: Vec<IniSectionData>,
    /// 所有命名空间。
    pub namespaces: Vec<String>,
}

// 以下是内部结构与可序列化结构之间的转换实现。
// 这些转换保证 `IniFile` ↔ `IniFileData` 之间可以无损互转。

impl From<&IniLine> for IniLineData {
    fn from(line: &IniLine) -> Self {
        Self {
            key: line.key.clone(),
            value: line.value.clone(),
            raw_line: line.raw_line.clone(),
            namespace: line.namespace.clone(),
            line_index: line.line_index,
        }
    }
}

impl From<&IniSection> for IniSectionData {
    fn from(section: &IniSection) -> Self {
        Self {
            name: section.name.clone(),
            namespace: section.namespace.clone(),
            lines: section.lines.iter().map(IniLineData::from).collect(),
            line_index: section.line_index,
            raw_lines: section.raw_lines.clone(),
        }
    }
}

impl From<&IniFile> for IniFileData {
    fn from(file: &IniFile) -> Self {
        Self {
            path: file.path.clone(),
            sections: file.sections.iter().map(IniSectionData::from).collect(),
            namespaces: file.namespaces.clone(),
        }
    }
}

impl From<IniFileData> for IniFile {
    fn from(data: IniFileData) -> Self {
        Self {
            path: data.path,
            sections: data
                .sections
                .into_iter()
                .map(|s| IniSection {
                    name: s.name,
                    namespace: s.namespace,
                    lines: s
                        .lines
                        .into_iter()
                        .map(|l| IniLine {
                            key: l.key,
                            value: l.value,
                            raw_line: l.raw_line,
                            namespace: l.namespace,
                            line_index: l.line_index,
                        })
                        .collect(),
                    line_index: s.line_index,
                    raw_lines: s.raw_lines,
                })
                .collect(),
            namespaces: data.namespaces,
        }
    }
}

/// INI 处理器结构体（封装异步加载/保存接口）。
///
/// 该结构体本身无状态，仅作为异步方法的载体。
pub struct IniHandler;

impl IniHandler {
    /// 创建一个新的 `IniHandler` 实例。
    pub fn new() -> Self {
        Self
    }

    /// 同步解析 INI 文件。
    #[allow(dead_code)]
    pub fn parse_file(&self, path: &str) -> Result<IniFile> {
        parse_file(path)
    }

    /// 同步写入 INI 文件。
    #[allow(dead_code)]
    pub fn write_file(&self, ini_file: &IniFile, path: &str) -> Result<()> {
        write_ini_file(ini_file, path)
    }

    /// 异步加载 INI 文件（在阻塞线程中执行文件读取与解析）。
    ///
    /// 参数：
    /// - `path`: INI 文件路径。
    ///
    /// 返回：解析后的 `IniFile`。
    pub async fn load_ini(&self, path: &str) -> Result<IniFile> {
        let path = path.to_string();
        tokio::task::spawn_blocking(move || parse_file(&path))
            .await
            .with_context(|| "Failed to spawn blocking task for loading INI file")?
    }

    /// 异步保存 INI 文件（在阻塞线程中执行文件写入）。
    ///
    /// 参数：
    /// - `ini_file`: 要写入的 INI 文件结构。
    /// - `path`: 目标文件路径。
    pub async fn save_ini(&self, ini_file: &IniFile, path: &str) -> Result<()> {
        let ini_file = ini_file.clone();
        let path = path.to_string();
        tokio::task::spawn_blocking(move || write_ini_file(&ini_file, &path))
            .await
            .with_context(|| "Failed to spawn blocking task for saving INI file")?
    }
}

impl Default for IniHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_comment_line() {
        assert!(is_comment_line("; this is a comment"));
        assert!(is_comment_line("  ; indented comment"));
        assert!(is_comment_line("// another comment"));
        assert!(!is_comment_line("key = value"));
        assert!(!is_comment_line("[section]"));
    }

    #[test]
    fn test_is_section_header() {
        assert!(is_section_header("[Section]"));
        assert!(is_section_header("  [IndentedSection]  "));
        assert!(!is_section_header("key = value"));
        assert!(!is_section_header("; [commented section]"));
    }

    #[test]
    fn test_parse_section_name() {
        assert_eq!(parse_section_name("[Section]"), Some("Section".to_string()));
        assert_eq!(parse_section_name("  [MySection]  "), Some("MySection".to_string()));
        assert_eq!(parse_section_name("[TextureOverride.Test]"), Some("TextureOverride.Test".to_string()));
        assert_eq!(parse_section_name("key = value"), None);
    }

    #[test]
    fn test_parse_key_value() {
        assert_eq!(
            parse_key_value("key = value"),
            Some(("key".to_string(), "value".to_string()))
        );
        assert_eq!(
            parse_key_value("  my_key  =  my value  "),
            Some(("my_key".to_string(), "my value".to_string()))
        );
        assert_eq!(
            parse_key_value("x = 100"),
            Some(("x".to_string(), "100".to_string()))
        );
        assert_eq!(parse_key_value("; comment"), None);
        assert_eq!(parse_key_value("[section]"), None);
        assert_eq!(parse_key_value(""), None);
    }

    #[test]
    fn test_detect_namespace() {
        assert_eq!(
            detect_namespace("TextureOverride.MyMod"),
            Some("MyMod".to_string())
        );
        assert_eq!(
            detect_namespace("textureoverride.mymod"),
            Some("mymod".to_string())
        );
        assert_eq!(
            detect_namespace("ShaderOverride.MyShader"),
            Some("MyShader".to_string())
        );
        assert_eq!(
            detect_namespace("Resource.MyResource"),
            Some("MyResource".to_string())
        );
        assert_eq!(
            detect_namespace("Constants"),
            None
        );
        assert_eq!(
            detect_namespace("TextureOverride"),
            None
        );
    }

    #[test]
    fn test_get_namespaced_section_name() {
        assert_eq!(
            get_namespaced_section_name("TextureOverrideTest", "MyMod"),
            "TextureOverride.MyModTest"
        );
        assert_eq!(
            get_namespaced_section_name("ShaderOverrideVS", "MyMod"),
            "ShaderOverride.MyModVS"
        );
        assert_eq!(
            get_namespaced_section_name("Constants", "MyMod"),
            "Constants"
        );
    }

    #[test]
    fn test_section_name_to_lower() {
        assert_eq!(section_name_to_lower("TextureOverride"), "textureoverride");
        assert_eq!(section_name_to_lower("MySection"), "mysection");
    }

    #[test]
    fn test_get_section_type() {
        assert_eq!(get_section_type("Constants"), SectionType::Constants);
        assert_eq!(get_section_type("CommandList"), SectionType::CommandList);
        assert_eq!(get_section_type("CommandListPre"), SectionType::CommandList);
        assert_eq!(get_section_type("CommandListPost"), SectionType::CommandListPost);
        assert_eq!(get_section_type("Key1"), SectionType::Key);
        assert_eq!(get_section_type("Key"), SectionType::Key);
        assert_eq!(get_section_type("TextureOverrideTest"), SectionType::TextureOverride);
        assert_eq!(get_section_type("ShaderOverrideVS"), SectionType::ShaderOverride);
        assert_eq!(get_section_type("ResourceRes1"), SectionType::Resource);
        assert_eq!(get_section_type("UnknownSection"), SectionType::Unknown);
        assert_eq!(
            get_section_type("TextureOverride.MyMod"),
            SectionType::TextureOverride
        );
    }

    #[test]
    fn test_detect_flow_control() {
        assert_eq!(detect_flow_control("if x > 0"), Some(FlowControlType::If));
        assert_eq!(detect_flow_control("elif x < 0"), Some(FlowControlType::ElseIf));
        assert_eq!(detect_flow_control("else if x < 0"), Some(FlowControlType::ElseIf));
        assert_eq!(detect_flow_control("else"), Some(FlowControlType::Else));
        assert_eq!(detect_flow_control("endif"), Some(FlowControlType::EndIf));
        assert_eq!(detect_flow_control("run = CommandList"), None);
    }

    const TEST_INI_CONTENT: &str = r#"; Test INI file
[Constants]
global persist $myVar = 100

[TextureOverride.TestTex]
hash = 0x12345678
run = CommandList.TestCmd

[CommandList.TestCmd]
if $myVar > 50
    x = 100
else
    x = 0
endif

[Key.Toggle]
key = VK_F1
back = 1
condition = $toggle
"#;

    #[test]
    fn test_parse_content_basic() {
        let ini_file = parse_content(TEST_INI_CONTENT).unwrap();
        
        assert_eq!(ini_file.sections.len(), 4);
        assert_eq!(ini_file.sections[0].name, "Constants");
        assert_eq!(ini_file.sections[1].name, "TextureOverride.TestTex");
        assert_eq!(ini_file.sections[2].name, "CommandList.TestCmd");
        assert_eq!(ini_file.sections[3].name, "Key.Toggle");
        
        assert_eq!(ini_file.sections[0].lines.len(), 1);
        assert_eq!(ini_file.sections[0].lines[0].key, "global persist $myVar");
        assert_eq!(ini_file.sections[0].lines[0].value, "100");
        
        assert_eq!(ini_file.sections[1].lines.len(), 2);
        assert_eq!(ini_file.sections[1].lines[0].key, "hash");
        assert_eq!(ini_file.sections[1].lines[0].value, "0x12345678");
    }

    #[test]
    fn test_extract_namespaces_from_content() {
        let content = r#"
[TextureOverride.Mod1Tex]
hash = 0x11111111

[ShaderOverride.Mod1Shader]
hash = 0x22222222

[TextureOverride.Mod2Tex]
hash = 0x33333333

[Constants]
x = 1
"#;
        let ini_file = parse_content(content).unwrap();
        let namespaces = extract_namespaces(&ini_file);
        
        assert_eq!(namespaces.len(), 3);
        assert!(namespaces.iter().any(|n| n == "Mod1Tex"));
        assert!(namespaces.iter().any(|n| n == "Mod1Shader"));
        assert!(namespaces.iter().any(|n| n == "Mod2Tex"));
    }

    #[test]
    fn test_bom_handling() {
        let content_with_bom = format!("\u{FEFF}{}", TEST_INI_CONTENT);
        let ini_file = parse_content(&content_with_bom).unwrap();
        
        assert_eq!(ini_file.sections.len(), 4);
        assert_eq!(ini_file.sections[0].name, "Constants");
    }

    #[test]
    fn test_section_namespace_field() {
        let ini_file = parse_content(TEST_INI_CONTENT).unwrap();
        
        assert_eq!(ini_file.sections[0].namespace, "");
        assert_eq!(ini_file.sections[1].namespace, "TestTex");
        assert_eq!(ini_file.sections[2].namespace, "TestCmd");
        assert_eq!(ini_file.sections[3].namespace, "Toggle");
    }

    #[test]
    fn test_line_index_tracking() {
        let ini_file = parse_content(TEST_INI_CONTENT).unwrap();
        
        assert_eq!(ini_file.sections[0].line_index, 1);
        assert_eq!(ini_file.sections[1].line_index, 4);
        assert_eq!(ini_file.sections[0].lines[0].line_index, 2);
    }

    #[test]
    fn test_empty_sections() {
        let content = r#"
[EmptySection]

[NextSection]
key = value
"#;
        let ini_file = parse_content(content).unwrap();
        
        assert_eq!(ini_file.sections.len(), 2);
        assert_eq!(ini_file.sections[0].name, "EmptySection");
        assert_eq!(ini_file.sections[0].lines.len(), 0);
        assert_eq!(ini_file.sections[1].lines.len(), 1);
    }

    #[test]
    fn test_extract_keybind_from_section_basic() {
        let content = r#"[Key.Toggle]
key = VK_F1
back = 1
condition = $toggle
type = keypress
"#;
        let ini_file = parse_content(content).unwrap();
        let section = &ini_file.sections[0];
        let keybind = extract_keybind_from_section(section).unwrap();

        assert_eq!(keybind.section_name, "Key.Toggle");
        assert_eq!(keybind.namespace, "Toggle");
        assert_eq!(keybind.keys, vec!["VK_F1"]);
        assert_eq!(keybind.back, Some("1".to_string()));
        assert_eq!(keybind.condition, Some("$toggle".to_string()));
        assert_eq!(keybind.type_, Some("keypress".to_string()));
        assert_eq!(keybind.line_index, 0);
    }

    #[test]
    fn test_extract_keybind_multiple_keys() {
        let content = r#"[Key.MultiBind]
key = VK_F1
key = VK_F2
key = VK_CONTROL + VK_F3
"#;
        let ini_file = parse_content(content).unwrap();
        let section = &ini_file.sections[0];
        let keybind = extract_keybind_from_section(section).unwrap();

        assert_eq!(keybind.keys.len(), 3);
        assert_eq!(keybind.keys[0], "VK_F1");
        assert_eq!(keybind.keys[1], "VK_F2");
        assert_eq!(keybind.keys[2], "VK_CONTROL + VK_F3");
    }

    #[test]
    fn test_extract_keybind_optional_fields_missing() {
        let content = r#"[Key.Simple]
key = VK_SPACE
"#;
        let ini_file = parse_content(content).unwrap();
        let section = &ini_file.sections[0];
        let keybind = extract_keybind_from_section(section).unwrap();

        assert_eq!(keybind.keys, vec!["VK_SPACE"]);
        assert_eq!(keybind.back, None);
        assert_eq!(keybind.condition, None);
        assert_eq!(keybind.type_, None);
    }

    #[test]
    fn test_extract_keybind_non_key_section_returns_none() {
        let content = r#"[Constants]
x = 1
"#;
        let ini_file = parse_content(content).unwrap();
        let section = &ini_file.sections[0];
        let result = extract_keybind_from_section(section);

        assert!(result.is_none());
    }

    #[test]
    fn test_extract_keybinds_multiple_sections() {
        let content = r#"[Constants]
x = 1

[Key.Toggle]
key = VK_F1

[TextureOverride.Test]
hash = 0x1234

[Key.Next]
key = VK_F2
back = 0
"#;
        let ini_file = parse_content(content).unwrap();
        let keybinds = extract_keybinds(&ini_file);

        assert_eq!(keybinds.len(), 2);
        assert_eq!(keybinds[0].section_name, "Key.Toggle");
        assert_eq!(keybinds[0].namespace, "Toggle");
        assert_eq!(keybinds[0].keys, vec!["VK_F1"]);
        assert_eq!(keybinds[1].section_name, "Key.Next");
        assert_eq!(keybinds[1].namespace, "Next");
        assert_eq!(keybinds[1].keys, vec!["VK_F2"]);
        assert_eq!(keybinds[1].back, Some("0".to_string()));
    }

    #[test]
    fn test_extract_keybinds_empty_when_no_key_sections() {
        let content = r#"[Constants]
x = 1

[TextureOverride.Test]
hash = 0x1234
"#;
        let ini_file = parse_content(content).unwrap();
        let keybinds = extract_keybinds(&ini_file);

        assert!(keybinds.is_empty());
    }

    #[test]
    fn test_raw_lines_preserves_comments_and_empty_lines() {
        let content = r#"[Key.Test]
; this is a comment
key = VK_F1

// another comment
back = 1

"#;
        let ini_file = parse_content(content).unwrap();
        let section = &ini_file.sections[0];

        assert_eq!(section.raw_lines.len(), 6);
        assert_eq!(section.raw_lines[0], "; this is a comment");
        assert_eq!(section.raw_lines[1], "key = VK_F1");
        assert_eq!(section.raw_lines[2], "");
        assert_eq!(section.raw_lines[3], "// another comment");
        assert_eq!(section.raw_lines[4], "back = 1");
        assert_eq!(section.raw_lines[5], "");
    }

    #[test]
    fn test_write_ini_file_round_trip() {
        let original = r#"; Header comment
[Constants]
global persist $var = 100

; Key section with comments
[Key.Toggle]
key = VK_F1
; inline comment
back = 1

[TextureOverride.TestTex]
hash = 0x12345678
run = CommandList.Test
"#;
        let ini_file = parse_content(original).unwrap();

        let mut lines = Vec::new();
        for section in &ini_file.sections {
            lines.push(format!("[{}]", section.name));
            for raw_line in &section.raw_lines {
                lines.push(raw_line.clone());
            }
        }

        let reconstructed = lines.join("\n");
        let expected = r#"[Constants]
global persist $var = 100

; Key section with comments
[Key.Toggle]
key = VK_F1
; inline comment
back = 1

[TextureOverride.TestTex]
hash = 0x12345678
run = CommandList.Test"#;

        assert_eq!(reconstructed, expected);
    }

    #[test]
    fn test_round_trip_full_content() {
        let original = r#"; Test INI file
[Constants]
global persist $myVar = 100

[TextureOverride.TestTex]
hash = 0x12345678
run = CommandList.TestCmd

[CommandList.TestCmd]
if $myVar > 50
    x = 100
else
    x = 0
endif

[Key.Toggle]
key = VK_F1
back = 1
condition = $toggle
"#;
        let ini_file = parse_content(original).unwrap();
        let ini_file_2 = parse_content(original).unwrap();

        assert_eq!(ini_file.sections.len(), ini_file_2.sections.len());
        for i in 0..ini_file.sections.len() {
            assert_eq!(ini_file.sections[i].name, ini_file_2.sections[i].name);
            assert_eq!(ini_file.sections[i].raw_lines, ini_file_2.sections[i].raw_lines);
            assert_eq!(ini_file.sections[i].lines.len(), ini_file_2.sections[i].lines.len());
        }
    }

    #[test]
    fn test_keybind_case_insensitive_keys() {
        let content = r#"[Key.Test]
Key = VK_F1
BACK = 1
Condition = $test
Type = keypress
"#;
        let ini_file = parse_content(content).unwrap();
        let keybind = extract_keybind_from_section(&ini_file.sections[0]).unwrap();

        assert_eq!(keybind.keys, vec!["VK_F1"]);
        assert_eq!(keybind.back, Some("1".to_string()));
        assert_eq!(keybind.condition, Some("$test".to_string()));
        assert_eq!(keybind.type_, Some("keypress".to_string()));
    }

    #[test]
    fn test_keybind_namespace_without_dot() {
        let content = r#"[Key1]
key = VK_F1
"#;
        let ini_file = parse_content(content).unwrap();
        let keybind = extract_keybind_from_section(&ini_file.sections[0]).unwrap();

        assert_eq!(keybind.section_name, "Key1");
        assert_eq!(keybind.namespace, "");
    }

    #[test]
    fn test_detect_namespace_backslash() {
        assert_eq!(
            detect_namespace("TextureOverride\\MyMod\\Tex"),
            Some("MyMod\\Tex".to_string())
        );
        assert_eq!(
            detect_namespace("TextureOverride\\MyMod"),
            Some("MyMod".to_string())
        );
        assert_eq!(
            detect_namespace("textureoverride.mymod"),
            Some("mymod".to_string())
        );
        assert_eq!(detect_namespace("Constants"), None);
    }

    #[test]
    fn test_section_type_backslash() {
        assert_eq!(get_section_type("TextureOverride\\MyMod"), SectionType::TextureOverride);
        assert_eq!(get_section_type("Key\\Toggle"), SectionType::Key);
        assert_eq!(get_section_type("CommandListPost\\Post"), SectionType::CommandListPost);
    }

    #[test]
    fn test_parse_content_backslash_namespace() {
        let content = r#"[TextureOverride\MyMod]
hash = 0x1234

[Key\Toggle]
key = VK_F1
"#;
        let ini_file = parse_content(content).unwrap();

        assert_eq!(ini_file.sections.len(), 2);
        assert_eq!(ini_file.sections[0].name, "TextureOverride\\MyMod");
        assert_eq!(ini_file.sections[0].namespace, "MyMod");
        assert_eq!(ini_file.sections[0].lines[0].key, "hash");
        assert_eq!(ini_file.sections[1].name, "Key\\Toggle");
        assert_eq!(ini_file.sections[1].namespace, "Toggle");
    }

    #[test]
    fn test_performance_1000_lines() {
        let mut content = String::with_capacity(20000);
        for i in 0..100 {
            content.push_str(&format!("[Key.Section{}]\n", i));
            for j in 0..8 {
                content.push_str(&format!("key = VK_F{}\n", j));
            }
            content.push_str("back = 1\n");
            content.push_str("condition = $test\n");
        }

        let start = std::time::Instant::now();
        let ini_file = parse_content(&content).unwrap();
        let keybinds = extract_keybinds(&ini_file);
        let elapsed = start.elapsed();

        assert_eq!(ini_file.sections.len(), 100);
        assert_eq!(keybinds.len(), 100);
        assert!(
            elapsed.as_millis() < 50,
            "Parsing 1000 lines took {}ms, expected < 50ms",
            elapsed.as_millis()
        );
    }

    #[test]
    fn test_uncomment_prefix_parsed_as_section() {
        let content = ";-;[TextureOverride.MyMod]\n;-;hash = 1234\n";
        let ini = parse_content(content).unwrap();

        assert_eq!(ini.sections.len(), 1);
        assert_eq!(ini.sections[0].name, "TextureOverride.MyMod");
        assert_eq!(ini.sections[0].lines.len(), 1);
        assert_eq!(ini.sections[0].lines[0].key, "hash");
        assert_eq!(ini.sections[0].lines[0].value, "1234");
        // raw_lines 必须保留原始前缀，写回时不改变格式
        assert_eq!(ini.sections[0].raw_lines[0], ";-;hash = 1234");
    }

    #[test]
    fn test_uncomment_prefix_mixed_with_comments() {
        let content = r#"
[Constants]
;-;hash = 1234
; real comment
x = 1
"#;
        let ini = parse_content(content).unwrap();

        let section = ini.sections.iter().find(|s| s.name == "Constants").unwrap();
        assert_eq!(section.lines.len(), 2);
        assert_eq!(section.lines[0].key, "hash");
        assert_eq!(section.lines[0].value, "1234");
        assert_eq!(section.lines[1].key, "x");
    }
}
