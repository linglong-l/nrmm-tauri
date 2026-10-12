//! 模组相关 Tauri 命令
//!
//! 提供模组管理的前端接口：
//! - get_mods: 获取模组列表（优先缓存，轻量扫描）
//! - refresh_mods: 强制刷新模组列表（轻量扫描，更新缓存）
//! - apply_mods: 应用模组选择（深度扫描+INI注入）
//! - switch_mod: 切换普通分组选中模组
//! - deselect_group: 取消选中分组
//! - toggle_mod_enabled/toggle_mutex_mod_enabled: 启用/禁用模组
//! - toggle_favorite: 切换收藏
//! - open_mod_folder: 打开模组目录
//! - import_mod_from_archive: 从压缩包导入模组
//! - delete_mod: 删除模组
//! - select_folder: 系统文件夹选择对话框
//!
//! # 性能设计
//! - 列表查询用轻量扫描+缓存（<100ms）
//! - apply 操作用深度扫描（完整 INI 解析，可能几秒）
//! - 所有 IO 操作使用 spawn_blocking 避免阻塞 UI
//!
//! > **Phase 6-2 归档说明（2026-09-19）**：本文件已按域拆出命令——查询/扫描域在
//! > [`mod_query_commands`](mod_query_commands)、选择/分组域在
//! > [`mod_group_commands`](mod_group_commands)、启停/恢复/操作域在
//! > [`mod_mod_commands`](mod_mod_commands)。本文件保留命令契约结构
//! > （`SelectModArgs` / `ToggleModResult`）与 `mod tests`（测试挂载于
//! > [`mod_commands_tests`](mod_commands_tests)）。共享辅助已迁至
//! > [`mod_commands_util`](mod_commands_util)。

use crate::ipc::typed_paths::ManagedPath;
use crate::models::mod_data::GroupDelta;

/// 选择模组命令的参数结构体
///
/// 将 select_mod 命令的 9 个独立参数封装为单个结构体，
/// 以避免 clippy::too_many_arguments 警告，并提升参数可读性。
/// 通过 #[serde(rename_all = "camelCase")] 与前端 camelCase 键名保持一致。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectModArgs {
    /// 目标游戏标识（如 "wuwa"、"genshin"）
    pub game: String,
    /// 模组根目录路径
    pub mods_path: ManagedPath,
    /// 分组索引
    pub group_index: u32,
    /// 模组索引
    pub mod_index: u32,
    /// 是否为互斥组
    pub is_mutex: bool,
    /// 分组路径
    pub group_path: ManagedPath,
    /// 模组路径
    pub mod_path: ManagedPath,
    /// 屏幕光标 X 坐标（可选，像素值）
    pub cursor_x: Option<i32>,
    /// 屏幕光标 Y 坐标（可选，像素值）
    pub cursor_y: Option<i32>,
}

/// 单模组启用/禁用结果（零扫描路径）。
///
/// 缓存 patch 未命中、或大树被外部持有（COW 保护）时**不再同步重扫**——交互路径不得因
/// 扫描阻塞（原组级重扫在连续操作时叠加为秒级卡顿 + 命令排队 + 通知齐发）；改为标记该
/// 游戏缓存失效，由 watcher 增量 / 下次 get_mods 全量兜底修正，前端乐观更新已即时呈现。
/// 故两字段恒为 `false` / 空占位（保留以兼容 IPC 契约）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToggleModResult {
    /// 操作后模组目录绝对路径（rename 后；幂等未改名时为原路径）
    pub new_path: String,
    /// 操作后的禁用状态（true=已禁用）
    pub disabled: bool,
    /// 恒为 `false`：启停不再走同步重扫降级（保留字段以兼容 IPC 契约）
    pub degraded: bool,
    /// 恒为空占位（同上；保留字段以兼容 IPC 契约）
    pub group_delta: GroupDelta,
}

/// 单元测试：update_mod_data / restore_all_inis 逻辑整合于此
/// （挂载自独立文件 mod_commands_tests.rs）。
#[cfg(test)]
#[path = "mod_commands_tests.rs"]
mod tests;

/// 命令级 Delta 组装（R1）测试：挂载自 mod_commands_delta_tests.rs。
#[cfg(test)]
#[path = "mod_commands_delta_tests.rs"]
mod delta_tests;
