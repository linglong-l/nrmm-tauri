//! 目标游戏进程检测模块。
//!
//! 提供：
//! - [`TargetGame`] 枚举：表示当前关注的目标游戏（含 `None` 与 5 款游戏）；
//! - [`ProcessDetector`]：检测指定进程是否在运行、获取前台进程名、
//!   将前台进程名匹配到 [`TargetGame`]。
//!
//! ## 平台支持
//! - [`ProcessDetector::is_process_running`] 与 [`ProcessDetector::get_process_list`]
//!   基于 `sysinfo`，跨平台可用；
//! - [`ProcessDetector::get_foreground_process_name`] 仅在 Windows 上实现，
//!   其他平台直接返回错误。

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sysinfo::System;

use crate::settings::Settings;

/// 目标游戏枚举。
///
/// 用于标识当前用户选中的游戏，或在检测前台进程时表示匹配到的游戏。
///
/// # 变体说明
/// - [`TargetGame::None`]：未匹配到任何游戏（如桌面/浏览器在前台）；
/// - 其余 5 个变体：分别对应 5 款支持的游戏。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetGame {
    /// 未匹配到任何目标游戏。
    None,
    /// 鸣潮（Wuthering Waves）。
    WutheringWaves,
    /// 原神（Genshin Impact）。
    GenshinImpact,
    /// 崩坏：星穹铁道（Honkai: Star Rail）。
    HonkaiStarRail,
    /// 绝区零（Zenless Zone Zero）。
    ZenlessZoneZero,
    /// 明日方舟：终末地（Arknights: Endfield）。
    ArknightsEndfield,
}

impl TargetGame {
    /// 返回枚举对应的字符串标识。
    ///
    /// 用于序列化到设置文件、与前端通信等场景。
    ///
    /// # 返回值
    /// 返回 `&'static str`，如 `"WutheringWaves"`、`"None"`。
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetGame::None => "None",
            TargetGame::WutheringWaves => "WutheringWaves",
            TargetGame::GenshinImpact => "GenshinImpact",
            TargetGame::HonkaiStarRail => "HonkaiStarRail",
            TargetGame::ZenlessZoneZero => "ZenlessZoneZero",
            TargetGame::ArknightsEndfield => "ArknightsEndfield",
        }
    }
}

/// 进程检测器（无状态）。
///
/// 所有方法均无内部可变状态，可安全地通过 `Arc` 共享。
pub struct ProcessDetector;

impl ProcessDetector {
    /// 构造空实例。
    pub fn new() -> Self {
        Self
    }

    /// 检测指定名称的进程是否正在运行。
    ///
    /// # 业务逻辑
    /// 1. 通过 `sysinfo` 获取全量进程列表；
    /// 2. 大小写不敏感地比较进程名与目标名称；
    /// 3. 只要存在任一匹配进程即返回 `true`。
    ///
    /// # 参数
    /// - `process_name`：目标进程名（如 `"Wuthering Waves.exe"`）。
    ///
    /// # 返回值
    /// 运行中返回 `Ok(true)`，否则返回 `Ok(false)`。
    ///
    /// # 限制
    /// - 每次调用都会刷新全量进程列表，开销较大，不适合高频调用；
    /// - 异步方法但目前为同步实现，调用方应通过 `spawn_blocking` 等方式避免阻塞异步运行时。
    pub fn is_process_running(&self, process_name: &str) -> Result<bool> {
        let system = System::new_all();
        let process_name_lower = process_name.to_lowercase();

        Ok(system.processes().values().any(|p| {
            p.name()
                .to_string_lossy()
                .to_lowercase()
                == process_name_lower
        }))
    }

    /// 获取当前所有运行中进程的名称列表（去重 + 排序）。
    ///
    /// # 业务逻辑
    /// 1. 通过 `sysinfo` 获取全量进程列表；
    /// 2. 用 [`HashSet`] 对进程名去重；
    /// 3. 转为 `Vec` 并按字典序排序后返回。
    ///
    /// # 返回值
    /// 返回排序后的进程名列表。
    ///
    /// # 限制
    /// - 同样会刷新全量进程列表，开销较大。
    pub fn get_process_list(&self) -> Result<Vec<String>> {
        let system = System::new_all();
        let mut names: HashSet<String> = HashSet::new();

        for process in system.processes().values() {
            names.insert(process.name().to_string_lossy().into_owned());
        }

        let mut sorted: Vec<String> = names.into_iter().collect();
        sorted.sort();
        Ok(sorted)
    }

    /// 获取当前前台进程的可执行文件名。
    ///
    /// # 平台分发
    /// - Windows：委托给 [`Self::get_foreground_process_name_windows`]；
    /// - 其他平台：直接返回错误。
    ///
    /// # 返回值
    /// 成功返回进程名（如 `"Wuthering Waves.exe"`）；失败返回错误。
    pub fn get_foreground_process_name() -> Result<String> {
        #[cfg(windows)]
        {
            Self::get_foreground_process_name_windows()
        }
        #[cfg(not(windows))]
        {
            anyhow::bail!("前台进程检测仅支持 Windows 平台")
        }
    }

