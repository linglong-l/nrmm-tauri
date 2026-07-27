use serde::{Serialize, Deserialize};
use std::path::PathBuf;
use super::enums::*;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct KeybindData {
    pub key: String,
    pub value: String,
    pub section: String,
    pub disabled: bool,
    pub extension: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ErroredLines {
    #[serde(default)]
    pub line_number: u32,
    #[serde(default)]
    pub line: String,
    pub error_type: u8,
    #[serde(default)]
    pub error_message: String,
    #[serde(default)]
    pub line_numbers: Vec<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModIniData {
    pub ini_path: String,
    #[serde(default)]
    pub ini_filename: String,
    #[serde(default)]
    pub file_relative_path: String,
    #[serde(default)]
    pub keybinds: Vec<KeybindData>,
    #[serde(default)]
    pub keybind_commands: Vec<KeybindData>,
    #[serde(default)]
    pub constants: Vec<KeybindData>,
    #[serde(default)]
    pub overrides: Vec<KeybindData>,
    #[serde(default)]
    pub command_lists: Vec<KeybindData>,
    #[serde(default)]
    pub present_sections: Vec<String>,
    #[serde(default)]
    pub section_count: u32,
    #[serde(default)]
    pub key_sections: u32,
    #[serde(default)]
    pub texture_override_sections: u32,
    #[serde(default)]
    pub shader_override_sections: u32,
    #[serde(default)]
    pub command_list_sections: u32,
    #[serde(default)]
    pub resource_sections: u32,
    #[serde(default)]
    pub has_include: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModData {
    pub mod_path: String,
    pub mod_name: String,
    pub mod_ini: Option<ModIniData>,
    #[serde(default)]
    pub mod_ini_data: Vec<ModIniData>,
    #[serde(default)]
    pub full_path: PathBuf,
    #[serde(default)]
    pub parent_folder: PathBuf,
    #[serde(default)]
    pub preview_image_path: Option<PathBuf>,
    pub is_active: bool,
    pub is_favorite: bool,
    pub is_namespaced: bool,
    #[serde(default)]
    pub has_nonmanaged_mods_crashline_fix: bool,
    #[serde(default)]
    pub errored_lines: Vec<ErroredLines>,
    #[serde(default)]
    pub errored_preexisting: Vec<ErroredLines>,
    #[serde(default)]
    pub missing_endif: Vec<String>,
    #[serde(default)]
    pub namespaces: Vec<String>,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub known_libraries: Vec<String>,
    #[serde(default)]
    pub duplicate_libraries: Vec<(String, Vec<String>)>,
    #[serde(default)]
    pub nonexistent_libraries: Vec<String>,
    #[serde(default)]
    pub namespace_error: bool,
    pub mod_disabled: bool,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub path_too_long: bool,
    pub mod_index: u32,
    #[serde(default)]
    pub group_index: u32,
    #[serde(default)]
    pub key_sections: u32,
    #[serde(default)]
    pub texture_override_sections: u32,
    #[serde(default)]
    pub shader_override_sections: u32,
    #[serde(default)]
    pub command_list_sections: u32,
    #[serde(default)]
    pub resource_sections: u32,
    #[serde(default)]
    pub total_section_count: u32,
    #[serde(default)]
    pub name: String,
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
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModGroupData {
    pub group_path: String,
    pub group_name: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub full_path: PathBuf,
    pub group_id: u32,
    pub group_index: u32,
    pub mods: Vec<ModData>,
    #[serde(default)]
    pub mod_paths: Vec<PathBuf>,
    pub mod_count: u32,
    pub is_active: bool,
    pub is_favorite: bool,
    pub group_disabled: bool,
    pub group_type: GroupType,
    pub has_child: bool,
    pub children: Vec<ModGroupData>,
    #[serde(default)]
    pub child_groups: Vec<ModGroupData>,
    pub active_mod_index: i32,
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
        }
    }
}

impl ModGroupData {
    pub fn group_index(&self) -> u32 {
        self.group_index
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub os: String,
    pub desktop_session: String,
    pub is_wayland: bool,
    pub is_x11: bool,
    pub is_wslg: bool,
    pub transparency_supported: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CloudLink {
    pub name: String,
    pub url: String,
    pub icon: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CloudMessage {
    pub title: String,
    pub content: String,
    pub level: String,
    pub date: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CloudData {
    pub links: Vec<CloudLink>,
    pub messages: Vec<CloudMessage>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct KeybindConflict {
    pub hotkey_id: String,
    pub conflict_with: String,
}
