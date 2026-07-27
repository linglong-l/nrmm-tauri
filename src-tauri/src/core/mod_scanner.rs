use anyhow::Result;
use std::path::{Path, PathBuf};
use std::collections::{VecDeque, HashSet};
use std::fs;
use regex::Regex;
use once_cell::sync::Lazy;
use crate::core::constants;
use crate::core::ini_handler::IniFile;
use crate::core::namespace_handler;
use crate::models::enums::{GroupType, TargetGame, ModsPathStatus};
use crate::models::mod_data::{ModData, ModGroupData, ModIniData, ErroredLines};

static ICON_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico"];
static GROUP_N_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^group_\d+$").unwrap());
static DISABLED_PREFIX_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?i:disabled)[_\- ]*").unwrap());

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub groups: Vec<ModGroupData>,
    pub mods: Vec<ModData>,
    pub total_mods_count: usize,
    pub enabled_mods_count: usize,
    pub disabled_mods_count: usize,
}

pub fn get_managed_folder(game_mods_path: &Path) -> PathBuf {
    game_mods_path.join(constants::MANAGED_FOLDER)
}

pub fn check_mods_path(game: TargetGame, mods_path: &Path) -> ModsPathStatus {
    if !mods_path.exists() {
        return ModsPathStatus::NotFound;
    }
    let managed = mods_path.join(constants::MANAGED_FOLDER);
    if !managed.exists() {
        return ModsPathStatus::ManagedFolderNotFound;
    }
    let d3dx_name = game.d3dx_ini_name();
    let d3dx_path = mods_path.join(d3dx_name);
    if !d3dx_path.exists() {
        return ModsPathStatus::D3dxIniNotFound;
    }
    ModsPathStatus::Valid
}

pub fn scan_mods(game_mods_path: &Path) -> Result<ScanResult> {
    let managed_folder = get_managed_folder(game_mods_path);
    if !managed_folder.exists() {
        fs::create_dir_all(&managed_folder)?;
        return Ok(ScanResult {
            groups: vec![],
            mods: vec![],
            total_mods_count: 0,
            enabled_mods_count: 0,
            disabled_mods_count: 0,
        });
    }

    let mut groups: Vec<ModGroupData> = Vec::new();
    let mut all_mods: Vec<ModData> = Vec::new();
    let mut known_libraries = HashSet::new();

    let entries = fs::read_dir(&managed_folder)?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();

        if !GROUP_N_RE.is_match(&dir_name) {
            continue;
        }

        let (group, mods) = scan_group_directory(&path, &dir_name, GroupType::NormalGroup)?;

        for mod_data in &mods {
            for ini_data in &mod_data.mod_ini_data {
                if let Ok(ini) = IniFile::parse(&PathBuf::from(&ini_data.ini_path)) {
                    let libs = ini.defined_libraries();
                    known_libraries.extend(libs);
                }
            }
        }

        groups.push(group);
        all_mods.extend(mods);
    }

    groups.sort_by_key(|g| g.group_index());

    let enabled = all_mods.iter().filter(|m| !m.disabled && !m.mod_disabled).count();
    let disabled = all_mods.iter().filter(|m| m.disabled || m.mod_disabled).count();

    Ok(ScanResult {
        groups,
        total_mods_count: all_mods.len(),
        enabled_mods_count: enabled,
        disabled_mods_count: disabled,
        mods: all_mods,
    })
}

