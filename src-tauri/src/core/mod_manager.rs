use anyhow::{Result, Context};
use std::path::{Path, PathBuf};
use std::collections::HashSet;
use std::fs;
use crate::core::constants;
use crate::core::ini_handler::IniFile;
use crate::core::namespace_handler;
use crate::core::mod_scanner;
use crate::models::enums::TargetGame;
use crate::models::mod_data::{ModData, ErroredLines};
use crate::models::settings::AppSettings;

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateResult {
    pub total_groups: u32,
    pub total_mods: u32,
    pub enabled_mods: u32,
    pub disabled_mods: u32,
    pub processed_mods: u32,
    pub errors: Vec<ErroredLines>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RestoredCount {
    pub restored: u32,
    pub failed: u32,
}

pub fn update_mod_data(game: TargetGame, game_mods_path: &Path, _settings: &AppSettings) -> Result<UpdateResult> {
    let managed_folder = game_mods_path.join(constants::MANAGED_FOLDER);
    if !managed_folder.exists() {
        fs::create_dir_all(&managed_folder)?;
    }

    let scan_result = mod_scanner::scan_mods(game_mods_path)?;

    let main_ini_name = game.d3dx_ini_name();
    let main_ini_path = game_mods_path.join(main_ini_name);

    if !main_ini_path.exists() {
        create_default_main_ini(&main_ini_path, main_ini_name)?;
    }

    let backup_path = main_ini_path.with_extension(constants::BACKUP_EXTENSION);
    if !backup_path.exists() {
        fs::copy(&main_ini_path, &backup_path)
            .with_context(|| format!("Failed to backup main INI: {:?}", main_ini_path))?;
    }

    let main_ini_content = IniFile::force_read_as_utf8(&main_ini_path)?;
    let main_ini_content = strip_nrmm_injected_content(&main_ini_content);

    let enabled_mods: Vec<&ModData> = scan_result.mods.iter()
        .filter(|m| !m.disabled && !m.mod_disabled)
        .collect();

    let mut known_libraries = HashSet::new();
    for mod_data in &enabled_mods {
        for ini_data in &mod_data.mod_ini_data {
            let ini_path = PathBuf::from(&ini_data.ini_path);
            if let Ok(ini) = IniFile::parse(&ini_path) {
                for lib in ini.defined_libraries() {
                    known_libraries.insert(lib);
                }
            }
        }
    }

    let mut include_paths: Vec<PathBuf> = Vec::new();
    let mut all_errors: Vec<ErroredLines> = Vec::new();
    let mut processed_mods = 0u32;

    for (mod_idx, mod_data) in enabled_mods.iter().enumerate() {
        let group_id = mod_data.group_index;
        let mod_id = mod_idx as u32;

        for ini_data in &mod_data.mod_ini_data {
            let ini_path = PathBuf::from(&ini_data.ini_path);

            let mod_backup = ini_path.with_extension(constants::BACKUP_EXTENSION);
            if !mod_backup.exists() {
                if let Err(e) = fs::copy(&ini_path, &mod_backup) {
                    log::warn!("Failed to backup mod INI {}: {}", ini_path.display(), e);
                }
            }

            match IniFile::parse(&ini_path) {
                Ok(mut ini) => {
                    let errors = ini.detect_errors(&ini_path, &known_libraries);
                    if !errors.is_empty() {
                        all_errors.extend(errors);
                    }

                    if let Some(ns) = namespace_handler::extract_namespace(&ini) {
                        namespace_handler::expand_ini_variables(&mut ini, &ns);
                    }

                    ini.inject_slot_conditions(group_id, mod_id);

                    let crash_lines = ini.comment_crash_lines();
                    if !crash_lines.is_empty() {
                        log::info!("Commented {} crash lines in {}", crash_lines.len(), ini_path.display());
                    }

                    ini.remove_empty_if_blocks();
                    ini.apply_indentation();

                    ini.write_atomic(&ini_path)?;

                    include_paths.push(ini_path.clone());
                    processed_mods += 1;
                }
                Err(e) => {
                    log::error!("Failed to process mod INI {}: {}", ini_path.display(), e);
                    all_errors.push(ErroredLines {
                        error_type: 3,
                        error_message: format!("Processing error: {}", e),
                        ..Default::default()
                    });
                }
            }
        }
    }

    let injected = generate_nrmm_injected_content(&include_paths, game_mods_path)?;
    let final_content = if main_ini_content.is_empty() {
        injected
    } else {
        format!("{}\n\n{}", main_ini_content, injected)
    };

    let tmp_path = main_ini_path.with_extension("ini.tmp");
    fs::write(&tmp_path, &final_content)
        .with_context(|| format!("Failed to write temp main INI: {:?}", tmp_path))?;
    fs::rename(&tmp_path, &main_ini_path)
        .with_context(|| format!("Failed to rename temp main INI to: {:?}", main_ini_path))?;

    Ok(UpdateResult {
        total_groups: scan_result.groups.len() as u32,
        total_mods: scan_result.total_mods_count as u32,
        enabled_mods: enabled_mods.len() as u32,
        disabled_mods: scan_result.disabled_mods_count as u32,
        processed_mods,
        errors: all_errors,
    })
}

fn strip_nrmm_injected_content(content: &str) -> String {
    let start_marker = ";NRMM_INI_START";
    let end_marker = ";NRMM_INI_END";

    let mut result = String::new();
    let mut in_injected = false;

    for line in content.lines() {
        if line.contains(start_marker) {
            in_injected = true;
            continue;
        }
        if line.contains(end_marker) {
            in_injected = false;
            continue;
        }
        if !in_injected {
            result.push_str(line);
            result.push('\n');
        }
    }

    result.trim_end().to_string()
}

fn generate_nrmm_injected_content(include_paths: &[PathBuf], game_mods_path: &Path) -> Result<String> {
    let mut content = String::new();

    content.push_str(";NRMM_INI_START\n");
    content.push_str("; ==========================================\n");
    content.push_str("; No-Reload Mod Manager managed section\n");
    content.push_str("; Do not edit this section manually\n");
    content.push_str("; ==========================================\n\n");

    content.push_str("[Constants]\n");
    content.push_str("global persist $managed_slot_id = 0\n");
    content.push_str("global persist $managed_selected_group = 0\n");
    content.push_str("global persist $managed_selected_mod = 0\n\n");

    for path in include_paths {
        let rel_path = path.strip_prefix(game_mods_path).unwrap_or(path);
        let rel_str = rel_path.to_string_lossy().replace('\\', "/");
        content.push_str(&format!("include = {}\n", rel_str));
    }

    content.push_str("\n; ==========================================\n");
    content.push_str("; End of NRMM managed section\n");
    content.push_str(";NRMM_INI_END\n");

    Ok(content)
}

fn create_default_main_ini(path: &Path, ini_name: &str) -> Result<()> {
    let content = format!(
r#"; {} - Generated by NRMM
[Constants]
global persist $managed_slot_id = 0
global persist $managed_selected_group = 0
global persist $managed_selected_mod = 0
"#,
        ini_name
    );
    fs::write(path, content)
        .with_context(|| format!("Failed to create default main INI: {:?}", path))?;
    Ok(())
}

pub fn switch_mod(
    game: TargetGame,
    game_mods_path: &Path,
    settings: &AppSettings,
    group_index: u32,
    mod_index: u32,
) -> Result<UpdateResult> {
    let scan_result = mod_scanner::scan_mods(game_mods_path)?;

    for mod_data in &scan_result.mods {
        if mod_data.group_index == group_index {
            let mod_dir = &mod_data.full_path;
            let dir_name = mod_dir.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let target_disabled = dir_name.to_uppercase().starts_with("DISABLED");

            let is_target = mod_data.mod_index == mod_index;

            if is_target && target_disabled {
                let new_name = dir_name
                    .trim_start_matches("DISABLED")
                    .trim_start_matches("disabled")
                    .trim_start_matches(|c: char| c == '_' || c == ' ' || c == '-');
                let new_path = mod_dir.parent().unwrap_or(mod_dir).join(new_name);
                if mod_dir != &new_path {
                    fs::rename(mod_dir, &new_path)
                        .with_context(|| format!("Failed to enable mod: {:?}", mod_dir))?;
                }
            } else if !is_target && !target_disabled {
                let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                let new_path = mod_dir.parent().unwrap_or(mod_dir).join(new_name);
                if mod_dir != &new_path {
                    fs::rename(mod_dir, &new_path)
                        .with_context(|| format!("Failed to disable mod: {:?}", mod_dir))?;
                }
            }
        }
    }

    update_mod_data(game, game_mods_path, settings)
}

pub fn toggle_mod(mod_path: &Path, enable: bool) -> Result<()> {
    let dir_name = mod_path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let is_disabled = dir_name.to_uppercase().starts_with("DISABLED");

    if enable && is_disabled {
        let new_name = dir_name
            .trim_start_matches("DISABLED")
            .trim_start_matches("disabled")
            .trim_start_matches(|c: char| c == '_' || c == ' ' || c == '-');
        let new_path = mod_path.parent().unwrap_or(mod_path).join(new_name);
        if mod_path != &new_path {
            fs::rename(mod_path, &new_path)
                .with_context(|| format!("Failed to enable mod: {:?}", mod_path))?;
        }
    } else if !enable && !is_disabled {
        let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
        let new_path = mod_path.parent().unwrap_or(mod_path).join(new_name);
        if mod_path != &new_path {
            fs::rename(mod_path, &new_path)
                .with_context(|| format!("Failed to disable mod: {:?}", mod_path))?;
        }
    }

    Ok(())
}

pub fn deselect_group_mods(
    game: TargetGame,
    game_mods_path: &Path,
    settings: &AppSettings,
    group_index: u32,
) -> Result<UpdateResult> {
    let scan_result = mod_scanner::scan_mods(game_mods_path)?;

    for mod_data in &scan_result.mods {
        if mod_data.group_index == group_index {
            let mod_dir = &mod_data.full_path;
            let dir_name = mod_dir.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_disabled = dir_name.to_uppercase().starts_with("DISABLED");

            if !is_disabled {
                let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                let new_path = mod_dir.parent().unwrap_or(mod_dir).join(new_name);
                if mod_dir != &new_path {
                    fs::rename(mod_dir, &new_path)
                        .with_context(|| format!("Failed to disable mod: {:?}", mod_dir))?;
                }
            }
        }
    }

    update_mod_data(game, game_mods_path, settings)
}

pub fn restore_all_inis(game_mods_path: &Path) -> Result<RestoredCount> {
    let mut restored = 0u32;
    let mut failed = 0u32;

    for &ini_name in &["d3dx.ini", "RatioShot.ini"] {
        let ini_path = game_mods_path.join(ini_name);
        let backup_path = ini_path.with_extension(constants::BACKUP_EXTENSION);
        if backup_path.exists() {
            match fs::copy(&backup_path, &ini_path) {
                Ok(_) => {
                    restored += 1;
                    fs::remove_file(&backup_path).ok();
                }
                Err(e) => {
                    log::error!("Failed to restore main INI {}: {}", ini_path.display(), e);
                    failed += 1;
                }
            }
        }
    }

    let managed_folder = game_mods_path.join(constants::MANAGED_FOLDER);
    if managed_folder.exists() {
        restore_inis_recursive(&managed_folder, &mut restored, &mut failed)?;
    }

    Ok(RestoredCount { restored, failed })
}

fn restore_inis_recursive(dir: &Path, restored: &mut u32, failed: &mut u32) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            restore_inis_recursive(&path, restored, failed)?;
        } else if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("ini") {
                let backup_path = path.with_extension(constants::BACKUP_EXTENSION);
                if backup_path.exists() {
                    match fs::copy(&backup_path, &path) {
                        Ok(_) => {
                            *restored += 1;
                            fs::remove_file(&backup_path).ok();
                        }
                        Err(e) => {
                            log::error!("Failed to restore mod INI {}: {}", path.display(), e);
                            *failed += 1;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_env() -> TempDir {
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

    fn create_main_ini(base: &Path, game: TargetGame) {
        let ini_name = game.d3dx_ini_name();
        fs::write(base.join(ini_name), "; original main ini\n").unwrap();
    }

    #[test]
    fn test_strip_nrmm_content() {
        let content = r#"; original header
[Constants]
x = 1
;NRMM_INI_START
managed stuff here
include = mod1.ini
;NRMM_INI_END
; trailing content
y = 2
"#;
        let result = strip_nrmm_injected_content(content);
        assert!(!result.contains("managed stuff"));
        assert!(!result.contains("include = mod1.ini"));
        assert!(result.contains("original header"));
        assert!(result.contains("x = 1"));
        assert!(result.contains("y = 2"));
        assert!(!result.contains("NRMM_INI_START"));
        assert!(!result.contains("NRMM_INI_END"));
    }

    #[test]
    fn test_strip_nrmm_content_no_injection() {
        let content = "; no injection here\n[Section]\nkey=val\n";
        let result = strip_nrmm_injected_content(content);
        assert_eq!(result.trim(), content.trim());
    }

    #[test]
    fn test_generate_injected_content() {
        let dir = TempDir::new().unwrap();
        let mod1 = dir.path().join("_MANAGED_/group_1/Mod1/mod.ini");
        fs::create_dir_all(mod1.parent().unwrap()).unwrap();
        fs::write(&mod1, "").unwrap();

        let mod2 = dir.path().join("_MANAGED_/group_1/Mod2/config.ini");
        fs::create_dir_all(mod2.parent().unwrap()).unwrap();
        fs::write(&mod2, "").unwrap();

        let paths = vec![mod1, mod2];
        let result = generate_nrmm_injected_content(&paths, dir.path()).unwrap();

        assert!(result.contains(";NRMM_INI_START"));
        assert!(result.contains(";NRMM_INI_END"));
        assert!(result.contains("[Constants]"));
        assert!(result.contains("$managed_slot_id"));
        assert!(result.contains("include = _MANAGED_/group_1/Mod1/mod.ini"));
        assert!(result.contains("include = _MANAGED_/group_1/Mod2/config.ini"));
        assert!(!result.contains("\\"));
    }

    #[test]
    fn test_create_default_main_ini() {
        let dir = TempDir::new().unwrap();
        let ini_path = dir.path().join("d3dx.ini");
        create_default_main_ini(&ini_path, "d3dx.ini").unwrap();

        assert!(ini_path.exists());
        let content = fs::read_to_string(&ini_path).unwrap();
        assert!(content.contains("d3dx.ini - Generated by NRMM"));
        assert!(content.contains("[Constants]"));
        assert!(content.contains("$managed_slot_id"));
    }

    #[test]
    fn test_toggle_mod_enable_disable() {
        let dir = TempDir::new().unwrap();
        let enabled_path = dir.path().join("MyMod");
        fs::create_dir_all(&enabled_path).unwrap();

        toggle_mod(&enabled_path, false).unwrap();
        let disabled_name = enabled_path.file_name().unwrap().to_str().unwrap().to_string();
        let new_path = dir.path().join(format!("DISABLED_{}", disabled_name));
        assert!(new_path.exists());
        assert!(!enabled_path.exists());

        toggle_mod(&new_path, true).unwrap();
        assert!(enabled_path.exists());
        assert!(!new_path.exists());
    }

    #[test]
    fn test_toggle_mod_already_in_state() {
        let dir = TempDir::new().unwrap();

        let enabled_path = dir.path().join("EnabledMod");
        fs::create_dir_all(&enabled_path).unwrap();
        toggle_mod(&enabled_path, true).unwrap();
        assert!(enabled_path.exists());

        let disabled_path = dir.path().join("DISABLED_DisabledMod");
        fs::create_dir_all(&disabled_path).unwrap();
        toggle_mod(&disabled_path, false).unwrap();
        assert!(disabled_path.exists());
    }

    #[test]
    fn test_update_mod_data_empty() {
        let dir = setup_test_env();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        assert_eq!(result.total_mods, 0);
        assert_eq!(result.enabled_mods, 0);
        assert_eq!(result.processed_mods, 0);
        assert!(result.errors.is_empty());

        let main_ini = dir.path().join("d3dx.ini");
        let content = fs::read_to_string(&main_ini).unwrap();
        assert!(content.contains(";NRMM_INI_START"));
        assert!(content.contains(";NRMM_INI_END"));
    }

    #[test]
    fn test_update_mod_data_with_mods() {
        let dir = setup_test_env();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(
            &group_path,
            "TestMod",
            "[TextureOverrideTest]\nhash = 0x12345678\nps-t0 = ResourceTest\ndrawindexed = auto\n"
        );

        let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        assert_eq!(result.total_mods, 1);
        assert_eq!(result.enabled_mods, 1);
        assert_eq!(result.processed_mods, 1);

        let main_content = fs::read_to_string(dir.path().join("d3dx.ini")).unwrap();
        assert!(main_content.contains("include = _MANAGED_/group_1/TestMod/mod.ini"));

        let mod_ini_path = group_path.join("TestMod/mod.ini");
        let backup_path = mod_ini_path.with_extension(constants::BACKUP_EXTENSION);
        assert!(backup_path.exists());
    }

    #[test]
    fn test_restore_all_inis() {
        let dir = setup_test_env();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let group_path = create_group_dir(dir.path(), "group_1");
        let _mod_path = create_mod_with_ini(
            &group_path,
            "RestoreMod",
            "[TextureOverrideTest]\nhash = 0x1\n"
        );

        update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();

        let main_ini_path = dir.path().join("d3dx.ini");
        let modified_content = fs::read_to_string(&main_ini_path).unwrap();
        assert!(modified_content.contains(";NRMM_INI_START"));

        let result = restore_all_inis(dir.path()).unwrap();
        assert!(result.restored >= 2);
        assert_eq!(result.failed, 0);

        let restored_content = fs::read_to_string(&main_ini_path).unwrap();
        assert!(!restored_content.contains(";NRMM_INI_START"));

        let backup_path = main_ini_path.with_extension(constants::BACKUP_EXTENSION);
        assert!(!backup_path.exists());
    }

    #[test]
    fn test_update_mod_data_creates_default_ini() {
        let dir = setup_test_env();
        let settings = AppSettings::default();

        assert!(!dir.path().join("d3dx.ini").exists());
        let result = update_mod_data(TargetGame::GenshinImpact, dir.path(), &settings).unwrap();
        assert_eq!(result.total_mods, 0);
        assert!(dir.path().join("d3dx.ini").exists());
    }

    #[test]
    fn test_update_mod_data_hsr_ini() {
        let dir = setup_test_env();
        let settings = AppSettings::default();

        let result = update_mod_data(TargetGame::HonkaiStarRail, dir.path(), &settings).unwrap();
        assert_eq!(result.total_mods, 0);
        assert!(dir.path().join("RatioShot.ini").exists());
    }

    #[test]
    fn test_strip_nrmm_multiline() {
        let content = "before\n;NRMM_INI_START\na\nb\nc\n;NRMM_INI_END\nafter";
        let result = strip_nrmm_injected_content(content);
        assert_eq!(result.trim(), "before\nafter");
    }
}
