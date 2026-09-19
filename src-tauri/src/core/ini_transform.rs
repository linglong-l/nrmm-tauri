//! INI 变换域（注入）：槽位条件守卫注入、键条件注入、段属性键保障
//!
//! 从 ini_handler.rs 拆出（2026-09-18 试点 3.4）。
//! `impl IniFile` 跨文件扩展（模型在 ini_model）。

use crate::core::constants;
use crate::core::ini_model::{IniFile, IniLine, IniSection};

impl IniFile {
    /// 段属性键（3Dmigoto 元数据），应始终放在 `if` 守卫之前。
    /// 这类键决定段如何与 draw call 匹配，不参与条件逻辑的主体执行。
    fn is_section_attribute_key(key: &str) -> bool {
        let lower = key.to_lowercase();
        lower == "hash"
            || lower == "match_priority"
            || lower == "match_first_index"
            || lower == "match_type"
            || lower == "allow_duplicate_hash"
            || lower == "match_index_count"
            || lower == "filter"
            || lower == "filter_index"
            || lower == "type"
            || lower == "override_vertex_count"
            || lower == "override_byte_stride"
            || lower == "uav_byte_stride"
            // CustomShader / ShaderOverride 段属性（NRMM 基线：位于 if 守卫之前）
            || lower == "blend"
            || lower.starts_with("blend_factor")
            || lower == "vs"
            || lower == "ps"
            || lower == "cull"
            || lower == "topology"
    }

    fn first_command_line_index(lines: &[IniLine]) -> Option<usize> {
        for (i, line) in lines.iter().enumerate() {
            match line {
                IniLine::Empty { .. } | IniLine::Comment(_) => continue,
                IniLine::KeyValue { key, .. } if Self::is_section_attribute_key(key) => continue,
                _ => return Some(i),
            }
        }
        None
    }

    #[allow(dead_code)]
    fn last_command_line_index(lines: &[IniLine]) -> Option<usize> {
        for (i, line) in lines.iter().enumerate().rev() {
            match line {
                IniLine::Empty { .. } | IniLine::Comment(_) => continue,
                _ => return Some(i),
            }
        }
        None
    }

    /// 清理 condition 表达式中的 manager 部分，对齐 Dart _sanitizeKeyConditionExpressionFromModManager
    pub(crate) fn sanitize_condition_for_managed(expr: &str) -> String {
        let managed_prefix = "$managed_slot_id ==";
        let idx = expr.find(managed_prefix);
        if let Some(pos) = idx {
            // 找到 && 管理器表达式开始位置
            if pos > 0 {
                let before = &expr[..pos];
                // 回退到最后一个 &&
                if let Some(and_pos) = before.rfind("&&") {
                    return before[..and_pos].trim().to_string();
                }
                return before.trim().to_string();
            }
            return String::new(); // 整行都是管理器表达式
        }
        expr.trim().to_string()
    }

