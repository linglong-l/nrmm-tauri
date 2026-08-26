//! 组合键解析模块（对齐 NRMM `keypress_simulator_manager.dart`）
//!
//! 将 3Dmigoto/XXMI 的组合键字符串（如 `no_modifiers 5`、`alt 5`、`ctrl shift F10`、
//! `VK_CLEAR VK_RETURN`、`0x70`）解析为虚拟键码序列，供按键模拟使用。
//!
//! 移植自原版 Dart 实现：
//! - `_vkMappings`（第 135-375 行）：虚拟键名 → 键码映射表（Obtained from 3dmigoto source code）
//! - `_parseVKey`（第 377-407 行）：单 token → 键码
//! - `simulateKeysFromKeySections`（第 82-111 行）的解析部分：combo → 键码序列
//!
//! 行为保持与 Dart 完全一致，包括原版固有行为：
//! - `no_*` 前缀 token 跳过（no_ctrl / no_modifiers 等）
//! - SHIFT（VK_SHIFT/VK_LSHIFT/VK_RSHIFT）移到序列末尾
//! - `0x` 十六进制先反向查表得到键名再查表（映射表外码值 → 失败）
//! - legacy 混合大小写键（"Num 1" 等）因 `to_uppercase` 永不命中（原版固有行为，不"修复"）

