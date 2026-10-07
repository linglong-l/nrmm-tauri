//! IPC 边界类型层（方案 C：把路径校验前移到反序列化）
//!
//! # 为什么存在
//! Tauri v2 对任何实现 [`serde::Deserialize`] 的类型自动提供
//! [`tauri::ipc::CommandArg`] 实现，因此命令入参在**进入函数体之前**就会被解析。
//! 把「路径必须在已登记的 Mods 根内」这类不变量写进 `Deserialize`，校验便无法被
//! 某个命令忘记——原实现里 84 个命令仅 10 处调用了 `validate_managed_path`，
//! 漏掉的每一个都是任意文件读写/改名/执行面。
//!
//! # 分层
//! 本模块只依赖 `std`/`serde`/`anyhow` 与同级的 [`ipc_errors`]，
//! 不引用 `core`/`commands`/`window`/`tray`/`hotkey`/`updater`，
//! 以便后续纳入 `tests/arch/arch_gate.rs` 的分层冻结表。

pub mod ipc_errors;
pub mod mods_root_registry;
pub mod typed_paths;
