//! 按键模拟模块
//!
//! 该模块封装了 Windows 平台下的键盘与鼠标输入模拟功能，用于：
//! - 模拟单键按下/释放（如触发 3DMigoto 的快捷键）
//! - 模拟组合键（如 Ctrl+F）
//! - 模拟鼠标移动与点击
//!
//! 实现基于 Windows API 的 `keybd_event` 与 `mouse_event` 函数，
//! 非 Windows 平台会返回错误。所有公开方法均为异步，通过 `tokio::time::sleep`
//! 在按键间隔中引入延迟，模拟真实的人工输入节奏。

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};

// ===== Windows 虚拟键码（Virtual Key Codes）常量 =====
// 以下常量对应 Windows API 中定义的虚拟键码，用于 `keybd_event` 调用。

/// 鼠标左键（仅用于标识，不通过 keybd_event 发送）。
pub const VK_LBUTTON: u16 = 0x01;
/// 鼠标右键（仅用于标识，不通过 keybd_event 发送）。
pub const VK_RBUTTON: u16 = 0x02;
/// Cancel 键。
pub const VK_CANCEL: u16 = 0x03;
/// 鼠标中键（仅用于标识）。
pub const VK_MBUTTON: u16 = 0x04;
/// 退格键（Backspace）。
pub const VK_BACK: u16 = 0x08;
/// Tab 键。
pub const VK_TAB: u16 = 0x09;
/// Clear 键（数字键盘 5 在 NumLock 关闭时）。NRMM 默认用它作为「选择模组」的触发键。
pub const VK_CLEAR: u16 = 0x0C;
/// 回车键（Enter / Return）。
pub const VK_RETURN: u16 = 0x0D;
/// Shift 修饰键（左/右通用）。
pub const VK_SHIFT: u16 = 0x10;
/// Ctrl 修饰键（左/右通用）。
pub const VK_CONTROL: u16 = 0x11;
/// Alt 修饰键（左/右通用），Windows API 中称为 MENU。
pub const VK_MENU: u16 = 0x12;
/// Pause 键。
pub const VK_PAUSE: u16 = 0x13;
/// Caps Lock 键。
pub const VK_CAPITAL: u16 = 0x14;
/// Esc 键。
pub const VK_ESCAPE: u16 = 0x1B;
/// 空格键。
pub const VK_SPACE: u16 = 0x20;
/// Page Up 键。
pub const VK_PRIOR: u16 = 0x21;
/// Page Down 键。
pub const VK_NEXT: u16 = 0x22;
/// End 键。
pub const VK_END: u16 = 0x23;
/// Home 键。
pub const VK_HOME: u16 = 0x24;
/// 左方向键。
pub const VK_LEFT: u16 = 0x25;
/// 上方向键。
pub const VK_UP: u16 = 0x26;
/// 右方向键。
pub const VK_RIGHT: u16 = 0x27;
/// 下方向键。
pub const VK_DOWN: u16 = 0x28;
/// Select 键。
pub const VK_SELECT: u16 = 0x29;
/// Print 键。
pub const VK_PRINT: u16 = 0x2A;
/// Execute 键。
pub const VK_EXECUTE: u16 = 0x2B;
/// Print Screen 键。
pub const VK_SNAPSHOT: u16 = 0x2C;
/// Insert 键。
pub const VK_INSERT: u16 = 0x2D;
/// Delete 键。
pub const VK_DELETE: u16 = 0x2E;
/// Help 键。
pub const VK_HELP: u16 = 0x2F;

/// 数字键 0。
pub const VK_0: u16 = 0x30;
/// 数字键 1。
pub const VK_1: u16 = 0x31;
/// 数字键 2。
pub const VK_2: u16 = 0x32;
/// 数字键 3。
pub const VK_3: u16 = 0x33;
/// 数字键 4。
pub const VK_4: u16 = 0x34;
/// 数字键 5。
pub const VK_5: u16 = 0x35;
/// 数字键 6。
pub const VK_6: u16 = 0x36;
/// 数字键 7。
pub const VK_7: u16 = 0x37;
/// 数字键 8。
pub const VK_8: u16 = 0x38;
/// 数字键 9。
pub const VK_9: u16 = 0x39;