/// 虚拟键名 → 键码映射表（照搬 Dart `_vkMappings`，注释掉的按键不搬）
///
/// 注意：键名保持 Dart 原始大小写（含 legacy 混合大小写条目），
/// 查表时统一 `to_ascii_uppercase()`，与 Dart `_parseVKey` 的
/// `name.toUpperCase()` 查表行为逐字节一致。
pub static VK_MAPPINGS: &[(&str, u16)] = &[
    ("CANCEL", 0x03),
    ("BACK", 0x08),
    ("BACKSPACE", 0x08),
    ("BACK_SPACE", 0x08),
    ("TAB", 0x09),
    ("CLEAR", 0x0C),
    ("RETURN", 0x0D),
    ("ENTER", 0x0D),
    ("SHIFT", 0x10),
    ("CONTROL", 0x11),
    ("CTRL", 0x11),
    ("MENU", 0x12),
    ("ALT", 0x12),
    ("PAUSE", 0x13),
    ("CAPITAL", 0x14),
    ("CAPS", 0x14),
    ("CAPSLOCK", 0x14),
    ("CAPS_LOCK", 0x14),
    ("KANA", 0x15),
    ("HANGUEL", 0x15),
    ("HANGUL", 0x15),
    ("JUNJA", 0x17),
    ("FINAL", 0x18),
    ("HANJA", 0x19),
    ("KANJI", 0x19),
    ("ESCAPE", 0x1B),
    ("CONVERT", 0x1C),
    ("NONCONVERT", 0x1D),
    ("ACCEPT", 0x1E),
    ("MODECHANGE", 0x1F),
    ("SPACE", 0x20),
    ("PRIOR", 0x21),
    ("PGUP", 0x21),
    ("PAGEUP", 0x21),
    ("PAGE_UP", 0x21),
    ("NEXT", 0x22),
    ("PGDN", 0x22),
    ("PAGEDOWN", 0x22),
    ("PAGE_DOWN", 0x22),
    ("END", 0x23),
    ("HOME", 0x24),
    ("LEFT", 0x25),
    ("UP", 0x26),
    ("RIGHT", 0x27),
    ("DOWN", 0x28),
    ("SELECT", 0x29),
    ("PRINT", 0x2A),
    ("EXECUTE", 0x2B),
    ("SNAPSHOT", 0x2C),
    ("PRSCR", 0x2C),
    ("PRINTSCREEN", 0x2C),
    ("PRINT_SCREEN", 0x2C),
    ("INSERT", 0x2D),
    ("DELETE", 0x2E),
    ("HELP", 0x2F),
    ("NUMPAD0", 0x60),
    ("NUMPAD1", 0x61),
    ("NUMPAD2", 0x62),
    ("NUMPAD3", 0x63),
    ("NUMPAD4", 0x64),
    ("NUMPAD5", 0x65),
    ("NUMPAD6", 0x66),
    ("NUMPAD7", 0x67),
    ("NUMPAD8", 0x68),
    ("NUMPAD9", 0x69),
    ("MULTIPLY", 0x6A),
    ("ADD", 0x6B),
    ("SEPARATOR", 0x6C),
    ("SUBTRACT", 0x6D),
    ("DECIMAL", 0x6E),
    ("DIVIDE", 0x6F),
    ("F1", 0x70),
    ("F2", 0x71),
    ("F3", 0x72),
    ("F4", 0x73),
    ("F5", 0x74),
    ("F6", 0x75),
    ("F7", 0x76),
    ("F8", 0x77),
    ("F9", 0x78),
    ("F10", 0x79),
    ("F11", 0x7A),
    ("F12", 0x7B),
    ("F13", 0x7C),
    ("F14", 0x7D),
    ("F15", 0x7E),
    ("F16", 0x7F),
    ("F17", 0x80),
    ("F18", 0x81),
    ("F19", 0x82),
    ("F20", 0x83),
    ("F21", 0x84),
    ("F22", 0x85),
    ("F23", 0x86),
    ("F24", 0x87),
    ("NUMLOCK", 0x90),
    ("SCROLL", 0x91),
    ("LSHIFT", 0xA0),
    ("LEFT_SHIFT", 0xA0),
    ("RSHIFT", 0xA1),
    ("RIGHT_SHIFT", 0xA1),
    ("LCONTROL", 0xA2),
    ("LEFT_CONTROL", 0xA2),
    ("LCTRL", 0xA2),
    ("LEFT_CTRL", 0xA2),
    ("RCONTROL", 0xA3),
    ("RIGHT_CONTROL", 0xA3),
    ("RCTRL", 0xA3),
    ("RIGHT_CTRL", 0xA3),
    ("LMENU", 0xA4),
    ("LEFT_MENU", 0xA4),
    ("LALT", 0xA4),
    ("LEFT_ALT", 0xA4),
    ("RMENU", 0xA5),
    ("RIGHT_MENU", 0xA5),
    ("RALT", 0xA5),
    ("RIGHT_ALT", 0xA5),
    ("OEM_1", 0xBA),
    (";", 0xBA),
    (":", 0xBA),
    ("COLON", 0xBA),
    ("SEMICOLON", 0xBA),
    ("SEMI_COLON", 0xBA),
    ("OEM_PLUS", 0xBB),
    ("=", 0xBB),
    ("PLUS", 0xBB),
    ("EQUALS", 0xBB),
    ("OEM_COMMA", 0xBC),
    (",", 0xBC),
    ("<", 0xBC),
    ("COMMA", 0xBC),
    ("OEM_MINUS", 0xBD),
    ("MINUS", 0xBD),
    ("UNDERSCORE", 0xBD),
    ("_", 0xBD),
    ("OEM_PERIOD", 0xBE),
    (".", 0xBE),
    (">", 0xBE),
    ("PERIOD", 0xBE),
    ("OEM_2", 0xBF),
    ("/", 0xBF),
    ("?", 0xBF),
    ("SLASH", 0xBF),
    ("FORWARD_SLASH", 0xBF),
    ("QUESTION", 0xBF),
    ("QUESTION_MARK", 0xBF),
    ("OEM_3", 0xC0),
    ("`", 0xC0),
    ("~", 0xC0),
    ("TILDE", 0xC0),
    ("GRAVE", 0xC0),
    ("OEM_4", 0xDB),
    ("[", 0xDB),
    ("{", 0xDB),
    ("OEM_5", 0xDC),
    ("\\", 0xDC),
    ("|", 0xDC),
    ("BACKSLASH", 0xDC),
    ("BACK_SLASH", 0xDC),
    ("PIPE", 0xDC),
    ("VERTICAL_BAR", 0xDC),
    ("OEM_6", 0xDD),
    ("]", 0xDD),
    ("}", 0xDD),
    ("OEM_7", 0xDE),
    ("'", 0xDE),
    ("\"", 0xDE),
    ("QUOTE", 0xDE),
    ("DOUBLE_QUOTE", 0xDE),
    ("OEM_8", 0xDF),
    ("PLAY", 0xFA),
    ("ZOOM", 0xFB),
    ("NONAME", 0xFC),
    // legacy（混合大小写，为兼容旧 d3dx.ini 英文默认键位）
    // 注意：这些键名因 `to_ascii_uppercase()` 查表而永不命中（与 Dart 原版行为一致）
    ("Num 1", 0x61),
    ("Num 2", 0x62),
    ("Num 3", 0x63),
    ("Num 4", 0x64),
    ("Num 5", 0x65),
    ("Num 6", 0x66),
    ("Num 7", 0x67),
    ("Num 8", 0x68),
    ("Num 9", 0x69),
    ("*", 0x6A),
    ("Num /", 0x6F),
    ("-", 0x6D),
    ("+", 0x6B),
    ("Prnt Scrn", 0x2C),
];

