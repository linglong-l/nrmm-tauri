//! 工具模块集合。
//!
//! 收纳后端通用的辅助工具（如日志采样器、目录遍历器），供其他业务模块复用。

pub mod dir_walker;
pub mod error_boundary;
pub mod log_sampler;

#[allow(unused_imports)]
pub use dir_walker::{DEFAULT_MAX_TRAVERSAL_DEPTH, DirEntry, DirWalker, FileKind, VisitedPathPool};
