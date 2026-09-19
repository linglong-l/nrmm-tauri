//! 模组相关 Tauri 命令
//!
//! 提供模组管理的前端接口：
//! - get_mods: 获取模组列表（优先缓存，轻量扫描）
//! - refresh_mods: 强制刷新模组列表（轻量扫描，更新缓存）
//! - check_mods_path_status: 检查模组路径有效性
//! - apply_mods: 应用模组选择（深度扫描+INI注入）
//! - switch_mod: 切换普通分组选中模组
//! - deselect_group: 取消选中分组
//! - toggle_mod_enabled/toggle_mutex_mod_enabled: 启用/禁用模组
//! - toggle_favorite: 切换收藏
//! - open_mod_folder: 打开模组目录
//! - import_mod_from_archive: 从压缩包导入模组
//! - delete_mod: 删除模组
//! - restore_all_inis: 恢复所有 INI 备份
//! - select_folder: 系统文件夹选择对话框
//!
//! # 性能设计
//! - 列表查询用轻量扫描+缓存（<100ms）
//! - apply 操作用深度扫描（完整 INI 解析，可能几秒）
//! - 所有 IO 操作使用 spawn_blocking 避免阻塞 UI

use crate::config::settings_store;
use crate::core::constants;
use crate::core::mod_cache;
use crate::core::mod_manager;
use crate::core::mod_scanner;
use crate::core::resolution;
use crate::models::enums::TargetGame;
use crate::models::mod_data::{BatchGroupDelta, GroupDelta};
use crate::sel_dbg;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

/// 选择操作防抖窗口（毫秒）
const SELECTION_DEBOUNCE_MS: u64 = 500;

/// 选择防抖登记表：键为「分组路径 + 模组路径」复合键，值为最近一次放行时间
///
/// P1 修复：原实现以 `group_path` 单键 + 500ms 窗口，同组快速连续选择**不同**模组时
/// 第二次会被误判为「重复触发」并返回 `"debounced"`（前端静默丢弃）→ 用户表现为
/// 「点了没反应 / 选择失效」。改复合键后仅当「同组且同模组」的重复触发才命中防抖。
static SELECTION_DEBOUNCE: LazyLock<Mutex<HashMap<(String, String), Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 选择防抖判定（P1 修复）
///
/// 仅当「同分组且同模组」在 [`SELECTION_DEBOUNCE_MS`] 窗口内重复触发时命中；
/// 每次写入前 `retain` 掉已过窗口的条目，避免登记表无限增长（原实现永不清理）。
///
/// # 参数
/// - `group_path`: 分组目录绝对路径
/// - `mod_path`: 目标模组绝对路径
///
/// # 返回
/// `Ok(true)` = 命中防抖（调用方应返回 `"debounced"`）；
/// `Ok(false)` = 放行（已记录本次时间戳）；锁中毒时返回 `Err`
fn selection_debounce_hit(group_path: &str, mod_path: &str) -> Result<bool, String> {
    let mut debounce_map = SELECTION_DEBOUNCE
        .lock()
        .map_err(|_| "debounce lock poisoned".to_string())?;
    let now = Instant::now();
    let window = Duration::from_millis(SELECTION_DEBOUNCE_MS);
    // 清理过期条目：超过窗口的条目永不可能再被命中，保留只会让表无限增长
    debounce_map.retain(|_, last| now.saturating_duration_since(*last) < window);
    let key = (group_path.to_string(), mod_path.to_string());
    if let Some(last_time) = debounce_map.get(&key) {
        if now.saturating_duration_since(*last_time) < window {
            return Ok(true);
        }
    }
    debounce_map.insert(key, now);
    Ok(false)
}

/// 「_MANAGED_ 写操作全局锁」（H1；C12 复用并扩展）。
///
/// 凡会写 `_MANAGED_` 目录（delete_group_ini_files / create_group_ini / tmp
/// rename / selectedindex 等）的写操作以此异步 Mutex 强制串行：
/// - `update_mod_data`（H1）：双击 / 连点可并发两次 update 同时写同一目标，
///   导致半成品 INI 进入 group INI 或命名冲突。锁包裹整个 spawn_blocking +
///   .await 执行期，禁止并发重入。
/// - `select_mod` / `switch_mod`（C12）：update 批量写入期间用户点选模组，
///   switch_mod 全量扫描会读到 update 写到一半的中间态目录树，并交错写
///   selectedindex。select_mod 在命令体入口获取该锁，与 update / 导入互斥，
///   同样跨 await 持锁（tokio Mutex 守卫可跨 .await 持有，不阻塞 worker）。
static UPDATE_LOCK: LazyLock<AsyncMutex<()>> = LazyLock::new(AsyncMutex::default);

/// 全局轻量扫描串行锁（singleflight，2026-08-19）。
///
/// `get_mods` cache miss 时并发调用（多入口：窗口显隐/切游戏/页面挂载）会各自执行
/// 全量 light scan（实测 ~4s×N，日志证实并发两次 "Cache miss"），既浪费 CPU 又使
/// 前端两次响应时序不定（叠加竞态）。拿锁后二次检查缓存（double-checked）：
/// 等锁期间首个扫描者已回填缓存，后续调用直接取缓存——并发 N 次只扫一次。
/// `refresh_mods`（强制重扫语义）同样串行以防并发重复扫描，但不跳过扫描。
static SCAN_LOCK: LazyLock<AsyncMutex<()>> = LazyLock::new(AsyncMutex::default);

/// 选择直写快路径的分组路径守卫：路径非空、位于 `_MANAGED_` 下且目录名为
/// 严格 group_xx（复用扫描器判定，拒绝 group_0/前导零/互斥目录/任意路径）。
///
/// `write_selectedindex_direct` 内部仍有 group_xx 守卫；此处前置判定用于
/// 无效路径回退 `switch_mod` / `deselect_group_mods` 全量路径（防御），而非直接报错。
fn is_direct_writable_group(group_path: &Path, managed_path: &Path) -> bool {
    if group_path.as_os_str().is_empty() {
        return false;
    }
    if !mod_cache::path_within(group_path, managed_path) {
        return false;
    }
    match group_path.file_name() {
        Some(name) => mod_scanner::is_normal_group_dir(&name.to_string_lossy()).is_some(),
        None => false,
    }
}

/// 选择模组命令的参数结构体
///
/// 将 select_mod 命令的 9 个独立参数封装为单个结构体，
/// 以避免 clippy::too_many_arguments 警告，并提升参数可读性。
/// 通过 #[serde(rename_all = "camelCase")] 与前端 camelCase 键名保持一致。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectModArgs {
    /// 目标游戏标识（如 "wuwa"、"genshin"）
    game: String,
    /// 模组根目录路径
    mods_path: String,
    /// 分组索引
    group_index: u32,
    /// 模组索引
    mod_index: u32,
    /// 是否为互斥组
    is_mutex: bool,
    /// 分组路径
    group_path: String,
    /// 模组路径
    mod_path: String,
    /// 屏幕光标 X 坐标（可选，像素值）
    cursor_x: Option<i32>,
    /// 屏幕光标 Y 坐标（可选，像素值）
    cursor_y: Option<i32>,
}

/// 获取模组列表（轻量扫描+缓存）
///
/// 优先从内存缓存返回，缓存未命中时执行轻量扫描。
/// 这是 UI 初始化和列表展示的主要入口。
#[tauri::command]
pub async fn get_mods(
    game: String,
    mods_path: String,
) -> Result<Arc<mod_scanner::ScanResult>, String> {
    log::debug!(
        "[commands::mod_commands] [get_mods] game={} mods_path={}",
        game,
        mods_path
    );
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    // 先在锁作用域内取出缓存结果，锁释放后再触发启动备份（fire-and-forget）
    let cached = {
        let cache = crate::core::mod_cache::MOD_CACHE.read();
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
        let cache = crate::core::mod_cache::MOD_CACHE.read();
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
    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<mod_scanner::ScanResult, String> {
            mod_scanner::scan_mods_light(&scan_path)
                .map_err(crate::core::error_normalizer::err_to_ui)
        })
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
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
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
    Ok(mod_scanner::check_mods_path(game, &mods_path))
}

/// 刷新模组列表（轻量扫描，更新缓存）
#[tauri::command]
pub async fn refresh_mods(
    game: String,
    mods_path: String,
) -> Result<Arc<mod_scanner::ScanResult>, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    log::info!("[refresh_mods] Scanning light...");
    let start = std::time::Instant::now();

    // singleflight：与 get_mods 共用 SCAN_LOCK 串行，防止并发重复全量扫描
    // （refresh 语义为强制重扫，不二次检查缓存，仅消除并发重复扫描）
    let _scan_guard = SCAN_LOCK.lock().await;

    let scan_path = mods_path.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<mod_scanner::ScanResult, String> {
            mod_scanner::scan_mods_light(&scan_path)
                .map_err(crate::core::error_normalizer::err_to_ui)
        })
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
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.set(game, &mods_path, result.clone());
    }

    Ok(Arc::new(result))
}

