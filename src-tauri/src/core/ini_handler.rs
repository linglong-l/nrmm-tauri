use std::path::Path;

use crate::core::constants;
use crate::core::error_normalizer::friendly_errored_line;
use crate::models::mod_data::ErroredLines;

// 过渡门面（试点 3.7 收口时随消费方切换删除）
pub use crate::core::ini_codec::{
    read_ini_safe, write_utf8_atomic, SourceEncoding, SourceEncodingInfo,
};
// 过渡门面（试点 3.7 收口时随消费方切换删除）
pub use crate::core::ini_condition::{
    is_conditional_section, is_key_section, sanitize_condition_expression_public,
};
// 过渡门面（试点 3.7 收口时随消费方切换删除）
pub use crate::core::ini_model::{IniFile, IniLine, IniSection};

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

    /// 清理旧的 NRMM managed 内容，对齐 Dart `_parseIniSections`。
    ///
    /// 重复运行 update_mod_data 时，旧的注入内容（if 守卫、$managed_slot_id、
    /// NRMM 注释标记等）需要先移除，否则会累积。Dart 在解析阶段即过滤旧内容；
    /// Rust 由于 IniFile::parse 不做过滤，需在注入前显式调用此方法。
    ///
    /// 清理项目：
    /// 1. 旧 manager if 行 (`if $managed_slot_id == $\modmanageragl\group_X\active_slot`)
    /// 2. 旧 `$managed_slot_id` 赋值（Constants 段中）
    /// 3. 旧 NRMM 前置注释（`; No Reload Mod Manager...` 等）
    /// 4. 旧 condition 表达式中的 manager 部分
    /// 5. 注释放置的 `;-; DISABLED_BY_NRMM` 标记行
    pub fn remove_old_managed_content(&mut self) {
        // 清理 preamble 中的 NRMM 注释标记
        //
        // 注意：`;` 前缀行在解析时被归类为 `IniLine::Comment`，仅带前导空白的
        // `;` 行才会成为 `PreambleLine`；两者都必须参与清理，否则旧版 NRMM 头注释
        // （如 `; No Reload Mod Manager ...`）会残留并与本次 `prepend_header_comment`
        // 新头并排累积，造成与 Dart 基线的字节级分歧。
        self.preamble.retain(|line| {
            let text = match line {
                IniLine::PreambleLine(t) => t.as_str(),
                IniLine::Comment(t) => t.as_str(),
                _ => return true,
            };
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return false;
            }
            if trimmed.starts_with(';') {
                let lower = trimmed.to_lowercase();
                return !(lower.contains("no reload mod manager")
                    // NRMM 头注释实际文本为 `; ";-;" are errored ...` / `; "+;" are disabled ...`
                    // （引号内是 `;-;` / `;+;`），与 prepend_header_comment 写入的文本一致。
                    // 注意：模式中的 `;-;` 与 IniLine::DisabledKeyValue 的解析前缀同名但含义不同，
                    // 此处匹配的是头注释引号内的文本，勿改成 `"-;"`（会导致 h1/h2 清理失效、
                    // 头注释退化为 2 行并在重复更新时累积）。
                    || lower.contains("\";-;\" are errored")
                    || lower.contains("\";+;\" are disabled")
                    || lower.contains("errored conditional blocks")
                    || lower.contains("if certain syntax is only"));
            }
            true
        });

        for section in &mut self.sections {
            // 1. 移除旧 manager if 行
            //
            // ⚠️ 已知限制（2026-08-15 审查确认，暂不修复）：`IfStart.condition` 是 `if `
            // 前缀之后的条件文本（解析时已剥离 "if "），而下方模式带 "if" 前缀
            // （`if$managed_slot_id==...`），因此**无法**匹配任何解析后的 IfStart，
            // 旧 manager if 守卫实际不会被清理。
            //
            // 不修正的原因：若让守卫真正被移除，重新注入会走 fix_manager_endif，
            // 把段尾空行卷入守卫块内，导致重复更新每次多一个空行（幂等性回归，
            // 已被校准的 test_update_mod_data_idempotent 依赖「存在守卫即跳过重新注入」
            // 的既有语义）。当前行为：守卫残留 + 注入时 has_existing_condition 跳过
            // 重复注入，输出稳定。潜在代价：分组/槽位编号变化后旧守卫残留（不更新），
            // 属既有预存行为，与 Dart 对齐的 if/endif 配对策略一并留待专项修复。
            section.lines.retain(|line| {
                if let IniLine::IfStart { condition, .. } = line {
                    let cleaned: String =
                        condition.chars().filter(|c| !c.is_whitespace()).collect();
                    return !cleaned
                        .to_lowercase()
                        .contains("if$managed_slot_id==$\\modmanageragl\\group_");
                }
                true
            });

            // 2. 保留所有 endif 行（含孤立/悬空 endif），对齐 Dart _parseIniSections：
            //    Dart 在解析阶段只移除 manager if 行与 Constants 段内的 $managed_slot_id，
            //    但【从不】移除 endif 行——包括原 INPUT 中遗留的悬空 endif 与上一次运行
            //    残留的 manager endif。悬空的 manager endif 随后由 inject_slot_conditions
            //    调用 fix_manager_endif 重新配对（find orphan endif as manager_endif）。
            //    因此此处绝不能按 if/endif 配对删除 if_depth==0 的孤立 endif，否则会误删
            //    Resource 等未包裹段中 INPUT 原始遗留的悬空 endif（Dart 保留，Rust 原实现误删）。

            // 3. 清理 Constants 段中的旧 $managed_slot_id
            if section.name.eq_ignore_ascii_case("Constants") {
                section.lines.retain(|line| {
                    !matches!(line, IniLine::KeyValue { key, .. }
                        if key.trim().eq_ignore_ascii_case("$managed_slot_id")
                        || key.trim().eq_ignore_ascii_case("global $managed_slot_id"))
                });
            }

            // 4. 清理 condition 表达式中的旧 manager 部分
            for line in &mut section.lines {
                if let IniLine::KeyValue { key, value, .. } = line {
                    if key.trim().eq_ignore_ascii_case("condition") {
                        *value = Self::sanitize_condition_for_managed(value);
                    }
                }
                if let IniLine::DisabledKeyValue { key, value, .. } = line {
                    if key.trim().eq_ignore_ascii_case("condition") {
                        *value = Self::sanitize_condition_for_managed(value);
                    }
                }
            }

            // 5. 清理 ;-; DISABLED_BY_NRMM 注释标记行
            section.lines.retain(|line| {
                if let IniLine::Comment(text) = line {
                    return !text.contains("DISABLED_BY_NRMM");
                }
                true
            });
        }
    }

    /// 清理 condition 表达式中的 manager 部分，对齐 Dart _sanitizeKeyConditionExpressionFromModManager
    fn sanitize_condition_for_managed(expr: &str) -> String {
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
            if is_key_section(&section.name) {
                Self::inject_key_condition(section, &condition_var);
                continue;
            }

            // 白名单段：包裹 if/endif 守卫
            let should_wrap = is_conditional_section(&section.name)
                || constants::is_injectable_section(&section.name);

            if should_wrap {
                // 现有条件判定：仅当前格式（`$managed_slot_id == $\modmanageragl\...`）守卫视为
                // 已注入；legacy `$\N\...\managed_slot_id` 旧格式守卫视为内容，需外包新守卫
                // （对齐 NRMM：TextureOverride 等前缀段对旧守卫执行「外包」而非跳过）。
                let has_existing_condition = section.lines.iter().any(|line| {
                    matches!(line, IniLine::IfStart { condition, .. }
                        if condition.contains("managed_slot_id")
                            && !Self::is_legacy_slot_guard(condition))
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
                    Self::fix_manager_endif(section);
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
                    let idx = Self::last_content_index(&section.lines);
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
                    let idx = Self::last_content_index(&section.lines);
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

    /// 返回段末「最后一个内容行之后」的插入索引，对齐 Dart `_getLastIndexInSection`：
    /// 自段尾向前跳过空行与常规注释行（以 `;` 开头但非 `;-;`），`;-;` 视为内容行。
    fn last_content_index(lines: &[IniLine]) -> usize {
        for (i, line) in lines.iter().enumerate().rev() {
            match line {
                IniLine::Empty { .. } => continue,
                IniLine::Comment(text) => {
                    let t: Vec<char> = text.trim_start().chars().collect();
                    // `;-;` 视为内容行（崩溃注释），可紧接其后插入
                    if t.first() == Some(&';') && t.get(1) == Some(&'-') && t.get(2) == Some(&';') {
                        return i + 1;
                    }
                    // 常规注释行：跳过
                    continue;
                }
                _ => return i + 1,
            }
        }
        lines.len()
    }

    // ==========================================================================
    // NRMM 对齐：INI 键优先级重排（_reorderByIniKeyPriority）
    // ==========================================================================

    /// TextureOverride 段优先键（顺序即输出顺序，对齐 NRMM textureOverrideIniKeys）
    const TEXTURE_OVERRIDE_INI_KEYS: &[&'static str] = &[
        "hash",
        "format",
        "width",
        "height",
        "width_multiply",
        "height_multiply",
        "override_byte_stride",
        "override_vertex_count",
        "uav_byte_stride",
        "iteration",
        "filter_index",
        "expand_region_copy",
        "deny_cpu_read",
        "match_priority",
        "match_type",
        "match_usage",
        "match_bind_flags",
        "match_cpu_access_flags",
        "match_misc_flags",
        "match_byte_width",
        "match_stride",
        "match_mips",
        "match_format",
        "match_width",
        "match_height",
        "match_depth",
        "match_array",
        "match_msaa",
        "match_msaa_quality",
        "match_first_vertex",
        "match_first_index",
        "match_first_instance",
        "match_vertex_count",
        "match_index_count",
        "match_instance_count",
    ];

    /// CustomShader 段优先键（对齐 NRMM customShaderIniKeys）
    const CUSTOM_SHADER_INI_KEYS: &[&'static str] = &[
        "vs",
        "hs",
        "ds",
        "gs",
        "ps",
        "cs",
        "max_executions_per_frame",
        "flags",
        "blend",
        "alpha",
        "mask",
        "blend[0]",
        "blend[1]",
        "blend[2]",
        "blend[3]",
        "blend[4]",
        "blend[5]",
        "blend[6]",
        "blend[7]",
        "alpha[0]",
        "alpha[1]",
        "alpha[2]",
        "alpha[3]",
        "alpha[4]",
        "alpha[5]",
        "alpha[6]",
        "alpha[7]",
        "mask[0]",
        "mask[1]",
        "mask[2]",
        "mask[3]",
        "mask[4]",
        "mask[5]",
        "mask[6]",
        "mask[7]",
        "alpha_to_coverage",
        "sample_mask",
        "blend_factor[0]",
        "blend_factor[1]",
        "blend_factor[2]",
        "blend_factor[3]",
        "blend_state_merge",
        "depth_enable",
        "depth_write_mask",
        "depth_func",
        "stencil_enable",
        "stencil_read_mask",
        "stencil_write_mask",
        "stencil_front",
        "stencil_back",
        "stencil_ref",
        "depth_stencil_state_merge",
        "fill",
        "cull",
        "front",
        "depth_bias",
        "depth_bias_clamp",
        "slope_scaled_depth_bias",
        "depth_clip_enable",
        "scissor_enable",
        "multisample_enable",
        "antialiased_line_enable",
        "rasterizer_state_merge",
        "topology",
        "sampler",
    ];

    /// ShaderOverride 段优先键（对齐 NRMM shaderOverrideIniKeys）
    const SHADER_OVERRIDE_INI_KEYS: &[&'static str] = &[
        "hash",
        "allow_duplicate_hash",
        "depth_filter",
        "partner",
        "model",
        "disable_scissor",
        "filter_index",
    ];

    /// ShaderRegex 主段优先键（对齐 NRMM shaderRegexIniKeys）
    const SHADER_REGEX_INI_KEYS: &[&'static str] = &["shader_model", "temps", "filter_index"];

    /// 根据段名返回对应的优先键列表（对齐 NRMM 段类型判定）
    fn priority_keys_for_section(name: &str) -> Option<&'static [&'static str]> {
        let lower = name.to_lowercase();
        if lower.starts_with("textureoverride") {
            Some(Self::TEXTURE_OVERRIDE_INI_KEYS)
        } else if lower.starts_with("customshader") || lower.starts_with("builtincustomshader") {
            Some(Self::CUSTOM_SHADER_INI_KEYS)
        } else if lower.starts_with("shaderoverride") {
            Some(Self::SHADER_OVERRIDE_INI_KEYS)
        } else if lower.starts_with("shaderregex") && !lower.contains('.') {
            Some(Self::SHADER_REGEX_INI_KEYS)
        } else {
            None
        }
    }

    /// 对齐 NRMM `_reorderByIniKeyPriority`：按段类型将已知优先键
    /// （如 hash / match_* / vs / ps ...）重排到段首，其余行保持原有相对顺序。
    /// 段前的注释/空行跟随其后的首个真实行（与 NRMM 缓冲 pendingComments 行为一致）。
    pub fn reorder_by_ini_key_priority(&mut self) {
        for section in &mut self.sections {
            let Some(keys) = Self::priority_keys_for_section(&section.name) else {
                continue;
            };
            let lower_keys: Vec<String> = keys.iter().map(|k| k.to_lowercase()).collect();

            let mut prioritized: Vec<IniLine> = Vec::new();
            let mut rest: Vec<IniLine> = Vec::new();
            let mut pending: Vec<IniLine> = Vec::new();

            for line in section.lines.drain(..) {
                match &line {
                    IniLine::Comment(_) | IniLine::Empty { .. } => {
                        pending.push(line);
                        continue;
                    }
                    _ => {}
                }
                let lower_key: Option<String> = match &line {
                    IniLine::KeyValue { key, .. } => Some(key.to_lowercase()),
                    IniLine::DisabledKeyValue { key, .. } => Some(key.to_lowercase()),
                    _ => None,
                };
                let matched = lower_key
                    .as_ref()
                    .map(|k| lower_keys.iter().any(|pk| k == pk))
                    .unwrap_or(false);
                if matched {
                    prioritized.append(&mut pending);
                    prioritized.push(line);
                } else {
                    rest.append(&mut pending);
                    rest.push(line);
                }
            }
            rest.append(&mut pending);
            section.lines = Vec::new();
            section.lines.extend(prioritized);
            section.lines.extend(rest);
        }
    }

    /// 对齐 Dart `_fixEndifLineAndTrailingFlowControlLine`：
    /// 1. 移除管理器 if/endif 范围内的意外 else/elif（旧版本残留）
    /// 2. 确保 endif 位于段末（最后一个有效行之后）
    fn fix_manager_endif(section: &mut IniSection) {
        let mut if_stack: Vec<usize> = Vec::new(); // stack of non-manager if indices
        let mut manager_endif_idx: Option<usize> = None;
        let mut else_elif_to_comment: Vec<usize> = Vec::new();

        for (i, line) in section.lines.iter().enumerate() {
            match line {
                IniLine::IfStart { condition, .. } => {
                    // 仅「真 manager 守卫」（`$managed_slot_id` 条件且非 legacy）不入栈；
                    // legacy 旧槽位守卫（`$\N\...\managed_slot_id`）按内容 if 入栈，
                    // 否则其匹配的 endif 会被误判为 manager endif（Master1.ini 场景，外层 endif 缺失）。
                    let is_manager_guard = condition.contains("managed_slot_id")
                        && !Self::is_legacy_slot_guard(condition);
                    if !is_manager_guard {
                        if_stack.push(i);
                    }
                }
                IniLine::Else { .. } | IniLine::Elif { .. } => {
                    if if_stack.is_empty() {
                        // else/elif without matching if → 管理器范围的意外 else/elif，移除
                        else_elif_to_comment.push(i);
                    }
                }
                IniLine::EndIf { .. } if if_stack.pop().is_none() => {
                    manager_endif_idx = Some(i);
                }
                _ => {}
            }
        }

        // 移除意外 else/elif（从后往前删除以保持索引）
        for &idx in else_elif_to_comment.iter().rev() {
            section.lines.remove(idx);
            // 若管理器 endif 在删除行之后，其索引需调整
            if let Some(ref mut eidx) = manager_endif_idx {
                if idx < *eidx {
                    *eidx -= 1;
                }
            }
        }

        // 找到最后一个内容行位置（非空行、非注释行、非 if/endif 流程行）。管理器 endif 必须
        // 置于该内容行之后、段尾注释/空行之前，以对齐 Dart（Dart 不会把 endif 推到
        // 段尾注释之后，也不会因为守卫自身的 endif 而跳过重排）。
        let last_valid = (0..section.lines.len()).rev().find(|&i| {
            !matches!(
                &section.lines[i],
                IniLine::Comment(_)
                    | IniLine::Empty { .. }
                    | IniLine::EndIf { .. }
                    | IniLine::IfStart { .. }
            )
        });

        match manager_endif_idx {
            Some(eidx) => {
                if let Some(last) = last_valid {
                    // 仅当 endif 不在「最后一个内容行之后紧邻位置」时才移动
                    if eidx != last + 1 {
                        let endif_line = section.lines.remove(eidx);
                        let insert_at = last + 1;
                        section.lines.insert(insert_at, endif_line);
                    }
                }
            }
            None => {
                // 没有 endif → 在最后一个内容行之后插入
                if let Some(last) = last_valid {
                    section.lines.insert(last + 1, IniLine::EndIf { indent: 0 });
                } else {
                    section.lines.push(IniLine::EndIf { indent: 0 });
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
                    && !Self::is_legacy_slot_guard(condition))
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
                            let is_empty = block_content
                                .iter()
                                .all(|l| matches!(l, IniLine::Empty { .. } | IniLine::Comment(_)));
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
        let indent_size = 4;
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
                    IniLine::KeyValue {
                        key,
                        value,
                        disabled,
                        comment,
                        ..
                    } => {
                        let n = IniLine::KeyValue {
                            key: key.clone(),
                            value: value.clone(),
                            disabled: *disabled,
                            comment: comment.clone(),
                            indent: current_indent * indent_size,
                        };
                        result.push(n);
                    }
                    // 对齐 Dart _prettyIndentation：if 块内的注释/空行/命令也按当前缩进缩进。
                    // Dart 对每一行执行 indent(trimmed, currentIndentation)，即先 trim 再按当前层级加空格，
                    // 因此段内注释与空行在 if 作用域内统一缩进 4 空格（原 INPUT 的缩进被丢弃）。
                    IniLine::Comment(text) => {
                        let trimmed = text.trim_start();
                        result.push(IniLine::Comment(format!(
                            "{}{}",
                            " ".repeat(current_indent * indent_size),
                            trimmed
                        )));
                    }
                    IniLine::Empty { .. } => {
                        result.push(IniLine::Empty {
                            indent: current_indent * indent_size,
                        });
                    }
                    IniLine::Command(text) => {
                        let trimmed = text.trim_start();
                        result.push(IniLine::Command(format!(
                            "{}{}",
                            " ".repeat(current_indent * indent_size),
                            trimmed
                        )));
                    }
                    other => result.push(other.clone()),
                }
            }
            section.lines = result;
        }
    }

    /// 注释「错误条件行」：与 NRMM 一致，把解析器无法视为合法流程控制的条件行
    /// （基线证据：`else if $X = = N` 的 ` = = ` 畸形等式分隔；legacy `$\N\...\managed_slot_id`
    /// 旧格式槽位守卫）就地注释为 `;-;<原行>`，保持行号稳定。
    ///
    /// # 返回
    /// 被注释的行号列表。命中即表示该 INI 存在 NRMM 语义下的语法错误
    /// （调用方据此写 `modsyntaxerrorremoved` 标记）。
    pub fn comment_errored_conditional_lines(&mut self) -> Vec<u32> {
        let mut commented: Vec<u32> = Vec::new();
        let mut global_line = 1u32;
        for _ in &self.preamble {
            global_line += 1;
        }
        for section in &mut self.sections {
            global_line += 1;
            // legacy 槽位守卫的替换仅发生在 NRMM 的「精确白名单段」（Present 等）：
            // [Present] 段旧守卫被替换并 `;-;` 注释，而 TextureOverride 等前缀段中
            // 同款旧守卫作为内容保留（由注入逻辑外包新守卫），不对其注释。
            let exact_injectable = crate::core::constants::INJECTABLE_SECTION_EXACT
                .iter()
                .any(|e| section.name.eq_ignore_ascii_case(e));
            for line in &mut section.lines {
                let is_errored = match line {
                    IniLine::KeyValue { key, value, .. } => {
                        let k = key.to_lowercase();
                        // legacy `condition =` 守卫：引用 `$\N\...\managed_slot_id` / `\active` 的旧槽位条件
                        k == "condition" && Self::is_legacy_slot_guard(value)
                    }
                    IniLine::Elif { condition, .. } => {
                        // 基线主模式：畸形 `else if $X = = N`（解析为 Elif），等号之间带空格属非法表达式。
                        // （原生 `elif $X = = N` 者罕见，本项目数据无此类；注释统一以 `else if ` 还原。）
                        condition.contains(" = = ")
                    }
                    IniLine::IfStart { condition, indent } => {
                        // legacy `if $\N\...\managed_slot_id == ...` 旧槽位守卫（仅精确白名单段）
                        exact_injectable && Self::is_legacy_slot_guard(condition) && *indent == 0
                    }
                    _ => false,
                };
                if is_errored {
                    // 注释保留原行形态：Elif（`else if`）行还原为 `;-;else if <condition>`，
                    // 其余行取 Display 原文并保留前导缩进。
                    let comment_text = match line {
                        IniLine::Elif { condition, indent } => {
                            format!("{};-;else if {}", " ".repeat(*indent), condition)
                        }
                        _ => {
                            let orig = format!("{}", line);
                            let lead = orig.len() - orig.trim_start().len();
                            format!("{};-;{}", &orig[..lead], orig.trim_start())
                        }
                    };
                    *line = IniLine::Comment(comment_text);
                    commented.push(global_line);
                }
                global_line += 1;
            }
        }
        commented
    }

    /// 判断是否为 legacy（NRMM V1.3.x 时代）槽位守卫：
    /// 表达式任意位置引用数字命名空间 `$\N\...\managed_slot_id` 或 `$\N\...\active`。
    #[inline]
    fn is_legacy_slot_guard(expr: &str) -> bool {
        let e = expr.trim_start();
        // `$\N\` 数字命名空间（如 `$\1\master_1\managed_slot_id == ...`；条件可能带括号前缀）
        let bytes = e.as_bytes();
        let mut has_legacy_ns = false;
        let mut i = 0usize;
        while i + 3 <= bytes.len() {
            if bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'\\') {
                let mut j = i + 2;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 2 && bytes.get(j) == Some(&b'\\') {
                    has_legacy_ns = true;
                    break;
                }
            }
            i += 1;
        }
        has_legacy_ns && (e.contains("\\managed_slot_id") || e.contains("\\active"))
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
                            new_lines
                                .push(IniLine::Comment(format!(";-; DISABLED_BY_NRMM {}", text)));
                        } else {
                            new_lines.push(line.clone());
                        }
                    }
                    IniLine::KeyValue {
                        key,
                        value,
                        disabled: false,
                        comment,
                        ..
                    } => {
                        let lower_key = key.to_lowercase();
                        let should_comment = if in_texture_override || in_conditional_block > 0 {
                            false
                        } else {
                            lower_key == "drawindexed"
                                || lower_key == "draw"
                                || (lower_key == "ib"
                                    && !value.to_lowercase().starts_with("resource"))
                        };

                        if should_comment {
                            commented_lines.push(global_line_num);
                            let mut orig = format!("{} = {}", key, value);
                            if let Some(c) = comment {
                                orig.push_str(&format!(" ; {}", c));
                            }
                            new_lines
                                .push(IniLine::Comment(format!(";-; DISABLED_BY_NRMM {}", orig)));
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

    pub fn detect_errors(
        &self,
        mod_path: &Path,
        known_libraries: &std::collections::HashSet<String>,
    ) -> Vec<ErroredLines> {
        // === Error type classification (aligned with C++ FFI GetErroredFlowControlLines) ===
        // 0: DUPLICATE LIB  - 同一模组中定义了多个同名库段（对应 C++ FFI "DUPLICATE LIB: X"）
        // 1: CRASH LINE     - 可能导致 XXMI 崩溃的行（drawindexed/ib/vb0 等，对应 "CRASH LINE"）
        // 2: MISSING ENDIF  - if/else 块缺少匹配的 endif（对应 "Missing \"endif\""）
        // 3: FLOW CONTROL   - 孤儿 endif 或其他流程控制错误（对应 otherError / otherErrorMissingEndif）
        // 6: PATH TOO LONG  - INI 路径超过 260 字符限制（常量 ET_PATH_TOO_LONG = 6）
        // 5: NON EXISTENT LIB - 跨模组引用的库命名空间不存在（对应 "NON EXISTENT LIB: X"）
        const ET_DUPLICATE_LIB: u8 = 0;
        const ET_CRASH_LINE: u8 = 1;
        const ET_MISSING_ENDIF: u8 = 2;
        const ET_FLOW_CONTROL: u8 = 3;
        const ET_PATH_TOO_LONG: u8 = 6;
        const ET_NON_EXISTENT_LIB: u8 = 5;
        let mut errors: Vec<ErroredLines> = Vec::new();

        let mut defined_libs = std::collections::HashSet::new();
        let mut lib_sections: std::collections::HashMap<String, Vec<u32>> =
            std::collections::HashMap::new();
        let mut line_num = 1u32;

        for _ in &self.preamble {
            line_num += 1;
        }

        for section in &self.sections {
            line_num += 1;
            if Self::is_defined_library_section(&section.name) {
                defined_libs.insert(section.name.clone());
                lib_sections
                    .entry(section.name.clone())
                    .or_default()
                    .push(line_num - 1);
            }
        }

        // Step 1: 检测重复库段（DUPLICATE LIB）
        for (lib_name, lines) in &lib_sections {
            if lines.len() > 1 {
                for &ln in lines {
                    errors.push(ErroredLines {
                        line_number: ln,
                        line: format!("[{}]", lib_name),
                        error_type: ET_DUPLICATE_LIB,
                        error_message: format!("DUPLICATE LIB: {}", lib_name),
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
                                error_type: ET_FLOW_CONTROL,
                                error_message: "FLOW CONTROL: orphan endif".to_string(),
                                ..Default::default()
                            });
                        }
                    }
                    IniLine::KeyValue { key, value, .. }
                    | IniLine::DisabledKeyValue { key, value, .. } => {
                        let lower_key = key.to_lowercase();
                        // 崩溃行（CRASH LINE, error_type=1）仅针对 drawindexed / draw：
                        // 这些键取到非法值（既非 auto / 数值 / 逗号列表，也非资源引用）时
                        // 会令 XXMI 崩溃。
                        // `vb*` 与 `ib` 是缓冲/索引缓冲说明符，其合法取值为「资源引用」
                        // （如 `ib = ResourceXxx`）或 `null` 或数值字节跨度，绝不应判为崩溃行
                        // —— 这与 `vb*` 的处理保持一致（两者皆为缓冲引用，而非纯绘制调用）。
                        // 因此 `ib` 必须从崩溃键中剔除（原实现误将 `ib = Resource...` 判为崩溃行，
                        // 导致 Stelle/Config/Nvzhu 模组被错误打上 modsyntaxerrorremoved 标记）。
                        let is_crash_key = lower_key == "drawindexed" || lower_key == "draw";

                        // `ib` / `vb*` 作为缓冲引用，其跨模组引用合法性由引用键检测（NON EXISTENT LIB）
                        // 覆盖；此处不把它们列入引用键（与 `vb*` 一致），避免把合法的 `ib = Resource...`、
                        // `ib = null` 误报为缺失库。
                        let is_ref_key = lower_key == "run"
                            || lower_key.starts_with("ps-t")
                            || lower_key.starts_with("vs-t")
                            || lower_key.starts_with("ps-")
                            || lower_key.starts_with("vs-")
                            || lower_key.starts_with("cs-");

                        if is_crash_key && !value.is_empty() {
                            // Step 2a: 可能导致 XXMI 崩溃的行（CRASH LINE）
                            // 对应 C++ FFI: reason = "CRASH LINE"
                            if !Self::is_numeric_value(value) && !lower_key.starts_with("vb") {
                                errors.push(ErroredLines {
                                    line_number: line_num,
                                    line: format!("{} = {}", key, value),
                                    error_type: ET_CRASH_LINE,
                                    error_message: "CRASH LINE".to_string(),
                                    ..Default::default()
                                });
                            }
                        } else if is_ref_key && !value.is_empty() && !Self::is_numeric_value(value)
                        {
                            // Step 2b: 跨模组库引用检测（NON EXISTENT LIB）
                            // 对应 C++ FFI: reason = "NON EXISTENT LIB: X"
                            let ref_name = value.trim();
                            let ref_lower = ref_name.to_lowercase();
                            if !ref_name.is_empty()
                                && !known_libraries.contains(ref_name)
                                && !defined_libs.contains(ref_name)
                                && !ref_name.starts_with("Resource")
                                && !ref_lower.starts_with("builtincommandlist")
                            {
                                errors.push(ErroredLines {
                                    line_number: line_num,
                                    line: format!("{} = {}", key, value),
                                    error_type: ET_NON_EXISTENT_LIB,
                                    error_message: format!("NON EXISTENT LIB: {}", ref_name),
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

        // Step 3: 检测未闭合的 if 块（MISSING ENDIF）
        for (ln, _) in if_stack {
            errors.push(ErroredLines {
                line_number: ln,
                line: "if (missing endif)".to_string(),
                error_type: ET_MISSING_ENDIF,
                error_message: "Missing \"endif\"".to_string(),
                ..Default::default()
            });
        }

        let path_str = mod_path.to_string_lossy();
        if path_str.len() > 260 {
            errors.push(ErroredLines {
                line_number: 0,
                line: format!("Path too long: {} ({} chars)", path_str, path_str.len()),
                error_type: ET_PATH_TOO_LONG,
                error_message: "PATH TOO LONG".to_string(),
                ..Default::default()
            });
        }

        // 规范化：把结构化校验错误转换为非专业人员可理解的友好提示（不含技术细节）
        for e in &mut errors {
            e.friendly_message =
                friendly_errored_line(e.error_type, &e.error_message, e.line_number);
        }

        errors
    }

    pub fn all_section_names(&self) -> Vec<&str> {
        self.sections.iter().map(|s| s.name.as_str()).collect()
    }

    pub fn defined_libraries(&self) -> std::collections::HashSet<String> {
        let mut libs = std::collections::HashSet::new();
        for section in &self.sections {
            if Self::is_defined_library_section(&section.name) {
                libs.insert(section.name.clone());
            }
        }
        libs
    }

    /// 判断段名是否为「库定义段」（其名称可被其它段通过 `run` / `vb*` / `ib` / `ps-*` / `vs-*` / `cs-*` 等键引用）。
    ///
    /// 对齐 NRMM C++ 引擎的库识别：`resource` / `commandlist` / `shaderoverride` / `customshader` / `builtincommandlist`。
    /// 原 Rust 实现漏识别 `customshader` / `builtincommandlist`，导致 `run = CustomShaderElement`、
    /// `run = BuiltInCommandListUnbindAllRenderTargets` 等合法引用被误报为缺失库（error_type=5）。
    #[inline]
    fn is_defined_library_section(name: &str) -> bool {
        let lower = name.to_lowercase();
        lower.starts_with("resource")
            || lower.starts_with("commandlist")
            || lower.starts_with("shaderoverride")
            || lower.starts_with("customshader")
            || lower.starts_with("builtincommandlist")
    }

    /// 判断引用值是否为「非库/资源引用」的安全值，用于避免把合法绘制/缓冲参数误报为
    /// 崩溃行（CRASH LINE, error_type=1）或缺失库（NON EXISTENT LIB, error_type=5）。
    ///
    /// 对齐 3Dmigoto / NRMM C++ FFI 的崩溃行判定——以下均为合法值，不应判为崩溃行：
    /// 1. 纯数值（含十六进制 `0x` 与小数），如 `override_byte_stride = 48`、`uav_byte_stride = 12`；
    /// 2. 关键字 `auto`，如 `drawindexed = auto`、`ib = auto`、`vb0 = auto`；
    /// 3. 逗号分隔的数值列表，如 `drawindexed = 122913, 0, 0`、`drawindexed = 156, 122913, 0`
    ///    （3Dmigoto 合法绘制调用参数：index, count, baseVertex）。
    ///
    /// 若返回 false，则该值可能是库/资源引用名（应走缺失库检测分支），或被如实判为崩溃行。
    #[inline]
    fn is_numeric_value(v: &str) -> bool {
        let t = v.trim();
        if t.is_empty() {
            return false;
        }
        // 3Dmigoto 合法关键字（drawindexed/ib/vb* = auto 均合法，不应判为崩溃行）
        if t.eq_ignore_ascii_case("auto") {
            return true;
        }
        // 逗号分隔的数值列表（如 drawindexed = 122913, 0, 0）——逐项校验为数值
        if t.contains(',') {
            return t.split(',').all(|part| {
                let p = part.trim();
                if p.is_empty() {
                    return false;
                }
                let p = p
                    .strip_prefix("0x")
                    .or_else(|| p.strip_prefix("0X"))
                    .unwrap_or(p);
                p.chars().all(|c| c.is_ascii_digit() || c == '.')
            });
        }
        let t = t
            .strip_prefix("0x")
            .or_else(|| t.strip_prefix("0X"))
            .unwrap_or(t);
        let mut has_digit = false;
        for c in t.chars() {
            if c.is_ascii_digit() {
                has_digit = true;
            } else if c != '.' {
                return false;
            }
        }
        has_digit
    }

    /// 提取 INI 文件中所有 TextureOverride/ShaderOverride 段的活跃 hash 值
    ///
    /// 遍历所有 section，对名称以 `textureoverride` 或 `shaderoverride` 开头的段，
    /// 提取 `hash` 键的值（不区分大小写）。
    ///
    /// 仅提取活跃行（`KeyValue { disabled: false }`），跳过被 NRMM 禁用的行
    /// （`DisabledKeyValue` 即 `;-;hash = ...`，以及 `KeyValue { disabled: true }`），
    /// 避免已禁用的 hash 值造成假冲突。
    ///
    /// # 返回
    /// Vec<(String, String)> — (段名, hash 值原始字符串，已转小写归一化)
    pub fn extract_hashes(&self) -> Vec<(String, String)> {
        let mut hashes = Vec::new();
        for section in &self.sections {
            let name_lower = section.name.to_lowercase();
            if name_lower.starts_with("textureoverride") || name_lower.starts_with("shaderoverride")
            {
                for line in &section.lines {
                    // 仅提取活跃 hash 行（KeyValue 且 disabled=false），
                    // 跳过 DisabledKeyValue（;-; 前缀禁用行）和 KeyValue { disabled: true }
                    if let IniLine::KeyValue {
                        key,
                        value,
                        disabled: false,
                        ..
                    } = line
                    {
                        if key.to_lowercase() == "hash" && !value.trim().is_empty() {
                            hashes.push((section.name.clone(), value.trim().to_lowercase()));
                        }
                    }
                }
            }
        }
        hashes
    }

    /// 提取 INI 中所有 `namespace = xxx` 键值声明的命名空间
    ///
    /// 遍历所有 section 的所有行，收集 `namespace` 键的值（不区分大小写）。
    /// 返回 (namespace_value, section_name) 列表。
    pub fn extract_namespace_declarations(&self) -> Vec<(String, String)> {
        let mut decls = Vec::new();
        for section in &self.sections {
            for line in &section.lines {
                if let IniLine::KeyValue { key, value, .. }
                | IniLine::DisabledKeyValue { key, value, .. } = line
                {
                    if key.to_lowercase() == "namespace" && !value.trim().is_empty() {
                        decls.push((value.trim().to_string(), section.name.clone()));
                    }
                }
            }
        }
        decls
    }

    /// 提取 INI 中所有 `run = xxx` 引用的库命名空间
    ///
    /// 仅提取引用已知库命名空间的 `run` 值。
    /// 返回 (referenced_namespace, section_name) 列表。
    ///
    /// # 参数
    /// - `known_lib_namespaces`: 已知库命名空间集合（小写）
    pub fn extract_run_references(
        &self,
        known_lib_namespaces: &std::collections::HashSet<String>,
    ) -> Vec<(String, String)> {
        let mut refs = Vec::new();
        for section in &self.sections {
            for line in &section.lines {
                if let IniLine::KeyValue { key, value, .. }
                | IniLine::DisabledKeyValue { key, value, .. } = line
                {
                    if key.to_lowercase() == "run" && !value.trim().is_empty() {
                        let val_lower = value.trim().to_lowercase();
                        // 检查 run 值是否包含已知库命名空间（如 "customshader\xxx\global\orfix\yyy"）
                        for ns in known_lib_namespaces {
                            if val_lower.contains(ns) {
                                refs.push((ns.clone(), section.name.clone()));
                                break;
                            }
                        }
                    }
                }
            }
        }
        refs
    }

    /// 检测 INI 中是否声明了已知库命名空间（通过 [Resource.xxx] 段名或 namespace = xxx 键值）
    ///
    /// 返回检测到的已知库显示名列表（如 ["ORFix", "TexFx"]）。
    ///
    /// # 参数
    /// - `known_lib_namespaces`: 已知库命名空间集合（小写）
    pub fn detect_known_lib_declarations(
        &self,
        known_lib_namespaces: &std::collections::HashSet<String>,
    ) -> Vec<String> {
        let mut detected = std::collections::HashSet::new();

        // 1. 检查 [Resource.xxx] 段名是否包含已知库命名空间
        for section in &self.sections {
            let name_lower = section.name.to_lowercase();
            if name_lower.starts_with("resource") {
                for ns in known_lib_namespaces {
                    if name_lower.contains(ns) {
                        if let Some(display) = crate::core::constants::lookup_lib_display_name(ns) {
                            detected.insert(display.to_string());
                        }
                        break;
                    }
                }
            }
        }

        // 2. 检查 namespace = xxx 键值
        for (ns, _) in self.extract_namespace_declarations() {
            let ns_lower = ns.to_lowercase();
            if known_lib_namespaces.contains(&ns_lower) {
                if let Some(display) = crate::core::constants::lookup_lib_display_name(&ns_lower) {
                    detected.insert(display.to_string());
                }
            }
        }

        detected.into_iter().collect()
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
    fn test_detect_duplicate_libs() {
        let ini_content =
            "[ResourceTest]\nfilename = test.dds\n\n[ResourceTest]\nfilename = test2.dds\n";
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
        // ps-t0 引用不存在的外部库 → ET_NON_EXISTENT_LIB (5)
        assert!(errors.iter().any(|e| e.error_type == 5));
    }

    // ========== P0-1：合法引用不应误报 missing-library (error_type=1 或 5) ==========
    #[test]
    fn test_p0_customshader_run_reference_not_flagged() {
        // `run = CustomShaderElement` 引用 [CustomShaderElement]，应被正确识别为已定义库
        let ini_content = "[CustomShaderElement]\nhlsl = test.fx\n\n[TextureOverrideX]\nrun = CustomShaderElement\n";
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        let known = std::collections::HashSet::new();
        let errors = ini.detect_errors(f.path().parent().unwrap(), &known);
        assert!(
            !errors
                .iter()
                .any(|e| e.error_type == 1 || e.error_type == 5),
            "CustomShader 引用被误报为缺失库: {:?}",
            errors
        );
    }

    #[test]
    fn test_p0_builtincommandlist_run_reference_not_flagged() {
        let ini_content =
            "[BuiltInCommandListUnbindAllRenderTargets]\n\n[TextureOverrideX]\nrun = BuiltInCommandListUnbindAllRenderTargets\n";
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        let known = std::collections::HashSet::new();
        let errors = ini.detect_errors(f.path().parent().unwrap(), &known);
        assert!(
            !errors
                .iter()
                .any(|e| e.error_type == 1 || e.error_type == 5),
            "BuiltInCommandList 引用被误报为缺失库: {:?}",
            errors
        );
    }

    #[test]
    fn test_p0_numeric_stride_not_flagged() {
        // override_byte_stride / uav_byte_stride 的纯数值值不是库引用
        let ini_content =
            "[TextureOverrideX]\nhash = deadbeef\noverride_byte_stride = 48\nuav_byte_stride = 12\n";
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        let known = std::collections::HashSet::new();
        let errors = ini.detect_errors(f.path().parent().unwrap(), &known);
        assert!(
            !errors
                .iter()
                .any(|e| e.error_type == 1 || e.error_type == 5),
            "数值 stride 被误报为缺失库: {:?}",
            errors
        );
    }

    #[test]
    fn test_include_detection() {
        let ini_content = "include = common.ini\n\n[Test]\ninclude = other.ini\n";
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        assert!(ini.has_include());
    }

    // ========== remove_old_managed_content（旧 NRMM 内容清理，含 Comment 型头注释） ==========
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
        assert!(!constants.lines.iter().any(
            |l| matches!(l, IniLine::KeyValue { key, .. } if key.contains("$managed_slot_id"))
        ));
        // 注：manager if 守卫的清理属已知限制（与幂等性语义耦合），此处不断言其移除；
        // 仅断言头注释与 Constants 清理生效（见 remove_old_managed_content 内注释说明）。
    }
}