    pub fn inject_slot_conditions(&mut self, group_id: u32, mod_index: u32) {
        let condition_var = format!(
            "$managed_slot_id == $\\modmanageragl\\group_{}\\active_slot",
            group_id
        );

        // === 步骤 1: 注入 $managed_slot_id 到 Constants 段 ===
        // 对齐 Dart: lines.insert(0, 'global $managed_slot_id = $modIndex')
        let has_constants = self
            .sections
            .iter()
            .any(|s| s.name.eq_ignore_ascii_case("Constants"));
        if has_constants {
            for section in &mut self.sections {
                if section.name.eq_ignore_ascii_case("Constants") {
                    section.lines.retain(|line| {
                        !matches!(line, IniLine::KeyValue { key, .. } if key.trim().eq_ignore_ascii_case("$managed_slot_id"))
                    });
                    section.lines.insert(
                        0,
                        IniLine::KeyValue {
                            key: "global $managed_slot_id".to_string(),
                            value: mod_index.to_string(),
                            disabled: false,
                            comment: None,
                            indent: 0,
                        },
                    );
                    break;
                }
            }
        } else {
            let constants_section = IniSection {
                name: "Constants".to_string(),
                lines: vec![IniLine::KeyValue {
                    key: "global $managed_slot_id".to_string(),
                    value: mod_index.to_string(),
                    disabled: false,
                    comment: None,
                    indent: 0,
                }],
                is_conditional: false,
            };
            self.sections.insert(0, constants_section);
        }

        // === 步骤 2: 按段逐段处理 ===
        for section in &mut self.sections {
            // Key 段：condition 追加方式（对齐 NRMM）
            if crate::core::ini_condition::is_key_section(&section.name) {
                Self::inject_key_condition(section, &condition_var);
                continue;
            }

            // 白名单段：包裹 if/endif 守卫
            let should_wrap = crate::core::ini_condition::is_conditional_section(&section.name)
                || constants::is_injectable_section(&section.name);

            if should_wrap {
                // 现有条件判定：仅当前格式（`$managed_slot_id == $\modmanageragl\...`）守卫视为
                // 已注入；legacy `$\N\...\managed_slot_id` 旧格式守卫视为内容，需外包新守卫
                // （对齐 NRMM：TextureOverride 等前缀段对旧守卫执行「外包」而非跳过）。
                let has_existing_condition = section.lines.iter().any(|line| {
                    matches!(line, IniLine::IfStart { condition, .. }
                        if condition.contains("managed_slot_id")
                            && !IniFile::is_legacy_slot_guard(condition))
                });

                if has_existing_condition {
                    continue;
                }

                // 对齐 Dart _checkAndModifySections / _parseIniSections：
                // 段级属性行（hash / match_* / blend 系 / override_* 等）保持在顶层、位于 if 守卫之前；
                // if $managed_slot_id 守卫插入在「首个段体行之前最后一个属性行」之后
                // （属性行之间的注释随属性块留在顶层；最后一个属性之后的注释进入守卫内部）、
                // 仅包裹段体；末尾 endif 置于最后一个内容行之后、段尾注释/空行之前。
                let body_start = Self::first_command_line_index(&section.lines);
                let insert_at = if let Some(body) = body_start {
                    let mut last_attr: Option<usize> = None;
                    for (i, l) in section.lines.iter().enumerate().take(body) {
                        if let IniLine::KeyValue { key, .. } = l {
                            if Self::is_section_attribute_key(key) {
                                last_attr = Some(i);
                            }
                        }
                    }
                    // 有属性：守卫插在最后一个属性之后；无属性：守卫插在段首（index 0），
                    // 前置注释随段体进入守卫内部（NRMM 基线：help.ini / 段首 `;hash` 场景）
                    last_attr.map_or(0, |idx| idx + 1)
                } else {
                    0
                };
                let has_body = body_start.is_some();

                if has_body {
                    section.lines.insert(
                        insert_at,
                        IniLine::IfStart {
                            condition: condition_var.clone(),
                            indent: 0,
                        },
                    );

                    // 对齐 Dart _fixEndifLineAndTrailingFlowControlLine：endif 置于段末内容行之后
                    IniFile::fix_manager_endif(section);
                }
            }
        }
    }

