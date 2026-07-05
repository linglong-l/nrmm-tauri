//! 管理员权限检测模块。
//!
//! 提供跨平台的 `is_admin` 函数，用于检测当前进程是否以管理员/Root 权限运行。

/// 检测当前进程是否拥有管理员权限。
///
/// - Windows：调用 `shell32.dll` 导出的 `IsUserAnAdmin`。
/// - Linux/macOS：检测 effective UID 是否为 0。
///
/// 返回：管理员权限状态。检测失败时默认返回 `false`。
pub fn is_admin() -> bool {
    #[cfg(target_os = "windows")]
    {
        unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Unix-like 系统：root 的 effective UID 为 0
        unsafe { libc::geteuid() == 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_admin_returns_boolean() {
        // 正常构建环境下不应 panic，返回值取决于运行环境
        let _ = is_admin();
    }
}