/// 字母 A 键。
pub const VK_A: u16 = 0x41;
/// 字母 B 键。
pub const VK_B: u16 = 0x42;
/// 字母 C 键。
pub const VK_C: u16 = 0x43;
/// 字母 D 键。
pub const VK_D: u16 = 0x44;
/// 字母 E 键。
pub const VK_E: u16 = 0x45;
/// 字母 F 键。
pub const VK_F: u16 = 0x46;
/// 字母 G 键。
pub const VK_G: u16 = 0x47;
/// 字母 H 键。
pub const VK_H: u16 = 0x48;
/// 字母 I 键。
pub const VK_I: u16 = 0x49;
/// 字母 J 键。
pub const VK_J: u16 = 0x4A;
/// 字母 K 键。
pub const VK_K: u16 = 0x4B;
/// 字母 L 键。
pub const VK_L: u16 = 0x4C;
/// 字母 M 键。
pub const VK_M: u16 = 0x4D;
/// 字母 N 键。
pub const VK_N: u16 = 0x4E;
/// 字母 O 键。
pub const VK_O: u16 = 0x4F;
/// 字母 P 键。
pub const VK_P: u16 = 0x50;
/// 字母 Q 键。
pub const VK_Q: u16 = 0x51;
/// 字母 R 键。
pub const VK_R: u16 = 0x52;
/// 字母 S 键。
pub const VK_S: u16 = 0x53;
/// 字母 T 键。
pub const VK_T: u16 = 0x54;
/// 字母 U 键。
pub const VK_U: u16 = 0x55;
/// 字母 V 键。
pub const VK_V: u16 = 0x56;
/// 字母 W 键。
pub const VK_W: u16 = 0x57;
/// 字母 X 键。
pub const VK_X: u16 = 0x58;
/// 字母 Y 键。
pub const VK_Y: u16 = 0x59;
/// 字母 Z 键。
pub const VK_Z: u16 = 0x5A;

/// 左 Windows 键。
pub const VK_LWIN: u16 = 0x5B;
/// 右 Windows 键。
pub const VK_RWIN: u16 = 0x5C;
/// Applications 键。
pub const VK_APPS: u16 = 0x5D;
/// Sleep 键。
pub const VK_SLEEP: u16 = 0x5F;

/// 小键盘数字键 0。
pub const VK_NUMPAD0: u16 = 0x60;
/// 小键盘数字键 1。
pub const VK_NUMPAD1: u16 = 0x61;
/// 小键盘数字键 2。
pub const VK_NUMPAD2: u16 = 0x62;
/// 小键盘数字键 3。
pub const VK_NUMPAD3: u16 = 0x63;
/// 小键盘数字键 4。
pub const VK_NUMPAD4: u16 = 0x64;
/// 小键盘数字键 5。
pub const VK_NUMPAD5: u16 = 0x65;
/// 小键盘数字键 6。
pub const VK_NUMPAD6: u16 = 0x66;
/// 小键盘数字键 7。
pub const VK_NUMPAD7: u16 = 0x67;
/// 小键盘数字键 8。
pub const VK_NUMPAD8: u16 = 0x68;
/// 小键盘数字键 9。
pub const VK_NUMPAD9: u16 = 0x69;
/// 小键盘乘号。
pub const VK_MULTIPLY: u16 = 0x6A;
/// 小键盘加号。
pub const VK_ADD: u16 = 0x6B;
/// 小键盘分隔符。
///
/// 当前未映射到键名，保留以保持 VK 常量表完整。
#[allow(dead_code)]
pub const VK_SEPARATOR: u16 = 0x6C;
/// 小键盘减号。
pub const VK_SUBTRACT: u16 = 0x6D;
/// 小键盘小数点。
pub const VK_DECIMAL: u16 = 0x6E;
/// 小键盘除号。
pub const VK_DIVIDE: u16 = 0x6F;

/// F1 功能键。
pub const VK_F1: u16 = 0x70;
/// F2 功能键。
pub const VK_F2: u16 = 0x71;
/// F3 功能键。
pub const VK_F3: u16 = 0x72;
/// F4 功能键。
pub const VK_F4: u16 = 0x73;
/// F5 功能键。
pub const VK_F5: u16 = 0x74;
/// F6 功能键。
pub const VK_F6: u16 = 0x75;
/// F7 功能键。
pub const VK_F7: u16 = 0x76;
/// F8 功能键。
pub const VK_F8: u16 = 0x77;
/// F9 功能键。
pub const VK_F9: u16 = 0x78;
/// F10 功能键。NRMM 默认用它作为「重载模组」的触发键。
pub const VK_F10: u16 = 0x79;
/// F11 功能键。
pub const VK_F11: u16 = 0x7A;
/// F12 功能键。
pub const VK_F12: u16 = 0x7B;
/// F13 功能键。
pub const VK_F13: u16 = 0x7C;
/// F14 功能键。
pub const VK_F14: u16 = 0x7D;
/// F15 功能键。
pub const VK_F15: u16 = 0x7E;
/// F16 功能键。
pub const VK_F16: u16 = 0x7F;
/// F17 功能键。
pub const VK_F17: u16 = 0x80;
/// F18 功能键。
pub const VK_F18: u16 = 0x81;
/// F19 功能键。
pub const VK_F19: u16 = 0x82;
/// F20 功能键。
pub const VK_F20: u16 = 0x83;
/// F21 功能键。
pub const VK_F21: u16 = 0x84;
/// F22 功能键。
pub const VK_F22: u16 = 0x85;
/// F23 功能键。
pub const VK_F23: u16 = 0x86;
/// F24 功能键。
pub const VK_F24: u16 = 0x87;

/// Num Lock 键。
pub const VK_NUMLOCK: u16 = 0x90;
/// Scroll Lock 键。
pub const VK_SCROLL: u16 = 0x91;

