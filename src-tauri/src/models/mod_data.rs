//! 模组相关数据模型
//!
//! 定义模组、分组、INI 数据等核心数据结构

use serde::{Serialize, Deserialize};
use std::path::PathBuf;
use super::enums::*;

/// 按键绑定数据
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct KeybindData {
    pub key: String,
    pub value: String,
    pub section: String,
    pub disabled: bool,
    pub extension: String,
}

/// INI 错误行信息
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ErroredLines {
    /// 行号
    #[serde(default)]
    pub line_number: u32,
    /// 行内容
    #[serde(default)]
    pub line: String,
    /// 错误类型
    pub error_type: u8,
    /// 错误消息
    #[serde(default)]
    pub error_message: String,
    /// 相关行号列表
    #[serde(default)]
    pub line_numbers: Vec<u32>,
}

/// 单个 INI 文件的解析数据
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModIniData {
    /// INI 文件路径
    pub ini_path: String,
    /// INI 文件名
    #[serde(default)]
    pub ini_filename: String,
    /// 相对路径
    #[serde(default)]
    pub file_relative_path: String,
    /// 按键绑定列表
    #[serde(default)]
    pub keybinds: Vec<KeybindData>,
    /// 按键命令列表
    #[serde(default)]
    pub keybind_commands: Vec<KeybindData>,
    /// 常量定义列表
    #[serde(default)]
    pub constants: Vec<KeybindData>,
    /// 覆盖定义列表
    #[serde(default)]
    pub overrides: Vec<KeybindData>,
    /// 命令列表
    #[serde(default)]
    pub command_lists: Vec<KeybindData>,
    /// Present 段列表
    #[serde(default)]
    pub present_sections: Vec<String>,
    /// 总段数
    #[serde(default)]
    pub section_count: u32,
    /// Key 段数
    #[serde(default)]
    pub key_sections: u32,
    /// TextureOverride 段数
    #[serde(default)]
    pub texture_override_sections: u32,
    /// ShaderOverride 段数
    #[serde(default)]
    pub shader_override_sections: u32,
    /// CommandList 段数
    #[serde(default)]
    pub command_list_sections: u32,
    /// Resource 段数
    #[serde(default)]
    pub resource_sections: u32,
    /// 是否包含 include 语句
    #[serde(default)]
    pub has_include: bool,
}

/// 模组数据（单个模组的完整信息）
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModData {
    /// 模组相对路径
    pub mod_path: String,
    /// 模组名称（原始目录名）
    pub mod_name: String,
    /// 主 INI 数据（深度扫描时填充）
    pub mod_ini: Option<ModIniData>,
    /// 所有 INI 文件数据（深度扫描时填充）
    #[serde(default)]
    pub mod_ini_data: Vec<ModIniData>,
    /// 完整路径
    #[serde(default)]
    pub full_path: PathBuf,
    /// 父文件夹路径
    #[serde(default)]
    pub parent_folder: PathBuf,
    /// 预览图片路径
    #[serde(default)]
    pub preview_image_path: Option<PathBuf>,
    /// 是否激活（当前选中）
    pub is_active: bool,
    /// 是否收藏
    pub is_favorite: bool,
    /// 是否使用命名空间
    pub is_namespaced: bool,
    /// 是否有非托管模组崩溃行修复
    #[serde(default)]
    pub has_nonmanaged_mods_crashline_fix: bool,
    /// 错误行列表
    #[serde(default)]
    pub errored_lines: Vec<ErroredLines>,
    /// 预存错误列表
    #[serde(default)]
    pub errored_preexisting: Vec<ErroredLines>,
    /// 缺少 endif 的段
    #[serde(default)]
    pub missing_endif: Vec<String>,
    /// 命名空间列表
    #[serde(default)]
    pub namespaces: Vec<String>,
    /// 命名空间
    #[serde(default)]
    pub namespace: Option<String>,
    /// 已知库列表
    #[serde(default)]
    pub known_libraries: Vec<String>,
    /// 重复库列表（库名 → 定义位置列表）
    #[serde(default)]
    pub duplicate_libraries: Vec<(String, Vec<String>)>,
    /// 不存在的库引用列表
    #[serde(default)]
    pub nonexistent_libraries: Vec<String>,
    /// 是否有命名空间错误
    #[serde(default)]
    pub namespace_error: bool,
    /// 模组是否被禁用（DISABLED 前缀）
    pub mod_disabled: bool,
    /// 是否禁用（兼容字段）
    #[serde(default)]
    pub disabled: bool,
    /// 路径是否过长
    #[serde(default)]
    pub path_too_long: bool,
    /// 模组在分组内的索引
    pub mod_index: u32,
    /// 所属分组索引
    #[serde(default)]
    pub group_index: u32,
    /// Key 段数（汇总）
    #[serde(default)]
    pub key_sections: u32,
    /// TextureOverride 段数（汇总）
    #[serde(default)]
    pub texture_override_sections: u32,
    /// ShaderOverride 段数（汇总）
    #[serde(default)]
    pub shader_override_sections: u32,
    /// CommandList 段数（汇总）
    #[serde(default)]
    pub command_list_sections: u32,
    /// Resource 段数（汇总）
    #[serde(default)]
    pub resource_sections: u32,
    /// 总段数（汇总）
    #[serde(default)]
    pub total_section_count: u32,
    /// 显示名称（去掉 DISABLED 前缀）
    #[serde(default)]
    pub name: String,
    /// 是否为互斥组模组
    #[serde(default)]
    pub is_mutex: bool,
}