/// 反向查表：键码 → 键名（Dart `getKeyName` 等价）
///
/// 返回映射表中第一个 `value == key_code` 的键名；未找到返回 `"missing"`。
/// 注意：与 Dart 一致，返回的键名会再经过 `to_ascii_uppercase` 查表，
/// 因此 legacy 混合大小写键（"Num 1" 等）无法被命中。
pub fn get_key_name(key_code: u16) -> &'static str {
    VK_MAPPINGS
        .iter()
        .find(|(_, v)| *v == key_code)
        .map(|(k, _)| *k)
        .unwrap_or("missing")
}

/// 解析单个按键 token 为虚拟键码（Dart `_parseVKey` 等价）
///
/// 处理三类输入：
/// 1. 单字符 `A-Z` / `0-9`（ASCII 值，大写）
/// 2. `0x` 十六进制（如 `0x70` → 反向查表得到键名再查表）
/// 3. 映射表键名（`F10` / `ALT` / `VK_CLEAR` / `VK_RETURN` / `VK_SPACE` 等，
///    `VK_` 前缀自动剥除，大小写不敏感）
pub fn parse_vkey(name: &str) -> Option<u16> {
    let mut name = name.trim();

    // 单字符：'A'..'Z' 或 '0'..'9'
    if name.len() == 1 {
        let c = name.to_ascii_uppercase().chars().next()?;
        let code = c as u16;
        if (0x30..=0x39).contains(&code) || (0x41..=0x5A).contains(&code) {
            return Some(code);
        }
    }

    // hex literal：如 "0x70"（先反向查表，与 Dart 一致）
    if let Some(hex) = name.strip_prefix("0x") {
        let code = u16::from_str_radix(hex, 16).ok()?;
        name = get_key_name(code);
    }

    // 剥 VK_ 前缀
    if name.to_ascii_uppercase().starts_with("VK_") {
        name = &name[3..];
    }

    // 大小写不敏感查表
    let upper = name.to_ascii_uppercase();
    VK_MAPPINGS
        .iter()
        .find(|(k, _)| *k == upper)
        .map(|(_, v)| *v)
}

/// 是否为 SHIFT 键（VK_SHIFT / VK_LSHIFT / VK_RSHIFT）
fn is_shift_key(vk: u16) -> bool {
    vk == 0x10 || vk == 0xA0 || vk == 0xA1
}

