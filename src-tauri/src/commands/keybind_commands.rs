//! 按键切换（Keybind Simulation）Tauri 命令
//!
//! 对齐 NRMM 原版实现：
//! - 读取：`keybinds_tab.dart` 的 `loadKeys`（`[KeyXXX]` 段 + `key =` / `back =` + `;+;` 禁用前缀）
//! - 写回：`keybinds_tab.dart` 的 `saveKeys`（原位改写段头 / key 行 / 段体其他行的 `;+;`）
//! - 模拟：`keypress_simulator_manager.dart` 的 `simulateKeysFromKeySections`（`core::key_combo`）
//! - 发现 INI：`mod_manager.dart` 的 `findIniFilesRecursiveExcludeDisabled`
//!   （递归收集 `.ini`，相对路径任何段以 `disabled` 开头（大小写不敏感）则排除，排除 `desktop.ini`）
//!
//! 注意：后端 `IniFile::parse` 只识别 `;-;`，`;+;` 会落入注释分支被丢弃。
//! 因此本模块统一用 `IniFile::force_read_as_utf8` 读原始行 + NRMM 正则解析（最忠实）。

use crate::core::ini_handler::IniFile;
use crate::core::key_combo;
use crate::models::{KeybindEntry, KeybindFile, KeybindSection, SaveKeybindResult};
use crate::platform::get_key_simulator;
use crate::utils::atomic_write;
use regex::Regex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// 匹配 `[KeyXXX]` 段（捕获 XXX 作为段别名，大小写不敏感；NRMM loadKeys 等价）
static KEY_SECTION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*\[Key([^\]\r\n]*)").expect("valid key section regex"));
/// 匹配 `key = <combo>`（主键）
static KEY_VALUE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*key\s*=\s*(.+)").expect("valid key value regex"));
/// 匹配 `back = <combo>`（反向键）
static BACK_VALUE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*back\s*=\s*(.+)").expect("valid back value regex"));

/// Windows 长路径支持：为路径加 `\\?\` 前缀（对齐 NRMM `findIniFilesRecursiveExcludeDisabled`
/// 对 `\\?\` 的 workaround）。Rust std::fs 直接访问超过 MAX_PATH（260）的路径会失败，
/// 加前缀后 Windows API 接受长路径。非 Windows 平台原样返回。
#[cfg(target_os = "windows")]
fn ext_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s.starts_with(r"\\?\") {
        return p.to_path_buf();
    }
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let abs_s = abs.to_string_lossy();
    if abs_s.starts_with(r"\\") {
        // UNC 路径：\\server\share → \\?\UNC\server\share
        PathBuf::from(format!(r"\\?\UNC\{}", abs_s.trim_start_matches(r"\\")))
    } else {
        PathBuf::from(format!(r"\\?\{abs_s}"))
    }
}

#[cfg(not(target_os = "windows"))]
fn ext_path(p: &Path) -> PathBuf {
    p.to_path_buf()
}

/// 还原 `\\?\` 前缀（Windows），返回用户可读的原始路径（非 Windows 原样返回）
#[cfg(target_os = "windows")]
fn deext_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

#[cfg(not(target_os = "windows"))]
fn deext_path(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

/// 递归收集目录下所有 `.ini` 文件（对齐 NRMM `findIniFilesRecursiveExcludeDisabled`）
///
/// - **root 自身（depth=0）永远保留**：mod 目录名可能带 `DISABLED` 前缀（禁用模组），
///   NRMM 的 disabled 检查基于 `p.relative(path, from: modPath)`——从 mod 根内部的相对路径，
///   不检查 mod 根目录名本身；禁用模组同样可读取按键
/// - 相对路径任何段（目录或文件名）以 `disabled` 开头（大小写不敏感）→ 排除
/// - 排除 `desktop.ini`（大小写不敏感）
/// - 遍历使用原始路径（walkdir 对 `\\?\` 前缀不兼容）；**读取/写入**时由调用方
///   对每个文件路径再走 `ext_path`（Windows 长路径支持）
fn collect_ini_files(root: &Path) -> Vec<PathBuf> {
    let mut result: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // root 自身始终保留（其名可能为 DISABLED...，属禁用模组而非内部禁用段）
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            let lower = name.to_lowercase();
            // NRMM 语义：mod 根内相对路径任何段以 disabled 开头即排除（目录整棵跳过，文件单独跳过）
            !lower.starts_with("disabled")
        })
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            let name = entry.file_name().to_string_lossy();
            let is_ini =
                name.to_lowercase().ends_with(".ini") && !name.eq_ignore_ascii_case("desktop.ini");
            if is_ini {
                result.push(entry.path().to_path_buf());
            }
        }
    }
    result.sort();
    result
}

