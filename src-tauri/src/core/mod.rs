//! 核心功能模块
//!
//! 本模块包含 NRMM 的核心业务逻辑：
//! - `constants`: 全局常量定义（文件名、前缀、正则等）
//! - `file_watcher`: 文件系统监控器（使用 notify 库，带防抖功能）
//! - `ini_handler`: INI 文件解析和处理
//! - `mod_manager`: 模组管理核心逻辑（启用/禁用、互斥选择、INI 注入）
//! - `mod_scanner`: 模组扫描器（轻量扫描和深度扫描两种模式）
//! - `namespace_handler`: 命名空间处理（namespace 变量展开）
//! - `archive_handler`: 压缩包处理（7z/zip/rar 解压导入）
//! - `cloud_data`: 云端数据管理（链接、消息、图标库等远程资源）
//! - `mod_cache`: 模组数据内存缓存（避免重复文件系统扫描）

pub mod constants;
pub mod file_watcher;
pub mod ini_handler;
pub mod mod_manager;
pub mod mod_scanner;
pub mod namespace_handler;
pub mod archive_handler;
pub mod cloud_data;
pub mod mod_cache;
pub mod d3dxini_cache;
pub mod incremental_updater;
