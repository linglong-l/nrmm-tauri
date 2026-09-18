use std::path::Path;

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
}