/// 解析单个 INI 文件的按键段（对齐 NRMM `loadKeys`）
///
/// `raw_lines` 为按 `\n` 切分的原始行（可能带 `\r` 行尾；解析前剥离行尾 `\r`）。
/// 仅保留「含至少一个 key」的段；段头带 `;+;` 前缀 → `disabled=true`。
fn parse_key_sections(raw_lines: &[String]) -> Vec<KeybindSection> {
    let mut result: Vec<KeybindSection> = Vec::new();
    let mut current_section: Option<String> = None;
    let mut section_line_index: Option<usize> = None;
    let mut current_keys: Vec<KeybindEntry> = Vec::new();
    let mut current_section_is_disabled = false;

    for (i, raw_line) in raw_lines.iter().enumerate() {
        let line = raw_line.trim_end_matches('\r');
        let effective = line.strip_prefix(";+;").unwrap_or(line);

        // 段头：`[KeyXXX]`（新的段开始时，收尾上一个段）
        if let Some(caps) = KEY_SECTION_RE.captures(effective) {
            if !current_keys.is_empty() {
                if let (Some(section), Some(idx)) = (&current_section, section_line_index) {
                    result.push(KeybindSection {
                        section: section.clone(),
                        disabled: current_section_is_disabled,
                        section_line_index: idx,
                        keys: std::mem::take(&mut current_keys),
                    });
                }
            }
            current_section = caps.get(1).map(|m| m.as_str().to_string());
            section_line_index = Some(i);
            current_section_is_disabled = line != effective;
            current_keys.clear();
            continue;
        }

        // 段内 key = / back =（仅当已处于某个 Key 段内）
        if current_section.is_some() {
            if let Some(caps) = KEY_VALUE_RE.captures(effective) {
                current_keys.push(KeybindEntry {
                    is_main_key: true,
                    value: caps
                        .get(1)
                        .map(|m| m.as_str().trim().to_string())
                        .unwrap_or_default(),
                    line_index: i,
                });
            }
            if let Some(caps) = BACK_VALUE_RE.captures(effective) {
                current_keys.push(KeybindEntry {
                    is_main_key: false,
                    value: caps
                        .get(1)
                        .map(|m| m.as_str().trim().to_string())
                        .unwrap_or_default(),
                    line_index: i,
                });
            }
        }
    }

    // 收尾：最后一个段
    if !current_keys.is_empty() {
        if let (Some(section), Some(idx)) = (&current_section, section_line_index) {
            result.push(KeybindSection {
                section: section.clone(),
                disabled: current_section_is_disabled,
                section_line_index: idx,
                keys: current_keys,
            });
        }
    }

    result
}

/// 读取 INI preamble 的 `namespace=` 值（小写），未找到则回退为文件路径小写
/// （对齐 NRMM `_getNamespaceLowercase`，用于重复段名检测）
fn get_namespace_lowercase(lines: &[String], file_path: &str) -> String {
    let mut namespace = file_path.to_lowercase();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with(';') {
            continue;
        }
        if trimmed
            .to_lowercase()
            .replace(' ', "")
            .starts_with("namespace=")
        {
            if let Some(idx) = trimmed.find('=') {
                namespace = trimmed[idx + 1..].trim().to_lowercase();
            }
            break;
        }
        if trimmed.starts_with('[') {
            break;
        }
    }
    namespace
}

