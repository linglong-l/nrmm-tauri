//! Tauri 命令处理模块
//!
//! 本模块包含所有暴露给前端（WebView）调用的 Tauri 命令处理器：
//! - `mod_commands`: 模组相关命令（扫描、切换、导入、收藏等）
//! - `settings_commands`: 设置相关命令（读取、保存、路径选择等）
//! - `thumbnail_commands`: 缩略图命令（预览图缩放缓存，WebView 内存优化）
//! - `log_commands`: 前端日志转发命令（fire-and-forget 写入后端统一日志）
//!
//! 所有命令都遵循 Tauri 的 command 约定：
//! - 使用 #[tauri::command] 宏标记
//! - 支持 async/await（耗时操作使用 spawn_blocking）
//! - 返回 Result<T, String> 类型（错误自动序列化为字符串）

pub mod archive_commands;
pub mod cache_commands;
pub mod cloud_commands;
pub mod keybind_commands;
pub mod log_commands;
pub mod mod_commands;
pub mod mod_group_commands;
pub mod mod_mod_commands;
pub mod mod_query_commands;
pub mod mod_selection_commands;
pub mod settings_commands;
pub mod thumbnail_commands;
pub mod watcher_commands;
