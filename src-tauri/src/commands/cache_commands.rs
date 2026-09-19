//! 缓存有效性检查命令域
//!
//! 从 core/mod_cache.rs 迁出（2026-09-19 Phase 6-4）。
//! 承载缓存有效性查询命令，内部经 `crate::hotkey::parse_game` 解析游戏标识。

use crate::core::mod_cache::MOD_CACHE;
use std::path::Path;

/// 检查缓存有效性
///
/// # 返回
///
/// - `true`: 缓存有效，可直接使用。
/// - `false`: 缓存已失效或不存在。
///
/// # 注意
///
/// 若 `parse_game` 解析失败（未知游戏名称），使用 `unwrap_or(TargetGame::GenshinImpact)` 作为兜底。
#[tauri::command]
pub fn check_mod_cache_valid(game: String, mods_path: String) -> bool {
    use crate::models::enums::TargetGame;
    let parsed = crate::hotkey::parse_game(&game).unwrap_or(TargetGame::GenshinImpact);
    let cache = MOD_CACHE.read();
    cache.is_valid(parsed, Path::new(&mods_path))
}