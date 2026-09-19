//! 模组操作结果/状态类型域（纯数据，无逻辑）
//!
//! 从 mod_manager.rs 拆出（2026-09-19 Phase 4-2）。
//! 被 commands/mod_commands.rs 序列化与消费；不依赖其他领域模块。

use std::path::PathBuf;

use crate::models::mod_data::{ErroredLines, GroupDelta, OrfixDetection};

/// 主 INI（游戏根 d3dx.ini）处理状态，上报前端用于缺失警告。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Default)]
pub enum MainIniStatus {
    /// 主 INI 存在于游戏根，备份/读取/注入流程正常执行
    #[default]
    Resolved,
    /// 游戏根未找到 d3dx.ini：模组处理仍完成，但游戏侧不会加载管理段
    MissingMainIni,
}

/// 模组更新结果结构体
// rename_all 对齐项目其余模型（mod_data/ScanResult 均为 camelCase）；
// 缺失时前端按 camelCase 读取 mainIniStatus/needReloadManual 等字段拿到 undefined（潜在缺陷）。
#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResult {
    /// 总分组数
    pub total_groups: u32,
    /// 总模组数
    pub total_mods: u32,
    /// 启用的模组数
    pub enabled_mods: u32,
    /// 禁用的模组数
    pub disabled_mods: u32,
    /// 实际处理（INI 注入）的模组数
    pub processed_mods: u32,
    /// 实际写盘（INI 内容有变化）的 INI 计数
    #[serde(default)]
    pub written_mods: u32,
    /// 因内容未变化跳过写盘（幂等快速路径）的 INI 计数
    #[serde(default)]
    pub skipped_mods: u32,
    /// INI 解析/处理过程中的错误
    pub errors: Vec<ErroredLines>,
    /// 是否需要用户手动重载（3Dmigoto）
    #[serde(default)]
    pub need_reload_manual: bool,
    /// 选择操作（switch_mod）成功写入磁盘 selectedindex 文件的最终值。
    /// - NormalGroup：范围为 [0, g.mods.len()-1]，0 代表 None 槽位
    /// - update_mod_data（非选择类操作）：默认返回 None（null）
    /// - MutexGroup：不使用 switch_mod 路径，始终 None
    #[serde(default)]
    pub selected_mod_index: Option<i32>,
    /// 是否检测到标准 XXMI/3DMigoto 环境（[Constants] + XXMI 注入标记 + [Inject] 段）
    #[serde(default)]
    pub is_standard_xxmi: bool,
    /// 主 INI（游戏根 d3dx.ini）处理状态（Resolved / MissingMainIni）
    #[serde(default)]
    pub main_ini_status: MainIniStatus,
    /// ORFix/TexFx 检测结果（在 INI 修改之前使用原始内容检测）
    #[serde(default)]
    pub orfix_detection: OrfixDetection,
    /// 因文件名非 UTF-8 被跳过的条目总数（update_mod_data 深扫期间）
    #[serde(default, skip_serializing_if = "is_zero_count")]
    pub invalid_utf8_count: usize,
    /// 被跳过的非法条目明细（空时省略）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalid_utf8_entries: Vec<crate::core::fs_walk::SkippedEntry>,
}

/// serde 辅助：跳过 0 值字段
fn is_zero_count(v: &usize) -> bool {
    *v == 0
}

/// INI 恢复结果统计
#[derive(Debug, Clone, serde::Serialize)]
pub struct RestoredCount {
    /// 成功恢复的文件数
    pub restored: u32,
    /// 恢复失败的文件数
    pub failed: u32,
}

/// Save Customizations 结果
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct SaveCustomizationsResult {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
}

/// 模组移除结果
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveModResult {
    /// 模组名称
    pub mod_name: String,
    /// 移动后的目标路径
    pub moved_to: PathBuf,
    /// INI 还原是否成功（true=全部成功，false=部分或全部失败）
    pub restored: bool,
    /// 还原过程中处理的 INI 文件数
    pub ini_count: u32,
    /// 还原过程中失败的文件数
    pub failed_count: u32,
    /// 受影响分组的完整新状态（命令级增量，序列化为 groupDelta）
    ///
    /// core 层构造占位（`GroupDelta::default()`），由命令层（`mod_commands::remove_mod`）
    /// 局部重扫受影响分组后组装真实值覆盖——core 层不感知缓存与局部扫描上下文。
    pub group_delta: GroupDelta,
    /// 是否降级为增量扫描（命令层缓存剔除未命中时置 true；前端据此消费 group_delta 并告知用户）
    #[serde(default)]
    pub degraded: bool,
}

/// 还原模组 INI 结果（还原区功能）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreManagedResult {
    /// 被还原的目录路径
    pub path: PathBuf,
    /// 处理的 INI 文件数
    pub ini_count: u32,
    /// 还原失败的文件数
    pub failed_count: u32,
    /// 是否全部还原成功
    pub success: bool,
}