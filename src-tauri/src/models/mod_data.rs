//! 模组相关数据模型
//!
//! 定义模组、分组、INI 数据等核心数据结构

use super::enums::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

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

/// 按键切换（Keybind Simulation）单个按键条目
///
/// 对应 NRMM `keybinds_tab.dart` 的 `KeyData`：`key = <combo>`（主键）或
/// `back = <combo>`（反向键）。`line_index` 为原始 INI 中的行号，写回时原位定位。
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct KeybindEntry {
    /// true = `key =`，false = `back =`
    pub is_main_key: bool,
    /// combo 原始值（如 `no_modifiers 5`）
    pub value: String,
    /// 原始 INI 中的行号（0-based，写回定位用）
    pub line_index: usize,
}

/// 按键切换：单个 `[KeyXXX]` 段（`XXX` 为段别名，如 Top/Shoulder）
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct KeybindSection {
    /// 去 `Key` 前缀后的段名（如 `Top`）
    pub section: String,
    /// 段头是否带 `;+;` 前缀（NRMM 禁用标记）
    pub disabled: bool,
    /// 段头行号（0-based，写回定位用）
    pub section_line_index: usize,
    /// 段内按键条目（`key =` 与 `back =` 混合，保持原始顺序）
    pub keys: Vec<KeybindEntry>,
}

/// 按键切换：一个 INI 文件的按键段集合
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct KeybindFile {
    /// INI 文件绝对路径
    pub file_path: String,
    /// 该文件的按键段列表
    pub sections: Vec<KeybindSection>,
}

/// 保存按键切换的结果
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SaveKeybindResult {
    /// 是否存在重复段名（大小写不敏感 + namespace 后缀，对齐 NRMM saveKeys）
    pub duplicate_sections: bool,
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
    /// 面向非专业人员的友好提示（由 error_normalizer 规范化，不含技术细节）
    #[serde(default)]
    pub friendly_message: String,
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
    /// 完整路径（前端零消费，跳过序列化以减小 IPC 载荷）
    ///
    /// 存储为 `Arc<str>`：同一模组的 `full_path` 在分组 `mods`、扁平 `mods`、
    /// `mod_paths` 等多处副本间共享同一底层字符串，避免 `PathBuf` 克隆带来
    /// 多余的整串堆分配（内存优化）。
    #[serde(default, skip_serializing)]
    pub full_path: Arc<str>,
    /// 父文件夹路径（前端零消费，同上）
    #[serde(default, skip_serializing)]
    pub parent_folder: Arc<str>,
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
    /// 磁盘状态签名（C-3b 增量复用判定，skip_serializing，前端零感知）
    ///
    /// 三源 mtime 纳秒混合：mod 目录 mtime ^ fav 标记 mtime ^ 所属组
    /// selectedindex mtime。轻量扫描填充；深度扫描产物恒为 0。
    /// 签名相等 = 三源 mtime 均未变 → disabled（目录名承载，路径匹配已排除
    /// 重命名）/ is_favorite（fav 源）/ is_active（组 selectedindex 源）等
    /// 派生字段与磁盘一致 → `subtree_replace` 合并时复用旧 `Arc` 实例
    /// （零重分配）。0 = 无签名（深扫产物或目录不可 stat），永不复用。
    #[serde(skip_serializing, default)]
    pub disk_signature: u64,
}