/// 将按键段编辑应用到原始行（对齐 NRMM `saveKeys` 第 436-473 行）
///
/// 就地修改 `lines`（含 `\r` 保真；段头 / key 行按 NRMM 原样整行覆盖）：
/// 1. 段头行 → `[Key{section}]` 或 `;+;[Key{section}]`
/// 2. key/back 行 → `{key|back} = {value}` 或 `;+;{key|back} = {value}`
/// 3. 段体其他行（段头到下一个 `[` 段头之间、非 key/back 行）→ 加 / 剥 `;+;`（幂等）
fn apply_keybind_edits(lines: &mut [String], sections: &[KeybindSection]) {
    for section in sections {
        // 越界防御：段头行号无效则跳过该段
        if section.section_line_index >= lines.len() {
            continue;
        }

        // 1) 段头行
        lines[section.section_line_index] = if section.disabled {
            format!(";+;[Key{}]", section.section)
        } else {
            format!("[Key{}]", section.section)
        };

        // 2) key/back 行
        let key_line_indices: HashSet<usize> = section.keys.iter().map(|k| k.line_index).collect();
        for key in &section.keys {
            if key.line_index >= lines.len() {
                continue;
            }
            let kw = if key.is_main_key { "key" } else { "back" };
            lines[key.line_index] = if section.disabled {
                format!(";+;{kw} = {}", key.value)
            } else {
                format!("{kw} = {}", key.value)
            };
        }

        // 3) 段体其他行（段头之后直到下一个 `[` 段头）
        let mut section_end = lines.len();
        for (j, l) in lines
            .iter()
            .enumerate()
            .skip(section.section_line_index + 1)
        {
            let l = l.trim_end_matches('\r');
            let effective = l.strip_prefix(";+;").unwrap_or(l);
            if effective.trim_start().starts_with('[') {
                section_end = j;
                break;
            }
        }
        for (j, raw) in lines
            .iter_mut()
            .enumerate()
            .skip(section.section_line_index + 1)
            .take(section_end.saturating_sub(section.section_line_index + 1))
        {
            if key_line_indices.contains(&j) {
                continue;
            }
            if section.disabled {
                if !raw.starts_with(";+;") {
                    *raw = format!(";+;{raw}");
                }
            } else if let Some(stripped) = raw.strip_prefix(";+;") {
                *raw = stripped.to_string();
            }
        }
    }
}