fn scan_group_directory(dir_path: &Path, group_name: &str, group_type: GroupType) -> Result<(ModGroupData, Vec<ModData>)> {
    let mut mods = Vec::new();
    let mut subgroups: Vec<ModGroupData> = Vec::new();
    let mut subgroup_paths: HashSet<PathBuf> = HashSet::new();

    let mut queue = VecDeque::new();
    queue.push_back(dir_path.to_path_buf());

    let mut visited_dirs = HashSet::new();
    visited_dirs.insert(dir_path.to_path_buf());

    while let Some(current_path) = queue.pop_front() {
        let (has_ini, has_icon, icon_path, ini_files) = check_directory_for_mod(&current_path)?;

        if has_ini || has_icon {
            let parent_groups: Vec<String> = Vec::new();
            let mod_data = build_mod_data(&current_path, group_name, &parent_groups, ini_files, icon_path)?;
            mods.push(mod_data);
        } else {
            let sub_entries = match fs::read_dir(&current_path) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let mut has_subdirs = false;
            for sub_entry in sub_entries {
                let sub_entry = match sub_entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let sub_path = sub_entry.path();
                if sub_path.is_dir() {
                    let sub_name = sub_entry.file_name().to_string_lossy().to_string();
                    if sub_name.starts_with('.') {
                        continue;
                    }
                    has_subdirs = true;
                    if !visited_dirs.contains(&sub_path) {
                        visited_dirs.insert(sub_path.clone());
                        queue.push_back(sub_path);
                    }
                }
            }

            if current_path != dir_path && has_subdirs {
                if !subgroup_paths.contains(&current_path) {
                    subgroup_paths.insert(current_path.clone());
                    let subgroup_name = current_path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let subgroup = ModGroupData {
                        name: subgroup_name.clone(),
                        group_name: subgroup_name,
                        group_type,
                        full_path: current_path.clone(),
                        group_path: current_path.to_string_lossy().to_string(),
                        ..Default::default()
                    };
                    subgroups.push(subgroup);
                }
            }
        }
    }

    let group_index: u32 = group_name
        .trim_start_matches("group_")
        .parse()
        .unwrap_or(0);

    let mod_paths: Vec<PathBuf> = mods.iter().map(|m| m.full_path.clone()).collect();

    let has_children = !mods.is_empty() || !subgroups.is_empty();
    let group = ModGroupData {
        name: group_name.to_string(),
        group_name: group_name.to_string(),
        group_type,
        full_path: dir_path.to_path_buf(),
        group_path: dir_path.to_string_lossy().to_string(),
        group_index,
        child_groups: subgroups.clone(),
        children: subgroups,
        has_child: has_children,
        mod_count: mods.len() as u32,
        mod_paths: mod_paths.clone(),
        mods: mods.clone(),
        ..Default::default()
    };

    for (idx, mod_data) in mods.iter_mut().enumerate() {
        mod_data.group_index = group_index;
        mod_data.mod_index = idx as u32;
        mod_data.mod_path = mod_data.full_path.to_string_lossy().to_string();
        mod_data.mod_name = mod_data.name.clone();
        mod_data.mod_disabled = mod_data.disabled;
        if let Some(first_ini) = mod_data.mod_ini_data.first() {
            mod_data.mod_ini = Some(first_ini.clone());
        }
        if mod_data.namespace.is_some() {
            mod_data.is_namespaced = true;
            mod_data.namespaces = mod_data.namespace.clone().into_iter().collect();
        }
    }

    Ok((group, mods))
}

fn check_directory_for_mod(dir: &Path) -> Result<(bool, bool, Option<PathBuf>, Vec<PathBuf>)> {
    let mut has_ini = false;
    let mut has_icon = false;
    let mut icon_path: Option<PathBuf> = None;
    let mut ini_files: Vec<PathBuf> = Vec::new();
    let mut first_image: Option<PathBuf> = None;

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok((false, false, None, vec![])),
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                let ext_lower = ext.to_string_lossy().to_lowercase();
                if ext_lower == "ini" {
                    has_ini = true;
                    ini_files.push(path);
                } else if ICON_EXTENSIONS.contains(&ext_lower.as_str()) {
                    has_icon = true;
                    let stem = path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase();
                    if stem == "icon" {
                        icon_path = Some(path);
                    } else if first_image.is_none() {
                        let stem_lower = stem.to_lowercase();
                        if !stem_lower.starts_with("disable") {
                            first_image = Some(path);
                        }
                    }
                }
            }
        }
    }

    if icon_path.is_none() {
        icon_path = first_image;
    }

    ini_files.sort();
    Ok((has_ini, has_icon, icon_path, ini_files))
}

