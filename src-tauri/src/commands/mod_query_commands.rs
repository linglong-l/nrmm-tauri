//! 模组查询/扫描域命令
//!
//! 从 mod_commands.rs 拆出（2026-09-19 Phase 6-1）。
//! 承载模组列表查询、路径有效性检查、强制刷新、重量级更新、更新标记同步与 hash 冲突检测。
//! 共享辅助（parse_game / SCAN_LOCK / UPDATE_LOCK）经 `use crate::commands::mod_commands_util::{...}`
//! 引用，保持单一实现，避免重复。

use crate::commands::mod_commands_util::{parse_game, SCAN_LOCK, UPDATE_LOCK};
use crate::config::settings_store;
use std::path::PathBuf;
use std::sync::Arc;

/// 获取模组列表（轻量扫描+缓存）
///
/// 优先从内存缓存返回，缓存未命中时执行轻量扫描。
/// 这是 UI 初始化和列表展示的主要入口。
#[tauri::command]
pub async fn get_mods(
    game: String,
    mods_path: String,
) -> Result<Arc<crate::core::mod_scan_light::ScanResult>, String> {
    log::debug!(
        "[commands::mod_commands] [get_mods] game={} mods_path={}",
        game,
        mods_path
    );
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    // 先在锁作用域内取出缓存结果，锁释放后再触发启动备份（fire-and-forget）
    let cached = {
        let cache = crate::core::mod_cache_core::MOD_CACHE.read();
        cache.get(game, &mods_path)
    };
    if let Some(result) = cached {
        log::info!("[get_mods] Cache hit for {}", game.as_str());
        crate::core::ini_backup_scheduler::ensure_game_full_backup(mods_path.clone());
        return Ok(result);
    }

    log::info!("[get_mods] Cache miss, scanning light...");
    let start = std::time::Instant::now();

    // singleflight：串行化并发扫描。拿锁后二次检查缓存——等锁期间首个扫描者
    // 已回填，直接取缓存免扫（并发 N 次 cache miss 只扫一次）
    let _scan_guard = SCAN_LOCK.lock().await;
    {
        let cache = crate::core::mod_cache_core::MOD_CACHE.read();
        if let Some(result) = cache.get(game, &mods_path) {
            log::info!(
                "[get_mods] concurrent scan deduplicated (cache filled by first caller), waited {}ms",
                start.elapsed().as_millis()
            );
            crate::core::ini_backup_scheduler::ensure_game_full_backup(mods_path.clone());
            return Ok(result);
        }
    }

    let scan_path = mods_path.clone();
    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::core::mod_scan_light::ScanResult, String> {
            crate::core::mod_scan_light::scan_mods_light(&scan_path)
                .map_err(crate::core::error_normalizer::err_to_ui)
        },
    )
    .await
    .map_err(|e| {
        log::error!("[get_mods] spawn_blocking join error: {}", e);
        crate::core::error_normalizer::join_error_to_ui()
    })??;

    let elapsed = start.elapsed();
    log::info!(
        "Light scan completed in {}ms, {} mods, {} groups",
        elapsed.as_millis(),
        result.total_mods_count,
        result.groups.len()
    );

    {
        let mut cache = crate::core::mod_cache_core::MOD_CACHE.write();
        cache.set(game, &mods_path, result.clone());
    }

    // 用户主动加载该游戏：触发该游戏路径的全量 INI 备份（进程内每游戏一次，后台线程池执行）
    crate::core::ini_backup_scheduler::ensure_game_full_backup(mods_path.clone());

    Ok(Arc::new(result))
}

#[tauri::command]
pub fn check_mods_path_status(
    game: String,
    mods_path: String,
) -> Result<crate::models::enums::ModsPathStatus, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    Ok(crate::core::mod_scan_light::check_mods_path(
        game, &mods_path,
    ))
}

/// 刷新模组列表（轻量扫描，更新缓存）
#[tauri::command]
pub async fn refresh_mods(
    game: String,
    mods_path: String,
) -> Result<Arc<crate::core::mod_scan_light::ScanResult>, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    log::info!("[refresh_mods] Scanning light...");
    let start = std::time::Instant::now();

    // singleflight：与 get_mods 共用 SCAN_LOCK 串行，防止并发重复全量扫描
    // （refresh 语义为强制重扫，不二次检查缓存，仅消除并发重复扫描）
    let _scan_guard = SCAN_LOCK.lock().await;

    let scan_path = mods_path.clone();
    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::core::mod_scan_light::ScanResult, String> {
            crate::core::mod_scan_light::scan_mods_light(&scan_path)
                .map_err(crate::core::error_normalizer::err_to_ui)
        },
    )
    .await
    .map_err(|e| {
        log::error!("[refresh_mods] spawn_blocking join error: {}", e);
        crate::core::error_normalizer::join_error_to_ui()
    })??;

    let elapsed = start.elapsed();
    log::info!(
        "Light refresh completed in {}ms, {} mods, {} groups",
        elapsed.as_millis(),
        result.total_mods_count,
        result.groups.len()
    );

    {
        let mut cache = crate::core::mod_cache_core::MOD_CACHE.write();
        cache.set(game, &mods_path, result.clone());
    }

    Ok(Arc::new(result))
}