    /// 补齐段级属性键（match_priority / allow_duplicate_hash）
    ///
    /// 对齐 Dart `_parseIniSections`：在解析阶段为 TextureOverride / ShaderOverride 段
    /// 补入缺失的关键属性键。插入点为「段末（段尾注释/空行之前）」，随后由
    /// `reorder_by_ini_key_priority` 将其移至优先级位置（hash 之后、段体之前）。
    ///
    /// - TextureOverride：若不存在任意 `match_*` 键，补 `match_priority = 0`
    /// - ShaderOverride：若不存在 `allow_duplicate_hash`，补 `allow_duplicate_hash = true`
    ///
    /// 必须在 `reorder_by_ini_key_priority` **之前**调用（顺序对齐 Dart 管线）。
    pub fn ensure_section_attribute_keys(&mut self) {
        for section in &mut self.sections {
            let lower = section.name.to_lowercase();
            if lower.starts_with("textureoverride") {
                let has_match = section.lines.iter().any(|l| {
                    matches!(l, IniLine::KeyValue { key, .. } if key.to_lowercase().starts_with("match_"))
                });
                if !has_match {
                    let idx = IniFile::last_content_index(&section.lines);
                    section.lines.insert(
                        idx,
                        IniLine::KeyValue {
                            key: "match_priority".to_string(),
                            value: "0".to_string(),
                            disabled: false,
                            comment: None,
                            indent: 0,
                        },
                    );
                }
            } else if lower.starts_with("shaderoverride") {
                let has_adh = section.lines.iter().any(|l| {
                    matches!(l, IniLine::KeyValue { key, .. } if key.to_lowercase().eq_ignore_ascii_case("allow_duplicate_hash"))
                });
                if !has_adh {
                    let idx = IniFile::last_content_index(&section.lines);
                    section.lines.insert(
                        idx,
                        IniLine::KeyValue {
                            key: "allow_duplicate_hash".to_string(),
                            value: "true".to_string(),
                            disabled: false,
                            comment: None,
                            indent: 0,
                        },
                    );
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
        // 若已用当前格式 if 包裹（含 managed_slot_id 的 IfStart），保持幂等，跳过注入；
        // legacy `$\N\...\managed_slot_id` 旧格式守卫不视为已注入（对齐 NRMM 语义）。
        if section.lines.iter().any(|l| {
            matches!(l, IniLine::IfStart { condition, .. }
                if condition.contains("managed_slot_id")
                    && !IniFile::is_legacy_slot_guard(condition))
        }) {
            return;
        }

        // 阶段1：清理所有 condition/disabled condition 行的旧管理器表达式
        // （同时记录原行型与用户注释，重写时保留 `;-;` 禁用行型并还原注释）
        struct CleanedCond {
            index: usize,
            disabled: bool,
            value: String,
            comment: Option<String>,
            was_disabled_kv: bool,
        }
        let mut cleaned: Vec<CleanedCond> = Vec::new();
        for (i, line) in section.lines.iter().enumerate() {
            let (key, value, disabled, comment, was_disabled_kv) = match line {
                IniLine::KeyValue {
                    key,
                    value,
                    disabled,
                    comment,
                    ..
                } => (key, value, *disabled, comment.clone(), false),
                IniLine::DisabledKeyValue {
                    key,
                    value,
                    comment,
                    ..
                } => (key, value, true, comment.clone(), true),
                _ => continue,
            };
            if key.eq_ignore_ascii_case("condition") {
                cleaned.push(CleanedCond {
                    index: i,
                    disabled,
                    value: crate::core::ini_condition::sanitize_condition_expression(value),
                    comment,
                    was_disabled_kv,
                });
            }
        }

        if cleaned.is_empty() {
            // 无 condition 行，插入新条件
            section.lines.insert(
                0,
                IniLine::KeyValue {
                    key: "condition".to_string(),
                    value: condition_var.to_string(),
                    disabled: false,
                    comment: None,
                    indent: 0,
                },
            );
        } else {
            // 有 condition 行，逐条追加（清理后为空则直接替换为管理器表达式）
            for cond in &cleaned {
                let rhs = cond.value.trim();
                let new_value = if rhs.is_empty() {
                    condition_var.to_string()
                } else if crate::core::ini_condition::is_wrapped_in_matching_parens(rhs) {
                    format!("{} && {}", rhs, condition_var)
                } else {
                    format!("({}) && {}", rhs, condition_var)
                };
                // 按原行型重建：DisabledKeyValue 保留 `;-;` 禁用前缀，并还原用户注释
                let new_line = if cond.was_disabled_kv {
                    IniLine::DisabledKeyValue {
                        key: "condition".to_string(),
                        value: new_value,
                        comment: cond.comment.clone(),
                    }
                } else {
                    IniLine::KeyValue {
                        key: "condition".to_string(),
                        value: new_value,
                        disabled: cond.disabled,
                        comment: cond.comment.clone(),
                        indent: 0,
                    }
                };
                section.lines[cond.index] = new_line;
            }
        }
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
    fn test_inject_slot_conditions() {
        let ini_content = "[Constants]\n$active = 0\n$other = hello\n\n[TextureOverrideTest]\nhash = 0x123\nps-t0 = Res\ndrawindexed\n\n[Present]\nx = 1\n";
        let f = write_temp_ini(ini_content);
        let mut ini = IniFile::parse(f.path()).unwrap();
        // 对齐 Dart 管线：ensure → reorder → inject
        ini.ensure_section_attribute_keys();
        ini.reorder_by_ini_key_priority();
        ini.inject_slot_conditions(1, 2);

        // TextureOverride 段：对齐 NRMM Dart 基线——属性行（hash / match_priority）保持在
        // 顶层、位于 if 守卫之前；守卫插入在属性之后、段体（首个命令行）之前（仅包裹段体）。
        let to_lines = &ini.sections[1].lines;
        assert!(
            matches!(to_lines[0], IniLine::KeyValue { ref key, .. } if key == "hash"),
            "属性行 hash 应位于段顶、不被守卫包裹"
        );
        let guard_idx = to_lines
            .iter()
            .position(|l| matches!(l, IniLine::IfStart { ref condition, .. } if condition.contains("active_slot")))
            .expect("if 守卫应存在");
        assert!(
            guard_idx >= 2,
            "if 守卫应插入在 hash / match_priority 等属性行之后"
        );
        assert!(to_lines.iter().any(|l| matches!(l, IniLine::EndIf { .. })));
        // TextureOverride 段无 match_* 键时补 match_priority = 0（位于段内、守卫之前）
        assert!(to_lines.iter().any(|l| matches!(l, IniLine::KeyValue { ref key, ref value, .. } if key == "match_priority" && value == "0")));
        assert!(!to_lines.iter().any(
            |l| matches!(l, IniLine::KeyValue { ref key, .. } if key == "allow_duplicate_hash")
        ));

        // Constants 段：注入了 $managed_slot_id = 2，且 NRMM 不包裹
        let constants_lines = &ini.sections[0].lines;
        assert!(constants_lines.iter().any(|l| matches!(l, IniLine::KeyValue { ref key, ref value, .. } if key.contains("$managed_slot_id") && value == "2")));
        assert!(!constants_lines
            .iter()
            .any(|l| matches!(l, IniLine::IfStart { .. })));
        assert!(!constants_lines
            .iter()
            .any(|l| matches!(l, IniLine::EndIf { .. })));
    }

    #[test]
    fn test_inject_slot_conditions_present_trailing_comment_after_endif() {
        // Master1.ini 基线场景：[Present] 段含 legacy 旧守卫（注释化）+ 紧贴段尾的注释行，
        // 注入后 endif 应置于最后内容行之后、段尾注释之前（注释不被守卫包裹）。
        let ini_content = "[Present]\n\
             if $\\1\\master_1\\managed_slot_id == $\\modmanageragl\\group_3\\active_slot\n\
             post $active = 0\n\
             endif\n\
             ; Overrides ---------------------------\n\
             \n\
             [TextureOverride1Position]\n\
             match_priority = 0\n\
             if $\\1\\master_1\\managed_slot_id == $\\modmanageragl\\group_3\\active_slot\n\
             $\\1\\master_1\\active = 1\n\
             endif\n\
             \n\
             \n\
             ; .ini 文件由 WWMI 合并脚本生成\n\
             ; 如果你遇到任何问题\n\
             ;.防倒卖：A.A\n";
        let f = write_temp_ini(ini_content.trim_start());
        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.comment_errored_conditional_lines();
        ini.remove_old_managed_content();
        ini.ensure_section_attribute_keys();
        ini.reorder_by_ini_key_priority();
        ini.inject_slot_conditions(3, 9);
        ini.remove_empty_if_blocks();
        ini.apply_indentation();
        for s in &ini.sections {
            println!(
                "S[{}]: {:?}",
                s.name,
                s.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>()
            );
        }
        let present = ini
            .sections
            .iter()
            .find(|s| s.name == "Present")
            .expect("存在 [Present] 段");
        let rendered: Vec<String> = present.lines.iter().map(|l| l.to_string()).collect();
        let (post_idx, endif_idx, comment_idx) = (
            rendered
                .iter()
                .position(|l| l.trim_start() == "post $active = 0"),
            rendered.iter().position(|l| l.trim_start() == "endif"),
            rendered
                .iter()
                .position(|l| l.trim_start().starts_with("; Overrides")),
        );
        println!("present lines: {:?}", rendered);
        assert!(post_idx < endif_idx, "post 应在 endif 之前: {:?}", rendered);
        assert!(
            comment_idx > endif_idx,
            "段尾注释应位于 endif 之后（不被守卫包裹）: {:?}",
            rendered
        );
    }

    /// Master1.ini [TextureOverride1Position] 基线场景：TextureOverride 前缀段含 legacy 旧守卫
    /// （内容保留）+ 注入外层守卫。外层守卫必须获得配对 endif，且置于最后内容行之后、
    /// footer 注释之前（footer 不被守卫包裹、不缩进）。
    #[test]
    fn test_inject_slot_conditions_legacy_guard_gets_outer_endif() {
        let ini_content = "[TextureOverride1Position]\n\
             match_priority = 0\n\
             if $\\1\\master_1\\managed_slot_id == $\\modmanageragl\\group_3\\active_slot\n\
             $\\1\\master_1\\active = 1\n\
             endif\n\
             \n\
             \n\
             ; .ini 文件由 WWMI 合并脚本生成\n\
             ; 如果你遇到任何问题\n\
             ;.防倒卖：A.A\n";
        let f = write_temp_ini(ini_content.trim_start());
        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.comment_errored_conditional_lines();
        ini.remove_old_managed_content();
        ini.ensure_section_attribute_keys();
        ini.reorder_by_ini_key_priority();
        ini.inject_slot_conditions(3, 9);
        ini.remove_empty_if_blocks();
        ini.apply_indentation();

        let to = ini
            .sections
            .iter()
            .find(|s| s.name == "TextureOverride1Position")
            .expect("存在 [TextureOverride1Position] 段");
        let rendered: Vec<String> = to.lines.iter().map(|l| l.to_string()).collect();
        println!("textureoverride lines: {:?}", rendered);

        // 断言 1：if（注入守卫 + legacy 守卫）与 endif 数量相等 —— 外层守卫有配对 endif
        let if_count = rendered
            .iter()
            .filter(|l| l.trim_start().starts_with("if "))
            .count();
        let endif_count = rendered
            .iter()
            .filter(|l| l.trim_start() == "endif")
            .count();
        assert_eq!(
            if_count, endif_count,
            "if/endif 数量不平衡（缺外层 endif）: {:?}",
            rendered
        );

        // 断言 2：footer 注释位于最后一个 endif 之后（未被守卫包裹）
        let footer_idx = rendered
            .iter()
            .position(|l| l.trim_start().starts_with("; .ini 文件由 WWMI"))
            .expect("存在 footer 注释");
        let last_endif_idx = rendered
            .iter()
            .rposition(|l| l.trim_start() == "endif")
            .expect("存在 endif");
        assert!(
            footer_idx > last_endif_idx,
            "footer 注释应在最后一个 endif 之后: {:?}",
            rendered
        );

        // 断言 3：footer 注释无前导缩进（apply_indentation 后 current_indent 已归零）
        assert!(
            !rendered[footer_idx].starts_with(' '),
            "footer 注释不应被缩进: {:?}",
            rendered
        );
    }

    #[test]
    fn test_inject_slot_conditions_match_first_index_skips_match_priority() {
        // TextureOverride 段已含 match_first_index 时，不应再插入 match_priority
        let ini_content =
            "[TextureOverrideHasMatch]\nhash = 0x1\nmatch_first_index = 0\nib = ResourceIB\n";
        let f = write_temp_ini(ini_content);
        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.ensure_section_attribute_keys();
        ini.inject_slot_conditions(1, 0);

        // 无 [Constants] 段时，注入逻辑会在 index 0 创建；TextureOverride 段被推到 index 1
        let to_lines = &ini.sections[1].lines;
        assert!(to_lines.iter().any(|l| matches!(l, IniLine::IfStart { ref condition, .. } if condition.contains("active_slot"))));
        assert!(
            !to_lines
                .iter()
                .any(|l| matches!(l, IniLine::KeyValue { ref key, .. } if key == "match_priority")),
            "已有 match_first_index 时不应插入 match_priority"
        );
    }

    #[test]
    fn test_inject_slot_conditions_all_attribute_section_gets_match_priority() {
        // 全属性段（如 Draw：override_vertex_count 等）不包裹 if，但仍补 match_priority = 0
        let ini_content = "[TextureOverrideDraw]\nhash = 0x1\noverride_vertex_count = 100\noverride_byte_stride = 56\nuav_byte_stride = 4\n";
        let f = write_temp_ini(ini_content);
        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.ensure_section_attribute_keys();
        ini.inject_slot_conditions(1, 0);

        // 无 [Constants] → index 0 创建，段推到 index 1
        let to_lines = &ini.sections[1].lines;
        // 不产生 if 包裹（全属性段无 body 行）
        assert!(!to_lines
            .iter()
            .any(|l| matches!(l, IniLine::IfStart { .. })));
        // 补 match_priority = 0
        assert!(to_lines.iter().any(|l| matches!(l, IniLine::KeyValue { ref key, ref value, .. } if key == "match_priority" && value == "0")));
    }

    #[test]
    fn test_inject_key_condition_no_existing() {
        // Key 段无 condition 行时，应插入一条 condition = manager_expr
        let ini_content = "[KeyDefault]\nkey = a\n";
        let f = write_temp_ini(ini_content);
        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.inject_slot_conditions(1, 0);

        // 注入 [Constants] 段在 index 0，Key 段被推到 index 1
        let lines = &ini.sections[1].lines;
        assert!(
            matches!(lines[0], IniLine::KeyValue { ref key, ref value, .. }
            if key.eq_ignore_ascii_case("condition") && value.contains("$managed_slot_id"))
        );
        // 不应产生 if 包裹
        assert!(!lines.iter().any(|l| matches!(l, IniLine::IfStart { .. })));
    }

    #[test]
    fn test_inject_key_condition_append() {
        // Key 段已有 condition 行时，应追加 && manager_expr
        let ini_content = "[KeyDefault]\ncondition = $Key1 == 1\n";
        let f = write_temp_ini(ini_content);
        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.inject_slot_conditions(1, 0);

        // 注入 [Constants] 段在 index 0，Key 段被推到 index 1
        let lines = &ini.sections[1].lines;
        let cond = lines.iter().find_map(|l| match l {
            IniLine::KeyValue { key, value, .. } if key.eq_ignore_ascii_case("condition") => {
                Some(value.clone())
            }
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
        ini.inject_slot_conditions(1, 0);
        ini.inject_slot_conditions(1, 0);
        ini.inject_slot_conditions(1, 0);

        // 注入 [Constants] 段在 index 0，Key 段被推到 index 1
        let lines = &ini.sections[1].lines;
        let cond = lines.iter().find_map(|l| match l {
            IniLine::KeyValue { key, value, .. } if key.eq_ignore_ascii_case("condition") => {
                Some(value.clone())
            }
            _ => None,
        });
        let cond = cond.expect("condition line should exist");
        // 仅应出现一次 $managed_slot_id
        assert_eq!(cond.matches("$managed_slot_id").count(), 1);
    }

    #[test]
    fn test_inject_key_condition_preserves_comment_and_disabled_line() {
        // 重写 condition 行时：KeyValue 保留用户注释；DisabledKeyValue（`;-;` 行）保留
        // 禁用行型、注释，并清理旧管理器表达式后追加新条件。
        let ini_content = r#"[KeyDefault]
condition = $Key1 == 1  ; 用户注释
;-;condition = $managed_slot_id == $\modmanageragl\group_1\active_slot  ; 禁用注释
"#;
        let f = write_temp_ini(ini_content);
        let mut ini = IniFile::parse(f.path()).unwrap();
        ini.inject_slot_conditions(1, 0);

        // 注入 [Constants] 段在 index 0，Key 段被推到 index 1
        let lines = &ini.sections[1].lines;

        // 活跃 condition 行：保留注释并追加管理器表达式
        let active = lines.iter().find_map(|l| match l {
            IniLine::KeyValue {
                key,
                value,
                comment,
                ..
            } if key.eq_ignore_ascii_case("condition") => Some((value.clone(), comment.clone())),
            _ => None,
        });
        let (active_val, active_comment) = active.expect("active condition line should exist");
        assert_eq!(active_comment.as_deref(), Some("用户注释"));
        assert!(active_val.contains("$Key1 == 1"));
        assert!(active_val.contains("$managed_slot_id"));

        // 禁用 condition 行（DisabledKeyValue）：行型保留、注释保留、旧管理器表达式被清理（不嵌套）
        let disabled = lines
            .iter()
            .filter_map(|l| match l {
                IniLine::DisabledKeyValue {
                    key,
                    value,
                    comment,
                } if key.eq_ignore_ascii_case("condition") => {
                    Some((value.clone(), comment.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            disabled.len(),
            1,
            "should keep exactly one disabled condition line"
        );
        let (dis_val, dis_comment) = &disabled[0];
        assert_eq!(dis_comment.as_deref(), Some("禁用注释"));
        // 清理后为空 → 直接替换为新注入的单一管理器表达式（无嵌套累积）
        assert_eq!(
            dis_val,
            "$managed_slot_id == $\\modmanageragl\\group_1\\active_slot"
        );
    }
}
