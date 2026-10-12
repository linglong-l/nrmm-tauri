//! 手柄切换热键（`settings.gamepadHotkeyToggle`）
//!
//! - Windows：轮询 4 个 XInput 用户槽，检测配置组合（`LB+RB` / `LB+Start` / `RB+Back`）
//!   从「未满足」到「全部按下」的边沿，命中即触发与键盘窗口热键相同的显隐逻辑。
//! - 其它平台：占位实现，只记录一次 warn，不启动线程。
//!   Linux 侧不做轮询的原因是没有统一入口：evdev 直读 `/dev/input` 需要 `input` 组权限，
//!   Wayland 下无全局按键注入接口，X11 也不产生手柄事件；等确定方案后再实现。
//!
//! 轮询线程只读 XInput 状态并投递到主线程执行窗口操作，自身不触碰任何 UI 句柄。

/// XInput 按钮位掩码（与 `XINPUT_GAMEPAD_BUTTON_FLAGS` 取值一致）
pub const BUTTON_LB: u16 = 0x0100;
/// 右 bumper
pub const BUTTON_RB: u16 = 0x0200;
/// Start 键
pub const BUTTON_START: u16 = 0x0010;
/// Back（Select）键
pub const BUTTON_BACK: u16 = 0x0020;

/// 解析手柄组合键配置串为按钮位掩码。
///
/// 形如 `"LB+RB"`，大小写与空格不敏感；任一 token 未知或为空则返回 `None`
/// （返回 `None` 表示"不轮询"，而不是"轮询但永不命中"，避免设置写错时空转线程）。
pub fn parse_combo(spec: &str) -> Option<u16> {
    let mut mask = 0u16;
    for token in spec.split('+') {
        match token.trim().to_ascii_uppercase().as_str() {
            "LB" => mask |= BUTTON_LB,
            "RB" => mask |= BUTTON_RB,
            "START" => mask |= BUTTON_START,
            "BACK" => mask |= BUTTON_BACK,
            _ => return None,
        }
    }
    if mask == 0 {
        None
    } else {
        Some(mask)
    }
}

/// 手柄组合命中后要执行的动作。
///
/// 由上层（`lib.rs` 的 setup）注入：platform 属底层，不得反向依赖 hotkey 模块，
/// 该方向被 F1 分层门禁冻结。
pub type TriggerHandler = std::sync::Arc<dyn Fn() + Send + Sync>;