/// 左 Shift 键。
pub const VK_LSHIFT: u16 = 0xA0;
/// 右 Shift 键。
pub const VK_RSHIFT: u16 = 0xA1;
/// 左 Ctrl 键。
pub const VK_LCONTROL: u16 = 0xA2;
/// 右 Ctrl 键。
pub const VK_RCONTROL: u16 = 0xA3;
/// 左 Alt 键。
pub const VK_LMENU: u16 = 0xA4;
/// 右 Alt 键。
pub const VK_RMENU: u16 = 0xA5;

/// OEM 1 键（`;:`）。
pub const VK_OEM_1: u16 = 0xBA;
/// OEM Plus 键（`=+`）。
pub const VK_OEM_PLUS: u16 = 0xBB;
/// OEM Comma 键（`,<`）。
pub const VK_OEM_COMMA: u16 = 0xBC;
/// OEM Minus 键（`-_`）。
pub const VK_OEM_MINUS: u16 = 0xBD;
/// OEM Period 键（`.>`）。
pub const VK_OEM_PERIOD: u16 = 0xBE;
/// OEM 2 键（`/?`）。
pub const VK_OEM_2: u16 = 0xBF;
/// OEM 3 键（`` `~ ``）。
pub const VK_OEM_3: u16 = 0xC0;
/// OEM 4 键（`[{`）。
pub const VK_OEM_4: u16 = 0xDB;
/// OEM 5 键（`\|`）。
pub const VK_OEM_5: u16 = 0xDC;
/// OEM 6 键（`]}`）。
pub const VK_OEM_6: u16 = 0xDD;
/// OEM 7 键（`'"`）。
pub const VK_OEM_7: u16 = 0xDE;
/// OEM 8 键。
pub const VK_OEM_8: u16 = 0xDF;
/// OEM 102 键（部分键盘的 `<>` 键）。
pub const VK_OEM_102: u16 = 0xE2;

/// 鼠标按键枚举。
///
/// 用于 `simulate_mouse_click` 指定要点击的鼠标按键。
#[allow(dead_code)]
pub enum MouseButton {
    /// 鼠标左键。
    Left,
    /// 鼠标右键。
    Right,
    /// 鼠标中键（滚轮按下）。
    Middle,
}

/// 按键模拟器结构体。
///
/// 内部维护一个「键名 → 虚拟键码」的映射表，支持通过字符串名称（如 `"f10"`、`"ctrl"`）
/// 查找对应的虚拟键码，便于从前端接收按键配置。
pub struct KeypressSimulator {
    /// 键名（小写）到虚拟键码的映射表。
    key_name_map: HashMap<String, u16>,
}

