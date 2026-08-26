//! NRMM 原版状态同步模块
//!
//! 原版 NRMM（Flutter + shared_preferences）在 Windows 上将「是否需要更新模组数据」
//! 按游戏持久化到 `shared_preferences.json`（磁盘 key 带 `flutter.` 前缀，例如
//! `flutter.genshinUpdateMod`）。本项目与原版 NRMM 共用同一套 Mods 目录，因此
//! 需要在该文件中同步写入/清除对应标记：
//! - 模组启用/禁用后写 `true`（与原版 showUpdateModSnackbar 一致）
//! - `update_mod_data` 成功后写 `false`（与原版 manageMods 完成一致）
//!
//! 这样在两款应用切换时，NRMM 不会因本项目已更新模组数据而残留「未更新」提示。
//!
//! ## 平台说明
//! NRMM 为 Windows Flutter 应用，仅 Windows 执行真实读写；非 Windows 平台为无操作。

use crate::models::enums::TargetGame;
use serde_json::{Map, Value};
use std::path::PathBuf;

/// NRMM shared_preferences 所在相对路径（基于用户主目录）
/// `%USERPROFILE%\AppData\Roaming\com.aglg\No Reload Mod Manager\shared_preferences.json`
#[cfg(target_os = "windows")]
const NRMM_REL_PATH: [&str; 4] = ["AppData", "Roaming", "com.aglg", "No Reload Mod Manager"];

/// shared_preferences.json 文件名（Windows 插件默认）
const NRMM_PREFS_FILE: &str = "shared_preferences.json";

/// shared_preferences 默认 key 前缀（Windows 插件 `_defaultPrefix`）
const SP_PREFIX: &str = "flutter.";

/// 将本项目游戏枚举映射为 NRMM「需要更新模组数据」标记的 key 后缀。
///
/// 对应原版 `shared_pref.dart` 常量：`keyGenshinUpdateMod = "genshinUpdateMod"` 等。
/// 本项目比 NRMM 多出 `HonkaiImpact3rd`（崩坏3），NRMM 无对应标记，返回 `None` 表示忽略。
///
/// # 参数
/// - `game`: 目标游戏
///
/// # 返回
/// - 映射到 NRMM 标记后缀时返回 `Some(&str)`
/// - NRMM 不支持的游戏中返回 `None`
fn update_mod_key_suffix(game: TargetGame) -> Option<&'static str> {
    match game {
        TargetGame::GenshinImpact => Some("genshinUpdateMod"),
        TargetGame::HonkaiStarRail => Some("hsrUpdateMod"),
        TargetGame::Wuwa => Some("wuwaUpdateMod"),
        TargetGame::ZZZ => Some("zzzUpdateMod"),
        TargetGame::ArknightsEndfield => Some("endfieldUpdateMod"),
        // 原版 NRMM 未收录崩坏3，没有对应标记，忽略同步
        TargetGame::HonkaiImpact3rd => None,
    }
}

/// 构造 NRMM 的 `shared_preferences.json` 完整路径（仅 Windows）。
///
/// NRMM 将配置存于 Flutter `shared_preferences` 的 Roaming AppData 目录下。
///
/// # 返回
/// - 路径构造成功时返回 `Some(PathBuf)`
/// - 非 Windows 平台或无法确定用户主目录时返回 `None`
#[cfg(target_os = "windows")]
fn nrmm_prefs_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let mut p = home;
    for seg in NRMM_REL_PATH {
        p.push(seg);
    }
    p.push(NRMM_PREFS_FILE);
    Some(p)
}

/// 读取 existing `shared_preferences.json`（若存在并合法）为 JSON 键值集合。
///
/// 文件不存在或不是合法 JSON 对象时视为空集合处理（容错，不 panic）。
#[cfg(target_os = "windows")]
fn read_existing_prefs(path: &PathBuf) -> Map<String, Value> {
    match std::fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => serde_json::from_str::<Value>(&content)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default(),
        _ => Map::new(),
    }
}

