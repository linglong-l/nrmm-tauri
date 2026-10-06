//! INI 条件与行解析域：条件段判定、条件表达式清洗、键值行解析
//!
//! 从 ini_handler.rs 拆出（2026-09-18 试点 3.2）。
//! 依赖 constants（条件段前缀表）；被 model（parse）与 maintain（条件清洗）消费。

use once_cell::sync::Lazy;
use regex::Regex;

use crate::core::constants;

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
pub(crate) fn is_wrapped_in_matching_parens(expr: &str) -> bool {
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

/// 匹配 `$managed_slot_id == $\modmanageragl\group_<数字>\<token>`
static MANAGER_SLOT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\$managed_slot_id\s*==\s*\$\\modmanageragl\\group_\d+\\[A-Za-z0-9_]+").unwrap()
});
static PAREN_AND_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(\s*&&\s*").unwrap());
static PAREN_OR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(\s*\|\|\s*").unwrap());
static AND_PAREN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*&&\s*\)").unwrap());
static OR_PAREN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*\|\|\s*\)").unwrap());
static EMPTY_PAREN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(\s*\)").unwrap());
// 连接符去重（M3：原先在函数体内联 Regex::new 每次调用重新编译，热路径浪费；
// 迁移到 Lazy 静态，与上方清洗正则同一模式）
static DOUBLE_AND_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"&&\s*&&").unwrap());
static AND_OR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"&&\s*\|\|").unwrap());
static DOUBLE_OR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\|\|\s*\|\|").unwrap());
static OR_AND_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\|\|\s*&&").unwrap());

pub(crate) fn sanitize_condition_expression(expression: &str) -> String {
    // 匹配 `$managed_slot_id == $\modmanageragl\group_<数字>\<token>`，
    // token 可为 active_slot（NRMM）或数字编号（本项目历史注入）。
    if !MANAGER_SLOT_RE.is_match(expression) {
        return expression.to_string();
    }

    let mut expr = MANAGER_SLOT_RE
        .replace_all(expression, "")
        .trim()
        .to_string();

    // "(&& x)" -> "(x)"
    expr = PAREN_AND_RE.replace_all(&expr, "(").trim().to_string();
    // "(|| x)" -> "(x)"
    expr = PAREN_OR_RE.replace_all(&expr, "(").trim().to_string();
    // "(x && )" -> "(x)"
    expr = AND_PAREN_RE.replace_all(&expr, ")").trim().to_string();
    // "(x || )" -> "(x)"
    expr = OR_PAREN_RE.replace_all(&expr, ")").trim().to_string();
    // "()" -> ""
    expr = EMPTY_PAREN_RE.replace_all(&expr, "").trim().to_string();
    while expr.trim_end().ends_with("&&") {
        expr = expr.trim_end()[..expr.trim_end().len() - 2]
            .trim()
            .to_string();
    }
    // 尾部悬空 "||"
    while expr.trim_end().ends_with("||") {
        expr = expr.trim_end()[..expr.trim_end().len() - 2]
            .trim()
            .to_string();
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
    expr = DOUBLE_AND_RE.replace_all(&expr, "&&").trim().to_string();
    // "&& ||" -> "||"
    expr = AND_OR_RE.replace_all(&expr, "||").trim().to_string();
    // "|| ||" -> "||"
    expr = DOUBLE_OR_RE.replace_all(&expr, "||").trim().to_string();
    // "|| &&" -> "&&"
    expr = OR_AND_RE.replace_all(&expr, "&&").trim().to_string();

    // 若管理器表达式整体被外层括号包裹，解包
    if is_wrapped_in_matching_parens(&expr) {
        expr = expr[1..expr.len() - 1].trim().to_string();
    }
    expr
}

pub(crate) fn trim_trailing_whitespace(s: &str) -> &str {
    s.trim_end_matches(|c: char| ['\r', '\n', ' ', '\t'].contains(&c))
}

pub(crate) fn parse_key_value(line: &str) -> Option<(String, String, Option<String>)> {
    let equal_pos = line.find('=')?;
    let key = line[..equal_pos].trim();
    let rest = &line[equal_pos + 1..];

    let (value, comment) = if rest.trim_start().starts_with('"') {
        let trimmed_rest = rest.trim_start();
        let offset = rest.len() - trimmed_rest.len();
        if let Some(close_quote) = trimmed_rest[1..].find('"') {
            let value_part = &rest[..offset + close_quote + 2];
            let after_quote = &rest[offset + close_quote + 2..];
            let comment = after_quote
                .find(';')
                .map(|semi_pos| after_quote[semi_pos + 1..].trim().to_string());
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
            None => (rest.trim().to_string(), None),
        }
    };

    if key.is_empty() {
        None
    } else {
        Some((key.to_string(), value, comment))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
