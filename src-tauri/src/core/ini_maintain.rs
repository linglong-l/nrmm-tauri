//! INI 维护域（清理/修复/缩进）：托管内容清除、键序重排、endif 修复、
//! 空块清除、缩进规范、错误条件行与崩溃行注释
//!
//! 从 ini_handler.rs 拆出（2026-09-18 试点 3.5）。
//! `impl IniFile` 跨文件扩展（模型在 ini_model）。

use crate::core::ini_model::{IniFile, IniLine, IniSection};

impl IniFile {
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
                        *value = IniFile::sanitize_condition_for_managed(value);
                    }
                }
                if let IniLine::DisabledKeyValue { key, value, .. } = line {
                    if key.trim().eq_ignore_ascii_case("condition") {
                        *value = IniFile::sanitize_condition_for_managed(value);
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

    /// 返回段末「最后一个内容行之后」的插入索引，对齐 Dart `_getLastIndexInSection`：
    /// 自段尾向前跳过空行与常规注释行（以 `;` 开头但非 `;-;`），`;-;` 视为内容行。
    pub(crate) fn last_content_index(lines: &[IniLine]) -> usize {
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
    pub(crate) fn fix_manager_endif(section: &mut IniSection) {
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
    pub(crate) fn is_legacy_slot_guard(expr: &str) -> bool {
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
}

#[cfg(test)]
#[path = "ini_maintain_tests.rs"]
mod tests;