impl KeypressSimulator {
    /// 创建一个新的 `KeypressSimulator` 实例。
    ///
    /// 会预先注册常用键名到映射表中，覆盖字母键、数字键、方向键、功能键、
    /// 小键盘键、控制键、OEM 符号键及修饰键，并包含 3DMigoto / NRMM 常用的别名。
    pub fn new() -> Self {
        let mut key_name_map = HashMap::new();

        // 鼠标按键标识（仅用于键名解析，不通过 keybd_event 发送）
        key_name_map.insert("lbutton".to_string(), VK_LBUTTON);
        key_name_map.insert("lclick".to_string(), VK_LBUTTON);
        key_name_map.insert("leftclick".to_string(), VK_LBUTTON);
        key_name_map.insert("rbutton".to_string(), VK_RBUTTON);
        key_name_map.insert("rclick".to_string(), VK_RBUTTON);
        key_name_map.insert("rightclick".to_string(), VK_RBUTTON);
        key_name_map.insert("mbutton".to_string(), VK_MBUTTON);
        key_name_map.insert("mclick".to_string(), VK_MBUTTON);
        key_name_map.insert("middleclick".to_string(), VK_MBUTTON);

        // 控制键
        key_name_map.insert("cancel".to_string(), VK_CANCEL);
        key_name_map.insert("back".to_string(), VK_BACK);
        key_name_map.insert("backspace".to_string(), VK_BACK);
        key_name_map.insert("tab".to_string(), VK_TAB);
        key_name_map.insert("clear".to_string(), VK_CLEAR);
        key_name_map.insert("enter".to_string(), VK_RETURN);
        key_name_map.insert("return".to_string(), VK_RETURN);
        key_name_map.insert("shift".to_string(), VK_SHIFT);
        key_name_map.insert("ctrl".to_string(), VK_CONTROL);
        key_name_map.insert("control".to_string(), VK_CONTROL);
        key_name_map.insert("alt".to_string(), VK_MENU);
        key_name_map.insert("menu".to_string(), VK_MENU);
        key_name_map.insert("pause".to_string(), VK_PAUSE);
        key_name_map.insert("capslock".to_string(), VK_CAPITAL);
        key_name_map.insert("capital".to_string(), VK_CAPITAL);
        key_name_map.insert("esc".to_string(), VK_ESCAPE);
        key_name_map.insert("escape".to_string(), VK_ESCAPE);
        key_name_map.insert("space".to_string(), VK_SPACE);
        key_name_map.insert("spacebar".to_string(), VK_SPACE);
        key_name_map.insert("pageup".to_string(), VK_PRIOR);
        key_name_map.insert("prior".to_string(), VK_PRIOR);
        key_name_map.insert("pagedown".to_string(), VK_NEXT);
        key_name_map.insert("next".to_string(), VK_NEXT);
        key_name_map.insert("end".to_string(), VK_END);
        key_name_map.insert("home".to_string(), VK_HOME);
        key_name_map.insert("left".to_string(), VK_LEFT);
        key_name_map.insert("up".to_string(), VK_UP);
        key_name_map.insert("right".to_string(), VK_RIGHT);
        key_name_map.insert("down".to_string(), VK_DOWN);
        key_name_map.insert("select".to_string(), VK_SELECT);
        key_name_map.insert("print".to_string(), VK_PRINT);
        key_name_map.insert("execute".to_string(), VK_EXECUTE);
        key_name_map.insert("snapshot".to_string(), VK_SNAPSHOT);
        key_name_map.insert("printscreen".to_string(), VK_SNAPSHOT);
        key_name_map.insert("prntscrn".to_string(), VK_SNAPSHOT);
        key_name_map.insert("prtsc".to_string(), VK_SNAPSHOT);
        key_name_map.insert("insert".to_string(), VK_INSERT);
        key_name_map.insert("delete".to_string(), VK_DELETE);
        key_name_map.insert("del".to_string(), VK_DELETE);
        key_name_map.insert("help".to_string(), VK_HELP);
        key_name_map.insert("lwin".to_string(), VK_LWIN);
        key_name_map.insert("rwin".to_string(), VK_RWIN);
        key_name_map.insert("apps".to_string(), VK_APPS);
        key_name_map.insert("sleep".to_string(), VK_SLEEP);
        key_name_map.insert("numlock".to_string(), VK_NUMLOCK);
        key_name_map.insert("scrolllock".to_string(), VK_SCROLL);
        key_name_map.insert("scroll".to_string(), VK_SCROLL);
        key_name_map.insert("lshift".to_string(), VK_LSHIFT);
        key_name_map.insert("leftshift".to_string(), VK_LSHIFT);
        key_name_map.insert("rshift".to_string(), VK_RSHIFT);
        key_name_map.insert("rightshift".to_string(), VK_RSHIFT);
        key_name_map.insert("lctrl".to_string(), VK_LCONTROL);
        key_name_map.insert("leftctrl".to_string(), VK_LCONTROL);
        key_name_map.insert("rctrl".to_string(), VK_RCONTROL);
        key_name_map.insert("rightctrl".to_string(), VK_RCONTROL);
        key_name_map.insert("lalt".to_string(), VK_LMENU);
        key_name_map.insert("leftalt".to_string(), VK_LMENU);
        key_name_map.insert("ralt".to_string(), VK_RMENU);
        key_name_map.insert("rightalt".to_string(), VK_RMENU);

        // 主键盘数字键 0-9
        key_name_map.insert("0".to_string(), VK_0);
        key_name_map.insert("1".to_string(), VK_1);
        key_name_map.insert("2".to_string(), VK_2);
        key_name_map.insert("3".to_string(), VK_3);
        key_name_map.insert("4".to_string(), VK_4);
        key_name_map.insert("5".to_string(), VK_5);
        key_name_map.insert("6".to_string(), VK_6);
        key_name_map.insert("7".to_string(), VK_7);
        key_name_map.insert("8".to_string(), VK_8);
        key_name_map.insert("9".to_string(), VK_9);

        // 字母键 A-Z
        key_name_map.insert("a".to_string(), VK_A);
        key_name_map.insert("b".to_string(), VK_B);
        key_name_map.insert("c".to_string(), VK_C);
        key_name_map.insert("d".to_string(), VK_D);
        key_name_map.insert("e".to_string(), VK_E);
        key_name_map.insert("f".to_string(), VK_F);
        key_name_map.insert("g".to_string(), VK_G);
        key_name_map.insert("h".to_string(), VK_H);
        key_name_map.insert("i".to_string(), VK_I);
        key_name_map.insert("j".to_string(), VK_J);
        key_name_map.insert("k".to_string(), VK_K);
        key_name_map.insert("l".to_string(), VK_L);
        key_name_map.insert("m".to_string(), VK_M);
        key_name_map.insert("n".to_string(), VK_N);
        key_name_map.insert("o".to_string(), VK_O);
        key_name_map.insert("p".to_string(), VK_P);
        key_name_map.insert("q".to_string(), VK_Q);
        key_name_map.insert("r".to_string(), VK_R);
        key_name_map.insert("s".to_string(), VK_S);
        key_name_map.insert("t".to_string(), VK_T);
        key_name_map.insert("u".to_string(), VK_U);
        key_name_map.insert("v".to_string(), VK_V);
        key_name_map.insert("w".to_string(), VK_W);
        key_name_map.insert("x".to_string(), VK_X);
        key_name_map.insert("y".to_string(), VK_Y);
        key_name_map.insert("z".to_string(), VK_Z);

        // 小键盘数字键
        key_name_map.insert("num0".to_string(), VK_NUMPAD0);
        key_name_map.insert("num1".to_string(), VK_NUMPAD1);
        key_name_map.insert("num2".to_string(), VK_NUMPAD2);
        key_name_map.insert("num3".to_string(), VK_NUMPAD3);
        key_name_map.insert("num4".to_string(), VK_NUMPAD4);
        key_name_map.insert("num5".to_string(), VK_NUMPAD5);
        key_name_map.insert("num6".to_string(), VK_NUMPAD6);
        key_name_map.insert("num7".to_string(), VK_NUMPAD7);
        key_name_map.insert("num8".to_string(), VK_NUMPAD8);
        key_name_map.insert("num9".to_string(), VK_NUMPAD9);
        key_name_map.insert("numpad0".to_string(), VK_NUMPAD0);
        key_name_map.insert("numpad1".to_string(), VK_NUMPAD1);
        key_name_map.insert("numpad2".to_string(), VK_NUMPAD2);
        key_name_map.insert("numpad3".to_string(), VK_NUMPAD3);
        key_name_map.insert("numpad4".to_string(), VK_NUMPAD4);
        key_name_map.insert("numpad5".to_string(), VK_NUMPAD5);
        key_name_map.insert("numpad6".to_string(), VK_NUMPAD6);
        key_name_map.insert("numpad7".to_string(), VK_NUMPAD7);
        key_name_map.insert("numpad8".to_string(), VK_NUMPAD8);
        key_name_map.insert("numpad9".to_string(), VK_NUMPAD9);
        key_name_map.insert("num *".to_string(), VK_MULTIPLY);
        key_name_map.insert("num*".to_string(), VK_MULTIPLY);
        key_name_map.insert("multiply".to_string(), VK_MULTIPLY);
        key_name_map.insert("num +".to_string(), VK_ADD);
        key_name_map.insert("num+".to_string(), VK_ADD);
        key_name_map.insert("add".to_string(), VK_ADD);
        key_name_map.insert("num -".to_string(), VK_SUBTRACT);
        key_name_map.insert("num-".to_string(), VK_SUBTRACT);
        key_name_map.insert("subtract".to_string(), VK_SUBTRACT);
        key_name_map.insert("num .".to_string(), VK_DECIMAL);
        key_name_map.insert("num.".to_string(), VK_DECIMAL);
        key_name_map.insert("decimal".to_string(), VK_DECIMAL);
        key_name_map.insert("num /".to_string(), VK_DIVIDE);
        key_name_map.insert("num/".to_string(), VK_DIVIDE);
        key_name_map.insert("divide".to_string(), VK_DIVIDE);
        key_name_map.insert("num enter".to_string(), VK_RETURN);
        key_name_map.insert("numenter".to_string(), VK_RETURN);

        // 功能键 F1-F24
        key_name_map.insert("f1".to_string(), VK_F1);
        key_name_map.insert("f2".to_string(), VK_F2);
        key_name_map.insert("f3".to_string(), VK_F3);
        key_name_map.insert("f4".to_string(), VK_F4);
        key_name_map.insert("f5".to_string(), VK_F5);
        key_name_map.insert("f6".to_string(), VK_F6);
        key_name_map.insert("f7".to_string(), VK_F7);
        key_name_map.insert("f8".to_string(), VK_F8);
        key_name_map.insert("f9".to_string(), VK_F9);
        key_name_map.insert("f10".to_string(), VK_F10);
        key_name_map.insert("f11".to_string(), VK_F11);
        key_name_map.insert("f12".to_string(), VK_F12);
        key_name_map.insert("f13".to_string(), VK_F13);
        key_name_map.insert("f14".to_string(), VK_F14);
        key_name_map.insert("f15".to_string(), VK_F15);
        key_name_map.insert("f16".to_string(), VK_F16);
        key_name_map.insert("f17".to_string(), VK_F17);
        key_name_map.insert("f18".to_string(), VK_F18);
        key_name_map.insert("f19".to_string(), VK_F19);
        key_name_map.insert("f20".to_string(), VK_F20);
        key_name_map.insert("f21".to_string(), VK_F21);
        key_name_map.insert("f22".to_string(), VK_F22);
        key_name_map.insert("f23".to_string(), VK_F23);
        key_name_map.insert("f24".to_string(), VK_F24);

        // OEM 符号键
        key_name_map.insert(";".to_string(), VK_OEM_1);
        key_name_map.insert("oem1".to_string(), VK_OEM_1);
        key_name_map.insert("=".to_string(), VK_OEM_PLUS);
        key_name_map.insert("+".to_string(), VK_OEM_PLUS);
        key_name_map.insert("oemplus".to_string(), VK_OEM_PLUS);
        key_name_map.insert(",".to_string(), VK_OEM_COMMA);
        key_name_map.insert("oemcomma".to_string(), VK_OEM_COMMA);
        key_name_map.insert("-".to_string(), VK_OEM_MINUS);
        key_name_map.insert("oemminus".to_string(), VK_OEM_MINUS);
        key_name_map.insert(".".to_string(), VK_OEM_PERIOD);
        key_name_map.insert("oemperiod".to_string(), VK_OEM_PERIOD);
        key_name_map.insert("/".to_string(), VK_OEM_2);
        key_name_map.insert("oem2".to_string(), VK_OEM_2);
        key_name_map.insert("`".to_string(), VK_OEM_3);
        key_name_map.insert("~".to_string(), VK_OEM_3);
        key_name_map.insert("oem3".to_string(), VK_OEM_3);
        key_name_map.insert("[".to_string(), VK_OEM_4);
        key_name_map.insert("oem4".to_string(), VK_OEM_4);
        key_name_map.insert("\\".to_string(), VK_OEM_5);
        key_name_map.insert("oem5".to_string(), VK_OEM_5);
        key_name_map.insert("]".to_string(), VK_OEM_6);
        key_name_map.insert("oem6".to_string(), VK_OEM_6);
        key_name_map.insert("'".to_string(), VK_OEM_7);
        key_name_map.insert("\"".to_string(), VK_OEM_7);
        key_name_map.insert("oem7".to_string(), VK_OEM_7);
        key_name_map.insert("oem8".to_string(), VK_OEM_8);
        key_name_map.insert("oem102".to_string(), VK_OEM_102);

        Self { key_name_map }
    }

