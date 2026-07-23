//! 模组管理器模块
//!
//! 该模块是 XXMI-NRMM（No Reload Mod Manager）的核心后端模块，负责管理游戏的模组目录结构、
//! 分组扫描、模组启用/禁用、收藏标记、INI 文件自动注入管理等任务。
//!
//! 目录约定：
//! - Mods 根目录下存在一个名为 `_MANAGED_` 的特殊文件夹，用于存放由本管理器自动生成的
//!   分组配置 INI 文件以及被移除的分组等管理性内容。
//! - 分组目录命名规则：`group_<index>`，其中 <index> 为正整数（从 1 开始）。
//! - 以 `#` 开头的目录会被递归展开，其内部所有 `#` 开头子目录均被视为分组候选项。
//! - 普通目录若符合「模组目录」特征（包含 icon.png 或 *.ini），也会被识别为单个分组。
//! - 模组目录名以 `DISABLED` 前缀表示该模组处于禁用状态。

pub mod path_utils;
pub mod game_interaction;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use rayon::prelude::*;
use rars::ArchiveReader;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use zip::ZipArchive;
use sevenz_rust::{decompress_file, Password};

use crate::ini_handler::error_detection::ErroredLinesReport;
use crate::ini_handler::{
    extract_namespace_from_ini_content, get_section_type, is_comment_line, is_section_header,
    parse_section_name, replace_namespace_in_content, SectionType,
};
use crate::process::TargetGame;
use crate::settings::Settings;
use crate::utils::{DirWalker, FileKind, VisitedPathPool, DEFAULT_MAX_TRAVERSAL_DEPTH};

/// 禁用模组目录名前缀。被禁用的模组目录会被重命名为 `DISABLED<原名>`，
/// 3DMigoto 框架会忽略此前缀开头的目录，从而实现「不删除文件即可禁用」的效果。
pub const DISABLED_PREFIX: &str = "DISABLED";

/// 收藏标记文件名。在模组/分组目录下存在该文件即表示已被收藏，
/// 文件内容为收藏时的时间戳字符串（用于排序）。
const FAVORITE_FILE: &str = ".favorite";

/// NRMM 使用的收藏标记文件名。NRMM 以该文件的存在表示收藏，并以文件修改时间作为收藏时间。
const NRMM_FAVORITE_FILE: &str = "fav";

/// 支持的图标文件扩展名列表。扫描图标时会按此列表的顺序优先匹配 `icon.<ext>`，
/// 找不到时再回退到目录中任意一个匹配扩展名的文件。
const ICON_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "ico", "webp", "bmp"];

/// 管理文件夹名称。所有由 NRMM 自动生成的分组 INI 及管理性内容均存放于此。
pub const MANAGED_FOLDER: &str = "_MANAGED_";

/// 已被移除的 Managed 分组存放目录名。删除分组时，分组目录会被移动到此目录。
pub const DISABLED_MANAGED_REMOVED: &str = "DISABLED_MANAGED_REMOVED";

/// 分组显示名称持久化文件名。文件内容为该分组的可读名称（独立于 `group_<index>` 目录名）。
const GROUP_NAME_FILE: &str = "groupname";

/// 模组显示名称持久化文件名。文件内容为该模组的可读名称（独立于目录名）。
const MOD_NAME_FILE: &str = "modname";

/// 选中索引持久化文件名。分组目录下保存当前选中的模组索引，
/// `_MANAGED_` 目录下保存当前选中的分组索引。
const SELECTED_INDEX_FILE: &str = "selectedindex";

/// 被 NRMM 管理修改的 INI 文件的原始备份扩展名。
/// 第一次修改 INI 时会生成 `<ini>.ini_managed_backup` 备份，便于回滚。
/// 扩展名与 NRMM 原始项目保持一致（NRMM 使用 `ini_managed_backup`）。
const MANAGED_BACKUP_EXT: &str = "ini_managed_backup";

/// 分组「None 槽位」专用图标文件名。每个分组第一个槽位为 None（不启用任何模组），
/// 若分组目录下存在此文件，则作为该槽位的图标。
const NONE_SLOT_ICON: &str = "none_slot_icon.png";

/// 单个模组的元数据描述。
///
/// 该结构体通过 `serde` 序列化为 camelCase JSON 后传递给前端，
/// 前端依据其中的字段渲染模组列表 UI。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModData {
    /// 模组目录的绝对路径。特殊值 `"None"` 表示分组中的 None 槽位（不启用任何模组）。
    pub mod_path: String,
    /// 模组图标文件的绝对路径。若目录中不存在任何支持的图标文件则为 `None`。
    pub icon_path: Option<String>,
    /// 模组的显示名称。优先取自 `modname` 文件，否则回退到目录名（去除 `DISABLED` 前缀）。
    pub mod_name: String,
    /// 模组在分组内的真实索引（从 1 开始；None 槽位固定为 0）。
    /// 此索引会被注入到 INI 文件的 `global $managed_slot_id` 变量中。
    pub real_index: i32,
    /// 标记该模组是否曾因版本过旧而被自动修复（存在 `modforced` 标记文件）。
    pub is_old_auto_fixed: bool,
    /// 标记该模组是否曾被移除语法错误（存在 `modsyntaxerrorremoved` 标记文件）。
    pub is_syntax_error_removed: bool,
    /// 标记该模组是否未经过优化（存在 `modunoptimized` 标记文件）。
    pub is_unoptimized: bool,
    /// 标记该模组是否已被命名空间化处理（存在 `modnamespaced` 标记文件）。
    pub is_namespaced: bool,
    /// 该模组当前是否处于禁用状态（目录名以 `DISABLED` 开头）。
    pub is_disabled: bool,
    /// 收藏时间戳字符串。`None` 表示未收藏；存在 `.favorite` 文件时为文件内容。
    pub favorite_date_time: Option<String>,
}

/// 单个分组（Group）的元数据描述，包含该分组下的所有模组列表。
///
/// 一个分组对应一个互斥选择集：同一分组内同一时刻只能有一个模组被激活，
/// 通过 3DMigoto 的 `active_slot` 变量与 `if $managed_slot_id == ...` 条件实现。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModGroupData {
    /// 分组目录的绝对路径。
    pub group_path: String,
    /// 分组图标路径（可选）。
    pub icon_path: Option<String>,
    /// 分组显示名称。优先取自 `groupname` 文件，否则回退到目录名。
    pub group_name: String,
    /// 分组收藏时间戳（可选）。
    pub favorite_date_time: Option<String>,
    /// 分组内所有模组的列表（含首位的 None 槽位）。
    pub mods_in_group: Vec<ModData>,
    /// 分组的真实索引（`group_<index>` 中的 index，或递归/普通目录的递增序号）。
    pub real_index: i32,
    /// 上次持久化保存的选中模组索引（用于 UI 还原选中状态）。
    pub previous_selected_mod_on_group: i32,
    /// 嵌套子分组列表（树形结构，# 目录下的子 # 目录）。
    #[serde(default)]
    pub children: Vec<ModGroupData>,
    /// 是否为树节点（# 开头的目录）。
    #[serde(default)]
    pub is_tree_node: bool,
    /// 是否为虚拟分类节点（如 "Group" 主分类，无真实文件路径，仅作容器）。
    #[serde(default)]
    pub is_virtual: bool,
    /// 分组是否处于禁用状态（目录名以 DISABLED 开头）。
    #[serde(default)]
    pub is_disabled: bool,
}

/// 日志条目，用于将后端处理过程中的信息传递给前端展示。
///
/// 通过 `level` 字段区分级别（info/warn/error/success），
/// 前端可据此使用不同颜色或图标渲染。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// 日志正文内容。
    pub message: String,
    /// 日志级别字符串：`"info"` / `"warn"` / `"error"` / `"success"`。
    pub level: String,
    /// 可选的详细信息（例如错误堆栈、关联文件路径等）。
    pub detail: Option<String>,
}

impl LogEntry {
    /// 创建一条 info 级别的日志。
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            level: "info".to_string(),
            detail: None,
        }
    }

    /// 创建一条 warn 级别的日志。
    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            level: "warn".to_string(),
            detail: None,
        }
    }

    /// 创建一条 error 级别的日志。
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            level: "error".to_string(),
            detail: None,
        }
    }

    /// 创建一条 success 级别的日志（表示操作成功完成）。
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            level: "success".to_string(),
            detail: None,
        }
    }

    /// 为日志附加详细信息（链式调用）。
    #[allow(dead_code)]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Hash 冲突检测相关结构
///
/// 用于描述单个模组的基本信息及其内容 hash，便于检测多个启用模组间的内容重复冲突。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HashedModInfo {
    /// 模组目录路径。
    pub mod_path: String,
    /// 模组显示名称。
    pub mod_name: String,
    /// 所属分组名称。
    pub group_name: String,
    /// 该模组所有 INI 文件合并后计算得到的 hash 字符串。
    pub hash: String,
}

/// 单组 hash 冲突条目。
///
/// 将 `HashConflictReport.enabled_mod_hashes` 中按 hash 分组后的模组集合
/// 抽取为一条结构化记录，便于前端渲染（如「mod_a 与 mod_b 冲突，hash: a1b2c3d4」）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HashConflictEntry {
    /// 冲突的 hash 字符串（完整长度）。
    pub hash: String,
    /// 共享该 hash 的模组显示名称列表。
    pub mod_names: Vec<String>,
    /// 共享该 hash 的模组目录路径列表，与 `mod_names` 一一对应。
    pub mod_paths: Vec<String>,
    /// 共享该 hash 的模组所属分组名称列表，与 `mod_names` 一一对应。
    pub group_names: Vec<String>,
}

/// Hash 冲突检测报告。
///
/// 由 `update_mod_data` 流程或独立的 `check_hash_conflicts` 命令生成，
/// 包含启用模组的内容 hash 冲突信息，用于在前端提示用户可能存在重复的模组。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HashConflictReport {
    /// 启用的 mod hash 冲突：hash -> 具有相同 hash 的模组列表（旧字段，保持向后兼容）
    pub enabled_mod_hashes: HashMap<String, Vec<HashedModInfo>>,
    /// 命名空间 hash：namespace hash -> 文件路径列表
    pub namespace_hashes: HashMap<String, Vec<String>>,
    /// 结构化冲突条目列表：每个条目对应一组共享相同 hash 的模组。
    /// 与 `enabled_mod_hashes` 内容保持一致（前者按 hash 分组，后者按条目扁平化）。
    #[serde(default)]
    pub conflicts: Vec<HashConflictEntry>,
}

/// `update_mod_data` 命令的返回结果，包含执行状态、日志、耗时及各项检测报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateModDataResult {
    /// 整体是否成功完成。
    pub success: bool,
    /// 执行过程中产生的日志条目列表。
    pub logs: Vec<LogEntry>,
    /// 总耗时（毫秒）。
    pub duration_ms: u64,
    /// INI 语法错误检测报告（成功时存在）。
    pub error_report: Option<ErroredLinesReport>,
    /// 启用模组的 hash 冲突报告（成功时存在）。
    pub hash_conflict_report: Option<HashConflictReport>,
    /// 每个模组的处理错误列表（仅当前请求周期有效）。
    pub per_mod_errors: Vec<ModManageError>,
    /// 分组处理摘要。
    pub group_summaries: Vec<ModProcessSummary>,
    /// 总共处理的模组数量。
    pub total_mods_processed: u32,
    /// 错误总数。
    pub total_errors: u32,
}

/// 模组级处理错误。
///
/// 仅在当前 `update_mod_data` 请求周期内有效，随响应返回给前端后销毁。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModManageError {
    /// 模组路径。
    pub mod_path: String,
    /// 模组显示名称。
    pub mod_name: String,
    /// 错误阶段：`ini_backup` | `ini_modify` | `ini_write` | `validate`。
    pub stage: String,
    /// 用户友好的错误描述。
    pub message: String,
    /// 出错的 ini 文件名（可选）。
    pub ini_file: Option<String>,
}

/// 分组处理摘要。
///
/// 统计每个分组在 `update_mod_data` 流程中的处理结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModProcessSummary {
    /// 分组名称。
    pub group_name: String,
    /// 该分组下处理的模组总数。
    pub total_mods: u32,
    /// 处理成功的模组数。
    pub success_count: u32,
    /// 处理失败的模组数。
    pub error_count: u32,
    /// 命名空间修复记录列表。
    #[serde(default)]
    pub namespace_fixes: Vec<NamespaceFix>,
}

/// 命名空间冲突修复记录。
///
/// 描述一次命名空间冲突自动修复的详细信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceFix {
    /// 发生冲突的模组名称。
    pub mod_name: String,
    /// 原始命名空间。
    pub original_namespace: String,
    /// 修复后的新命名空间。
    pub new_namespace: String,
}

/// Mods 路径校验状态枚举。
///
/// `validate_mods_path` 函数会按以下顺序进行检查，并返回首个失败的状态：
/// 1. 路径是否存在且为目录
/// 2. 目录名是否为 `Mods`
/// 3. 父目录下是否存在 `d3dx.ini`
/// 4. 父目录下是否存在 `d3d11.dll`
/// 5. Mods 目录下是否存在 `_MANAGED_` 文件夹
/// 6. 全部通过则返回 `Valid`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModsPathStatus {
    /// 路径不存在或不是目录。
    InvalidNotExist,
    /// 目录名不是 `Mods`。
    InvalidNotModsFolder,
    /// 缺少 `d3dx.ini`（3DMigoto 核心配置文件）。
    InvalidMissingD3dx,
    /// 缺少 `d3d11.dll`（3DMigost 核心动态库）。
    InvalidMissingDll,
    /// 缺少 `_MANAGED_` 管理文件夹（会在首次 update_mod_data 时自动创建）。
    InvalidWithoutManagedFolder,
    /// 缺少其他前置必需文件（保留枚举，当前未在 validate 流程中使用）。
    InvalidWithoutPrerequisiteFiles,
    /// 3DMigoto 版本过旧（保留枚举，当前未在 validate 流程中使用）。
    InvalidOutdated,
    /// 路径校验通过，可以使用。
    Valid,
}

/// 分组排序方式枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortGroupMethod {
    /// 按分组真实索引升序排序。
    ByIndex,
    /// 按分组名称字母序排序（不区分大小写）。
    ByName,
}

impl SortGroupMethod {
    /// 将 i32 值转换为排序方式：`1` 表示按名称，其他值（含 0）表示按索引。
    pub fn from_i32(value: i32) -> Self {
        match value {
            1 => SortGroupMethod::ByName,
            _ => SortGroupMethod::ByIndex,
        }
    }
}

/// 支持的压缩文件类型枚举。
///
/// 用于 `detect_archive_type` / `validate_archive_file` 等函数的返回值，
/// 标识压缩文件的实际格式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveType {
    /// ZIP 格式（.zip）
    Zip,
    /// 7z 格式（.7z）
    SevenZip,
    /// RAR 格式（.rar）
    Rar,
    /// 未知格式或不支持
    Unknown,
}

/// 模组管理器主结构体。
///
/// 该结构体本身无内部状态（所有方法均接受外部传入的参数），
/// 通过 `new()` / `default()` 构造实例后即可调用其关联函数。
pub struct ModManager;

impl ModManager {
    /// 创建一个新的 `ModManager` 实例。
    /// 由于该结构体无内部状态，此构造仅返回一个空的占位实例。
    pub fn new() -> Self {
        Self
    }

    /// 判断目录名是否以 DISABLED 前缀开头（不区分大小写）。
    ///
    /// NRMM 在 Linux/macOS 上可能使用小写 "disabled" 前缀，
    /// 因此判断逻辑需放宽，但输出仍保持大写 `DISABLED_` 以保持一致。
    pub fn is_disabled_name(name: &str) -> bool {
        name.get(..DISABLED_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(DISABLED_PREFIX))
    }

    /// 在模组列表中找到第一个已启用模组的索引。
    ///
    /// 用于 # 目录分组的 `previous_selected_mod_on_group` 推导。
    /// 由于 # 目录分组采用互斥模式（同一时间最多一个启用模组），
    /// 不需要 `selectedindex` 文件，直接根据当前状态计算即可。
    ///
    /// 行为：
    /// - 找到第一个 `is_disabled == false` 且 `mod_path != "None"` 的模组，返回其在数组中的索引。
    /// - 索引语义与 `previous_selected_mod_on_group` 一致：包含 None 槽位。
    /// - 若没有找到启用的模组（全部禁用或仅有 None 槽位），返回 0（指向 None 槽位）。
    /// - 若 `mods_in_group` 为空（递归子分组无 mod），返回 0。
    ///
    /// 参数：
    /// - `mods_in_group`: 模组列表引用。
    ///
    /// 返回：第一个已启用模组的索引（`i32` 类型），未找到时返回 0。
    pub fn get_enabled_mod_index_in_group(mods_in_group: &[ModData]) -> i32 {
        mods_in_group
            .iter()
            .position(|m| !m.is_disabled && m.mod_path != "None")
            .map(|idx| idx as i32)
            .unwrap_or(0)
    }

    /// 校验传入的 Mods 路径是否为合法的 3DMigoto Mods 目录。
    ///
    /// 校验顺序：
    /// 1. 路径存在且为目录 → 否则返回 `InvalidNotExist`
    /// 2. 目录名为 `Mods` → 否则返回 `InvalidNotModsFolder`
    /// 3. 父目录存在 → 否则返回 `InvalidNotModsFolder`
    /// 4. 父目录下存在 `d3dx.ini` → 否则返回 `InvalidMissingD3dx`
    /// 5. 父目录下存在 `d3d11.dll` → 否则返回 `InvalidMissingDll`
    /// 6. Mods 下存在 `_MANAGED_` 文件夹 → 否则返回 `InvalidWithoutManagedFolder`
    /// 7. 全部通过返回 `Valid`
    ///
    /// 参数：
    /// - `mods_path`: 待校验的 Mods 目录绝对路径字符串。
    ///
    /// 返回：`ModsPathStatus` 枚举值，表示首个失败的检查项或最终通过状态。
    pub fn validate_mods_path(mods_path: &str) -> ModsPathStatus {
        let path = Path::new(mods_path);

        if !path.exists() || !path.is_dir() {
            log::warn!("Mods path does not exist or is not a directory: {}", mods_path);
            return ModsPathStatus::InvalidNotExist;
        }

        let folder_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if folder_name != "Mods" {
            log::warn!("Mods path directory name is not 'Mods': {}", folder_name);
            return ModsPathStatus::InvalidNotModsFolder;
        }

        let parent = match path.parent() {
            Some(p) => p,
            None => {
                log::warn!("Mods path has no parent directory: {}", mods_path);
                return ModsPathStatus::InvalidNotModsFolder;
            }
        };

        #[cfg(not(target_os = "linux"))]
        {
            let d3dx_path = parent.join("d3dx.ini");
            if !d3dx_path.exists() {
                log::warn!("d3dx.ini not found in parent directory: {:?}", d3dx_path);
                return ModsPathStatus::InvalidMissingD3dx;
            }

            let dll_path = parent.join("d3d11.dll");
            if !dll_path.exists() {
                log::warn!("d3d11.dll not found in parent directory: {:?}", dll_path);
                return ModsPathStatus::InvalidMissingDll;
            }
        }

        #[cfg(target_os = "linux")]
        {
            log::debug!("Running on Linux, skipping d3dx.ini and d3d11.dll checks");
        }

        let managed_path = path.join(MANAGED_FOLDER);
        if !managed_path.exists() || !managed_path.is_dir() {
            log::warn!("_MANAGED_ folder does not exist in mods path: {:?}", managed_path);
            return ModsPathStatus::InvalidWithoutManagedFolder;
        }

        log::debug!("Mods path validation passed: {}", mods_path);
        ModsPathStatus::Valid
    }

    /// 根据目标游戏类型，从全局设置中取出对应的 Mods 路径。
    ///
    /// 参数：
    /// - `settings`: 全局设置（包含各游戏的 Mods 路径）。
    /// - `game`: 目标游戏枚举值。
    ///
    /// 返回：对应游戏的 Mods 路径字符串；`TargetGame::None` 返回空字符串。
    pub fn get_mods_path_for_game(settings: &Settings, game: TargetGame) -> String {
        match game {
            TargetGame::None => String::new(),
            TargetGame::WutheringWaves => settings.mods_path_wuwa.clone(),
            TargetGame::GenshinImpact => settings.mods_path_genshin.clone(),
            TargetGame::HonkaiStarRail => settings.mods_path_hsr.clone(),
            TargetGame::ZenlessZoneZero => settings.mods_path_zzz.clone(),
            TargetGame::ArknightsEndfield => settings.mods_path_endfield.clone(),
        }
    }