fn build_mod_data(
    dir: &Path,
    _group_name: &str,
    _parent_groups: &[String],
    ini_files: Vec<PathBuf>,
    icon_path: Option<PathBuf>,
) -> Result<ModData> {
    let dir_name = dir.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let disabled = dir_name.to_uppercase().starts_with("DISABLED");

    let mod_name = if disabled {
        DISABLED_PREFIX_RE.replace(&dir_name, "").to_string()
    } else {
        dir_name.clone()
    };

    let mut mod_ini_data: Vec<ModIniData> = Vec::new();
    let mut all_errored_lines: Vec<ErroredLines> = Vec::new();
    let mut total_sections = 0;
    let mut total_key_sections = 0;
    let mut total_texture_sections = 0;
    let mut total_shader_sections = 0;
    let mut total_command_lists = 0;
    let mut total_resources = 0;
    let mut mod_namespace: Option<String> = None;
    let mut defined_libraries = HashSet::new();

    for ini_path in &ini_files {
        match IniFile::parse(ini_path) {
            Ok(ini) => {
                let mut keys = 0usize;
                let mut textures = 0usize;
                let mut shaders = 0usize;
                let mut commands = 0usize;
                let mut resources = 0usize;

                for section in &ini.sections {
                    let sname = section.name.to_lowercase();
                    if sname.starts_with("keypress")
                        || (sname.starts_with("key")
                            && !sname.starts_with("keyboard")
                            && !sname.starts_with("keybind"))
                    {
                        keys += 1;
                    } else if sname.starts_with("textureoverride") {
                        textures += 1;
                    } else if sname.starts_with("shaderoverride") {
                        shaders += 1;
                    } else if sname.starts_with("commandlist") {
                        commands += 1;
                    } else if sname.starts_with("resource") {
                        resources += 1;
                    }
                }

                total_sections += ini.sections.len();
                total_key_sections += keys;
                total_texture_sections += textures;
                total_shader_sections += shaders;
                total_command_lists += commands;
                total_resources += resources;

                if mod_namespace.is_none() {
                    mod_namespace = namespace_handler::extract_namespace(&ini);
                }

                defined_libraries.extend(ini.defined_libraries());

                let known_libs = HashSet::new();
                let errors = ini.detect_errors(ini_path, &known_libs);
                all_errored_lines.extend(errors);

                let ini_filename = ini_path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                mod_ini_data.push(ModIniData {
                    ini_path: ini_path.to_string_lossy().to_string(),
                    ini_filename,
                    file_relative_path: ini_path.to_string_lossy().to_string(),
                    section_count: ini.sections.len() as u32,
                    key_sections: keys as u32,
                    texture_override_sections: textures as u32,
                    shader_override_sections: shaders as u32,
                    command_list_sections: commands as u32,
                    resource_sections: resources as u32,
                    has_include: ini.has_include(),
                    ..Default::default()
                });
            }
            Err(e) => {
                log::warn!("Failed to parse INI {}: {}", ini_path.display(), e);
                all_errored_lines.push(ErroredLines {
                    error_type: 3,
                    error_message: format!("Parse error: {}", e),
                    ..Default::default()
                });
            }
        }
    }

    let is_namespaced = mod_namespace.is_some();
    let namespaces_vec = mod_namespace.as_ref().map(|ns| vec![ns.clone()]).unwrap_or_default();

    Ok(ModData {
        name: mod_name.clone(),
        mod_name,
        full_path: dir.to_path_buf(),
        mod_path: dir.to_string_lossy().to_string(),
        parent_folder: dir.parent().unwrap_or(dir).to_path_buf(),
        preview_image_path: icon_path,
        disabled,
        mod_disabled: disabled,
        mod_ini_data,
        errored_lines: all_errored_lines,
        namespace: mod_namespace,
        namespaces: namespaces_vec,
        is_namespaced,
        known_libraries: defined_libraries.into_iter().collect(),
        key_sections: total_key_sections as u32,
        texture_override_sections: total_texture_sections as u32,
        shader_override_sections: total_shader_sections as u32,
        command_list_sections: total_command_lists as u32,
        resource_sections: total_resources as u32,
        total_section_count: total_sections as u32,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        dir
    }

    fn create_group_dir(base: &Path, group_name: &str) -> PathBuf {
        let group_path = base.join("_MANAGED_").join(group_name);
        fs::create_dir_all(&group_path).unwrap();
        group_path
    }

    fn create_mod_with_ini(group_path: &Path, mod_name: &str, ini_content: &str) -> PathBuf {
        let mod_path = group_path.join(mod_name);
        fs::create_dir_all(&mod_path).unwrap();
        let ini_path = mod_path.join("mod.ini");
        fs::write(&ini_path, ini_content).unwrap();
        mod_path
    }

    fn create_d3dx_ini(base: &Path) {
        fs::write(base.join("d3dx.ini"), "; test").unwrap();
    }

    #[test]
    fn test_scan_empty_managed_folder() {
        let dir = setup_test_dir();
        let result = scan_mods(dir.path()).unwrap();
        assert!(result.groups.is_empty());
        assert!(result.mods.is_empty());
        assert_eq!(result.total_mods_count, 0);
    }

    #[test]
    fn test_scan_single_mod() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&group_path, "TestMod", "[TextureOverrideTest]\nhash = 0x123\n");

        let result = scan_mods(dir.path()).unwrap();
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.mods.len(), 1);
        assert_eq!(result.total_mods_count, 1);
        assert_eq!(result.mods[0].name, "TestMod");
        assert!(!result.mods[0].disabled);
    }

    #[test]
    fn test_scan_disabled_mod() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&group_path, "DISABLED_MyMod", "[TextureOverrideTest]\nhash = 0x456\n");

        let result = scan_mods(dir.path()).unwrap();
        assert_eq!(result.mods.len(), 1);
        assert!(result.mods[0].disabled);
        assert!(result.mods[0].mod_disabled);
        assert!(result.mods[0].name.contains("MyMod"));
        assert_eq!(result.disabled_mods_count, 1);
        assert_eq!(result.enabled_mods_count, 0);
    }

    #[test]
    fn test_scan_ignores_non_group_dirs() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let mutex_path = dir.path().join("_MANAGED_").join("#MutexMods");
        fs::create_dir_all(&mutex_path).unwrap();
        create_mod_with_ini(&mutex_path, "MutexMod", "[TextureOverrideTest]\nhash = 0x789\n");

        let other_path = dir.path().join("_MANAGED_").join("OtherFolder");
        fs::create_dir_all(&other_path).unwrap();
        create_mod_with_ini(&other_path, "OtherMod", "[TextureOverrideTest]\nhash = 0xABC\n");

        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&group_path, "ValidMod", "[TextureOverrideTest]\nhash = 0xDEF\n");

        let result = scan_mods(dir.path()).unwrap();
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.mods.len(), 1);
        assert_eq!(result.mods[0].name, "ValidMod");
    }

    #[test]
    fn test_scan_nested_mods() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let subdir = group_path.join("SubCategory");
        fs::create_dir_all(&subdir).unwrap();
        let mod_path = subdir.join("NestedMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[KeyTest]\nkey = VkA\n").unwrap();

        let result = scan_mods(dir.path()).unwrap();
        assert_eq!(result.mods.len(), 1);
        assert_eq!(result.mods[0].name, "NestedMod");
    }

    #[test]
    fn test_scan_mod_with_icon() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let mod_path = group_path.join("IconMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[Constants]\nx=1\n").unwrap();
        fs::write(mod_path.join("icon.png"), b"fake png").unwrap();

        let result = scan_mods(dir.path()).unwrap();
        assert_eq!(result.mods.len(), 1);
        assert!(result.mods[0].preview_image_path.is_some());
        assert!(result.mods[0].preview_image_path.as_ref().unwrap().ends_with("icon.png"));
    }

    #[test]
    fn test_check_mods_path_valid() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let status = check_mods_path(TargetGame::GenshinImpact, dir.path());
        assert_eq!(status, ModsPathStatus::Valid);
    }

    #[test]
    fn test_check_mods_path_missing_managed() {
        let dir = TempDir::new().unwrap();
        create_d3dx_ini(dir.path());
        let status = check_mods_path(TargetGame::GenshinImpact, dir.path());
        assert_eq!(status, ModsPathStatus::ManagedFolderNotFound);
    }

    #[test]
    fn test_check_mods_path_not_found() {
        let dir = TempDir::new().unwrap();
        let nonexistent = dir.path().join("nonexistent");
        let status = check_mods_path(TargetGame::GenshinImpact, &nonexistent);
        assert_eq!(status, ModsPathStatus::NotFound);
    }

    #[test]
    fn test_check_mods_path_missing_d3dx() {
        let dir = setup_test_dir();
        let status = check_mods_path(TargetGame::GenshinImpact, dir.path());
        assert_eq!(status, ModsPathStatus::D3dxIniNotFound);
    }

    #[test]
    fn test_bfs_stops_at_mod_directory() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let mod_path = group_path.join("StoppingMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[TextureOverrideA]\nhash=1\n").unwrap();

        let subdir_under_mod = mod_path.join("ShouldNotBeScanned");
        fs::create_dir_all(&subdir_under_mod).unwrap();
        fs::write(subdir_under_mod.join("ignored.ini"), "[TextureOverrideB]\nhash=2\n").unwrap();

        let result = scan_mods(dir.path()).unwrap();
        assert_eq!(result.mods.len(), 1);
        assert_eq!(result.mods[0].name, "StoppingMod");
    }

    #[test]
    fn test_multiple_groups() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let g1 = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(&g1, "Mod1", "[TextureOverride1]\nhash=1\n");

        let g2 = create_group_dir(dir.path(), "group_2");
        create_mod_with_ini(&g2, "Mod2", "[TextureOverride2]\nhash=2\n");

        let g10 = create_group_dir(dir.path(), "group_10");
        create_mod_with_ini(&g10, "Mod10", "[TextureOverride10]\nhash=10\n");

        let result = scan_mods(dir.path()).unwrap();
        assert_eq!(result.groups.len(), 3);
        assert_eq!(result.mods.len(), 3);
        assert_eq!(result.groups[0].group_name, "group_1");
        assert_eq!(result.groups[1].group_name, "group_2");
        assert_eq!(result.groups[2].group_name, "group_10");
    }

    #[test]
    fn test_ini_section_counting() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let ini_content = r#"