    /// 将键名转换为虚拟键码。
    ///
    /// 查找顺序：
    /// 1. 在预设映射表中按名称（不区分大小写）查找。
    /// 2. 尝试将键名解析为数字（支持直接传入数字形式的键码）。
    ///
    /// 参数：
    /// - `key_name`: 键名（如 `"f10"`、`"ctrl"` 或 `"0x79"`）。
    ///
    /// 返回：对应的虚拟键码。无法识别时返回 `anyhow::Error`。
    pub fn key_name_to_vk(&self, key_name: &str) -> Result<u16> {
        let lower = key_name.to_lowercase();
        if let Some(&vk) = self.key_name_map.get(&lower) {
            return Ok(vk);
        }

        // 支持 0x 十六进制前缀，例如 "0x41" → 65
        if let Some(hex) = lower.strip_prefix("0x") {
            if let Ok(vk) = u16::from_str_radix(hex, 16) {
                return Ok(vk);
            }
        }

        // 支持 VK_ 前缀，例如 "VK_A" / "vk_a" → 65，"VK_0x79" → 121
        if let Some(vk_suffix) = lower.strip_prefix("vk_") {
            if let Some(&vk) = self.key_name_map.get(vk_suffix) {
                return Ok(vk);
            }
            if let Some(hex) = vk_suffix.strip_prefix("0x") {
                if let Ok(vk) = u16::from_str_radix(hex, 16) {
                    return Ok(vk);
                }
            }
            if let Ok(vk) = vk_suffix.parse::<u16>() {
                return Ok(vk);
            }
        }

        // 支持直接传入数字形式的键码
        if let Ok(vk) = lower.parse::<u16>() {
            return Ok(vk);
        }

        anyhow::bail!("Unknown key name: {}", key_name)
    }

