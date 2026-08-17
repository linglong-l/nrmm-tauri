//! DLL 能力检测模块
//!
//! 对齐 NRMM（No-Reload-Mod-Manager）`mod_manager.dart` 的：
//! - `_isDllForNrmm`：检测 DLL 是否为 NRMM 自定义 XXMI DLL（支持 `[Loader]` 段的 `manager` key）
//! - `dllSupportsAdditionalForegroundWindow` / `_checkRdata`：检测 DLL 是否支持
//!   `additional_foreground_window`（在 `.rdata` / `.rodata` 段含 UTF-16LE 标记）
//!
//! 依据检测结果选择 `_MANAGED_` 下的 `nrmm_keypress.txt` 模板：
//! - Manager 自定义 DLL → `listen_keypress_manager.txt`（`[Loader] manager = ...`）
//! - 支持额外前台窗口 → `listen_keypress_additional_window.txt`（`[System] additional_foreground_window = ...`）
//! - 其他 → `listen_keypress_even_on_background.txt`（`[System] check_foreground_window = 0`）

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// NRMM 自定义 DLL 标记串（“Manager” key supported in [Loader] section）
const NRMM_MANAGER_MARKER: &[u8] = b"\"Manager\" key supported in [Loader] section";

/// 额外前台窗口能力标记串（UTF-16LE）
const ADDITIONAL_FOREGROUND_WINDOW: &str = "additional_foreground_window";

/// 选择的按键监听模板类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeypressTemplate {
    /// NRMM 自定义 XXMI DLL（[Loader] manager）
    Manager,
    /// 支持 additional_foreground_window 的 DLL（[System] additional_foreground_window）
    AdditionalWindow,
    /// 默认：仅在后台监听按键（[System] check_foreground_window = 0）
    EvenOnBackground,
}

impl KeypressTemplate {
    /// 返回对应的内置资源字节（已含 `{game}` 占位符，调用方需替换）
    pub fn template_bytes(&self) -> &'static [u8] {
        match self {
            KeypressTemplate::Manager => crate::resources::LISTEN_KEYPRESS_MANAGER,
            KeypressTemplate::AdditionalWindow => {
                crate::resources::LISTEN_KEYPRESS_ADDITIONAL_WINDOW
            }
            KeypressTemplate::EvenOnBackground => {
                crate::resources::LISTEN_KEYPRESS_EVEN_ON_BACKGROUND
            }
        }
    }
}