/// 重量级更新模组数据（仅用户按钮触发）
#[tauri::command]
pub async fn update_mod_data(
    game: String,
    mods_path: String,
) -> Result<mod_manager::UpdateResult, String> {
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
        move || -> Result<mod_manager::UpdateResult, String> {
            mod_manager::update_mod_data(game, &update_path, &settings)
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
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
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
) -> Result<mod_manager::HashConflictResult, String> {
    let _busy = crate::window::lifecycle::BusyOpGuard::new();
    let mods_path = PathBuf::from(mods_path);
    log::info!(
        "[detect_hash_conflicts] Starting full hash conflict scan: {}",
        mods_path.display()
    );
    let start = std::time::Instant::now();

    let scan_path = mods_path.clone();
    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<mod_manager::HashConflictResult, String> {
            mod_manager::detect_hash_conflicts(&scan_path)
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

/// 选择模组（支持互斥组）
#[tauri::command]
pub async fn select_mod(args: SelectModArgs) -> Result<mod_manager::UpdateResult, String> {
    // 「_MANAGED_ 写操作全局锁」（C12）：select/switch 与 update/导入互斥，
    // 防止 update 批量写入期间全量扫描读到中间态，并交错写 selectedindex。
    // 命令体入口即获取，跨下方两个分支的所有 spawn_blocking + .await 持锁。
    let _update_guard = UPDATE_LOCK.lock().await;
    let SelectModArgs {
        game,
        mods_path,
        group_index,
        mod_index,
        is_mutex,
        group_path,
        mod_path,
        cursor_x,
        cursor_y,
    } = args;
    // mod_index 归一化（与 switch_mod 的 safe_mod_index 语义保持一致）：
    // 扫描器约定 None 空槽位占据 mod_index=0，真实模组从 1 开始（$managed_slot_id 亦为 1-based）。
    // 若前端因任何路径误传 0（如收藏模式显示列表下标错位），统一转为 1，
    // 否则会出现「持久化 selectedindex=1 而按键模拟仍用 x=0」的分歧——
    // 分组 Y 坐标正确、模组 X 坐标($active_slot)为 0，导致无模组匹配而失效。
    sel_dbg!(
        "mod_commands",
        "select_mod",
        "mod_index 归一化前 | mod_index_before_normalize={}",
        mod_index
    );
    let mod_index = if mod_index == 0 { 1 } else { mod_index };
    sel_dbg!(
        "mod_commands",
        "select_mod",
        "mod_index 归一化后 | mod_index_after_normalize={}",
        mod_index
    );
    // 提取模组名称与分组名称，便于在调试日志中直接定位所属模组
    let mod_name = Path::new(&mod_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let group_name = Path::new(&group_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    sel_dbg!(
        "mod_commands",
        "select_mod",
        "入口 | 所属模组名称={} 所属分组名称={} game={} 分组索引={} 模组索引={} 是否互斥组={} 传入光标坐标=({:?},{:?}) 模组路径={} 分组路径={}",
        mod_name, group_name, game, group_index, mod_index, is_mutex, cursor_x, cursor_y, mod_path, group_path
    );
    log::debug!(
        "[commands::mod_commands] [select_mod] game={} group={} mod={} mutex={}",
        game,
        group_index,
        mod_index,
        is_mutex
    );
    let _start = std::time::Instant::now();
    let game_enum = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    sel_dbg!(
        "mod_commands",
        "select_mod",
        "分支判定 | is_mutex={} → 进入 {} 分支",
        is_mutex,
        if is_mutex {
            "互斥组(enable_mutex_mod，无按键模拟)"
        } else {
            "普通组(switch_mod 持久化 + 按键模拟)"
        }
    );
    if is_mutex {
        // Mutex 分支防抖（与非 Mutex 分支一致，防止快速重复点击触发多次互斥切换）
        // P1：复合键（分组 + 模组），同组不同模组的连续选择不再被误判为重复触发
        if selection_debounce_hit(&group_path, &mod_path)? {
            return Err("debounced".to_string());
        }

        let mod_path_buf = PathBuf::from(mod_path.clone());
        let managed_path = mods_path.join(constants::MANAGED_FOLDER);
        // 守卫：mod_path 必须位于 _MANAGED_ 下且第一段非 group_xx（互斥模组），
        // 否则禁止调用 enable_mutex_mod，避免作用于错误父目录成批禁用其下含 ini 的目录。
        if !mod_manager::is_mutex_mod(&mod_path_buf, &managed_path) {
            return Err(format!(
                "Invalid mutex mod path (not under _MANAGED_ or is a group_xx): {:?}",
                mod_path_buf
            ));
        }
        sel_dbg!(
            "mod_commands",
            "select_mod",
            "互斥组分支 | 准备调用 enable_mutex_mod | 模组名称={} 模组路径={}",
            mod_name,
            mod_path
        );

        tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            mod_manager::enable_mutex_mod(&mod_path_buf).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;

        {
            let mut cache = crate::core::mod_cache::MOD_CACHE.write();
            cache.invalidate_game(game_enum); // 彻底失效该游戏缓存（invalidate_by_prefix 是 no-op 陷阱）
        }

        Ok(mod_manager::UpdateResult {
            need_reload_manual: false,
            ..Default::default()
        })
    } else {
        // === NormalGroup 分支 ===
        // 执行顺序对齐用户要求："先持久化再按键模拟"
        // 1. 防抖检查
        // P1：复合键（分组 + 模组），同组不同模组的连续选择不再被误判为重复触发
        if selection_debounce_hit(&group_path, &mod_path)? {
            return Err("debounced".to_string());
        }

        // 2. 先持久化：优先直写快路径（O(1) 原子写 selectedindex + 缓存定向 patch，
        //    免 switch_mod 全量轻扫），group_path 无效时回退全量路径（防御）
        let settings = settings_store::get_settings();
        let simulate_enabled = settings.simulate_key_on_selection;
        let managed_path = mods_path.join(constants::MANAGED_FOLDER);
        // TargetGame 是 Copy，提前复制用于后续按键模拟
        let game_enum_for_sim = game_enum;

        let group_path_buf = PathBuf::from(&group_path);
        // P3：直写快路径同样必须按 mod_path 解析后端排序后的真实下标。
        // 前端展示序因「收藏前置 / 新增置末位」等本地近似重排与后端 mod_order_cmp
        // 分叉，直接写前端传入的 mod_index 会把 selectedindex 落到错误模组上。
        // 缓存快照即最近一次后端扫描结果（权威后端序，无磁盘 I/O，保持 O(1) 意图）；
        // 缓存缺失 / 路径未命中时回退到传入下标（行为不劣化）。
        let resolved_index = mod_manager::resolve_mod_index_by_path_from_cache(
            game_enum,
            &mods_path,
            &group_path_buf,
            &mod_path,
        );
        let direct_index = resolved_index.unwrap_or(mod_index);
        if let Some(r) = resolved_index {
            if r != mod_index {
                log::warn!(
                    "[select_mod] direct path: mod_index corrected by mod_path | frontend={} backend={} mod_path={}",
                    mod_index,
                    r,
                    mod_path
                );
            }
        }
        let result = if is_direct_writable_group(&group_path_buf, &managed_path) {
            sel_dbg!(
                "mod_commands",
                "select_mod",
                "普通组直写快路径 | group_path={} mod_index={}（免全量轻扫）",
                group_path,
                direct_index
            );
            // O(1) 原子写 selectedindex（内部含 group_xx 守卫与 WatcherGuard）
            let write_path = group_path_buf.clone();
            let write_start = std::time::Instant::now();
            tauri::async_runtime::spawn_blocking(move || {
                mod_manager::write_selectedindex_direct(&write_path, direct_index)
            })
            .await
            .map_err(|e| {
                log::error!("[select_mod] spawn_blocking join error: {}", e);
                format!("switch task failed: {}", e)
            })?
            .map_err(|e| {
                log::error!("[select_mod] write_selectedindex_direct error: {}", e);
                e.to_string()
            })?;
            log::debug!(
                "[select_mod] direct write_selectedindex done | elapsed_ms={}",
                write_start.elapsed().as_millis()
            );
            // 缓存定向 patch：命中则免整缓存失效；未命中不失效（watcher / 下次全量扫描对账）
            // 注意须用按路径解析后的 direct_index（与磁盘写入值同源），否则缓存与磁盘不一致。
            //
            // 改为 try_write（非阻塞）：实测启用模组的目录重命名必然触发 watcher 增量合并，
            // 其 `subtree_replace` 写锁段在 6542 模组/109 分组下长达数秒；此处若用阻塞
            // write() 会排在它后面空等（实测 6.7s），连带把后续按键模拟（游戏内真正生效的
            // 动作）一起推迟，用户感知为「启用后选择延迟 ~7 秒」。
            // 拿不到锁即跳过本次 patch —— 降级语义是现成的：磁盘 selectedindex 已由
            // write_selectedindex_direct 写成功，缓存对账由 watcher 增量 / 下次全量扫描完成。
            // MOD_CACHE 为 parking_lot::RwLock：try_write() 返回 Option（持锁时为 None）
            let patched = match mod_cache::MOD_CACHE.try_write() {
                Some(mut cache) => {
                    cache.patch_selected_index(game_enum, &mods_path, &group_path_buf, direct_index)
                }
                None => {
                    log::warn!(
                        "[select_mod] cache patch skipped: MOD_CACHE write lock busy (watcher merge in progress); \
                         selectedindex already persisted, cache reconciled by watcher/next full scan"
                    );
                    false
                }
            };
            log::debug!(
                "[commands::mod_commands] [select_mod] direct write done | patched={} selected={}",
                patched,
                direct_index
            );
            mod_manager::UpdateResult {
                selected_mod_index: Some(direct_index as i32),
                ..Default::default()
            }
        } else {
            sel_dbg!(
                "mod_commands",
                "select_mod",
                "普通组回退全量路径 | group_path={} 无效，走 switch_mod 全量轻扫",
                group_path
            );
            // P3：把前端 mod_path 透传给 switch_mod，使其按路径解析后端排序后的真实
            // mod_index（前端近似重排后的整数下标不可信），路径未命中时后端自动回退。
            let mod_path_for_switch = mod_path.clone();
            let result = tauri::async_runtime::spawn_blocking(
                move || -> Result<mod_manager::UpdateResult, String> {
                    mod_manager::switch_mod(
                        game_enum,
                        &mods_path,
                        &settings,
                        group_index,
                        mod_index,
                        Some(mod_path_for_switch.as_str()),
                    )
                    .map_err(|e| e.to_string())
                },
            )
            .await
            .map_err(|e| {
                log::error!("[select_mod] spawn_blocking join error: {}", e);
                format!("switch task failed: {}", e)
            })?
            .map_err(|e| {
                log::error!("[select_mod] switch_mod error: {}", e);
                e
            })?;

            // 整缓存失效（旧行为，确保下次 get_mods 全量扫描）
            {
                let mut cache = mod_cache::MOD_CACHE.write();
                cache.invalidate_game(game_enum); // 彻底失效该游戏缓存（invalidate_by_prefix 是 no-op 陷阱）
            }
            log::debug!(
                "[commands::mod_commands] [select_mod] cache invalidated | game={}",
                game.as_str()
            );
            result
        };
        log::debug!(
            "[commands::mod_commands] [select_mod] switch_result={:?}",
            result
        );

        // 4. 再按键模拟（确保文件状态已就绪）
        // 对齐 NRMM：选择后不发送 F10，$active_slot 通过 persist 跨重载保持，
        // 3Dmigoto 会在下次自然重载时重新求值 if/endif 条件
        log::debug!(
            "[commands::mod_commands] [select_mod] simulate_key_on_selection={} (after persist)",
            simulate_enabled
        );
        // 选择逻辑(按键模拟)触发前：记录当前光标位置（屏幕坐标），用于对比模拟前后变化
        let cursor_before = crate::platform::get_foreground_detector()
            .get_cursor_position()
            .ok();
        sel_dbg!(
            "mod_commands",
            "select_mod",
            "选择逻辑(按键模拟)触发前光标位置={:?}（屏幕坐标，simulate_key_on_selection={}）",
            cursor_before,
            simulate_enabled
        );
        if simulate_enabled {
            let mut simulator = crate::platform::get_key_simulator();
            let process_names = game_enum_for_sim.process_names();
            for pn in process_names {
                if simulator.set_target_process(pn).is_ok() {
                    break;
                }
            }
            // 参数语义对齐NRMM：优先使用调用方传入的屏幕坐标（cursor_x/cursor_y 为实际像素时），
            // 否则 fallback 到 (mod_index, group_index) 作为虚拟坐标（3Dmigoto/xxmi 据此识别）。
            // 注意：NRMM 将 x=realModIndex y=realGroupIndex 直接传入 SetCursorPos，
            // 因此当未传像素坐标时，索引值本身即作为坐标输入。
            // P3 合并修正：x 坐标必须与磁盘写入的 selectedindex **同源**——
            // 取本命令两条路径（直写快路径 / switch_mod 全量路径）返回的权威
            // selected_mod_index（均已按 mod_path 校正），否则会出现「持久化 X 而
            // 按键模拟用旧 x」的分歧：分组 Y 坐标正确、模组 X 坐标无模组匹配而失效。
            let sim_mod_index = result
                .selected_mod_index
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(mod_index);
            let (sim_g, sim_m) = match (cursor_x, cursor_y) {
                (Some(px), Some(py)) => {
                    let g = u32::try_from(py.max(0)).unwrap_or(group_index);
                    let m = u32::try_from(px.max(0)).unwrap_or(sim_mod_index);
                    (g, m)
                }
                _ => (group_index, sim_mod_index),
            };
            sel_dbg!(
                "mod_commands",
                "select_mod",
                "坐标传递 | cursor_x_input={:?} cursor_y_input={:?} sim_g={} sim_m={} coordinate_mode={} 模组名称={}",
                cursor_x,
                cursor_y,
                sim_g,
                sim_m,
                if cursor_x.is_some() && cursor_y.is_some() { "屏幕像素坐标" } else { "索引虚拟坐标" },
                mod_name
            );
            let sim_start = std::time::Instant::now();
            let result: Result<(), String> = simulator
                .simulate_select_full(sim_g, sim_m)
                .map_err(|e| e.to_string());
            let sim_elapsed = sim_start.elapsed();
            sel_dbg!(
                "mod_commands",
                "select_mod",
                "simulate_select_full 调用完成 | sim_g={} sim_m={} 耗时={}μs result={:?}",
                sim_g,
                sim_m,
                sim_elapsed.as_micros(),
                result
            );
            log::debug!(
                "[commands::mod_commands] [select_mod] simulate_result={:?}",
                result
            );
            if let Err(e) = result {
                log::warn!(
                    "select_mod: simulate_select_full failed (g={}, m={}): {}",
                    group_index,
                    mod_index,
                    e
                );
            }
        }

        // 选择逻辑(按键模拟)触发后：再次记录光标位置，与触发前对比，验证光标是否被正确还原
        let cursor_after = crate::platform::get_foreground_detector()
            .get_cursor_position()
            .ok();
        sel_dbg!(
            "mod_commands",
            "select_mod",
            "选择逻辑(按键模拟)触发后光标位置={:?}（屏幕坐标）| 触发前={:?} 是否一致={}",
            cursor_after,
            cursor_before,
            cursor_before == cursor_after
        );

        log::debug!("[commands::mod_commands] [select_mod] completed | elapsed={:?}ms | selected_mod_index={:?}", _start.elapsed().as_millis(), result.selected_mod_index);
        Ok(result)
    }
}

/// 取消选中分组内模组
///
/// `group_path` 提供且有效（位于 `_MANAGED_` 下 + group_xx）时走直写快路径：
/// O(1) 原子写 selectedindex=0 + 缓存定向 patch，免全量轻扫与整缓存失效；
/// 未提供或无效时回退 `deselect_group_mods` 全量扫描路径（旧行为）。
#[tauri::command]
pub async fn deselect_group_mod(
    game: String,
    mods_path: String,
    group_index: u32,
    group_path: Option<String>,
) -> Result<mod_manager::UpdateResult, String> {
    // P2：「_MANAGED_ 写操作全局锁」覆盖不全的补齐。取消选择同样写 selectedindex
    // 标记文件，此前游离于锁外，可与 select_mod / update_mod_data 并发交错。
    let _update_guard = UPDATE_LOCK.lock().await;
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let managed_path = mods_path.join(constants::MANAGED_FOLDER);

    let group_path_buf = group_path.map(PathBuf::from);
    if let Some(gp) = group_path_buf.filter(|gp| is_direct_writable_group(gp, &managed_path)) {
        sel_dbg!(
            "mod_commands",
            "deselect_group_mod",
            "直写快路径 | group_path={} group_index={}（写 selectedindex=0，免全量轻扫）",
            gp.display(),
            group_index
        );
        let write_path = gp.clone();
        tauri::async_runtime::spawn_blocking(move || {
            mod_manager::write_selectedindex_direct(&write_path, 0)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| {
            log::error!(
                "[deselect_group_mod] write_selectedindex_direct error: {}",
                e
            );
            e.to_string()
        })?;
        // 同 select_mod：非阻塞 try_write，避免排在 watcher 增量合并的写锁段后空等
        // （selectedindex=0 已落盘，缓存对账交给 watcher / 下次全量扫描）
        let patched = match mod_cache::MOD_CACHE.try_write() {
            Some(mut cache) => cache.patch_selected_index(game, &mods_path, &gp, 0),
            None => {
                log::warn!(
                    "[deselect_group_mod] cache patch skipped: MOD_CACHE write lock busy (watcher merge in progress)"
                );
                false
            }
        };
        log::debug!(
            "[commands::mod_commands] [deselect_group_mod] direct write done | patched={}",
            patched
        );
        return Ok(mod_manager::UpdateResult {
            selected_mod_index: Some(0),
            ..Default::default()
        });
    }

    // 回退：全量轻扫定位分组（旧行为，含整缓存失效）
    let settings = settings_store::get_settings();
    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<mod_manager::UpdateResult, String> {
            mod_manager::deselect_group_mods(game, &mods_path, &settings, group_index)
                .map_err(|e| e.to_string())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = mod_cache::MOD_CACHE.write();
        cache.invalidate_game(game); // 彻底失效该游戏缓存（invalidate_by_prefix 是 no-op 陷阱）
    }

    Ok(result)
}

/// 添加分组
#[tauri::command]
pub async fn add_group(
    game: String,
    mods_path: String,
    group_name: Option<String>,
) -> Result<mod_manager::UpdateResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<mod_manager::UpdateResult, String> {
            let managed_folder = mods_path.join(constants::MANAGED_FOLDER);
            if !managed_folder.exists() {
                fs::create_dir_all(&managed_folder).map_err(|e| e.to_string())?;
            }

            let mut used_numbers: Vec<u32> = Vec::new();
            if let Ok(entries) = fs::read_dir(&managed_folder) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if let Some(rest) = name.strip_prefix("group_") {
                        if let Ok(num) = rest.parse::<u32>() {
                            used_numbers.push(num);
                        }
                    }
                }
            }
            used_numbers.sort();
            let mut group_num = 1u32;
            for used in &used_numbers {
                if *used == group_num {
                    group_num += 1;
                } else if *used > group_num {
                    break;
                }
            }

            // 对齐 NRMM 约定：分组上限 = 屏幕 Y 轴（高度）上限，由动态分辨率上限决定。
            // Dart 以 500 作为硬编码安全上限；NRMM 实际约定为基于主屏幕分辨率的动态上限
            // （group=y / mod=x），故此处采用 resolution::compute_limits().max_groups 作为权威上限。
            let max_groups = resolution::compute_limits().max_groups;
            if group_num > max_groups {
                return Err(format!(
                    "分组数量已达动态上限（{}，基于屏幕分辨率 Y 轴）",
                    max_groups
                ));
            }

            let dir_name = format!("group_{}", group_num);
            let group_path = managed_folder.join(&dir_name);
            fs::create_dir(&group_path).map_err(|e| e.to_string())?;

            let template_str = String::from_utf8_lossy(crate::resources::TEMPLATE_GROUP);
            let ini_content = template_str
                .replace("{group_x}", &dir_name)
                .replace("{x}", &group_num.to_string());
            let ini_path = group_path.join("ModFolder.ini");
            crate::utils::atomic_write(&ini_path, ini_content.as_bytes())
                .map_err(|e| e.to_string())?;

            if let Some(custom_name) = group_name {
                let trimmed = custom_name.trim();
                if !trimmed.is_empty() && trimmed != dir_name {
                    let new_group_path = managed_folder.join(trimmed);
                    if !new_group_path.exists() {
                        fs::rename(&group_path, &new_group_path).map_err(|e| e.to_string())?;
                    }
                }
            }

            Ok(mod_manager::UpdateResult::default())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    let _ = game;
    Ok(result)
}

/// 删除分组（移至回收站）
///
/// 命令级增量（R1）：分组目录已删除无需磁盘重扫，直接 `subtree_remove`
/// 裁剪缓存，返回 `new_group = None` 的 [`GroupDelta`]（前端按 oldGroupPath
/// 剔除该组及其模组条目），不再触发全量失效与全量刷新。
#[tauri::command]
pub async fn remove_group(group_path: String) -> Result<GroupDelta, String> {
    // P2：删除分组会移走 _MANAGED_ 下整棵子树，须与选择、更新串行
    // （否则「删除分组后立刻选择」会出现组已消失但选择仍乐观成功的静默失败）。
    let _update_guard = UPDATE_LOCK.lock().await;
    let (mods_root, group_dir) = resolve_managed_context(Path::new(&group_path))
        .ok_or_else(|| format!("Path is not inside a managed group: {}", group_path))?;
    let path = settings_store::validate_managed_path(&group_path).map_err(|e| e.to_string())?;
    let game = settings_store::get_settings().target_game;

    let delta = tauri::async_runtime::spawn_blocking(move || -> Result<GroupDelta, String> {
        // 抑制 watcher 批次在删除/裁剪中途读取中间态（C11：暂停期事件保留，恢复后幂等合并）
        let _guard = crate::core::file_watcher::WatcherGuard::new();
        if !path.exists() {
            return Err("Group path does not exist".to_string());
        }
        trash_delete(&path).map_err(|e| e.to_string())?;
        let delta = build_removed_group_delta(game, &mods_root, &group_dir);
        log_delta("remove_group", &delta);
        Ok(delta)
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(delta)
}

/// 移除模组（NRMM 对齐：移至 DISABLED_MANAGED_REMOVED + 还原 INI）
///
/// 完整流程（后端移除逻辑不变）：
/// 1. 将模组文件夹移动到 `Mods/DISABLED_MANAGED_REMOVED/<modname>`（冲突追加 _1、_2…）
/// 2. 对移动后的模组执行 INI 还原（清理所有 NRMM 注入内容）
/// 3. 零扫描：MOD_CACHE 模组条目直接剔除（组树 + 扁平列表），前后端索引同步清除
/// 4. 缓存条目未命中（异常）→ 降级对受影响分组局部重扫 + `subtree_replace` 合并缓存，
///    `degraded=true` 返回完整组增量（前端 patch 后告知用户发生异常）
#[tauri::command]
pub async fn remove_mod(
    mod_path: String,
) -> Result<crate::core::mod_manager::RemoveModResult, String> {
    // P2：移除模组会把目录移入 DISABLED_MANAGED_REMOVED 并改写组内 INI，
    // 与 select_mod / update_mod_data 同属 _MANAGED_ 写路径，须统一串行化。
    let _update_guard = UPDATE_LOCK.lock().await;
    let (mods_root, old_group_dir) = resolve_managed_context(Path::new(&mod_path))
        .ok_or_else(|| format!("Path is not inside a managed group: {}", mod_path))?;
    let path = settings_store::validate_managed_path(&mod_path).map_err(|e| e.to_string())?;
    let game = settings_store::get_settings().target_game;

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::core::mod_manager::RemoveModResult, String> {
            // 抑制 watcher 批次在移动/还原中途读取中间态（C11：暂停期事件保留，恢复后幂等合并）
            let _guard = crate::core::file_watcher::WatcherGuard::new();
            let mut result =
                crate::core::mod_manager::remove_mod(&path).map_err(|e| e.to_string())?;
            // 零扫描缓存剔除：组树 + 扁平列表移除该模组条目；未命中（缓存无此条）→ 降级增量重扫
            let mut degraded = false;
            {
                let mut cache = crate::core::mod_cache::MOD_CACHE.write();
                if !cache.remove_mod_entry(game, &mods_root, &path) {
                    degraded = true;
                }
            } // 释放写锁
            if degraded {
                // 局部重扫入口：mod 目录已被移走（旧路径不存在），传 parent（分组目录，
                // 操作前后恒存在）——scan_partial_path 会扩展到所属分组
                let rescan_dir = path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| old_group_dir.clone());
                result.group_delta =
                    build_group_delta(game, &mods_root, &old_group_dir, &rescan_dir)
                        .map_err(|e| e.to_string())?;
                log_delta("remove_mod(degraded)", &result.group_delta);
            }
            result.degraded = degraded;
            Ok(result)
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    Ok(result)
}

/// 重命名模组
///
/// 命令级增量（R1）：重命名（或写 modname 标记文件）后对受影响分组局部重扫，
/// 返回 [`GroupDelta`]（新模组路径可从 delta.new_group 内获取），不再触发
/// 全量失效与全量刷新。
#[tauri::command]
pub async fn rename_mod(mod_path: String, new_name: String) -> Result<GroupDelta, String> {
    // P2：重命名会写 modname 标记文件 / 重命名 _MANAGED_ 下目录，须与选择、更新串行。
    let _update_guard = UPDATE_LOCK.lock().await;
    let (mods_root, old_group_dir) = resolve_managed_context(Path::new(&mod_path))
        .ok_or_else(|| format!("Path is not inside a managed group: {}", mod_path))?;
    let path = settings_store::validate_managed_path(&mod_path).map_err(|e| e.to_string())?;
    settings_store::validate_filename(&new_name).map_err(|e| e.to_string())?;
    let game = settings_store::get_settings().target_game;

    let delta = tauri::async_runtime::spawn_blocking(move || -> Result<GroupDelta, String> {
        // 抑制 watcher 批次在重命名/写标记文件中途读取中间态
        let _guard = crate::core::file_watcher::WatcherGuard::new();
        if !path.exists() {
            return Err("Mod path does not exist".to_string());
        }
        let parent = path
            .parent()
            .ok_or_else(|| "Invalid mod path".to_string())?;
        let parent_name = parent
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // NRMM 逻辑：group_xx 普通分组下的模组，重命名仅修改 modname 标记文件（展示名），文件夹名保持不变
        let new_path = if mod_scanner::is_group_xx_dir(&parent_name) {
            let modname_path = path.join("modname");
            crate::utils::atomic_write(&modname_path, new_name.as_bytes())
                .map_err(|e| format!("Failed to write modname file: {}", e))?;
            path.clone()
        } else {
            // 其余情况（互斥组等）：重命名文件夹
            let dir_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_disabled = mod_scanner::is_disabled_dir(&dir_name);
            let final_name = if is_disabled {
                format!(
                    "{}{}",
                    constants::DISABLED_PREFIX,
                    mod_scanner::strip_disabled_prefix(&new_name)
                )
            } else {
                new_name.clone()
            };
            let new_path = parent.join(final_name);
            fs::rename(&path, &new_path).map_err(|e| e.to_string())?;
            new_path
        };

        // 局部重扫入口（须为操作后存在的目录）：
        // - group_xx 分支 → mod_path（modname 文件变化，mod 目录未动）
        // - mutex 分支 → 重命名后的 mod 目录
        // 两分支返回值恰好即各自场景的存在目录，直接以其为重扫入口
        let delta = build_group_delta(game, &mods_root, &old_group_dir, &new_path)
            .map_err(|e| e.to_string())?;
        log_delta("rename_mod", &delta);
        Ok(delta)
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(delta)
}

/// 重命名分组
///
/// 根据分组类型采用不同策略：
/// - group_xx（is_group_xx=true）：修改 groupname 文件内容（无则创建）
/// - 非group（is_group_xx=false）：重命名目录
///
/// 路径不存在时返回异常。
///
/// 命令级增量（R1）：重命名后对受影响分组局部重扫（group_xx 分支重扫原
/// 分组目录、非 group 分支重扫新组目录），返回 [`GroupDelta`]（分组自身
/// 重命名时 oldGroupPath 与 newGroup.groupPath 不同，前端按旧键定位替换）。
#[tauri::command]
pub async fn rename_group(
    group_path: String,
    new_name: String,
    is_group_xx: bool,
) -> Result<GroupDelta, String> {
    // P2：写 groupname 标记文件 / 重命名 _MANAGED_ 下分组目录，须与选择、更新串行。
    let _update_guard = UPDATE_LOCK.lock().await;
    let (mods_root, old_group_dir) = resolve_managed_context(Path::new(&group_path))
        .ok_or_else(|| format!("Path is not inside a managed group: {}", group_path))?;
    let path = settings_store::validate_managed_path(&group_path).map_err(|e| e.to_string())?;
    settings_store::validate_filename(&new_name).map_err(|e| e.to_string())?;
    let game = settings_store::get_settings().target_game;

    let delta = tauri::async_runtime::spawn_blocking(move || -> Result<GroupDelta, String> {
        // 抑制 watcher 批次在重命名/写标记文件中途读取中间态
        let _guard = crate::core::file_watcher::WatcherGuard::new();
        if !path.exists() {
            return Err("Group path does not exist".to_string());
        }

        // 局部重扫入口（须为操作后存在的目录）：
        // - group_xx 分支 → path（仅 groupname 文件变化，目录未动）
        // - 非 group 分支 → 新组目录（旧目录已重命名不存在）
        let rescan_dir = if is_group_xx {
            let groupname_path = path.join("groupname");
            crate::utils::atomic_write(&groupname_path, new_name.as_bytes())
                .map_err(|e| format!("Failed to write groupname file: {}", e))?;
            path.clone()
        } else {
            let parent = path
                .parent()
                .ok_or_else(|| "Invalid group path".to_string())?;
            let target = parent.join(&new_name);
            if target.exists() {
                return Err(format!(
                    "Target directory already exists: {}",
                    target.display()
                ));
            }
            fs::rename(&path, &target).map_err(|e| e.to_string())?;
            target
        };

        let delta = build_group_delta(game, &mods_root, &old_group_dir, &rescan_dir)
            .map_err(|e| e.to_string())?;
        log_delta("rename_group", &delta);
        Ok(delta)
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(delta)
}

/// 单模组启用/禁用结果（零扫描路径；degraded=true 时前端消费 group_delta 并提示异常）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToggleModResult {
    /// 操作后模组目录绝对路径（rename 后；幂等未改名时为原路径）
    pub new_path: String,
    /// 操作后的禁用状态（true=已禁用）
    pub disabled: bool,
    /// 是否降级为增量扫描（缓存条目未命中等异常）；true 时请用 `group_delta` 整体 patch 并告知用户
    pub degraded: bool,
    /// 异常降级时的受影响分组增量（degraded=false 时为空占位）
    pub group_delta: GroupDelta,
}

/// 切换单模组启用/禁用状态
///
/// 零扫描（对齐 NRMM `completeDisableMod`/`enableMod`）：仅目录重命名（`DISABLED_`
/// 前缀增删）+ MOD_CACHE 条目就地 patch，不触发任何扫描/重载。
/// 异常（缓存条目未命中 / 重命名失败）→ 降级组级增量重扫，`degraded=true` +
/// `group_delta`（前端 patch 后告知用户发生异常）。
#[tauri::command]
pub async fn toggle_mod_disabled(
    mod_path: String,
    enable: bool,
    is_mutex: bool,
) -> Result<ToggleModResult, String> {
    // P2：启停通过 DISABLED 前缀重命名改写 _MANAGED_ 目录树。此前未持锁，
    // 与 select_mod（写 selectedindex + 清缓存）可并发交错，是「选择 vs 启用状态」
    // 竞态的核心缺口——统一在此入口串行化。
    let _update_guard = UPDATE_LOCK.lock().await;
    let (mods_root, old_group_dir) = resolve_managed_context(Path::new(&mod_path))
        .ok_or_else(|| format!("Path is not inside a managed group: {}", mod_path))?;
    let path = settings_store::validate_managed_path(&mod_path).map_err(|e| e.to_string())?;
    let game = settings_store::get_settings().target_game;

    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<ToggleModResult, String> {
            // 抑制 watcher 批次在目录重命名中途读取中间态（C11：暂停期事件保留，恢复后幂等合并）
            let _guard = crate::core::file_watcher::WatcherGuard::new();
            if is_mutex {
                if enable {
                    mod_manager::enable_mutex_mod(&path).map_err(|e| e.to_string())?;
                } else {
                    mod_manager::disable_mutex_mod(&path).map_err(|e| e.to_string())?;
                }
            } else {
                mod_manager::toggle_mod(&path, enable).map_err(|e| e.to_string())?;
            }
            // 推导操作后路径（与 toggle_mod 同口径；幂等时返回原路径）
            let new_path = derive_toggled_path(&path, enable);
            // 操作后的禁用态：enable=true（目标启用）→ 新禁用态 false；enable=false（目标禁用）→ true。
            // 注意不可直接传 `enable`（它是"目标启用"语义；曾以反相交入缓存导致启用后缓存态为禁用、
            // 禁用后为启用的三方一致性污染）。
            let target_disabled = !enable;
            // 零扫描缓存 patch；未命中（缓存无此条目/组树无定位）→ 降级增量重扫
            let mut degraded = false;
            {
                let mut cache = crate::core::mod_cache::MOD_CACHE.write();
                if !cache.patch_mod_entry(game, &mods_root, &path, &new_path, target_disabled) {
                    degraded = true;
                }
            } // 释放写锁
            let group_delta = if degraded {
                build_group_delta(
                    game,
                    &mods_root,
                    &old_group_dir,
                    &path
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| old_group_dir.clone()),
                )
                .map_err(|e| e.to_string())?
            } else {
                GroupDelta::default()
            };
            if degraded {
                log_delta("toggle_mod_disabled(degraded)", &group_delta);
            }
            Ok(ToggleModResult {
                new_path: new_path.to_string_lossy().to_string(),
                disabled: target_disabled,
                degraded,
                group_delta,
            })
        })
        .await
        .map_err(|e| e.to_string())??;

    Ok(result)
}

/// 推导启用/禁用后模组目录路径（与 `mod_manager::toggle_mod` 的重命名口径一致：
/// 禁用 = 前缀 `DISABLED`；启用 = 去除 `DISABLED` 前缀；幂等命中时返回原路径）。
fn derive_toggled_path(path: &Path, enable: bool) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = path.parent().unwrap_or(path);
    if enable {
        let stripped = mod_scanner::strip_disabled_prefix(&name);
        if stripped == name || stripped.is_empty() {
            path.to_path_buf() // 已启用（幂等）
        } else {
            parent.join(stripped)
        }
    } else if mod_scanner::is_disabled_dir(&name) {
        path.to_path_buf() // 已禁用（幂等）
    } else {
        parent.join(format!("{}{}", constants::DISABLED_PREFIX, name))
    }
}

/// 切换模组收藏状态
///
/// 设计说明（R1 命令级增量）：fav 是模组目录内的空标记文件（创建/删除），
/// 不产生目录重命名或路径变化——**不做局部重扫也不返回组 delta**：
/// - 前端按返回的 `bool` 本地翻转 `isFavorite`（单条目状态，无级联）；
/// - 缓存由 fav 文件的 watcher 事件增量修正（事件收敛到 mod 目录 →
///   `scan_partial_path` 整组局部重扫，`subtree_replace` 幂等合并）。
#[tauri::command]
pub async fn toggle_favorite(mod_path: String) -> Result<bool, String> {
    let path = settings_store::validate_managed_path(&mod_path).map_err(|e| e.to_string())?;

    let is_fav = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
        let fav_path = path.join(constants::FAV_MARKER);
        if fav_path.exists() {
            fs::remove_file(&fav_path).map_err(|e| e.to_string())?;
            Ok(false)
        } else {
            crate::utils::atomic_write(&fav_path, b"").map_err(|e| e.to_string())?;
            Ok(true)
        }
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(is_fav)
}

#[tauri::command]
pub fn is_favorite(mod_path: String) -> bool {
    PathBuf::from(mod_path).join(constants::FAV_MARKER).exists()
}

/// 错误前缀标识：路径不存在，前端捕获后需执行清除缓存+重读模组
const ERR_PREFIX_PATH_NOT_FOUND: &str = "[PATH_NOT_FOUND]";

#[tauri::command]
pub fn open_mod_folder(mod_path: String) -> Result<(), String> {
    let path = PathBuf::from(&mod_path);
    if !path.exists() {
        // 路径不存在 → 清除后端模组缓存，返回带标识的错误通知前端刷新数据
        let mut cache = mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
        drop(cache);
        log::warn!("Open mod folder: path not found, invalidated cache and requesting frontend refresh: {:?}", path);
        return Err(format!(
            "{}{}",
            ERR_PREFIX_PATH_NOT_FOUND, "Path does not exist"
        ));
    }
    open_path_in_explorer(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_group_folder(group_path: String) -> Result<(), String> {
    let path = PathBuf::from(&group_path);
    if !path.exists() {
        // 路径不存在 → 清除后端模组缓存，返回带标识的错误通知前端刷新数据
        let mut cache = mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
        drop(cache);
        log::warn!("Open group folder: path not found, invalidated cache and requesting frontend refresh: {:?}", path);
        return Err(format!(
            "{}{}",
            ERR_PREFIX_PATH_NOT_FOUND, "Path does not exist"
        ));
    }
    open_path_in_explorer(&path).map_err(|e| e.to_string())
}

/// 恢复所有INI文件备份
#[tauri::command]
pub async fn restore_all_inis(mods_path: String) -> Result<mod_manager::RestoredCount, String> {
    let _busy = crate::window::lifecycle::BusyOpGuard::new();
    let path = PathBuf::from(&mods_path);

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<mod_manager::RestoredCount, String> {
            mod_manager::restore_all_inis(&path).map_err(|e| e.to_string())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(result)
}

/// 还原模组 INI 到管理前状态（还原区功能）
///
/// 对用户拖拽/选择的目录递归清理 NRMM 管理器注入的专属行，
/// 使模组无需模组管理器即可在 3Dmigoto 中使用。
///
/// # 参数
/// - `path`: 要还原的模组文件夹路径
///
/// # 返回值
/// 返回 `RestoreManagedResult`，包含路径、INI 处理数、失败数与成功标志。
///
/// # 错误
/// - 路径不存在（带 `[PATH_NOT_FOUND]` 前缀，供前端识别）
#[tauri::command]
pub async fn restore_managed_folder(
    path: String,
) -> Result<mod_manager::RestoreManagedResult, String> {
    let path_buf = settings_store::validate_managed_path(&path).map_err(|e| e.to_string())?;
    if !path_buf.exists() {
        log::warn!("Restore managed folder: path not found: {:?}", path_buf);
        return Err(format!(
            "{}{}",
            ERR_PREFIX_PATH_NOT_FOUND, "Path does not exist"
        ));
    }

    let result =
        tauri::async_runtime::spawn_blocking(move || -> mod_manager::RestoreManagedResult {
            let (ini_count, failed_count) = mod_manager::restore_managed_mod(&path_buf);
            mod_manager::RestoreManagedResult {
                path: path_buf,
                ini_count,
                failed_count,
                success: failed_count == 0,
            }
        })
        .await
        .map_err(|e| e.to_string())?;

    Ok(result)
}

/// Save Mod Customizations：保存用户自定义INI设置到d3dx_user.ini
#[tauri::command]
pub async fn save_customizations(
    game: String,
    mods_path: String,
) -> Result<mod_manager::SaveCustomizationsResult, String> {
    let _busy = crate::window::lifecycle::BusyOpGuard::new();
    let game = parse_game(&game)?;
    let path = PathBuf::from(&mods_path);

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<mod_manager::SaveCustomizationsResult, String> {
            mod_manager::save_customizations(&path, game).map_err(|e| e.to_string())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    Ok(result)
}

/// 批量切换模组启用/禁用状态
#[tauri::command]
pub async fn batch_toggle_mods(
    mod_paths: Vec<String>,
    enable: bool,
    is_mutex: bool,
) -> Result<u32, String> {
    // P2：批量启停会重命名 _MANAGED_ 下多个模组目录，须与选择、更新串行。
    let _update_guard = UPDATE_LOCK.lock().await;
    let count = tauri::async_runtime::spawn_blocking(move || -> Result<u32, String> {
        mod_manager::batch_toggle_mods(&mod_paths, enable, is_mutex).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(count)
}

/// 模拟 F10 按键（3Dmigoto 重载快捷键）
///
/// 与 NRMM 的 simulateKeyF10() 对齐，用于 update_mod_data 完成后
/// 需要手动重载 3Dmigoto 时向游戏发送 F10 按键。
/// 若传入 game 参数则尝试定向发送到目标游戏窗口，否则全局发送。
///
/// 若用户在设置中关闭了 `simulate_key_on_selection`（触发选择功能），
/// 则跳过 F10 发送，避免关闭后仍触发刷新的异常。
#[tauri::command]
pub async fn simulate_f10(game: Option<String>) -> Result<(), String> {
    let settings = settings_store::get_settings();
    sel_dbg!(
        "mod_commands",
        "simulate_f10",
        "入口 | game={:?} simulate_key_on_selection={}",
        game,
        settings.simulate_key_on_selection
    );
    if !settings.simulate_key_on_selection {
        log::debug!("[simulate_f10] simulate_key_on_selection=false, 跳过 F10 发送");
        sel_dbg!(
            "mod_commands",
            "simulate_f10",
            "已跳过 F10 发送（simulate_key_on_selection=false）"
        );
        return Ok(());
    }
    let game_for_log = game.clone();

    let mut simulator = crate::platform::get_key_simulator();
    if let Some(game_str) = game {
        if let Ok(game_enum) = parse_game(&game_str) {
            let process_names = game_enum.process_names();
            if let Some(first_pn) = process_names.first() {
                let _ = simulator.set_target_process(first_pn);
            }
        }
    }
    simulator.simulate_f10().map_err(|e| e.to_string())?;
    sel_dbg!(
        "mod_commands",
        "simulate_f10",
        "已发送 F10 重载按键（目标游戏={:?}）",
        game_for_log
    );
    Ok(())
}

fn parse_game(game: &str) -> Result<TargetGame, String> {
    match game.to_lowercase().as_str() {
        "genshinimpact" | "genshin" | "gi" => Ok(TargetGame::GenshinImpact),
        "honkaistarrail" | "starrail" | "hsr" => Ok(TargetGame::HonkaiStarRail),
        "wuwa" | "wutheringwaves" | "wuthering waves" => Ok(TargetGame::Wuwa),
        "zzz" | "zenlesszonezero" => Ok(TargetGame::ZZZ),
        "honkaiimpact3rd" | "honkaiimpact3" | "hi3" => Ok(TargetGame::HonkaiImpact3rd),
        "arknightsendfield" | "endfield" | "af" | "arknights endfield" => {
            Ok(TargetGame::ArknightsEndfield)
        }
        _ => Err(format!("Unknown game: {}", game)),
    }
}

// =========================================================================
// 命令级 Delta 更新（R1）
//
// 目标：toggle/remove/rename/批量组命令不再 invalidate_all 全量失效缓存，
// 而是对受影响分组局部重扫（scan_partial_path 自动扩展到 _MANAGED_ 一级
// 子目录粒度）、subtree_replace 就地合并缓存，并返回 GroupDelta（该组
// 完整新状态）供前端原地替换——零全量重扫、零全量 IPC。
//
// 缓存键取 settings.target_game（与 watcher 增量链路一致）；mods_root 从
// 命令传入路径解析并保留原始形态（与 get_mods 前端入参形态一致），保证
// 局部重扫产物中的 group_path 能与缓存条目按路径匹配（原位合并的前提）。
// =========================================================================

/// 从 `_MANAGED_` 内路径解析（Mods 根目录, 受影响顶层分组目录）。
///
/// 遍历路径组件定位 `_MANAGED_` 段：其前缀即 Mods 根目录，其后第一段即
/// 受影响分组（与 `scan_partial_path` 的收敛粒度一致）。纯组件解析，
/// 不要求路径存在；保留传入路径的原始形态（见上方区块说明）。
///
/// # 参数
/// - `path`: `_MANAGED_` 目录内任意层级的路径
///
/// # 返回
///
/// `(mods_root, group_dir)` 二元组；路径不含 `_MANAGED_` 段或其下无分组段时返回 `None`。
fn resolve_managed_context(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let comps: Vec<_> = path.components().collect();
    for (i, c) in comps.iter().enumerate() {
        if c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(constants::MANAGED_FOLDER)
        {
            let group_seg = comps.get(i + 1)?;
            let mut mods_root = PathBuf::new();
            for cc in &comps[..i] {
                mods_root.push(cc.as_os_str());
            }
            let group_dir = mods_root.join(constants::MANAGED_FOLDER).join(group_seg);
            return Some((mods_root, group_dir));
        }
    }
    None
}

/// 命令级 delta 组装：对受影响分组局部重扫 + 合并缓存，返回该组完整新状态。
///
/// 流程（与 watcher 增量链路同构）：
/// 1. `scan_partial_path` 对 `rescan_dir` 局部重扫（自动扩展到所属顶层分组；
///    产物含该组完整节点与扁平 mods，磁盘为唯一真相源）；
/// 2. 读锁收集旧分组状态（须在 subtree_replace 覆盖旧数据之前完成）；
/// 3. 写锁 `subtree_replace` 就地合并缓存（缓存未命中时内部跳过，
///    下次 get_mods 全量扫描回填）。
///
/// # 参数
/// - `game`: 目标游戏（缓存键）。
/// - `mods_path`: Mods 根目录（与 `get_mods` 相同的路径形态）。
/// - `old_group_dir`: 旧分组目录（缓存匹配与 old_group_path 兜底键）。
/// - `rescan_dir`: 局部重扫入口目录（须存在；不必为分组目录，扫描会自动扩展到所属分组）。
///
/// # 返回
///
/// 受影响分组的 [`GroupDelta`]（new_group 为该组重扫后的完整新节点）。
///
/// # Errors
///
/// `scan_partial_path` 底层扫描失败时传播错误。
fn build_group_delta(
    game: TargetGame,
    mods_path: &Path,
    old_group_dir: &Path,
    rescan_dir: &Path,
) -> Result<GroupDelta> {
    let partial = mod_scanner::scan_partial_path(mods_path, rescan_dir)
        .with_context(|| format!("partial rescan failed for {:?}", rescan_dir))?;
    // 新分组节点：partial 中覆盖 rescan_dir 的分组（可能含多个节点，按 full_path 前缀匹配）
    let new_group = partial
        .groups
        .iter()
        .find(|g| mod_cache::path_within(rescan_dir, &g.full_path))
        .cloned();
    // 旧状态收集（读锁；必须在写锁合并前完成——subtree_replace 会覆盖旧数据）
    let (old_group_path, old_mod_paths) =
        mod_cache::collect_old_group_state(game, mods_path, old_group_dir);
    // 缓存合并（写锁）
    mod_cache::MOD_CACHE
        .write()
        .subtree_replace(game, mods_path, partial);
    Ok(GroupDelta {
        old_group_path,
        new_group,
        old_mod_paths,
    })
}

/// 分组移除 delta 组装：从缓存裁剪组子树（磁盘目录已移除，无需重扫）。
///
/// 读锁收集移除前的组状态，随后写锁 `subtree_remove` 裁剪缓存
/// （组节点 + 其全部 mods + 计数重算）。返回 `new_group = None` 的
/// [`GroupDelta`]（前端按 oldGroupPath 定位并剔除该组）。
///
/// # 参数
/// - `game`: 目标游戏（缓存键）。
/// - `mods_path`: Mods 根目录。
/// - `group_dir`: 被移除分组的目录路径。
///
/// # 返回
///
/// `new_group = None` 的 [`GroupDelta`]。
fn build_removed_group_delta(game: TargetGame, mods_path: &Path, group_dir: &Path) -> GroupDelta {
    // 旧状态收集（读锁；必须在写锁裁剪前完成）
    let (old_group_path, old_mod_paths) =
        mod_cache::collect_old_group_state(game, mods_path, group_dir);
    mod_cache::MOD_CACHE
        .write()
        .subtree_remove(game, mods_path, group_dir);
    GroupDelta {
        old_group_path,
        new_group: None,
        old_mod_paths,
    }
}

/// 埋点：命令级 delta 组装结果日志（每个改造命令组装后调用一处）。
///
/// affected_mods 为新分组子树内真实模组数（排除 None 空槽位），
/// removed 为旧扁平条目数（前端需从扁平 mods 列表剔除的条目数）。
fn log_delta(command_name: &str, delta: &GroupDelta) {
    let affected_mods = delta
        .new_group
        .as_ref()
        .map(mod_cache::collect_group_mod_paths)
        .map_or(0, |paths| paths.len());
    log::debug!(
        "[delta] {} 命令 | 组={} affected_mods={} removed={}",
        command_name,
        delta.old_group_path,
        affected_mods,
        delta.old_mod_paths.len()
    );
}

fn trash_delete(path: &Path) -> Result<()> {
    match trash::delete(path) {
        Ok(_) => Ok(()),
        Err(e) => {
            log::warn!(
                "Failed to move to trash: {}, falling back to permanent delete",
                e
            );
            if path.is_dir() {
                fs::remove_dir_all(path)?;
            } else {
                fs::remove_file(path)?;
            }
            Ok(())
        }
    }
}

fn open_path_in_explorer(path: &Path) -> Result<()> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| anyhow::anyhow!(e.to_string()))
}

/// 校验子文件夹名称合法性
///
/// # 参数
/// - parent_path: 父目录路径
/// - folder_name: 待校验的文件夹名称
///
/// # 返回值
/// - Ok((sanitized_name, true)): 名称合法，返回清理后的名称
/// - Ok((sanitized_name, false)): 名称不合法，sanitized_name为清理后名称，附带错误信息
/// - Err: 校验过程出错
#[tauri::command]
pub fn validate_subfolder_name(
    parent_path: String,
    folder_name: String,
) -> Result<(String, bool, String), String> {
    let parent = PathBuf::from(&parent_path);

    // 1. 清理名称：trim首尾空白
    let sanitized = folder_name.trim().to_string();

    // 2. 空名检查
    if sanitized.is_empty() {
        return Ok((sanitized, false, "分组名不能为空".to_string()));
    }

    // 3. 通用禁止名称：. 和 ..
    if sanitized == "." || sanitized == ".." {
        return Ok((
            sanitized,
            false,
            "文件夹名称不能为 \".\" 或 \"..\"".to_string(),
        ));
    }

    // 4. 平台非法字符检查
    #[cfg(target_os = "windows")]
    {
        let illegal_chars: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
        let mut found_chars = Vec::new();
        for c in illegal_chars {
            if sanitized.contains(*c) {
                found_chars.push(*c);
            }
        }
        // 检查控制字符 0x00-0x1F
        for c in sanitized.chars() {
            if (c as u32) < 0x20 {
                found_chars.push(' ');
                break;
            }
        }
        if !found_chars.is_empty() {
            let chars_str: String = found_chars.iter().collect();
            return Ok((
                sanitized,
                false,
                format!("目录名包含非法字符: {}", chars_str),
            ));
        }

        // Windows: 末尾不能是点或空格
        if sanitized.ends_with('.') || sanitized.ends_with(' ') {
            return Ok((
                sanitized.trim_end_matches(&['.', ' '][..]).to_string(),
                false,
                "文件夹名称末尾不能是点或空格".to_string(),
            ));
        }

        // Windows保留名称检查（不区分大小写）
        let upper_name = sanitized.to_uppercase();
        let reserved_names = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        for name in &reserved_names {
            if upper_name == *name {
                return Ok((
                    sanitized,
                    false,
                    "该名称为系统保留名称，请换一个名称".to_string(),
                ));
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if sanitized.contains('/') || sanitized.contains('\0') {
            return Ok((sanitized, false, "目录名包含非法字符: /".to_string()));
        }
    }

    // 5. 父目录存在检查
    if !parent.exists() {
        return Ok((sanitized, false, "父目录不存在，请刷新后重试".to_string()));
    }

    // 6. 目标路径是否已存在
    let target_path = parent.join(&sanitized);
    if target_path.exists() {
        return Ok((
            sanitized,
            false,
            "该名称的文件夹已存在，请换一个名称".to_string(),
        ));
    }

    // 7. 路径长度检查（Windows MAX_PATH 限制约260字符）
    #[cfg(target_os = "windows")]
    {
        let path_len = target_path.as_os_str().len();
        if path_len > 240 {
            return Ok((sanitized, false, "文件夹路径过长，请缩短名称".to_string()));
        }
    }

    Ok((sanitized, true, String::new()))
}

/// 创建子文件夹
///
/// # 参数
/// - parent_path: 父目录路径
/// - folder_name: 文件夹名称（应先通过validate_subfolder_name校验）
///
/// # 返回值
/// - Ok(()): 创建成功
/// - Err: 错误信息（用户友好描述）
#[tauri::command]
pub fn create_subfolder(parent_path: String, folder_name: String) -> Result<(), String> {
    let parent = PathBuf::from(&parent_path);

    // 二次校验
    let (sanitized, valid, err_msg) = validate_subfolder_name(parent_path, folder_name)?;
    if !valid {
        return Err(err_msg);
    }

    let target_path = parent.join(&sanitized);

    // 创建目录（只创建最后一级，不递归创建父目录）
    match fs::create_dir(&target_path) {
        Ok(_) => Ok(()),
        Err(e) => {
            let err_msg = match e.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    "没有权限在此位置创建文件夹，请检查权限设置".to_string()
                }
                std::io::ErrorKind::AlreadyExists => {
                    "该名称的文件夹已存在，请换一个名称".to_string()
                }
                std::io::ErrorKind::InvalidFilename => "文件夹名称包含不允许的字符".to_string(),
                _ => "创建文件夹失败，请检查路径是否可访问".to_string(),
            };
            log::error!("Failed to create subfolder {:?}: {}", target_path, e);
            Err(err_msg)
        }
    }
}

/// 禁用指定分组下所有一级模组（不含 .ini 的子分组目录不处理）
///
/// 零扫描（对齐 NRMM）：目录重命名后对缓存逐条 `patch_mod_entry` 就地更新，
/// 不触发任何扫描/重载，返回路径对列表供前端本地 patch。
/// 异常（rename 失败 / 缓存条目未命中）→ 降级增量扫描，`degraded=true` + 完整分组增量。
#[tauri::command]
pub async fn disable_all_mods_in_group(group_path: String) -> Result<BatchGroupDelta, String> {
    let (mods_root, group_dir) = resolve_managed_context(Path::new(&group_path))
        .ok_or_else(|| format!("Path is not inside a managed group: {}", group_path))?;
    let path = PathBuf::from(group_path);
    let game = settings_store::get_settings().target_game;

    batch_toggle_group_impl(game, mods_root, group_dir, path, true).await
}

/// 启用指定分组下所有一级禁用模组（不含 .ini 的子分组目录不处理）
///
/// 零扫描（对齐 NRMM）：目录重命名后对缓存逐条 `patch_mod_entry` 就地更新。
/// 异常 → 降级增量扫描，`degraded=true` + 完整分组增量。
#[tauri::command]
pub async fn enable_all_mods_in_group(group_path: String) -> Result<BatchGroupDelta, String> {
    let (mods_root, group_dir) = resolve_managed_context(Path::new(&group_path))
        .ok_or_else(|| format!("Path is not inside a managed group: {}", group_path))?;
    let path = PathBuf::from(group_path);
    let game = settings_store::get_settings().target_game;

    batch_toggle_group_impl(game, mods_root, group_dir, path, false).await
}

/// 批量启停实现（禁用/启用共用）：先目录重命名（返回路径对），再零扫描 patch 缓存；
/// 任一条缓存 patch 失败 → 降级组级增量重扫（degraded=true）。
async fn batch_toggle_group_impl(
    game: TargetGame,
    mods_root: PathBuf,
    group_dir: PathBuf,
    group_path: PathBuf,
    disable: bool,
) -> Result<BatchGroupDelta, String> {
    // P2：批量禁用/启用整组会重命名组内全部一级模组目录，属 _MANAGED_ 写路径。
    // 锁加在此共用实现上（而非两个外层命令），既覆盖两条入口又避免重复加锁。
    let _update_guard = UPDATE_LOCK.lock().await;
    let dir_display = group_path.display().to_string();
    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<BatchGroupDelta, String> {
            // 抑制 watcher 批次在批量重命名中途读取中间态（C11：暂停期事件保留，恢复后幂等合并）
            let _guard = crate::core::file_watcher::WatcherGuard::new();
            let renamed = if disable {
                crate::core::mod_manager::disable_all_mods_in_group(&group_path)
                    .map_err(|e| e.to_string())?
            } else {
                crate::core::mod_manager::enable_all_mods_in_group(&group_path)
                    .map_err(|e| e.to_string())?
            };
            let changed = renamed.len() as u32;

            // 零扫描缓存 patch：逐条就地更新（组树 + 扁平列表）；任一条未命中 → 降级
            let mut degraded = false;
            {
                let mut cache = crate::core::mod_cache::MOD_CACHE.write();
                for (old, new) in &renamed {
                    if !cache.patch_mod_entry(game, &mods_root, old, new, disable) {
                        degraded = true;
                        break;
                    }
                }
            } // 释放写锁（build_group_delta 内部需获取读/写锁）

            let delta = if degraded {
                build_group_delta(game, &mods_root, &group_dir, &group_dir)
                    .map_err(|e| e.to_string())?
            } else {
                // 正常路径不消费 delta（toggled 路径对足够），返回空增量占位
                GroupDelta::default()
            };

            let toggled = renamed
                .into_iter()
                .map(|(old, new)| crate::models::mod_data::PathPair {
                    old_path: old.to_string_lossy().to_string(),
                    new_path: new.to_string_lossy().to_string(),
                })
                .collect();
            Ok(BatchGroupDelta {
                changed,
                toggled,
                degraded,
                delta,
            })
        })
        .await
        .map_err(|e| e.to_string())??;
    log::info!(
        "[{}] changed={} degraded={} dir={}",
        if disable {
            "disable_all_mods_in_group"
        } else {
            "enable_all_mods_in_group"
        },
        result.changed,
        result.degraded,
        dir_display
    );
    Ok(result)
}

/// 移除分组（NRMM 对齐：移至 DISABLED_MANAGED_REMOVED；非group先移子分组到父级再移除）
///
/// 顶层函数：负责从设置数据获取 Mods 根目录、在线程池中执行底层移动、
/// 处理错误并记录日志。具体「哪些目录移动到哪里」由底层编排层 `remove_group_ex` 决定。
///
/// 命令级增量（R1）：分组目录（含其全部子树）已移出扫描范围，无需磁盘重扫，
/// 直接 `subtree_remove` 裁剪缓存，返回 `new_group = None` 的 [`GroupDelta`]。
#[tauri::command]
pub async fn remove_group_ex(group_path: String, is_group_xx: bool) -> Result<GroupDelta, String> {
    // P2：与 remove_group 同理，移动 _MANAGED_ 下分组子树，须统一串行化
    let _update_guard = UPDATE_LOCK.lock().await;
    // 路径边界校验（R1）：确认 group_path canonicalize 后位于任一已配置 mods_root 内
    let _validated =
        settings_store::validate_managed_path(&group_path).map_err(|e| e.to_string())?;
    let path = PathBuf::from(group_path);
    // 命令级增量（R1）：定位受影响顶层分组（磁盘目录将被移出，直接裁剪缓存子树）
    let (mods_root, group_dir) = resolve_managed_context(&path)
        .ok_or_else(|| format!("Path is not inside a managed group: {}", path.display()))?;
    let game = settings_store::get_settings().target_game;
    log::info!(
        "[remove_group_ex] start | group_path={:?} is_group_xx={}",
        path,
        is_group_xx
    );

    // 从设置数据定位 Mods 根目录（与 _MANAGED_ 同级；供底层移动编排使用）
    let settings = settings_store::get_settings();
    let mods_root_for_move = settings
        .game_mods_path
        .values()
        .map(PathBuf::from)
        .find(|p| path.starts_with(p))
        .ok_or_else(|| {
            log::error!(
                "[remove_group_ex] failed to locate Mods root from settings for {:?}",
                path
            );
            "无法从设置数据定位 Mods 根目录".to_string()
        })?;
    log::debug!("[remove_group_ex] mods_root={:?}", mods_root_for_move);

    let path_for_log = path.clone();
    let delta = tauri::async_runtime::spawn_blocking(move || -> Result<GroupDelta, String> {
        // 抑制 watcher 批次在批量移动中途读取中间态（C11：暂停期事件保留，恢复后幂等合并）
        let _guard = crate::core::file_watcher::WatcherGuard::new();
        crate::core::mod_manager::remove_group_ex(&path, is_group_xx, &mods_root_for_move)
            .map_err(|e| e.to_string())?;
        let delta = build_removed_group_delta(game, &mods_root, &group_dir);
        log_delta("remove_group_ex", &delta);
        Ok(delta)
    })
    .await
    .map_err(|e| e.to_string())??;

    log::info!(
        "[remove_group_ex] completed | group_path={:?}",
        path_for_log
    );
    Ok(delta)
}

// =========================================================================
// 单元测试：update_mod_data 逻辑整合于此（项目内测试）
//
// 设计要点：
// - 修复原外部集成测试（tests/update_mod_data_test.rs）的两个缺陷：
//   1) 使用 tempfile::TempDir 自动销毁 → 测试后磁盘无残留，无法观测"目录是否变化"；
//   2) 基线映射颠倒（input=NRMM-test 已处理产物，baseline=NRMM-Rust-test 原始输入）
//      → 实际上从不验证 update_mod_data 是否真正生成了 managed 文件。
// - 本模块使用【持久化运行时目录】tests/update_mod_data_runtime/，测试执行后可直接在
//   磁盘查看 _MANAGED_ 产物，验证目录确实发生变化。
// - 数据集映射保持正确：NRMM-Rust-test = 输入（原始），NRMM-test = 预期输出基线。
// - 若测试中发现异常（目录未变化 / 与基线不一致），应直接修正项目源码
//   （mod_manager.rs / ini_handler.rs 等），而非仅修改此处测试脚手架。
// =========================================================================

#[cfg(test)]
mod tests {
    use super::{build_group_delta, build_removed_group_delta};
    use crate::core::constants;
    use crate::core::mod_manager;
    use crate::models::enums::TargetGame;
    use crate::models::settings::AppSettings;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// 持久化运行时目录：cargo test 后可直接在磁盘查看产物，验证目录确实变化
    const RUNTIME_DIR: &str = "tests/runtime/update_mod_data_runtime";

    /// 串行化对共享 `RUNTIME_DIR` 的访问：多个测试并行执行时会互相 `remove_dir_all`
    /// 同一目录（Windows 下目录句柄占用 → PermissionDenied）。引用该目录的测试入口须持有此锁。
    static RUNTIME_DIR_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

    /// 输入数据集（原始，未经 update_mod_data 处理）
    fn input_dataset() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("datasets")
            .join("NRMM-Rust-test");
        if !manifest_dir.exists() {
            panic!(
                "Input dataset directory does not exist: {}",
                manifest_dir.display()
            );
        }
        manifest_dir
    }

    /// 预期输出基线（Dart 原版 NRMM 实测产物）
    fn baseline_managed() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("datasets")
            .join("NRMM-test")
            .join("Mods")
            .join("_MANAGED_")
    }

    fn read_file(p: &Path) -> String {
        fs::read_to_string(p).unwrap_or_default()
    }

    fn normalize_paths(s: &str) -> String {
        s.replace('\\', "/")
    }

    /// 规范化 INI 内容，忽略绝对路径 / 注释噪声 / $managed_slot_id 值，保留结构
    fn normalize_ini_content(s: &str) -> String {
        let mut out = String::new();
        for line in s.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lower = trimmed.to_lowercase();
            if lower.starts_with("ini_path_absolute") {
                continue;
            }
            if lower.starts_with("global $managed_slot_id") {
                out.push_str("global $managed_slot_id = <SLOT>\n");
                continue;
            }
            if trimmed.starts_with(';') {
                if trimmed.contains("NRMM")
                    || trimmed.contains("DISABLED")
                    || trimmed.starts_with("; \"")
                {
                    out.push_str(line.trim_end());
                    out.push('\n');
                }
                continue;
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out.replace("\r\n", "\n")
    }

    /// 递归复制目录
    fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
        // 确保源目录存在且为目录
        if !src.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} is not a directory", src.display()),
            ));
        }

        for entry in walkdir::WalkDir::new(src).follow_links(false) {
            let entry = entry.map_err(|e| std::io::Error::other(e.to_string()))?;

            // ✅ 核心：计算相对于源根的路径
            let relative: PathBuf = entry
                .path()
                .strip_prefix(src)
                .map_err(std::io::Error::other)?
                .to_path_buf();

            // 跳过根目录本身（relative == ""）
            if relative.as_os_str().is_empty() {
                continue;
            }

            let target = dst.join(&relative);

            if entry.file_type().is_dir() {
                fs::create_dir_all(&target)?;
            } else if entry.file_type().is_symlink() {
                // 符号链接：读取原始目标并重建
                #[cfg(unix)]
                {
                    let link_target = fs::read_link(entry.path())?;
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let _ = fs::remove_file(&target);
                    std::os::unix::fs::symlink(link_target, &target)?;
                }
                #[cfg(windows)]
                {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(entry.path(), &target)?;
                }
            } else {
                // 普通文件：确保父目录存在后复制
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(entry.path(), &target)?;
            }

            // println!("copied: {}", relative.display());
        }

        Ok(())
    }

    /// 将输入数据集还原到持久化运行时目录。
    /// 路径约定（与原版 NRMM / 集成测试一致）：
    /// - d3dx.ini → <runtime>/d3dx.ini（游戏根目录）
    /// - Mods/_MANAGED_ → <runtime>/Mods/_MANAGED_/
    /// - 其余目录/文件 → <runtime>/Mods/<name>
    ///
    /// 返回 game_mods_path（必须以 "Mods" 结尾）
    fn restore_dataset_to(runtime: &Path) -> PathBuf {
        // fail-fast：remove 静默失败会让旧运行产物（manager_group.ini 等）残留，
        // 后续「运行前不应存在生成文件」断言确定性失败且极难排查（Windows 下
        // 目录被杀软/索引/上一进程句柄短暂占用时 remove_dir_all 会失败）。
        fs::remove_dir_all(runtime)
            .or_else(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .expect("清理运行时目录失败（可能被占用），请手动删除后重试");
        fs::create_dir_all(runtime).expect("创建运行时根目录失败");

        let src_root = input_dataset();
        copy_dir(&src_root, runtime).unwrap();

        runtime.join("Mods")
    }

    /// 验证测试目录确实发生变化：运行后必须生成以下文件（运行前不存在），
    /// 且关键文件与 NRMM-test 基线语义一致。
    fn assert_managed_outputs(managed: &Path) {
        // 1) 目录变化验证：以下文件在运行前不存在，运行后必须存在且非空
        for f in [
            "manager_group.ini",
            "nrmm_include.ini",
            "nrmm_keypress.txt",
            "selectedindex",
        ] {
            let p = managed.join(f);
            assert!(p.exists(), "[目录变化] 缺失生成文件: {}", f);
            let c = read_file(&p);
            assert!(!c.trim().is_empty(), "[目录变化] 生成文件为空: {}", f);
        }
        // group_1 下由 update_mod_data 生成的分组文件
        let g1_ini = managed.join("group_1").join("group_1.ini");
        let g1_sel = managed.join("group_1").join("selectedindex");
        assert!(g1_ini.exists(), "[目录变化] group_1.ini 应被生成");
        assert!(g1_sel.exists(), "[目录变化] group_1/selectedindex 应被生成");

        // 2) 与 NRMM-test 基线语义比对（已知对齐项）
        let base = baseline_managed();
        for f in ["nrmm_include.ini", "nrmm_keypress.txt"] {
            let a = normalize_ini_content(&normalize_paths(&read_file(&managed.join(f))));
            let e = normalize_ini_content(&normalize_paths(&read_file(&base.join(f))));
            assert_eq!(a, e, "[基线比对] {} 与 NRMM-test 不一致", f);
        }
        let g1 = normalize_ini_content(&normalize_paths(&read_file(&g1_ini)));
        let g1b = normalize_ini_content(&normalize_paths(&read_file(
            &base.join("group_1").join("group_1.ini"),
        )));
        assert_eq!(g1, g1b, "[基线比对] group_1.ini 与 NRMM-test 不一致");

        let sel = read_file(&managed.join("selectedindex")).trim().to_string();
        assert_eq!(sel, "0", "[基线比对] 根 selectedindex 应为 0");
    }

    /// 主测试：构造参数 → 调用 update_mod_data → 验证目录变化 + 基线一致
    ///
    /// 使用持久化运行时目录，测试执行后可直接在磁盘检查
    /// `src-tauri/tests/update_mod_data_runtime/` 下的 _MANAGED_ 产物。
    #[test]
    fn update_mod_data_test() {
        let _runtime_lock = RUNTIME_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        println!("\n=== update_mod_data_test（集成到 mod_commands.rs）===");
        let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(RUNTIME_DIR);
        let game_mods_path = restore_dataset_to(&runtime);
        let managed = game_mods_path.join(constants::MANAGED_FOLDER);

        // 运行前快照：输入数据集仅有 group_1 目录，尚无生成文件
        assert!(
            !managed.join("manager_group.ini").exists(),
            "运行前不应存在 manager_group.ini"
        );
        assert!(
            !managed.join("nrmm_include.ini").exists(),
            "运行前不应存在 nrmm_include.ini"
        );

        // 构造参数并调用 update_mod_data（核心函数，命令层仅为其异步包装）
        let settings = AppSettings::default();
        // 注意：NRMM-test 基线由 NRMM 以 Zenless（nrmm_name = "Zenless_Zone_Zero"）生成，
        // keypress 模板 `{game}` 替换依赖游戏标识，故此处必须用同款游戏以保证 parity 比对一致。
        let result = mod_manager::update_mod_data(TargetGame::ZZZ, &game_mods_path, &settings)
            .expect("update_mod_data 执行失败");

        println!(
            "  [result] enabled={} disabled={} processed={} errors={} groups={}",
            result.enabled_mods,
            result.disabled_mods,
            result.processed_mods,
            result.errors.len(),
            result.total_groups
        );

        // 验证返回结果合理
        assert!(
            result.processed_mods >= 1,
            "至少应处理 1 个模组，实际 {}",
            result.processed_mods
        );

        // 验证测试目录确实发生变化（生成产物存在且与基线一致）
        assert_managed_outputs(&managed);

        println!("  [PASS] update_mod_data_test");
        println!("  运行时产物目录: {:?}", runtime);
        println!("=== update_mod_data_test 完成 ===\n");
    }

    /// 场景：空 Mods 目录（无模组）
    #[test]
    fn update_mod_data_test_empty_mods() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mods = tmp.path().join("Mods");
        fs::create_dir_all(&mods).unwrap();
        let managed = mods.join(constants::MANAGED_FOLDER);
        fs::create_dir_all(&managed).unwrap();

        let settings = AppSettings::default();
        let result = mod_manager::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
            .expect("update_mod_data 失败");
        assert_eq!(result.processed_mods, 0, "空目录不应处理任何模组");
        // 仍应生成 managed 基础文件（目录发生变化）
        assert!(managed.join("nrmm_include.ini").exists());
        assert!(managed.join("nrmm_keypress.txt").exists());
        assert!(managed.join("manager_group.ini").exists());
        assert!(managed.join("selectedindex").exists());
        println!("[PASS] update_mod_data_test_empty_mods");
    }

    /// 场景：DISABLED 模组被跳过
    #[test]
    fn update_mod_data_test_disabled_mod_skipped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mods = tmp.path().join("Mods");
        fs::create_dir_all(&mods).unwrap();
        let managed = mods.join(constants::MANAGED_FOLDER);
        fs::create_dir_all(&managed).unwrap();

        let group_dir = managed.join("group_1");
        fs::create_dir_all(&group_dir).unwrap();
        fs::write(
            group_dir.join("group_1.ini"),
            ";template\n[Constants]\nglobal $group_id = 1\n",
        )
        .unwrap();

        // 启用模组
        let enabled = group_dir.join("EnabledMod");
        fs::create_dir_all(&enabled).unwrap();
        fs::write(
            enabled.join("mod.ini"),
            "[TextureOverride]\nhash = 0xAAAA\n",
        )
        .unwrap();

        // 禁用模组（DISABLED 前缀）
        let disabled = group_dir.join("DISABLED_TestMod");
        fs::create_dir_all(&disabled).unwrap();
        fs::write(
            disabled.join("mod.ini"),
            "[TextureOverride]\nhash = 0xBBBB\n",
        )
        .unwrap();

        let settings = AppSettings::default();
        let result = mod_manager::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
            .expect("update_mod_data 失败");
        assert_eq!(result.enabled_mods, 1, "应只有 1 个启用模组");
        assert_eq!(result.disabled_mods, 1, "应有 1 个禁用模组");
        assert_eq!(result.processed_mods, 1, "应仅处理启用的模组");
        println!("[PASS] update_mod_data_test_disabled_mod_skipped");
    }

    /// 场景：多分组更新
    #[test]
    fn update_mod_data_test_multiple_groups() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mods = tmp.path().join("Mods");
        fs::create_dir_all(&mods).unwrap();
        let managed = mods.join(constants::MANAGED_FOLDER);
        fs::create_dir_all(&managed).unwrap();

        for g in 1..=2u32 {
            let gd = managed.join(format!("group_{}", g));
            fs::create_dir_all(&gd).unwrap();
            fs::write(
                gd.join(format!("group_{}.ini", g)),
                format!(";t\n[Constants]\nglobal $group_id = {}\n", g),
            )
            .unwrap();
            for m in 1..=2u32 {
                let md = gd.join(format!("Mod_{}", m));
                fs::create_dir_all(&md).unwrap();
                fs::write(
                    md.join("mod.ini"),
                    format!(
                        "[TextureOverride_G{}_M{}]\nhash = 0x{:08X}\n",
                        g,
                        m,
                        0x12345678 + g * 256 + m * 16
                    ),
                )
                .unwrap();
            }
        }

        let settings = AppSettings::default();
        let result = mod_manager::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
            .expect("update_mod_data 失败");
        assert!(result.total_groups >= 2, "应检测到至少 2 个分组");
        assert!(result.processed_mods >= 4, "应处理至少 4 个模组");
        assert!(managed.join("group_1/group_1.ini").exists());
        assert!(managed.join("group_2/group_2.ini").exists());
        println!("[PASS] update_mod_data_test_multiple_groups");
    }

    /// 场景：幂等性（连续两次更新结果一致）
    #[test]
    fn update_mod_data_test_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mods = tmp.path().join("Mods");
        fs::create_dir_all(&mods).unwrap();
        let managed = mods.join(constants::MANAGED_FOLDER);
        fs::create_dir_all(&managed).unwrap();

        let group_dir = managed.join("group_1");
        fs::create_dir_all(&group_dir).unwrap();
        fs::write(
            group_dir.join("group_1.ini"),
            ";template\n[Constants]\nglobal $group_id = 1\n",
        )
        .unwrap();
        let en = group_dir.join("EnabledMod");
        fs::create_dir_all(&en).unwrap();
        fs::write(en.join("mod.ini"), "[TextureOverride]\nhash = 0xAAAA\n").unwrap();

        let settings = AppSettings::default();
        let r1 = mod_manager::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
            .expect("第一次 update_mod_data 失败");
        let r2 = mod_manager::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
            .expect("第二次 update_mod_data 失败");
        assert_eq!(
            r1.processed_mods, r2.processed_mods,
            "两次更新 processed_mods 应一致"
        );
        assert_eq!(
            r1.enabled_mods, r2.enabled_mods,
            "两次更新 enabled_mods 应一致"
        );
        println!("[PASS] update_mod_data_test_idempotent");
    }

    // =====================================================================
    // 还原数据测试（update 目录 → 还原 → 全树比对）
    //
    // 流程：输入数据集 → update_mod_data 注入 → restore_all_inis 还原 →
    //   1) 还原数据 = 注入前原始内容（与输入数据集逐字节一致）
    //   2) 管理生成文件被清理（manager_group/nrmm_include/keypress/group_X.ini/备份）
    //   3) 还原安全窗口内重扫不新建管理标记（groupname/modname/selectedindex）
    //
    // 持有 TEST_WINDOW_LOCK：restore_all_inis 内部开启还原安全窗口（全局状态），
    // 与依赖标记创建的并发测试互斥；结束时显式清除窗口，防止 60s 泄漏。
    // =====================================================================
    #[test]
    fn restore_after_update_test() {
        use crate::core::mod_scanner;
        use std::collections::BTreeSet;
        use walkdir::WalkDir;

        // 共享运行时目录互斥：与 update_mod_data_test 并行时不得互删同一目录
        let _runtime_lock = RUNTIME_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // 窗口全局状态互斥（含 mod_scanner 内的标记创建断言测试）
        let _window_lock = mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // panic 安全：测试退出（含断言失败）即清除窗口
        struct ClearWindow;
        impl Drop for ClearWindow {
            fn drop(&mut self) {
                mod_scanner::test_clear_restore_safe_window();
            }
        }
        let _clear = ClearWindow;

        println!("\n=== restore_after_update_test ===");
        let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(RUNTIME_DIR);
        let game_mods_path = restore_dataset_to(&runtime);
        let managed = game_mods_path.join(constants::MANAGED_FOLDER);
        let input_root = input_dataset();

        // 收集管理标记文件相对路径清单（groupname/modname/selectedindex）
        fn marker_inventory(root: &Path) -> BTreeSet<String> {
            let mut set = BTreeSet::new();
            for entry in WalkDir::new(root).follow_links(false) {
                let e = entry.unwrap();
                if !e.file_type().is_file() {
                    continue;
                }
                let name = e.file_name().to_string_lossy().to_string();
                if name == "groupname" || name == "modname" || name == "selectedindex" {
                    let rel = e
                        .path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    set.insert(rel);
                }
            }
            set
        }

        // 1) 注入（与 update_mod_data_test 同款参数保证 parity）
        let settings = AppSettings::default();
        let result = mod_manager::update_mod_data(TargetGame::ZZZ, &game_mods_path, &settings)
            .expect("update_mod_data 执行失败");
        assert!(result.processed_mods >= 1, "至少应处理 1 个模组");
        assert_managed_outputs(&managed);

        let markers_before = marker_inventory(&runtime);

        // 2) 还原
        let restored =
            mod_manager::restore_all_inis(&game_mods_path).expect("restore_all_inis 执行失败");
        println!(
            "  [restore] restored={} failed={}",
            restored.restored, restored.failed
        );
        assert!(restored.restored >= 1, "至少应还原 1 个 INI");
        assert_eq!(restored.failed, 0, "还原不应有失败项");

        // 3) 管理生成文件被清理
        for f in [
            "manager_group.ini",
            "nrmm_include.ini",
            "nrmm_keypress.txt",
            "group_1.ini",
        ] {
            assert!(!managed.join(f).exists(), "[清理] 还原后仍存在: {}", f);
        }
        // 注入备份应全部被消费；例外 = 输入数据集自带的孤儿备份
        // （原 .ini 已被用户删除而备份残留，如 bakup (2).ini_managed_backup，
        //   Dart 基线同样保留 → 非本次还原产生，不得误报）
        let mut leftover_backups = Vec::new();
        for entry in WalkDir::new(&runtime).follow_links(false) {
            let e = entry.unwrap();
            if e.file_type().is_file()
                && e.file_name()
                    .to_string_lossy()
                    .ends_with(constants::BACKUP_EXTENSION)
            {
                leftover_backups.push(e.path().to_path_buf());
            }
        }
        let orphan_backups: Vec<&PathBuf> = leftover_backups
            .iter()
            .filter(|p| !input_root.join(p.strip_prefix(&runtime).unwrap()).exists())
            .collect();
        assert!(
            orphan_backups.is_empty(),
            "还原残留非输入数据集自带的注入备份: {:?}",
            orphan_backups
        );

        // 4) 还原数据正确性：全树与输入数据集比对
        //    语义：还原 = 回到「注入前原始内容」。
        //    - 输入无备份 → 期望运行时 == 输入（update 注入后还原应回到原样）
        //    - 输入自带备份 → 输入文件本身是上次 NRMM 会话的已注入形态，
        //      期望运行时 == 输入备份内容（真·原始；备份已存在时 update 不覆盖，
        //      还原回到备份记录的原始版本，Dart 同语义）
        //    备份命名与 mod_manager 一致：Path::with_extension（替换 .ini 后缀），
        //    即 X.ini → X.ini_managed_backup（非追加）。
        let mut content_diff: Vec<String> = Vec::new();
        for entry in WalkDir::new(&input_root).follow_links(false) {
            let e = entry.unwrap();
            if !e.file_type().is_file() {
                continue;
            }
            let rel = e.path().strip_prefix(&input_root).unwrap();
            let rt_path = runtime.join(rel);
            let file_name = e.file_name().to_string_lossy().to_string();
            let parent = e.path().parent().unwrap();
            let backup_suffix = format!(".{}", constants::BACKUP_EXTENSION);

            // 输入自带的备份文件（X.ini_managed_backup）：
            //   主文件存在 → 还原消费备份（拷回主文件后删除）→ 期望运行时缺失
            //   主文件不存在（孤儿备份，如 bakup (2).ini_managed_backup）→ 原样保留
            if file_name.ends_with(&backup_suffix) {
                // 主文件名反推：with_extension 是「替换 .ini」→ Herta.ini_managed_backup
                // 的主文件是 Herta + ".ini" = Herta.ini（restore 仅对 .ini 建备份）
                let main_name = format!("{}.ini", file_name.trim_end_matches(&backup_suffix));
                if parent.join(&main_name).exists() {
                    if rt_path.exists() {
                        content_diff.push(format!("备份应被还原消费但仍存在: {}", rel.display()));
                    }
                } else if !rt_path.exists() {
                    content_diff.push(format!("孤儿备份运行时缺失: {}", rel.display()));
                } else {
                    let a = fs::read(e.path()).unwrap();
                    let b = fs::read(&rt_path).unwrap();
                    if a != b {
                        content_diff.push(format!("孤儿备份内容不一致: {}", rel.display()));
                    }
                }
                continue;
            }

            if !rt_path.exists() {
                content_diff.push(format!("运行时缺失: {}", rel.display()));
                continue;
            }

            // 非备份文件：仅 .ini 参与注入/还原（对齐 restore_inis_recursive 条件）；
            // 输入自带同主名备份 → 期望还原到备份内容
            let is_ini = Path::new(&file_name)
                .extension()
                .map(|x| x.eq_ignore_ascii_case("ini"))
                .unwrap_or(false)
                && !constants::is_desktop_ini(e.path());
            let backup_path = Path::new(&file_name)
                .with_extension(constants::BACKUP_EXTENSION)
                .to_string_lossy()
                .to_string();
            let has_backup = is_ini && parent.join(&backup_path).exists();
            let expected = if has_backup {
                parent.join(&backup_path)
            } else {
                e.path().to_path_buf()
            };
            let a = fs::read(&expected).unwrap();
            let b = fs::read(&rt_path).unwrap();
            if a != b {
                content_diff.push(format!(
                    "内容不一致: {}（期望 {}）",
                    rel.display(),
                    if has_backup {
                        "输入备份内容（注入前原始）".to_string()
                    } else {
                        "输入内容".to_string()
                    }
                ));
            }
        }
        assert!(
            content_diff.is_empty(),
            "还原后与输入数据集存在差异（{} 处）:\n{}",
            content_diff.len(),
            content_diff.join("\n")
        );

        let mut extra_files: Vec<String> = Vec::new();
        for entry in WalkDir::new(&runtime).follow_links(false) {
            let e = entry.unwrap();
            if !e.file_type().is_file() {
                continue;
            }
            let rel = e.path().strip_prefix(&runtime).unwrap();
            if !input_root.join(rel).exists() {
                extra_files.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        // 白名单：update 生成、还原按既有语义不回收的文件
        let extra_unexpected: Vec<&String> = extra_files
            .iter()
            .filter(|p| {
                let name = p.rsplit('/').next().unwrap_or("");
                // 根/组级 selectedindex：update 显式生成，还原不清理（Dart 同语义）
                !(name == "selectedindex")
            })
            .collect();
        assert!(
            extra_unexpected.is_empty(),
            "还原后存在白名单之外的运行时新增文件: {:?}",
            extra_unexpected
        );
        println!(
            "  [extra] 白名单内新增文件 {} 个: {:?}",
            extra_files.len(),
            extra_files
        );

        // 5) 还原安全窗口内重扫：不新建任何管理标记
        //    （模拟 watcher 防抖 1s 后的重扫，窗口 60s 内必然处于活跃态）
        mod_scanner::scan_mods_light(&game_mods_path).expect("还原后重扫失败");
        let markers_after = marker_inventory(&runtime);
        let new_markers: Vec<&String> = markers_after.difference(&markers_before).collect();
        assert!(
            new_markers.is_empty(),
            "还原安全窗口内重扫新建了管理标记（违反还原不回写语义）: {:?}",
            new_markers
        );

        println!("  [PASS] restore_after_update_test");
        println!("  运行时产物目录: {:?}", runtime);
        println!("=== restore_after_update_test 完成 ===\n");
    }

    // =====================================================================
    // 命令级 Delta 组装（R1）：局部重扫 + subtree_replace/subtree_remove 合并
    //
    // 注意：以下测试直接调用模块内组装函数（不经过 Tauri 命令层），
    // 使用全局 MOD_CACHE（各测试临时目录路径唯一，缓存键互不冲突）；
    // 故意不在结束时 invalidate_all——并行测试下清空全局缓存会破坏
    // 其他正在进行的 delta 测试的前置数据。全局缓存写者（含 file_watcher
    // R2 测试）共 4 个 > 容量上限 3，须持 CACHE_WRITER_TEST_LOCK 串行，
    // 防 set() 的 LRU 淘汰在他测窗口内淘汰其条目（详见锁文档）。
    // =====================================================================

    /// 冒烟测试：toggle 禁用后 delta.new_group 与磁盘一致、old_mod_paths 正确、缓存已合并。
    #[test]
    fn toggle_group_delta_assembly_smoke() {
        use crate::core::mod_cache;
        use crate::core::mod_scanner;
        use tempfile::TempDir;

        // 写全局 MOD_CACHE 的测试串行（容量 3 + 4 个写者，防 LRU 跨测试淘汰，
        // 见 mod_cache::CACHE_WRITER_TEST_LOCK 文档）
        let _cache_guard = mod_cache::CACHE_WRITER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join(constants::MANAGED_FOLDER);
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let group1 = managed.join("group_1");
        fs::create_dir_all(group1.join("ModA")).unwrap();
        fs::write(group1.join("ModA").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        fs::create_dir_all(group1.join("ModB")).unwrap();
        fs::write(group1.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        // 另一组不应受影响
        fs::create_dir_all(managed.join("group_2").join("ModC")).unwrap();
        fs::write(
            managed.join("group_2").join("ModC").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();

        // 预填充缓存（模拟 get_mods 已加载该游戏）
        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        mod_cache::MOD_CACHE
            .write()
            .set(TargetGame::GenshinImpact, &mods_path, full);

        // 执行禁用（目录重命名：ModA → DISABLED 前缀形态）
        let mod_a = group1.join("ModA");
        mod_manager::toggle_mod(&mod_a, false).expect("toggle_mod 失败");

        // delta 组装：toggle 场景对 parent（分组目录）局部重扫
        let delta = build_group_delta(
            TargetGame::GenshinImpact,
            &mods_path,
            &group1,
            mod_a.parent().expect("mod 目录应有 parent"),
        )
        .expect("delta 组装失败");

        // old_group_path 为缓存中该组的 group_path（前端树定位键）
        assert_eq!(
            delta.old_group_path,
            group1.to_string_lossy().to_string(),
            "old_group_path 应为缓存内该组的分组路径"
        );
        // old_mod_paths：磁盘操作前该组全部真实模组（排除 None 槽位）
        assert_eq!(delta.old_mod_paths.len(), 2, "该组应有 2 个旧模组条目");
        assert!(
            delta.old_mod_paths.iter().any(|p| p.ends_with("ModA")),
            "old_mod_paths 应含旧 ModA 路径"
        );
        assert!(
            delta.old_mod_paths.iter().any(|p| p.ends_with("ModB")),
            "old_mod_paths 应含 ModB 路径"
        );

        // new_group 与磁盘一致：含禁用后的 ModA 与未受影响的 ModB
        let new_group = delta.new_group.as_ref().expect("new_group 不应为 None");
        assert_eq!(new_group.group_name, "group_1");
        assert_eq!(
            mod_cache::collect_group_mod_paths(new_group).len(),
            2,
            "新分组应含 2 个真实模组"
        );
        let disabled = new_group
            .mods
            .iter()
            .find(|m| m.mod_name.ends_with("ModA"))
            .expect("新分组应含重命名后的 ModA");
        assert!(
            disabled.mod_disabled && disabled.disabled,
            "禁用后的模组应带 disabled 标记"
        );
        assert!(
            new_group.mods.iter().any(|m| m.mod_name == "ModB"),
            "未受影响的 ModB 应保留"
        );

        // 缓存已合并：group_1 节点反映禁用后状态，扁平列表剔除旧路径、计数重算
        let cached = mod_cache::MOD_CACHE
            .read()
            .get(TargetGame::GenshinImpact, &mods_path)
            .expect("缓存应命中");
        let g1 = mod_cache::find_group_node(&cached, &group1).expect("缓存应含 group_1");
        assert!(
            g1.mods
                .iter()
                .any(|m| m.mod_name.ends_with("ModA") && m.mod_disabled),
            "缓存内 group_1 应已反映禁用状态"
        );
        assert!(
            !cached
                .mods
                .iter()
                .any(|m| m.mod_path.ends_with("\\ModA") || m.mod_path.ends_with("/ModA")),
            "缓存扁平列表不应残留旧启用态 ModA 路径（应替换为 DISABLED 前缀路径）"
        );
        assert!(
            cached.mods.iter().any(|m| m.mod_name == "ModB"),
            "缓存扁平列表应保留未受影响的 ModB（删除传播后由 partial 重新合并）"
        );
        assert!(
            cached
                .mods
                .iter()
                .any(|m| m.mod_disabled && m.mod_path.contains("DISABLED")),
            "缓存扁平列表应含禁用后的 ModA 新条目"
        );
        assert_eq!(
            (
                cached.total_mods_count,
                cached.enabled_mods_count,
                cached.disabled_mods_count
            ),
            (3, 2, 1),
            "合并后计数应重算：3 总 / 2 启用（ModB+ModC）/ 1 禁用"
        );
        // group_2 未受影响
        assert!(
            cached.mods.iter().any(|m| m.mod_name == "ModC"),
            "未受影响分组的模组应保留"
        );
        println!("[PASS] toggle_group_delta_assembly_smoke");
    }

    /// 冒烟测试：remove_group 场景 delta（new_group=None，缓存子树被裁剪）。
    #[test]
    fn remove_group_delta_clears_cache_subtree() {
        use crate::core::mod_cache;
        use crate::core::mod_scanner;
        use tempfile::TempDir;

        // 写全局 MOD_CACHE 的测试串行（容量 3 + 4 个写者，防 LRU 跨测试淘汰，
        // 见 mod_cache::CACHE_WRITER_TEST_LOCK 文档）
        let _cache_guard = mod_cache::CACHE_WRITER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join(constants::MANAGED_FOLDER);
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let group1 = managed.join("group_1");
        fs::create_dir_all(group1.join("ModA")).unwrap();
        fs::write(group1.join("ModA").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        fs::create_dir_all(group1.join("ModB")).unwrap();
        fs::write(group1.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        fs::create_dir_all(managed.join("group_2").join("ModC")).unwrap();
        fs::write(
            managed.join("group_2").join("ModC").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();

        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        mod_cache::MOD_CACHE
            .write()
            .set(TargetGame::GenshinImpact, &mods_path, full);

        // 磁盘删除 group_1（模拟 trash_delete 的效果）
        fs::remove_dir_all(&group1).unwrap();

        let delta = build_removed_group_delta(TargetGame::GenshinImpact, &mods_path, &group1);

        assert!(
            delta.new_group.is_none(),
            "remove_group 场景 new_group 应为 None"
        );
        assert_eq!(delta.old_group_path, group1.to_string_lossy().to_string());
        assert_eq!(
            delta.old_mod_paths.len(),
            2,
            "应收集被移除组的 2 个模组路径"
        );

        let cached = mod_cache::MOD_CACHE
            .read()
            .get(TargetGame::GenshinImpact, &mods_path)
            .expect("缓存应命中");
        assert!(
            mod_cache::find_group_node(&cached, &group1).is_none(),
            "缓存中 group_1 节点应被移除"
        );
        assert!(
            cached
                .mods
                .iter()
                .all(|m| m.mod_name != "ModA" && m.mod_name != "ModB"),
            "缓存扁平列表不应残留 group_1 模组"
        );
        assert!(
            cached.mods.iter().any(|m| m.mod_name == "ModC"),
            "其他分组应保留"
        );
        assert_eq!(
            (
                cached.total_mods_count,
                cached.enabled_mods_count,
                cached.disabled_mods_count
            ),
            (1, 1, 0),
            "裁剪后计数应重算"
        );
        println!("[PASS] remove_group_delta_clears_cache_subtree");
    }

    /// 冒烟测试：互斥组模组重命名场景，以重命名后的新目录局部重扫。
    #[test]
    fn rename_mutex_mod_delta_uses_new_path() {
        use crate::core::mod_cache;
        use crate::core::mod_scanner;
        use tempfile::TempDir;

        // 写全局 MOD_CACHE 的测试串行（容量 3 + 4 个写者，防 LRU 跨测试淘汰，
        // 见 mod_cache::CACHE_WRITER_TEST_LOCK 文档）
        let _cache_guard = mod_cache::CACHE_WRITER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join(constants::MANAGED_FOLDER);
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let mutex_root = managed.join("#Mutex");
        fs::create_dir_all(mutex_root.join("ModY")).unwrap();
        fs::write(mutex_root.join("ModY").join("mod.ini"), "[Section]\nx=1\n").unwrap();

        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        mod_cache::MOD_CACHE
            .write()
            .set(TargetGame::GenshinImpact, &mods_path, full);

        // 重命名 mod 目录（mutex 分支：目录重命名）
        let old_path = mutex_root.join("ModY");
        let new_path = mutex_root.join("NewY");
        fs::rename(&old_path, &new_path).expect("重命名失败");

        let delta = build_group_delta(
            TargetGame::GenshinImpact,
            &mods_path,
            &mutex_root,
            &new_path,
        )
        .expect("delta 组装失败");

        // new_group 为 #Mutex 整组（局部重扫自动扩展到所属分组），含新目录模组
        let new_group = delta.new_group.as_ref().expect("new_group 不应为 None");
        assert_eq!(new_group.group_name, "#Mutex");
        let new_paths = mod_cache::collect_group_mod_paths(new_group);
        assert!(
            new_paths.iter().any(|p| p.ends_with("NewY")),
            "新分组应含重命名后的 NewY"
        );
        assert!(
            !new_paths.iter().any(|p| p.ends_with("ModY")),
            "新分组不应残留旧 ModY"
        );
        // old_mod_paths 含旧路径（供前端剔除）
        assert!(
            delta.old_mod_paths.iter().any(|p| p.ends_with("ModY")),
            "old_mod_paths 应含旧 ModY 路径"
        );
        // 缓存合并后同样无旧条目
        let cached = mod_cache::MOD_CACHE
            .read()
            .get(TargetGame::GenshinImpact, &mods_path)
            .expect("缓存应命中");
        assert!(
            cached.mods.iter().all(|m| m.mod_name != "ModY"),
            "缓存扁平列表不应残留旧 ModY"
        );
        assert!(
            cached.mods.iter().any(|m| m.mod_name == "NewY"),
            "缓存扁平列表应含 NewY"
        );
        println!("[PASS] rename_mutex_mod_delta_uses_new_path");
    }
}
