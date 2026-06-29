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

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::{info, warn};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use walkdir::WalkDir;

use crate::ini_handler::error_detection::ErroredLinesReport;
use crate::ini_handler::{
    get_section_type, is_comment_line, is_section_header, parse_section_name, SectionType,
};
use crate::process::TargetGame;
use crate::settings::Settings;

/// 禁用模组目录名前缀。被禁用的模组目录会被重命名为 `DISABLED<原名>`，
/// 3DMigoto 框架会忽略此前缀开头的目录，从而实现「不删除文件即可禁用」的效果。
const DISABLED_PREFIX: &str = "DISABLED";

/// 收藏标记文件名。在模组/分组目录下存在该文件即表示已被收藏，
/// 文件内容为收藏时的时间戳字符串（用于排序）。
const FAVORITE_FILE: &str = ".favorite";

/// 支持的图标文件扩展名列表。扫描图标时会按此列表的顺序优先匹配 `icon.<ext>`，
/// 找不到时再回退到目录中任意一个匹配扩展名的文件。
const ICON_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "ico", "webp", "bmp"];

/// 管理文件夹名称。所有由 NRMM 自动生成的分组 INI 及管理性内容均存放于此。
const MANAGED_FOLDER: &str = "_MANAGED_";

/// 分组显示名称持久化文件名。文件内容为该分组的可读名称（独立于 `group_<index>` 目录名）。
const GROUP_NAME_FILE: &str = "groupname";

/// 模组显示名称持久化文件名。文件内容为该模组的可读名称（独立于目录名）。
const MOD_NAME_FILE: &str = "modname";

/// 选中索引持久化文件名。分组目录下保存当前选中的模组索引，
/// `_MANAGED_` 目录下保存当前选中的分组索引。
const SELECTED_INDEX_FILE: &str = "selectedindex";

/// 被 NRMM 管理修改的 INI 文件的原始备份扩展名。
/// 第一次修改 INI 时会生成 `<ini>.baknrmm` 备份，便于回滚。
const MANAGED_BACKUP_EXT: &str = "baknrmm";

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