/// 检测 DLL 是否为 NRMM 自定义 XXMI DLL（支持 `[Loader]` 段的 `manager` key）。
///
/// 对齐 Dart `_isDllForNrmm`：
/// 1. 文件大小须介于 1MB ~ 150MB（真实 d3d11.dll 的合理范围）
/// 2. 文件内容中包含 ASCII 标记串 `"Manager" key supported in [Loader] section`
pub fn is_dll_for_nrmm(dll_path: &Path) -> bool {
    let meta = match fs::metadata(dll_path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let size = meta.len();
    if !(1024 * 1024..=150 * 1024 * 1024).contains(&size) {
        return false;
    }
    let bytes = match fs::read(dll_path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    boyer_moore_search(&bytes, NRMM_MANAGER_MARKER)
}

/// 检测 DLL 是否支持 `additional_foreground_window` 能力。
///
/// 对齐 Dart `dllSupportsAdditionalForegroundWindow` → `_checkRdata`：
/// 解析 PE 文件，遍历各 Section Header，定位只读数据段（`.rdata` / `.rodata`
/// 或具备 `CNT_INITIALIZED_DATA | MEM_READ` 且非 `MEM_WRITE` 特征），
/// 在段的原始数据（RawData）中搜索 UTF-16LE 编码的 `additional_foreground_window`。
pub fn dll_supports_additional_foreground_window(dll_path: &Path) -> bool {
    let mut file = match fs::File::open(dll_path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    check_rdata(&mut file)
}

/// 综合两项检测，选择 `nrmm_keypress.txt` 应使用的模板。
///
/// 优先级：Manager 自定义 DLL > 支持额外前台窗口 > 默认后台监听。
pub fn select_keypress_template(dll_path: &Path) -> KeypressTemplate {
    if is_dll_for_nrmm(dll_path) {
        KeypressTemplate::Manager
    } else if dll_supports_additional_foreground_window(dll_path) {
        KeypressTemplate::AdditionalWindow
    } else {
        KeypressTemplate::EvenOnBackground
    }
}

/// PE 解析：检测任一只读数据段是否含额外前台窗口标记。
fn check_rdata<F: Read + Seek>(file: &mut F) -> bool {
    // DOS 头（64 字节）
    let mut dos = [0u8; 64];
    if file.seek(SeekFrom::Start(0)).is_err() {
        return false;
    }
    if file.read_exact(&mut dos).is_err() {
        return false;
    }
    if dos.len() < 64 || dos[0] != 0x4D || dos[1] != 0x5A {
        return false; // 非 'MZ' 开头
    }

    // PE 头部偏移（DOS 头 0x3C 处的 u32 LE）
    let pe_offset = u32::from_le_bytes([dos[0x3C], dos[0x3D], dos[0x3E], dos[0x3F]]) as u64;

    // COFF 头（PE 偏移 + 4 起，24 字节）
    let mut coff = [0u8; 24];
    if file.seek(SeekFrom::Start(pe_offset)).is_err() {
        return false;
    }
    if file.read_exact(&mut coff).is_err() {
        return false;
    }
    if coff[0] != 0x50 || coff[1] != 0x45 {
        return false; // 非 'PE'
    }

    let num_sections = u16::from_le_bytes([coff[6], coff[7]]);
    let opt_header_size = u16::from_le_bytes([coff[20], coff[21]]);
    let section_table_start = pe_offset + 4 + 20 + opt_header_size as u64;

    let mut sh = [0u8; 40];
    for i in 0..num_sections {
        if file
            .seek(SeekFrom::Start(section_table_start + i as u64 * 40))
            .is_err()
        {
            break;
        }
        if file.read_exact(&mut sh).is_err() {
            break;
        }

        // 段特征（偏移 36，u32 LE）
        let characteristics = u32::from_le_bytes([sh[36], sh[37], sh[38], sh[39]]);
        // IMAGE_SCN_CNT_INITIALIZED_DATA (0x40) | IMAGE_SCN_MEM_READ (0x40000000)
        // 且非 IMAGE_SCN_MEM_WRITE (0x80000000)
        let is_read_only_data = (characteristics & 0x00000040) != 0
            && (characteristics & 0x40000000) != 0
            && (characteristics & 0x80000000) == 0;

        // 段名（前 8 字节，去尾随 \0）
        let name = {
            let end = sh[0..8].iter().position(|&b| b == 0).unwrap_or(8);
            String::from_utf8_lossy(&sh[0..end]).to_string()
        };
        let is_target_name = name == ".rdata" || name == ".rodata";

        // 命中条件：只读数据段 或 目标段名
        if !is_read_only_data && !is_target_name {
            continue;
        }

        let raw_size = u32::from_le_bytes([sh[16], sh[17], sh[18], sh[19]]) as u64;
        let raw_offset = u32::from_le_bytes([sh[20], sh[21], sh[22], sh[23]]) as u64;
        if raw_size == 0 || raw_offset == 0 {
            continue;
        }

        let mut section = vec![0u8; raw_size as usize];
        if file.seek(SeekFrom::Start(raw_offset)).is_err() {
            continue;
        }
        if file.read_exact(&mut section).is_err() {
            continue;
        }

        if contains_utf16_le(&section, ADDITIONAL_FOREGROUND_WINDOW) {
            return true;
        }
    }

    false
}

/// 在字节串中搜索 UTF-16LE 编码的 needle（ASCII 串作为 UTF-16LE 即每字符后跟 0x00）。
fn contains_utf16_le(haystack: &[u8], needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let needle_bytes = needle.as_bytes();
    let mut pattern = Vec::with_capacity(needle_bytes.len() * 2);
    for &b in needle_bytes {
        pattern.push(b);
        pattern.push(0u8);
    }

    let needle_len = pattern.len();
    if haystack.len() < needle_len {
        return false;
    }

    let first = pattern[0];
    let second = pattern[1];
    let mut idx = 0usize;
    while idx <= haystack.len() - needle_len {
        idx = match haystack[idx..].iter().position(|&b| b == first) {
            Some(p) => idx + p,
            None => break,
        };
        if idx > haystack.len() - needle_len {
            break;
        }
        if haystack[idx + 1] == second {
            let mut j = 2usize;
            while j < needle_len && haystack[idx + j] == pattern[j] {
                j += 1;
            }
            if j == needle_len {
                return true;
            }
        }
        idx += 1;
    }
    false
}

/// Boyer–Moore–Horspool 子串搜索（用于 ASCII 标记串）。
fn boyer_moore_search(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    let n = needle.len();
    let mut skip = [n as u8; 256];
    for i in 0..n - 1 {
        skip[needle[i] as usize] = (n - 1 - i) as u8;
    }
    let mut i = 0usize;
    while i <= haystack.len() - n {
        let mut j = n - 1;
        while needle[j] == haystack[i + j] {
            if j == 0 {
                return true;
            }
            j -= 1;
        }
        i += skip[haystack[i + n - 1] as usize] as usize;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boyer_moore_basic() {
        assert!(boyer_moore_search(
            b"abc \"Manager\" key supported in [Loader] section xyz",
            NRMM_MANAGER_MARKER
        ));
        assert!(!boyer_moore_search(b"no marker here", NRMM_MANAGER_MARKER));
    }

    #[test]
    fn utf16_le_basic() {
        // "ab" as UTF-16LE
        let bytes = [b'a', 0u8, b'b', 0u8];
        assert!(contains_utf16_le(&bytes, "ab"));
        assert!(!contains_utf16_le(&bytes, "ac"));
    }
}
