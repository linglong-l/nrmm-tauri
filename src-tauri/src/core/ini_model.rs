//! INI 数据模型与核心 IO：IniLine/IniSection/IniFile、解析、渲染、原子写、备份恢复
//!
//! 从 ini_handler.rs 拆出（2026-09-18 试点 3.3）。
//! 变换/维护/分析方法按域分布于 ini_transform/ini_maintain/ini_analysis
//! （同 crate 多文件 `impl IniFile`），本文件只承载模型与 IO。

use anyhow::{bail, Context, Result};
use std::borrow::Cow;
use std::cell::RefCell;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::core::ini_condition::is_conditional_section;

// 线程级逐行/解码缓冲池：跨多次 `IniFile::parse` 复用（`READ_BUF` 读取缓冲在 ini_codec）。
thread_local! {
    static LINE_BUF: RefCell<String> = RefCell::new(String::with_capacity(1024));
    static DECODED_BUF: RefCell<String> = RefCell::new(String::with_capacity(1024));
}

#[derive(Debug, Clone, PartialEq)]
pub enum IniLine {
    Empty {
        indent: usize,
    },
    Comment(String),
    DisabledKeyValue {
        key: String,
        value: String,
        comment: Option<String>,
    },
    KeyValue {
        key: String,
        value: String,
        disabled: bool,
        comment: Option<String>,
        indent: usize,
    },
    IfStart {
        condition: String,
        indent: usize,
    },
    Elif {
        condition: String,
        indent: usize,
    },
    Else {
        indent: usize,
    },
    EndIf {
        indent: usize,
    },
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

impl std::fmt::Display for IniLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IniLine::Empty { indent } => write!(f, "{}", " ".repeat(*indent)),
            IniLine::Comment(text) => write!(f, "{}", text),
            IniLine::DisabledKeyValue {
                key,
                value,
                comment,
            } => {
                write!(f, ";-;{} = {}", key, value)?;
                if let Some(c) = comment {
                    write!(f, " ; {}", c)?;
                }
                Ok(())
            }
            IniLine::KeyValue {
                key,
                value,
                disabled,
                comment,
                indent,
            } => {
                if *disabled {
                    write!(f, ";-;")?;
                }
                write!(f, "{}{} = {}", " ".repeat(*indent), key, value)?;
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
        let file =
            fs::File::open(path).with_context(|| format!("Failed to open file: {:?}", path))?;
        let mut reader = BufReader::with_capacity(crate::core::ini_codec::READ_BUFFER_CAPACITY, file);

        // 从线程级缓冲池取出可复用缓冲区（取出后 RefCell 内留下默认空串，跨调用复用，降低分配开销）
        let mut line_buf = LINE_BUF.with(|lb| std::mem::take(&mut *lb.borrow_mut()));
        let mut decoded = DECODED_BUF.with(|db| std::mem::take(&mut *db.borrow_mut()));

        let mut preamble: Vec<IniLine> = Vec::new();
        let mut sections: Vec<IniSection> = Vec::new();
        let mut current_section: Option<usize> = None;
        let mut first_line = true;

        loop {
            line_buf.clear();
            let n = reader
                .read_line(&mut line_buf)
                .with_context(|| format!("Failed to read file: {:?}", path))?;
            if n == 0 {
                break;
            }

            // 逐行有损解码：等价于整文件 from_utf8_lossy，但避免整文件驻留内存（流式 + 内存复用）
            decoded.clear();
            match String::from_utf8_lossy(line_buf.as_bytes()) {
                Cow::Borrowed(b) => decoded.push_str(b),
                Cow::Owned(o) => decoded.push_str(&o),
            }
            if decoded.ends_with('\n') {
                decoded.pop();
            }
            if decoded.ends_with('\r') {
                decoded.pop();
            }
            if first_line {
                first_line = false;
                if decoded.starts_with('\u{feff}') {
                    decoded.remove(0);
                }
            }

            let raw_line = &decoded;
            let line = crate::core::ini_condition::trim_trailing_whitespace(raw_line);

            if line.is_empty() {
                match current_section {
                    Some(idx) => sections[idx].lines.push(IniLine::Empty { indent: 0 }),
                    None => preamble.push(IniLine::Empty { indent: 0 }),
                }
                continue;
            }

            if line.starts_with(";-;") {
                let after_prefix = line.strip_prefix(";-;").unwrap_or(line).trim_start();
                match crate::core::ini_condition::parse_key_value(after_prefix) {
                    Some((key, value, comment)) => {
                        let kv = IniLine::DisabledKeyValue {
                            key,
                            value,
                            comment,
                        };
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
                let condition = trimmed
                    .strip_prefix("if ")
                    .unwrap_or(trimmed)
                    .trim()
                    .to_string();
                let l = IniLine::IfStart { condition, indent };
                match current_section {
                    Some(idx) => sections[idx].lines.push(l),
                    None => preamble.push(IniLine::PreambleLine(l.to_string())),
                }
                continue;
            }

            if trimmed.starts_with("elif ") {
                let condition = trimmed
                    .strip_prefix("elif ")
                    .unwrap_or(trimmed)
                    .trim()
                    .to_string();
                let l = IniLine::Elif { condition, indent };
                match current_section {
                    Some(idx) => sections[idx].lines.push(l),
                    None => preamble.push(IniLine::PreambleLine(l.to_string())),
                }
                continue;
            }

            // `else if <cond>` 是 3Dmigoto/NRMM 的合法条件流（等价 elif）。显式解析为 Elif
            // 以保留条件原文：否则会落入 KeyValue（key=`else if $X`、value=`= N`），
            // 无法区分畸形 `else if $X = = N` 与合法 `else if $X == N`。
            if trimmed.starts_with("else if ") {
                let condition = trimmed
                    .strip_prefix("else if ")
                    .unwrap_or(trimmed)
                    .trim()
                    .to_string();
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
                match crate::core::ini_condition::parse_key_value(line) {
                    Some((key, value, comment)) => {
                        let lower_key = key.to_lowercase();
                        if lower_key == "include" {
                            let l = IniLine::Include(value);
                            match current_section {
                                Some(idx) => sections[idx].lines.push(l),
                                None => preamble.push(l),
                            }
                        } else {
                            let l = IniLine::KeyValue {
                                key,
                                value,
                                disabled: false,
                                comment,
                                indent: 0,
                            };
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

        // 归还缓冲区到线程级缓冲池，供后续解析复用
        LINE_BUF.with(|lb| *lb.borrow_mut() = line_buf);
        DECODED_BUF.with(|db| *db.borrow_mut() = decoded);

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
        // 幂等：若 preamble 已以首行头部注释开头，则不再重复插入（避免多次 update 后头部倍增）
        let already_has = self
            .preamble
            .iter()
            .take(1)
            .any(|l| matches!(l, IniLine::Comment(text) if text == header_lines[0]));
        if already_has {
            return;
        }
        for line in header_lines.iter().rev() {
            self.preamble.insert(0, IniLine::Comment(line.to_string()));
        }
    }

    pub fn write_atomic(&self, path: &Path) -> Result<()> {
        let tmp_path = path.with_extension("ini.tmp");
        let file = fs::File::create(&tmp_path)
            .with_context(|| format!("Failed to create temp file: {:?}", tmp_path))?;
        let mut writer = BufWriter::new(file);
        self.render(&mut writer)?;
        writer.flush()?;
        drop(writer);

        fs::rename(&tmp_path, path)
            .with_context(|| format!("Failed to rename temp file to {:?}", path))?;

        Ok(())
    }

    /// 将 INI 内容序列化到指定 writer（[`Self::write_atomic`] 的渲染核心）。
    ///
    /// 格式与原内联实现逐字节一致：preamble 行 → 各段（`[name]` + 段内行，段间空行分隔，
    /// 段尾 Empty 行剔除）→ 文件末尾条件空行（对齐 Dart `_getLiteralIni` 的 needsSeparator）。
    ///
    /// # 参数
    /// - `writer`: 目标写入器
    ///
    /// # 返回
    /// - `Ok(())`: 序列化完成
    /// - `Err(e)`: 写入失败
    fn render(&self, writer: &mut impl std::io::Write) -> Result<()> {
        for line in &self.preamble {
            writeln!(writer, "{}", line)?;
        }

        let mut first_section = true;
        let mut last_section_ends_with_real = false;
        for section in &self.sections {
            if !first_section {
                writeln!(writer)?;
            }
            first_section = false;

            writeln!(writer, "[{}]", section.name)?;
            // 跳过段尾的 Empty 行（对齐 Dart：段尾空行不计入 needsSeparator）
            let lines: Vec<&IniLine> = section
                .lines
                .iter()
                .rev()
                .skip_while(|l| matches!(l, IniLine::Empty { .. }))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            for line in &lines {
                writeln!(writer, "{}", line)?;
            }
            // 段末最后一行是否为「真实内容」（非空、非纯 ; 注释；;-; 注释在 Dart 中视为真实内容）。
            // 仅当最后一段（文件末尾）末行为真实内容时，才在文末追加一个空行，
            // 对齐 Dart _getLiteralIni 的 needsSeparator（段后/文件末尾空行分隔）。
            last_section_ends_with_real =
                lines.last().is_some_and(|l| Self::is_real_content_line(l));
        }

        // 文件末尾空行（对齐 Dart 对最后一段的 needsSeparator 处理）
        if last_section_ends_with_real {
            writeln!(writer)?;
        }

        Ok(())
    }

    /// 内容不变则跳过写入的原子写（[`Self::write_atomic`] 的幂等快速路径）。
    ///
    /// 性能关键（2026-08-18）：update_mod_data 重复运行时，管线输出与磁盘内容逐字节相同的
    /// INI 占绝大多数；跳过写盘可省去每文件 create-temp + rename 两次系统调用与杀毒实时扫描
    /// （实测 2.7 万 INI 的组处理阶段 ~30% 耗时在无变化写回上）。写入语义与
    /// [`Self::write_atomic`] 完全一致（临时文件 + rename 原子替换），仅当序列化字节与
    /// 现有文件字节完全相同时不写。文件不存在、读取失败均视为「有变化」走正常写入。
    ///
    /// # 参数
    /// - `path`: 目标 INI 路径
    ///
    /// # 返回
    /// - `Ok(true)`: 内容有变化，已原子写入
    /// - `Ok(false)`: 内容未变化，已跳过写入
    /// - `Err(e)`: 序列化或写入失败
    pub fn write_atomic_skip_unchanged(&self, path: &Path) -> Result<bool> {
        let mut buf: Vec<u8> = Vec::with_capacity(4096);
        self.render(&mut buf)?;
        match fs::read(path) {
            Ok(existing) if existing == buf => Ok(false),
            _ => {
                let tmp_path = path.with_extension("ini.tmp");
                fs::write(&tmp_path, &buf)
                    .with_context(|| format!("Failed to write temp file: {:?}", tmp_path))?;
                fs::rename(&tmp_path, path)
                    .with_context(|| format!("Failed to rename temp file to {:?}", path))?;
                // 加固（根因 4）：真实写盘后显式失效 mod_ini_cache 条目。
                // 常规失效依赖 (mtime, len) 自动未命中，但 mtime 保留型编辑器 / 粗分辨率
                // 同秒等长替换会让 (mtime, len) 恰好不变 → get_or_parse_ini 返回旧解析，
                // 下轮 update 基于旧内容渲染并误判「未修改」跳过写盘。写后立即失效，
                // 强制下次读取重新解析磁盘真实内容。
                crate::core::mod_ini_cache::invalidate_ini(path);
                Ok(true)
            }
        }
    }

    /// 对齐 Dart _getLiteralIni 的 needsSeparator 判定：
    /// 空行与纯 `;` 注释不计为真实内容；`;-;` 注释（被注释掉的键/命令）在 Dart 中视为真实内容。
    fn is_real_content_line(line: &IniLine) -> bool {
        match line {
            IniLine::Empty { .. } => false,
            IniLine::Comment(c) => c.starts_with(";-;"),
            _ => true,
        }
    }

    pub fn backup(path: &Path) -> Result<PathBuf> {
        let backup_path = path.with_extension("ini_managed_backup");
        if backup_path.exists() {
            return Ok(backup_path);
        }
        crate::utils::atomic_copy(path, &backup_path)
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
        let bytes = crate::core::ini_codec::read_file_bytes(path)?;
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
        assert!(matches!(
            lines[0],
            IniLine::KeyValue {
                disabled: false,
                ..
            }
        ));
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
            indent: 0,
        });
        ini.write_atomic(f.path()).unwrap();
        assert_ne!(IniFile::force_read_as_utf8(f.path()).unwrap(), orig);

        IniFile::restore_from_backup(f.path()).unwrap();
        assert!(!backup.exists());
        assert_eq!(IniFile::force_read_as_utf8(f.path()).unwrap(), orig);
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
    fn test_parse_empty_file() {
        let f = write_temp_ini("");
        let ini = IniFile::parse(f.path()).unwrap();
        assert!(ini.preamble.is_empty());
        assert!(ini.sections.is_empty());
        assert!(IniFile::force_read_as_utf8(f.path()).unwrap().is_empty());
    }

    #[test]
    fn test_parse_duplicate_keys_preserved() {
        // 重复键：解析器保留全部行（3Dmigoto 语义为后写覆盖），不得丢行
        let ini_content = "[Section]\nkey = first\nkey = second\n";
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        let count = ini.sections[0]
            .lines
            .iter()
            .filter(|l| matches!(l, IniLine::KeyValue { key, .. } if key == "key"))
            .count();
        assert_eq!(count, 2, "重复键应全部保留");
    }

    #[test]
    fn test_parse_quoted_value_and_special_chars() {
        // 引号值内含分号不应被当作注释；反斜杠与中文应原样保留
        let ini_content =
            "[Section]\nkey = \"quoted ; value\" ; comment\n$var = a\\b\\c\nname = 中文测试\n";
        let f = write_temp_ini(ini_content);
        let ini = IniFile::parse(f.path()).unwrap();
        let lines = &ini.sections[0].lines;
        assert!(
            matches!(&lines[0], IniLine::KeyValue { value, comment: Some(c), .. }
                if value == "\"quoted ; value\"" && c == "comment"),
            "引号值中的分号不应截断值: {:?}",
            lines[0]
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, IniLine::KeyValue { key, value, .. }
                if key == "$var" && value == "a\\b\\c")),
            "反斜杠应原样保留"
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, IniLine::KeyValue { key, value, .. }
                if key == "name" && value == "中文测试")),
            "中文值应原样保留"
        );
    }
}