    /// 将虚拟键码转换为可读的键名（反向查询）。
    ///
    /// 参数：
    /// - `vk_code`: 虚拟键码。
    ///
    /// 返回：键名字符串；未注册的键码返回 `"VK_<code>"` 形式。
    #[allow(dead_code)]
    pub fn get_key_name(vk_code: u16) -> String {
        match vk_code {
            VK_CLEAR => "Clear".to_string(),
            VK_SPACE => "Space".to_string(),
            VK_RETURN => "Enter".to_string(),
            VK_ESCAPE => "Esc".to_string(),
            VK_BACK => "Backspace".to_string(),
            VK_TAB => "Tab".to_string(),
            VK_LEFT => "Left".to_string(),
            VK_UP => "Up".to_string(),
            VK_RIGHT => "Right".to_string(),
            VK_DOWN => "Down".to_string(),
            VK_PRIOR => "PageUp".to_string(),
            VK_NEXT => "PageDown".to_string(),
            VK_HOME => "Home".to_string(),
            VK_END => "End".to_string(),
            VK_INSERT => "Insert".to_string(),
            VK_DELETE => "Delete".to_string(),
            VK_SNAPSHOT => "PrintScreen".to_string(),
            VK_F10 => "F10".to_string(),
            VK_SHIFT => "Shift".to_string(),
            VK_CONTROL => "Ctrl".to_string(),
            VK_MENU => "Alt".to_string(),
            VK_0..=VK_9 => ((vk_code - VK_0 + b'0' as u16) as u8 as char).to_string(),
            VK_A..=VK_Z => ((vk_code - VK_A + b'A' as u16) as u8 as char).to_string(),
            VK_F1..=VK_F24 => format!("F{}", vk_code - VK_F1 + 1),
            VK_NUMPAD0..=VK_NUMPAD9 => format!("Num{}", vk_code - VK_NUMPAD0),
            _ => format!("VK_{}", vk_code),
        }
    }

    /// 模拟单键按下并释放（完整的一次按键动作）。
    ///
    /// 参数：
    /// - `key`: 键名（通过 `key_name_to_vk` 解析）。
    ///
    /// 返回：成功返回 `Ok(())`，失败返回 `anyhow::Error`。
    pub async fn simulate_key_press(&self, key: &str) -> Result<()> {
        let vk_code = self.key_name_to_vk(key)?;
        self.simulate_key_press_internal(vk_code).await
    }

