//! NRMM 应用程序主入口
//!
//! 这是一个极简的主函数文件，仅调用 xxmi_nrmm_lib 库中的 run() 函数启动 Tauri 应用。
//! 所有实际的初始化和应用逻辑都在 lib.rs 中实现，这种设计模式允许：
//! - 更好的代码组织和测试性
//! - 其他 crate 可以引用库功能
//! - 符合 Rust 项目的最佳实践

// 见 lib.rs：关闭主观的 pedantic / nursery 两组，保留 `-D warnings` 把关真实问题。
#![allow(clippy::pedantic, clippy::nursery)]

fn main() {
    xxmi_nrmm_lib::run();
}