/// 启动手柄热键轮询（Windows 起线程；其它平台记录一次占位日志）
pub fn start_window_hotkey_poller(app: tauri::AppHandle, on_trigger: TriggerHandler) {
    #[cfg(target_os = "windows")]
    {
        let spec = crate::config::settings_store::get_settings()
            .gamepad_hotkey_toggle
            .clone();
        match parse_combo(&spec) {
            None => {
                log::warn!(
                    "[gamepad] 手柄组合键配置不可解析（{:?}），本次运行不轮询手柄热键",
                    spec
                );
            }
            Some(mask) => {
                if let Err(e) = std::thread::Builder::new()
                    .name("gamepad-hotkey-poll".to_string())
                    .spawn(move || poll_loop(app, mask, on_trigger))
                {
                    log::warn!("[gamepad] 手柄轮询线程启动失败: {}", e);
                } else {
                    log::info!(
                        "[gamepad] 手柄切换热键轮询已启动（组合掩码 0x{:04X}）",
                        mask
                    );
                }
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, on_trigger);
        log::warn!("[gamepad] 手柄切换热键目前仅在 Windows 实现，当前平台为占位（不轮询）");
    }
}

/// 轮询间隔（毫秒）。40ms ≈ 25Hz，足以捕捉组合键按下边沿且 CPU 开销可忽略。
#[cfg(target_os = "windows")]
const POLL_INTERVAL_MS: u64 = 40;

/// 每多少个 tick 重新读取一次设置里的组合配置（约 1 秒）
#[cfg(target_os = "windows")]
const SETTINGS_REFRESH_TICKS: u32 = 25;

/// 两次触发之间的最小间隔，防止抖动与长按连发
#[cfg(target_os = "windows")]
const TRIGGER_COOLDOWN_MS: u64 = 400;

/// Windows 侧轮询主循环
///
/// # Errors
/// 本函数不返回错误：XInput 调用失败视为"该槽位无手柄"，线程异常仅记日志并退出。
#[cfg(target_os = "windows")]
fn poll_loop(app: tauri::AppHandle, initial_mask: u16, on_trigger: TriggerHandler) {
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::UI::Input::XboxController::{
        XInputGetState, XINPUT_GAMEPAD_BUTTON_FLAGS, XINPUT_STATE,
    };

    let mut wanted = initial_mask;
    let mut prev_pressed = [false; 4];
    let mut ticks = 0u32;
    let mut last_fire = Instant::now() - Duration::from_millis(TRIGGER_COOLDOWN_MS + 1);

    loop {
        // 周期性重读设置，让用户在设置页改组合后无需重启
        ticks += 1;
        if ticks.is_multiple_of(SETTINGS_REFRESH_TICKS) {
            let spec = crate::config::settings_store::get_settings()
                .gamepad_hotkey_toggle
                .clone();
            match parse_combo(&spec) {
                Some(mask) => wanted = mask,
                // 配置变成不可解析：保持上一个可用组合，避免误清空后彻底失灵
                None => log::debug!("[gamepad] 忽略不可解析的手柄组合配置: {:?}", spec),
            }
        }

        let mut any_pressed = false;
        for slot in 0..4u32 {
            let mut state = XINPUT_STATE::default();
            // SAFETY: XInputGetState 只写入调用方提供的、长度正确的 XINPUT_STATE；
            // 返回非 ERROR_SUCCESS 表示该槽位无连接手柄，此时 state 不被读取。
            let connected = unsafe { XInputGetState(slot, &mut state) } == ERROR_SUCCESS.0;
            let pressed = connected
                && state
                    .Gamepad
                    .wButtons
                    .contains(XINPUT_GAMEPAD_BUTTON_FLAGS(wanted));
            if pressed {
                any_pressed = true;
            }
            // 上升沿触发：仅在「上一帧未满足、本帧满足」时动作，长按不会连发
            if pressed && !prev_pressed[slot as usize] {
                prev_pressed[slot as usize] = true;
                if last_fire.elapsed().as_millis() as u64 >= TRIGGER_COOLDOWN_MS {
                    last_fire = Instant::now();
                    log::info!("[gamepad] 手柄组合命中，触发窗口显隐（slot {}）", slot);
                    let handler = on_trigger.clone();
                    let dispatch = app.run_on_main_thread(move || handler());
                    if let Err(e) = dispatch {
                        log::warn!("[gamepad] 投递主线程失败: {}", e);
                    }
                }
            } else if !pressed {
                prev_pressed[slot as usize] = false;
            }
        }

        if !any_pressed {
            std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
        } else {
            // 组合仍按住时短睡，等待释放边沿
            std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS / 2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_combo_accepts_the_three_configured_combos() {
        assert_eq!(parse_combo("LB+RB"), Some(BUTTON_LB | BUTTON_RB));
        assert_eq!(parse_combo("LB+Start"), Some(BUTTON_LB | BUTTON_START));
        assert_eq!(parse_combo("RB+Back"), Some(BUTTON_RB | BUTTON_BACK));
    }

    #[test]
    fn parse_combo_is_case_and_space_insensitive() {
        assert_eq!(parse_combo(" lb + rb "), Some(BUTTON_LB | BUTTON_RB));
        assert_eq!(parse_combo("START+back"), Some(BUTTON_START | BUTTON_BACK));
    }

    #[test]
    fn parse_combo_rejects_unknown_empty_and_partial_tokens() {
        assert_eq!(parse_combo(""), None);
        assert_eq!(parse_combo("LB+TRIGGER"), None);
        assert_eq!(parse_combo("LB+"), None);
        assert_eq!(parse_combo("A+B"), None);
    }

    #[test]
    fn single_button_combo_is_allowed() {
        assert_eq!(parse_combo("LB"), Some(BUTTON_LB));
    }
}
