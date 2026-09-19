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
//!
//! > **Phase 6-2 归档说明（2026-09-19）**：本文件已按域拆出命令——查询/扫描域在
//! > [`mod_query_commands`](mod_query_commands)、选择/分组域在
//! > [`mod_group_commands`](mod_group_commands)、启停/恢复/操作域在
//! > [`mod_mod_commands`](mod_mod_commands)。本文件保留共享私有辅助与
//! > `mod tests`（测试直接调用模块内组装函数 build_group_delta / build_removed_group_delta）。

use crate::core::constants;
use crate::core::{mod_cache_core, mod_cache_delta, mod_cache_path};

use crate::models::enums::TargetGame;
use crate::models::mod_data::GroupDelta;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
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
        Some(name) => crate::core::mod_scan_util::is_normal_group_dir(&name.to_string_lossy()).is_some(),
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
    pub game: String,
    /// 模组根目录路径
    pub mods_path: String,
    /// 分组索引
    pub group_index: u32,
    /// 模组索引
    pub mod_index: u32,
    /// 是否为互斥组
    pub is_mutex: bool,
    /// 分组路径
    pub group_path: String,
    /// 模组路径
    pub mod_path: String,
    /// 屏幕光标 X 坐标（可选，像素值）
    pub cursor_x: Option<i32>,
    /// 屏幕光标 Y 坐标（可选，像素值）
    pub cursor_y: Option<i32>,
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
pub(crate) fn build_removed_group_delta(game: TargetGame, mods_path: &Path, group_dir: &Path) -> GroupDelta {
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

pub(crate) fn trash_delete(path: &Path) -> Result<()> {
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

pub(crate) fn open_path_in_explorer(path: &Path) -> Result<()> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| anyhow::anyhow!(e.to_string()))
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
        let result = crate::core::mod_update_pipeline::update_mod_data(TargetGame::ZZZ, &game_mods_path, &settings)
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
        let result = crate::core::mod_update_pipeline::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
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
        let result = crate::core::mod_update_pipeline::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
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
        let result = crate::core::mod_update_pipeline::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
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
        let r1 = crate::core::mod_update_pipeline::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
            .expect("第一次 update_mod_data 失败");
        let r2 = crate::core::mod_update_pipeline::update_mod_data(TargetGame::GenshinImpact, &mods, &settings)
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
        
        use std::collections::BTreeSet;
        use walkdir::WalkDir;

        // 共享运行时目录互斥：与 update_mod_data_test 并行时不得互删同一目录
        let _runtime_lock = RUNTIME_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // 窗口全局状态互斥（含 mod_scanner 内的标记创建断言测试）
        let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // panic 安全：测试退出（含断言失败）即清除窗口
        struct ClearWindow;
        impl Drop for ClearWindow {
            fn drop(&mut self) {
                crate::core::mod_scan_marker::test_clear_restore_safe_window();
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
        let result = crate::core::mod_update_pipeline::update_mod_data(TargetGame::ZZZ, &game_mods_path, &settings)
            .expect("update_mod_data 执行失败");
        assert!(result.processed_mods >= 1, "至少应处理 1 个模组");
        assert_managed_outputs(&managed);

        let markers_before = marker_inventory(&runtime);

        // 2) 还原
        let restored =
            crate::core::mod_restore::restore_all_inis(&game_mods_path).expect("restore_all_inis 执行失败");
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
        crate::core::mod_scan_light::scan_mods_light(&game_mods_path).expect("还原后重扫失败");
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
        use crate::core::{mod_cache_core, mod_cache_delta, mod_cache_path};
        
        use tempfile::TempDir;

        // 写全局 MOD_CACHE 的测试串行（容量 3 + 4 个写者，防 LRU 跨测试淘汰，
        // 见 mod_cache_core::CACHE_WRITER_TEST_LOCK 文档）
        let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
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
        let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
        mod_cache_core::MOD_CACHE
            .write()
            .set(TargetGame::GenshinImpact, &mods_path, full);

        // 执行禁用（目录重命名：ModA → DISABLED 前缀形态）
        let mod_a = group1.join("ModA");
        crate::core::mod_toggle::toggle_mod(&mod_a, false).expect("toggle_mod 失败");

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
            mod_cache_delta::collect_group_mod_paths(new_group).len(),
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
        let cached = mod_cache_core::MOD_CACHE
            .read()
            .get(TargetGame::GenshinImpact, &mods_path)
            .expect("缓存应命中");
        let g1 = mod_cache_path::find_group_node(&cached, &group1).expect("缓存应含 group_1");
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
        use crate::core::{mod_cache_core, mod_cache_path};
        
        use tempfile::TempDir;

        // 写全局 MOD_CACHE 的测试串行（容量 3 + 4 个写者，防 LRU 跨测试淘汰，
        // 见 mod_cache_core::CACHE_WRITER_TEST_LOCK 文档）
        let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
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

        let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
        mod_cache_core::MOD_CACHE
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

        let cached = mod_cache_core::MOD_CACHE
            .read()
            .get(TargetGame::GenshinImpact, &mods_path)
            .expect("缓存应命中");
        assert!(
            mod_cache_path::find_group_node(&cached, &group1).is_none(),
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
        use crate::core::{mod_cache_core, mod_cache_delta};
        
        use tempfile::TempDir;

        // 写全局 MOD_CACHE 的测试串行（容量 3 + 4 个写者，防 LRU 跨测试淘汰，
        // 见 mod_cache_core::CACHE_WRITER_TEST_LOCK 文档）
        let _cache_guard = mod_cache_core::CACHE_WRITER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join(constants::MANAGED_FOLDER);
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let mutex_root = managed.join("#Mutex");
        fs::create_dir_all(mutex_root.join("ModY")).unwrap();
        fs::write(mutex_root.join("ModY").join("mod.ini"), "[Section]\nx=1\n").unwrap();

        let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
        mod_cache_core::MOD_CACHE
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
        let new_paths = mod_cache_delta::collect_group_mod_paths(new_group);
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
        let cached = mod_cache_core::MOD_CACHE
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