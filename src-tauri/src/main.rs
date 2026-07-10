//! 程序二进制入口模块。
//!
//! 此 crate 实际上以库形式编译（`lib.rs`），二进制仅负责调用库的 [`xxmi_nrmm_lib::run`]。
//! 这样设计便于集成测试与移动端复用同一套启动逻辑。

// 在 release 构建中将 Windows 子系统设为 `windows`，避免弹出额外的控制台窗口。
// 必须保留——在 debug 构建下仍保留控制台以便查看日志输出。
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// 程序入口。
///
/// 仅做一件事：转交给 [`xxmi_nrmm_lib::run`] 完成所有初始化与事件循环。
///
/// # 参数
/// 无。
///
/// # 返回值
/// 无。
fn main() {
    xxmi_nrmm_lib::run()
}