    /// 模拟单键按下并释放（直接使用虚拟键码）。
    ///
    /// 参数：
    /// - `vk_code`: 虚拟键码。
    #[allow(dead_code)]
    pub async fn simulate_key_press_vk(&self, vk_code: u16) -> Result<()> {
        self.simulate_key_press_internal(vk_code).await
    }

    /// 单键按压的内部实现：按下 → 等待 50ms → 释放。
    ///
    /// 50ms 的间隔用于模拟真实按键时长，确保目标程序能正确识别按键事件。
    async fn simulate_key_press_internal(&self, vk_code: u16) -> Result<()> {
        self.simulate_key_down(vk_code).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.simulate_key_up(vk_code).await?;
        Ok(())
    }

    /// 模拟按键按下（不释放）。
    ///
    /// 在 Windows 上调用 `keybd_event`，flags 为 0 表示按下。
    ///
    /// 参数：
    /// - `vk_code`: 虚拟键码。
    pub async fn simulate_key_down(&self, vk_code: u16) -> Result<()> {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS;
            Self::keybd_event_windows(vk_code as u8, 0, KEYBD_EVENT_FLAGS(0), 0)
                .context("Failed to simulate key down")?;
        }
        #[cfg(not(windows))]
        {
            let _ = vk_code;
            anyhow::bail!("按键模拟仅支持 Windows 平台");
        }
        Ok(())
    }

    /// 模拟按键释放。
    ///
    /// 在 Windows 上调用 `keybd_event`，flags 为 `KEYEVENTF_KEYUP` 表示释放。
    ///
    /// 参数：
    /// - `vk_code`: 虚拟键码。
    pub async fn simulate_key_up(&self, vk_code: u16) -> Result<()> {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP;
            Self::keybd_event_windows(vk_code as u8, 0, KEYEVENTF_KEYUP, 0)
                .context("Failed to simulate key up")?;
        }
        #[cfg(not(windows))]
        {
            let _ = vk_code;
            anyhow::bail!("按键模拟仅支持 Windows 平台");
        }
        Ok(())
    }

    /// 模拟组合键（如 Ctrl+Shift+F）。
    ///
    /// 流程：
    /// 1. 按列表顺序依次按下每个键（每个间隔 10ms）。
    /// 2. 全部按下后等待 50ms。
    /// 3. 按相反顺序依次释放每个键（每个间隔 10ms）。
    ///
    /// 参数：
    /// - `keys`: 键名列表，按按下顺序排列（修饰键通常在前）。
    ///
    /// 返回：成功返回 `Ok(())`，任一键无法识别或模拟失败时返回错误。
    pub async fn simulate_key_combination(&self, keys: Vec<String>) -> Result<()> {
        let mut vk_codes = Vec::new();
        for key in &keys {
            let vk = self.key_name_to_vk(key)?;
            vk_codes.push(vk);
        }
        self.simulate_key_combination_internal(&vk_codes).await
    }

    /// 模拟组合键（直接使用虚拟键码列表）。
    ///
    /// 参数：
    /// - `vk_codes`: 虚拟键码列表。
    #[allow(dead_code)]
    pub async fn simulate_key_combination_vk(&self, vk_codes: &[u16]) -> Result<()> {
        self.simulate_key_combination_internal(vk_codes).await
    }

    /// 组合键的内部实现。
    ///
    /// 依次按下所有键 → 等待 50ms → 逆序依次释放所有键。
    async fn simulate_key_combination_internal(&self, vk_codes: &[u16]) -> Result<()> {
        // 按顺序按下所有键
        for &vk in vk_codes {
            self.simulate_key_down(vk).await?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // 保持 50ms 的按下状态
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 逆序释放所有键
        for &vk in vk_codes.iter().rev() {
            self.simulate_key_up(vk).await?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        Ok(())
    }

    /// 模拟鼠标相对移动。
    ///
    /// 在 Windows 上调用 `mouse_event`，使用 `MOUSEEVENTF_MOVE` 标志，
    /// `dx`/`dy` 为相对移动量（非绝对坐标）。
    ///
    /// 参数：
    /// - `dx`: 水平方向相对移动量（正值向右，负值向左）。
    /// - `dy`: 垂直方向相对移动量（正值向下，负值向上）。
    ///
    /// 返回：成功返回 `Ok(())`，非 Windows 平台返回错误。
    pub async fn simulate_mouse_move(&self, dx: i32, dy: i32) -> Result<()> {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_MOVE;
            Self::mouse_event_windows(MOUSEEVENTF_MOVE, dx, dy, 0, 0)
                .context("Failed to simulate mouse move")?;
        }
        #[cfg(not(windows))]
        {
            let _ = dx;
            let _ = dy;
            anyhow::bail!("鼠标模拟仅支持 Windows 平台");
        }
        Ok(())
    }

    /// 模拟鼠标点击（按下 → 50ms → 释放）。
    ///
    /// 参数：
    /// - `button`: 要点击的鼠标按键。
    #[allow(dead_code)]
    pub async fn simulate_mouse_click(&self, button: MouseButton) -> Result<()> {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN,
                MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
            };

            let (down_flags, up_flags) = match button {
                MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
                MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
                MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
            };

            Self::mouse_event_windows(down_flags, 0, 0, 0, 0)
                .context("Failed to simulate mouse button down")?;

            tokio::time::sleep(Duration::from_millis(50)).await;

            Self::mouse_event_windows(up_flags, 0, 0, 0, 0)
                .context("Failed to simulate mouse button up")?;
        }
        #[cfg(not(windows))]
        {
            let _ = button;
            anyhow::bail!("鼠标模拟仅支持 Windows 平台");
        }
        Ok(())
    }

    /// 模拟「选择模组」的按键序列（按下 Clear 键）。
    ///
    /// 对应 3DMigoto 中绑定的模组选择快捷键。
    #[allow(dead_code)]
    pub async fn select_mod_key_sequence(&self) -> Result<()> {
        self.simulate_key_press_vk(VK_CLEAR).await
    }

    /// 模拟「重载模组」的按键序列（按下 F10 键）。
    ///
    /// 对应 3DMigoto 的 F10 重载快捷键。
    #[allow(dead_code)]
    pub async fn reload_mod_key_sequence(&self) -> Result<()> {
        self.simulate_key_press_vk(VK_F10).await
    }

    /// Windows 平台 `keybd_event` 的封装。
    ///
    /// 参数对应 Windows API 的 `keybd_event` 签名：
    /// - `b_vk`: 虚拟键码。
    /// - `b_scan`: 硬件扫描码（通常为 0）。
    /// - `dw_flags`: 事件标志（0=按下，KEYEVENTF_KEYUP=释放）。
    /// - `dw_extra_info`: 附加信息（通常为 0）。
    #[cfg(windows)]
    fn keybd_event_windows(
        b_vk: u8,
        b_scan: u8,
        dw_flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
        dw_extra_info: usize,
    ) -> Result<()> {
        use windows::Win32::UI::Input::KeyboardAndMouse::keybd_event;

        unsafe {
            keybd_event(b_vk, b_scan, dw_flags, dw_extra_info);
        }

        Ok(())
    }

    /// Windows 平台 `mouse_event` 的封装。
    ///
    /// 参数对应 Windows API 的 `mouse_event` 签名：
    /// - `dw_flags`: 事件标志（如 MOUSEEVENTF_MOVE、MOUSEEVENTF_LEFTDOWN 等）。
    /// - `dx`/`dy`: 移动量或绝对坐标。
    /// - `dw_data`: 滚轮滚动量等附加数据。
    /// - `dw_extra_info`: 附加信息。
    #[cfg(windows)]
    fn mouse_event_windows(
        dw_flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
        dx: i32,
        dy: i32,
        dw_data: i32,
        dw_extra_info: usize,
    ) -> Result<()> {
        use windows::Win32::UI::Input::KeyboardAndMouse::mouse_event;

        unsafe {
            mouse_event(dw_flags, dx, dy, dw_data, dw_extra_info);
        }

        Ok(())
    }
}