[Constants]
x = 1

[KeyTest]
key = VkA

[KeyPressB]
key = VkB

[TextureOverrideTex1]
hash = 0x1

[TextureOverrideTex2]
hash = 0x2

[ShaderOverrideS1]
hash = 0x3

[CommandListCL1]
x = 1

[ResourceR1]
filename = test.dds
"#;
        create_mod_with_ini(&group_path, "CountingMod", ini_content);

        let result = scan_mods(dir.path()).unwrap();
        let m = &result.mods[0];
        assert_eq!(m.key_sections, 2);
        assert_eq!(m.texture_override_sections, 2);
        assert_eq!(m.shader_override_sections, 1);
        assert_eq!(m.command_list_sections, 1);
        assert_eq!(m.resource_sections, 1);
        assert_eq!(m.total_section_count, 8);
    }

    #[test]
    fn test_has_include_detection() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let ini_content = "include = common.ini\n[Section]\nx=1\n";
        create_mod_with_ini(&group_path, "IncludeMod", ini_content);

        let result = scan_mods(dir.path()).unwrap();
        let ini_data = &result.mods[0].mod_ini_data[0];
        assert!(ini_data.has_include);
    }

    #[test]
    fn test_namespace_extraction() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let group_path = create_group_dir(dir.path(), "group_1");

        let ini_content = "namespace = MyTestMod\n[TextureOverrideT]\nhash=1\n";
        create_mod_with_ini(&group_path, "NsMod", ini_content);

        let result = scan_mods(dir.path()).unwrap();
        assert!(result.mods[0].is_namespaced);
        assert_eq!(result.mods[0].namespace, Some("MyTestMod".to_string()));
    }
}
