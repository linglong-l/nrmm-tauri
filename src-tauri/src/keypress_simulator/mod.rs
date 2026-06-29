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

/// Clear 键（数字键盘 5 在 NumLock 关闭时）。NRMM 默认用它作为「选择模组」的触发键。
pub const VK_CLEAR: u16 = 0x0C;
/// 空格键。
pub const VK_SPACE: u16 = 0x20;
/// 回车键（Enter / Return）。
pub const VK_RETURN: u16 = 0x0D;
/// 左方向键。
pub const VK_LEFT: u16 = 0x25;
/// 上方向键。
pub const VK_UP: u16 = 0x26;
/// 右方向键。
pub const VK_RIGHT: u16 = 0x27;
/// 下方向键。
pub const VK_DOWN: u16 = 0x28;
/// 字母 F 键。
pub const VK_F: u16 = 0x46;
/// 字母 R 键。
pub const VK_R: u16 = 0x52;
/// 字母 Q 键。
pub const VK_Q: u16 = 0x51;
/// 字母 E 键。
pub const VK_E: u16 = 0x45;
/// 字母 W 键。
pub const VK_W: u16 = 0x57;
/// 字母 A 键。
pub const VK_A: u16 = 0x41;
/// 字母 S 键。
pub const VK_S: u16 = 0x53;
/// 字母 D 键。
pub const VK_D: u16 = 0x44;
/// F10 功能键。NRMM 默认用它作为「重载模组」的触发键。
pub const VK_F10: u16 = 0x79;
/// Shift 修饰键（左/右通用）。
pub const VK_SHIFT: u16 = 0x10;
/// Ctrl 修饰键（左/右通用）。
pub const VK_CONTROL: u16 = 0x11;
/// Alt 修饰键（左/右通用），Windows API 中称为 MENU。
pub const VK_MENU: u16 = 0x12;

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
    /// 会预先注册常用键名到映射表中，包括字母键、方向键、功能键及修饰键。
    pub fn new() -> Self {
        let mut key_name_map = HashMap::new();

        key_name_map.insert("clear".to_string(), VK_CLEAR);
        key_name_map.insert("space".to_string(), VK_SPACE);
        key_name_map.insert("enter".to_string(), VK_RETURN);
        key_name_map.insert("return".to_string(), VK_RETURN);
        key_name_map.insert("left".to_string(), VK_LEFT);
        key_name_map.insert("up".to_string(), VK_UP);
        key_name_map.insert("right".to_string(), VK_RIGHT);
        key_name_map.insert("down".to_string(), VK_DOWN);
        key_name_map.insert("f".to_string(), VK_F);
        key_name_map.insert("r".to_string(), VK_R);
        key_name_map.insert("q".to_string(), VK_Q);
        key_name_map.insert("e".to_string(), VK_E);
        key_name_map.insert("w".to_string(), VK_W);
        key_name_map.insert("a".to_string(), VK_A);
        key_name_map.insert("s".to_string(), VK_S);
        key_name_map.insert("d".to_string(), VK_D);
        key_name_map.insert("f10".to_string(), VK_F10);
        key_name_map.insert("shift".to_string(), VK_SHIFT);
        key_name_map.insert("ctrl".to_string(), VK_CONTROL);
        key_name_map.insert("control".to_string(), VK_CONTROL);
        key_name_map.insert("alt".to_string(), VK_MENU);
        key_name_map.insert("menu".to_string(), VK_MENU);

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
            VK_LEFT => "Left".to_string(),
            VK_UP => "Up".to_string(),
            VK_RIGHT => "Right".to_string(),
            VK_DOWN => "Down".to_string(),
            VK_F => "F".to_string(),
            VK_R => "R".to_string(),
            VK_Q => "Q".to_string(),
            VK_E => "E".to_string(),
            VK_W => "W".to_string(),
            VK_A => "A".to_string(),
            VK_S => "S".to_string(),
            VK_D => "D".to_string(),
            VK_F10 => "F10".to_string(),
            VK_SHIFT => "Shift".to_string(),
            VK_CONTROL => "Ctrl".to_string(),
            VK_MENU => "Alt".to_string(),
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
            anyhow::bail!("Keypress simulation is only supported on Windows");
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
            anyhow::bail!("Keypress simulation is only supported on Windows");
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
            anyhow::bail!("Mouse simulation is only supported on Windows");
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
            anyhow::bail!("Mouse simulation is only supported on Windows");
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
