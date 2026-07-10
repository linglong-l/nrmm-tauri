//! 桌面入口创建模块。
//!
//! 提供跨平台的桌面快捷方式创建功能：
//! - Linux：创建 `.desktop` 文件（符合 freedesktop.org 规范）
//! - Windows：创建 `.lnk` 桌面快捷方式（通过 COM IShellLinkW）
//!
//! 所有输出信息均通过 `log` crate 宏输出。

use std::path::PathBuf;

/// 桌面入口文件标识符常量。
/// 与 [tauri.conf.json](file:///d:/PC/TestProjects/XXMI-NRMM/src-tauri/tauri.conf.json) 中的 `identifier` 字段保持一致。
#[allow(dead_code)]
const DESKTOP_FILE_IDENTIFIER: &str = "com.ye.nrmm-rust";
/// 应用默认显示名称。
#[allow(dead_code)]
const APP_DEFAULT_NAME: &str = "NRMM-Rust";
/// 应用描述。
#[allow(dead_code)]
const APP_COMMENT: &str = "XXMI Game Mod Manager";

/// 桌面入口管理器。
///
/// 封装平台特定的桌面入口创建逻辑：
/// - Linux：创建 `.desktop` 文件（freedesktop.org 规范）
/// - Windows：创建 `.lnk` 桌面快捷方式
/// - macOS：不支持，返回错误信息
pub struct DesktopEntryManager;

impl DesktopEntryManager {
    /// 创建桌面入口的入口函数。
    ///
    /// # 参数
    /// - `name`: 可选的快捷方式显示名称，未提供时使用默认名称。
    ///
    /// # 返回
    /// 成功返回 `Ok(())`，若文件已存在则返回错误信息。
    pub fn create_desktop_entry(name: Option<String>) -> Result<(), String> {
        let display_name = name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| APP_DEFAULT_NAME.to_string());

        #[cfg(target_os = "linux")]
        {
            Self::create_linux_desktop(&display_name)
        }
        #[cfg(windows)]
        {
            Self::create_windows_shortcut(&display_name)
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            Err("当前平台不支持此操作".to_string())
        }
    }

    /// 获取当前可执行文件的规范路径。
    fn get_current_exe() -> Result<PathBuf, String> {
        std::env::current_exe()
            .map_err(|e| format!("无法获取当前可执行文件路径: {}", e))
            .and_then(|p| {
                p.canonicalize()
                    .map_err(|e| format!("无法解析可执行文件路径 {}: {}", p.display(), e))
            })
    }

    // ===================== Linux =====================

    /// Linux 平台：创建 `.desktop` 文件。
    #[cfg(target_os = "linux")]
    fn create_linux_desktop(display_name: &str) -> Result<(), String> {
        let exe_path = Self::get_current_exe()?;
        let desktop_filename = format!("{}.desktop", DESKTOP_FILE_IDENTIFIER);

        // 1) 检查系统级目录
        let system_path = PathBuf::from("/usr/share/applications").join(&desktop_filename);
        if system_path.exists() {
            return Err(format!(
                "应用程序菜单中已存在桌面文件: {}",
                system_path.display()
            ));
        }

        // 2) 检查用户级目录
        let user_applications_dir = dirs::data_dir()
            .ok_or_else(|| "无法获取用户数据目录".to_string())?
            .join("applications");
        let user_path = user_applications_dir.join(&desktop_filename);

        if user_path.exists() {
            return Err(format!(
                "用户级目录中已存在桌面文件: {}",
                user_path.display()
            ));
        }

        // 3) 创建新的 .desktop 文件
        log::info!("创建桌面文件: {}", user_path.display());
        std::fs::create_dir_all(&user_applications_dir)
            .map_err(|e| format!("无法创建用户 applications 目录: {}", e))?;

        let content = Self::build_desktop_content(display_name, &exe_path);
        std::fs::write(&user_path, &content)
            .map_err(|e| format!("无法写入桌面文件: {}", e))?;

        // 设置文件权限 644（用户读写，组和其他用户只读）
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&user_path)
                .map_err(|e| format!("无法读取桌面文件权限: {}", e))?
                .permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(&user_path, perms)
                .map_err(|e| format!("无法设置桌面文件权限: {}", e))?;
        }

        log::info!("桌面文件创建完成: {}", user_path.display());
        Ok(())
    }

    /// 构建 `.desktop` 文件内容。
    #[cfg(target_os = "linux")]
    fn build_desktop_content(display_name: &str, exe_path: &PathBuf) -> String {
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name={}\n\
             Comment={}\n\
             Exec={}\n\
             Icon={}\n\
             Categories=Game;Utility;\n\
             Terminal=false\n\
             StartupWMClass=nrmm-rust\n",
            display_name,
            APP_COMMENT,
            exe_path.display(),
            DESKTOP_FILE_IDENTIFIER,
        )
    }

    // ===================== Windows =====================

    /// Windows 平台：创建桌面快捷方式（`.lnk` 文件）。
    #[cfg(windows)]
    fn create_windows_shortcut(display_name: &str) -> Result<(), String> {
        use windows::core::{HSTRING, Interface};
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER, IPersistFile};
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

        let exe_path = Self::get_current_exe()?;
        let desktop_dir = dirs::desktop_dir()
            .ok_or_else(|| "无法获取桌面路径".to_string())?;
        let shortcut_path = desktop_dir.join(format!("{}.lnk", display_name));

        // 检查是否已存在
        if shortcut_path.exists() {
            return Err(format!(
                "桌面快捷方式已存在: {}",
                shortcut_path.display()
            ));
        }

        log::info!("创建桌面快捷方式: {}", shortcut_path.display());

        // 获取可执行文件所在目录作为工作目录
        let working_dir = exe_path
            .parent()
            .map(|p| HSTRING::from(p.to_string_lossy().as_ref()))
            .unwrap_or_else(|| HSTRING::from("."));

        let exe_path_hstr = HSTRING::from(exe_path.to_string_lossy().as_ref());
        let desc_hstr = HSTRING::from(APP_COMMENT);
        let shortcut_path_hstr = HSTRING::from(shortcut_path.to_string_lossy().as_ref());

        // 使用 COM 接口创建 ShellLink
        unsafe {
            let shell_link: IShellLinkW = CoCreateInstance(
                &ShellLink,
                None,
                CLSCTX_INPROC_SERVER,
            )
            .map_err(|e| format!("创建 ShellLink COM 对象失败: {}", e))?;

            shell_link
                .SetPath(&exe_path_hstr)
                .map_err(|e| format!("设置快捷方式目标路径失败: {}", e))?;

            shell_link
                .SetWorkingDirectory(&working_dir)
                .map_err(|e| format!("设置快捷方式工作目录失败: {}", e))?;

            shell_link
                .SetDescription(&desc_hstr)
                .map_err(|e| format!("设置快捷方式描述失败: {}", e))?;

            let persist_file: IPersistFile = shell_link
                .cast()
                .map_err(|e| format!("转换为 IPersistFile 接口失败: {}", e))?;

            persist_file
                .Save(&shortcut_path_hstr, true)
                .map_err(|e| format!("保存快捷方式文件失败: {}", e))?;
        }

        log::info!("桌面快捷方式创建完成: {}", shortcut_path.display());
        Ok(())
    }
}