impl Default for ModData {
    fn default() -> Self {
        Self {
            mod_path: String::new(),
            mod_name: String::new(),
            mod_ini: None,
            mod_ini_data: Vec::new(),
            full_path: PathBuf::new(),
            parent_folder: PathBuf::new(),
            preview_image_path: None,
            is_active: false,
            is_favorite: false,
            is_namespaced: false,
            has_nonmanaged_mods_crashline_fix: false,
            errored_lines: Vec::new(),
            errored_preexisting: Vec::new(),
            missing_endif: Vec::new(),
            namespaces: Vec::new(),
            namespace: None,
            known_libraries: Vec::new(),
            duplicate_libraries: Vec::new(),
            nonexistent_libraries: Vec::new(),
            namespace_error: false,
            mod_disabled: false,
            disabled: false,
            path_too_long: false,
            mod_index: 0,
            group_index: 0,
            key_sections: 0,
            texture_override_sections: 0,
            shader_override_sections: 0,
            command_list_sections: 0,
            resource_sections: 0,
            total_section_count: 0,
            name: String::new(),
            is_mutex: false,
        }
    }
}

/// 模组分组数据
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModGroupData {
    /// 分组路径
    pub group_path: String,
    /// 分组名称（原始目录名）
    pub group_name: String,
    /// 显示名称
    #[serde(default)]
    pub name: String,
    /// 完整路径
    #[serde(default)]
    pub full_path: PathBuf,
    /// 分组 ID
    pub group_id: u32,
    /// 分组索引
    pub group_index: u32,
    /// 分组内的模组列表
    pub mods: Vec<ModData>,
    /// 模组路径列表
    #[serde(default)]
    pub mod_paths: Vec<PathBuf>,
    /// 模组数量
    pub mod_count: u32,
    /// 是否激活
    pub is_active: bool,
    /// 是否收藏
    pub is_favorite: bool,
    /// 分组是否被禁用
    pub group_disabled: bool,
    /// 分组类型
    pub group_type: GroupType,
    /// 是否有子分组
    pub has_child: bool,
    /// 子分组列表（兼容字段）
    pub children: Vec<ModGroupData>,
    /// 子分组列表
    #[serde(default)]
    pub child_groups: Vec<ModGroupData>,
    /// 当前激活的模组索引（-1 表示无选中）
    pub active_mod_index: i32,
    /// 预览图片路径
    #[serde(default)]
    pub preview_image_path: Option<PathBuf>,
}

impl Default for ModGroupData {
    fn default() -> Self {
        Self {
            group_path: String::new(),
            group_name: String::new(),
            name: String::new(),
            full_path: PathBuf::new(),
            group_id: 0,
            group_index: 0,
            mods: Vec::new(),
            mod_paths: Vec::new(),
            mod_count: 0,
            is_active: false,
            is_favorite: false,
            group_disabled: false,
            group_type: GroupType::NormalGroup,
            has_child: false,
            children: Vec::new(),
            child_groups: Vec::new(),
            active_mod_index: -1,
            preview_image_path: None,
        }
    }
}

impl ModGroupData {
    /// 获取分组索引
    pub fn group_index(&self) -> u32 {
        self.group_index
    }
}

/// 云端链接数据
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CloudLink {
    pub name: String,
    pub url: String,
    pub icon: Option<String>,
}

/// 云端消息数据
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CloudMessage {
    pub title: String,
    pub content: String,
    pub level: String,
    pub date: Option<String>,
}

/// 云端数据集合
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CloudData {
    pub links: Vec<CloudLink>,
    pub messages: Vec<CloudMessage>,
}

/// 按键绑定冲突信息
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct KeybindConflict {
    pub hotkey_id: String,
    pub conflict_with: String,
}
