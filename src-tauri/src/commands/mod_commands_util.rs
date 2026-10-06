//! 模组命令共享辅助（从 mod_commands.rs 迁出，2026-09-19 Phase 6-3）
//!
//! 承载分散在模组命令家族（query/group/selection/mod 各兄弟文件）之间的共享符号：
//! 全局锁（UPDATE_LOCK/SCAN_LOCK）、选择防抖（SELECTION_DEBOUNCE_MS +
//! selection_debounce_hit）、路径/分组判定（is_direct_writable_group /
//! resolve_managed_context）、命令级 Delta 组装（build_group_delta /
//! build_removed_group_delta / log_delta）、游戏解析（parse_game）与通用
//! IO（trash_delete / open_path_in_explorer / derive_toggled_path）。
//!
//! 语义零改动迁移：符号按 clippy 裁决升 `pub(crate)`，供兄弟命令文件
//! 经 `use crate::commands::mod_commands_util::{...}` 引用。

use crate::core::constants;
use crate::core::{mod_cache_core, mod_cache_delta, mod_cache_path};

use crate::models::enums::TargetGame;
use crate::models::mod_data::GroupDelta;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

/// 选择操作防抖窗口（毫秒）
pub(crate) const SELECTION_DEBOUNCE_MS: u64 = 500;

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
pub(crate) fn selection_debounce_hit(group_path: &str, mod_path: &str) -> Result<bool, String> {
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
pub(crate) static UPDATE_LOCK: LazyLock<AsyncMutex<()>> = LazyLock::new(AsyncMutex::default);

/// 全局轻量扫描串行锁（singleflight，2026-08-19）。
///
/// `get_mods` cache miss 时并发调用（多入口：窗口显隐/切游戏/页面挂载）会各自执行
/// 全量 light scan（实测 ~4s×N，日志证实并发两次 "Cache miss"），既浪费 CPU 又使
/// 前端两次响应时序不定（叠加竞态）。拿锁后二次检查缓存（double-checked）：
/// 等锁期间首个扫描者已回填缓存，后续调用直接取缓存——并发 N 次只扫一次。
/// `refresh_mods`（强制重扫语义）同样串行以防并发重复扫描，但不跳过扫描。
pub(crate) static SCAN_LOCK: LazyLock<AsyncMutex<()>> = LazyLock::new(AsyncMutex::default);

/// 选择直写快路径的分组路径守卫：路径非空、位于 `_MANAGED_` 下且目录名为
/// 严格 group_xx（复用扫描器判定，拒绝 group_0/前导零/互斥目录/任意路径）。
///
/// `write_selectedindex_direct` 内部仍有 group_xx 守卫；此处前置判定用于
/// 无效路径回退 `switch_mod` / `deselect_group_mods` 全量路径（防御），而非直接报错。
pub(crate) fn is_direct_writable_group(group_path: &Path, managed_path: &Path) -> bool {
    if group_path.as_os_str().is_empty() {
        return false;
    }
    if !mod_cache_path::path_within(group_path, managed_path) {
        return false;
    }
    match group_path.file_name() {
        Some(name) => {
            crate::core::mod_scan_util::is_normal_group_dir(&name.to_string_lossy()).is_some()
        }
        None => false,
    }
}

/// 推导启用/禁用后模组目录路径（与 `crate::core::mod_toggle::toggle_mod` 的重命名口径一致：
/// 禁用 = 前缀 `DISABLED`；启用 = 去除 `DISABLED` 前缀；幂等命中时返回原路径）。
pub(crate) fn derive_toggled_path(path: &Path, enable: bool) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = path.parent().unwrap_or(path);
    if enable {
        let stripped = crate::core::mod_scan_util::strip_disabled_prefix(&name);
        if stripped == name || stripped.is_empty() {
            path.to_path_buf() // 已启用（幂等）
        } else {
            parent.join(stripped)
        }
    } else if crate::core::mod_scan_util::is_disabled_dir(&name) {
        path.to_path_buf() // 已禁用（幂等）
    } else {
        parent.join(format!("{}{}", constants::DISABLED_PREFIX, name))
    }
}

pub(crate) fn parse_game(game: &str) -> Result<TargetGame, String> {
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
pub(crate) fn resolve_managed_context(path: &Path) -> Option<(PathBuf, PathBuf)> {
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
pub(crate) fn build_group_delta(
    game: TargetGame,
    mods_path: &Path,
    old_group_dir: &Path,
    rescan_dir: &Path,
) -> Result<GroupDelta> {
    let partial = crate::core::mod_scan_partial::scan_partial_path(mods_path, rescan_dir)
        .with_context(|| format!("partial rescan failed for {:?}", rescan_dir))?;
    // 新分组节点：partial 中覆盖 rescan_dir 的分组（可能含多个节点，按 full_path 前缀匹配）
    let new_group = partial
        .groups
        .iter()
        .find(|g| mod_cache_path::path_within(rescan_dir, &g.full_path))
        .cloned();
    // 旧状态收集（读锁；必须在写锁合并前完成——subtree_replace 会覆盖旧数据）
    let (old_group_path, old_mod_paths) =
        mod_cache_delta::collect_old_group_state(game, mods_path, old_group_dir);
    // 缓存合并（写锁）
    mod_cache_core::MOD_CACHE
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
pub(crate) fn build_removed_group_delta(
    game: TargetGame,
    mods_path: &Path,
    group_dir: &Path,
) -> GroupDelta {
    // 旧状态收集（读锁；必须在写锁裁剪前完成）
    let (old_group_path, old_mod_paths) =
        mod_cache_delta::collect_old_group_state(game, mods_path, group_dir);
    mod_cache_core::MOD_CACHE
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
pub(crate) fn log_delta(command_name: &str, delta: &GroupDelta) {
    let affected_mods = delta
        .new_group
        .as_ref()
        .map(mod_cache_delta::collect_group_mod_paths)
        .map_or(0, |paths| paths.len());
    log::debug!(
        "[delta] {} 命令 | 组={} affected_mods={} removed={}",
        command_name,
        delta.old_group_path,
        affected_mods,
        delta.old_mod_paths.len()
    );
}

/// 删除路径：优先移入系统回收站；失败时**直接报错，不再降级为永久删除**（M6 修复）。
///
/// # 行为变更（M6）
/// 旧实现在回收站不可用（网络盘、部分 Linux 文件系统、权限受限环境）时静默
/// `remove_dir_all` 永久删除且无任何确认——模组是用户数据，静默永久丢失不可接受。
/// 现在回收站不可用即返回错误，由前端弹窗告知用户「移入回收站失败，删除已取消」，
/// 用户可改用资源管理器手动删除，操作权交还用户。
pub(crate) fn trash_delete(path: &Path) -> Result<()> {
    trash::delete(path).map_err(|e| {
        // 永不静默降级为永久删除：带上下文上抛，前端可见
        anyhow::anyhow!(
            "Failed to move to trash, deletion cancelled (path not permanently deleted): {} ({}): {}",
            path.display(),
            e,
            "请手动在文件管理器中删除，或检查该路径所在的磁盘是否支持回收站"
        )
    })
}

pub(crate) fn open_path_in_explorer(path: &Path) -> Result<()> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| anyhow::anyhow!(e.to_string()))
}