    /// 获取 `_MANAGED_` 目录下所有 `group_<index>` 形式的分组路径列表。
    ///
    /// 仅识别名称形如 `group_<数字>` 的目录，其他目录（如 `#` 开头或普通目录）不会被包含。
    /// 返回结果按 index 升序排序。
    ///
    /// 参数：
    /// - `mods_path`: Mods 根目录路径。
    ///
    /// 返回：`Vec<(分组路径字符串, 分组索引)>`。若 Mods 路径或 `_MANAGED_` 不存在则返回空 Vec。
    pub fn get_group_folders(mods_path: &str) -> Result<Vec<(String, i32)>> {
        let mods_path = Path::new(mods_path);
        if !mods_path.exists() || !mods_path.is_dir() {
            return Ok(Vec::new());
        }

        let managed_path = mods_path.join(MANAGED_FOLDER);
        if !managed_path.exists() || !managed_path.is_dir() {
            return Ok(Vec::new());
        }

        let mut group_paths: Vec<(i32, PathBuf)> = Vec::new();

        if let Ok(entries) = fs::read_dir(&managed_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.starts_with("group_") {
                        if let Some(idx_str) = name.strip_prefix("group_") {
                            if let Ok(idx) = idx_str.parse::<i32>() {
                                group_paths.push((idx, path));
                            }
                        }
                    }
                }
            }
        }

        // 按分组索引升序排序，保证返回顺序稳定
        group_paths.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(group_paths
            .into_iter()
            .map(|(idx, p)| (p.to_string_lossy().to_string(), idx))
            .collect())
    }

    /// 读取分组的显示名称。
    ///
    /// 优先读取分组目录下的 `groupname` 文件内容（去除首尾空白）；
    /// 若该文件不存在，则回退到目录名，并将回退值写回 `groupname` 文件以固化。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    ///
    /// 返回：分组显示名称字符串。读取/写入失败时返回 `anyhow::Error`。
    pub fn get_group_name(group_path: &str) -> Result<String> {
        let group_path = Path::new(group_path);
        let name_file = group_path.join(GROUP_NAME_FILE);

        if name_file.exists() {
            let content = fs::read_to_string(&name_file)
                .with_context(|| format!("Failed to read group name: {:?}", name_file))?;
            Ok(content.trim().to_string())
        } else {
            // 文件不存在时回退到目录名，并持久化写回
            let folder_name = group_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("group")
                .to_string();
            fs::write(&name_file, &folder_name)
                .with_context(|| format!("Failed to write group name: {:?}", name_file))?;
            Ok(folder_name)
        }
    }

    /// 设置分组的显示名称（覆盖写入 `groupname` 文件）。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    /// - `name`: 要写入的分组名称。
    pub fn set_group_name(group_path: &str, name: &str) -> Result<()> {
        let group_path = Path::new(group_path);
        let name_file = group_path.join(GROUP_NAME_FILE);
        fs::write(&name_file, name)
            .with_context(|| format!("Failed to write group name: {:?}", name_file))?;
        Ok(())
    }

    /// 读取模组的显示名称。
    ///
    /// 优先读取模组目录下的 `modname` 文件内容；若不存在，则回退到目录名，
    /// 并在回退时去除 `DISABLED` 前缀（若存在），随后将回退值写回 `modname` 文件。
    ///
    /// 参数：
    /// - `mod_path`: 模组目录路径。
    ///
    /// 返回：模组显示名称字符串。
    pub fn get_mod_name(mod_path: &str) -> Result<String> {
        let mod_path = Path::new(mod_path);
        let name_file = mod_path.join(MOD_NAME_FILE);

        if name_file.exists() {
            let content = fs::read_to_string(&name_file)
                .with_context(|| format!("Failed to read mod name: {:?}", name_file))?;
            Ok(content.trim().to_string())
        } else {
            // 回退到目录名，去除禁用前缀
            let folder_name = mod_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("mod")
                .to_string();
            let display_name = if Self::is_disabled_name(&folder_name) {
                folder_name[DISABLED_PREFIX.len()..].trim_start_matches('_').to_string()
            } else {
                folder_name.clone()
            };
            fs::write(&name_file, &display_name)
                .with_context(|| format!("Failed to write mod name: {:?}", name_file))?;
            Ok(display_name)
        }
    }

    /// 获取分组内当前选中的模组索引。
    ///
    /// 读取分组目录下的 `selectedindex` 文件，解析为 i32。
    /// 若文件不存在则写入 `"0"` 并返回 0；若解析值越界（< 0 或 >= mods_count）则返回 0。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    /// - `mods_count`: 该分组内模组的总数（用于边界校验）。
    ///
    /// 返回：当前选中的模组索引（保证在 `[0, mods_count)` 范围内）。
    pub fn get_selected_mod_in_group(group_path: &str, mods_count: usize) -> Result<i32> {
        let group_path = Path::new(group_path);
        let index_file = group_path.join(SELECTED_INDEX_FILE);

        if index_file.exists() {
            let content = fs::read_to_string(&index_file)
                .with_context(|| format!("Failed to read selected index: {:?}", index_file))?;
            let index = content.trim().parse::<i32>().unwrap_or(0);
            if index >= 0 && index < mods_count as i32 {
                Ok(index)
            } else {
                // 越界时回退到 0
                Ok(0)
            }
        } else {
            // 文件不存在时初始化为 0
            fs::write(&index_file, "0")
                .with_context(|| format!("Failed to write selected index: {:?}", index_file))?;
            Ok(0)
        }
    }

    /// 持久化分组内当前选中的模组索引。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    /// - `index`: 要保存的模组索引。
    pub fn set_selected_mod_in_group(group_path: &str, index: i32) -> Result<()> {
        let group_path = Path::new(group_path);
        let index_file = group_path.join(SELECTED_INDEX_FILE);
        fs::write(&index_file, index.to_string())
            .with_context(|| format!("Failed to write selected index: {:?}", index_file))?;
        Ok(())
    }

    /// 获取 `_MANAGED_` 目录下当前选中的分组索引。
    ///
    /// 逻辑同 `get_selected_mod_in_group`，但读取 `_MANAGED_` 目录下的 `selectedindex` 文件。
    ///
    /// 参数：
    /// - `managed_path`: `_MANAGED_` 目录路径。
    /// - `group_count`: 分组总数（用于边界校验）。
    pub fn get_selected_group_index(managed_path: &str, group_count: usize) -> Result<i32> {
        let managed_path = Path::new(managed_path);
        let index_file = managed_path.join(SELECTED_INDEX_FILE);

        if index_file.exists() {
            let content = fs::read_to_string(&index_file).with_context(|| {
                format!("Failed to read selected group index: {:?}", index_file)
            })?;
            let index = content.trim().parse::<i32>().unwrap_or(0);
            if index >= 0 && index < group_count as i32 {
                Ok(index)
            } else {
                Ok(0)
            }
        } else {
            fs::write(&index_file, "0").with_context(|| {
                format!("Failed to write selected group index: {:?}", index_file)
            })?;
            Ok(0)
        }
    }

    /// 持久化 `_MANAGED_` 目录下当前选中的分组索引。
    ///
    /// 参数：
    /// - `managed_path`: `_MANAGED_` 目录路径。
    /// - `index`: 要保存的分组索引。
    pub fn set_selected_group_index(managed_path: &str, index: i32) -> Result<()> {
        let managed_path = Path::new(managed_path);
        let index_file = managed_path.join(SELECTED_INDEX_FILE);
        fs::write(&index_file, index.to_string())
            .with_context(|| format!("Failed to write selected group index: {:?}", index_file))?;
        Ok(())
    }

    /// 判断指定路径是否已收藏，并返回收藏时间戳。
    ///
    /// 读取顺序：
    /// 1. 优先读取 XXMI-NRMM 原生 `.favorite` 文件：
    ///    - 文件存在且内容非空：返回 `Some(内容)`。
    ///    - 文件存在但内容为空或读取失败：返回 `Some(当前UTC时间)`（兼容旧数据）。
    /// 2. 若 `.favorite` 不存在，回退读取 NRMM 的 `fav` 文件：
    ///    - 文件存在：以文件修改时间作为收藏时间，返回 `Some(UTC时间)`。
    /// 3. 两个文件都不存在：返回 `None`（未收藏）。
    ///
    /// 参数：
    /// - `path`: 模组或分组目录路径。
    pub fn is_favorite(path: &str) -> Result<Option<String>> {
        let path = Path::new(path);

        // 优先读取 XXMI-NRMM 原生 .favorite 文件
        let fav_path = path.join(FAVORITE_FILE);
        if fav_path.exists() {
            match fs::read_to_string(&fav_path) {
                Ok(content) => {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        return Ok(Some(trimmed.to_string()));
                    }
                    // 旧版数据可能写入空内容，回退到当前时间
                    return Ok(Some(Self::current_datetime_string()));
                }
                Err(_) => return Ok(Some(Self::current_datetime_string())),
            }
        }

        // 兼容 NRMM 的 fav 文件：以文件修改时间作为收藏时间
        let nrmm_fav_path = path.join(NRMM_FAVORITE_FILE);
        if nrmm_fav_path.exists() {
            let metadata = fs::metadata(&nrmm_fav_path)
                .with_context(|| format!("Failed to read fav file metadata: {:?}", nrmm_fav_path))?;
            let mtime = metadata
                .modified()
                .with_context(|| format!("Failed to get fav file mtime: {:?}", nrmm_fav_path))?;
            let datetime = OffsetDateTime::from(mtime);
            let formatted = datetime
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| format!("{}", datetime));
            return Ok(Some(formatted));
        }

        Ok(None)
    }

    /// 切换路径的收藏状态（收藏 ↔ 取消收藏）。
    ///
    /// - 已收藏：删除 `.favorite` 文件，返回 `false`。
    /// - 未收藏：写入当前 UTC 时间到 `.favorite` 文件，返回 `true`。
    ///
    /// 参数：
    /// - `path`: 模组或分组目录路径。
    ///
    /// 返回：操作后该路径是否处于收藏状态。
    /// 错误：路径不存在或非目录时返回 `anyhow::Error`。
    pub fn toggle_favorite(path: &str) -> Result<bool> {
        let path = Path::new(path);
        if !path.exists() || !path.is_dir() {
            anyhow::bail!("Path does not exist or is not a directory: {:?}", path);
        }

        let fav_path = path.join(FAVORITE_FILE);
        if fav_path.exists() {
            Self::move_to_trash(&fav_path)
                .with_context(|| format!("Failed to move favorite file to trash: {:?}", fav_path))?;
            Ok(false)
        } else {
            let datetime = Self::current_datetime_string();
            fs::write(&fav_path, &datetime)
                .with_context(|| format!("Failed to write favorite file: {:?}", fav_path))?;
            Ok(true)
        }
    }

    /// 查找目录中的图标文件路径。
    ///
    /// 查找策略：
    /// 1. 优先按 `ICON_EXTENSIONS` 顺序匹配 `icon.<ext>`（如 `icon.png`）。
    /// 2. 若未找到，遍历目录中所有文件，返回第一个扩展名匹配的文件。
    /// 3. 仍未找到则返回 `None`。
    ///
    /// 参数：
    /// - `path`: 模组或分组目录路径。
    pub fn get_icon_path(path: impl Into<PathBuf>) -> Option<String> {
        let dir_path = path.into();
        // 第一阶段：按优先顺序查找 icon.<ext>
        for ext in ICON_EXTENSIONS {
            let icon_name = format!("icon.{}", ext);
            let icon_path = dir_path.join(&icon_name);
            if icon_path.exists() {
                return Some(icon_path.to_string_lossy().to_string());
            }
        }

        // 第二阶段：回退到目录中任意匹配扩展名的文件
        if let Ok(entries) = fs::read_dir(&dir_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        let ext_lower = ext.to_lowercase();
                        if ICON_EXTENSIONS.iter().any(|e| *e == ext_lower) {
                            return Some(path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }

        None
    }

    /// 将指定源图标文件复制到目标目录下作为 `icon.png`。
    ///
    /// 参数：
    /// - `path`: 目标目录路径。
    /// - `icon_source_path`: 源图标文件的路径。
    ///
    /// 错误：源文件不存在或复制失败时返回 `anyhow::Error`。
    pub fn set_icon(path: &str, icon_source_path: &str) -> Result<()> {
        let target_path = Path::new(path).join("icon.png");
        let source_path = Path::new(icon_source_path);

        if !source_path.exists() {
            anyhow::bail!("Source icon file does not exist: {:?}", source_path);
        }

        fs::copy(source_path, &target_path).with_context(|| {
            format!(
                "Failed to copy icon: {:?} -> {:?}",
                source_path, target_path
            )
        })?;

        Ok(())
    }

    /// 移除目录下的图标文件（仅删除 `icon.<ext>`，不影响其他文件）。
    ///
    /// 按 `ICON_EXTENSIONS` 顺序查找并删除第一个存在的 `icon.<ext>` 文件。
    /// 若不存在任何图标文件，则静默返回 `Ok(())`。
    ///
    /// 参数：
    /// - `path`: 目标目录路径。
    pub fn remove_icon(path: &str) -> Result<()> {
        let dir_path = Path::new(path);
        for ext in ICON_EXTENSIONS {
            let icon_name = format!("icon.{}", ext);
            let icon_path = dir_path.join(&icon_name);
            if icon_path.exists() {
                Self::move_to_trash(&icon_path)
                    .with_context(|| format!("Failed to remove icon: {:?}", icon_path))?;
                return Ok(());
            }
        }
        Ok(())
    }

    /// 切换模组的启用/禁用状态。
    ///
    /// 通过重命名模组目录实现：在目录名前添加或移除 `DISABLED` 前缀。
    /// 3DMigoto 会忽略 `DISABLED` 开头的目录，从而实现禁用效果。
    ///
    /// 参数：
    /// - `mod_path`: 模组目录路径。
    ///
    /// 返回：操作后该模组是否处于「已禁用」状态（`true` 表示已禁用）。
    /// 错误：
    /// - 路径不存在或非目录。
    /// - 路径无父目录。
    /// - 目标路径已存在（避免覆盖）。
    /// - 重命名失败。
    pub fn toggle_mod_disabled(mod_path: &str) -> Result<bool> {
        let path = Path::new(mod_path);
        if !path.exists() || !path.is_dir() {
            error!("toggle_mod_disabled failed: mod path does not exist or is not a directory: {:?}", path);
            anyhow::bail!("Mod path does not exist: {:?}", path);
        }

        let parent = match path.parent() {
            Some(p) => p,
            None => {
                error!("toggle_mod_disabled failed: mod path has no parent directory: {:?}", path);
                anyhow::bail!("Invalid mod path: no parent directory");
            }
        };

        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let is_disabled = Self::is_disabled_name(&dir_name);
        // 计算新目录名：禁用 → 启用（移除前缀），启用 → 禁用（添加前缀）
        let new_name = if is_disabled {
            dir_name[DISABLED_PREFIX.len()..].trim_start_matches('_').to_string()
        } else {
            format!("{}{}", DISABLED_PREFIX, dir_name)
        };

        let new_path = parent.join(&new_name);
        if new_path.exists() {
            error!("toggle_mod_disabled failed: destination path already exists: {:?}", new_path);
            anyhow::bail!("Destination path already exists: {:?}", new_path);
        }

        if let Err(e) = fs::rename(path, &new_path) {
            error!("toggle_mod_disabled failed to rename mod: {:?} -> {:?}: {}", path, new_path, e);
            return Err(e).with_context(|| format!("Failed to rename mod: {:?} -> {:?}", path, new_path));
        }

        // 启用时递归去除内部嵌套的 DISABLED 前缀
        if is_disabled {
            if let Ok(stripped) = Self::strip_disabled_prefixes_deep(&new_path) {
                if stripped > 0 {
                    info!("Stripped {} DISABLED prefixes inside {:?}", stripped, new_path);
                }
            }
        }

        // 返回操作后的禁用状态（与原状态相反）
        Ok(!is_disabled)
    }

    /// 深度优先遍历目录树，使用显式栈迭代移除所有文件/目录名中的 DISABLED 前缀。
    ///
    /// 此函数使用迭代式显式栈进行 DFS 遍历，避免递归深度过大导致栈溢出。包含深度限制
    /// (`DEFAULT_MAX_TRAVERSAL_DEPTH`) 和符号链接保护：不跟随符号链接，同时使用
    /// canonical 路径 visited 集合防止循环引用导致的无限遍历。若文件/目录名
    /// 恰好等于 `DISABLED`（不区分大小写）本身（无后续内容），则跳过不处理，避免误删除
    /// 名称为 DISABLED 的文件/目录。
    ///
    /// 参数：
    /// - `dir`: 要处理的目录路径（已去除顶层 DISABLED 前缀后的路径）。
    ///
    /// 返回：成功处理的条目数量。
    pub fn strip_disabled_prefixes_deep(dir: &Path) -> Result<usize> {
        let mut count = 0;

        if !dir.exists() || !dir.is_dir() {
            return Ok(0);
        }

        let pool = VisitedPathPool::new();
        let root_canonical = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        pool.mark(&root_canonical);

        let mut stack: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];

        while let Some((current_dir, depth)) = stack.pop() {
            if depth >= DEFAULT_MAX_TRAVERSAL_DEPTH {
                warn!("Max traversal depth reached at {:?}, skipping deeper", current_dir);
                continue;
            }

            if !current_dir.exists() || !current_dir.is_dir() {
                continue;
            }

            if current_dir.is_symlink() {
                continue;
            }

            let entries: Vec<PathBuf> = fs::read_dir(&current_dir)
                .with_context(|| format!("Failed to read directory: {:?}", current_dir))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .collect();

            for entry_path in entries {
                if entry_path.is_symlink() {
                    continue;
                }

                let entry_canonical = fs::canonicalize(&entry_path)
                    .unwrap_or_else(|_| entry_path.clone());
                if pool.check_and_mark(&entry_canonical) {
                    continue;
                }

                let name = entry_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");

                if name.eq_ignore_ascii_case(DISABLED_PREFIX) {
                    continue;
                }

                if Self::is_disabled_name(name) {
                    let new_name = name[DISABLED_PREFIX.len()..].trim_start_matches('_').to_string();
                    let parent = entry_path.parent().unwrap_or(&current_dir);
                    let new_path = parent.join(&new_name);

                    if !new_path.exists() {
                        fs::rename(&entry_path, &new_path).with_context(|| {
                            format!("Failed to rename {:?} -> {:?}", entry_path, new_path)
                        })?;
                        count += 1;

                        if new_path.is_dir() {
                            let new_canonical = fs::canonicalize(&new_path)
                                .unwrap_or_else(|_| new_path.clone());
                            pool.mark(&new_canonical);
                            stack.push((new_path, depth + 1));
                        }
                    }
                } else if entry_path.is_dir() {
                    stack.push((entry_path, depth + 1));
                }
            }
        }

        Ok(count)
    }

    /// 判断给定路径是否位于某个 # 目录下（向上遍历查找最近的 # 开头目录）。
    ///
    /// 参数：
    /// - `path`: 待判断的路径。
    ///
    /// 返回：
    /// - `Some(PathBuf)`：找到的 # 目录路径。
    /// - `None`：不在任何 # 目录下。
    fn find_parent_tree_node(path: &Path) -> Option<PathBuf> {
        let mut current = path.parent()?;
        loop {
            if let Some(name) = current.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('#') {
                    return Some(current.to_path_buf());
                }
            }
            match current.parent() {
                Some(parent) if parent != current => current = parent,
                _ => return None,
            }
        }
    }

    /// 切换树节点（# 目录）下模组的启用/禁用状态（互斥模式）。
    ///
    /// 与普通 `toggle_mod_disabled` 的区别：
    /// - **启用操作**：先禁用同 # 目录下所有其他模组，再启用目标模组（单选互斥）。
    /// - **禁用操作**：直接禁用目标模组，不影响其他模组。
    /// - 不涉及 INI 文件修改，纯靠目录重命名实现。
    ///
    /// 参数：
    /// - `mod_path`: 目标模组目录路径。
    ///
    /// 返回：`(新模组路径, 操作后是否禁用)`。
    pub fn toggle_tree_node_mod_disabled(mod_path: &str) -> Result<(String, bool)> {
        let path = Path::new(mod_path);
        if !path.exists() || !path.is_dir() {
            anyhow::bail!("Mod path does not exist: {:?}", path);
        }

        let parent = match path.parent() {
            Some(p) => p,
            None => anyhow::bail!("Invalid mod path: no parent directory"),
        };

        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let is_disabled = Self::is_disabled_name(&dir_name);

        if is_disabled {
            // 禁用 -> 启用：先禁用同目录下所有其他启用的模组，再启用目标
            let tree_node_dir =
                Self::find_parent_tree_node(path).unwrap_or_else(|| parent.to_path_buf());

            // 第一步：遍历同级目录，禁用所有启用的非 #、非隐藏目录
            if let Ok(entries) = fs::read_dir(&tree_node_dir) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if !entry_path.is_dir() {
                        continue;
                    }
                    let entry_name = entry_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("");
                    // 跳过 # 子目录、隐藏目录、已经禁用的目录
                    if entry_name.starts_with('#')
                        || entry_name.starts_with('.')
                        || Self::is_disabled_name(entry_name)
                    {
                        continue;
                    }
                    // 跳过目标模组（后面单独处理）
                    if entry_path == path {
                        continue;
                    }
                    // 禁用该模组（添加 DISABLED 前缀）
                    let new_name = format!("{}{}", DISABLED_PREFIX, entry_name);
                    let new_path = tree_node_dir.join(&new_name);
                    if !new_path.exists() {
                        if let Err(e) = fs::rename(&entry_path, &new_path) {
                            warn!("Failed to disable tree node mod {:?} -> {:?}: {}", entry_path, new_path, e);
                        }
                    }
                }
            }

            // 第二步：启用目标模组（移除 DISABLED 前缀）
            let new_name = dir_name[DISABLED_PREFIX.len()..].trim_start_matches('_').to_string();
            let new_path = parent.join(&new_name);
            if new_path.exists() {
                anyhow::bail!("Destination path already exists: {:?}", new_path);
            }
            fs::rename(path, &new_path)
                .with_context(|| format!("Failed to rename mod: {:?} -> {:?}", path, new_path))?;

            // 启用时递归去除内部嵌套的 DISABLED 前缀
            if let Ok(stripped) = Self::strip_disabled_prefixes_deep(&new_path) {
                if stripped > 0 {
                    info!("Stripped {} DISABLED prefixes inside {:?}", stripped, new_path);
                }
            }

            Ok((new_path.to_string_lossy().to_string(), false))
        } else {
            // 启用 -> 禁用：直接禁用，不影响其他模组
            let new_name = format!("{}{}", DISABLED_PREFIX, dir_name);
            let new_path = parent.join(&new_name);
            if new_path.exists() {
                anyhow::bail!("Destination path already exists: {:?}", new_path);
            }
            fs::rename(path, &new_path)
                .with_context(|| format!("Failed to rename mod: {:?} -> {:?}", path, new_path))?;

            Ok((new_path.to_string_lossy().to_string(), true))
        }
    }

    /// 安全禁用指定模组目录（仅添加 `DISABLED` 前缀，不切换）。
    ///
    /// 与 `toggle_tree_node_mod_disabled` 不同，本函数只执行禁用方向的重命名：
    /// - 若目录名尚未以 `DISABLED` 开头，则添加前缀；
    /// - 若已处于禁用状态，直接返回原路径。
    ///
    /// 参数：
    /// - `mod_path`: 目标模组目录路径。
    ///
    /// 返回：操作后的新路径字符串。
    /// 错误：
    /// - 路径不存在或非目录。
    /// - 目录名包含非 UTF-8 字符。
    /// - 目标路径已存在（避免覆盖）。
    /// - 重命名失败。
    pub fn disable_tree_node_mod(mod_path: &str) -> Result<String> {
        let path = Path::new(mod_path);
        if !path.exists() || !path.is_dir() {
            error!("disable_tree_node_mod failed: mod path does not exist or is not a directory: {:?}", path);
            anyhow::bail!("Mod path does not exist: {:?}", path);
        }

        let parent = match path.parent() {
            Some(p) => p,
            None => {
                error!("disable_tree_node_mod failed: mod path has no parent directory: {:?}", path);
                anyhow::bail!("Invalid mod path: no parent directory");
            }
        };

        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                error!("disable_tree_node_mod failed: directory name is not valid UTF-8: {:?}", path);
                anyhow::anyhow!("Directory name is not valid UTF-8: {:?}", path)
            })?;

        if Self::is_disabled_name(dir_name) {
            return Ok(mod_path.to_string());
        }

        let new_name = format!("{}{}", DISABLED_PREFIX, dir_name);
        let new_path = parent.join(&new_name);
        if new_path.exists() {
            error!("disable_tree_node_mod failed: destination path already exists: {:?}", new_path);
            anyhow::bail!("Destination path already exists: {:?}", new_path);
        }

        fs::rename(path, &new_path)
            .with_context(|| format!("Failed to rename mod: {:?} -> {:?}", path, new_path))?;

        Ok(new_path.to_string_lossy().to_string())
    }

    /// 扫描 Mods 目录下的所有分组。
    ///
    /// 默认读取 `_MANAGED_` 目录下的内容，支持三种分组识别方式：
    ///
    /// 1. **`group_<index>` 目录**：标准命名形式，调用 `scan_single_group` 处理，
    ///    `real_index` 取目录名中的数字。
    /// 2. **`#` 开头目录**：递归展开形式，调用 `scan_recursive_with_queue` 收集所有
    ///    `#` 开头的子目录，每个子目录作为一个分组，`real_index` 从 1 开始递增。
    /// 3. **普通目录**：若通过 `is_mod_directory` 判定为模组目录（包含 icon.png 或 *.ini），
    ///    则该目录本身作为一个分组，`real_index` 从 1 开始递增。
    ///
    /// 扫描完成后按 `sort_method` 排序：收藏的分组优先，其次按索引或名称排序。
    ///
    /// 参数：
    /// - `mods_path`: Mods 根目录路径。
    /// - `sort_method`: 分组排序方式。
    ///
    /// 返回：分组数据列表。Mods 路径或 `_MANAGED_` 不存在时返回空 Vec。
    pub fn scan_groups(
        managed_path_str: &str,
        sort_method: SortGroupMethod,
    ) -> Result<Vec<ModGroupData>> {
        let managed_path = Path::new(managed_path_str);
        if !managed_path.exists() || !managed_path.is_dir() {
            return Ok(Vec::new());
        }

        let mut groups: Vec<ModGroupData> = Vec::new();
        // 收集所有 group_<index> 目录，后续统一归入虚拟 "Group" 分类节点
        let mut group_children: Vec<ModGroupData> = Vec::new();
        // 用于递归/普通目录的递增索引（group_ 形式有自己的索引）
        let mut index: i32 = 1;

        if let Ok(entries) = fs::read_dir(&managed_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.is_empty() {
                        continue;
                    };
                    if name.starts_with("group_") {
                        // 情况 1：标准 group_<index> 目录，归入 "Group" 分类
                        if let Some(idx_str) = name.strip_prefix("group_") {
                            if let Ok(idx) = idx_str.parse::<i32>() {
                                if let Ok(group) = Self::scan_single_group(&path, idx) {
                                    group_children.push(group);
                                }
                            }
                        }
                    } else if name.starts_with('#') {
                        // 情况 2：# 开头目录，构建树形嵌套结构
                        if let Some(tree_group) = Self::scan_tree_node(&path) {
                            groups.push(tree_group);
                        }
                    } else if !name.starts_with('.') && Self::is_mod_directory(&path) {
                        // 情况 3：普通模组目录（跳过隐藏目录 .xxx）
                        let group_name = name.to_string();
                        if let Ok(group) =
                            Self::scan_single_group_by_path(&path, index, &group_name)
                        {
                            groups.push(group);
                            index += 1;
                        }
                    }
                }
            }
        }

        // 若存在 group_<index> 目录，创建虚拟 "Group" 父分类节点
        if !group_children.is_empty() {
            // 子分组按 real_index 升序排序，保证分类内顺序稳定
            group_children.sort_by(|a, b| a.real_index.cmp(&b.real_index));
            let group_node = ModGroupData {
                group_path: format!("{}__virtual__Group", managed_path_str),
                icon_path: None,
                group_name: "Group".to_string(),
                favorite_date_time: None,
                mods_in_group: Vec::new(),
                real_index: 0,
                previous_selected_mod_on_group: 0,
                children: group_children,
                is_tree_node: true,
                is_virtual: true,
                is_disabled: false,
            };
            groups.push(group_node);
        }

        // 排序：收藏优先，其次按指定方式
        Self::sort_groups(&mut groups, sort_method);
        info!("Scanned {} groups from {:?}", groups.len(), managed_path);
        Ok(groups)
    }

    /// 使用队列（BFS）递归收集所有以 `#` 开头的子目录。
    ///
    /// - 采用迭代式广度优先搜索而非递归，避免栈溢出
    /// - 自动解析符号链接并去重，避免循环引用死循环
    /// - 同时存储原始路径和 canonical 绝对路径用于去重
    ///
    /// 参数：
    /// - `base_path`: 起始目录路径（应为 `#` 开头目录）。
    ///
    /// 返回：所有 `#` 开头子目录的路径列表（包含 `base_path` 自身）。
    /// 使用显式栈（非递归）扫描目录树，构建树形嵌套结构。
    ///
    /// 所有子目录均参与分类：
    /// - 模组目录（含 .ini 或 icon.*）：构建为 ModData，加入 mods_in_group
    /// - 常规目录（不含上述文件）：作为子树节点，加入 children，继续深入
    ///
    /// index 由函数内部自行维护，类型为 u32。
    ///
    /// 参数：
    /// - `base_path`: 起始目录路径。
    ///
    /// 返回：构建完成的 ModGroupData（树形结构）。
    fn scan_tree_node(base_path: &Path) -> Option<ModGroupData> {
        struct NodeInfo {
            /// 当前节点路径（保留用于调试，当前未读取）。
            #[allow(dead_code)]
            path: PathBuf,
            child_paths: Vec<PathBuf>,
            mod_paths: Vec<PathBuf>,
            // 模组 real_index 映射表：key 为模组目录路径，value 为目录列表原始顺序位置（1-indexed）
            // 与 NRMM 的 `realIndex: index + 1` 一致，real_index 来自原始目录列表顺序而非排序后顺序
            mod_original_indices: std::collections::HashMap<PathBuf, i32>,
            is_disabled: bool,
        }

        let pool = VisitedPathPool::new();
        let mut node_info_map: std::collections::HashMap<PathBuf, NodeInfo> =
            std::collections::HashMap::new();
        let mut post_order: Vec<PathBuf> = Vec::new();
        let mut stack: Vec<(PathBuf, usize)> = Vec::new();

        stack.push((base_path.to_path_buf(), 0));

        while let Some((path, depth)) = stack.pop() {
            if depth >= DEFAULT_MAX_TRAVERSAL_DEPTH {
                warn!("Max traversal depth reached at {:?} in scan_tree_node, skipping deeper", path);
                continue;
            }

            if !path.exists() || !path.is_dir() {
                continue;
            }

            if path.is_symlink() {
                let target_canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if pool.check_and_mark(&target_canonical) {
                    continue;
                }
            } else {
                let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if pool.check_and_mark(&canonical) {
                    continue;
                }
            }

            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let is_disabled = Self::is_disabled_name(&dir_name);

            let mut mod_paths: Vec<PathBuf> = Vec::new();
            let mut child_paths: Vec<PathBuf> = Vec::new();

            if let Ok(entries) = fs::read_dir(&path) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if !entry_path.is_dir() {
                        continue;
                    }

                    let name = entry_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("");
                    if name.starts_with('.') {
                        continue;
                    }

                    if Self::is_mod_directory(&entry_path) {
                        mod_paths.push(entry_path);
                    } else {
                        child_paths.push(entry_path);
                    }
                }
            }

            // 在排序前记录每个模组目录的原始顺序索引（与 NRMM 的 realIndex: index + 1 一致）
            // real_index 必须来自 fs::read_dir 原始返回顺序，而非排序后顺序
            let mut mod_original_indices: std::collections::HashMap<PathBuf, i32> =
                std::collections::HashMap::new();
            for (i, mp) in mod_paths.iter().enumerate() {
                mod_original_indices.insert(mp.clone(), (i as i32) + 1);
            }

            mod_paths.sort_by(|a, b| {
                let a_name = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let b_name = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let a_disabled = Self::is_disabled_name(a_name);
                let b_disabled = Self::is_disabled_name(b_name);
                match (a_disabled, b_disabled) {
                    (true, false) => std::cmp::Ordering::Greater,
                    (false, true) => std::cmp::Ordering::Less,
                    _ => {
                        let a_fav_dt = Self::is_favorite(&a.to_string_lossy()).unwrap_or(None);
                        let b_fav_dt = Self::is_favorite(&b.to_string_lossy()).unwrap_or(None);
                        let a_fav = a_fav_dt.is_some();
                        let b_fav = b_fav_dt.is_some();
                        match (a_fav, b_fav) {
                            (true, false) => std::cmp::Ordering::Less,
                            (false, true) => std::cmp::Ordering::Greater,
                            (true, true) => {
                                match (&a_fav_dt, &b_fav_dt) {
                                    (Some(ad), Some(bd)) => bd.cmp(ad),
                                    _ => std::cmp::Ordering::Equal,
                                }
                            }
                            _ => Self::natural_cmp(a_name.to_lowercase().as_str(), b_name.to_lowercase().as_str()),
                        }
                    }
                }
            });

            child_paths.sort_by(|a, b| {
                let a_name = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let b_name = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
                a_name.to_lowercase().cmp(&b_name.to_lowercase())
            });

            node_info_map.insert(
                path.clone(),
                NodeInfo {
                    path: path.clone(),
                    child_paths: child_paths.clone(),
                    mod_paths,
                    mod_original_indices,
                    is_disabled,
                },
            );

            post_order.push(path.clone());

            for child in child_paths.iter().rev() {
                stack.push((child.clone(), depth + 1));
            }
        }

        let mut built_map: std::collections::HashMap<PathBuf, ModGroupData> =
            std::collections::HashMap::new();
        let mut index: u32 = 0;

        for path in post_order.iter().rev() {
            let info = match node_info_map.get(path) {
                Some(i) => i,
                None => continue,
            };

            let group_path_str = path.to_string_lossy().to_string();
            let group_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let icon_path = Self::get_icon_path(path);
            let favorite_date_time = Self::is_favorite(&group_path_str).unwrap_or(None);

            let mut mods_in_group: Vec<ModData> = Vec::new();

            if !info.mod_paths.is_empty() {
                let none_icon_path = path.join(NONE_SLOT_ICON);
                mods_in_group.push(ModData {
                    mod_path: "None".to_string(),
                    icon_path: if none_icon_path.exists() {
                        Some(none_icon_path.to_string_lossy().to_string())
                    } else {
                        None
                    },
                    mod_name: "None".to_string(),
                    real_index: 0,
                    is_old_auto_fixed: false,
                    is_syntax_error_removed: false,
                    is_unoptimized: false,
                    is_namespaced: false,
                    is_disabled: false,
                    favorite_date_time: None,
                });

                for mod_path in &info.mod_paths {
                    let mod_path_str = mod_path.to_string_lossy().to_string();
                    let mod_name = path_utils::get_mod_display_name_readonly(mod_path).unwrap_or_else(|_| {
                        mod_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("mod")
                            .to_string()
                    });
                    let mod_icon = Self::get_icon_path(mod_path);
                    let mod_favorite = Self::is_favorite(&mod_path_str).unwrap_or(None);
                    let mod_dir_name = mod_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let mod_is_disabled = Self::is_disabled_name(mod_dir_name);

                    // real_index 来自目录列表原始顺序（与 NRMM 一致），从 mod_original_indices 查表获取
                    // 注意：此处遍历的是已排序的 mod_paths（用于显示顺序），但 real_index 值来自排序前的原始位置
                    let real_index = info
                        .mod_original_indices
                        .get(mod_path)
                        .copied()
                        .unwrap_or(0);

                    mods_in_group.push(ModData {
                        mod_path: mod_path_str,
                        icon_path: mod_icon,
                        mod_name,
                        real_index,
                        is_old_auto_fixed: mod_path.join("modforced").exists(),
                        is_syntax_error_removed: mod_path.join("modsyntaxerrorremoved").exists(),
                        is_unoptimized: mod_path.join("modunoptimized").exists(),
                        is_namespaced: mod_path.join("modnamespaced").exists(),
                        is_disabled: mod_is_disabled,
                        favorite_date_time: mod_favorite,
                    });
                }
            }

            // # 目录分组根据当前启用的模组推导索引（互斥模式下同一分组最多一个启用模组）
            // 注意：仅在 mods_in_group 非空时设置（递归分组可能没有 mods）
            let previous_selected_mod_on_group = if !mods_in_group.is_empty() {
                Self::get_enabled_mod_index_in_group(&mods_in_group)
            } else {
                0
            };

            let mut children: Vec<ModGroupData> = Vec::new();
            for child_path in &info.child_paths {
                if let Some(child_group) = built_map.get(child_path) {
                    children.push(child_group.clone());
                }
            }

            let current_index = index as i32;
            index += 1;

            info!(
                "Tree node scanned: {} (index={}, children={}, mods={}, disabled={})",
                group_path_str,
                current_index,
                children.len(),
                mods_in_group.len(),
                info.is_disabled
            );

            built_map.insert(
                path.clone(),
                ModGroupData {
                    group_path: group_path_str,
                    icon_path,
                    group_name,
                    favorite_date_time,
                    mods_in_group,
                    real_index: current_index,
                    previous_selected_mod_on_group,
                    children,
                    is_tree_node: true,
                    is_virtual: false,
                    is_disabled: info.is_disabled,
                },
            );
        }

        built_map.remove(base_path)
    }

    /// 判断一个目录是否为「模组目录」。
    ///
    /// 判定标准：目录中存在 `icon.png` 文件或任意 `*.ini` 文件。
    /// 此函数用于 `scan_groups` 中识别普通目录是否应被视为单个分组。
    ///
    /// 参数：
    /// - `path`: 待判定的目录路径。
    ///
    /// 返回：是模组目录返回 `true`，否则返回 `false`。
    fn is_mod_directory(path: &Path) -> bool {
        if !path.exists() || !path.is_dir() {
            return false;
        }

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy().to_lowercase();

                // 存在 icon.png 即判定为模组目录
                if name == "icon.png" {
                    return true;
                }

                // 存在任意 .ini 文件也判定为模组目录
                if name.ends_with(".ini") {
                    return true;
                }
            }
        }
        false
    }

    /// 放宽版的模组目录判定（用于 # 目录下的子目录）。
    ///
    /// 判定标准（满足任一即可）：
    /// 1. 存在 icon.png 或 *.ini 文件（同标准判定）
    /// 2. 目录下有子目录（可能是多层结构的模组）
    /// 3. 目录下有 .txt / .json / .xml 等常见配置文件
    ///
    /// 目的：避免因判定条件过严导致模组丢失。
    ///
    /// 参数：
    /// - `path`: 待判定的目录路径。
    ///
    /// 返回：是模组目录返回 `true`，否则返回 `false`。
    ///
    /// 当前未使用，保留作为放宽判定策略的备用实现。
    #[allow(dead_code)]
    fn is_mod_directory_relaxed(path: &Path) -> bool {
        if !path.exists() || !path.is_dir() {
            return false;
        }

        // 标准判定：icon.png 或 .ini 文件
        if Self::is_mod_directory(path) {
            return true;
        }

        // 放宽判定：检查目录内容
        if let Ok(entries) = fs::read_dir(path) {
            let mut has_sub_dir = false;
            let mut has_config_file = false;

            for entry in entries.flatten() {
                let file_type = match entry.file_type() {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };

                if file_type.is_dir() {
                    has_sub_dir = true;
                } else if file_type.is_file() {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if name.ends_with(".txt")
                        || name.ends_with(".json")
                        || name.ends_with(".xml")
                        || name.ends_with(".toml")
                        || name.ends_with(".yaml")
                        || name.ends_with(".yml")
                        || name.ends_with(".dds")
                        || name.ends_with(".png")
                        || name.ends_with(".jpg")
                        || name.ends_with(".jpeg")
                    {
                        has_config_file = true;
                    }
                }
            }

            // 有子目录 或 有配置/资源文件，视为模组目录
            if has_sub_dir || has_config_file {
                return true;
            }
        }

        false
    }

    /// 通过显式路径与名称扫描单个分组（用于 `#` 开头目录和普通目录）。
    ///
    /// 与 `scan_single_group` 的区别：本函数直接使用传入的 `group_name`，
    /// 而非从 `groupname` 文件读取。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    /// - `real_index`: 分组的真实索引。
    /// - `group_name`: 分组显示名称。
    fn scan_single_group_by_path(
        group_path: &Path,
        real_index: i32,
        group_name: &str,
    ) -> Result<ModGroupData> {
        let group_path_str = group_path.to_string_lossy().to_string();
        let icon_path = Self::get_icon_path(&group_path_str);
        let favorite_date_time = Self::is_favorite(&group_path_str).unwrap_or(None);
        let mods_in_group = Self::get_mods_on_group(&group_path_str)?;
        let mods_count = mods_in_group.len();
        let previous_selected_mod_on_group =
            Self::get_selected_mod_in_group(&group_path_str, mods_count).unwrap_or(0);

        Ok(ModGroupData {
            group_path: group_path_str,
            icon_path,
            group_name: group_name.to_string(),
            favorite_date_time,
            mods_in_group,
            real_index,
            previous_selected_mod_on_group,
            children: vec![],
            is_tree_node: false,
            is_virtual: false,
            is_disabled: false,
        })
    }

    /// 扫描单个标准 `group_<index>` 分组。
    ///
    /// 通过 `get_group_name` 读取分组显示名称（若 `groupname` 文件不存在则回退到目录名）。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    /// - `real_index`: 分组的真实索引。
    fn scan_single_group(group_path: &Path, real_index: i32) -> Result<ModGroupData> {
        let group_path_str = group_path.to_string_lossy().to_string();
        let group_name = Self::get_group_name(&group_path_str).unwrap_or_else(|_| {
            // 读取失败时回退到目录名
            group_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string()
        });

        let icon_path = Self::get_icon_path(&group_path_str);
        let favorite_date_time = Self::is_favorite(&group_path_str).unwrap_or(None);
        let mods_in_group = Self::get_mods_on_group(&group_path_str)?;
        let mods_count = mods_in_group.len();
        let previous_selected_mod_on_group =
            Self::get_selected_mod_in_group(&group_path_str, mods_count).unwrap_or(0);

        Ok(ModGroupData {
            group_path: group_path_str,
            icon_path,
            group_name,
            favorite_date_time,
            mods_in_group,
            real_index,
            previous_selected_mod_on_group,
            children: vec![],
            is_tree_node: false,
            is_virtual: false,
            is_disabled: false,
        })
    }

    /// 获取分组内的所有模组列表。
    ///
    /// 返回列表的第一个元素固定为 None 槽位（`mod_path = "None"`），
    /// 表示「不启用任何模组」。其后依次为分组目录下的所有子目录（模组）。
    ///
    /// 排序规则：
    /// 1. 启用的模组排在禁用的模组之前。
    /// 2. 收藏的模组排在未收藏的模组之前。
    /// 3. 同等状态下按目录名（小写）字母序排序。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    ///
    /// 返回：模组数据列表（首位为 None 槽位）。
    pub fn get_mods_on_group(group_path: &str) -> Result<Vec<ModData>> {
        let group_path = Path::new(group_path);
        if !group_path.exists() || !group_path.is_dir() {
            return Ok(Vec::new());
        }

        let mut mods: Vec<ModData> = Vec::new();
        let mut real_index = 1;

        let entries = fs::read_dir(group_path)
            .with_context(|| format!("Failed to read directory: {:?}", group_path))?;

        let mut dir_entries: Vec<PathBuf> = Vec::new();
        for entry in entries {
            match entry {
                Ok(e) => {
                    let path = e.path();
                    if path.is_dir() {
                        // 过滤掉 # 开头的子目录（树形结构的子分组）
                        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if !name.starts_with('#') {
                            dir_entries.push(path);
                        }
                    }
                }
                Err(e) => warn!("Failed to read entry: {}", e),
            }
        }

        // 插入 None 槽位（索引 0），表示不启用任何模组
        let none_icon_path = group_path.join(NONE_SLOT_ICON);
        mods.push(ModData {
            mod_path: "None".to_string(),
            icon_path: if none_icon_path.exists() {
                Some(none_icon_path.to_string_lossy().to_string())
            } else {
                None
            },
            mod_name: "None".to_string(),
            real_index: 0,
            is_old_auto_fixed: false,
            is_syntax_error_removed: false,
            is_unoptimized: false,
            is_namespaced: false,
            is_disabled: false,
            favorite_date_time: None,
        });

        // 1. 先按目录列表原始顺序构建 ModData，分配 real_index
        //    与 NRMM 的 `realIndex: index + 1` 一致，real_index 来自 fs::read_dir 原始返回顺序
        //    （fs::read_dir 与 Dart Directory.list 在 Windows 上底层均为 FindFirstFile/FindNextFile）
        let mut mod_datas: Vec<ModData> = Vec::new();
        for dir_path in &dir_entries {
            let dir_name = dir_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            // 跳过隐藏目录（以 . 开头）
            if dir_name.starts_with('.') {
                continue;
            }

            let is_disabled = Self::is_disabled_name(&dir_name);
            let dir_path_str = dir_path.to_string_lossy().to_string();
            let mod_name = Self::get_mod_name(&dir_path_str).unwrap_or_else(|_| {
                // 读取 modname 失败时回退到目录名（去除禁用前缀）
                if is_disabled {
                    dir_name[DISABLED_PREFIX.len()..].trim_start_matches('_').to_string()
                } else {
                    dir_name.clone()
                }
            });

            let icon_path = Self::get_icon_path(&dir_path_str);
            let favorite_date_time = Self::is_favorite(&dir_path_str).unwrap_or(None);
            // 通过标记文件判断模组的处理状态
            let is_old_auto_fixed = dir_path.join("modforced").exists();
            let is_syntax_error_removed = dir_path.join("modsyntaxerrorremoved").exists();
            let is_unoptimized = dir_path.join("modunoptimized").exists();
            let is_namespaced = dir_path.join("modnamespaced").exists();

            mod_datas.push(ModData {
                mod_path: dir_path_str,
                icon_path,
                mod_name,
                real_index,
                is_old_auto_fixed,
                is_syntax_error_removed,
                is_unoptimized,
                is_namespaced,
                is_disabled,
                favorite_date_time,
            });

            real_index += 1;
        }

        // 2. 再排序 ModData 向量（用于显示顺序，与 NRMM 排序逻辑一致）
        //    排序规则：禁用状态 → 收藏状态 → 最新收藏优先 → 自然排序
        //    注意：排序仅改变数组顺序，不改变已分配的 real_index 值
        mod_datas.sort_by(|a, b| {
            match (a.is_disabled, b.is_disabled) {
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => {
                    let a_fav = a.favorite_date_time.is_some();
                    let b_fav = b.favorite_date_time.is_some();
                    match (a_fav, b_fav) {
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        (true, true) => {
                            match (&a.favorite_date_time, &b.favorite_date_time) {
                                (Some(ad), Some(bd)) => bd.cmp(ad),
                                _ => std::cmp::Ordering::Equal,
                            }
                        }
                        _ => {
                            let a_name = Path::new(&a.mod_path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("");
                            let b_name = Path::new(&b.mod_path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("");
                            Self::natural_cmp(a_name.to_lowercase().as_str(), b_name.to_lowercase().as_str())
                        }
                    }
                }
            }
        });

        // 3. 将排序后的 mod_datas 追加到 mods（None 槽位已在最前）
        mods.extend(mod_datas);

        Ok(mods)
    }

    /// 只读方式获取分组下的模组列表，不会写入 modname 文件。
    ///
    /// 该函数功能与 `get_mods_on_group` 一致，但使用只读方式获取模组名称，
    /// 不会在模组目录下创建或修改 modname 文件。适用于不允许写入元数据的场景（如 # 目录下的模组）。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    ///
    /// 返回：模组数据列表（首位为 None 槽位）。
    pub fn get_mods_on_group_readonly(group_path: &str) -> Result<Vec<ModData>> {
        let group_path = Path::new(group_path);
        if !group_path.exists() || !group_path.is_dir() {
            return Ok(Vec::new());
        }

        let mut mods: Vec<ModData> = Vec::new();
        let mut real_index = 1;

        let entries = fs::read_dir(group_path)
            .with_context(|| format!("Failed to read directory: {:?}", group_path))?;

        let mut dir_entries: Vec<PathBuf> = Vec::new();
        for entry in entries {
            match entry {
                Ok(e) => {
                    let path = e.path();
                    if path.is_dir() {
                        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if !name.starts_with('#') {
                            dir_entries.push(path);
                        }
                    }
                }
                Err(e) => warn!("Failed to read entry: {}", e),
            }
        }

        let none_icon_path = group_path.join(NONE_SLOT_ICON);
        mods.push(ModData {
            mod_path: "None".to_string(),
            icon_path: if none_icon_path.exists() {
                Some(none_icon_path.to_string_lossy().to_string())
            } else {
                None
            },
            mod_name: "None".to_string(),
            real_index: 0,
            is_old_auto_fixed: false,
            is_syntax_error_removed: false,
            is_unoptimized: false,
            is_namespaced: false,
            is_disabled: false,
            favorite_date_time: None,
        });

        let mut mod_datas: Vec<ModData> = Vec::new();
        for dir_path in &dir_entries {
            let dir_name = dir_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            if dir_name.starts_with('.') {
                continue;
            }

            let is_disabled = Self::is_disabled_name(&dir_name);
            let dir_path_str = dir_path.to_string_lossy().to_string();
            let mod_name = path_utils::get_mod_display_name_readonly(dir_path).unwrap_or_else(|_| {
                if is_disabled {
                    dir_name[DISABLED_PREFIX.len()..].trim_start_matches('_').to_string()
                } else {
                    dir_name.clone()
                }
            });

            let icon_path = Self::get_icon_path(&dir_path_str);
            let favorite_date_time = Self::is_favorite(&dir_path_str).unwrap_or(None);
            let is_old_auto_fixed = dir_path.join("modforced").exists();
            let is_syntax_error_removed = dir_path.join("modsyntaxerrorremoved").exists();
            let is_unoptimized = dir_path.join("modunoptimized").exists();
            let is_namespaced = dir_path.join("modnamespaced").exists();

            mod_datas.push(ModData {
                mod_path: dir_path_str,
                icon_path,
                mod_name,
                real_index,
                is_old_auto_fixed,
                is_syntax_error_removed,
                is_unoptimized,
                is_namespaced,
                is_disabled,
                favorite_date_time,
            });

            real_index += 1;
        }

        mod_datas.sort_by(|a, b| {
            match (a.is_disabled, b.is_disabled) {
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => {
                    let a_fav = a.favorite_date_time.is_some();
                    let b_fav = b.favorite_date_time.is_some();
                    match (a_fav, b_fav) {
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        (true, true) => {
                            match (&a.favorite_date_time, &b.favorite_date_time) {
                                (Some(ad), Some(bd)) => bd.cmp(ad),
                                _ => std::cmp::Ordering::Equal,
                            }
                        }
                        _ => {
                            let a_name = Path::new(&a.mod_path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("");
                            let b_name = Path::new(&b.mod_path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("");
                            Self::natural_cmp(a_name.to_lowercase().as_str(), b_name.to_lowercase().as_str())
                        }
                    }
                }
            }
        });

        mods.extend(mod_datas);

        Ok(mods)
    }

    /// 对分组列表进行排序。
    ///
    /// 排序规则（与NRMM一致）：
    /// 1. 收藏的分组始终排在未收藏的分组之前。
    /// 2. 收藏分组内按 `favorite_date_time` 降序（最近收藏的在前）。
    /// 3. 未收藏分组按 `sort_method` 指定的方式排序：
    ///    - `ByIndex`：按 `real_index` 升序。
    ///    - `ByName`：按 `group_name`（小写）字母序。
    fn sort_groups(groups: &mut Vec<ModGroupData>, sort_method: SortGroupMethod) {
        groups.sort_by(|a, b| {
            let a_fav = a.favorite_date_time.is_some();
            let b_fav = b.favorite_date_time.is_some();
            match (a_fav, b_fav) {
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
                _ => {}
            }

            if a_fav && b_fav {
                match (&a.favorite_date_time, &b.favorite_date_time) {
                    (Some(ad), Some(bd)) => return bd.cmp(ad),
                    _ => {}
                }
            }

            match sort_method {
                SortGroupMethod::ByIndex => a.real_index.cmp(&b.real_index),
                SortGroupMethod::ByName => a
                    .group_name
                    .to_lowercase()
                    .cmp(&b.group_name.to_lowercase()),
            }
        });
    }

    /// 自然排序比较函数。
    ///
    /// 将字符串按数字和非数字段分段比较，数字段按数值大小比较，非数字段按字符比较。
    /// 例如："mod1", "mod2", "mod10" 会正确排序为 mod1 < mod2 < mod10。
    fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
        let a_chars: Vec<char> = a.chars().collect();
        let b_chars: Vec<char> = b.chars().collect();
        let mut a_idx = 0;
        let mut b_idx = 0;

        while a_idx < a_chars.len() && b_idx < b_chars.len() {
            let a_is_digit = a_chars[a_idx].is_ascii_digit();
            let b_is_digit = b_chars[b_idx].is_ascii_digit();

            match (a_is_digit, b_is_digit) {
                (true, true) => {
                    let mut a_num = String::new();
                    while a_idx < a_chars.len() && a_chars[a_idx].is_ascii_digit() {
                        a_num.push(a_chars[a_idx]);
                        a_idx += 1;
                    }
                    let mut b_num = String::new();
                    while b_idx < b_chars.len() && b_chars[b_idx].is_ascii_digit() {
                        b_num.push(b_chars[b_idx]);
                        b_idx += 1;
                    }
                    let a_val: u64 = a_num.parse().unwrap_or(0);
                    let b_val: u64 = b_num.parse().unwrap_or(0);
                    if a_val != b_val {
                        return a_val.cmp(&b_val);
                    }
                }
                (false, false) => {
                    if a_chars[a_idx] != b_chars[b_idx] {
                        return a_chars[a_idx].cmp(&b_chars[b_idx]);
                    }
                    a_idx += 1;
                    b_idx += 1;
                }
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
            }
        }

        a_chars.len().cmp(&b_chars.len())
    }

    /// 验证目录名称是否符合平台文件系统命名规范。
    ///
    /// Windows 平台禁止的字符：`<`, `>`, `:`, `"`, `/`, `\`, `|`, `?`, `*`
    /// Linux 平台禁止的字符：`/`, `\0`
    /// 通用规则：名称不能为空，不能以 `.` 开头，不能以空格结尾
    ///
    /// 参数：
    /// - `name`: 待验证的目录名称。
    ///
    /// 返回：成功返回 `Ok(())`，失败返回包含错误信息的 `Err`。
    pub fn validate_directory_name(name: &str) -> Result<()> {
        let trimmed = name.trim();
        
        if trimmed.is_empty() {
            anyhow::bail!("Directory name cannot be empty");
        }
        
        if trimmed.starts_with('.') {
            anyhow::bail!("Directory name cannot start with '.'");
        }
        
        if trimmed.ends_with(' ') {
            anyhow::bail!("Directory name cannot end with space");
        }
        
        #[cfg(windows)]
        {
            let forbidden_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
            if trimmed.chars().any(|c| forbidden_chars.contains(&c)) {
                anyhow::bail!("Directory name contains invalid characters for Windows: < > : \" / \\ | ? *");
            }
        }
        
        #[cfg(not(windows))]
        {
            if trimmed.contains('/') {
                anyhow::bail!("Directory name cannot contain '/'");
            }
        }
        
        Ok(())
    }

    /// 在指定父目录下新建一个分组。
    ///
    /// 根据父目录类型决定创建方式：
    /// - `_MANAGED_` 目录下：创建 `group_<index>` 格式的分组目录
    /// - `#` 目录下：直接使用用户输入的名称作为目录名
    ///
    /// 参数：
    /// - `parent_path`: 父目录路径。
    /// - `group_name`: 新分组的显示名称。
    ///
    /// 返回：新分组的索引（对于 group_xx 分组）或 0（对于 # 目录下的分组）。
    pub fn add_child_group(parent_path: &str, group_name: &str) -> Result<i32> {
        let parent_path = Path::new(parent_path);
        if !parent_path.exists() || !parent_path.is_dir() {
            anyhow::bail!("Parent path does not exist: {:?}", parent_path);
        }

        let parent_name = parent_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        
        if parent_name == MANAGED_FOLDER {
            let mods_root = parent_path
                .parent()
                .and_then(|p| p.to_str())
                .ok_or_else(|| anyhow::anyhow!("Failed to get parent directory of managed folder: {:?}", parent_path))?;
            Self::add_group(mods_root, group_name)
        } else {
            Self::validate_directory_name(group_name)?;
            
            let group_path = parent_path.join(group_name);
            if group_path.exists() {
                anyhow::bail!("Group directory already exists: {:?}", group_path);
            }
            
            fs::create_dir_all(&group_path)
                .with_context(|| format!("Failed to create group directory: {:?}", group_path))?;
            
            Ok(0)
        }
    }

    /// 在 `_MANAGED_` 目录下新建一个分组。
    ///
    /// 自动寻找最小的可用索引（从 1 开始递增直到找到不存在的 `group_<index>`），
    /// 创建对应目录，并在 `group_name` 非空时写入 `groupname` 文件。
    ///
    /// 参数：
    /// - `mods_path`: Mods 根目录路径。
    /// - `group_name`: 新分组的显示名称（可为空字符串，表示不写入名称文件）。
    ///
    /// 返回：新分组的索引。
    pub fn add_group(mods_path: &str, group_name: &str) -> Result<i32> {
        let mods_path = Path::new(mods_path);
        if !mods_path.exists() || !mods_path.is_dir() {
            anyhow::bail!("Mods path does not exist: {:?}", mods_path);
        }

        let managed_path = mods_path.join(MANAGED_FOLDER);
        fs::create_dir_all(&managed_path)
            .with_context(|| format!("Failed to create managed folder: {:?}", managed_path))?;

        let mut new_index = 1;
        loop {
            let test_name = format!("group_{}", new_index);
            let test_path = managed_path.join(&test_name);
            if !test_path.exists() {
                break;
            }
            new_index += 1;
        }

        let folder_name = format!("group_{}", new_index);
        let group_path = managed_path.join(&folder_name);
        fs::create_dir_all(&group_path)
            .with_context(|| format!("Failed to create group directory: {:?}", group_path))?;

        if !group_name.is_empty() {
            let name_file = group_path.join(GROUP_NAME_FILE);
            let _ = fs::write(&name_file, group_name);
        }

        Ok(new_index)
    }

    /// 从指定路径添加 Mod（复制文件/目录到目标分组目录）。
    ///
    /// - 支持复制文件或目录
    /// - 目录会被递归复制
    /// - 目标路径存在时会覆盖
    ///
    /// 参数：
    /// - `source_paths`: 源文件/目录路径列表。
    /// - `target_group_path`: 目标分组目录路径。
    ///
    /// 返回：`true` 表示添加成功。
    pub async fn add_mods(
        &self,
        source_paths: Vec<String>,
        target_group_path: &str,
    ) -> Result<bool> {
        let target_group_path = target_group_path.to_string();

        // 在阻塞线程中执行文件复制，避免阻塞异步运行时
        tokio::task::spawn_blocking(move || Self::add_mods_sync(&source_paths, &target_group_path))
            .await
            .with_context(|| "Failed to spawn blocking task for adding mods")?
    }

    /// 同步版本的 add_mods（在阻塞线程中执行）。
    fn add_mods_sync(source_paths: &[String], target_group_path: &str) -> Result<bool> {
        let target = Path::new(target_group_path);

        // 确保目标目录存在
        if !target.exists() {
            anyhow::bail!("Target group path does not exist: {:?}", target);
        }
        if !target.is_dir() {
            anyhow::bail!("Target path is not a directory: {:?}", target);
        }

        for source_path in source_paths {
            let source = Path::new(source_path);

            if !source.exists() {
                warn!("Source path does not exist, skipping: {:?}", source);
                continue;
            }

            // 安全验证
            if let Err(reason) = Self::validate_drop_path(source, target_group_path) {
                warn!("Rejected unsafe path: {}", reason);
                continue;
            }

            let name = source
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let raw_dest = target.join(name);
            let dest = Self::get_safe_target(&raw_dest);

            if source.is_dir() {
                Self::copy_dir_recursive(source, &dest).with_context(|| {
                    format!("Failed to copy directory {:?} to {:?}", source, dest)
                })?;
                info!("Copied directory {:?} to {:?}", source, dest);
            } else {
                // 复制文件
                fs::copy(source, &dest)
                    .with_context(|| format!("Failed to copy file {:?} to {:?}", source, dest))?;
                info!("Copied file {:?} to {:?}", source, dest);
            }
        }

        Ok(true)
    }

    /// 获取安全的目标路径，处理名称冲突。
    ///
    /// 如果目标路径不存在，直接返回原路径。
    /// 如果存在，在名称后追加 _1, _2, _3... 直到找到不存在的路径。
    /// 对目录和文件都有效。
    ///
    /// 参数：
    /// - `target`: 期望的目标路径。
    ///
    /// 返回：不存在的可用路径。
    fn get_safe_target(target: &Path) -> PathBuf {
        if !target.exists() {
            return target.to_path_buf();
        }

        let parent = target.parent().unwrap_or_else(|| Path::new(""));
        let file_stem = target.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let extension = target.extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e))
            .unwrap_or_default();

        let mut i = 1;
        loop {
            let candidate_name = if extension.is_empty() {
                format!("{}_{}", file_stem, i)
            } else {
                format!("{}_{}{}", file_stem, i, extension)
            };
            let candidate = parent.join(&candidate_name);
            if !candidate.exists() {
                return candidate;
            }
            i += 1;
        }
    }

    /// 从 target_group_path 向上查找 Mods 根目录。
    ///
    /// 通过向上遍历父目录，找到名为 "Mods" 且包含 `_MANAGED_` 子目录的目录。
    ///
    /// 参数：
    /// - `target_group_path`: 目标分组路径。
    ///
    /// 返回：Mods 根目录路径，找不到时返回 None。
    fn find_mods_root(target_group_path: &Path) -> Option<PathBuf> {
        let mut current = target_group_path.to_path_buf();
        loop {
            if current.file_name()?.to_str()? == "Mods" {
                let managed = current.join(MANAGED_FOLDER);
                if managed.exists() && managed.is_dir() {
                    return Some(current);
                }
            }
            let parent = current.parent()?;
            if parent == current {
                return None;
            }
            current = parent.to_path_buf();
        }
    }

    /// 检查目录内部（BFS 遍历，深度限制）是否包含 `_MANAGED_` 子目录。
    ///
    /// 使用 DirWalker BFS 遍历，内部通过 VisitedPathPool 防止符号链接循环，
    /// 深度限制为 DEFAULT_MAX_TRAVERSAL_DEPTH。跟随符号链接（follow_symlinks=true），
    /// 但因 visited 池使用 canonical 路径去重，不会产生无限循环。
    ///
    /// 参数：
    /// - `dir`: 待检查的目录路径。
    ///
    /// 返回：包含 `_MANAGED_` 返回 true，否则返回 false。
    fn dir_contains_managed(dir: &Path) -> bool {
        if !dir.exists() || !dir.is_dir() {
            return false;
        }

        let dir_name = dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if dir_name.eq_ignore_ascii_case(MANAGED_FOLDER) {
            return true;
        }

        let mut found = false;
        DirWalker::new()
            .follow_symlinks(true)
            .include_files(false)
            .skip_hidden(false)
            .walk(dir, None, |entry| {
                if entry.depth == 0 {
                    return true;
                }
                let name = entry.path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                if name.eq_ignore_ascii_case(MANAGED_FOLDER) {
                    found = true;
                    return false;
                }
                true
            });

        found
    }

    /// 验证拖入路径的安全性。
    ///
    /// 检查以下条件（任一不满足则拒绝）：
    /// a) 路径不是 `_MANAGED_` 目录的子目录
    /// b) 路径不是当前可执行文件所在目录的子目录
    /// c) 拖入的目录不包含 `_MANAGED_` 子目录（BFS，深度限制 DEFAULT_MAX_TRAVERSAL_DEPTH）
    /// d) 路径不是 Mods 根目录的父目录或其祖先
    ///
    /// 参数：
    /// - `source_path`: 拖入的源路径。
    /// - `target_group_path`: 目标分组路径（用于定位 Mods 根目录）。
    ///
    /// 返回：`Ok(())` 表示安全，`Err(String)` 包含拒绝原因。
    fn validate_drop_path(source_path: &Path, target_group_path: &str) -> Result<(), String> {
        let target_group = Path::new(target_group_path);

        // a) 检查路径是否位于 _MANAGED_ 目录下
        let mut current = source_path.to_path_buf();
        loop {
            if let Some(name) = current.file_name().and_then(|n| n.to_str()) {
                if name.eq_ignore_ascii_case(MANAGED_FOLDER) {
                    return Err(format!(
                        "Path is under _MANAGED_ directory, which is not allowed: {:?}",
                        source_path
                    ));
                }
            }
            match current.parent() {
                Some(parent) if parent != current => current = parent.to_path_buf(),
                _ => break,
            }
        }

        // b) 检查路径是否位于当前可执行文件所在目录下
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                if let Ok(exe_dir_canon) = exe_dir.canonicalize() {
                    if let Ok(source_canon) = source_path.canonicalize() {
                        if source_canon.starts_with(&exe_dir_canon) {
                            return Err(format!(
                                "Path is under the tool's directory, which is not allowed: {:?}",
                                source_path
                            ));
                        }
                    }
                }
                // 额外检查：拖入的路径是否是工具目录的父目录
                if let Ok(exe_dir_canon) = exe_dir.canonicalize() {
                    if let Ok(source_canon) = source_path.canonicalize() {
                        if exe_dir_canon.starts_with(&source_canon) {
                            return Err(format!(
                                "Path is an ancestor of the tool's directory, which is not allowed: {:?}",
                                source_path
                            ));
                        }
                    }
                }
            }
        }

        // c) 检查目录内部是否包含 _MANAGED_ 子目录
        if source_path.is_dir() && Self::dir_contains_managed(source_path) {
            return Err(format!(
                "Directory contains _MANAGED_ folder, which is not allowed: {:?}",
                source_path
            ));
        }

        // d) 检查路径是否是 Mods 根目录的父目录或其祖先
        if let Some(mods_root) = Self::find_mods_root(target_group) {
            if let Ok(mods_root_canon) = mods_root.canonicalize() {
                if let Ok(source_canon) = source_path.canonicalize() {
                    if mods_root_canon.starts_with(&source_canon) && source_canon != mods_root_canon {
                        return Err(format!(
                            "Path is an ancestor of Mods directory, which is not allowed: {:?}",
                            source_path
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// 移除分组（移动到 `_MANAGED_` 下并重命名加 `_removed_<timestamp>` 后缀）。
    ///
    /// 此操作不会真正删除分组目录，而是将其移动到 `_MANAGED_` 目录下并附加时间戳后缀，
    /// 便于用户误操作后恢复。`_MANAGED_` 目录会自动创建以确保存在。
    ///
    /// 参数：
    /// - `group_path`: 待移除的分组目录路径。
    pub fn remove_group(group_path: &str) -> Result<()> {
        let path = Path::new(group_path);
        if !path.exists() || !path.is_dir() {
            anyhow::bail!("Group path does not exist: {:?}", path);
        }

        let group_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("group");

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let dest_name = format!("{}_removed_{}", group_name, timestamp);

        let mods_path = match path.parent().and_then(|p| p.parent()) {
            Some(p) => p,
            None => anyhow::bail!("Invalid group path: cannot find mods root directory"),
        };

        let removed_path = mods_path.join(DISABLED_MANAGED_REMOVED);
        fs::create_dir_all(&removed_path)
            .with_context(|| format!("Failed to create removed folder: {:?}", removed_path))?;

        let dest_path = removed_path.join(&dest_name);

        fs::rename(path, &dest_path).with_context(|| {
            format!(
                "Failed to move group to DISABLED_MANAGED_REMOVED: {:?} -> {:?}",
                path, dest_path
            )
        })?;

        info!("Group removed to DISABLED_MANAGED_REMOVED: {:?}", dest_path);
        Ok(())
    }

    /// 移除单个模组：先还原（启用）再移动到 DISABLED_MANAGED_REMOVED 目录。
    ///
    /// 流程：
    /// 1. 如果模组处于禁用状态（目录名以 DISABLED 开头），先还原为启用状态
    /// 2. 将模组目录移动到 `mods/DISABLED_MANAGED_REMOVED/` 下，附加时间戳后缀
    ///
    /// 参数：
    /// - `mod_path`: 模组目录路径
    pub fn remove_mod(mod_path: &str) -> Result<()> {
        let path = Path::new(mod_path);
        if !path.exists() || !path.is_dir() {
            anyhow::bail!("Mod path does not exist: {:?}", path);
        }

        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("mod")
            .to_string();

        let mods_root = path
            .parent()  // group dir
            .and_then(|p| p.parent())  // mods dir
            .ok_or_else(|| anyhow::anyhow!("Invalid mod path: cannot find mods root"))?;

        // 步骤 1：还原（如果处于禁用状态则启用）
        let actual_path = if Self::is_disabled_name(&dir_name) {
            let restored_name = dir_name[DISABLED_PREFIX.len()..].trim_start_matches('_').to_string();
            let parent = path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("Invalid mod path: cannot get parent directory: {:?}", path))?;
            let restored_path = parent.join(&restored_name);
            if restored_path.exists() {
                anyhow::bail!("Cannot restore mod: destination path already exists: {:?}", restored_path);
            }
            fs::rename(path, &restored_path)
                .with_context(|| format!("Failed to restore mod: {:?} -> {:?}", path, restored_path))?;
            info!("Mod restored before removal: {:?} -> {:?}", path, restored_path);
            restored_path
        } else {
            path.to_path_buf()
        };

        // 步骤 2：移动到 DISABLED_MANAGED_REMOVED
        let actual_name = actual_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("mod");

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let dest_name = format!("{}_removed_{}", actual_name, timestamp);
        let removed_dir = mods_root.join(DISABLED_MANAGED_REMOVED);
        fs::create_dir_all(&removed_dir)
            .with_context(|| format!("Failed to create DISABLED_MANAGED_REMOVED: {:?}", removed_dir))?;

        let dest_path = removed_dir.join(&dest_name);
        fs::rename(&actual_path, &dest_path)
            .with_context(|| format!("Failed to move mod to DISABLED_MANAGED_REMOVED: {:?} -> {:?}", actual_path, dest_path))?;

        info!("Mod removed to DISABLED_MANAGED_REMOVED: {:?}", dest_path);
        Ok(())
    }

    /// 切换 # 目录分组的启用/禁用状态。
    ///
    /// 通过在目录名前添加或移除 `DISABLED` 前缀实现状态切换：
    /// - 禁用 → 启用：移除 `DISABLED` 前缀（如 `DISABLED#角色` → `#角色`）
    /// - 启用 → 禁用：添加 `DISABLED` 前缀（如 `#角色` → `DISABLED#角色`）
    ///
    /// 该功能与 `group_xx` 逻辑完全独立，不涉及 INI 文件修改，不使用 selectindex 机制。
    ///
    /// 参数：
    /// - `group_path`: 目标分组目录的绝对路径。
    ///
    /// 返回：操作后的禁用状态（true = 已禁用，false = 已启用）。
    pub fn toggle_tree_node_group_disabled(group_path: &str) -> Result<bool> {
        let path = Path::new(group_path);
        if !path.exists() || !path.is_dir() {
            anyhow::bail!("Group path does not exist: {:?}", path);
        }

        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // 仅对 # 目录分组生效：目录名以 # 开头或以 DISABLED# 开头
        let is_hash_dir = dir_name.starts_with('#')
            || (Self::is_disabled_name(&dir_name)
                && dir_name[DISABLED_PREFIX.len()..].trim_start_matches('_').starts_with('#'));

        if !is_hash_dir {
            anyhow::bail!("This operation is only available for # directory groups, not for group_xx groups");
        }

        let parent = match path.parent() {
            Some(p) => p,
            None => anyhow::bail!("Invalid group path: no parent directory"),
        };

        let is_disabled = Self::is_disabled_name(&dir_name);
        let new_name = if is_disabled {
            dir_name[DISABLED_PREFIX.len()..].trim_start_matches('_').to_string()
        } else {
            format!("{}{}", DISABLED_PREFIX, dir_name)
        };

        let new_path = parent.join(&new_name);
        if new_path.exists() {
            anyhow::bail!("Destination path already exists: {:?}", new_path);
        }

        fs::rename(path, &new_path).with_context(|| {
            format!("Failed to rename group: {:?} -> {:?}", path, new_path)
        })?;

        info!("Tree node group toggled: {:?} -> {:?}", path, new_path);
        Ok(!is_disabled)
    }

    /// 重命名分组目录（直接修改目录名）。
    ///
    /// 注意：此操作仅修改目录名，不会更新 `groupname` 文件。
    /// 若目标名称已存在则会失败。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    /// - `new_name`: 新的目录名称。
    pub fn rename_group(group_path: &str, new_name: &str) -> Result<()> {
        let path = Path::new(group_path);
        if !path.exists() || !path.is_dir() {
            anyhow::bail!("Group path does not exist: {:?}", path);
        }

        let parent = match path.parent() {
            Some(p) => p,
            None => anyhow::bail!("Invalid group path: no parent directory"),
        };

        let new_path = parent.join(new_name);
        if new_path.exists() {
            anyhow::bail!("Destination path already exists: {:?}", new_path);
        }

        fs::rename(path, &new_path)
            .with_context(|| format!("Failed to rename group: {:?} -> {:?}", path, new_path))?;

        info!("Group renamed: {:?} -> {:?}", path, new_path);
        Ok(())
    }

    /// 重命名模组目录。
    ///
    /// 参数：
    /// - `mod_path`: 模组目录路径。
    /// - `new_name`: 新的目录名称（不含 DISABLED 前缀）。
    ///
    /// 返回：操作结果。
    pub fn rename_mod(mod_path: &str, new_name: &str) -> Result<()> {
        let path = Path::new(mod_path);
        if !path.exists() || !path.is_dir() {
            anyhow::bail!("Mod path does not exist: {:?}", path);
        }

        let dir_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => anyhow::bail!("Invalid mod path: no file name"),
        };

        let is_disabled = Self::is_disabled_name(dir_name);

        let parent = match path.parent() {
            Some(p) => p,
            None => anyhow::bail!("Invalid mod path: no parent directory"),
        };

        let final_new_name = if is_disabled {
            format!("{}{}", DISABLED_PREFIX, new_name)
        } else {
            new_name.to_string()
        };

        let new_path = parent.join(&final_new_name);
        if new_path.exists() {
            anyhow::bail!("Destination path already exists: {:?}", new_path);
        }

        fs::rename(path, &new_path)
            .with_context(|| format!("Failed to rename mod: {:?} -> {:?}", path, new_path))?;

        info!(
            "Mod renamed: {:?} -> {:?} (disabled={})",
            path, new_path, is_disabled
        );
        Ok(())
    }

    /// 在所有分组中搜索名称包含关键词的模组。
    ///
    /// 参数：
    /// - `groups`: 分组列表。
    /// - `keyword`: 搜索关键词（不区分大小写）。
    ///
    /// 返回：`Vec<(分组索引, 模组索引)>`，表示所有匹配项的位置。
    #[allow(dead_code)]
    pub fn search_mods(groups: &[ModGroupData], keyword: &str) -> Vec<(usize, usize)> {
        let keyword_lower = keyword.to_lowercase();
        let mut results: Vec<(usize, usize)> = Vec::new();

        for (group_idx, group) in groups.iter().enumerate() {
            for (mod_idx, mod_data) in group.mods_in_group.iter().enumerate() {
                if mod_data.mod_name.to_lowercase().contains(&keyword_lower) {
                    results.push((group_idx, mod_idx));
                }
            }
        }

        results
    }

    /// 加载当前目标游戏的 Mods 数据。
    ///
    /// 流程：
    /// 1. 从设置中获取目标游戏对应的 Mods 路径。
    /// 2. 校验路径有效性，无效时返回空列表并记录警告。
    /// 3. 使用 `spawn_blocking` 在阻塞线程中执行 `scan_groups`，避免阻塞异步运行时。
    ///
    /// 参数：
    /// - `settings`: 全局设置。
    ///
    /// 返回：分组数据列表。
    pub async fn load_mods(&self, settings: &Settings) -> Result<Vec<ModGroupData>> {
        log::debug!("读取游戏模组：{:?}", &settings.target_game);
        let target_game: TargetGame = settings.target_game;
        let mods_path: String = Self::get_mods_path_for_game(settings, target_game);

        if mods_path.is_empty() {
            warn!("No mods path configured for game: {:?}", target_game);
            return Ok(vec![]);
        }

        let status: ModsPathStatus = Self::validate_mods_path(&mods_path);
        if status != ModsPathStatus::Valid {
            warn!(
                "Mods path is not valid: {:?}, status: {:?}",
                mods_path, status
            );
            return Ok(vec![]);
        }

        let managed_path: PathBuf = Path::new(&mods_path).join(MANAGED_FOLDER);
        let managed_path_str: String = managed_path.to_string_lossy().to_string();

        let sort_method: SortGroupMethod = SortGroupMethod::from_i32(settings.sort_group_method);

        // 在阻塞线程中执行文件系统扫描，避免阻塞 Tokio 运行时
        let groups: Vec<ModGroupData> =
            tokio::task::spawn_blocking(move || Self::scan_groups(&managed_path_str, sort_method))
                .await
                .with_context(|| "Failed to spawn blocking task for scanning groups")??;

        info!("Loaded {} groups", groups.len());
        Ok(groups)
    }

    /// 独立执行 Hash 冲突检测（不依赖 `update_mod_data`）。
    ///
    /// 流程：
    /// 1. 根据目标游戏从设置中获取 Mods 路径。
    /// 2. 校验路径有效性，无效时返回空报告。
    /// 3. 在阻塞线程中扫描分组并计算 hash 冲突。
    ///
    /// 该方法专为独立的 `check_hash_conflicts` Tauri 命令设计，
    /// 通过 `TaskQueue` 互斥执行，避免与 `update_mod_data` 阻塞。
    ///
    /// 参数：
    /// - `settings`: 全局设置。
    ///
    /// 返回：`HashConflictReport`（包含 `enabled_mod_hashes` 与 `conflicts`）。
    pub async fn check_hash_conflicts_async(
        &self,
        settings: &Settings,
    ) -> Result<HashConflictReport> {
        let target_game: TargetGame = settings.target_game;
        let mods_path: String = Self::get_mods_path_for_game(settings, target_game);

        if mods_path.is_empty() {
            warn!("No mods path configured for game: {:?}", target_game);
            return Ok(HashConflictReport::default());
        }

        let status: ModsPathStatus = Self::validate_mods_path(&mods_path);
        if status != ModsPathStatus::Valid {
            warn!(
                "Mods path is not valid: {:?}, status: {:?}",
                mods_path, status
            );
            return Ok(HashConflictReport::default());
        }

        let managed_path: PathBuf = Path::new(&mods_path).join(MANAGED_FOLDER);
        let managed_path_str: String = managed_path.to_string_lossy().to_string();
        let sort_method: SortGroupMethod = SortGroupMethod::from_i32(settings.sort_group_method);

        // 在阻塞线程中执行扫描 + hash 检测，避免阻塞 Tokio 运行时
        let report = tokio::task::spawn_blocking(move || -> Result<HashConflictReport> {
            let groups = Self::scan_groups(&managed_path_str, sort_method)?;
            Self::check_enabled_mod_hash_conflicts(&managed_path_str, &groups)
        })
        .await
        .with_context(|| "Failed to spawn blocking task for hash conflict check")??;

        info!(
            "Independent hash conflict check: {} conflicts found",
            report.conflicts.len()
        );
        Ok(report)
    }

    /// 根据指定的 Mods 路径刷新模组数据（不依赖全局设置）。
    ///
    /// 与 `load_mods` 的区别：直接接受 `mods_path` 参数，且固定使用 `ByIndex` 排序。
    ///
    /// 参数：
    /// - `mods_path`: Mods 根目录路径。
    pub async fn refresh_mod_data(&self, mods_path: &str) -> Result<Vec<ModGroupData>> {
        let mods_path = mods_path.to_string();
        let groups = tokio::task::spawn_blocking(move || {
            Self::scan_groups(&mods_path, SortGroupMethod::ByIndex)
        })
        .await
        .with_context(|| "Failed to spawn blocking task for refreshing mod data")??;

        Ok(groups)
    }

    /// 执行模组数据更新（核心管理流程）。
    ///
    /// 该函数是 NRMM 的核心流程，会：
    /// 1. 在阻塞线程中调用 `update_mod_data_sync` 完成实际的 INI 注入与管理。
    /// 2. 收集执行日志、错误报告、hash 冲突报告。
    /// 3. 计算总耗时并组装为 `UpdateModDataResult` 返回。
    ///
    /// 参数：
    /// - `mods_path`: Mods 根目录路径。
    /// - `known_libraries`: 已知的模组库命名空间映射（key 为命名空间，value 为描述）。
    ///
    /// 返回：`UpdateModDataResult`，包含成功状态、日志、耗时及各项报告。
    pub async fn update_mod_data(
        &self,
        mods_path: &str,
        known_libraries: &HashMap<String, String>,
    ) -> Result<UpdateModDataResult, String> {
        let start_time = std::time::Instant::now();
        let mut logs: Vec<LogEntry> = Vec::new();
        let mut success = true;

        let mods_path = mods_path.to_string();
        let known_libraries = known_libraries.clone();

        let result = tokio::task::spawn_blocking(move || {
            Self::update_mod_data_sync(&mods_path, &known_libraries)
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?;

        let (result_logs, error_report, result_success, hash_conflict_report, per_mod_errors, group_summaries, total_mods_processed, total_errors) = match result {
            Ok((logs, report, hash_report, per_mod_errs, group_sums)) => {
                let total_mods: u32 = group_sums.iter().map(|s| s.total_mods).sum();
                let total_errs: u32 = group_sums.iter().map(|s| s.error_count).sum();
                (logs, Some(report), true, Some(hash_report), per_mod_errs, group_sums, total_mods, total_errs)
            }
            Err(e) => {
                success = false;
                let mut error_logs =
                    vec![LogEntry::error(format!("Update Mod Data failed: {}", e))];
                error_logs.push(LogEntry::error(
                    "Please check the logs above for more details".to_string(),
                ));
                (error_logs, None, false, None, Vec::new(), Vec::new(), 0, 0)
            }
        };

        logs.extend(result_logs);

        let duration_ms = start_time.elapsed().as_millis() as u64;

        if success {
            logs.push(LogEntry::success(format!(
                "Mods successfully managed in {}ms",
                duration_ms
            )));
        }

        Ok(UpdateModDataResult {
            success: result_success,
            logs,
            duration_ms,
            error_report,
            hash_conflict_report,
            per_mod_errors,
            group_summaries,
            total_mods_processed,
            total_errors,
        })
    }

    /// `update_mod_data` 的同步实现（在阻塞线程中执行）。
    ///
    /// 流程：
    /// 1. 校验 Mods 路径并确保 `_MANAGED_` 目录存在。
    /// 2. 获取所有 `group_<index>` 分组目录。
    /// 3. 调用 `error_detection::check_all_errors` 检测所有 INI 语法错误。
    /// 4. 并行处理每个分组：删除旧 INI → 创建新分组 INI → 管理每个模组（注入条件）。
    /// 5. 收集所有分组数据用于 hash 冲突检测。
    /// 6. 调用 `check_and_report_hash_conflicts` 生成冲突报告。
    ///
    /// 参数：
    /// - `mods_path`: Mods 根目录路径。
    /// - `known_libraries`: 已知模组库命名空间映射。
    ///
    /// 返回：`(日志列表, 错误报告, hash 冲突报告)`。
    fn update_mod_data_sync(
        mods_path: &str,
        known_libraries: &HashMap<String, String>,
    ) -> Result<(
        Vec<LogEntry>,
        ErroredLinesReport,
        HashConflictReport,
        Vec<ModManageError>,
        Vec<ModProcessSummary>,
    )> {
        let mut logs: Vec<LogEntry> = Vec::new();

        logs.push(LogEntry::info("Starting Update Mod Data..."));
        debug!("[update_mod_data] mods_path: {:?}", mods_path);

        let mods_path = Path::new(mods_path);
        if !mods_path.exists() || !mods_path.is_dir() {
            anyhow::bail!("Mods path does not exist: {:?}", mods_path);
        }

        let managed_path = mods_path.join(MANAGED_FOLDER);
        let managed_path_str = managed_path.to_string_lossy().to_string();
        debug!("[update_mod_data] managed_path: {:?}", managed_path_str);

        // 确保 _MANAGED_ 目录存在
        if !managed_path.exists() {
            fs::create_dir_all(&managed_path).with_context(|| {
                format!("Failed to create _MANAGED_ folder: {:?}", managed_path)
            })?;
            logs.push(LogEntry::info("Created _MANAGED_ folder"));
        }

        let group_folders = Self::get_group_folders(&managed_path_str)?;
        logs.push(LogEntry::info(format!(
            "Found {} groups",
            group_folders.len()
        )));
        debug!(
            "[update_mod_data] group folders: {:?}",
            group_folders
                .iter()
                .map(|(p, i)| format!("group_{}: {}", i, p))
                .collect::<Vec<_>>()
        );

        let known_lib_namespaces: Vec<String> = known_libraries.keys().cloned().collect();

        // 检测所有 INI 语法错误
        let error_report = crate::ini_handler::error_detection::check_all_errors(
            &managed_path_str,
            &known_lib_namespaces,
        )?;

        logs.push(LogEntry::info("Error detection completed"));
        debug!(
            "[update_mod_data] error detection: {} duplicate libs, {} crash lines, {} other errors, {} missing endif",
            error_report.duplicate_libs.len(),
            error_report.crash_lines.len(),
            error_report.other_errors.len(),
            error_report.missing_endif_errors.len(),
        );

        if !error_report.duplicate_libs.is_empty() {
            logs.push(LogEntry::warn(format!(
                "Found {} duplicate libraries",
                error_report.duplicate_libs.len()
            )));
        }

        if !error_report.crash_lines.is_empty() {
            logs.push(LogEntry::warn(format!(
                "Found {} files with crash lines",
                error_report.crash_lines.len()
            )));
        }

        // 并行处理每个 group_<index> 分组（# 目录和普通目录不在此处理）
        debug!(
            "[update_mod_data] starting parallel processing of {} groups",
            group_folders.len()
        );

        let per_mod_errors: Arc<Mutex<Vec<ModManageError>>> =
            Arc::new(Mutex::new(Vec::new()));
        let group_summaries: Arc<Mutex<Vec<ModProcessSummary>>> =
            Arc::new(Mutex::new(Vec::new()));
        let logs: Arc<Mutex<Vec<LogEntry>>> = Arc::new(Mutex::new(logs));

        group_folders
            .par_iter()
            .for_each(|(group_path, group_index)| {
                let group_name = format!("group_{}", group_index);
                debug!(
                    "[update_mod_data] processing group: {} (path: {})",
                    group_name, group_path
                );

                // 删除分组目录下旧的 INI 文件
                if let Err(e) = Self::delete_group_ini_files(group_path) {
                    error!("update_mod_data: failed to delete group INI files for {}: {}", group_path, e);
                }

                // 创建新的分组 INI 文件（包含 active_slot 变量与切换快捷键）
                if let Err(e) = Self::create_group_ini(group_path, &group_name, *group_index) {
                    error!("update_mod_data: failed to create group INI for {}: {}", group_path, e);
                }

                // 管理分组内的每个模组
                // 状态过滤系统：仅处理启用状态的模组，禁用模组（is_disabled=true）会被自动忽略
                // 状态保护机制：本流程不会改变任何模组的启用/禁用状态，状态由目录名的 DISABLED 前缀决定
                let mut group_total = 0u32;

                match Self::get_mods_on_group(group_path) {
                    Ok(mods) => {
                        let enabled_count = mods.iter().filter(|m| !m.is_disabled && m.mod_path != "None").count();
                        debug!(
                            "[update_mod_data] group {}: {} mods total, {} enabled",
                            group_name,
                            mods.len(),
                            enabled_count
                        );

                        group_total = mods.iter().filter(|m| m.mod_path != "None" && !m.is_disabled).count() as u32;

                        let mut namespace_fixes = Vec::new();
                        match Self::fix_namespace_conflicts_for_group(group_path, &mods, known_libraries) {
                            Ok(fixes) => {
                                if !fixes.is_empty() {
                                    info!(
                                        "[update_mod_data] group {}: fixed {} namespace conflicts",
                                        group_name,
                                        fixes.len()
                                    );
                                    if let Ok(mut logs_guard) = logs.lock() {
                                        for fix in &fixes {
                                            logs_guard.push(LogEntry::info(format!(
                                                "Namespace conflict fixed: '{}' -> '{}' in mod '{}'",
                                                fix.original_namespace, fix.new_namespace, fix.mod_name
                                            )));
                                        }
                                    }
                                }
                                namespace_fixes = fixes;
                            }
                            Err(e) => {
                                warn!(
                                    "[update_mod_data] group {}: failed to fix namespace conflicts: {}",
                                    group_name, e
                                );
                            }
                        }

                        mods.par_iter().for_each(|mod_data| {
                            // 状态过滤：跳过 None 槽位和禁用模组
                            if mod_data.mod_path == "None" || mod_data.is_disabled {
                                return;
                            }

                            debug!(
                                "[update_mod_data]   managing mod: {} (real_index: {}) in group {}",
                                mod_data.mod_path, mod_data.real_index, group_name
                            );

                            if let Err(e) = Self::manage_mod(
                                &mod_data.mod_path,
                                &group_name,
                                mod_data.real_index,
                                *group_index,
                            ) {
                                let err_msg = e.to_string();
                                // 根据错误消息确定阶段
                                let stage = if err_msg.contains("路径无特殊字符") {
                                    "validate"
                                } else if err_msg.contains("无法创建模组备份") {
                                    "ini_backup"
                                } else {
                                    "ini_modify"
                                };

                                let mod_err = ModManageError {
                                    mod_path: mod_data.mod_path.clone(),
                                    mod_name: mod_data.mod_name.clone(),
                                    stage: stage.to_string(),
                                    message: err_msg,
                                    ini_file: None,
                                };

                                if let Ok(mut errors) = per_mod_errors.lock() {
                                    errors.push(mod_err);
                                }
                            }
                        });

                        let group_error_count = per_mod_errors.lock().map(|errors| {
                            errors.iter()
                                .filter(|e| e.mod_path.starts_with(group_path))
                                .count() as u32
                        }).unwrap_or(0);
                        let group_success_count = group_total.saturating_sub(group_error_count);

                        let summary = ModProcessSummary {
                            group_name,
                            total_mods: group_total,
                            success_count: group_success_count,
                            error_count: group_error_count,
                            namespace_fixes,
                        };

                        if let Ok(mut summaries) = group_summaries.lock() {
                            summaries.push(summary);
                        }
                    }
                    Err(e) => {
                        error!("update_mod_data: failed to get mods for group {}: {}", group_path, e);

                        let summary = ModProcessSummary {
                            group_name,
                            total_mods: 0,
                            success_count: 0,
                            error_count: 0,
                            namespace_fixes: Vec::new(),
                        };

                        if let Ok(mut summaries) = group_summaries.lock() {
                            summaries.push(summary);
                        }
                    }
                }
            });

        // 在所有分组处理完成后，检测启用的 mod 的 hash 冲突
        debug!(
            "[update_mod_data] parallel processing done, collecting data for hash conflict check"
        );
        let mut groups_for_hash_check: Vec<ModGroupData> = Vec::new();
        for (group_path, _group_index) in &group_folders {
            if let Ok(mods) = Self::get_mods_on_group(group_path) {
                let group_name = Self::get_group_name(group_path).unwrap_or_else(|_| {
                    Path::new(group_path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Unknown")
                        .to_string()
                });

                let group_data = ModGroupData {
                    group_path: group_path.clone(),
                    icon_path: None,
                    group_name,
                    favorite_date_time: None,
                    mods_in_group: mods,
                    real_index: *_group_index,
                    previous_selected_mod_on_group: 0,
                    children: Vec::new(),
                    is_tree_node: false,
                    is_virtual: false,
                    is_disabled: false,
                };
                groups_for_hash_check.push(group_data);
            }
        }
        debug!(
            "[update_mod_data] hash conflict check: {} groups collected",
            groups_for_hash_check.len()
        );

        let (_hash_conflict_count, hash_logs, hash_conflict_report) =
            Self::check_and_report_hash_conflicts(&managed_path_str, &groups_for_hash_check);

        let mut logs = logs
            .lock()
            .map_err(|e| anyhow::anyhow!("Logs mutex poisoned: {}", e))?
            .clone();
        logs.extend(hash_logs);

        logs.push(LogEntry::info("All groups processed"));

        let per_mod_errors = per_mod_errors
            .lock()
            .map_err(|e| anyhow::anyhow!("Per-mod errors mutex poisoned: {}", e))?
            .clone();
        let group_summaries_list = group_summaries
            .lock()
            .map_err(|e| anyhow::anyhow!("Group summaries mutex poisoned: {}", e))?
            .clone();

        Ok((
            logs,
            error_report,
            hash_conflict_report,
            per_mod_errors,
            group_summaries_list,
        ))
    }

    /// 删除分组目录下的所有 `.ini` 文件。
    ///
    /// 在每次 `update_mod_data` 时会先清除旧 INI，再重新生成分组 INI，
    /// 避免遗留的过期配置文件干扰 3DMigoto。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    fn delete_group_ini_files(group_path: &str) -> Result<()> {
        let group_path = Path::new(group_path);
        if !group_path.exists() || !group_path.is_dir() {
            debug!(
                "[delete_group_ini_files] group path does not exist, skipping: {:?}",
                group_path
            );
            return Ok(());
        }

        let entries = fs::read_dir(group_path)
            .with_context(|| format!("Failed to read group directory: {:?}", group_path))?;

        let mut deleted_count = 0;
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if ext.eq_ignore_ascii_case("ini") {
                            // 将分组 INI 文件移至回收站；失败时静默忽略，不影响整体流程，但记录 debug 便于排查
                            if let Err(e) = Self::move_to_trash(&path) {
                                debug!("Failed to move group INI file to trash {:?}: {}", path, e);
                            } else {
                                deleted_count += 1;
                            }
                        }
                    }
                }
            }
        }

        debug!(
            "[delete_group_ini_files] deleted {} .ini files from {:?}",
            deleted_count, group_path
        );
        Ok(())
    }

    /// 在分组目录下创建分组 INI 文件（`<group_name>.ini`）。
    ///
    /// 该 INI 文件包含：
    /// - `[Constants]` 段：定义 `active_slot` 持久化变量（初始为 0）。
    /// - `[Key.NRMM_Group_<index>_Next]` 段：切换到下一个槽位的快捷键绑定。
    /// - `[Key.NRMM_Group_<index>_Prev]` 段：切换到上一个槽位的快捷键绑定。
    ///
    /// 用户可在 3DMigoto 的快捷键配置中为这些 Key 段绑定实际按键。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    /// - `group_name`: 分组名称（用作 INI 文件名）。
    /// - `group_index`: 分组索引（用于命名空间隔离）。
    fn create_group_ini(group_path: &str, group_name: &str, group_index: i32) -> Result<()> {
        let group_path = Path::new(group_path);
        let ini_path = group_path.join(format!("{}.ini", group_name));

        let content = format!(
            "; No Reload Mod Manager - Group {}\n\
             ; This file is auto-generated. Do not edit manually.\n\
             \n\
             [Constants]\n\
             global persist $\\modmanageragl\\group_{}\\active_slot = 0\n\
             \n\
             [Key.NRMM_Group_{}_Next]\n\
             key = \n\
             back = 1\n\
             $\\modmanageragl\\group_{}\\active_slot = ($\\modmanageragl\\group_{}\\active_slot + 1) % 500\n\
             \n\
             [Key.NRMM_Group_{}_Prev]\n\
             key = \n\
             back = 1\n\
             $\\modmanageragl\\group_{}\\active_slot = ($\\modmanageragl\\group_{}\\active_slot - 1 + 500) % 500\n",
            group_index, group_index, group_index, group_index, group_index, group_index, group_index, group_index
        );

        fs::write(&ini_path, content)
            .with_context(|| format!("Failed to create group INI: {:?}", ini_path))?;

        debug!(
            "[create_group_ini] created group INI: {:?} for group_{}",
            ini_path, group_index
        );
        Ok(())
    }

    /// 验证模组路径是否在允许的范围内。
    ///
    /// 安全检查规则：
    /// 1. 模组路径必须位于 Mods/_MANAGED_/ 目录下；
    /// 2. 路径中不得包含任何以 '#' 开头的目录（防止越权访问系统文件）；
    /// 3. 模组必须直接属于某个 group_xx 子目录。
    ///
    /// 参数：
    /// - `mods_root`: Mods 根目录路径。
    /// - `mod_path`: 待验证的模组目录路径。
    ///
    /// 返回：`true` 表示路径合法，`false` 表示路径超出允许范围。
    fn is_valid_mod_path(mods_root: &Path, mod_path: &Path) -> bool {
        // 检查是否在 _MANAGED_ 目录下
        let managed_path = mods_root.join(MANAGED_FOLDER);
        if !mod_path.starts_with(&managed_path) {
            return false;
        }

        // 检查路径中是否包含 # 目录（安全限制：禁止访问系统隐藏目录）
        for component in mod_path.components() {
            if let Some(name) = component.as_os_str().to_str() {
                if name.starts_with('#') {
                    return false;
                }
            }
        }

        // 检查是否直接属于 group_xx 目录
        let relative = match mod_path.strip_prefix(&managed_path) {
            Ok(r) => r,
            Err(_) => return false,
        };

        let components: Vec<&str> = relative
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();

        !components.is_empty() && components[0].starts_with("group_")
    }

    /// 为分组内所有启用的模组检测并修复命名空间冲突。
    ///
    /// 流程：
    /// 1. 初始化已占用命名空间集合（包含已知模组库的命名空间）
    /// 2. 按模组顺序遍历每个启用的模组
    /// 3. 对模组内每个 INI 文件，先收集所有 namespace 信息
    /// 4. 若模组存在 namespace，检查是否冲突，若冲突则生成唯一的新名称（追加 _1, _2...）
    /// 5. 使用确定的新命名空间替换模组内所有相关 INI 文件
    /// 6. 记录修复日志
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径
    /// - `mods`: 分组内的模组列表
    /// - `known_libraries`: 已知模组库命名空间映射（这些命名空间不重命名）
    ///
    /// 返回：该分组内的命名空间修复记录列表
    fn fix_namespace_conflicts_for_group(
        _group_path: &str,
        mods: &[ModData],
        known_libraries: &HashMap<String, String>,
    ) -> Result<Vec<NamespaceFix>> {
        let mut fixes: Vec<NamespaceFix> = Vec::new();
        let mut used_namespaces: HashSet<String> = HashSet::new();

        for (ns_lower, _) in known_libraries {
            used_namespaces.insert(ns_lower.clone());
        }

        for mod_data in mods {
            if mod_data.mod_path == "None" || mod_data.is_disabled {
                continue;
            }

            let mod_path = Path::new(&mod_data.mod_path);
            if !mod_path.exists() || !mod_path.is_dir() {
                continue;
            }

            let ini_files = Self::find_ini_files_bfs(mod_path);

            let mut mod_original_ns: Option<String> = None;
            let mut mod_file_contents: Vec<(PathBuf, String)> = Vec::new();

            for ini_file in &ini_files {
                let backup_path_str = format!(
                    "{}.{}",
                    ini_file.to_string_lossy(),
                    MANAGED_BACKUP_EXT
                );
                let backup_path = Path::new(&backup_path_str);

                if !backup_path.exists() {
                    if let Err(e) = fs::copy(ini_file, backup_path) {
                        warn!(
                            "[fix_namespace_conflicts] failed to create backup for {:?}: {}",
                            ini_file, e
                        );
                        continue;
                    }
                    debug!(
                        "[fix_namespace_conflicts] created backup: {:?} -> {:?}",
                        ini_file, backup_path
                    );
                }

                let content = match fs::read_to_string(ini_file) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(
                            "[fix_namespace_conflicts] failed to read {:?}: {}",
                            ini_file, e
                        );
                        continue;
                    }
                };

                if mod_original_ns.is_none() {
                    if let Some(ns) = extract_namespace_from_ini_content(&content) {
                        mod_original_ns = Some(ns);
                    }
                }

                mod_file_contents.push((ini_file.clone(), content));
            }

            let original_ns = match mod_original_ns {
                Some(ns) => ns,
                None => continue,
            };

            let original_ns_lower = original_ns.to_lowercase();

            let is_known_lib = known_libraries
                .keys()
                .any(|k| k.eq_ignore_ascii_case(&original_ns_lower));
            if is_known_lib {
                used_namespaces.insert(original_ns_lower);
                continue;
            }

            let mut new_ns = original_ns.clone();
            let mut new_ns_lower = new_ns.to_lowercase();
            let mut suffix = 1;

            while used_namespaces.contains(&new_ns_lower) {
                new_ns = format!("{}_{}", original_ns, suffix);
                new_ns_lower = new_ns.to_lowercase();
                suffix += 1;
            }

            if new_ns != original_ns {
                let mut mod_was_modified = false;

                for (ini_file, content) in &mod_file_contents {
                    let (new_content, was_modified) =
                        replace_namespace_in_content(content, &original_ns, &new_ns);

                    if was_modified {
                        if let Err(e) = fs::write(ini_file, &new_content) {
                            warn!(
                                "[fix_namespace_conflicts] failed to write {:?}: {}",
                                ini_file, e
                            );
                            continue;
                        }
                        mod_was_modified = true;
                        debug!(
                            "[fix_namespace_conflicts] updated file {:?} for namespace rename",
                            ini_file.file_name().unwrap_or_default()
                        );
                    }
                }

                if mod_was_modified {
                    info!(
                        "[fix_namespace_conflicts] renamed namespace '{}' -> '{}' in mod '{}'",
                        original_ns, new_ns, mod_data.mod_name
                    );

                    fixes.push(NamespaceFix {
                        mod_name: mod_data.mod_name.clone(),
                        original_namespace: original_ns.clone(),
                        new_namespace: new_ns.clone(),
                    });
                }
            }

            used_namespaces.insert(new_ns_lower);
        }

        Ok(fixes)
    }

    /// 管理单个模组：备份并修改其 INI 文件以支持槽位切换。
    ///
    /// **状态保护机制**：本函数仅修改 INI 文件内容，不会改变模组的启用/禁用状态。
    /// 模组状态由目录名的 `DISABLED` 前缀决定，任何可能导致状态变更的代码均禁止在此函数中出现。
    ///
    /// 流程：
    /// 1. 验证路径安全性（确保仅处理 _MANAGED_/group_xx 下的模组）。
    /// 2. 递归查找模组目录下的所有 `.ini` 文件。
    /// 3. 对每个 INI 文件：若不存在 `.baknrmm` 备份则创建备份。
    /// 4. 调用 `modify_ini_file` 注入 `managed_slot_id` 变量与条件块。
    ///
    /// 参数：
    /// - `mod_path`: 模组目录路径。
    /// - `group_folder_name`: 所属分组文件夹名称（如 `group_1`）。
    /// - `mod_index`: 模组在分组内的索引（注入为 `$managed_slot_id`）。
    /// - `group_index`: 分组索引（用于匹配 `active_slot` 变量）。
    fn manage_mod(
        mod_path: &str,
        group_folder_name: &str,
        mod_index: i32,
        group_index: i32,
    ) -> Result<()> {
        let mod_path = Path::new(mod_path);
        if !mod_path.exists() || !mod_path.is_dir() {
            anyhow::bail!("Mod path does not exist: {:?}", mod_path);
        }

        // 路径安全验证：确保模组位于 _MANAGED_/group_xx 目录下
        // 禁止处理 # 目录及其他非预期路径，防止越权访问系统文件
        let mods_root = match mod_path.parent().and_then(|p| p.parent()) {
            Some(p) => p,
            None => {
                anyhow::bail!(
                    "请确保路径无特殊字符（\\ / : * ? \" < > |），已跳过"
                );
            }
        };

        if !Self::is_valid_mod_path(mods_root, mod_path) {
            anyhow::bail!(
                "请确保路径无特殊字符（\\ / : * ? \" < > |），已跳过"
            );
        }

        let ini_files = Self::find_ini_files_bfs(mod_path);
        debug!(
            "[manage_mod] processing mod: {:?} (group: {}, mod_index: {}, group_index: {}) - found {} .ini files",
            mod_path, group_folder_name, mod_index, group_index,
            ini_files.len()
        );

        for ini_file in &ini_files {
            // 备份路径：<ini>.baknrmm
            let backup_path = format!("{}.{}", ini_file.to_string_lossy(), MANAGED_BACKUP_EXT);
            let backup_path = Path::new(&backup_path);

            // 仅在备份不存在时创建，避免覆盖原始备份
            if !backup_path.exists() {
                fs::copy(ini_file, backup_path).map_err(|_| {
                    anyhow::anyhow!(
                        "无法创建模组备份，请确认文件未被占用且磁盘空间充足"
                    )
                })?;
                debug!(
                    "[manage_mod] created backup: {:?} -> {:?}",
                    ini_file, backup_path
                );
            }

            Self::modify_ini_file(ini_file, group_folder_name, mod_index, group_index)
                .map_err(|_| {
                    anyhow::anyhow!(
                        "模组数据更新失败，请确认文件未被占用且磁盘空间充足"
                    )
                })?;
        }

        Ok(())
    }

    /// 使用 BFS（广度优先搜索）算法查找指定路径下的所有 `.ini` 文件。
    ///
    /// 采用 DirWalker BFS 遍历，通过 VisitedPathPool 防止符号链接循环，
    /// 深度限制为 DEFAULT_MAX_TRAVERSAL_DEPTH。跟随符号链接（follow_symlinks=true）。
    /// 支持传入文件路径（直接返回该文件，如果是.ini文件）或目录路径（递归查找）。
    ///
    /// 参数：
    /// - `path`: 起始路径（文件或目录）。
    ///
    /// 返回：INI 文件路径列表。路径不存在时返回空 Vec。
    pub fn find_ini_files_bfs(path: &Path) -> Vec<PathBuf> {
        if !path.exists() {
            return Vec::new();
        }

        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if ext.eq_ignore_ascii_case("ini") {
                    return vec![path.to_path_buf()];
                }
            }
            return Vec::new();
        }

        DirWalker::new()
            .follow_symlinks(true)
            .file_ext("ini")
            .include_dirs(false)
            .skip_hidden(false)
            .walk_bfs(path)
            .into_iter()
            .map(|e| e.path)
            .collect()
    }

    /// 处理 INI 文件，移除 xxmi 专属的 INI 语句。
    ///
    /// xxmi 专属语句包括：
    /// - `managed_slot_id` 相关变量定义
    /// - `if $managed_slot_id == ...` / `endif` 条件块
    /// - `$\\modmanageragl\\group_*` 相关引用
    ///
    /// 处理前会先尝试从 `.ini_managed_backup` 备份恢复原始文件。
    ///
    /// 参数：
    /// - `paths`: INI 文件路径列表。
    ///
    /// 返回：是否处理成功。
    pub fn process_ini_files(paths: &[String]) -> Result<bool> {
        for path_str in paths {
            let path = Path::new(path_str);
            if !path.exists() || !path.is_file() {
                warn!("INI file does not exist: {:?}", path);
                continue;
            }

            // 先尝试从 ini_managed_backup 恢复原始文件
            Self::try_restore_from_managed_backup(path);

            let content = fs::read_to_string(path)
                .with_context(|| format!("Failed to read INI file: {:?}", path))?;

            let processed_content = Self::remove_xxmi_ini_statements(&content);

            if content != processed_content {
                fs::write(path, &processed_content)
                    .with_context(|| format!("Failed to write INI file: {:?}", path))?;
                info!("Processed INI file: {:?}", path);
            }
        }

        Ok(true)
    }

    /// 尝试从管理的备份文件恢复原始 INI。
    ///
    /// 检查 `<ini>.ini_managed_backup` 是否存在，若存在则复制覆盖原文件。
    /// 这确保了在 Restore Zone 处理前文件已恢复到 NRMM 修改前的状态。
    ///
    /// 参数：
    /// - `ini_path`: INI 文件路径。
    fn try_restore_from_managed_backup(ini_path: &Path) {
        let backup_path_str = format!(
            "{}.{}",
            ini_path.to_string_lossy(),
            MANAGED_BACKUP_EXT
        );
        let backup_path = Path::new(&backup_path_str);

        if backup_path.exists() {
            match fs::copy(backup_path, ini_path) {
                Ok(_) => {
                    info!("Restored INI from backup: {:?} -> {:?}", backup_path, ini_path);
                }
                Err(e) => {
                    warn!(
                        "Failed to restore INI from backup {:?}: {}",
                        backup_path, e
                    );
                }
            }
        }
    }

    /// 从 INI 文件内容中移除 xxmi 专属语句。
    ///
    /// 移除规则：
    /// 1. 移除 `global $managed_slot_id = ...` 行
    /// 2. 移除 `if $managed_slot_id == ...` 和对应的 `endif` 行
    /// 3. 移除包含 `$\\modmanageragl\\` 的行
    ///
    /// 参数：
    /// - `content`: INI 文件原始内容。
    ///
    /// 返回：处理后的 INI 文件内容。
    fn remove_xxmi_ini_statements(content: &str) -> String {
        let lines: Vec<&str> = content.lines().collect();
        let mut result: Vec<String> = Vec::new();
        let mut if_stack: Vec<String> = Vec::new();

        for line in lines {
            let trimmed = line.trim();
            let trimmed_lower = trimmed.to_lowercase();

            // 新段开始时重置 if 栈（与 NRMM 行为一致）
            if trimmed.starts_with('[') {
                if_stack.clear();
                result.push(line.to_string());
                continue;
            }

            // 移除 NRMM 注释标记行
            if trimmed.starts_with(';')
                && (trimmed_lower.contains("no reload mod manager")
                    || trimmed_lower.contains("\";-;\" are errored")
                    || trimmed_lower.contains("\";+;\" are disabled keys")
                    || trimmed_lower.contains("errored conditional blocks")
                    || trimmed_lower.contains("if certain syntax is only available"))
            {
                continue;
            }

            // 移除 managed_slot_id 变量声明
            if trimmed_lower.replace(' ', "").starts_with("global$managed_slot_id=") {
                continue;
            }

            // 清理 condition= 行中的管理表达式
            match Self::sanitize_condition_line(line) {
                Some(processed) => {
                    result.push(processed);
                    continue;
                }
                None => {} // 非 condition 行，继续后续检查
            }

            // 处理 manager if 行（使用栈追踪配对）
            if trimmed_lower.starts_with("if ") {
                if_stack.push(trimmed_lower.clone());
                if trimmed_lower.replace(' ', "")
                    .contains(r"if$managed_slot_id==$\modmanageragl\group_")
                {
                    continue; // 移除 manager if 行
                }
            }

            // 处理与 manager if 配对的 endif
            if trimmed_lower == "endif" {
                if let Some(if_line) = if_stack.pop() {
                    if if_line.replace(' ', "")
                        .contains(r"if$managed_slot_id==$\modmanageragl\group_")
                    {
                        continue; // 移除配对的 endif
                    }
                }
            }

            // 移除包含 $\modmanageragl\ 的行（剩余的未被 condition= 处理的行）
            if trimmed.contains(r"$\modmanageragl\") {
                continue;
            }

            // 恢复缩进：移除前 4 空格（NRMM 行为）
            result.push(Self::remove_first_four_spaces(line));
        }

        result.into_iter()
            .filter(|l| !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 清理 condition= 行中的 NRMM 管理表达式。
    ///
    /// 从 `condition = expression && $managed_slot_id == $\modmanageragl\group_1\active_slot`
    /// 中移除管理部分，保留原始条件表达式。若无剩余条件则返回空字符串。
    ///
    /// 参数：
    /// - `line`: 原始行文本。
    ///
    /// 返回：清理后的行文本，若整行被移除则返回 None。
    fn sanitize_condition_line(line: &str) -> Option<String> {
        let trimmed_lower = line.trim().to_lowercase().replace(' ', "");
        let is_disabled_comment = trimmed_lower.starts_with(";-;condition=");
        let is_enabled_comment = trimmed_lower.starts_with(";+;condition=");
        let is_normal = trimmed_lower.starts_with("condition=");

        if !is_disabled_comment && !is_enabled_comment && !is_normal {
            return None; // 非 condition 行，不处理
        }

        let eq_pos = line.find('=')?;
        let expression = line[eq_pos + 1..].trim();

        // 移除管理表达式
        let sanitized = Self::sanitize_key_condition_expression(expression);

        if sanitized.is_empty() {
            return Some(String::new()); // 管理表达式为空，整行移除
        }

        Some(if is_disabled_comment {
            format!(";-;condition = {}", sanitized)
        } else if is_enabled_comment {
            format!(";+;condition = {}", sanitized)
        } else {
            format!("condition = {}", sanitized)
        })
    }

    /// 从条件表达式中移除 NRMM 管理表达式部分。
    ///
    /// 例如 `$active == 1 && $managed_slot_id == $\modmanageragl\group_1\active_slot`
    /// 会被清理为 `$active == 1`。
    ///
    /// 参数：
    /// - `expression`: 原始条件表达式字符串。
    ///
    /// 返回：清理后的表达式。
    fn sanitize_key_condition_expression(expression: &str) -> String {
        // 匹配并移除包含 $\modmanageragl\ 的完整子表达式
        let re = regex::Regex::new(
            r"\s*(&&|\|\|)?\s*\$managed_slot_id\s*==\s*\$\\modmanageragl\\[^\s&|]*"
        ).expect("Static sanitize_key_condition_expression regex should be valid");
        re.replace_all(expression, "").trim().to_string()
    }

    /// 移除行首的 4 个空格（恢复 NRMM 注入前的原始缩进）。
    ///
    /// 参数：
    /// - `line`: 原始行文本。
    ///
    /// 返回：移除 4 空格后的行文本。
    fn remove_first_four_spaces(line: &str) -> String {
        if line.starts_with("    ") {
            line[4..].to_string()
        } else {
            line.to_string()
        }
    }

    /// 修改单个 INI 文件，注入槽位管理逻辑。
    ///
    /// 修改内容：
    /// 1. **Constants 段**：在 `[Constants]` 段中插入 `global $managed_slot_id = <mod_index>`；
    ///    若文件无 `[Constants]` 段则在文件开头创建。
    /// 2. **条件块包裹**：对所有 `CommandList` 和 `Key` 类型段落的内容，
    ///    用 `if $managed_slot_id == $\modmanageragl\group_<group_index>\active_slot` ... `endif` 包裹。
    ///
    /// 修改采用「先写临时文件再重命名」的方式，确保原子性，避免写入中途崩溃导致文件损坏。
    ///
    /// 参数：
    /// - `ini_path`: INI 文件路径。
    /// - `_group_folder_name`: 分组文件夹名称（当前未使用）。
    /// - `mod_index`: 模组索引（注入为 `$managed_slot_id` 的值）。
    /// - `group_index`: 分组索引（用于 `active_slot` 变量路径）。
    fn modify_ini_file(
        ini_path: &Path,
        _group_folder_name: &str,
        mod_index: i32,
        group_index: i32,
    ) -> Result<()> {
        let content = fs::read_to_string(ini_path)
            .with_context(|| format!("Failed to read INI file: {:?}", ini_path))?;

        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

        let mut has_constants = false;
        let mut constants_section_idx: Option<usize> = None;
        let mut sections_with_commandlist: Vec<(usize, usize)> = Vec::new();
        let mut current_section_start: Option<usize> = None;
        let mut current_section_name = String::new();

        // 第一遍扫描：识别 Constants 段位置，以及所有 CommandList/Key 段的起止行
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if is_section_header(trimmed) {
                if let Some(section_name) = parse_section_name(trimmed) {
                    // 遇到新段时，记录上一段的起止范围
                    if let Some(start) = current_section_start {
                        let section_type = get_section_type(&current_section_name);
                        if section_type == SectionType::CommandList
                            || section_type == SectionType::Key
                        {
                            sections_with_commandlist.push((start, i));
                        }
                    }

                    let section_lower = section_name.to_lowercase();
                    if section_lower == "constants" {
                        has_constants = true;
                        constants_section_idx = Some(i);
                    }

                    current_section_start = Some(i);
                    current_section_name = section_name;
                }
            }
        }

        // 处理文件末尾的最后一个段
        if let Some(start) = current_section_start {
            let section_type = get_section_type(&current_section_name);
            if section_type == SectionType::CommandList || section_type == SectionType::Key {
                sections_with_commandlist.push((start, lines.len()));
            }
        }

        // 注入 $managed_slot_id 变量到 Constants 段
        let managed_var_line = format!("global $managed_slot_id = {}", mod_index);

        if has_constants {
            // 在已有 Constants 段头部插入变量
            if let Some(idx) = constants_section_idx {
                let mut insert_idx = idx + 1;
                while insert_idx < lines.len() {
                    let trimmed = lines[insert_idx].trim();
                    if trimmed.is_empty() || is_comment_line(trimmed) {
                        // 跳过空行和注释行
                        insert_idx += 1;
                    } else if is_section_header(trimmed) {
                        // 遇到下一段则停止
                        break;
                    } else {
                        // 跳过第一条实际内容行后插入
                        insert_idx += 1;
                        break;
                    }
                }
                lines.insert(insert_idx, managed_var_line);
            }
        } else {
            // 文件无 Constants 段，在开头创建
            let constants_header = "[Constants]".to_string();
            lines.insert(0, managed_var_line);
            lines.insert(0, constants_header);
            lines.insert(2, String::new());
        }

        // 构造条件判断行
        let condition = format!(
            "if $managed_slot_id == $\\modmanageragl\\group_{}\\active_slot",
            group_index
        );

        // 从后向前插入条件块，避免行号变化影响前面的插入位置
        sections_with_commandlist.sort_by(|a, b| b.0.cmp(&a.0));

        for (start, end) in &sections_with_commandlist {
            // 找到段内第一条实际内容行（跳过空行和注释）
            let mut first_content_line = *start + 1;
            while first_content_line < *end {
                let trimmed = lines[first_content_line].trim();
                if trimmed.is_empty() || is_comment_line(trimmed) {
                    first_content_line += 1;
                } else {
                    break;
                }
            }

            if first_content_line >= *end {
                // 段内无实际内容，跳过
                continue;
            }

            let last_content_line = end - 1;
            if last_content_line < first_content_line {
                continue;
            }

            // 在内容前后插入 if/endif
            lines.insert(first_content_line, condition.clone());
            lines.insert(last_content_line + 2, "endif".to_string());
        }

        // 使用 CRLF 行尾拼接（3DMigoto 在 Windows 下要求 CRLF）
        let mut output = String::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                output.push_str("\r\n");
            }
            output.push_str(line);
        }

        // 先写入临时文件，再原子重命名，避免写入中断导致文件损坏
        let tmp_path = ini_path.with_extension("tmp");
        fs::write(&tmp_path, &output)
            .with_context(|| format!("Failed to write temporary INI file: {:?}", tmp_path))?;

        fs::rename(&tmp_path, ini_path)
            .with_context(|| format!("Failed to rename temporary INI file: {:?}", ini_path))?;

        Ok(())
    }

    /// 获取当前 UTC 时间的字符串表示（用于收藏时间戳）。
    fn current_datetime_string() -> String {
        let now = OffsetDateTime::now_utc();
        format!("{}", now)
    }

    /// 计算 INI 文件内容的 hash
    ///
    /// 使用 `DefaultHasher`（默认 SipHash 算法）对内容进行 hash，
    /// 返回 16 进制字符串。用于检测多个启用模组的内容是否重复。
    fn compute_ini_content_hash(content: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// 一次性扫描整个 `_MANAGED_` 目录树，收集所有 INI 文件的路径。
    /// 返回 `模组根路径 → INI 文件路径列表` 的映射表。
    /// 相比逐个模组调用 `find_ini_files_bfs`，此方法将 N 次遍历合并为 1 次 DirWalker。
    fn scan_all_ini_files_in_managed(managed_path: &str) -> HashMap<String, Vec<PathBuf>> {
        let mut result: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let managed = Path::new(managed_path);
        if !managed.exists() {
            return result;
        }

        let entries = DirWalker::new()
            .follow_symlinks(false)
            .file_ext("ini")
            .include_dirs(false)
            .skip_hidden(false)
            .walk_bfs(managed);

        for entry in entries {
            let path = &entry.path;

            // 找到该文件在 _MANAGED_ 下的直接子目录（模组根目录）
            let mut ancestors = path.ancestors();
            let mut mod_root = None;
            while let Some(ancestor) = ancestors.next() {
                if ancestor == managed {
                    break;
                }
                mod_root = Some(ancestor);
            }

            if let Some(root) = mod_root {
                let key = root.to_string_lossy().to_string();
                result.entry(key).or_default().push(path.clone());
            }
        }

        // 排序确保 hash 计算一致性
        for files in result.values_mut() {
            files.sort();
        }
        result
    }

    /// 检测启用的 mod 的 hash 冲突
    /// 返回 HashConflictReport，其中 enabled_mod_hashes 只包含有冲突的 hash
    ///
    /// 流程：
    /// 1. 一次性扫描整个 _MANAGED_ 目录树收集所有 INI 文件（1 次 DirWalker 遍历）。
    /// 2. 遍历所有分组中启用的模组（跳过 None 槽位和禁用模组）。
    /// 3. 将每个模组下所有 INI 文件内容合并后计算 hash。
    /// 4. 按 hash 分组，仅保留出现次数 > 1 的 hash（即存在冲突）。
    /// 5. 同时构建 `conflicts` 结构化条目，便于前端直接渲染。
    ///
    /// 参数：
    /// - `managed_path`: `_MANAGED_` 目录路径。
    /// - `groups`: 分组数据列表。
    pub fn check_enabled_mod_hash_conflicts(
        managed_path: &str,
        groups: &[ModGroupData],
    ) -> Result<HashConflictReport> {
        let mut report = HashConflictReport::default();
        let mut content_by_hash: HashMap<String, Vec<HashedModInfo>> = HashMap::new();

        // 一次性预扫描所有 INI 文件，避免 N 次重复遍历
        let all_ini_files = Self::scan_all_ini_files_in_managed(managed_path);

        // 收集所有启用的 mod 的内容 hash
        for group in groups {
            for mod_data in &group.mods_in_group {
                // 跳过 None slot 和禁用的 mod
                if mod_data.mod_path == "None" || mod_data.is_disabled {
                    continue;
                }

                let mod_path = Path::new(&mod_data.mod_path);
                if !mod_path.exists() {
                    continue;
                }

                // 从预扫描结果中获取该模组的 INI 文件列表
                let mod_key = mod_path.to_string_lossy().to_string();
                let ini_files = match all_ini_files.get(&mod_key) {
                    Some(files) => files.clone(),
                    None => continue,
                };

                // 收集该模组下所有 INI 文件内容
                let mut combined_content = String::new();
                for ini_path in &ini_files {
                    if let Ok(content) = fs::read_to_string(ini_path) {
                        combined_content.push_str(&content);
                        combined_content.push('\n');
                    }
                }

                if !combined_content.is_empty() {
                    let hash = Self::compute_ini_content_hash(&combined_content);
                    let info = HashedModInfo {
                        mod_path: mod_data.mod_path.clone(),
                        mod_name: mod_data.mod_name.clone(),
                        group_name: group.group_name.clone(),
                        hash: hash.clone(),
                    };

                    content_by_hash.entry(hash).or_default().push(info);
                }
            }
        }

        // 找出有冲突的 hash（同一个 hash 有多个 mod）
        for (hash, mods) in content_by_hash {
            if mods.len() > 1 {
                report.enabled_mod_hashes.insert(hash.clone(), mods.clone());
                // 同时填充结构化冲突条目
                let entry = HashConflictEntry {
                    hash,
                    mod_names: mods.iter().map(|m| m.mod_name.clone()).collect(),
                    mod_paths: mods.iter().map(|m| m.mod_path.clone()).collect(),
                    group_names: mods.iter().map(|m| m.group_name.clone()).collect(),
                };
                report.conflicts.push(entry);
            }
        }

        info!(
            "Hash conflict check: {} conflicts found in {} groups",
            report.enabled_mod_hashes.len(),
            groups.len()
        );

        Ok(report)
    }

    /// 检查启用的 mod 是否存在 hash 冲突，并生成警告日志
    /// 返回：(冲突数量, 冲突详情)
    ///
    /// 该函数是对 `check_enabled_mod_hash_conflicts` 的封装，
    /// 将冲突报告转换为人类可读的日志条目。
    ///
    /// 参数：
    /// - `managed_path`: `_MANAGED_` 目录路径。
    /// - `groups`: 分组数据列表。
    ///
    /// 返回：`(冲突数量, 日志条目列表)`。
    pub fn check_and_report_hash_conflicts(
        managed_path: &str,
        groups: &[ModGroupData],
    ) -> (usize, Vec<LogEntry>, HashConflictReport) {
        match Self::check_enabled_mod_hash_conflicts(managed_path, groups) {
            Ok(report) => {
                let conflict_count = report.enabled_mod_hashes.len();
                let mut logs = Vec::new();

                if conflict_count > 0 {
                    logs.push(LogEntry::warn(format!(
                        "Found {} enabled mod(s) with hash conflicts",
                        conflict_count
                    )));

                    // 为每个冲突生成详细日志
                    for (hash, mods) in &report.enabled_mod_hashes {
                        let mod_names: Vec<String> =
                            mods.iter().map(|m| m.mod_name.clone()).collect();
                        logs.push(LogEntry::warn(format!(
                            "Hash conflict: [{}] appears in mods: {} (group: {})",
                            &hash[..8.min(hash.len())],
                            mod_names.join(", "),
                            mods.first()
                                .map(|m| m.group_name.clone())
                                .unwrap_or_default()
                        )));
                    }
                }

                (conflict_count, logs, report)
            }
            Err(e) => {
                warn!("Failed to check hash conflicts: {}", e);
                (
                    0,
                    vec![LogEntry::warn(format!("Hash conflict check failed: {}", e))],
                    HashConflictReport::default(),
                )
            }
        }
    }

    /// 检测文件的真实类型（通过文件头魔数）。
    ///
    /// 参数：
    /// - `path`: 文件路径。
    ///
    /// 返回：检测到的文件类型。
    pub fn detect_archive_type(path: &Path) -> ArchiveType {
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(_) => return ArchiveType::Unknown,
        };

        // ZIP 文件头: 50 4B 03 04
        if bytes.len() >= 4 && bytes[0] == 0x50 && bytes[1] == 0x4B && bytes[2] == 0x03 && bytes[3] == 0x04 {
            return ArchiveType::Zip;
        }

        // 7z 文件头: 37 7A BC AF 27 1C
        if bytes.len() >= 6 && bytes[0] == 0x37 && bytes[1] == 0x7A && bytes[2] == 0xBC && bytes[3] == 0xAF && bytes[4] == 0x27 && bytes[5] == 0x1C {
            return ArchiveType::SevenZip;
        }

        // RAR 文件头: 52 61 72 21 1A 07 00 (RAR 5.0) 或 52 61 72 21 1A 07 (RAR 4.x)
        if bytes.len() >= 7 && bytes[0] == 0x52 && bytes[1] == 0x61 && bytes[2] == 0x72 && bytes[3] == 0x21 && bytes[4] == 0x1A && bytes[5] == 0x07 {
            return ArchiveType::Rar;
        }

        ArchiveType::Unknown
    }

    /// 验证文件是否为有效的压缩文件。
    ///
    /// 验证策略：
    /// 1. 优先检查文件扩展名（.zip, .7z, .rar）
    /// 2. 同时检查文件头魔数确保文件格式真实性
    ///
    /// 参数：
    /// - `path`: 文件路径。
    ///
    /// 返回：(是否有效, 检测到的文件类型)
    pub fn validate_archive_file(path: &Path) -> (bool, ArchiveType) {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let expected_type = match ext.as_str() {
            "zip" => ArchiveType::Zip,
            "7z" => ArchiveType::SevenZip,
            "rar" => ArchiveType::Rar,
            _ => ArchiveType::Unknown,
        };

        let actual_type = Self::detect_archive_type(path);

        if expected_type != ArchiveType::Unknown && expected_type == actual_type {
            (true, actual_type)
        } else if actual_type != ArchiveType::Unknown {
            (true, actual_type)
        } else {
            (false, ArchiveType::Unknown)
        }
    }

    /// 检测归档文件是否加密（需要密码才能解压）。
    ///
    /// 参数：
    /// - `file_path`: 归档文件路径。
    ///
    /// 返回：true 表示文件已加密需要密码；false 表示未加密。
    pub fn is_archive_encrypted(file_path: &Path) -> Result<bool> {
        let (valid, archive_type) = Self::validate_archive_file(file_path);
        if !valid {
            anyhow::bail!("Invalid archive file: {:?}", file_path);
        }

        match archive_type {
            ArchiveType::Zip => {
                let file = fs::File::open(file_path)
                    .with_context(|| format!("Failed to open ZIP file: {:?}", file_path))?;
                let mut archive = ZipArchive::new(file)
                    .with_context(|| format!("Failed to read ZIP file: {:?}", file_path))?;
                for i in 0..archive.len() {
                    match archive.by_index(i) {
                        Ok(_) => continue,
                        Err(e) => {
                            let err_msg = e.to_string();
                            if err_msg.contains("encrypted") || err_msg.contains("password") {
                                return Ok(true);
                            }
                            return Err(e).with_context(|| format!("Failed to read entry {} in ZIP file", i));
                        }
                    }
                }
                Ok(false)
            }
            ArchiveType::SevenZip => {
                let mut file = fs::File::open(file_path)
                    .with_context(|| format!("Failed to open 7z file: {:?}", file_path))?;
                let len = file.metadata()
                    .with_context(|| format!("Failed to get file metadata: {:?}", file_path))?.len();
                match sevenz_rust::SevenZReader::new(&mut file, len, Password::empty()) {
                    Ok(_) => Ok(false),
                    Err(e) => {
                        let err_msg = e.to_string().to_lowercase();
                        if err_msg.contains("password") || err_msg.contains("encrypt") {
                            Ok(true)
                        } else {
                            Ok(false)
                        }
                    }
                }
            }
            ArchiveType::Rar => {
                let data = fs::read(file_path)
                    .with_context(|| format!("Failed to read RAR file: {:?}", file_path))?;
                let archive = match ArchiveReader::read(&data) {
                    Ok(a) => a,
                    Err(e) => {
                        let err_msg = e.to_string().to_lowercase();
                        if err_msg.contains("password") || err_msg.contains("encrypt") {
                            return Ok(true);
                        }
                        return Err(e).with_context(|| format!("Failed to parse RAR file: {:?}", file_path));
                    }
                };
                let result = archive.extract_to(None, |_| {
                    Ok(Box::new(std::io::sink()))
                });
                match result {
                    Ok(_) => Ok(false),
                    Err(e) => {
                        let err_msg = e.to_string().to_lowercase();
                        if err_msg.contains("password") || err_msg.contains("encrypt") || err_msg.contains("wrong password") {
                            Ok(true)
                        } else {
                            Err(anyhow::Error::from(e)).with_context(|| format!("Failed to read RAR file entries: {:?}", file_path))
                        }
                    }
                }
            }
            ArchiveType::Unknown => {
                anyhow::bail!("Unknown archive format");
            }
        }
    }

    /// 使用 BFS 算法递归查找目录下所有文件。
    ///
    /// 采用 DirWalker BFS 遍历，通过 VisitedPathPool 防止符号链接循环，
    /// 深度限制为 DEFAULT_MAX_TRAVERSAL_DEPTH。跟随符号链接（follow_symlinks=true）。
    ///
    /// 参数：
    /// - `path`: 起始目录路径。
    ///
    /// 返回：目录下所有文件的路径列表。
    pub fn find_all_files_bfs(path: &Path) -> Vec<PathBuf> {
        if !path.exists() || !path.is_dir() {
            return Vec::new();
        }

        DirWalker::new()
            .follow_symlinks(true)
            .include_dirs(false)
            .skip_hidden(false)
            .walk_bfs(path)
            .into_iter()
            .map(|e| e.path)
            .collect()
    }

    /// 校验解压条目路径是否安全，防止 Zip Slip 路径遍历攻击。
    ///
    /// 规则：
    /// - 条目名不得以 `/` 或 `\\` 开头（绝对路径）。
    /// - 条目名中不得包含 `..` 组件。
    /// - 规范化后的最终路径必须仍位于 `base_dir` 之下。
    ///
    /// 参数：
    /// - `base_dir`: 解压目标根目录。
    /// - `entry_name`: 压缩包内的原始条目名称。
    ///
    /// 返回：校验通过后的安全路径。
    fn sanitize_extract_path(base_dir: &Path, entry_name: &str) -> Result<PathBuf> {
        let normalized = entry_name.replace('\\', "/");
        if normalized.starts_with('/') {
            anyhow::bail!("Absolute path in archive is not allowed: {:?}", entry_name);
        }

        let mut components = Vec::new();
        for part in normalized.split('/') {
            match part {
                "" | "." => continue,
                ".." => {
                    if components.pop().is_none() {
                        anyhow::bail!(
                            "Invalid archive entry path (potential Zip Slip attack): {:?}",
                            entry_name
                        );
                    }
                }
                _ => components.push(part),
            }
        }

        let rel_path = components.join("/");
        let final_path = base_dir.join(&rel_path);

        // 再次确认最终路径以 base_dir 开头（防御性校验）
        if !final_path.starts_with(base_dir) {
            anyhow::bail!(
                "Invalid archive entry path (potential Zip Slip attack): {:?}",
                entry_name
            );
        }

        Ok(final_path)
    }

    /// 解压 ZIP 文件到指定目录（支持可选密码）。
    ///
    /// 参数：
    /// - `file_path`: ZIP 文件路径。
    /// - `dest_dir`: 目标目录路径。
    /// - `password`: 可选解压密码。
    ///
    /// 返回：是否解压成功。
    pub fn extract_zip(file_path: &Path, dest_dir: &Path, password: Option<&str>) -> Result<bool> {
        let file = fs::File::open(file_path)
            .with_context(|| format!("Failed to open ZIP file: {:?}", file_path))?;

        let mut archive = ZipArchive::new(file)
            .with_context(|| format!("Failed to read ZIP file: {:?}", file_path))?;

        fs::create_dir_all(dest_dir)
            .with_context(|| format!("Failed to create destination directory: {:?}", dest_dir))?;

        for i in 0..archive.len() {
            let mut file = if let Some(pwd) = password {
                match archive.by_index_decrypt(i, pwd.as_bytes()) {
                    Ok(f) => f,
                    Err(e) => anyhow::bail!("Failed to read ZIP entry {} (wrong password?): {}", i, e),
                }
            } else {
                archive.by_index(i)
                    .with_context(|| format!("Failed to read entry {} in ZIP file (file may be encrypted)", i))?
            };

            Self::write_zip_entry(&mut file, dest_dir)?;
        }

        info!("Extracted ZIP file: {:?} -> {:?}", file_path, dest_dir);
        Ok(true)
    }

    fn write_zip_entry<R: std::io::Read + std::io::Seek>(file: &mut zip::read::ZipFile<'_, R>, dest_dir: &Path) -> Result<()> {
        let entry_path = Self::sanitize_extract_path(dest_dir, file.name())?;

        if (*file.name()).ends_with('/') {
            fs::create_dir_all(&entry_path)
                .with_context(|| format!("Failed to create directory: {:?}", entry_path))?;
        } else {
            if let Some(parent) = entry_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create parent directory: {:?}", parent))?;
            }

            let mut out_file = fs::File::create(&entry_path)
                .with_context(|| format!("Failed to create file: {:?}", entry_path))?;

            std::io::copy(file, &mut out_file)
                .with_context(|| format!("Failed to write file: {:?}", entry_path))?;
        }
        Ok(())
    }

    /// 解压 7z 文件到指定目录（支持可选密码）。
    ///
    /// 先解压到临时目录，再逐项校验路径安全后移动到目标目录，
    /// 防止 sevenz-rust 内部出现路径穿越（Zip Slip）。
    ///
    /// 参数：
    /// - `file_path`: 7z 文件路径。
    /// - `dest_dir`: 目标目录路径。
    /// - `password`: 可选解压密码。
    ///
    /// 返回：是否解压成功。
    pub fn extract_7z(file_path: &Path, dest_dir: &Path, password: Option<&str>) -> Result<bool> {
        fs::create_dir_all(dest_dir)
            .with_context(|| format!("Failed to create destination directory: {:?}", dest_dir))?;

        let temp_dir = tempfile::tempdir()
            .with_context(|| "Failed to create temporary directory for 7z extraction")?;
        let temp_path = temp_dir.path();

        match password {
            Some(pwd) => sevenz_rust::decompress_file_with_password(file_path, temp_path, pwd.into())
                .with_context(|| format!("Failed to extract encrypted 7z file: {:?}", file_path))?,
            None => decompress_file(file_path, temp_path)
                .with_context(|| format!("Failed to extract 7z file: {:?}", file_path))?,
        }

        // 遍历临时目录，校验并移动文件到目标目录
        let entries = fs::read_dir(temp_path)
            .with_context(|| format!("Failed to read temporary directory: {:?}", temp_path))?;
        for entry in entries {
            let entry = entry.with_context(|| "Failed to read entry in temporary directory")?;
            let src = entry.path();
            let entry_name = src.strip_prefix(temp_path)
                .with_context(|| format!("Failed to strip prefix from {:?}", src))?;
            let entry_name_str = entry_name.to_string_lossy().replace('\\', "/");
            let dest = Self::sanitize_extract_path(dest_dir, &entry_name_str)?;

            if src.is_dir() {
                fs::create_dir_all(&dest)
                    .with_context(|| format!("Failed to create directory: {:?}", dest))?;
            } else {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("Failed to create parent directory: {:?}", parent))?;
                }
                fs::rename(&src, &dest)
                    .with_context(|| format!("Failed to move file: {:?} -> {:?}", src, dest))?;
            }
        }

        info!("Extracted 7z file: {:?} -> {:?}", file_path, dest_dir);
        Ok(true)
    }

    /// 解压 RAR 文件到指定目录（支持可选密码）。
    ///
    /// 参数：
    /// - `file_path`: RAR 文件路径。
    /// - `dest_dir`: 目标目录路径。
    /// - `password`: 可选解压密码。
    ///
    /// 返回：是否解压成功。
    pub fn extract_rar(file_path: &Path, dest_dir: &Path, password: Option<&str>) -> Result<bool> {
        fs::create_dir_all(dest_dir)
            .with_context(|| format!("Failed to create destination directory: {:?}", dest_dir))?;

        let data = fs::read(file_path)
            .with_context(|| format!("Failed to read RAR file: {:?}", file_path))?;
        let archive = ArchiveReader::read(&data)
            .with_context(|| format!("Failed to parse RAR file: {:?}", file_path))?;

        let password_bytes = password.map(|p| p.as_bytes());

        archive.extract_to(password_bytes, |meta| {
            let name_str = String::from_utf8_lossy(&meta.name);
            let entry_path = match Self::sanitize_extract_path(dest_dir, &name_str) {
                Ok(p) => p,
                Err(e) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Invalid path in RAR archive: {:?} - {}", name_str, e),
                    ).into());
                }
            };

            if meta.is_directory {
                fs::create_dir_all(&entry_path)?;
                Ok(Box::new(std::io::sink()))
            } else {
                if let Some(parent) = entry_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let file = fs::File::create(&entry_path)?;
                Ok(Box::new(file))
            }
        }).map_err(|e| anyhow::Error::from(e))
            .with_context(|| format!("Failed to extract RAR file: {:?}", file_path))?;

        info!("Extracted RAR file: {:?} -> {:?}", file_path, dest_dir);
        Ok(true)
    }

    /// 将文件移动到系统回收站。
    /// 如果回收站操作失败，返回错误但不删除文件（调用方应捕获并忽略错误）。
    pub fn move_to_trash(file_path: &Path) -> Result<()> {
        trash::delete(file_path)
            .with_context(|| format!("Failed to move file to trash: {:?}", file_path))?;
        info!("Moved file to recycle bin: {:?}", file_path);
        Ok(())
    }

    /// 解压压缩文件到指定目录（自动识别文件类型）。
    ///
    /// 参数：
    /// - `file_path`: 压缩文件路径。
    /// - `dest_dir`: 目标目录路径。
    /// - `password`: 可选解压密码。
    ///
    /// 返回：是否解压成功。
    pub fn extract_archive(file_path: &Path, dest_dir: &Path, password: Option<&str>) -> Result<bool> {
        let (valid, archive_type) = Self::validate_archive_file(file_path);

        if !valid {
            anyhow::bail!("Invalid archive file: {:?}", file_path);
        }

        let archive_stem = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("archive");

        let result = match archive_type {
            ArchiveType::Zip => Self::extract_zip(file_path, dest_dir, password),
            ArchiveType::SevenZip => Self::extract_7z(file_path, dest_dir, password),
            ArchiveType::Rar => Self::extract_rar(file_path, dest_dir, password),
            ArchiveType::Unknown => {
                anyhow::bail!("Unknown archive format");
            }
        };

        if result.is_ok() {
            if let Err(e) = Self::smart_flatten_archive_root(dest_dir, archive_stem) {
                warn!("Smart flatten archive root failed: {}", e);
            }
        }

        result
    }

    /// 智能扁平化解压目录（Bandizip 风格）：如果目标目录下只有一个子目录，
    /// 且该子目录名称与压缩包文件名（不含后缀）匹配（大小写不敏感），
    /// 则将子目录内容上移一层，避免 ArchiveName/ArchiveName/files 的双层嵌套问题。
    ///
    /// 参数：
    /// - `dest_dir`: 解压后的目标目录路径。
    /// - `archive_stem`: 压缩包文件名（不含后缀），用于名称匹配。
    ///
    /// 返回：是否执行了目录扁平化操作。
    fn smart_flatten_archive_root(dest_dir: &Path, archive_stem: &str) -> Result<bool> {
        if !dest_dir.exists() || !dest_dir.is_dir() {
            return Ok(false);
        }

        let entries = fs::read_dir(dest_dir)
            .with_context(|| format!("Failed to read directory: {:?}", dest_dir))?
            .filter_map(|e| e.ok())
            .collect::<Vec<_>>();

        if entries.is_empty() {
            return Ok(false);
        }

        let has_direct_files = entries.iter().any(|e| e.path().is_file());
        if has_direct_files {
            return Ok(false);
        }

        let dirs = entries
            .iter()
            .filter(|e| e.path().is_dir())
            .collect::<Vec<_>>();

        if dirs.len() != 1 || entries.len() != 1 {
            return Ok(false);
        }

        let inner_dir = dirs[0].path();
        let inner_dir_name = match inner_dir.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => return Ok(false),
        };

        if inner_dir_name.to_lowercase() != archive_stem.to_lowercase() {
            return Ok(false);
        }

        let inner_entries = fs::read_dir(&inner_dir)
            .with_context(|| format!("Failed to read inner directory: {:?}", inner_dir))?
            .filter_map(|e| e.ok())
            .collect::<Vec<_>>();

        for entry in inner_entries {
            let src = entry.path();
            let dest = dest_dir.join(entry.file_name());
            if dest.exists() {
                continue;
            }
            fs::rename(&src, &dest).with_context(|| {
                format!("Failed to move {:?} -> {:?}", src, dest)
            })?;
        }

        if let Ok(mut remaining) = fs::read_dir(&inner_dir) {
            if remaining.next().is_none() {
                if let Err(e) = fs::remove_dir(&inner_dir) {
                    debug!("Failed to remove empty inner directory {:?}: {}", inner_dir, e);
                }
                debug!("Smart-flattened archive root (matched stem '{}'): {:?}", archive_stem, dest_dir);
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// 递归复制目录内容到目标路径（使用 DirWalker BFS 遍历避免栈溢出）。
    ///
    /// 采用 DirWalker BFS 遍历，通过 VisitedPathPool 防止符号链接循环，
    /// 深度限制为 DEFAULT_MAX_TRAVERSAL_DEPTH。不跟随符号链接，遇到符号链接条目跳过。
    /// BFS 顺序保证父目录在子文件/子目录之前被处理。
    ///
    /// 参数：
    /// - `src`: 源目录路径。
    /// - `dst`: 目标目录路径（会被自动创建）。
    fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
        fs::create_dir_all(dst)
            .with_context(|| format!("Failed to create destination directory: {:?}", dst))?;

        if !src.exists() || !src.is_dir() {
            return Ok(());
        }

        let mut first_error: Option<anyhow::Error> = None;

        DirWalker::new()
            .follow_symlinks(false)
            .include_dirs(true)
            .include_files(true)
            .skip_hidden(false)
            .walk(src, None, |entry| {
                if entry.depth == 0 {
                    return true;
                }

                let rel = match entry.path.strip_prefix(src) {
                    Ok(r) => r,
                    Err(_) => return true,
                };
                let dest_path = dst.join(rel);

                match entry.file_type {
                    FileKind::Dir => {
                        if let Err(e) = fs::create_dir_all(&dest_path)
                            .with_context(|| format!("Failed to create directory: {:?}", dest_path))
                        {
                            first_error = Some(e);
                            return false;
                        }
                    }
                    FileKind::File => {
                        if let Err(e) = fs::copy(&entry.path, &dest_path)
                            .with_context(|| format!("Failed to copy file: {:?} -> {:?}", entry.path, dest_path))
                        {
                            first_error = Some(e);
                            return false;
                        }
                    }
                    FileKind::Symlink => {}
                }
                true
            });

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// 导出单个模组为 7z 压缩文件（极限压缩）。
    ///
    /// 导出流程：
    /// 1. 将模组目录复制到临时目录（避免修改原始文件）
    /// 2. 查找临时副本中所有 .ini 文件
    /// 3. 调用模组还原功能，移除 xxmi 专属 INI 语句
    /// 4. 使用极限压缩算法生成 7z 文件
    /// 5. 清理临时目录
    /// 6. 导出文件名称去除禁用标识符
    ///
    /// 参数：
    /// - `mod_path`: 模组目录路径。
    /// - `dest_dir`: 目标目录路径。
    ///
    /// 返回：导出文件的完整路径。
    pub fn export_mod(mod_path: &str, dest_dir: &str) -> Result<String> {
        let mod_path = Path::new(mod_path);
        let dest_dir = Path::new(dest_dir);

        if !mod_path.exists() || !mod_path.is_dir() {
            anyhow::bail!("Mod path does not exist: {:?}", mod_path);
        }

        fs::create_dir_all(dest_dir)
            .with_context(|| format!("Failed to create destination directory: {:?}", dest_dir))?;

        let mod_name = mod_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("mod");

        let clean_name = if Self::is_disabled_name(mod_name) {
            &mod_name[DISABLED_PREFIX.len()..].trim_start_matches('_')
        } else {
            mod_name
        };
        let dest_file = dest_dir.join(format!("{}.7z", clean_name));

        // 创建临时目录用于存放还原后的模组副本
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let temp_dir = std::env::temp_dir().join(format!("xxmi_export_mod_{}", timestamp));
        let temp_mod_dir = temp_dir.join(clean_name);

        // 执行复制→还原→压缩→清理流程
        let result = (|| -> Result<()> {
            // 1. 复制模组到临时目录
            Self::copy_dir_recursive(mod_path, &temp_mod_dir)?;

            // 2. 查找临时副本中所有 .ini 文件
            let ini_files = Self::find_ini_files_bfs(&temp_mod_dir);
            if !ini_files.is_empty() {
                let ini_paths: Vec<String> = ini_files
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                // 3. 处理 INI 文件，移除 xxmi 专属语句
                Self::process_ini_files(&ini_paths)?;
            }

            // 4. 压缩为 7z
            Self::compress_to_7z(&[temp_mod_dir.clone()], &dest_file)?;
            Ok(())
        })();

        // 5. 无论成功或失败，都清理临时目录
        let _ = fs::remove_dir_all(&temp_dir);

        result?;
        info!("Exported mod: {:?} -> {:?}", mod_path, dest_file);
        Ok(dest_file.to_string_lossy().to_string())
    }

    /// 导出分组模组为 7z 压缩文件（保持目录结构）。
    ///
    /// 导出流程：
    /// 1. 将分组目录复制到临时目录（避免修改原始文件）
    /// 2. 查找临时副本中所有 .ini 文件
    /// 3. 调用模组还原功能，移除所有模组的 xxmi 专属 INI 语句
    /// 4. 使用极限压缩算法生成 7z 文件
    /// 5. 清理临时目录
    /// 6. 保持"分组名称->模组名称"的目录结构
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    /// - `dest_dir`: 目标目录路径。
    ///
    /// 返回：导出文件的完整路径。
    pub fn export_group(group_path: &str, dest_dir: &str) -> Result<String> {
        let group_path = Path::new(group_path);
        let dest_dir = Path::new(dest_dir);

        if !group_path.exists() || !group_path.is_dir() {
            anyhow::bail!("Group path does not exist: {:?}", group_path);
        }

        fs::create_dir_all(dest_dir)
            .with_context(|| format!("Failed to create destination directory: {:?}", dest_dir))?;

        let group_name = group_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("group");

        let dest_file = dest_dir.join(format!("{}.7z", group_name));

        // 创建临时目录用于存放还原后的分组副本
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let temp_dir = std::env::temp_dir().join(format!("xxmi_export_group_{}", timestamp));
        let temp_group_dir = temp_dir.join(group_name);

        // 执行复制→还原→压缩→清理流程
        let result = (|| -> Result<()> {
            // 1. 复制分组到临时目录
            Self::copy_dir_recursive(group_path, &temp_group_dir)?;

            // 2. 查找临时副本中所有 .ini 文件（涵盖所有模组子目录）
            let ini_files = Self::find_ini_files_bfs(&temp_group_dir);
            if !ini_files.is_empty() {
                let ini_paths: Vec<String> = ini_files
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                // 3. 处理 INI 文件，移除 xxmi 专属语句
                Self::process_ini_files(&ini_paths)?;
            }

            // 4. 收集临时副本中的所有模组子目录作为压缩源
            let mut source_paths = Vec::new();
            if let Ok(entries) = fs::read_dir(&temp_group_dir) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if entry_path.is_dir() {
                        source_paths.push(entry_path);
                    }
                }
            }

            Self::compress_to_7z(&source_paths, &dest_file)?;
            Ok(())
        })();

        // 5. 无论成功或失败，都清理临时目录
        let _ = fs::remove_dir_all(&temp_dir);

        result?;
        info!("Exported group: {:?} -> {:?}", group_path, dest_file);
        Ok(dest_file.to_string_lossy().to_string())
    }

    /// 使用 7z 格式压缩文件（LZMA2 压缩算法）。
    ///
    /// 业务逻辑：
    /// - 对每个源路径，保留其目录名/文件名作为压缩包内的根条目。
    /// - 目录会递归遍历，保留相对路径结构。
    /// - 文件直接以文件名作为条目名压缩。
    ///
    /// 参数：
    /// - `source_paths`: 源文件/目录路径列表。
    /// - `dest_path`: 目标压缩文件路径（.7z）。
    ///
    /// 返回：是否压缩成功。
    pub fn compress_to_7z(source_paths: &[PathBuf], dest_path: &Path) -> Result<bool> {
        use sevenz_rust::{SevenZArchiveEntry, SevenZWriter};

        let mut writer = SevenZWriter::create(dest_path)
            .with_context(|| format!("Failed to create 7z archive: {:?}", dest_path))?;

        for source_path in source_paths {
            // 取源路径末尾名称作为压缩包内根条目名（保留目录结构）
            let base_name = source_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("root")
                .to_string();

            if source_path.is_file() {
                // 单个文件：直接以文件名作为条目名压缩
                let entry = SevenZArchiveEntry::from_path(source_path, base_name.clone());
                let file = fs::File::open(source_path)
                    .with_context(|| format!("Failed to open file: {:?}", source_path))?;
                writer
                    .push_archive_entry(entry, Some(file))
                    .with_context(|| format!("Failed to push archive entry: {:?}", source_path))?;
            } else if source_path.is_dir() {
                // 目录：先添加目录条目本身，再递归添加其内容
                let dir_entry = SevenZArchiveEntry::from_path(source_path, base_name.clone());
                writer
                    .push_archive_entry::<fs::File>(dir_entry, None)
                    .with_context(|| {
                        format!("Failed to push directory entry: {:?}", source_path)
                    })?;

                // 使用 DirWalker 递归遍历目录，跳过 depth=0 的根目录自身
                let dw_entries = DirWalker::new()
                    .follow_symlinks(false)
                    .include_dirs(true)
                    .include_files(true)
                    .skip_hidden(false)
                    .walk_bfs(source_path);

                for dw_entry in dw_entries {
                    if dw_entry.depth == 0 {
                        continue;
                    }
                    if dw_entry.file_type == FileKind::Symlink {
                        continue;
                    }
                    let relative = dw_entry
                        .path
                        .strip_prefix(source_path)
                        .with_context(|| "Failed to compute relative path")?;
                    let entry_name = format!(
                        "{}/{}",
                        base_name,
                        relative.to_string_lossy().replace('\\', "/")
                    );

                    let archive_entry = SevenZArchiveEntry::from_path(&dw_entry.path, entry_name);
                    let reader = if dw_entry.file_type == FileKind::File {
                        Some(fs::File::open(&dw_entry.path).with_context(|| {
                            format!("Failed to open file: {:?}", dw_entry.path)
                        })?)
                    } else {
                        None
                    };
                    writer
                        .push_archive_entry(archive_entry, reader)
                        .with_context(|| {
                            format!("Failed to push archive entry: {:?}", dw_entry.path)
                        })?;
                }
            }
        }

        writer
            .finish()
            .with_context(|| format!("Failed to finish 7z archive: {:?}", dest_path))?;

        Ok(true)
    }
}

