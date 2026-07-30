//! 配置管理模块
//!
//! 本模块负责应用配置的管理：
//! - `app_paths`: 应用路径配置（配置目录、缓存目录、临时目录等）
//! - `settings_store`: 设置存储（内存缓存+磁盘持久化，原子写入保证安全）
//!
//! # 设计要点
//! - 设置在内存中使用 RwLock 缓存，避免频繁磁盘 IO
//! - 写入使用临时文件+rename 原子操作，防止断电/崩溃导致配置损坏
//! - 遵循 XDG 目录规范（Windows 下使用 %APPDATA% 等标准路径）

pub mod app_paths;
pub mod settings_store;