/// 解析组合键字符串为虚拟键码序列（Dart `simulateKeysFromKeySections` 解析段等价）
///
/// 规则：
/// - 按空白切分 token
/// - 跳过 `no_*` 前缀 token（no_ctrl / no_modifiers 等）
/// - 非法键 → Err；组合键超过 10 个 → Err
/// - SHIFT 键移到序列末尾（修复 3dmigoto/xxmi 忽略 shift 模拟的 bug）
///
/// 全为 `no_*` token 时返回空序列（与 Dart `_simulateMultipleKeypresses` 的空序列
/// 短路行为一致）。
pub fn parse_combo(combo: &str) -> Result<Vec<u16>, String> {
    let mut key_codes: Vec<u16> = Vec::new();
    for part in combo.split_whitespace() {
        if part.to_lowercase().starts_with("no_") {
            continue;
        }
        let vk = parse_vkey(part).ok_or_else(|| format!("invalid key: {part}"))?;
        key_codes.push(vk);
        if key_codes.len() > 10 {
            return Err("key combination too long (max 10 keys)".to_string());
        }
    }

    // SHIFT 移到末尾
    let shift_keys: Vec<u16> = key_codes.iter().copied().filter(|k| is_shift_key(*k)).collect();
    key_codes.retain(|k| !is_shift_key(*k));
    key_codes.extend(shift_keys);

    Ok(key_codes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vkey_single_char_digit() {
        assert_eq!(parse_vkey("5"), Some(0x35));
        assert_eq!(parse_vkey("0"), Some(0x30));
        assert_eq!(parse_vkey("9"), Some(0x39));
    }

    #[test]
    fn parse_vkey_single_char_alpha() {
        assert_eq!(parse_vkey("a"), Some(0x41));
        assert_eq!(parse_vkey("Z"), Some(0x5A));
    }

    #[test]
    fn parse_vkey_hex_literal() {
        // 0x70 = F1
        assert_eq!(parse_vkey("0x70"), Some(0x70));
        // 0x79 = F10
        assert_eq!(parse_vkey("0x79"), Some(0x79));
        // LBUTTON (0x01) 被注释移除 → 反向查表返回 "missing" → 失败
        assert_eq!(parse_vkey("0x01"), None);
    }

    #[test]
    fn parse_vkey_named_keys() {
        assert_eq!(parse_vkey("F10"), Some(0x79));
        assert_eq!(parse_vkey("VK_F10"), Some(0x79));
        assert_eq!(parse_vkey("alt"), Some(0x12));
        assert_eq!(parse_vkey("VK_CLEAR"), Some(0x0C));
        assert_eq!(parse_vkey("VK_RETURN"), Some(0x0D));
        assert_eq!(parse_vkey("VK_SPACE"), Some(0x20));
        assert_eq!(parse_vkey("ctrl"), Some(0x11));
        assert_eq!(parse_vkey("CTRL"), Some(0x11));
    }

    #[test]
    fn parse_vkey_symbol_keys() {
        assert_eq!(parse_vkey(";"), Some(0xBA));
        assert_eq!(parse_vkey("*"), Some(0x6A));
        assert_eq!(parse_vkey("-"), Some(0x6D));
        assert_eq!(parse_vkey("+"), Some(0x6B));
    }

    #[test]
    fn parse_vkey_legacy_mixed_case_never_matches() {
        // 原版固有行为：legacy 混合大小写键因 to_uppercase 查表永不命中
        assert_eq!(parse_vkey("Num 1"), None);
        assert_eq!(parse_vkey("Prnt Scrn"), None);
    }

    #[test]
    fn parse_vkey_invalid() {
        assert_eq!(parse_vkey("INVALID_KEY"), None);
        assert_eq!(parse_vkey(""), None);
        assert_eq!(parse_vkey("0xZZ"), None);
    }

    #[test]
    fn parse_combo_basic() {
        assert_eq!(parse_combo("no_modifiers 5").unwrap(), vec![0x35]);
        assert_eq!(parse_combo("alt 5").unwrap(), vec![0x12, 0x35]);
        assert_eq!(parse_combo("0x70").unwrap(), vec![0x70]);
        assert_eq!(parse_combo("VK_F10").unwrap(), vec![0x79]);
        assert_eq!(parse_combo("F10").unwrap(), vec![0x79]);
        assert_eq!(parse_combo("a").unwrap(), vec![0x41]);
    }

    #[test]
    fn parse_combo_shift_moved_to_end() {
        assert_eq!(parse_combo("SHIFT 5").unwrap(), vec![0x35, 0x10]);
        assert_eq!(parse_combo("shift ctrl F10").unwrap(), vec![0x11, 0x79, 0x10]);
        assert_eq!(parse_combo("no_ctrl shift F10").unwrap(), vec![0x79, 0x10]);
        assert_eq!(parse_combo("lshift rshift a").unwrap(), vec![0x41, 0xA0, 0xA1]);
    }

    #[test]
    fn parse_combo_skips_no_prefix() {
        assert_eq!(parse_combo("no_ctrl").unwrap(), Vec::<u16>::new());
        assert_eq!(parse_combo("no_modifiers no_ctrl no_alt 5").unwrap(), vec![0x35]);
    }

    #[test]
    fn parse_combo_too_many_keys() {
        // 11 个有效键 → 超限
        let combo = "1 2 3 4 5 6 7 8 9 0 a";
        assert!(parse_combo(combo).is_err());
    }

    #[test]
    fn parse_combo_invalid_token() {
        assert!(parse_combo("F10 NOT_A_KEY").is_err());
        assert!(parse_combo("0x01").is_err());
    }

    #[test]
    fn get_key_name_reverse_lookup() {
        assert_eq!(get_key_name(0x79), "F10");
        // 第一个匹配（BACK 在 BACKSPACE 之前）
        assert_eq!(get_key_name(0x08), "BACK");
        // 映射表外码值 → "missing"
        assert_eq!(get_key_name(0x01), "missing");
    }
}