/// Hash 冲突检测报告。
///
/// 由 `update_mod_data` 流程生成，包含启用模组的内容 hash 冲突信息，
/// 用于在前端提示用户可能存在重复的模组。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HashConflictReport {
    /// 启用的 mod hash 冲突：hash -> 具有相同 hash 的模组列表
    pub enabled_mod_hashes: HashMap<String, Vec<HashedModInfo>>,
    /// 命名空间 hash：namespace hash -> 文件路径列表
    pub namespace_hashes: HashMap<String, Vec<String>>,
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
            return ModsPathStatus::InvalidNotExist;
        }

        let folder_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if folder_name != "Mods" {
            return ModsPathStatus::InvalidNotModsFolder;
        }

        let parent = match path.parent() {
            Some(p) => p,
            None => return ModsPathStatus::InvalidNotModsFolder,
        };

        let d3dx_path = parent.join("d3dx.ini");
        if !d3dx_path.exists() {
            return ModsPathStatus::InvalidMissingD3dx;
        }

        let dll_path = parent.join("d3d11.dll");
        if !dll_path.exists() {
            return ModsPathStatus::InvalidMissingDll;
        }

        let managed_path = path.join(MANAGED_FOLDER);
        if !managed_path.exists() || !managed_path.is_dir() {
            return ModsPathStatus::InvalidWithoutManagedFolder;
        }

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
            let display_name = if folder_name.starts_with(DISABLED_PREFIX) {
                folder_name.trim_start_matches(DISABLED_PREFIX).to_string()
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
            let content = fs::read_to_string(&index_file)
                .with_context(|| format!("Failed to read selected group index: {:?}", index_file))?;
            let index = content.trim().parse::<i32>().unwrap_or(0);
            if index >= 0 && index < group_count as i32 {
                Ok(index)
            } else {
                Ok(0)
            }
        } else {
            fs::write(&index_file, "0")
                .with_context(|| format!("Failed to write selected group index: {:?}", index_file))?;
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
    /// 读取路径下的 `.favorite` 文件：
    /// - 文件存在且内容非空：返回 `Some(内容)`。
    /// - 文件存在但内容为空或读取失败：返回 `Some(当前UTC时间)`（兼容旧数据）。
    /// - 文件不存在：返回 `None`（未收藏）。
    ///
    /// 参数：
    /// - `path`: 模组或分组目录路径。
    pub fn is_favorite(path: &str) -> Result<Option<String>> {
        let path = Path::new(path);
        let fav_path = path.join(FAVORITE_FILE);
        if fav_path.exists() {
            match fs::read_to_string(&fav_path) {
                Ok(content) => {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        Ok(Some(trimmed.to_string()))
                    } else {
                        // 旧版数据可能写入空内容，回退到当前时间
                        Ok(Some(Self::current_datetime_string()))
                    }
                }
                Err(_) => Ok(Some(Self::current_datetime_string())),
            }
        } else {
            Ok(None)
        }
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
            fs::remove_file(&fav_path)
                .with_context(|| format!("Failed to remove favorite file: {:?}", fav_path))?;
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
    pub fn get_icon_path(path: &str) -> Option<String> {
        let dir_path = Path::new(path);
        // 第一阶段：按优先顺序查找 icon.<ext>
        for ext in ICON_EXTENSIONS {
            let icon_name = format!("icon.{}", ext);
            let icon_path = dir_path.join(&icon_name);
            if icon_path.exists() {
                return Some(icon_path.to_string_lossy().to_string());
            }
        }

        // 第二阶段：回退到目录中任意匹配扩展名的文件
        if let Ok(entries) = fs::read_dir(dir_path) {
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

        fs::copy(source_path, &target_path)
            .with_context(|| format!("Failed to copy icon: {:?} -> {:?}", source_path, target_path))?;

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
                fs::remove_file(&icon_path)
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

        let is_disabled = dir_name.starts_with(DISABLED_PREFIX);
        // 计算新目录名：禁用 → 启用（移除前缀），启用 → 禁用（添加前缀）
        let new_name = if is_disabled {
            dir_name.trim_start_matches(DISABLED_PREFIX).to_string()
        } else {
            format!("{}{}", DISABLED_PREFIX, dir_name)
        };

        let new_path = parent.join(&new_name);
        if new_path.exists() {
            anyhow::bail!("Destination path already exists: {:?}", new_path);
        }

        fs::rename(path, &new_path)
            .with_context(|| format!("Failed to rename mod: {:?} -> {:?}", path, new_path))?;

        // 返回操作后的禁用状态（与原状态相反）
        Ok(!is_disabled)
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
        mods_path: &str,
        sort_method: SortGroupMethod,
    ) -> Result<Vec<ModGroupData>> {
        let mods_path = Path::new(mods_path);
        if !mods_path.exists() || !mods_path.is_dir() {
            return Ok(Vec::new());
        }

        let managed_path = mods_path.join(MANAGED_FOLDER);
        if !managed_path.exists() || !managed_path.is_dir() {
            return Ok(Vec::new());
        }

        let mut groups: Vec<ModGroupData> = Vec::new();
        // 用于递归/普通目录的递增索引（group_ 形式有自己的索引）
        let mut index: i32 = 1;

        if let Ok(entries) = fs::read_dir(&managed_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                    if name.starts_with("group_") {
                        // 情况 1：标准 group_<index> 目录
                        if let Some(idx_str) = name.strip_prefix("group_") {
                            if let Ok(idx) = idx_str.parse::<i32>() {
                                if let Ok(group) = Self::scan_single_group(&path, idx) {
                                    groups.push(group);
                                }
                            }
                        }
                    } else if name.starts_with('#') {
                        // 情况 2：# 开头目录，递归收集所有 # 子目录
                        let recursive_paths = Self::scan_recursive_with_queue(&path);
                        for rec_path in recursive_paths {
                            let group_name = rec_path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();

                            if let Ok(group) = Self::scan_single_group_by_path(&rec_path, index, &group_name) {
                                groups.push(group);
                                index += 1;
                            }
                        }
                    } else if !name.starts_with('.') && Self::is_mod_directory(&path) {
                        // 情况 3：普通模组目录（跳过隐藏目录 .xxx）
                        let group_name = name.to_string();
                        if let Ok(group) = Self::scan_single_group_by_path(&path, index, &group_name) {
                            groups.push(group);
                            index += 1;
                        }
                    }
                }
            }
        }

        // 排序：收藏优先，其次按指定方式
        Self::sort_groups(&mut groups, sort_method);
        info!("Scanned {} groups from {:?}", groups.len(), managed_path);
        Ok(groups)
    }

    /// 使用队列（BFS）递归收集所有以 `#` 开头的子目录。
    ///
    /// 采用迭代式广度优先搜索而非递归，以避免深层嵌套目录导致的栈溢出。
    /// 起始路径本身也会被包含在结果中。
    ///
    /// 参数：
    /// - `base_path`: 起始目录路径（应为 `#` 开头目录）。
    ///
    /// 返回：所有 `#` 开头子目录的路径列表（包含 `base_path` 自身）。
    fn scan_recursive_with_queue(base_path: &Path) -> Vec<PathBuf> {
        let mut result = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(base_path.to_path_buf());

        while let Some(current) = queue.pop_front() {
            if let Ok(entries) = fs::read_dir(&current) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if name.starts_with('#') {
                            // 同时入队继续搜索并加入结果列表
                            queue.push_back(path.clone());
                            result.push(path);
                        }
                    }
                }
            }
        }
        result
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

    /// 通过显式路径与名称扫描单个分组（用于 `#` 开头目录和普通目录）。
    ///
    /// 与 `scan_single_group` 的区别：本函数直接使用传入的 `group_name`，
    /// 而非从 `groupname` 文件读取。
    ///
    /// 参数：
    /// - `group_path`: 分组目录路径。
    /// - `real_index`: 分组的真实索引。
    /// - `group_name`: 分组显示名称。
    fn scan_single_group_by_path(group_path: &Path, real_index: i32, group_name: &str) -> Result<ModGroupData> {
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
                        dir_entries.push(path);
                    }
                }
                Err(e) => warn!("Failed to read entry: {}", e),
            }
        }

        // 多级排序：禁用状态 → 收藏状态 → 名称字母序
        dir_entries.sort_by(|a, b| {
            let a_name = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let b_name = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let a_disabled = a_name.starts_with(DISABLED_PREFIX);
            let b_disabled = b_name.starts_with(DISABLED_PREFIX);
            match (a_disabled, b_disabled) {
                // 启用 < 禁用
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => {
                    let a_fav = a.join(FAVORITE_FILE).exists();
                    let b_fav = b.join(FAVORITE_FILE).exists();
                    match (a_fav, b_fav) {
                        // 收藏 < 未收藏
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        _ => a_name.to_lowercase().cmp(&b_name.to_lowercase()),
                    }
                }
            }
        });

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

        // 遍历排序后的子目录，构建 ModData
        for dir_path in dir_entries {
            let dir_name = dir_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            // 跳过隐藏目录（以 . 开头）
            if dir_name.starts_with('.') {
                continue;
            }

            let is_disabled = dir_name.starts_with(DISABLED_PREFIX);
            let dir_path_str = dir_path.to_string_lossy().to_string();
            let mod_name = Self::get_mod_name(&dir_path_str).unwrap_or_else(|_| {
                // 读取 modname 失败时回退到目录名（去除禁用前缀）
                if is_disabled {
                    dir_name.trim_start_matches(DISABLED_PREFIX).to_string()
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

            let mod_data = ModData {
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
            };

            mods.push(mod_data);
            real_index += 1;
        }

        Ok(mods)
    }

    /// 对分组列表进行排序。
    ///
    /// 排序规则：
    /// 1. 收藏的分组始终排在未收藏的分组之前。
    /// 2. 同等收藏状态下，按 `sort_method` 指定的方式排序：
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

            match sort_method {
                SortGroupMethod::ByIndex => a.real_index.cmp(&b.real_index),
                SortGroupMethod::ByName => a
                    .group_name
                    .to_lowercase()
                    .cmp(&b.group_name.to_lowercase()),
            }
        });
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
        // 确保 _MANAGED_ 目录存在
        fs::create_dir_all(&managed_path)
            .with_context(|| format!("Failed to create managed folder: {:?}", managed_path))?;

        // 寻找最小可用索引
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

        // 仅在名称非空时写入 groupname 文件
        if !group_name.is_empty() {
            let name_file = group_path.join(GROUP_NAME_FILE);
            let _ = fs::write(&name_file, group_name);
        }

        Ok(new_index)
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

        // 取 Mods 根目录（分组的父目录的父目录）
        let mods_path = match path.parent() {
            Some(p) => p,
            None => anyhow::bail!("Invalid group path: no parent directory"),
        };

        let managed_path = mods_path.join(MANAGED_FOLDER);
        fs::create_dir_all(&managed_path)
            .with_context(|| format!("Failed to create managed folder: {:?}", managed_path))?;

        let group_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("group");

        // 生成时间戳后缀，避免多次移除同名分组时冲突
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let dest_name = format!("{}_removed_{}", group_name, timestamp);
        let dest_path = managed_path.join(&dest_name);

        fs::rename(path, &dest_path)
            .with_context(|| format!("Failed to move group to _MANAGED_: {:?} -> {:?}", path, dest_path))?;

        info!("Group removed to _MANAGED_: {:?}", dest_path);
        Ok(())
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
        let target_game = settings.target_game;
        let mods_path = Self::get_mods_path_for_game(settings, target_game);

        if mods_path.is_empty() {
            warn!("No mods path configured for game: {:?}", target_game);
            return Ok(Vec::new());
        }

        let status = Self::validate_mods_path(&mods_path);
        if status != ModsPathStatus::Valid {
            warn!("Mods path is not valid: {:?}, status: {:?}", mods_path, status);
            return Ok(Vec::new());
        }

        let managed_path = Path::new(&mods_path).join(MANAGED_FOLDER);
        let managed_path_str = managed_path.to_string_lossy().to_string();

        let sort_method = SortGroupMethod::from_i32(settings.sort_group_method);

        // 在阻塞线程中执行文件系统扫描，避免阻塞 Tokio 运行时
        let groups = tokio::task::spawn_blocking(move || {
            Self::scan_groups(&managed_path_str, sort_method)
        })
        .await
        .with_context(|| "Failed to spawn blocking task for scanning groups")??;

        info!("Loaded {} groups", groups.len());
        Ok(groups)
    }

    /// 刷新 Mods 数据（与 `load_mods` 等价，语义上表示强制重新加载）。
    ///
    /// 参数：
    /// - `settings`: 全局设置。
    pub async fn refresh_mods(&self, settings: &Settings) -> Result<Vec<ModGroupData>> {
        self.load_mods(settings).await
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

        let (result_logs, error_report, result_success, hash_conflict_report) = match result {
            Ok((logs, report, hash_report)) => (logs, Some(report), true, Some(hash_report)),
            Err(e) => {
                success = false;
                let mut error_logs = vec![LogEntry::error(format!(
                    "Update Mod Data failed: {}",
                    e
                ))];
                error_logs.push(LogEntry::error(
                    "Please check the logs above for more details".to_string(),
                ));
                (error_logs, None, false, None)
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
    ) -> Result<(Vec<LogEntry>, ErroredLinesReport, HashConflictReport)> {
        let mut logs: Vec<LogEntry> = Vec::new();

        logs.push(LogEntry::info("Starting Update Mod Data..."));

        let mods_path = Path::new(mods_path);
        if !mods_path.exists() || !mods_path.is_dir() {
            anyhow::bail!("Mods path does not exist: {:?}", mods_path);
        }

        let managed_path = mods_path.join(MANAGED_FOLDER);
        let managed_path_str = managed_path.to_string_lossy().to_string();

        // 确保 _MANAGED_ 目录存在
        if !managed_path.exists() {
            fs::create_dir_all(&managed_path)
                .with_context(|| format!("Failed to create _MANAGED_ folder: {:?}", managed_path))?;
            logs.push(LogEntry::info("Created _MANAGED_ folder"));
        }

        let group_folders = Self::get_group_folders(&managed_path_str)?;
        logs.push(LogEntry::info(format!(
            "Found {} groups",
            group_folders.len()
        )));

        let known_lib_namespaces: Vec<String> = known_libraries.keys().cloned().collect();

        // 检测所有 INI 语法错误
        let error_report = crate::ini_handler::error_detection::check_all_errors(
            &managed_path_str,
            &known_lib_namespaces,
        )?;

        logs.push(LogEntry::info("Error detection completed"));

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

        // 并行处理每个分组
        group_folders.par_iter().for_each(|(group_path, group_index)| {
            let group_name = format!("group_{}", group_index);

            // 删除分组目录下旧的 INI 文件
            if let Err(e) = Self::delete_group_ini_files(group_path) {
                warn!("Failed to delete group INI files for {}: {}", group_path, e);
            }

            // 创建新的分组 INI 文件（包含 active_slot 变量与切换快捷键）
            if let Err(e) = Self::create_group_ini(group_path, &group_name, *group_index) {
                warn!("Failed to create group INI for {}: {}", group_path, e);
            }

            // 管理分组内的每个模组
            match Self::get_mods_on_group(group_path) {
                Ok(mods) => {
                    mods.par_iter().for_each(|mod_data| {
                        // 跳过 None 槽位和禁用模组
                        if mod_data.mod_path == "None" || mod_data.is_disabled {
                            return;
                        }

                        if let Err(e) = Self::manage_mod(
                            &mod_data.mod_path,
                            &group_name,
                            mod_data.real_index,
                            *group_index,
                        ) {
                            warn!(
                                "Failed to manage mod {}: {}",
                                mod_data.mod_path, e
                            );
                        }
                    });
                }
                Err(e) => {
                    warn!("Failed to get mods for group {}: {}", group_path, e);
                }
            }
        });

        // 在所有分组处理完成后，检测启用的 mod 的 hash 冲突
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
                };
                groups_for_hash_check.push(group_data);
            }
        }

        let (_hash_conflict_count, hash_logs) =
            Self::check_and_report_hash_conflicts(&managed_path_str, &groups_for_hash_check);

        // 计算 hash 冲突报告
        let hash_conflict_report = Self::check_enabled_mod_hash_conflicts(
            &managed_path_str,
            &groups_for_hash_check,
        )
        .unwrap_or_default();

        logs.extend(hash_logs);

        logs.push(LogEntry::info("All groups processed"));

        Ok((logs, error_report, hash_conflict_report))
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
            return Ok(());
        }

        let entries = fs::read_dir(group_path)
            .with_context(|| format!("Failed to read group directory: {:?}", group_path))?;

        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if ext.eq_ignore_ascii_case("ini") {
                            // 删除失败时静默忽略，不影响整体流程
                            let _ = fs::remove_file(&path);
                        }
                    }
                }
            }
        }

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

        Ok(())
    }

    /// 管理单个模组：备份并修改其 INI 文件以支持槽位切换。
    ///
    /// 流程：
    /// 1. 递归查找模组目录下的所有 `.ini` 文件。
    /// 2. 对每个 INI 文件：若不存在 `.baknrmm` 备份则创建备份。
    /// 3. 调用 `modify_ini_file` 注入 `managed_slot_id` 变量与条件块。
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

        let ini_files = Self::find_ini_files_recursive(mod_path);

        for ini_file in &ini_files {
            // 备份路径：<ini>.baknrmm
            let backup_path = format!(
                "{}.{}",
                ini_file.to_string_lossy(),
                MANAGED_BACKUP_EXT
            );
            let backup_path = Path::new(&backup_path);

            // 仅在备份不存在时创建，避免覆盖原始备份
            if !backup_path.exists() {
                fs::copy(ini_file, backup_path).with_context(|| {
                    format!(
                        "Failed to create backup: {:?} -> {:?}",
                        ini_file, backup_path
                    )
                })?;
            }

            if let Err(e) =
                Self::modify_ini_file(ini_file, group_folder_name, mod_index, group_index)
            {
                warn!("Failed to modify INI file {:?}: {}", ini_file, e);
            }
        }

        Ok(())
    }

    /// 递归查找目录下所有 `.ini` 文件。
    ///
    /// 使用 `walkdir` 进行深度优先遍历，返回所有扩展名为 `.ini`（不区分大小写）的文件路径。
    ///
    /// 参数：
    /// - `dir`: 起始目录路径。
    ///
    /// 返回：INI 文件路径列表。目录不存在或非目录时返回空 Vec。
    fn find_ini_files_recursive(dir: &Path) -> Vec<PathBuf> {
        let mut ini_files = Vec::new();

        if !dir.exists() || !dir.is_dir() {
            return ini_files;
        }

        for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext.eq_ignore_ascii_case("ini") {
                        ini_files.push(path.to_path_buf());
                    }
                }
            }
        }

        ini_files
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
        let managed_var_line = format!(
            "global $managed_slot_id = {}",
            mod_index
        );

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

    /// 检测启用的 mod 的 hash 冲突
    /// 返回 HashConflictReport，其中 enabled_mod_hashes 只包含有冲突的 hash
    ///
    /// 流程：
    /// 1. 遍历所有分组中启用的模组（跳过 None 槽位和禁用模组）。
    /// 2. 将每个模组下所有 INI 文件内容合并后计算 hash。
    /// 3. 按 hash 分组，仅保留出现次数 > 1 的 hash（即存在冲突）。
    ///
    /// 参数：
    /// - `_managed_path`: `_MANAGED_` 目录路径（当前未使用）。
    /// - `groups`: 分组数据列表。
    pub fn check_enabled_mod_hash_conflicts(
        _managed_path: &str,
        groups: &[ModGroupData],
    ) -> Result<HashConflictReport> {
        let mut report = HashConflictReport::default();
        let mut content_by_hash: HashMap<String, Vec<HashedModInfo>> = HashMap::new();

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

                // 收集该模组下所有 INI 文件内容
                let mut combined_content = String::new();
                let ini_files = Self::find_ini_files_recursive(Path::new(&mod_data.mod_path));
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

                    content_by_hash
                        .entry(hash)
                        .or_default()
                        .push(info);
                }
            }
        }

        // 找出有冲突的 hash（同一个 hash 有多个 mod）
        for (hash, mods) in content_by_hash {
            if mods.len() > 1 {
                report.enabled_mod_hashes.insert(hash, mods);
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
    ) -> (usize, Vec<LogEntry>) {
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
                        let mod_names: Vec<String> = mods.iter().map(|m| m.mod_name.clone()).collect();
                        logs.push(LogEntry::warn(format!(
                            "Hash conflict: [{}] appears in mods: {} (group: {})",
                            &hash[..8.min(hash.len())],
                            mod_names.join(", "),
                            mods
                                .first()
                                .map(|m| m.group_name.clone())
                                .unwrap_or_default()
                        )));
                    }
                }

                (conflict_count, logs)
            }
            Err(e) => {
                warn!("Failed to check hash conflicts: {}", e);
                (0, vec![LogEntry::warn(format!("Hash conflict check failed: {}", e))])
            }
        }
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
}
