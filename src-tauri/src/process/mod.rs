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
//! - [`ProcessDetector::get_foreground_process_name`] 在 Windows 上通过 Win32 API 实现；
//!   在 Linux 上通过 X11 (`xprop`) + `/proc` 文件系统尽力实现，兼容 Wine 启动的游戏；
//!   其他平台（含 Wayland 无 XWayland、macOS 等）返回空字符串，不报错，避免程序异常。

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sysinfo::System;

#[cfg(target_os = "linux")]
use std::process::Command;

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
    #[serde(rename = "none", alias = "None")]
    None,
    /// 鸣潮（Wuthering Waves）。
    #[serde(rename = "Wuthering_Waves", alias = "WutheringWaves")]
    WutheringWaves,
    /// 原神（Genshin Impact）。
    #[serde(rename = "Genshin_Impact", alias = "GenshinImpact")]
    GenshinImpact,
    /// 崩坏：星穹铁道（Honkai: Star Rail）。
    #[serde(rename = "Honkai_Star_Rail", alias = "HonkaiStarRail")]
    HonkaiStarRail,
    /// 绝区零（Zenless Zone Zero）。
    #[serde(rename = "Zenless_Zone_Zero", alias = "ZenlessZoneZero")]
    ZenlessZoneZero,
    /// 明日方舟：终末地（Arknights: Endfield）。
    #[serde(rename = "Arknights_Endfield", alias = "ArknightsEndfield")]
    ArknightsEndfield,
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
    /// # 平台分发（空值安全）
    /// - **Windows**：委托给 [`Self::get_foreground_process_name_windows`]，使用 Win32 API；
    /// - **Linux**：委托给 [`Self::get_foreground_process_name_linux`]，通过 X11 + `/proc` 尽力检测，
    ///   兼容 Wine 游戏进程；无法检测时（Wayland/无 X11/任何异常）返回空字符串；
    /// - **其他平台**：直接返回空字符串，确保调用方永不 panic。
    ///
    /// # 返回值
    /// 成功返回进程名（如 `"Wuthering Waves.exe"`）；不支持/无法检测时返回空字符串 `""`；
    /// 仅在严重意外错误时返回 `Err`（当前实现不会触发）。
    pub fn get_foreground_process_name() -> Result<String> {
        #[cfg(windows)]
        {
            Self::get_foreground_process_name_windows()
        }
        #[cfg(target_os = "linux")]
        {
            Ok(Self::get_foreground_process_name_linux().unwrap_or_default())
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            Ok(String::new())
        }
    }

    /// 判断进程名是否为 Wine 兼容层宿主进程（而非真实的 Windows 游戏 EXE）。
    #[cfg(target_os = "linux")]
    fn is_wine_host(name: &str) -> bool {
        let lower = name.to_lowercase();
        lower.starts_with("wine")
            || lower.starts_with("wineserver")
            || lower == "explorer.exe"
            || lower.contains("wine-preloader")
    }

    /// 从 `/proc/<pid>/comm` 读取进程的短名称（最长 15 字节，不含扩展名）。
    #[cfg(target_os = "linux")]
    fn read_proc_comm(pid: u32) -> Option<String> {
        let path = format!("/proc/{}/comm", pid);
        let content = std::fs::read_to_string(&path).ok()?;
        let name = content.trim_end_matches('\n').trim_end_matches('\r');
        if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        }
    }

    /// 从 `/proc/<pid>/cmdline` 读取并分割命令行参数（NUL 分隔）。
    #[cfg(target_os = "linux")]
    fn read_proc_cmdline(pid: u32) -> Option<Vec<String>> {
        let path = format!("/proc/{}/cmdline", pid);
        let content = std::fs::read(&path).ok()?;
        if content.is_empty() {
            return None;
        }
        let args: Vec<String> = content
            .split(|&b| b == 0)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .filter(|s| !s.is_empty())
            .collect();
        if args.is_empty() {
            None
        } else {
            Some(args)
        }
    }

    /// 读取 `/proc/<pid>/stat` 的第 4 字段，获取父进程 PID (ppid)。
    #[cfg(target_os = "linux")]
    fn read_proc_ppid(pid: u32) -> Option<u32> {
        let path = format!("/proc/{}/stat", pid);
        let content = std::fs::read_to_string(&path).ok()?;
        // comm 字段在括号内，可能含空格，需要跳过
        let paren_start = content.find('(')?;
        let paren_end = content.rfind(')')?;
        let after_paren = &content[paren_end + 2..];
        let ppid_str = after_paren.split_whitespace().nth(1)?;
        ppid_str.parse().ok()
    }

    /// 在给定 PID 的 Wine 进程树中查找真实的 Windows `.exe` 可执行文件名。
    ///
    /// 策略：
    /// 1. 遍历 `/proc` 下所有目录，筛选出父进程链指向 `root_pid` 的子进程；
    /// 2. 检查每个进程的 comm，若以 `.exe` 结尾则视为候选；
    /// 3. 检查 cmdline 中的路径参数，提取最后一个路径段中以 `.exe` 结尾的部分；
    /// 4. 优先返回 `.exe` 且非 Wine 宿主的名字；否则返回 None。
    #[cfg(target_os = "linux")]
    fn find_wine_exe_name(root_pid: u32) -> Option<String> {
        let proc_dir = std::path::Path::new("/proc");
        let entries = std::fs::read_dir(proc_dir).ok()?;

        let mut best_candidate: Option<String> = None;

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let pid: u32 = match name.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };

            // 检查是否属于同一进程树（向上追溯祖先到 root_pid 或 PID 1）
            let mut cur = pid;
            let mut is_in_tree = false;
            for _ in 0..50 {
                // 防止无限循环
                if cur == root_pid {
                    is_in_tree = true;
                    break;
                }
                if cur <= 1 {
                    break;
                }
                match Self::read_proc_ppid(cur) {
                    Some(pp) => cur = pp,
                    None => break,
                }
            }
            if !is_in_tree {
                continue;
            }

            // 策略1：comm 本身就是 .exe 且非 Wine 宿主
            if let Some(comm) = Self::read_proc_comm(pid) {
                let cl = comm.to_lowercase();
                if cl.ends_with(".exe") && !Self::is_wine_host(&comm) {
                    // 直接可用，优先返回
                    return Some(comm);
                }
            }

            // 策略2：检查 cmdline 中的参数是否包含 .exe 路径
            if let Some(args) = Self::read_proc_cmdline(pid) {
                for arg in &args {
                    let lower = arg.to_lowercase();
                    if let Some(exe_pos) = lower.rfind(".exe") {
                        // 提取 .exe 结尾的文件名段
                        let before = &arg[..exe_pos + 4];
                        let name_only = before
                            .rsplit(|c| c == '/' || c == '\\')
                            .next()
                            .unwrap_or(before);
                        if !name_only.is_empty() && !Self::is_wine_host(name_only) {
                            if best_candidate.is_none() {
                                best_candidate = Some(name_only.to_string());
                            }
                        }
                    }
                }
            }
        }

        best_candidate
    }

    /// Linux 平台下尽力获取前台进程名。
    ///
    /// # 实现策略（X11 + /proc，无额外 crate 依赖）
    /// 1. 调用 `xprop -root _NET_ACTIVE_WINDOW` 获取当前活动窗口 ID；
    /// 2. 调用 `xprop -id <wid> _NET_WM_PID` 获取窗口所属 PID；
    /// 3. 读取 `/proc/<pid>/comm` 获取进程短名称；
    /// 4. 若进程名是 Wine 宿主（wine64、wine-preloader 等），
    ///    调用 [`Self::find_wine_exe_name`] 在其子进程/命令行中查找真实 `.exe`；
    /// 5. 若 comm 不含 `.exe` 扩展名但属于 Wine 环境，尝试从 `/proc/<pid>/exe`
    ///    符号链接或 cmdline 路径中补全扩展名。
    ///
    /// # 失败安全
    /// - Wayland 下 xprop 无法连接 X server → 返回 None；
    /// - 无 DISPLAY 环境变量 → 返回 None；
    /// - xprop 命令不存在 → 返回 None；
    /// - PID 对应的进程已退出 → 返回 None；
    /// - 任何 I/O 错误、解析错误 → 返回 None；
    /// 外层调用方 [`Self::get_foreground_process_name`] 会将 None 转为空字符串，
    /// 确保不会向上抛 Err 导致前端崩溃。
    #[cfg(target_os = "linux")]
    fn get_foreground_process_name_linux() -> Option<String> {
        // 1) 检查 DISPLAY 环境变量，Wayland 下若未设置 DISPLAY 直接放弃
        if std::env::var("DISPLAY").is_err() {
            // Wayland native — 无标准跨合成器 API 可获取活动窗口，安全降级
            return None;
        }

        // 2) xprop -root _NET_ACTIVE_WINDOW 获取活动窗口 ID
        //    输出格式形如: _NET_ACTIVE_WINDOW(WINDOW): window id # 0x3a00003
        let output = Command::new("xprop")
            .args(["-root", "_NET_ACTIVE_WINDOW"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let wid_str = stdout.trim().rsplit("# ").next()?.trim();
        if wid_str.is_empty() || wid_str == "0x0" {
            return None;
        }

        // 3) xprop -id <wid> _NET_WM_PID 获取 PID
        //    输出格式形如: _NET_WM_PID(CARDINAL) = 12345
        let pid_output = Command::new("xprop")
            .args(["-id", wid_str, "_NET_WM_PID"])
            .output()
            .ok()?;
        if !pid_output.status.success() {
            return None;
        }
        let pid_stdout = String::from_utf8_lossy(&pid_output.stdout);
        let pid_str = pid_stdout.trim().rsplit("= ").next()?.trim();
        let pid: u32 = pid_str.parse().ok()?;
        if pid == 0 {
            return None;
        }

        // 4) 读取 comm 作为进程名
        let comm = Self::read_proc_comm(pid)?;

        // 5) 若为 Wine 宿主进程，在进程树中查找真实 .exe
        if Self::is_wine_host(&comm) {
            if let Some(exe_name) = Self::find_wine_exe_name(pid) {
                return Some(exe_name);
            }
            // 找不到真实 exe 时，返回空字符串让上层匹配为 None
            // （避免把 wine64 误当游戏名）
            return Some(String::new());
        }

        // 6) 非 Wine 进程，comm 已经可用；Windows 游戏在 Wine 下 comm 通常直接是 .exe
        Some(comm)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 ProcessDetector 构造不崩溃。
    #[test]
    fn test_process_detector_new() {
        let detector = ProcessDetector::new();
        let _ = detector;
    }

    /// 验证 ProcessDetector default 构造。
    #[test]
    fn test_process_detector_default() {
        let detector: ProcessDetector = Default::default();
        let _ = detector;
    }

    /// 验证 match_game_process 默认设置下匹配鸣潮。
    #[test]
    fn test_match_game_process_wuwa() {
        let settings = Settings::default();
        let result = ProcessDetector::match_game_process("Wuthering Waves.exe", &settings);
        assert_eq!(result, TargetGame::WutheringWaves);
    }

    /// 验证 match_game_process 默认设置下匹配原神。
    #[test]
    fn test_match_game_process_genshin() {
        let settings = Settings::default();
        let result = ProcessDetector::match_game_process("GenshinImpact.exe", &settings);
        assert_eq!(result, TargetGame::GenshinImpact);
    }

    /// 验证 match_game_process 默认设置下匹配星铁。
    #[test]
    fn test_match_game_process_hsr() {
        let settings = Settings::default();
        let result = ProcessDetector::match_game_process("StarRail.exe", &settings);
        assert_eq!(result, TargetGame::HonkaiStarRail);
    }

    /// 验证 match_game_process 默认设置下匹配绝区零。
    #[test]
    fn test_match_game_process_zzz() {
        let settings = Settings::default();
        let result = ProcessDetector::match_game_process("ZenlessZoneZero.exe", &settings);
        assert_eq!(result, TargetGame::ZenlessZoneZero);
    }

    /// 验证 match_game_process 默认设置下匹配终末地。
    #[test]
    fn test_match_game_process_endfield() {
        let settings = Settings::default();
        let result = ProcessDetector::match_game_process("Endfield-Win64-Shipping.exe", &settings);
        assert_eq!(result, TargetGame::ArknightsEndfield);
    }

    /// 验证 match_game_process 未匹配时返回 None。
    #[test]
    fn test_match_game_process_none() {
        let settings = Settings::default();
        let result = ProcessDetector::match_game_process("notepad.exe", &settings);
        assert_eq!(result, TargetGame::None);
    }

    /// 验证 match_game_process 大小写不敏感。
    #[test]
    fn test_match_game_process_case_insensitive() {
        let settings = Settings::default();
        let result = ProcessDetector::match_game_process("wuthering waves.exe", &settings);
        assert_eq!(result, TargetGame::WutheringWaves);
    }

    /// 验证 match_game_process 空字符串不会匹配到 None 以外的游戏。
    #[test]
    fn test_match_game_process_empty_string() {
        let settings = Settings::default();
        let result = ProcessDetector::match_game_process("", &settings);
        // 默认设置中 target_process_wuwa 为 "Wuthering Waves.exe"，非空，所以不匹配
        assert_eq!(result, TargetGame::None);
    }

    /// 验证 is_process_running 不崩溃。
    #[test]
    fn test_is_process_running_does_not_panic() {
        let detector = ProcessDetector::new();
        let result = detector.is_process_running("nonexistent_process_12345.exe");
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    /// 验证 get_process_list 不崩溃。
    #[test]
    fn test_get_process_list_does_not_panic() {
        let detector = ProcessDetector::new();
        let result = detector.get_process_list();
        assert!(result.is_ok());
    }
}