impl Default for KeypressSimulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_name_to_vk_hex_and_vk_prefix() {
        let sim = KeypressSimulator::new();
        assert_eq!(sim.key_name_to_vk("0x41").unwrap(), VK_A);
        assert_eq!(sim.key_name_to_vk("VK_A").unwrap(), VK_A);
        assert_eq!(sim.key_name_to_vk("vk_a").unwrap(), VK_A);
        assert_eq!(sim.key_name_to_vk("0x10").unwrap(), VK_SHIFT);
        assert_eq!(sim.key_name_to_vk("VK_0x79").unwrap(), VK_F10);
    }

    #[test]
    fn test_get_key_name_extended() {
        assert_eq!(KeypressSimulator::get_key_name(VK_A), "A".to_string());
        assert_eq!(KeypressSimulator::get_key_name(VK_5), "5".to_string());
        assert_eq!(KeypressSimulator::get_key_name(VK_F10), "F10".to_string());
        assert_eq!(KeypressSimulator::get_key_name(VK_F1), "F1".to_string());
        assert_eq!(KeypressSimulator::get_key_name(VK_NUMPAD5), "Num5".to_string());
        assert_eq!(KeypressSimulator::get_key_name(VK_DELETE), "Delete".to_string());
        assert_eq!(KeypressSimulator::get_key_name(VK_SNAPSHOT), "PrintScreen".to_string());
    }

    #[test]
    fn test_key_name_map_aliases() {
        let sim = KeypressSimulator::new();
        assert_eq!(sim.key_name_to_vk("printscreen").unwrap(), VK_SNAPSHOT);
        assert_eq!(sim.key_name_to_vk("prntscrn").unwrap(), VK_SNAPSHOT);
        assert_eq!(sim.key_name_to_vk("backspace").unwrap(), VK_BACK);
        assert_eq!(sim.key_name_to_vk("enter").unwrap(), VK_RETURN);
        assert_eq!(sim.key_name_to_vk("space").unwrap(), VK_SPACE);
    }
}