impl Default for ModData {
    fn default() -> Self {
        Self {
            mod_path: String::new(),
            mod_name: String::new(),
            mod_ini: None,
            mod_ini_data: Vec::new(),
            full_path: Arc::from(""),
            parent_folder: Arc::from(""),
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
            disk_signature: 0,
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
    ///
    /// 元素为 `Arc<ModData>`（R3/C-3b）：组节点、扁平 `mods` 列表与缓存间
    /// 共享同一底层实例，增量合并（`subtree_replace`）时未变条目经磁盘签名
    /// 判定后直接复用旧 `Arc`（零重分配）。serde 对 `Arc<T>` 透明，
    /// 序列化产物与 `Vec<ModData>` 完全一致（前端 JSON/类型零变化）。
    pub mods: Vec<Arc<ModData>>,
    /// 模组路径列表（前端零消费，跳过序列化；Rust 内部仍可用）
    /// 元素为 `Arc<str>`，与各模组 `full_path` 共享底层字符串，避免重复整串分配。
    #[serde(default, skip_serializing)]
    pub mod_paths: Vec<Arc<str>>,
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
    ///
    /// 元素为 `Arc<ModGroupData>`（R3/C-3b）：mutex 树深层 patch 时链上节点
    /// 经 `Arc::make_mut` 重建、链外子树指针复用。serde 对 `Arc<T>` 透明，
    /// 序列化产物与 `Vec<ModGroupData>` 完全一致（前端零变化）。
    pub children: Vec<Arc<ModGroupData>>,
    /// 子分组列表（与 children 完全重复的兼容字段，前端零消费，跳过序列化；
    /// 维持值类型不动，内部消费点随编译适配）
    #[serde(default, skip_serializing)]
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

/// Hash 冲突单条详情（一个模组使用该 hash 的一个或多个 INI 文件）
///
/// 用于前端悬浮提示和点击展开详情，提供模组目录路径和冲突 INI 文件绝对路径列表。
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct HashConflictEntry {
    /// 模组名称
    pub mod_name: String,
    /// 模组目录路径（绝对路径）
    pub mod_path: String,
    /// 包含该 hash 的 INI 文件绝对路径列表（同一模组可能多个 INI 共用同一 hash）
    #[serde(default)]
    pub ini_vec: Vec<String>,
}

/// Hash 冲突信息（一个 hash 被多个模组使用）
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HashConflict {
    /// 冲突的 hash 值（原始字符串，如 "0x12345678"）
    pub hash: String,
    /// 使用该 hash 的模组详情列表（每个模组一条，含其全部冲突 INI）
    #[serde(default)]
    pub entries: Vec<HashConflictEntry>,
}

/// Hash 冲突检测结果
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct HashConflictResult {
    /// 所有冲突列表
    pub conflicts: Vec<HashConflict>,
    /// 扫描的模组总数
    pub scanned_mods: u32,
    /// 扫描的 hash 总数
    pub scanned_hashes: u32,
}

/// ORFix/TexFx 检测结果
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct OrfixDetection {
    /// 库在模组内（模组声明了已知库命名空间）：mod_name → library_name 列表
    pub libs_in_mods: Vec<LibInMod>,
    /// 重复声明（DUPLICATE LIB）：同一已知库命名空间被多个 INI 声明
    pub duplicate_libs: Vec<DuplicateLib>,
    /// 引用未声明（NON EXISTENT LIB）：run = 引用了已知库但全局未声明
    pub nonexistent_libs: Vec<NonExistentLib>,
    /// 是否检测到任何 ORFix/TexFx 异常
    pub has_detection: bool,
}

/// 库在模组内
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LibInMod {
    /// 模组名称
    pub mod_name: String,
    /// 模组路径
    pub mod_path: String,
    /// 检测到的库名称列表（如 "ORFix"、"TexFx"）
    pub lib_names: Vec<String>,
}

/// 重复声明库
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateLib {
    /// 库名称（如 "ORFix"）
    pub lib_name: String,
    /// 声明该库的模组名称列表
    pub mod_names: Vec<String>,
}

/// 引用未声明的库
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NonExistentLib {
    /// 被引用的库名称（如 "ORFix"）
    pub lib_name: String,
    /// 引用该库的模组名称列表
    pub mod_names: Vec<String>,
}

/// 命令级增量：受影响分组的完整新状态（前端按 oldGroupPath 定位并原地替换该组）
///
/// 用于状态切换/移除/重命名等命令的返回值，替代全量 invalidate_all + 前端全量 refresh。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupDelta {
    /// 旧分组路径（前端树定位键；分组自身重命名时与 newGroup.groupPath 不同）
    pub old_group_path: String,
    /// 新分组节点（含 mods/children/统计，后端局部重扫产物、已按既定规则排序）；
    /// None 表示分组已整体移除（remove_group 等场景）
    pub new_group: Option<ModGroupData>,
    /// 该分组旧的扁平 modPath 列表（前端从扁平 mods 列表剔除这些条目后 splice 新条目）
    pub old_mod_paths: Vec<String>,
}

impl Default for GroupDelta {
    /// 创建空 delta（无分组定位、无新状态、无旧条目）。
    ///
    /// 供 `RemoveModResult` 等结构体在 core 层构造占位、命令层组装真实值后覆盖。
    fn default() -> Self {
        Self {
            old_group_path: String::new(),
            new_group: None,
            old_mod_paths: Vec::new(),
        }
    }
}

/// 批量组操作增量（禁用/启用全组等）：保留原 changed 计数语义
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchGroupDelta {
    /// 受影响（被切换状态）的模组数（原命令返回值语义保留）
    pub changed: u32,
    /// 受影响分组增量
    pub delta: GroupDelta,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约锁定：GroupDelta 序列化字段名为 camelCase（前端并行开发依赖此契约）
    #[test]
    fn group_delta_serializes_camel_case() {
        let delta = GroupDelta {
            old_group_path: "D:/Mods/_MANAGED_/group_1".to_string(),
            new_group: Some(ModGroupData::default()),
            old_mod_paths: vec!["D:/Mods/_MANAGED_/group_1/ModA".to_string()],
        };
        let v = serde_json::to_value(&delta).expect("GroupDelta 序列化失败");
        assert!(v.get("oldGroupPath").is_some(), "应含 oldGroupPath 字段");
        assert!(v.get("newGroup").is_some(), "应含 newGroup 字段");
        assert!(v.get("oldModPaths").is_some(), "应含 oldModPaths 字段");
        assert!(
            v.get("old_group_path").is_none(),
            "不应出现 snake_case 字段"
        );
    }

    /// 契约锁定：BatchGroupDelta 序列化字段名为 camelCase 且保留 changed 计数
    #[test]
    fn batch_group_delta_serializes_camel_case() {
        let d = BatchGroupDelta {
            changed: 3,
            delta: GroupDelta::default(),
        };
        let v = serde_json::to_value(&d).expect("BatchGroupDelta 序列化失败");
        assert_eq!(
            v.get("changed").and_then(serde_json::Value::as_u64),
            Some(3),
            "应含 changed 字段且语义保留"
        );
        assert!(v.get("delta").is_some(), "应含 delta 字段");
        assert!(v.get("old_group_path").is_none());
    }

    /// 契约锁定（R3/C-3b）：Arc 化后序列化字节与值类型完全一致——
    /// serde 对 `Arc<T>` 透明，前端 JSON/类型零变化。
    #[test]
    fn arc_wrapped_serialization_is_transparent() {
        let m = ModData {
            mod_path: "D:/Mods/_MANAGED_/group_1/ModA".to_string(),
            mod_name: "ModA".to_string(),
            name: "ModA".to_string(),
            is_active: true,
            is_favorite: true,
            mod_index: 1,
            group_index: 1,
            disk_signature: 12345,
            ..Default::default()
        };

        let plain = serde_json::to_string(&m).expect("ModData 序列化失败");
        let arc = serde_json::to_string(&Arc::new(m.clone())).expect("Arc<ModData> 序列化失败");
        assert_eq!(
            plain, arc,
            "Arc<ModData> 与 ModData 序列化字节必须一致（serde 透明）"
        );
        assert!(
            !plain.contains("diskSignature"),
            "diskSignature 必须 skip_serializing（前端零感知）"
        );

        // 组节点：mods/children Arc 化后 JSON 形态不变
        let group = ModGroupData {
            mods: vec![Arc::new(m)],
            children: vec![Arc::new(ModGroupData {
                group_name: "Sub".to_string(),
                ..Default::default()
            })],
            ..Default::default()
        };
        let v = serde_json::to_value(&group).expect("ModGroupData 序列化失败");
        let mods = v
            .get("mods")
            .and_then(serde_json::Value::as_array)
            .expect("mods 应为数组");
        assert_eq!(mods.len(), 1);
        assert!(
            mods[0].get("modPath").is_some(),
            "组内模组字段保持 camelCase"
        );
        assert!(
            mods[0].get("diskSignature").is_none(),
            "组内模组 diskSignature 不得外泄"
        );
        assert!(
            v.get("children")
                .and_then(|c| c.as_array())
                .is_some_and(|c| !c.is_empty()),
            "children 数组形态保持"
        );
        assert!(
            v.get("childGroups").is_none(),
            "childGroups 保持 skip_serializing"
        );
    }

    /// 契约锁定（R3/C-3b）：diskSignature 缺省可反序列化（default = 0）。
    ///
    /// ModData/ModGroupData 目前无 IPC 入参反序列化消费点，此测试锁定
    /// derive 语义完备性：字段缺失时回退 0（无签名 → 增量合并永不复用）。
    #[test]
    fn mod_data_deserializes_without_disk_signature() {
        let m: ModData = serde_json::from_str(
            r#"{"modPath":"D:/M/ModA","modName":"ModA","isActive":false,
                "isFavorite":false,"isNamespaced":false,"modDisabled":false,
                "modIndex":1}"#,
        )
        .expect("缺省 diskSignature 应可反序列化");
        assert_eq!(m.disk_signature, 0, "缺省 diskSignature 应回退 0");
        assert_eq!(m.mod_name, "ModA");
    }
}