/// 设置或清除 NRMM 的「需要更新模组数据」标记。
///
/// 仅当 `path` 指向的文件已存在时才写回（避免在本机未安装原版 NRMM 时，
/// 在对方目录中凭空创建配置文件）；文件不存在则视为无需操作。
///
/// 写入采用原子方式（临时文件 + rename），失败静默返回错误而不影响主流程。
#[cfg(target_os = "windows")]
fn set_flag_in_file(path: &PathBuf, key_suffix: &str, needed: bool) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    // 带 `flutter.` 前缀的完整磁盘 key，必须与原版 shared_preferences 一致
    let full_key = format!("{SP_PREFIX}{key_suffix}");
    let mut map = read_existing_prefs(path);
    // 值相同则跳过写盘（减少无意义 IO，对数一致性无影响）
    if map.get(&full_key) == Some(&Value::Bool(needed)) {
        return Ok(());
    }
    map.insert(full_key, Value::Bool(needed));
    let json = serde_json::to_string(&map).map_err(|e| format!("serialize nrmm prefs: {e}"))?;
    crate::utils::atomic_write(path, json.as_bytes())
        .map_err(|e| format!("write nrmm prefs {:?}: {e}", path))
}

/// 同步 NRMM 「需要更新模组数据」标记（跨平台入口）。
///
/// 本项目支持 Windows 与 Linux：Windows 上真实读写 NRMM 的 `shared_preferences.json`，
/// Linux 上 NRMM 不可用，本函数为无操作。
///
/// # 参数
/// - `game`: 目标游戏
/// - `needed`: 是否需要更新模组数据；`true` 表示启用/禁用模组后置位，`false` 表示更新完成后清除
///
/// # 返回
/// 现有文件写入成功（或无需写入）返回 `Ok(())`；写入失败返回错误描述。
pub fn sync_update_needed(game: TargetGame, needed: bool) -> Result<(), String> {
    let Some(suffix) = update_mod_key_suffix(game) else {
        return Ok(());
    };
    #[cfg(target_os = "windows")]
    {
        let Some(path) = nrmm_prefs_path() else {
            return Ok(());
        };
        set_flag_in_file(&path, suffix, needed)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = suffix;
        let _ = needed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_mod_key_suffix_mapping() {
        assert_eq!(
            update_mod_key_suffix(TargetGame::GenshinImpact),
            Some("genshinUpdateMod")
        );
        assert_eq!(
            update_mod_key_suffix(TargetGame::HonkaiStarRail),
            Some("hsrUpdateMod")
        );
        assert_eq!(
            update_mod_key_suffix(TargetGame::Wuwa),
            Some("wuwaUpdateMod")
        );
        assert_eq!(update_mod_key_suffix(TargetGame::ZZZ), Some("zzzUpdateMod"));
        assert_eq!(
            update_mod_key_suffix(TargetGame::ArknightsEndfield),
            Some("endfieldUpdateMod")
        );
        // 崩坏3 无 NRMM 对应标记
        assert_eq!(update_mod_key_suffix(TargetGame::HonkaiImpact3rd), None);
    }

    #[test]
    fn set_flag_in_file_writes_and_clears() {
        let dir = std::env::temp_dir().join("nrmm_prefs_test");
        let path = dir.join("shared_preferences.json");
        std::fs::create_dir_all(&dir).unwrap();
        // 预置一份含其它键的已有配置，模拟 NRMM 首次运行后的状态
        std::fs::write(
            &path,
            r#"{"flutter.modsPathGenshin":"C:\\Mods","flutter.genshinUpdateMod":true}"#,
        )
        .unwrap();

        // true → 置位
        set_flag_in_file(&path, "genshinUpdateMod", true).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let map: Map<String, Value> = serde_json::from_str(&content).unwrap();
        assert_eq!(map["flutter.genshinUpdateMod"], Value::Bool(true));
        assert_eq!(
            map["flutter.modsPathGenshin"],
            Value::String("C:\\Mods".to_string())
        );

        // false → 清除
        set_flag_in_file(&path, "genshinUpdateMod", false).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let map: Map<String, Value> = serde_json::from_str(&content).unwrap();
        assert_eq!(map["flutter.genshinUpdateMod"], Value::Bool(false));

        // 幂等：值相同不触发重复写（内容不变）
        std::fs::write(&path, r#"{"flutter.genshinUpdateMod":false}"#).unwrap();
        set_flag_in_file(&path, "genshinUpdateMod", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"flutter.genshinUpdateMod":false}"#
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn set_flag_in_file_missing_file_is_noop() {
        let dir = std::env::temp_dir().join("nrmm_prefs_missing_test");
        let path = dir.join("shared_preferences.json");
        let _ = std::fs::remove_dir_all(&dir);
        // 文件不存在 → 不创建、返回 Ok
        set_flag_in_file(&path, "genshinUpdateMod", true).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn sync_update_needed_hi3_ignored() {
        // 崩坏3 无对应标记，返回 Ok，且不 panic（跨平台分支均可执行）
        let r = sync_update_needed(TargetGame::HonkaiImpact3rd, true);
        assert!(r.is_ok());
    }
}