/// 重量级更新模组数据（仅用户按钮触发）
#[tauri::command]
pub async fn update_mod_data(
    game: String,
    mods_path: String,
) -> Result<crate::core::mod_results::UpdateResult, String> {
    let _busy = crate::window::lifecycle::BusyOpGuard::new();
    log::debug!(
        "[commands::mod_commands] [update_mod_data] game={} mods_path={}",
        game,
        mods_path
    );
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let settings = settings_store::get_settings();

    log::info!("[update_mod_data] Running heavy update...");
    let start = std::time::Instant::now();

    let update_path = mods_path.clone();
    // 全局串行化：避免并发两次 update_mod_data 同时写 _MANAGED_ 竞态（H1）
    let _update_guard = UPDATE_LOCK.lock().await;
    // spawn_blocking 内部若 panic，JoinError 会被转为 String 错误返回，不会导致应用崩溃
    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::core::mod_results::UpdateResult, String> {
            crate::core::mod_update_pipeline::update_mod_data(game, &update_path, &settings)
                .map_err(crate::core::error_normalizer::err_to_ui)
        },
    )
    .await
    .map_err(|e| {
        log::error!("[update_mod_data] spawn_blocking join error: {}", e);
        crate::core::error_normalizer::join_error_to_ui()
    })?
    .map_err(|e| {
        log::error!("[update_mod_data] inner error: {}", e);
        e
    })?;

    let elapsed = start.elapsed();
    log::info!(
        "Heavy update completed in {}ms, processed {} mods",
        elapsed.as_millis(),
        result.processed_mods
    );

    // 缓存失效
    {
        let mut cache = crate::core::mod_cache_core::MOD_CACHE.write();
        cache.invalidate_game(game); // 彻底失效该游戏缓存：invalidate_by_prefix 的 contains 判断对子目录恒为 false，是 no-op 陷阱
    }

    Ok(result)
}

/// 同步原版 NRMM「需要更新模组数据」标记（跨应用状态双向同步）
///
/// 本项目与原版 NRMM（Flutter + shared_preferences）共用同一套 Mods 目录，
/// 通过该命令把本项目的更新状态写入 NRMM 的 `shared_preferences.json`：
/// - 前端启用/禁用模组后调用 `needed=true`（与原版 showUpdateModSnackbar 一致）
/// - `update_mod_data` 成功后调用 `needed=false`（与原版 manageMods 完成一致）
///
/// 这样在本项目更新完模组数据后，重新打开原版 NRMM 不会再显示「未更新」。
/// 仅在 Windows 上真实读写 NRMM 配置文件，Linux 上为无操作。
///
/// # 参数
/// - `game`: 目标游戏标识（如 "genshin"、"wuwa"，经 [`parse_game`] 解析）
/// - `needed`: 是否需要更新模组数据；`true` 置位，`false` 清除
///
/// # 返回
/// 同步成功（或非 Windows 平台无操作）返回 `Ok(())`；写文件失败返回错误描述
#[tauri::command]
pub async fn sync_nrmm_update_flag(game: String, needed: bool) -> Result<(), String> {
    let game = parse_game(&game)?;
    log::debug!(
        "[commands::mod_commands] [sync_nrmm_update_flag] game={} needed={}",
        game.as_str(),
        needed
    );
    tauri::async_runtime::spawn_blocking(move || {
        crate::core::nrmm_prefs::sync_update_needed(game, needed)
    })
    .await
    .map_err(|e| {
        log::error!("[sync_nrmm_update_flag] spawn_blocking join error: {}", e);
        crate::core::error_normalizer::join_error_to_ui()
    })??;
    Ok(())
}

/// 检测 hash 冲突命令
///
/// 全量扫描所有模组 INI 中的 hash 值，返回冲突列表。
/// 由用户在设置页面主动触发，扫描策略：
/// - NormalGroup（group_xx）：仅扫描当前选中模组
/// - MutexGroup（非 group_xx）：扫描所有启用模组
#[tauri::command]
pub async fn detect_hash_conflicts(
    mods_path: String,
) -> Result<crate::models::mod_data::HashConflictResult, String> {
    let _busy = crate::window::lifecycle::BusyOpGuard::new();
    let mods_path = PathBuf::from(mods_path);
    log::info!(
        "[detect_hash_conflicts] Starting full hash conflict scan: {}",
        mods_path.display()
    );
    let start = std::time::Instant::now();

    let scan_path = mods_path.clone();
    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::models::mod_data::HashConflictResult, String> {
            crate::core::mod_detect::detect_hash_conflicts(&scan_path)
                .map_err(crate::core::error_normalizer::err_to_ui)
        },
    )
    .await
    .map_err(|e| {
        log::error!("[detect_hash_conflicts] spawn_blocking join error: {}", e);
        crate::core::error_normalizer::join_error_to_ui()
    })??;

    log::info!(
        "[detect_hash_conflicts] Completed in {}ms, found {} conflicts",
        start.elapsed().as_millis(),
        result.conflicts.len()
    );
    Ok(result)
}