/// 读取模组的全部按键段（对齐 NRMM `loadKeys`）
///
/// 递归收集 mod 目录下所有 INI，解析每个文件的 `[KeyXXX]` 段，
/// 返回带行号的结构（供前端渲染与后续写回定位）。
#[tauri::command]
pub async fn read_mod_keybinds(mod_path: String) -> Result<Vec<KeybindFile>, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<KeybindFile>, String> {
        let root = PathBuf::from(&mod_path);
        if !root.is_dir() {
            return Err(format!(
                "[PATH_NOT_FOUND] mod directory does not exist: {mod_path}"
            ));
        }

        let ini_files = collect_ini_files(&root);
        let mut result: Vec<KeybindFile> = Vec::new();
        for ini in &ini_files {
            let raw = match IniFile::force_read_as_utf8(&ext_path(ini)) {
                Ok(r) => r,
                Err(e) => {
                    // 单文件读取失败不阻断整体（对齐 NRMM loadKeys 的 catch 语义），记录并跳过
                    log::warn!(
                        "[read_mod_keybinds] skip unreadable ini {}: {e}",
                        deext_path(ini)
                    );
                    continue;
                }
            };
            let raw_lines: Vec<String> = raw.split('\n').map(|s| s.to_string()).collect();
            let sections = parse_key_sections(&raw_lines);
            if sections.is_empty() {
                continue;
            }
            result.push(KeybindFile {
                file_path: deext_path(ini),
                sections,
            });
        }
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 保存编辑后的按键段（对齐 NRMM `saveKeys`）
///
/// 对每个文件：重新读取原始行 → 按传入行号原位改写 → 原子写回。
/// 返回是否检测到重复段名（大小写不敏感 + namespace 后缀）。
#[tauri::command]
pub async fn save_mod_keybinds(
    mod_path: String,
    files: Vec<KeybindFile>,
) -> Result<SaveKeybindResult, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<SaveKeybindResult, String> {
        let root = PathBuf::from(&mod_path);
        if !root.is_dir() {
            return Err(format!(
                "[PATH_NOT_FOUND] mod directory does not exist: {mod_path}"
            ));
        }

        let mut duplicate_sections = false;
        // 已见段签名：`{section_lowercase}{namespace_lowercase}`（对齐 NRMM lowerCasedSections）
        let mut seen_sections: HashSet<String> = HashSet::new();

        for file in &files {
            let path = PathBuf::from(&file.file_path);
            // Windows 长路径支持：读/写统一走 \\?\ 扩展路径
            let ext = ext_path(&path);
            let raw = IniFile::force_read_as_utf8(&ext)
                .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
            let mut raw_lines: Vec<String> = raw.split('\n').map(|s| s.to_string()).collect();

            apply_keybind_edits(&mut raw_lines, &file.sections);

            let content = raw_lines.join("\n");
            atomic_write(&ext, content.as_bytes())
                .map_err(|e| format!("failed to write {}: {e}", path.display()))?;

            // 重复段名检测（用写入后的行重新计算 namespace）
            let namespace = get_namespace_lowercase(&raw_lines, &file.file_path);
            for section in &file.sections {
                let sig = format!("{}{}", section.section.to_lowercase(), namespace);
                if !seen_sections.insert(sig) {
                    duplicate_sections = true;
                }
            }
        }

        Ok(SaveKeybindResult { duplicate_sections })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 模拟组合键（对齐 NRMM `simulateKeysFromKeySections`）
///
/// 解析 combo 字符串（如 `no_modifiers 5`、`alt 5`、`ctrl shift F10`）并
/// 通过平台按键模拟器发送。非法按键 / 超长组合返回错误。
///
/// `game`：当前目标游戏（可选）。传入时解析其进程名并定位游戏窗口，
/// 使按键模拟对齐 `simulate_f10` 的「定向对应游戏」语义——按键经由全局输入队列
/// （3Dmigoto 键盘钩子拦截）送达对应游戏，确保 mod 热键真正触发。
#[tauri::command]
pub async fn simulate_keys(combo: String, game: Option<String>) -> Result<(), String> {
    let keys = key_combo::parse_combo(&combo)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut sim = get_key_simulator();
        // 定向到对应游戏（对齐 simulate_f10）：解析游戏进程名并定位其窗口
        if let Some(game_str) = game {
            if let Ok(game_enum) = crate::hotkey::parse_game(&game_str) {
                if let Some(first_pn) = game_enum.process_names().first() {
                    let _ = sim.set_target_process(first_pn);
                }
            }
        }
        sim.simulate_combo(&keys).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_lines(raw: &str) -> Vec<String> {
        raw.split('\n').map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_simple_key_section() {
        let lines = to_lines("[KeyTop]\nkey = no_modifiers 5\n");
        let sections = parse_key_sections(&lines);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].section, "Top");
        assert!(!sections[0].disabled);
        assert_eq!(sections[0].section_line_index, 0);
        assert_eq!(sections[0].keys.len(), 1);
        assert!(sections[0].keys[0].is_main_key);
        assert_eq!(sections[0].keys[0].value, "no_modifiers 5");
        assert_eq!(sections[0].keys[0].line_index, 1);
    }

    #[test]
    fn parse_key_and_back() {
        let lines = to_lines("[KeyShoulder]\nkey = alt 5\nback = no_modifiers 6\n");
        let sections = parse_key_sections(&lines);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].keys.len(), 2);
        assert!(sections[0].keys[0].is_main_key);
        assert!(!sections[0].keys[1].is_main_key);
        assert_eq!(sections[0].keys[1].value, "no_modifiers 6");
        assert_eq!(sections[0].keys[1].line_index, 2);
    }

    #[test]
    fn parse_disabled_section() {
        let lines = to_lines(";+;[KeyTop]\n;+;key = alt 5\n");
        let sections = parse_key_sections(&lines);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].disabled);
        assert_eq!(sections[0].keys[0].value, "alt 5");
    }

    #[test]
    fn parse_multiple_sections_skip_non_key() {
        let lines = to_lines(
            "[TextureOverride]\nhash = 0x1234\n\n[KeyTop]\nkey = no_modifiers 5\n\n[KeyBody]\nkey = alt 5\n",
        );
        let sections = parse_key_sections(&lines);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].section, "Top");
        assert_eq!(sections[1].section, "Body");
    }

    #[test]
    fn parse_ignores_section_without_keys() {
        let lines = to_lines("[KeyEmpty]\ncondition = something\n");
        let sections = parse_key_sections(&lines);
        assert!(sections.is_empty());
    }

    #[test]
    fn parse_case_insensitive_section_and_key() {
        let lines = to_lines("[keytop]\nKEY = No_Modifiers 5\n");
        let sections = parse_key_sections(&lines);
        assert_eq!(sections.len(), 1);
        // 与 NRMM 一致：正则 `[Key`（大小写不敏感）消耗 "key"，捕获剩余 "top"
        assert_eq!(sections[0].section, "top");
        assert_eq!(sections[0].keys[0].value, "No_Modifiers 5");
    }

    #[test]
    fn apply_edit_disable_all() {
        let mut lines = to_lines("[KeyTop]\nkey = no_modifiers 5\n\n[KeyBody]\nkey = alt 5\n");
        let sections = vec![
            KeybindSection {
                section: "Top".to_string(),
                disabled: true,
                section_line_index: 0,
                keys: vec![KeybindEntry {
                    is_main_key: true,
                    value: "no_modifiers 5".to_string(),
                    line_index: 1,
                }],
            },
            KeybindSection {
                section: "Body".to_string(),
                disabled: true,
                section_line_index: 3,
                keys: vec![KeybindEntry {
                    is_main_key: true,
                    value: "alt 5".to_string(),
                    line_index: 4,
                }],
            },
        ];
        apply_keybind_edits(&mut lines, &sections);
        assert_eq!(lines[0], ";+;[KeyTop]");
        assert_eq!(lines[1], ";+;key = no_modifiers 5");
        assert_eq!(lines[3], ";+;[KeyBody]");
        assert_eq!(lines[4], ";+;key = alt 5");
    }

    #[test]
    fn apply_edit_enable_all() {
        let mut lines = to_lines(";+;[KeyTop]\n;+;key = no_modifiers 5\n;+;condition = x == 1\n");
        let sections = vec![KeybindSection {
            section: "Top".to_string(),
            disabled: false,
            section_line_index: 0,
            keys: vec![KeybindEntry {
                is_main_key: true,
                value: "no_modifiers 5".to_string(),
                line_index: 1,
            }],
        }];
        apply_keybind_edits(&mut lines, &sections);
        assert_eq!(lines[0], "[KeyTop]");
        assert_eq!(lines[1], "key = no_modifiers 5");
        // 段体其他行（非 key 行）也剥除 ;+;
        assert_eq!(lines[2], "condition = x == 1");
    }

    #[test]
    fn apply_edit_rename_section_and_value() {
        let mut lines = to_lines("[KeyTop]\nkey = no_modifiers 5\n");
        let sections = vec![KeybindSection {
            section: "Head".to_string(),
            disabled: false,
            section_line_index: 0,
            keys: vec![KeybindEntry {
                is_main_key: true,
                value: "alt 7".to_string(),
                line_index: 1,
            }],
        }];
        apply_keybind_edits(&mut lines, &sections);
        assert_eq!(lines[0], "[KeyHead]");
        assert_eq!(lines[1], "key = alt 7");
    }

    #[test]
    fn collect_ini_files_skips_disabled_and_desktop() {
        let dir = std::env::temp_dir().join(format!("keybind_cmd_test_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("DISABLED_Sub")).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.ini"), "").unwrap();
        std::fs::write(dir.join("desktop.ini"), "").unwrap();
        std::fs::write(dir.join("sub").join("b.INI"), "").unwrap();
        std::fs::write(dir.join("DISABLED_Sub").join("c.ini"), "").unwrap();

        let files = collect_ini_files(&dir);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"a.ini".to_string()));
        assert!(names.contains(&"b.INI".to_string()));
        assert!(!names.contains(&"desktop.ini".to_string()));
        assert!(!names.contains(&"c.ini".to_string()));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn collect_ini_files_keeps_root_with_disabled_prefix() {
        // 回归：mod 目录名带 DISABLED 前缀（禁用模组）时，root 自身不被过滤，
        // 其内部 ini 仍可发现（对齐 NRMM：disabled 检查基于 mod 根内相对路径）
        let dir =
            std::env::temp_dir().join(format!("keybind_disabled_root_{}", std::process::id()));
        let mod_dir = dir.join("DISABLED荧-冬狩夜巡NSFW切换");
        std::fs::create_dir_all(&mod_dir).unwrap();
        std::fs::write(
            mod_dir.join("merged.ini"),
            "[KeyTop]\nkey = no_modifiers 5\n",
        )
        .unwrap();

        let files = collect_ini_files(&mod_dir);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(
            names.contains(&"merged.ini".to_string()),
            "mod root named DISABLED... must still expose its ini files, got {names:?}"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn real_gimi_keybindings_file_parses() {
        // 真实 3Dmigoto 文件端到端验证（文件 IO + 解析链路）
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/NRMM-Rust-test/Core/GIMI");
        if !root.is_dir() {
            return; // 测试数据缺失时跳过（裁剪环境）
        }
        let files = collect_ini_files(&root);
        let kb = files
            .iter()
            .find(|p| p.file_name().is_some_and(|n| n == "KeyBindings.ini"))
            .expect("KeyBindings.ini found");

        let raw = IniFile::force_read_as_utf8(kb).unwrap();
        let raw_lines: Vec<String> = raw.split('\n').map(|s| s.to_string()).collect();
        let sections = parse_key_sections(&raw_lines);
        assert!(
            !sections.is_empty(),
            "real KeyBindings.ini should contain [KeyXXX] sections"
        );

        let toggle = sections
            .iter()
            .find(|s| s.section == "ToggleMods")
            .expect("KeyToggleMods parsed");
        assert!(toggle.keys[0].is_main_key);
        assert_eq!(toggle.keys[0].value, "no_modifiers F6");
    }
}