    /// Windows 平台下获取前台进程名的实现。
    ///
    /// # 业务逻辑（Win32 API 调用链）
    /// 1. `GetForegroundWindow` 获取前台窗口句柄；
    /// 2. `GetWindowThreadProcessId` 由句柄获取进程 PID；
    /// 3. `OpenProcess` 以 `PROCESS_QUERY_LIMITED_INFORMATION` 权限打开进程；
    /// 4. `QueryFullProcessImageNameW` 查询进程可执行文件完整路径；
    /// 5. 从完整路径中提取文件名并返回；
    /// 6. `CloseHandle` 关闭进程句柄（无论前几步是否成功）。
    ///
    /// # 安全性
    /// 涉及 Win32 FFI 调用，必须放在 `unsafe` 块中。所有错误均通过 `anyhow` 包装。
    ///
    /// # 返回值
    /// 成功返回文件名（不含路径）；任一 Win32 调用失败返回封装后的错误。
    ///
    /// # 边界情况
    /// - 前台窗口句柄为空（无前台窗口）时直接报错；
    /// - PID 为 0 时报错；
    /// - 进程路径转换 UTF-16 → UTF-8 失败时报错。
    #[cfg(windows)]
    fn get_foreground_process_name_windows() -> Result<String> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };

        unsafe {
            // 1) 获取前台窗口句柄
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                anyhow::bail!("Failed to get foreground window");
            }

            // 2) 由句柄获取 PID
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                anyhow::bail!("Failed to get process ID from foreground window");
            }

            // 3) 以受限查询权限打开进程
            let process_handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .with_context(|| format!("Failed to open process with PID {}", pid))?;

            if process_handle.0.is_null() {
                anyhow::bail!("Failed to open process with PID {}", pid);
            }

            // 4) 查询进程路径并提取文件名；5) 在闭包内处理以便统一释放句柄
            let result = (|| -> Result<String> {
                let mut buffer = [0u16; 512];
                let mut size = buffer.len() as u32;

                QueryFullProcessImageNameW(
                    process_handle,
                    PROCESS_NAME_WIN32,
                    windows::core::PWSTR(buffer.as_mut_ptr()),
                    &mut size,
                )
                .context("Failed to query process image name")?;

                // UTF-16 → UTF-8
                let path = String::from_utf16(&buffer[..size as usize])
                    .context("Failed to convert process path to UTF-8")?;

                // 仅保留文件名部分
                let file_name = Path::new(&path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .context("Failed to extract file name from process path")?;

                Ok(file_name.to_string())
            })();

            // 6) 无论查询是否成功都关闭句柄，避免资源泄漏
            let _ = CloseHandle(process_handle);
            result
        }
    }

    /// 将前台进程名匹配到 [`TargetGame`]。
    ///
    /// # 业务逻辑
    /// 将前台进程名与设置中各游戏的目标进程名逐一比较（大小写不敏感），
    /// 命中即返回对应游戏；全部未命中返回 [`TargetGame::None`]。
    ///
    /// 比较顺序：鸣潮 → 原神 → 星铁 → 绝区零 → 终末地。
    ///
    /// # 参数
    /// - `foreground_process`：前台进程名（如 `"Wuthering Waves.exe"`）；
    /// - `settings`：用户设置（包含各游戏的目标进程名）。
    ///
    /// # 返回值
    /// 匹配到的 [`TargetGame`]，未匹配返回 [`TargetGame::None`]。
    ///
    /// # 边界情况
    /// - 设置中目标进程名为空字符串时，仅当前台进程名也为空字符串时才会匹配（极少见）；
    /// - 用户可自定义目标进程名以支持非官方启动器或重命名后的可执行文件。
    pub fn match_game_process(foreground_process: &str, settings: &Settings) -> TargetGame {
        let foreground_lower = foreground_process.to_lowercase();

        // 逐一比较，命中即返回
        if foreground_lower == settings.target_process_wuwa.to_lowercase() {
            return TargetGame::WutheringWaves;
        }
        if foreground_lower == settings.target_process_genshin.to_lowercase() {
            return TargetGame::GenshinImpact;
        }
        if foreground_lower == settings.target_process_hsr.to_lowercase() {
            return TargetGame::HonkaiStarRail;
        }
        if foreground_lower == settings.target_process_zzz.to_lowercase() {
            return TargetGame::ZenlessZoneZero;
        }
        if foreground_lower == settings.target_process_endfield.to_lowercase() {
            return TargetGame::ArknightsEndfield;
        }

        // 全部未命中
        TargetGame::None
    }
}

impl Default for ProcessDetector {
    /// 默认实现等价于 [`ProcessDetector::new`]。
    fn default() -> Self {
        Self::new()
    }
}