impl Default for ModManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_mod_data_serialization() {
        let mod_data = ModData {
            mod_path: "/test/mod".to_string(),
            icon_path: Some("/test/mod/icon.png".to_string()),
            mod_name: "Test Mod".to_string(),
            real_index: 1,
            is_old_auto_fixed: false,
            is_syntax_error_removed: false,
            is_unoptimized: false,
            is_namespaced: false,
            is_disabled: false,
            favorite_date_time: None,
        };

        let json = serde_json::to_string(&mod_data).unwrap();
        assert!(json.contains("modPath"));
        assert!(json.contains("iconPath"));
    }

    #[test]
    fn test_update_mod_data_result_serialization() {
        let result = UpdateModDataResult {
            success: true,
            logs: vec![LogEntry::info("Test")],
            duration_ms: 100,
            error_report: None,
            hash_conflict_report: None,
            per_mod_errors: Vec::new(),
            group_summaries: Vec::new(),
            total_mods_processed: 1,
            total_errors: 0,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("success"));
        assert!(json.contains("durationMs"));
    }

    #[test]
    fn test_log_entry_levels() {
        let info = LogEntry::info("info");
        assert_eq!(info.level, "info");

        let warn = LogEntry::warn("warn");
        assert_eq!(warn.level, "warn");

        let error = LogEntry::error("error");
        assert_eq!(error.level, "error");

        let success = LogEntry::success("success");
        assert_eq!(success.level, "success");
    }

    /// 辅助函数：构造测试用 ModData。
    fn make_test_mod(mod_path: &str, is_disabled: bool) -> ModData {
        ModData {
            mod_path: mod_path.to_string(),
            icon_path: None,
            mod_name: mod_path.to_string(),
            real_index: 1,
            is_old_auto_fixed: false,
            is_syntax_error_removed: false,
            is_unoptimized: false,
            is_namespaced: false,
            is_disabled,
            favorite_date_time: None,
        }
    }

    #[test]
    fn test_get_enabled_mod_index_in_group_empty() {
        // 空数组应返回 0
        let mods: Vec<ModData> = Vec::new();
        let result = ModManager::get_enabled_mod_index_in_group(&mods);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_get_enabled_mod_index_in_group_none_only() {
        // 仅 None 槽位时应返回 0
        let mods = vec![make_test_mod("None", false)];
        let result = ModManager::get_enabled_mod_index_in_group(&mods);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_get_enabled_mod_index_in_group_all_disabled() {
        // 所有模组都禁用时应返回 0
        let mods = vec![
            make_test_mod("None", false),
            make_test_mod("/path/mod1", true),
            make_test_mod("/path/mod2", true),
        ];
        let result = ModManager::get_enabled_mod_index_in_group(&mods);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_get_enabled_mod_index_in_group_with_enabled() {
        // 存在启用模组时应返回其索引（包含 None 槽位）
        let mods = vec![
            make_test_mod("None", false),
            make_test_mod("/path/mod1", true), // 禁用
            make_test_mod("/path/mod2", false), // 启用 - 应返回索引 2
            make_test_mod("/path/mod3", false), // 启用 - 不应被选择
        ];
        let result = ModManager::get_enabled_mod_index_in_group(&mods);
        assert_eq!(result, 2);
    }

    #[test]
    fn test_get_enabled_mod_index_in_group_first_mod_enabled() {
        // 第一个非 None 模组启用时应返回索引 1
        let mods = vec![
            make_test_mod("None", false),
            make_test_mod("/path/mod1", false), // 启用 - 应返回索引 1
        ];
        let result = ModManager::get_enabled_mod_index_in_group(&mods);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_real_index_from_directory_listing_order() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let group_path = dir.path();

        // Create mod directories where sort order differs from raw directory listing order.
        // "zmod" is enabled; "DISABLED_amod" is disabled.
        // Raw alphabetical order (NTFS): "DISABLED_amod" < "zmod"
        // Sort order (enabled-first): "zmod" < "DISABLED_amod"
        fs::create_dir_all(group_path.join("zmod")).unwrap();
        fs::create_dir_all(group_path.join("DISABLED_amod")).unwrap();

        // Get raw directory listing order (matching what fs::read_dir returns)
        let raw_order: Vec<String> = fs::read_dir(group_path)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();

        // Verify test setup: raw order must differ from sort order
        let mut sorted_order = raw_order.clone();
        sorted_order.sort_by(|a, b| {
            let a_disabled = ModManager::is_disabled_name(a);
            let b_disabled = ModManager::is_disabled_name(b);
            match (a_disabled, b_disabled) {
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => a.to_lowercase().cmp(&b.to_lowercase()),
            }
        });
        assert_ne!(
            raw_order, sorted_order,
            "Test setup error: raw directory order must differ from sort order"
        );

        let mods = ModManager::get_mods_on_group(group_path.to_str().unwrap()).unwrap();

        // mods[0] is None with real_index=0
        assert_eq!(mods[0].mod_path, "None");
        assert_eq!(mods[0].real_index, 0);

        // Each non-None mod's real_index must match its position in raw directory listing (1-indexed)
        for mod_data in mods.iter().skip(1) {
            let dir_name = Path::new(&mod_data.mod_path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
            let raw_position = raw_order.iter().position(|n| *n == dir_name).unwrap() + 1;
            assert_eq!(
                mod_data.real_index, raw_position as i32,
                "Mod '{}' has real_index={} but expected {} from raw directory listing order",
                dir_name, mod_data.real_index, raw_position
            );
        }
    }

    #[test]
    fn test_none_slot_always_index_zero() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("amod")).unwrap();

        let mods = ModManager::get_mods_on_group(dir.path().to_str().unwrap()).unwrap();

        assert_eq!(mods.len(), 2);
        assert_eq!(mods[0].mod_path, "None");
        assert_eq!(mods[0].real_index, 0);
    }

    #[test]
    fn test_empty_group_returns_only_none() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();

        let mods = ModManager::get_mods_on_group(dir.path().to_str().unwrap()).unwrap();

        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].mod_path, "None");
        assert_eq!(mods[0].real_index, 0);
    }

    #[test]
    fn test_mods_returned_in_sorted_order() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let group_path = dir.path();

        fs::create_dir_all(group_path.join("zmod")).unwrap();
        fs::create_dir_all(group_path.join("amod")).unwrap();
        fs::create_dir_all(group_path.join("DISABLED_bmod")).unwrap();

        let mods = ModManager::get_mods_on_group(group_path.to_str().unwrap()).unwrap();

        assert_eq!(mods[0].mod_path, "None");
        assert!(mods[1].mod_path.ends_with("amod"));
        assert!(!mods[1].is_disabled);
        assert!(mods[2].mod_path.ends_with("zmod"));
        assert!(!mods[2].is_disabled);
        assert!(mods[3].mod_path.ends_with("DISABLED_bmod"));
        assert!(mods[3].is_disabled);
    }

    #[test]
    fn test_is_disabled_name_case_insensitive() {
        assert!(ModManager::is_disabled_name("DISABLED_mod"));
        assert!(ModManager::is_disabled_name("disabled_mod"));
        assert!(ModManager::is_disabled_name("Disabled_mod"));
        assert!(ModManager::is_disabled_name("DISABLEDmod"));
        assert!(!ModManager::is_disabled_name("mod"));
        assert!(!ModManager::is_disabled_name("DIS"));
        assert!(!ModManager::is_disabled_name(""));
    }

    #[test]
    fn test_is_disabled_name_does_not_panic_on_multibyte() {
        // 中文字符为 multi-byte UTF-8，直接按字节切片会导致 panic；
        // 此处验证使用 char boundary 安全的 get() 后不会崩溃。
        assert!(!ModManager::is_disabled_name("和"));
        assert!(!ModManager::is_disabled_name("中文模组"));
        assert!(!ModManager::is_disabled_name("DISABLE和"));
        assert!(ModManager::is_disabled_name("DISABLED_中文"));
    }

    #[test]
    fn test_lowercase_disabled_prefix_recognized() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let group_path = dir.path();

        fs::create_dir_all(group_path.join("amod")).unwrap();
        fs::create_dir_all(group_path.join("disabled_bmod")).unwrap();

        let mods = ModManager::get_mods_on_group(group_path.to_str().unwrap()).unwrap();

        assert_eq!(mods[0].mod_path, "None");
        assert!(mods[1].mod_path.ends_with("amod"));
        assert!(!mods[1].is_disabled);
        assert!(mods[2].mod_path.ends_with("disabled_bmod"));
        assert!(mods[2].is_disabled);
    }

    #[test]
    fn test_is_favorite_reads_nrmm_fav_file() {
        use std::fs;
        use tempfile::tempdir;
        use time::OffsetDateTime;

        let temp = tempdir().unwrap();
        let fav_path = temp.path().join("fav");
        fs::write(&fav_path, "").unwrap();
        let expected_time = fav_path.metadata().unwrap().modified().unwrap();

        let result = ModManager::is_favorite(temp.path().to_str().unwrap()).unwrap();
        assert!(result.is_some());

        let parsed = OffsetDateTime::parse(
            &result.unwrap(),
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let expected = OffsetDateTime::from(expected_time);
        // 允许 1 秒内误差
        assert!((parsed.unix_timestamp() - expected.unix_timestamp()).abs() <= 1);
    }

    #[test]
    fn test_is_favorite_prefers_dot_favorite_over_fav() {
        use std::fs;
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        fs::write(temp.path().join(".favorite"), "2024-01-15T10:30:00Z").unwrap();
        fs::write(temp.path().join("fav"), "").unwrap();

        let result = ModManager::is_favorite(temp.path().to_str().unwrap()).unwrap();
        assert_eq!(result, Some("2024-01-15T10:30:00Z".to_string()));
    }

    #[test]
    fn disable_tree_node_mod_adds_prefix() {
        let tmp = TempDir::new().unwrap();
        let mod_dir = tmp.path().join("MyMod");
        fs::create_dir(&mod_dir).unwrap();
        let result = ModManager::disable_tree_node_mod(mod_dir.to_str().unwrap()).unwrap();
        assert!(result.contains("DISABLED"));
        assert!(tmp.path().join("DISABLEDMyMod").exists());
    }

    #[test]
    fn disable_tree_node_mod_ignores_already_disabled() {
        let tmp = TempDir::new().unwrap();
        let mod_dir = tmp.path().join("DISABLEDMyMod");
        fs::create_dir(&mod_dir).unwrap();
        let result = ModManager::disable_tree_node_mod(mod_dir.to_str().unwrap()).unwrap();
        assert_eq!(result, mod_dir.to_string_lossy().to_string());
    }

    // ==================== natural_cmp 自然排序测试 ====================

    #[test]
    fn test_natural_cmp_basic() {
        assert_eq!(ModManager::natural_cmp("a", "b"), std::cmp::Ordering::Less);
        assert_eq!(ModManager::natural_cmp("b", "a"), std::cmp::Ordering::Greater);
        assert_eq!(ModManager::natural_cmp("a", "a"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_natural_cmp_numeric() {
        // 数字比较：2 < 10 （自然排序）
        assert_eq!(ModManager::natural_cmp("mod2", "mod10"), std::cmp::Ordering::Less);
    }

    #[test]
    fn test_natural_cmp_numeric_leading() {
        assert_eq!(ModManager::natural_cmp("2mod", "10mod"), std::cmp::Ordering::Less);
        assert_eq!(ModManager::natural_cmp("10mod", "2mod"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_natural_cmp_pure_numbers() {
        assert_eq!(ModManager::natural_cmp("123", "456"), std::cmp::Ordering::Less);
        assert_eq!(ModManager::natural_cmp("456", "123"), std::cmp::Ordering::Greater);
        assert_eq!(ModManager::natural_cmp("123", "123"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_natural_cmp_mixed_text_and_numbers() {
        assert_eq!(ModManager::natural_cmp("abc123def", "abc456def"), std::cmp::Ordering::Less);
        assert_eq!(ModManager::natural_cmp("abc10def", "abc2def"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_natural_cmp_different_lengths() {
        // 短字符串排在长字符串前面
        assert_eq!(ModManager::natural_cmp("mod", "mod1"), std::cmp::Ordering::Less);
        assert_eq!(ModManager::natural_cmp("mod1", "mod"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_natural_cmp_empty_string() {
        assert_eq!(ModManager::natural_cmp("", "a"), std::cmp::Ordering::Less);
        assert_eq!(ModManager::natural_cmp("a", ""), std::cmp::Ordering::Greater);
        assert_eq!(ModManager::natural_cmp("", ""), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_natural_cmp_case_sensitive() {
        // 大写字母排在小写字母前面（ASCII 排序）
        assert_eq!(ModManager::natural_cmp("A", "a"), std::cmp::Ordering::Less);
        assert_eq!(ModManager::natural_cmp("a", "A"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_natural_cmp_large_numbers() {
        // 大数字不溢出
        assert_eq!(
            ModManager::natural_cmp("a99999999999999999999", "a100000000000000000000"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_natural_cmp_digit_vs_non_digit() {
        // 数字 < 非数字
        assert_eq!(ModManager::natural_cmp("1a", "a1"), std::cmp::Ordering::Less);
    }

    // ==================== sanitize 辅助函数测试 ====================

    #[test]
    fn test_sanitize_condition_line_removes_managed_slot() {
        // 传入包含 managed_slot_id 的 condition 行，验证管理表达式被移除
        let result = ModManager::sanitize_condition_line(
            r"condition = $active == 1 && $managed_slot_id == $\modmanageragl\group_1\active_slot",
        );
        assert!(result.is_some());
        let cleaned = result.unwrap();
        assert!(!cleaned.contains("managed_slot_id"));
        assert!(cleaned.contains("$active == 1"));
    }

    #[test]
    fn test_sanitize_condition_line_no_condition() {
        // 非 condition 行返回 None
        let result = ModManager::sanitize_condition_line("if $var == 1");
        assert!(result.is_none());
    }

    #[test]
    fn test_sanitize_condition_line_empty() {
        let result = ModManager::sanitize_condition_line("");
        assert!(result.is_none());
    }

    #[test]
    fn test_sanitize_key_condition_expression_removes_managed_slot() {
        let result = ModManager::sanitize_key_condition_expression(
            r"$active == 1 && $managed_slot_id == $\modmanageragl\group_1\active_slot",
        );
        assert!(!result.contains("managed_slot_id"));
        assert!(result.contains("$active == 1"));
    }

    #[test]
    fn test_sanitize_key_condition_expression_no_managed() {
        let result = ModManager::sanitize_key_condition_expression("$var == 1");
        assert_eq!(result, "$var == 1");
    }

    #[test]
    fn test_sanitize_key_condition_expression_empty() {
        let result = ModManager::sanitize_key_condition_expression("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_remove_first_four_spaces_basic() {
        assert_eq!(ModManager::remove_first_four_spaces("    hello"), "hello");
        assert_eq!(ModManager::remove_first_four_spaces("  hello"), "  hello");
        assert_eq!(ModManager::remove_first_four_spaces("hello"), "hello");
    }

    #[test]
    fn test_remove_first_four_spaces_tabs() {
        // 制表符不受影响
        assert_eq!(ModManager::remove_first_four_spaces("\thello"), "\thello");
    }

    #[test]
    fn test_remove_first_four_spaces_empty() {
        assert_eq!(ModManager::remove_first_four_spaces(""), "");
        assert_eq!(ModManager::remove_first_four_spaces("    "), "");
    }

    #[test]
    fn test_remove_first_four_spaces_mixed() {
        assert_eq!(ModManager::remove_first_four_spaces("    hello world"), "hello world");
        assert_eq!(ModManager::remove_first_four_spaces("   hello world"), "   hello world");
    }

    // ==================== is_disabled_name 边界测试 ====================

    #[test]
    fn test_is_disabled_name_short_string() {
        // 短于 DISABLED 前缀长度（8 字符）的字符串
        assert!(!ModManager::is_disabled_name("D"));
        assert!(!ModManager::is_disabled_name("DIS"));
        assert!(!ModManager::is_disabled_name("DISABL"));
        // "DISABLED" 本身是前缀，应返回 true
        assert!(ModManager::is_disabled_name("DISABLED"));
    }

    #[test]
    fn test_is_disabled_name_with_underscore() {
        // "DISABLED_" 是纯粹的前缀，不检查后续字符
        assert!(ModManager::is_disabled_name("DISABLED_"));
        assert!(ModManager::is_disabled_name("DISABLED_anything"));
    }

    // ==================== remove_xxmi_ini_statements 测试 ====================

    #[test]
    fn test_remove_managed_statements_removes_managed_slot() {
        // 构造包含 managed_slot_id 声明的 INI 内容
        let content = "[Constants]\nglobal $managed_slot_id = 0\nkey = value\n";
        let result = ModManager::remove_xxmi_ini_statements(content);
        assert!(!result.contains("managed_slot_id"));
        assert!(result.contains("key = value"));
    }

    #[test]
    fn test_remove_managed_statements_no_marks() {
        let content = "[Constants]\nkey = value\n";
        let result = ModManager::remove_xxmi_ini_statements(content);
        assert_eq!(result.trim(), content.trim());
    }

    #[test]
    fn test_remove_managed_statements_empty() {
        let result = ModManager::remove_xxmi_ini_statements("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_remove_managed_statements_with_if_endif() {
        // 包含 manager if/endif 的块应被移除
        let content = "[Constants]\nif $managed_slot_id == $\\modmanageragl\\group_1\\active_slot\nx = 1\nendif\nkey = value\n";
        let result = ModManager::remove_xxmi_ini_statements(content);
        assert!(!result.contains("managed_slot_id"));
        assert!(!result.contains("modmanageragl"));
        assert!(result.contains("key = value"));
    }

    // ==================== strip_disabled_prefixes_deep 测试 ====================

    #[test]
    fn test_strip_disabled_single_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        fs::write(root.join("DISABLED_test.txt"), "content").unwrap();

        let count = ModManager::strip_disabled_prefixes_deep(root).unwrap();
        assert_eq!(count, 1);
        assert!(root.join("test.txt").exists());
        assert!(!root.join("DISABLED_test.txt").exists());
    }

    #[test]
    fn test_strip_disabled_nested_directories() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let dir1 = root.join("DISABLED_dir1");
        fs::create_dir(&dir1).unwrap();
        let dir2 = dir1.join("DISABLED_dir2");
        fs::create_dir(&dir2).unwrap();
        fs::write(dir2.join("DISABLED_file.txt"), "content").unwrap();

        let count = ModManager::strip_disabled_prefixes_deep(root).unwrap();
        assert_eq!(count, 3);
        assert!(root.join("dir1").exists());
        assert!(root.join("dir1").join("dir2").exists());
        assert!(root.join("dir1").join("dir2").join("file.txt").exists());
    }

    #[test]
    fn test_strip_disabled_exact_disabled_name_skipped() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let disabled_dir = root.join("DISABLED");
        fs::create_dir(&disabled_dir).unwrap();
        fs::write(disabled_dir.join("DISABLED_file.txt"), "content").unwrap();

        let count = ModManager::strip_disabled_prefixes_deep(root).unwrap();
        assert_eq!(count, 0);
        assert!(disabled_dir.exists());
        assert!(disabled_dir.join("DISABLED_file.txt").exists());
    }

    #[test]
    fn test_strip_disabled_depth_limit() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let mut current = root.to_path_buf();
        for i in 0..70 {
            current = current.join(format!("DISABLED_level{}", i));
            fs::create_dir(&current).unwrap();
        }

        let count = ModManager::strip_disabled_prefixes_deep(root).unwrap();
        assert!(count >= 64);
        assert!(count <= 70);
    }

    #[cfg(unix)]
    #[test]
    fn test_strip_disabled_symlink_skipped() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let real_dir = root.join("real_dir");
        fs::create_dir(&real_dir).unwrap();
        fs::write(real_dir.join("DISABLED_file.txt"), "content").unwrap();

        let link_dir = root.join("link_dir");
        std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();

        let count = ModManager::strip_disabled_prefixes_deep(root).unwrap();
        assert_eq!(count, 1);
        assert!(real_dir.join("file.txt").exists());
        assert!(link_dir.exists());
        assert!(link_dir.is_symlink());
    }

    // ==================== get_safe_target 测试 ====================

    #[test]
    fn test_get_safe_target_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nonexistent");
        let result = ModManager::get_safe_target(&target);
        assert_eq!(result, target);
    }

    #[test]
    fn test_get_safe_target_existing_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("test.txt");
        fs::write(&target, "content").unwrap();

        let result = ModManager::get_safe_target(&target);
        assert!(result.ends_with("test_1.txt"));
        assert!(!result.exists());
    }

    #[test]
    fn test_get_safe_target_existing_directory() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("mymod");
        fs::create_dir(&target).unwrap();

        let result = ModManager::get_safe_target(&target);
        assert!(result.ends_with("mymod_1"));
        assert!(!result.exists());
    }

    #[test]
    fn test_get_safe_target_multiple_conflicts() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("mymod")).unwrap();
        fs::create_dir(tmp.path().join("mymod_1")).unwrap();
        fs::create_dir(tmp.path().join("mymod_2")).unwrap();

        let target = tmp.path().join("mymod");
        let result = ModManager::get_safe_target(&target);
        assert!(result.ends_with("mymod_3"));
        assert!(!result.exists());
    }

    #[test]
    fn test_get_safe_target_file_without_extension() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("readme");
        fs::write(&target, "content").unwrap();

        let result = ModManager::get_safe_target(&target);
        assert!(result.ends_with("readme_1"));
        assert!(!result.exists());
    }

    // ==================== dir_contains_managed 测试 ====================

    #[test]
    fn test_dir_contains_managed_root_is_managed() {
        let tmp = TempDir::new().unwrap();
        let managed_dir = tmp.path().join("_MANAGED_");
        fs::create_dir(&managed_dir).unwrap();

        assert!(ModManager::dir_contains_managed(&managed_dir));
    }

    #[test]
    fn test_dir_contains_managed_root_is_managed_case_insensitive() {
        let tmp = TempDir::new().unwrap();
        let managed_dir = tmp.path().join("_managed_");
        fs::create_dir(&managed_dir).unwrap();

        assert!(ModManager::dir_contains_managed(&managed_dir));
    }

    #[test]
    fn test_dir_contains_managed_nested_managed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("mymod");
        fs::create_dir(&root).unwrap();
        let nested = root.join("subdir");
        fs::create_dir(&nested).unwrap();
        fs::create_dir(nested.join("_MANAGED_")).unwrap();

        assert!(ModManager::dir_contains_managed(&root));
    }

    #[test]
    fn test_dir_contains_managed_no_managed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("mymod");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("subdir")).unwrap();

        assert!(!ModManager::dir_contains_managed(&root));
    }

    #[test]
    fn test_dir_contains_managed_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let nonexistent = tmp.path().join("nonexistent");
        assert!(!ModManager::dir_contains_managed(&nonexistent));
    }

    // ==================== validate_drop_path 测试 ====================

    /// 创建一个模拟的 Mods 目录结构，返回 group_path
    fn create_test_mods_structure(tmp: &TempDir) -> PathBuf {
        let mods_root = tmp.path().join("Mods");
        let managed_dir = mods_root.join("_MANAGED_");
        let group_dir = managed_dir.join("group_1");
        fs::create_dir_all(&group_dir).unwrap();
        group_dir
    }

    #[test]
    fn test_validate_drop_path_safe_external_dir() {
        let tmp = TempDir::new().unwrap();
        let group_path = create_test_mods_structure(&tmp);
        let safe_dir = tmp.path().join("external_mod");
        fs::create_dir(&safe_dir).unwrap();

        let result = ModManager::validate_drop_path(&safe_dir, group_path.to_str().unwrap());
        assert!(result.is_ok(), "Expected safe path to pass: {:?}", result.err());
    }

    #[test]
    fn test_validate_drop_path_under_managed() {
        let tmp = TempDir::new().unwrap();
        let group_path = create_test_mods_structure(&tmp);
        let bad_dir = group_path
            .parent()
            .unwrap()
            .join("group_1")
            .join("inside_managed");
        fs::create_dir_all(&bad_dir).unwrap();

        let result = ModManager::validate_drop_path(&bad_dir, group_path.to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("_MANAGED_"));
    }

    #[test]
    fn test_validate_drop_path_contains_managed() {
        let tmp = TempDir::new().unwrap();
        let group_path = create_test_mods_structure(&tmp);
        let bad_dir = tmp.path().join("mod_with_managed");
        fs::create_dir(&bad_dir).unwrap();
        fs::create_dir(bad_dir.join("_MANAGED_")).unwrap();

        let result = ModManager::validate_drop_path(&bad_dir, group_path.to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("_MANAGED_"));
    }

    #[test]
    fn test_validate_drop_path_ancestor_of_mods() {
        let tmp = TempDir::new().unwrap();
        let group_path = create_test_mods_structure(&tmp);
        // tmp.path() 是 Mods 的祖先目录
        let ancestor = tmp.path();
        let result = ModManager::validate_drop_path(ancestor, group_path.to_str().unwrap());
        // 应该被拒绝（要么因为包含 _MANAGED_，要么因为是祖先）
        assert!(result.is_err(), "Ancestor directory should be rejected");
    }

    #[test]
    fn test_validate_drop_path_mods_root_itself() {
        let tmp = TempDir::new().unwrap();
        let group_path = create_test_mods_structure(&tmp);
        let mods_root = tmp.path().join("Mods");
        // Mods 根目录包含 _MANAGED_ 子目录，应该被拒绝
        let result = ModManager::validate_drop_path(&mods_root, group_path.to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("_MANAGED_"));
    }

    #[test]
    fn test_sanitize_extract_path_allows_safe_relative_paths() {
        let base = Path::new("/tmp/extract");
        assert_eq!(
            ModManager::sanitize_extract_path(base, "file.txt").unwrap(),
            PathBuf::from("/tmp/extract/file.txt")
        );
        assert_eq!(
            ModManager::sanitize_extract_path(base, "dir/subdir/file.txt").unwrap(),
            PathBuf::from("/tmp/extract/dir/subdir/file.txt")
        );
        assert_eq!(
            ModManager::sanitize_extract_path(base, "./file.txt").unwrap(),
            PathBuf::from("/tmp/extract/file.txt")
        );
    }

    #[test]
    fn test_sanitize_extract_path_rejects_zip_slip() {
        let base = Path::new("/tmp/extract");
        assert!(ModManager::sanitize_extract_path(base, "../secret.txt").is_err());
        assert!(ModManager::sanitize_extract_path(base, "dir/../../secret.txt").is_err());
        assert!(ModManager::sanitize_extract_path(base, "dir/./../../secret.txt").is_err());
    }

    #[test]
    fn test_sanitize_extract_path_rejects_absolute_paths() {
        let base = Path::new("/tmp/extract");
        assert!(ModManager::sanitize_extract_path(base, "/etc/passwd").is_err());
        assert!(ModManager::sanitize_extract_path(base, "\\Windows\\System32").is_err());
    }

    #[test]
    fn test_move_to_trash_removes_original_file() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("to_trash.txt");
        fs::write(&file_path, "trash me").unwrap();
        assert!(file_path.exists());

        ModManager::move_to_trash(&file_path).unwrap();

        assert!(!file_path.exists(), "Original file should be moved to trash");
    }

    /// 验证 extract_archive 能拒绝包含路径穿越的恶意 ZIP 文件（Zip Slip）。
    #[test]
    fn test_extract_archive_rejects_zip_slip() {
        use std::io::Write;

        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("zip_slip.zip");
        let dest_dir = tmp.path().join("extracted");

        let file = fs::File::create(&archive_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("../evil.txt", options).unwrap();
        zip.write_all(b"malicious").unwrap();
        zip.finish().unwrap();

        let result = ModManager::extract_archive(&archive_path, &dest_dir, None);
        assert!(result.is_err(), "Zip Slip attack should be rejected");
    }

    /// 验证 smart_flatten_archive_root 对各种目录结构的处理。
    #[test]
    fn test_smart_flatten_archive_root_scenarios() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        // 空目录：不应扁平化
        let empty = base.join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(!ModManager::smart_flatten_archive_root(&empty, "archive").unwrap());

        // 顶层包含文件：不应扁平化
        let with_file = base.join("with_file");
        fs::create_dir_all(&with_file).unwrap();
        fs::write(with_file.join("readme.txt"), "hello").unwrap();
        assert!(!ModManager::smart_flatten_archive_root(&with_file, "archive").unwrap());

        // 顶层包含多个目录：不应扁平化
        let multi_dir = base.join("multi_dir");
        fs::create_dir_all(&multi_dir.join("a")).unwrap();
        fs::create_dir_all(&multi_dir.join("b")).unwrap();
        assert!(!ModManager::smart_flatten_archive_root(&multi_dir, "archive").unwrap());

        // 顶层单个目录但名称不匹配：不应扁平化
        let mismatch = base.join("mismatch");
        let mismatch_inner = mismatch.join("wrong_name");
        fs::create_dir_all(&mismatch_inner).unwrap();
        fs::write(mismatch_inner.join("file.txt"), "x").unwrap();
        assert!(!ModManager::smart_flatten_archive_root(&mismatch, "archive").unwrap());

        // 顶层单个目录且名称与压缩包 stem 匹配：应提升内部文件并删除空目录
        let matched = base.join("matched");
        let matched_inner = matched.join("MyArchive");
        fs::create_dir_all(&matched_inner).unwrap();
        fs::write(matched_inner.join("file.txt"), "content").unwrap();
        assert!(ModManager::smart_flatten_archive_root(&matched, "MyArchive").unwrap());
        assert!(matched.join("file.txt").exists());
        assert!(!matched_inner.exists());
    }
}
