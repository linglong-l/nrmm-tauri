//! 文件系统监控模块
//!
//! 使用 notify 库监控 _MANAGED_ 目录的文件变化，实现：
//! - 递归监控整个模组目录
//! - 事件平息静默期（R2）：距最后一次事件 1s 才处理批次（独立线程 + mpsc channel）；
//!   溢出退避：连续溢出批次静默期逐轮 +1s、封顶 3s，使风暴合并更大批次
//! - 增量更新：通过 `IncrementalUpdater` 收集变更路径 → consolidate → 局部重扫 → subtree_replace 写入缓存，避免全量刷新；
//!   未溢出批次额外 emit `managed-partial-update`（直接携带 GroupDelta 列表，前端原地 patch）
//! - 双粒度路由（R3/C-3a）：未溢出批次的收敛根为模组级（组目录下含 .ini 的模组目录）时，
//!   走单模组扫描 + `mod_cache::patch_single_mod`（组内位置 patch + 兄弟 Arc 复用）；
//!   组级根（group_xx / mutex 顶层根）维持整组重扫 + subtree_replace 现状路径
//! - 暂停/恢复功能：程序自身写文件时暂停监控避免循环触发
//! - 过滤无关事件：临时文件、备份文件、隐藏文件不触发刷新
//! - 缓存失效：文件变化后自动失效模组缓存并通知前端刷新

use crate::core::constants;
use crate::models::mod_data::{GroupDelta, ModData};
use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use once_cell::sync::Lazy;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// 事件环形队列容量（2026-08-19 用户指定；R2 由 120 上调至 256）。
///
/// 设计意图：正常人工操作（增删改少量模组）产生的脏目录数远小于容量，走精确增量路径；
/// 风暴场景（INI 备份/更新模组数据写入数万文件，目录数远超容量）必然溢出，
/// 快速转入分层全量降级路径（按顶层 group/mutex 分类全量），避免逐目录精确处理
/// 数千至上万个根路径的串行开销。R2 配合 1s 事件平息静默期 + 溢出退避
/// （连续溢出逐轮 +1s 封顶 3s），风暴批次合并更大、溢出率进一步下降。
const RING_CAPACITY: usize = 256;

/// notify 事件环形缓冲：目录级聚合 + 溢出标志（notify 回调线程生产，防抖线程消费）。
///
/// - **目录级去重**：入队单元是事件文件的**父目录**（同目录多次文件事件合并为 1 条），
///   `dedup` 集合使重复目录直接跳过；
/// - **有界**：容量 [`RING_CAPACITY`]；满时置 `overflow` 并**丢弃后续新目录**——
///   溢出路径的全量扫描天然覆盖被丢弃目录，语义安全（宁全量不漏扫），且不阻塞 notify 回调线程；
/// - mpsc channel 仅承载**唤醒信号**（事件本体走本缓冲），空闲时防抖线程可无限期阻塞。
struct EventRingBuffer {
    /// 脏目录队列（FIFO）
    queue: VecDeque<PathBuf>,
    /// 已在队列中的目录集合（入队去重，O(1) 判重）
    dedup: HashSet<PathBuf>,
    /// 溢出标志：队列曾满，本轮处理需走分层全量降级
    overflow: bool,
    /// 最近一次 push 时刻（防抖窗口起点）
    last_push_at: Option<Instant>,
    /// 连续溢出批次数（溢出退避依据；非溢出批次 drain 后清零）
    consecutive_overflows: u32,
}

impl EventRingBuffer {
    /// 创建空缓冲
    fn new() -> Self {
        EventRingBuffer {
            queue: VecDeque::with_capacity(RING_CAPACITY),
            dedup: HashSet::with_capacity(RING_CAPACITY),
            overflow: false,
            last_push_at: None,
            consecutive_overflows: 0,
        }
    }

    /// 入队一个脏目录（去重/溢出处理见结构体文档）。
    ///
    /// # 参数
    /// - `dir`: 变更文件所在的目录路径
    fn push(&mut self, dir: PathBuf) {
        // 无论去重命中还是溢出丢弃，均刷新防抖窗口起点（风暴持续期间推迟触发）
        self.last_push_at = Some(Instant::now());
        if self.dedup.contains(&dir) {
            return;
        }
        if self.queue.len() >= RING_CAPACITY {
            self.overflow = true;
            return;
        }
        self.dedup.insert(dir.clone());
        self.queue.push_back(dir);
    }

    /// 队列是否为空（空闲判定：防抖线程据此切换无限期阻塞等待）
    fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// 防抖是否到期：队列非空且距最近一次 push 已超过静默期时长
    ///
    /// # 参数
    /// - `debounce`: 静默期时长（事件平息 1s；溢出退避时由调用方传入延长后的值）
    fn is_ready(&self, debounce: Duration) -> bool {
        match self.last_push_at {
            None => false,
            Some(t) => !self.queue.is_empty() && t.elapsed() >= debounce,
        }
    }

    /// 取走全部数据并复位（返回脏目录列表与是否曾溢出）
    ///
    /// 溢出退避计数（R2）：溢出批次 → 计数 +1（下一轮静默期延长），非溢出批次 → 清零。
    ///
    /// # 返回
    /// `(脏目录列表, 溢出标志)`；调用后缓冲回到初始空状态（计数保留）
    fn drain(&mut self) -> (Vec<PathBuf>, bool) {
        let dirs: Vec<PathBuf> = self.queue.drain(..).collect();
        self.dedup.clear();
        let overflow = self.overflow;
        self.overflow = false;
        self.last_push_at = None;
        if overflow {
            self.consecutive_overflows = self.consecutive_overflows.saturating_add(1);
        } else {
            self.consecutive_overflows = 0;
        }
        (dirs, overflow)
    }
}

/// 溢出退避静默期计算（R2）：`base + min(连续溢出轮数 - 1, 2) × 1000ms`，
/// 绝对上限 [`constants::FILE_WATCHER_MAX_BACKOFF_MS`]。
///
/// 连续溢出 1/2/3 轮 → 1000/2000/3000ms；退避拉长静默期使风暴合并更大批次，
/// 降低 ring 溢出频率；非溢出批次（计数清零）回到 base。
fn backoff_quiet_period(base: Duration, consecutive_overflows: u32) -> Duration {
    let extra_ms = u64::from(std::cmp::min(consecutive_overflows.saturating_sub(1), 2) * 1000);
    let quiet = base + Duration::from_millis(extra_ms);
    quiet.min(Duration::from_millis(
        constants::FILE_WATCHER_MAX_BACKOFF_MS,
    ))
}

/// 全局文件监控暂停标志（引用计数，0=未暂停，>0=暂停）
///
/// 当 NRMM 自身需要写标记文件（如 groupname、selectedindex、fav 等）时，
/// 通过 [`WatcherGuard`] 递增计数暂停监控，写完后递减恢复。
///
/// 使用 `AtomicUsize` 引用计数而非裸 `AtomicBool`，支持**嵌套暂停**：
/// 外层 `update_mod_data` 暂停期间，内部 `read_or_create_marker_file` / `switch_mod`
/// 等再暂停时计数递增，内部 Drop 只递减不恢复，直到最外层 Drop 计数归零才恢复监听。
/// 修复裸 `AtomicBool` 嵌套 `store(false)` 提前解除暂停、监听器在 update 写中途恢复
/// 并把半成品并入 `MOD_CACHE` 的竞态（B2）。
///
/// 跨线程可见，增减用 `AcqRel`，读取用 `Acquire`。
pub static WATCHER_PAUSED: Lazy<AtomicUsize> = Lazy::new(|| AtomicUsize::new(0));

/// 文件监听器暂停守卫（引用计数，RAII）
///
/// `new()` 时 `WATCHER_PAUSED` 计数 +1，`Drop` 时 -1。
/// 嵌套安全：外层守卫持有期间内部再创建守卫只增计数，内部 Drop 不恢复监听，
/// 直到最外层 Drop 计数归零才恢复。panic 安全（Drop 保证 fetch_sub）。
///
/// 用法：`let _g = WatcherGuard::new();` —— 守卫作用域内监听器保持暂停，离开作用域自动恢复。
pub struct WatcherGuard;

impl WatcherGuard {
    /// 创建守卫并递增暂停计数
    pub fn new() -> Self {
        WATCHER_PAUSED.fetch_add(1, Ordering::AcqRel);
        WatcherGuard
    }
}

impl Default for WatcherGuard {
    fn default() -> Self {
        WatcherGuard::new()
    }
}

impl Drop for WatcherGuard {
    fn drop(&mut self) {
        // fetch_sub 返回更新前的值；若 prev==0 说明计数本就为 0（逻辑错误），回补防下溢
        let prev = WATCHER_PAUSED.fetch_sub(1, Ordering::AcqRel);
        if prev == 0 {
            WATCHER_PAUSED.fetch_add(1, Ordering::AcqRel);
            log::error!("WatcherGuard::drop underflow: WATCHER_PAUSED was 0 before fetch_sub");
        }
    }
}

/// 查询全局监听器是否处于暂停状态（计数 > 0）
pub fn watcher_paused() -> bool {
    WATCHER_PAUSED.load(Ordering::Acquire) > 0
}

/// 溢出分层分类（2026-08-19 用户指定的降低全量更新时间策略）。
///
/// 将脏目录合并到 `_MANAGED_` 一级子目录粒度，按类型分层返回需全量扫描的根目录：
/// 1. 仅涉及 group 目录（单个或多个）→ 涉及的各 `group_<int>` 分组全量；
/// 2. 仅涉及 mutex 目录 → 涉及的各 mutex 目录全量；
/// 3. group/mutex 混合、脏目录位于 `_MANAGED_` 之外或即 `_MANAGED_` 根自身
///    → 返回空 Vec（调用方走 `invalidate_game` + 前端 loadMods 全量兜底）。
///
/// # 参数
/// - `dirty`: drain 出的脏目录列表
/// - `managed_path`: `_MANAGED_` 目录路径
///
/// # 返回
/// 需全量扫描的根目录列表；空列表表示应走全量兜底
fn classify_overflow_roots(dirty: &[PathBuf], managed_path: &Path) -> Vec<PathBuf> {
    let mut group_roots: Vec<PathBuf> = Vec::new();
    let mut mutex_roots: Vec<PathBuf> = Vec::new();
    // 是否存在无法归类到 _MANAGED_ 一级子目录的脏路径（外部门录 / _MANAGED_ 根自身）
    let mut unclassified = false;

    for dir in dirty {
        match dir.strip_prefix(managed_path) {
            Ok(rel) => match rel.components().next() {
                Some(first) => {
                    let name = first.as_os_str().to_string_lossy().to_string();
                    let root = managed_path.join(first);
                    if crate::core::mod_scan_util::is_normal_group_dir(&name).is_some() {
                        if !group_roots.contains(&root) {
                            group_roots.push(root);
                        }
                    } else if !mutex_roots.contains(&root) {
                        mutex_roots.push(root);
                    }
                }
                None => unclassified = true, // rel 为空：dirty 即 _MANAGED_ 根自身
            },
            Err(_) => unclassified = true, // _MANAGED_ 之外的变更
        }
    }

    if unclassified || (!group_roots.is_empty() && !mutex_roots.is_empty()) {
        Vec::new()
    } else if !group_roots.is_empty() {
        group_roots
    } else {
        mutex_roots
    }
}

/// 判断批次世代是否已过期（C4 世代校验）
///
/// `stored_gen` 为防抖批次捕获创建时的世代快照，`current_gen` 为当前世代计数。
/// 当两者不一致时说明批次属于旧监听（期间 `start_watching` 再次被调用、世代已递增，
/// 例如切换游戏），静默丢弃整批，防止 `subtree_replace` 把旧路径数据合入新游戏、
/// 或 emit 跨游戏事件。
fn is_generation_stale(stored_gen: u64, current_gen: u64) -> bool {
    stored_gen != current_gen
}

/// 未溢出路径的精确增量合并：脏目录经 [`IncrementalUpdater::consolidate`] 合并根路径
/// （复用既有算法：规范化、共同前缀、向上收敛到 group/mod 目录粒度）。
///
/// # 参数
/// - `dirty`: drain 出的脏目录列表
/// - `managed_path`: `_MANAGED_` 目录路径
///
/// # 返回
/// 增量扫描根路径列表（可能为空，如全部路径被合并规则归约）
fn consolidate_incremental_roots(dirty: &[PathBuf], managed_path: &Path) -> Vec<PathBuf> {
    let mut updater = crate::core::incremental_updater::IncrementalUpdater::new(
        constants::FILE_WATCHER_DEBOUNCE_MS,
    );
    for dir in dirty {
        updater.collect(dir.clone());
    }
    updater.consolidate(managed_path)
}

/// 增量分支 delta 组装 + 缓存合并（R2）：对每个局部重扫产物，
/// 合并前（读锁）收集各组旧状态 → 写锁 `subtree_replace` 就地合并 → 组装
/// [`GroupDelta`]（new_group 为该组重扫后的完整新节点）。
///
/// 逐 partial 顺序执行（而非先全量收集旧状态再统一合并）：多个 partial
/// 收敛到同一分组时（consolidate 的发散分支），后续 partial 的旧状态
/// 反映前一个已合并的缓存，前端连续 apply 不产生扁平重复条目。
///
/// # 参数
/// - `game`: 目标游戏（缓存键）
/// - `mods_path`: Mods 根目录
/// - `scan_results`: 并行局部重扫产物（root, partial）列表
///
/// # 返回
/// 本批次全部受影响分组的 [`GroupDelta`] 列表（partial 内每组一个）
fn merge_partials_and_build_deltas(
    game: crate::models::enums::TargetGame,
    mods_path: &Path,
    scan_results: &[(PathBuf, crate::core::mod_scan_light::ScanResult)],
) -> Vec<GroupDelta> {
    let mut deltas = Vec::new();
    let mut skipped = 0usize;
    for (_root, partial) in scan_results {
        // a. 合并前（读锁）收集该 partial 各组旧状态（复用命令层共享的
        //    mod_cache::collect_old_group_state；缓存 miss 时 old 为空、键回退目录路径串）
        for g in &partial.groups {
            // P1 修复（全量 partial 差异过滤）：与缓存无实质差异的组跳过 delta emit。
            // 全量兜底 partial 携带全部 109 组，无条件 emit 曾导致前端
            // applyGroupDelta ×110/轮（两轮 220 次）的选择后 UI 重排抖动；
            // 过滤后仅实际变化的组到达前端（缓存照常 subtree_replace，签名
            // 复用零拷贝，语义不变）。判定读锁短持有、O(组内) 比较，总 O(全树)。
            if !crate::core::mod_cache::group_has_real_change(game, mods_path, g) {
                skipped += 1;
                continue;
            }
            let (old_group_path, old_mod_paths) =
                crate::core::mod_cache::collect_old_group_state(game, mods_path, &g.full_path);
            // c. delta：new_group 为该组重扫后的完整新节点（磁盘为唯一真相源）
            deltas.push(GroupDelta {
                old_group_path,
                new_group: Some(g.clone()),
                old_mod_paths,
            });
        }
        // b. 写锁 subtree_replace 就地合并缓存（现状逻辑；缓存 miss 时内部跳过）
        crate::core::mod_cache::MOD_CACHE
            .write()
            .subtree_replace(game, mods_path, partial.clone());
    }
    if skipped > 0 {
        log::info!(
            "[watcher] delta 差异过滤 | 跳过无变化组={} 实际emit={}",
            skipped,
            deltas.len()
        );
    }
    deltas
}

/// 路径段键（C-3a 私有）：Windows 大小写不敏感比较用（其他平台原样）。
fn seg_key(s: &str) -> String {
    #[cfg(windows)]
    {
        s.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        s.to_string()
    }
}

/// 路径段切分（C-3a 私有）：统一分隔符、剥 Windows verbatim 前缀（`\\?\`）、
/// 保留**原大小写**（`group_xx` 判定对大小写敏感，需对齐 snap/扫描口径）。
fn path_segments_original_case(p: &Path) -> Vec<String> {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").map(str::to_string).unwrap_or(s);
    s.split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .map(str::to_string)
        .collect()
}

/// 计算收敛根相对 `_MANAGED_` 的路径段列表（C-3a 粒度路由依据）。
///
/// 收敛根来自 `IncrementalUpdater::consolidate` 的 canonicalize 产物（Windows 为
/// `\\?\` verbatim 形态），与 watcher 内部的普通形态 managed_path 经段键比较统一。
///
/// # 返回
/// `Some(segs)`：根在 `_MANAGED_` 子树内（根即 `_MANAGED_` 自身时为空段列表）；
/// `None`：根不在 `_MANAGED_` 下。
fn managed_relative_segments(root: &Path, managed_path: &Path) -> Option<Vec<String>> {
    let root_segs = path_segments_original_case(root);
    let managed_segs = path_segments_original_case(managed_path);
    if root_segs.len() < managed_segs.len() {
        return None;
    }
    for (a, b) in root_segs[..managed_segs.len()].iter().zip(&managed_segs) {
        if seg_key(a) != seg_key(b) {
            return None;
        }
    }
    Some(root_segs[managed_segs.len()..].to_vec())
}

/// C-3a 粒度路由判定：收敛根是否为模组级。
///
/// 与 `snap_to_group_or_mod_boundary` 语义一致：**组级根** = `group_xx` 目录或
/// `_MANAGED_` 一级子目录（mutex 顶层根，含根即模组的形态）；**模组级根** = 其下
/// 含 `.ini` 的模组目录（snap 保证深层非 `.ini` 目录不会成为根——含 `.ini` 判定在
/// 收敛时完成，此处的结构判定：相对深度 ≥ 2 且末段非 `group_xx` 目录名）。
///
/// # 参数
/// - `root`: 收敛根路径（任意形态，容忍 verbatim/大小写差异）。
/// - `managed_path`: `_MANAGED_` 目录路径。
fn is_mod_level_root(root: &Path, managed_path: &Path) -> bool {
    managed_relative_segments(root, managed_path).is_some_and(|segs| {
        segs.len() >= 2
            && segs
                .last()
                .is_some_and(|last| !crate::core::mod_scan_util::is_group_xx_dir(last))
    })
}

/// 取模组级根所属的顶层组目录（C-3a）：delta 旧状态收集与批次归并键
/// （`_MANAGED_` 一级子目录形态；路径等价比较容忍大小写形态差异）。
fn top_level_group_dir(root: &Path, managed_path: &Path) -> Option<PathBuf> {
    let segs = managed_relative_segments(root, managed_path)?;
    let first = segs.first()?;
    Some(managed_path.join(first))
}

/// 组级根并行局部重扫（现状路径提炼，C-3a 后溢出与组级路由共用）。
///
/// rayon 并行执行 `scan_partial_path`（纯读磁盘+解析，锁外执行）；单个根失败仅
/// 跳过该根（记日志），不中断其余。根不存在（整组目录被删除）→ 失效该游戏缓存
/// （全量兜底——`scan_partial_path` 会退化为全量扫描，且缓存会残留已删分组）。
fn scan_group_roots(
    game: crate::models::enums::TargetGame,
    mods_path: &Path,
    roots: &[PathBuf],
) -> Vec<(PathBuf, crate::core::mod_scan_light::ScanResult)> {
    roots
        .par_iter()
        .filter_map(|sub| {
            if !sub.exists() {
                log::info!(
                    "[Incremental] scan root no longer exists, invalidating game cache: {:?}",
                    sub
                );
                let mut cache_guard = crate::core::mod_cache::MOD_CACHE.write();
                cache_guard.invalidate_game(game);
                return None;
            }
            match crate::core::mod_scan_partial::scan_partial_path(mods_path, sub) {
                Ok(partial) => Some((sub.clone(), partial)),
                Err(e) => {
                    log::error!(
                        "[Incremental] scan_partial_path failed for {}: {}",
                        sub.display(),
                        e
                    );
                    None
                }
            }
        })
        .collect()
}

/// 模组级批次条目：`(模组根路径, 新模组数据)`（`None` = 删除 patch）。
type ModPatchItem = (PathBuf, Option<Arc<ModData>>);

/// 模组级批次按顶层组归并后统一 patch + 组装 delta（C-3a）。
///
/// 同批次多个模组级根落同一顶层组 → 合并为**一次**组 delta（一次旧状态收集 +
/// 一次写锁批次 + 单个 new_group），避免重复 make_mut/重复 delta。批次内逐模组
/// `patch_single_mod`（单模组扫描产物或删除 patch）；全部未命中（缓存 miss/组节点
/// 无定位）时该组不产出 delta，未命中根交还调用方降级组级重扫。
///
/// # 参数
/// - `game`: 目标游戏（缓存键）。
/// - `mods_path`: Mods 根目录。
/// - `managed_path`: `_MANAGED_` 目录路径（顶层组推导）。
/// - `batch`: `(模组根路径, 新模组数据)` 列表（`None` = 删除 patch）。
///
/// # 返回
/// `(该批次产出的 GroupDelta 列表, 归并后的顶层组数, 未命中的根列表)`
fn patch_mod_batch_and_build_deltas(
    game: crate::models::enums::TargetGame,
    mods_path: &Path,
    managed_path: &Path,
    batch: Vec<ModPatchItem>,
) -> (Vec<GroupDelta>, usize, Vec<PathBuf>) {
    if batch.is_empty() {
        return (Vec::new(), 0, Vec::new());
    }
    // 按顶层组归并（保持批次内出现顺序，组内逐模组顺序稳定）
    let mut order: Vec<PathBuf> = Vec::new();
    let mut buckets: HashMap<PathBuf, Vec<ModPatchItem>> = HashMap::new();
    for (root, new_mod) in batch {
        let Some(top) = top_level_group_dir(&root, managed_path) else {
            log::warn!("[Incremental] mod 级根不在 _MANAGED_ 下，跳过: {:?}", root);
            continue;
        };
        if !buckets.contains_key(&top) {
            order.push(top.clone());
        }
        buckets.entry(top).or_default().push((root, new_mod));
    }

    let mut deltas = Vec::new();
    let mut missed: Vec<PathBuf> = Vec::new();
    let merged_groups = order.len();
    for top in &order {
        let items = buckets.remove(top).unwrap_or_default();
        // 读锁收集旧状态（缓存 miss 时 old 为空、键回退目录路径串）
        let (old_group_path, old_mod_paths) =
            crate::core::mod_cache::collect_old_group_state(game, mods_path, top);
        // 写锁逐模组 patch（同组批次一次锁定；返回顶层组节点含定位链重建结果）
        let mut final_node: Option<crate::models::mod_data::ModGroupData> = None;
        {
            let mut cache = crate::core::mod_cache::MOD_CACHE.write();
            for (root, new_mod) in items {
                match cache.patch_single_mod(game, mods_path, &root, new_mod) {
                    Some(node) => final_node = Some(node),
                    None => {
                        log::warn!(
                            "[Incremental] mod 级 patch 未命中（缓存 miss/组节点无定位），降级组级重扫: {:?}",
                            root
                        );
                        missed.push(root);
                    }
                }
            }
        }
        // delta：new_group 为该组 patch 后的完整顶层节点（与组级 partial 同形状，
        // 前端 applyGroupDelta 无感）；全部未命中时缓存未变更，不产出
        if let Some(node) = final_node {
            deltas.push(GroupDelta {
                old_group_path,
                new_group: Some(node),
                old_mod_paths,
            });
        }
    }
    (deltas, merged_groups, missed)
}

/// 增量批次双粒度路由（R3/C-3a）：深部单文件变更（如 `group_2/ModA/textures/x.png`）
/// 的收敛根从「整组重扫」降为「单模组重扫 + 组内位置 patch + 兄弟 Arc 复用」。
///
/// - **模组级根**（`is_mod_level_root`）：存在 → `scan_single_mod` 单模组扫描（扫描
///   失败降级组级）；不存在（consolidate 后被删的竞态窗口）→ 删除 patch。
/// - **组级根**（group_xx / mutex 顶层根）：维持现状——并行 `scan_partial_path`
///   整组重扫 + `subtree_replace`（含 R2 delta 组装）。
/// - **批次聚合**：模组级根按顶层组归并，同组一次 delta（`patch_mod_batch_and_build_deltas`）。
/// - **降级兜底**：单模组扫描失败或 patch 未命中（如普通组内嵌套目录——轻扫不递归
///   NormalGroup，深层路径在缓存树中无模组定位）的根回退组级重扫（现状路径）。
///
/// delta 顺序与缓存合并顺序一致（组级 → 模组级 → 降级组级），前端按序 apply 的
/// 终态 = 最后一次合并的缓存状态。
///
/// # 返回
/// 本批次全部受影响分组的 [`GroupDelta`] 列表。
fn route_incremental_batch(
    game: crate::models::enums::TargetGame,
    mods_path: &Path,
    managed_path: &Path,
    scan_roots: &[PathBuf],
) -> Vec<GroupDelta> {
    let mut group_roots: Vec<PathBuf> = Vec::new();
    let mut mod_batch: Vec<ModPatchItem> = Vec::new();
    let mut mod_roots_total = 0usize;

    for root in scan_roots {
        if is_mod_level_root(root, managed_path) {
            mod_roots_total += 1;
            if root.exists() {
                match crate::core::mod_scan_light::scan_single_mod(mods_path, root) {
                    Ok((arc, _group_dir)) => mod_batch.push((root.clone(), Some(arc))),
                    Err(e) => {
                        log::warn!(
                            "[Incremental] scan_single_mod failed for {} (降级组级重扫): {}",
                            root.display(),
                            e
                        );
                        group_roots.push(root.clone());
                    }
                }
            } else {
                // 模组级目录被删除（consolidate 后竞态窗口）→ 删除 patch；
                // patch 未命中时降级组级（组级根不存在 → invalidate_game 兜底）
                mod_batch.push((root.clone(), None));
            }
        } else {
            group_roots.push(root.clone());
        }
    }

    // 组级根：已消失 → 删除语义（P1 修复）；存在 → 现状并行整组重扫 + subtree_replace
    //
    // P1 修复：整组被删/整组重命名的收敛根（组级）已不存在时，`scan_partial_path`
    // 的兜底是全量扫描（6542 模组 × 1.1s）——改为读旧状态 + `subtree_remove`
    // 裁剪缓存 + 组装 `new_group: None` 删除 delta（remove_group 命令同契约，
    // 前端 applyGroupDelta 已支持删除语义）。
    let mut vanished_deltas: Vec<GroupDelta> = Vec::new();
    let mut live_group_roots: Vec<PathBuf> = Vec::new();
    for r in &group_roots {
        if r.exists() {
            live_group_roots.push(r.clone());
        } else {
            let (old_group_path, old_mod_paths) =
                crate::core::mod_cache::collect_old_group_state(game, mods_path, r);
            let removed = crate::core::mod_cache::MOD_CACHE
                .write()
                .subtree_remove(game, mods_path, r);
            log::info!(
                "[Incremental] 组级根已消失 → 删除语义 {} (裁剪 {} 条)",
                r.display(),
                removed
            );
            vanished_deltas.push(GroupDelta {
                old_group_path,
                new_group: None,
                old_mod_paths,
            });
        }
    }
    let scan_results = scan_group_roots(game, mods_path, &live_group_roots);
    let mut deltas = merge_partials_and_build_deltas(game, mods_path, &scan_results);
    deltas.extend(vanished_deltas);

    // 模组级批次：按顶层组归并后统一 patch + delta（未命中根收集降级）
    let (mod_deltas, merged_groups, missed_roots) = patch_mod_batch_and_build_deltas(
        game,
        mods_path,
        managed_path,
        std::mem::take(&mut mod_batch),
    );
    deltas.extend(mod_deltas);

    // 降级兜底：patch 未命中的根走组级重扫（现状路径；scan_partial_path 自动扩展
    // 到所属整组），保证这些根与变更前的行为一致（不漏更新）
    if !missed_roots.is_empty() {
        let fallback_results = scan_group_roots(game, mods_path, &missed_roots);
        let fallback_deltas = merge_partials_and_build_deltas(game, mods_path, &fallback_results);
        deltas.extend(fallback_deltas);
    }

    // 埋点（批次级，与 R2 静默期埋点并列）
    log::debug!(
        "[watcher] mod 级 patch | mod_roots={} group_roots={} (live={} vanished={}) 合并组={}",
        mod_roots_total,
        group_roots.len(),
        live_group_roots.len(),
        group_roots.len() - live_group_roots.len(),
        merged_groups
    );
    deltas
}

/// `managed-partial-update` 事件 payload（R2）：直接携带分组增量列表。
///
/// 前端零额外 IPC 往返：对 `deltas` 逐组 `applyGroupDelta` 原地 patch
/// （复用 R1 契约），随后到达的 folder-changed 防抖 refresh 被回声抑制跳过。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedPartialUpdatePayload {
    /// 当前游戏（与既有事件一致的 Debug 字符串形态）
    game: String,
    /// 受影响分组的增量列表（多 root 批次可能含多个组）
    deltas: Vec<GroupDelta>,
}

/// 主窗口当前是否可见（不可见时静默合并缓存，不 emit 前端事件）
fn window_visible(app_handle: &AppHandle) -> bool {
    app_handle
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// 处理完成后通知前端：窗口可见时 emit `managed-folder-changed`（全量 refresh
/// 兜底信号）；窗口隐藏时静默（缓存已更新）。
fn emit_folder_changed(app_handle: &AppHandle, managed_path: &Path) {
    if window_visible(app_handle) {
        let _ = app_handle.emit(
            crate::core::events::MANAGED_FOLDER_CHANGED,
            managed_path.as_os_str(),
        );
    } else {
        log::info!("[Incremental] window hidden -> merged cache silently");
    }
}

/// 增量批次补充通知（R2）：emit `managed-partial-update`，payload 直接携带
/// [`GroupDelta`] 列表（推送替代拉取，前端零额外 IPC 往返直接 patch）。
///
/// 须在 [`emit_folder_changed`] **之后**调用：folder-changed 先到 → 前端 500ms
/// 防抖 timer 先建 → partial-update patch 置位 lastLocalPatchAt → timer 到期时
/// 被回声抑制跳过 → 全量 refresh 仅余兜底路径。窗口隐藏时静默。
///
/// # 参数
/// - `app_handle`: Tauri 应用句柄
/// - `game`: 当前游戏（payload 字段）
/// - `deltas`: 本批次组装的分组增量列表
fn emit_managed_partial_update(
    app_handle: &AppHandle,
    game: crate::models::enums::TargetGame,
    deltas: &[GroupDelta],
) {
    if window_visible(app_handle) {
        let payload = ManagedPartialUpdatePayload {
            game: format!("{game:?}"),
            deltas: deltas.to_vec(),
        };
        let _ = app_handle.emit(crate::core::events::MANAGED_PARTIAL_UPDATE, payload);
    }
}

/// 文件监控器结构体
///
/// 封装 notify 的 `RecommendedWatcher`，配合独立的防抖线程处理事件。
/// 事件经 `EventRingBuffer`（目录级聚合环形队列）传递，channel 仅承载唤醒信号。
/// 未溢出走 `IncrementalUpdater` 精确增量；溢出按顶层 group/mutex 分层全量降级。
pub struct FileWatcher {
    /// notify 库的文件监控器实例。
    /// `Option` 允许 `stop_watching` 时通过 `take` 释放底层 OS 资源。
    watcher: Option<RecommendedWatcher>,
    /// 当前被监控的 `_MANAGED_` 目录路径（完整路径，如 `.../Mods/_MANAGED_`）。
    watched_path: Option<PathBuf>,
    /// 实例级别的暂停标志，独立于全局 `WATCHER_PAUSED`。
    /// 防抖线程同时检查两者，任一为 `true` 则跳过事件触发。
    paused: Arc<AtomicBool>,
    /// mpsc channel 的发送端（仅承载唤醒信号，事件本体走 `EventRingBuffer`）。
    /// 保持所有权防止 channel 关闭（drop 后接收端收到 `Disconnected`，防抖线程退出）。
    _tx: Option<mpsc::Sender<()>>,
    /// 防抖处理线程句柄。`spawn_safe` 不返回 `JoinHandle`（内部丢弃），故恒为 `None`。
    /// 线程退出依赖 channel `Disconnected`；批次正确性由 `generation` 世代校验保证，
    /// 无需在此保存句柄 `join`。
    _debounce_thread: Option<std::thread::JoinHandle<()>>,
    /// 监听世代计数（C4）：每次 `start_watching` 递增，防抖线程捕获创建时的世代快照，
    /// drain 后、写缓存/emit 前校验世代。
    /// 切换游戏（旧线程已有 [channel] 中 un-drain 批次、正在 rayon 扫描）时，
    /// 新 `start_watching` 递增世代使旧批次过期，整批静默丢弃，
    /// 防止 `subtree_replace(current_game, 旧路径, partial)` 合入错误条目 + 跨游戏 emit。
    generation: Arc<AtomicU64>,
}

impl Default for FileWatcher {
    fn default() -> Self {
        FileWatcher::new()
    }
}

impl FileWatcher {
    /// 创建新的空 `FileWatcher` 实例
    ///
    /// 此时尚未开始监控任何目录，所有字段均为 `None`。
    /// 需要调用 [`start_watching`](Self::start_watching) 启动监控。
    /// 实例级暂停标志 `paused` 初始化为 `false`。
    pub fn new() -> Self {
        FileWatcher {
            watcher: None,
            watched_path: None,
            paused: Arc::new(AtomicBool::new(false)),
            _tx: None,
            _debounce_thread: None,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 暂停文件监控事件处理
    ///
    /// 设置 `paused` 标志为 `true`，防抖线程检测到此标志后会跳过事件触发（仅重置内部状态不触发刷新）。
    /// 注意：底层 watcher 本身仍在接收 OS 文件事件，只是不处理。
    /// 使用 `Ordering::SeqCst` 保证防抖线程立即可见。
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// 恢复文件监控事件处理
    ///
    /// 设置 `paused` 标志为 `false`，防抖线程恢复正常处理流程。
    /// 使用 `Ordering::SeqCst` 保证顺序一致性。
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    /// 开始监控指定游戏模组目录下的 `_MANAGED_` 文件夹
    ///
    /// # 实现步骤
    /// 1. **停止旧监控**：调用 `stop_watching()` 释放已有资源，避免重复监控
    /// 2. **确保目录存在**：拼接 `_MANAGED_` 路径，不存在则创建
    /// 3. **创建唤醒信号通道 + 事件环形缓冲**：notify 回调轻量过滤后将变更文件
    ///    的父目录聚合进 `EventRingBuffer`（容量 [`RING_CAPACITY`]，目录级去重），
    ///    并 send 空信号唤醒防抖线程
    /// 4. **初始化 notify watcher**：使用 `recommended_watcher` 递归监控整个目录
    /// 5. **启动防抖线程**：队列空时无限期阻塞（空闲零轮询）；非空时 50ms 超时检测
    ///    事件平息静默期到期（1s，溢出退避时延长）→ drain 环形队列 → 增量/分层处理
    ///    → emit 前端事件
    ///
    /// # 防抖 & 增量/分层更新管线（防抖线程内部）
    /// - 未溢出（≤256 脏目录）：`consolidate()` 合并根路径 → 双粒度路由（C-3a）：
    ///   模组级根走 `scan_single_mod()` 单模组扫描 → 短锁 `patch_single_mod()` 组内
    ///   位置 patch（兄弟 Arc 复用）；组级根 rayon 并行 `scan_partial_path()` 整组
    ///   局部重扫 → 短锁 `subtree_replace()` 替换缓存子树。两类均组装各受影响
    ///   分组的 [`GroupDelta`]（R2）
    /// - 溢出（队列曾满）：按顶层 group/mutex 分层全量降级（见 [`classify_overflow_roots`]），
    ///   混合场景走 `invalidate_game` + 前端全量 loadMods 兜底
    /// - emit `managed-folder-changed`；未溢出批次随后 emit `managed-partial-update`
    ///   （携带 GroupDelta 列表）；窗口隐藏时仅更新缓存
    ///
    /// # 参数
    /// - `app_handle`: Tauri 应用句柄，用于发送前端事件
    /// - `game_mods_path`: 游戏 Mods 目录路径（会自动拼接 `_MANAGED_`）
    ///
    /// # Errors
    /// - 目录创建失败（`std::fs::create_dir_all` 返回 IO 错误）
    /// - notify watcher 初始化失败（`notify::recommended_watcher` 或 `watcher.watch` 返回错误）
    pub fn start_watching(&mut self, app_handle: AppHandle, game_mods_path: &Path) -> Result<()> {
        // 先停止之前的监控，避免重复监控
        self.stop_watching();

        // 递增监听世代（C4）：本批次捕获的世代快照若在后续被再次 start_watching 覆盖，
        // 则该批次属旧监听（切换游戏），drain 后校验时静默丢弃，防跨游戏合并错误。
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let generation_clone = self.generation.clone();

        let managed_path = game_mods_path.join(constants::MANAGED_FOLDER);
        if !managed_path.exists() {
            std::fs::create_dir_all(&managed_path)?;
        }

        // 唤醒信号通道：notify 回调 push 环形队列后 send 一个空信号唤醒防抖线程
        // （事件本体走 EventRingBuffer，channel 仅承载唤醒信号——空闲时防抖线程可无限期阻塞）
        let (tx, rx) = mpsc::channel::<()>();

        // 事件环形缓冲：notify 回调线程生产（目录级聚合），防抖线程消费
        let ring = Arc::new(Mutex::new(EventRingBuffer::new()));
        let ring_cb = ring.clone();
        let backup_suffix = format!(".{}", constants::BACKUP_EXTENSION);
        let manager_backup_suffix = format!(".{}", constants::INI_MANAGER_BACKUP_SUFFIX);

        let tx_clone = tx.clone();
        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                // watcher 回调在 notify 内部线程：轻量过滤 → 父目录聚合入环形队列 → 唤醒信号。
                // 过滤逻辑前移至此（原在防抖线程逐事件处理），使风暴期间垃圾事件不占队列容量。
                if let Ok(event) = res {
                    use notify::EventKind;
                    // 仅关注 Create/Remove/Modify 事件，忽略 Access 等元数据事件
                    if !matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
                    ) {
                        return;
                    }
                    let mut pushed = false;
                    for p in event.paths {
                        let path_str = p.to_string_lossy();
                        let file_name = p
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        // 过滤临时文件（.tmp）、备份文件（NRMM 注入备份 + INI 启动备份）、隐藏文件
                        let relevant = !path_str.contains(".tmp")
                            && !path_str.ends_with(&backup_suffix)
                            && !path_str.ends_with(&manager_backup_suffix)
                            && !file_name.starts_with('.');
                        if relevant {
                            if let Some(parent) = p.parent() {
                                if let Ok(mut ring) = ring_cb.lock() {
                                    ring.push(parent.to_path_buf());
                                }
                                pushed = true;
                            }
                        }
                    }
                    if pushed {
                        // 尽力而为的唤醒信号（通道满/关闭均忽略：防抖线程超时路径兜底）
                        let _ = tx_clone.send(());
                    }
                }
            })?;

        // 递归监控 _MANAGED_ 目录下所有文件和子目录
        watcher.watch(&managed_path, RecursiveMode::Recursive)?;

        let managed_path_clone = managed_path.clone();
        let debounce_duration = Duration::from_millis(constants::FILE_WATCHER_DEBOUNCE_MS);
        let paused_clone = self.paused.clone();
        let ring_debounce = ring.clone();

        // 启动防抖线程：消费环形队列（经唤醒信号），实现防抖 + 增量/分层更新管线
        // 管线流程：ring.push（回调线程聚合目录）→ 防抖到期 drain →
        //   未溢出：consolidate（合并根路径）→ rayon 并行 scan_partial_path → 短锁 subtree_replace
        //   溢出：按顶层 group/mutex 分层全量降级（单分组/分组集/mutex 集/invalidate 全量）
        // → emit（通知前端）
        crate::utils::spawn_safe(
            "file_watcher_debounce",
            std::panic::AssertUnwindSafe(move || {
                loop {
                    // 空闲零轮询：队列空时无限期阻塞等待唤醒信号（事件到达即醒，
                    // 消除原 50ms recv_timeout 的常驻唤醒开销）
                    let ring_empty = ring_debounce.lock().map(|r| r.is_empty()).unwrap_or(true);
                    if ring_empty {
                        match rx.recv() {
                            Ok(()) => continue,
                            // channel 发送端已关闭（stop_watching 时 drop tx），线程退出
                            Err(_) => break,
                        }
                    }

                    // 队列非空：50ms 超时轮询（继续接收新事件 + 检测静默期到期）
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(()) => {}
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            // 事件平息静默期（R2）：距最后事件 1s 才处理批次；
                            // 溢出退避：连续溢出轮数逐轮 +1s（封顶 FILE_WATCHER_MAX_BACKOFF_MS），
                            // 使风暴合并更大批次、降低 ring 溢出频率
                            let (ready, backoff_rounds) = ring_debounce
                                .lock()
                                .map(|r| {
                                    (
                                        r.is_ready(backoff_quiet_period(
                                            debounce_duration,
                                            r.consecutive_overflows,
                                        )),
                                        r.consecutive_overflows,
                                    )
                                })
                                .unwrap_or((false, 0));
                            if !ready {
                                continue;
                            }

                            // 暂停检查前置（C11）：暂停期间不入 drain——事件保留在 ring，
                            // 恢复后仍会处理（原实现在 drain 后丢弃 = 外部工具在此窗口写入
                            // Mods 目录的变更既不触发增量也不失效缓存，MOD_CACHE 永久陈旧；
                            // 且避免 start/stop 无条件重置实例暂停标志而吞掉前端显式暂停意图）
                            let paused_global = WATCHER_PAUSED.load(Ordering::Acquire) > 0;
                            let paused_local = paused_clone.load(Ordering::SeqCst);
                            if paused_global || paused_local {
                                continue;
                            }

                            // 防抖到期：drain 环形队列（脏目录 + 溢出标志）
                            let (dirty_dirs, overflow) = ring_debounce
                                .lock()
                                .map(|mut r| r.drain())
                                .unwrap_or((Vec::new(), false));
                            if dirty_dirs.is_empty() {
                                continue;
                            }

                            // 世代校验（C4）：start_watching 递增世代后，本批次属旧监听
                            // （切换游戏）→ 静默丢弃整批，防止合入错误游戏条目 + 跨游戏 emit
                            let current_generation = generation_clone.load(Ordering::SeqCst);
                            if is_generation_stale(gen, current_generation) {
                                log::debug!(
                                    "[file_watcher] 丢弃过期世代 {} 批次（当前 {}）",
                                    gen,
                                    current_generation
                                );
                                continue;
                            }

                            let current_game =
                                crate::config::settings_store::get_settings().target_game;
                            let mods_path_buf = managed_path_clone
                                .parent()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_else(|| managed_path_clone.clone());

                            // 双路径处理：溢出走分层全量降级；未溢出走精确增量
                            let scan_roots: Vec<PathBuf> = if overflow {
                                classify_overflow_roots(&dirty_dirs, &managed_path_clone)
                            } else {
                                consolidate_incremental_roots(&dirty_dirs, &managed_path_clone)
                            };
                            if scan_roots.is_empty() {
                                // 分类返回空 = 走 invalidate_game 全量兜底（无需扫描根）
                                {
                                    let mut cache_guard = crate::core::mod_cache::MOD_CACHE.write();
                                    cache_guard.invalidate_game(current_game);
                                }
                                log::info!(
                                    "[Incremental] overflow={} mode=full_rescan dirty_dirs={}",
                                    overflow,
                                    dirty_dirs.len()
                                );
                                emit_folder_changed(&app_handle, &managed_path_clone);
                                continue;
                            }

                            log::info!(
                                "[Incremental] overflow={} mode={} roots={} dirty_dirs={}",
                                overflow,
                                if overflow {
                                    "tiered_full"
                                } else {
                                    "incremental"
                                },
                                scan_roots.len(),
                                dirty_dirs.len()
                            );

                            // 短锁合并缓存 + 增量分支 delta 组装（R2 + R3/C-3a）：
                            // - 未溢出：双粒度路由（`route_incremental_batch`）——模组级收敛根
                            //   走单模组扫描 + `patch_single_mod`（兄弟 Arc 复用），组级根维持
                            //   整组重扫 + `subtree_replace`（现状路径）；两类均产出各组
                            //   GroupDelta（事件直接携带，前端零额外 IPC 原地 patch）
                            // - 溢出（分层全量降级）：并行整组重扫 + 仅短锁合并缓存、不组装
                            //   delta（不发 partial-update，前端 folder-changed 全量 refresh 兜底）
                            let deltas: Vec<GroupDelta> = if overflow {
                                let scan_results =
                                    scan_group_roots(current_game, &mods_path_buf, &scan_roots);
                                if !scan_results.is_empty() {
                                    let mut cache_guard = crate::core::mod_cache::MOD_CACHE.write();
                                    for (_root, partial) in &scan_results {
                                        cache_guard.subtree_replace(
                                            current_game,
                                            &mods_path_buf,
                                            partial.clone(),
                                        );
                                    }
                                }
                                Vec::new()
                            } else {
                                route_incremental_batch(
                                    current_game,
                                    &mods_path_buf,
                                    &managed_path_clone,
                                    &scan_roots,
                                )
                            };

                            // 埋点：静默期批次（溢出退避计数可见）
                            log::debug!(
                                "[watcher] 静默期批次 | dirty={} overflow={} roots={} deltas={} backoff={}",
                                dirty_dirs.len(),
                                overflow,
                                scan_roots.len(),
                                deltas.len(),
                                backoff_rounds
                            );

                            // 通知前端：folder-changed 先（前端防抖 timer 先建），
                            // 增量分支随后发 partial-update（patch 置位时间戳 →
                            // timer 到期时被回声抑制跳过，全量 refresh 仅余兜底）
                            emit_folder_changed(&app_handle, &managed_path_clone);
                            if !overflow && !deltas.is_empty() {
                                emit_managed_partial_update(&app_handle, current_game, &deltas);
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            }),
        );

        self.watcher = Some(watcher);
        self._tx = Some(tx);
        self._debounce_thread = None;
        self.watched_path = Some(managed_path);

        Ok(())
    }

    /// 停止监控并清理所有资源
    ///
    /// 1. drop `watcher`（`Option::take` → 释放 notify 资源，自动调用 `unwatch` 停止 OS 级别监控）
    /// 2. drop `_tx`（关闭 channel 发送端，防抖线程 `recv_timeout` 收到 `Disconnected` 退出循环）
    /// 3. 防抖线程无需 `join`：`spawn_safe` 不返回 `JoinHandle`（`_debounce_thread` 恒 `None`），
    ///    线程退出由 channel `Disconnected` 保证；批次正确性由 `generation` 世代校验兜底
    /// 4. 清空 `watched_path`、`updater`（不重置实例级暂停标志——暂停意图由前端显式 `resume` 取消，C11）
    pub fn stop_watching(&mut self) {
        // drop watcher 会自动调用 unwatch
        self.watcher = None;
        // drop tx 会导致 rx 端收到 Disconnected，防抖线程退出循环
        self._tx = None;
        // 防抖线程退出由 channel `Disconnected` 保证；`spawn_safe` 不返回句柄，故此处恒不触发
        // （保留以显式表达意图；批次正确性由 `generation` 世代校验兜底，见 `generation` 字段）
        if let Some(handle) = self._debounce_thread.take() {
            let _ = handle.join();
        }
        self.watched_path = None;
    }

    /// 判断是否正在监控某个目录
    ///
    /// 同时满足两个条件才视为运行中：`watched_path` 存在（有目标路径）且 `watcher` 存在（watcher 活跃）。
    /// 停止监控后两者均为 `None`，此时返回 `false`。
    pub fn is_running(&self) -> bool {
        self.watched_path.is_some() && self.watcher.is_some()
    }

    /// 获取当前监控的 `_MANAGED_` 路径
    ///
    /// 返回 `Some(&PathBuf)` 表示正在监控中，`None` 表示未监控。
    /// 用于 `window-shown` 时对比是否需要切换监控路径。
    pub fn watched_path(&self) -> Option<&PathBuf> {
        self.watched_path.as_ref()
    }

    /// 切换监控路径（先停止旧监控再启动新监控）
    ///
    /// 等价于连续调用 `stop_watching()` 然后 `start_watching()` 的便捷方法。
    ///
    /// # 参数
    /// - `app_handle`: Tauri 应用句柄
    /// - `game_mods_path`: 新的游戏 Mods 目录路径
    ///
    /// # Errors
    /// 同 `start_watching`：目录创建失败或 notify watcher 初始化失败。
    pub fn switch_watched_path(
        &mut self,
        app_handle: AppHandle,
        game_mods_path: &Path,
    ) -> Result<()> {
        self.stop_watching();
        self.start_watching(app_handle, game_mods_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// WatcherGuard 引用计数：嵌套创建/销毁时计数正确（B2 修复验证）
    ///
    /// 用 before 差值验证，避免全局 `WATCHER_PAUSED` 在并发测试下的干扰：
    /// 外层守卫持有期间，内部再创建守卫只增计数，内部 Drop 只递减不归零，
    /// 直到最外层 Drop 计数才恢复到 before。
    #[test]
    fn watcher_guard_nested_count() {
        let before = WATCHER_PAUSED.load(Ordering::Acquire);
        {
            let _g1 = WatcherGuard::new();
            assert_eq!(WATCHER_PAUSED.load(Ordering::Acquire), before + 1);
            {
                let _g2 = WatcherGuard::new();
                assert_eq!(WATCHER_PAUSED.load(Ordering::Acquire), before + 2);
            } // _g2 drop → before+1（外层未 Drop，监听器仍暂停）
            assert_eq!(WATCHER_PAUSED.load(Ordering::Acquire), before + 1);
        } // _g1 drop → before
        assert_eq!(WATCHER_PAUSED.load(Ordering::Acquire), before);
    }

    /// EventRingBuffer 目录级去重：同目录多次 push 仅入队一次
    #[test]
    fn ring_buffer_dedup() {
        let mut ring = EventRingBuffer::new();
        let dir = PathBuf::from("/mods/_MANAGED_/group_1/ModA");
        ring.push(dir.clone());
        ring.push(dir.clone());
        ring.push(dir.clone());
        let (dirs, overflow) = ring.drain();
        assert_eq!(dirs.len(), 1, "同目录多次事件应合并为 1 条");
        assert!(!overflow);
        assert!(ring.is_empty(), "drain 后应回到空状态");
    }

    /// EventRingBuffer 溢出：容量满后丢弃新目录并置溢出标志（被丢弃目录由全量路径覆盖）
    #[test]
    fn ring_buffer_overflow() {
        let mut ring = EventRingBuffer::new();
        for i in 0..RING_CAPACITY {
            ring.push(PathBuf::from(format!("/mods/_MANAGED_/g{i}/Mod")));
        }
        assert!(!ring_is_overflowed(&ring));
        // 超容量：新目录被丢弃，但溢出标志置位
        ring.push(PathBuf::from("/mods/_MANAGED_/extra/Mod"));
        let (dirs, overflow) = ring.drain();
        assert_eq!(dirs.len(), RING_CAPACITY, "入队数量不得超过容量");
        assert!(overflow, "超容量 push 应置溢出标志");
        // 复位验证
        let (_, overflow2) = {
            let mut r = ring;
            r.push(PathBuf::from("/mods/_MANAGED_/g0/Mod"));
            r.drain()
        };
        assert!(!overflow2, "drain 后溢出标志应复位");
    }

    /// 辅助：读取溢出标志（测试用）
    fn ring_is_overflowed(r: &EventRingBuffer) -> bool {
        r.overflow
    }

    /// 分层分类规则 1：脏目录全部位于同一 group → 仅该分组全量
    #[test]
    fn classify_single_group() {
        let managed = Path::new("/mods/_MANAGED_");
        let dirty = vec![
            managed.join("group_1/ModA"),
            managed.join("group_1/ModB/sub"),
        ];
        let roots = classify_overflow_roots(&dirty, managed);
        assert_eq!(roots, vec![managed.join("group_1")]);
    }

    /// 分层分类规则 2：跨多个 group、无 mutex → 各涉及分组全量
    #[test]
    fn classify_multiple_groups() {
        let managed = Path::new("/mods/_MANAGED_");
        let dirty = vec![
            managed.join("group_1/ModA"),
            managed.join("group_2/ModB"),
            managed.join("group_1/ModC"),
        ];
        let mut roots = classify_overflow_roots(&dirty, managed);
        roots.sort();
        assert_eq!(
            roots,
            vec![managed.join("group_1"), managed.join("group_2")]
        );
    }

    /// 分层分类规则 3：仅 mutex 目录 → 各 mutex 全量
    #[test]
    fn classify_mutex_only() {
        let managed = Path::new("/mods/_MANAGED_");
        let dirty = vec![managed.join("#Mutex1/ModA"), managed.join("#Mutex2/ModB")];
        let mut roots = classify_overflow_roots(&dirty, managed);
        roots.sort();
        assert_eq!(
            roots,
            vec![managed.join("#Mutex1"), managed.join("#Mutex2")]
        );
    }

    /// 分层分类规则 4：group/mutex 混合或 _MANAGED_ 之外 → 空（全量兜底）
    #[test]
    fn classify_mixed_or_external() {
        let managed = Path::new("/mods/_MANAGED_");
        // 混合
        let dirty = vec![managed.join("group_1/ModA"), managed.join("#M/ModB")];
        assert!(classify_overflow_roots(&dirty, managed).is_empty());
        // _MANAGED_ 之外
        let dirty2 = vec![PathBuf::from("/mods/loose/ModA")];
        assert!(classify_overflow_roots(&dirty2, managed).is_empty());
        // _MANAGED_ 根自身
        let dirty3 = vec![managed.to_path_buf()];
        assert!(classify_overflow_roots(&dirty3, managed).is_empty());
    }

    /// 溢出退避静默期计算：连续溢出 1/2/3 轮 → 1000/2000/3000ms，非溢出（0 轮）为 base；
    /// 连续溢出 3 轮起封顶 FILE_WATCHER_MAX_BACKOFF_MS（R2 规格锁定）
    #[test]
    fn backoff_quiet_period_levels() {
        let base = Duration::from_millis(constants::FILE_WATCHER_DEBOUNCE_MS);
        assert_eq!(
            base.as_millis(),
            1000,
            "静默期基线应为事件平息 1s（R2：300ms → 1000ms）"
        );
        // 无溢出 / 首轮溢出后 → base
        assert_eq!(backoff_quiet_period(base, 0), base);
        assert_eq!(backoff_quiet_period(base, 1), base);
        // 连续溢出 2 轮 → +1s；3 轮 → +2s；更多轮封顶
        assert_eq!(backoff_quiet_period(base, 2), Duration::from_millis(2000));
        assert_eq!(backoff_quiet_period(base, 3), Duration::from_millis(3000));
        assert_eq!(backoff_quiet_period(base, 9), Duration::from_millis(3000));
        // 封顶为绝对上限：base 过大时不超过 FILE_WATCHER_MAX_BACKOFF_MS
        assert_eq!(
            backoff_quiet_period(Duration::from_millis(2500), 5),
            Duration::from_millis(constants::FILE_WATCHER_MAX_BACKOFF_MS)
        );
    }

    /// EventRingBuffer 连续溢出计数：溢出批次 drain 递增、非溢出批次清零（退避依据）
    #[test]
    fn ring_buffer_consecutive_overflow_counter() {
        assert_eq!(RING_CAPACITY, 256, "R2 容量规格：120 → 256");
        let mut ring = EventRingBuffer::new();
        // 填满 + 1 → 溢出批次
        let fill = |ring: &mut EventRingBuffer| {
            for i in 0..RING_CAPACITY {
                ring.push(PathBuf::from(format!("/mods/_MANAGED_/g{i}/Mod")));
            }
            ring.push(PathBuf::from("/mods/_MANAGED_/extra/Mod"));
        };
        fill(&mut ring);
        let (_, o) = ring.drain();
        assert!(o);
        assert_eq!(ring.consecutive_overflows, 1, "首次溢出后计数为 1");
        // 连续再溢出两轮
        fill(&mut ring);
        ring.drain();
        fill(&mut ring);
        ring.drain();
        assert_eq!(ring.consecutive_overflows, 3, "连续溢出 3 轮计数为 3");
        // 非溢出批次 → 清零
        ring.push(PathBuf::from("/mods/_MANAGED_/g0/Mod"));
        let (_, o2) = ring.drain();
        assert!(!o2);
        assert_eq!(ring.consecutive_overflows, 0, "非溢出批次清零退避");
    }

    /// 增量分支 delta 组装：局部重扫产物 → 读锁收集旧状态 → 写锁合并 → GroupDelta 列表。
    ///
    /// 对齐 mod_commands R1 冒烟测试模式（临时目录 fixture + 全局 MOD_CACHE 唯一路径键，
    /// 故意不 invalidate_all 以免破坏并行测试前置数据）。
    #[test]
    fn merge_partials_and_build_deltas_assembly() {
        use crate::core::mod_cache;
        
        use tempfile::TempDir;

        // 写全局 MOD_CACHE 的测试串行（容量 3 + 4 个写者，防 LRU 跨测试淘汰）
        let _cache_guard = mod_cache::CACHE_WRITER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join(constants::MANAGED_FOLDER);
        std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let group1 = managed.join("group_1");
        let group2 = managed.join("group_2");
        for (m, g) in [("ModA", &group1), ("ModB", &group1), ("ModC", &group2)] {
            std::fs::create_dir_all(g.join(m)).unwrap();
            std::fs::write(g.join(m).join("mod.ini"), "[Section]\nx=1\n").unwrap();
        }

        // 预填充缓存（模拟 get_mods 已加载该游戏）
        let game = crate::models::enums::TargetGame::GenshinImpact;
        let full = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
        mod_cache::MOD_CACHE.write().set(game, &mods_path, full);

        // 模拟外部变更：group_1 新增 ModD、group_2 新增 ModE（各自局部重扫产物）
        std::fs::create_dir_all(group1.join("ModD")).unwrap();
        std::fs::write(group1.join("ModD").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        std::fs::create_dir_all(group2.join("ModE")).unwrap();
        std::fs::write(group2.join("ModE").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        let scan_results = vec![
            (
                group1.clone(),
                crate::core::mod_scan_partial::scan_partial_path(&mods_path, &group1).unwrap(),
            ),
            (
                group2.clone(),
                crate::core::mod_scan_partial::scan_partial_path(&mods_path, &group2).unwrap(),
            ),
        ];

        let deltas = merge_partials_and_build_deltas(game, &mods_path, &scan_results);

        // 多 root 批次 → 每个受影响分组各产出 1 个 delta
        assert_eq!(deltas.len(), 2, "两个受影响分组应各产出一个 delta");
        let d1 = deltas
            .iter()
            .find(|d| d.old_group_path.ends_with("group_1"))
            .expect("应含 group_1 delta");
        assert_eq!(
            d1.old_group_path,
            group1.to_string_lossy().to_string(),
            "old_group_path 应为缓存内该组的分组路径"
        );
        // old_mod_paths 为合并前旧状态（ModA/ModB，不含新增 ModD）
        assert_eq!(d1.old_mod_paths.len(), 2, "旧状态应为合并前的 2 个模组");
        assert!(
            d1.old_mod_paths.iter().all(|p| !p.ends_with("ModD")),
            "old_mod_paths 不应含新增 ModD"
        );
        // new_group 为重扫后完整组节点（含新增 ModD）
        let ng = d1.new_group.as_ref().expect("new_group 不应为 None");
        assert_eq!(
            mod_cache::collect_group_mod_paths(ng).len(),
            3,
            "新分组应含 3 个真实模组（ModA/ModB/ModD）"
        );

        // 缓存已合并（后端缓存与 delta 一致）
        let cached = mod_cache::MOD_CACHE
            .read()
            .get(game, &mods_path)
            .expect("缓存应命中");
        let g1 = mod_cache::find_group_node(&cached, &group1).expect("缓存应含 group_1");
        assert_eq!(
            mod_cache::collect_group_mod_paths(g1).len(),
            3,
            "缓存 group_1 应已含新增 ModD"
        );
        assert_eq!(cached.total_mods_count, 5, "合并后计数应重算：A+B+C+D+E");

        // 同组重复 partial（多 root 收敛到同一组的场景）：第二个 partial 与
        // 已合并缓存完全一致 → P1 差异过滤直接跳过 delta emit（前端零打扰；
        // 「连续 apply 产生扁平重复条目」的问题随过滤不复存在）。缓存侧
        // subtree_replace 照常幂等合并，终态不变。
        let repeat = vec![(
            group1.clone(),
            crate::core::mod_scan_partial::scan_partial_path(&mods_path, &group1).unwrap(),
        )];
        let deltas2 = merge_partials_and_build_deltas(game, &mods_path, &repeat);
        assert!(
            deltas2.is_empty(),
            "与缓存一致的重复批次应被差异过滤，不 emit delta: {:?}",
            deltas2
        );
    }

    /// 契约锁定：managed-partial-update payload 序列化为 { game, deltas }（camelCase，
    /// 组内复用 GroupDelta 契约）
    #[test]
    fn managed_partial_update_payload_serializes_camel_case() {
        let payload = ManagedPartialUpdatePayload {
            game: "GenshinImpact".to_string(),
            deltas: vec![GroupDelta {
                old_group_path: "D:/Mods/_MANAGED_/group_1".to_string(),
                new_group: None,
                old_mod_paths: vec!["D:/Mods/_MANAGED_/group_1/ModA".to_string()],
            }],
        };
        let v = serde_json::to_value(&payload).expect("payload 序列化失败");
        assert!(v.get("game").is_some(), "应含 game 字段");
        let deltas = v
            .get("deltas")
            .and_then(serde_json::Value::as_array)
            .expect("deltas 应为数组");
        assert_eq!(deltas.len(), 1, "应携带 1 个 delta");
        assert!(
            deltas[0].get("oldGroupPath").is_some(),
            "组内 delta 字段应为 camelCase（复用 GroupDelta 契约）"
        );
    }

    /// C4 世代校验：批次世代与当前世代一致则非过期，否则过期静默丢弃
    ///
    /// 同一监听内（未再调用 `start_watching`）世代保持一致 → 不丢弃；
    /// 切换游戏触发 `start_watching`（世代递增）后，旧监听批次捕获的快照过时 → 丢弃。
    #[test]
    fn generation_stale_check() {
        // 同一监听内：世代一致 → 不过期
        assert!(!is_generation_stale(1, 1));
        assert!(!is_generation_stale(5, 5));
        // 旧监听批次：当前世代已推进（start_watching 再次递增）→ 过期
        assert!(is_generation_stale(1, 2));
        assert!(is_generation_stale(5, 6));
        // 世代回退不应发生，但判定同样视为不一致（防御）
        assert!(is_generation_stale(6, 5));
        // 初始世代越界场景（不可能出现，逻辑防御）
        assert!(is_generation_stale(0, 1));
    }

    // =====================================================================
    // C-3a：双粒度路由（is_mod_level_root / route_incremental_batch / 批次聚合）
    // =====================================================================

    /// C-3a 粒度路由判定：组级根（group_xx / _MANAGED_ 一级子目录）与模组级根分类
    #[test]
    fn is_mod_level_root_classification() {
        let managed = Path::new("/mods/_MANAGED_");
        // 组级：group_xx 目录
        assert!(!is_mod_level_root(&managed.join("group_1"), managed));
        // 组级：_MANAGED_ 一级子目录（mutex 顶层根，含根即模组的形态）
        assert!(!is_mod_level_root(&managed.join("#Mutex"), managed));
        assert!(!is_mod_level_root(&managed.join("StandaloneMod"), managed));
        // 组级：_MANAGED_ 根自身 / 之外
        assert!(!is_mod_level_root(managed, managed));
        assert!(!is_mod_level_root(
            &PathBuf::from("/mods/loose/ModA"),
            managed
        ));
        // 模组级：普通组下的模组目录
        assert!(is_mod_level_root(
            &managed.join("group_1").join("ModA"),
            managed
        ));
        // 模组级：mutex 根下模组 / 深层嵌套模组
        assert!(is_mod_level_root(
            &managed.join("#Mutex").join("ModX"),
            managed
        ));
        assert!(is_mod_level_root(
            &managed.join("#Mutex").join("Sub").join("ModX"),
            managed
        ));
        // 组级：mutex 树内 group_xx 命名的子分组（snap 会停在 group_xx 边界）
        assert!(!is_mod_level_root(
            &managed.join("#Mutex").join("group_5"),
            managed
        ));
        // 形态容忍：Windows verbatim（consolidate 的 canonicalize 产物形态）
        let verbatim = PathBuf::from(format!(
            "\\\\?\\{}",
            managed.join("group_1").join("ModA").to_string_lossy()
        ));
        assert!(
            is_mod_level_root(&verbatim, managed),
            "verbatim 形态的模组级根应被识别"
        );
    }

    /// C-3a 顶层组目录推导：模组级根 → `_MANAGED_` 一级子目录（含 verbatim/大小写容忍）
    #[test]
    fn top_level_group_dir_derivation() {
        let managed = Path::new("/mods/_MANAGED_");
        assert_eq!(
            top_level_group_dir(&managed.join("group_1").join("ModA"), managed),
            Some(managed.join("group_1"))
        );
        assert_eq!(
            top_level_group_dir(&managed.join("#M").join("Sub").join("ModX"), managed),
            Some(managed.join("#M"))
        );
        // 组外路径 → None
        assert_eq!(
            top_level_group_dir(&PathBuf::from("/mods/loose/ModA"), managed),
            None
        );
    }

    /// C-3a 路由行为证明（一）：模组级根走单模组 patch 而非整组重扫——
    /// 组内**未被指向的兄弟**的陈旧状态保留（整组重扫会以磁盘为准重扫并传播删除）。
    #[test]
    fn route_incremental_batch_mod_level_not_group_rescan() {
        use crate::core::mod_cache;
        
        use tempfile::TempDir;

        let _cache_guard = mod_cache::CACHE_WRITER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join(constants::MANAGED_FOLDER);
        std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let group1 = managed.join("group_1");
        for m in ["ModA", "ModB"] {
            std::fs::create_dir_all(group1.join(m)).unwrap();
            std::fs::write(group1.join(m).join("mod.ini"), "[Section]\nx=1\n").unwrap();
        }

        let game = crate::models::enums::TargetGame::GenshinImpact;
        mod_cache::MOD_CACHE.write().set(
            game,
            &mods_path,
            crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
        );

        // 磁盘侧「删除 ModB + 深部变更 ModA」，但收敛根仅指向 ModA：
        // 真实事件流中删除自身会收敛出组级根（整组重扫），此处模拟纯模组级批次
        std::fs::remove_dir_all(group1.join("ModB")).unwrap();
        std::fs::write(group1.join("ModA").join("fav"), "").unwrap();

        let deltas = route_incremental_batch(game, &mods_path, &managed, &[group1.join("ModA")]);

        // 产出 group_1 的单个 delta（模组级 patch 形状：整组节点）
        assert_eq!(deltas.len(), 1, "单模组级根应产出一个组 delta");
        let d = &deltas[0];
        assert!(d.old_group_path.ends_with("group_1"));
        let ng = d.new_group.as_ref().expect("new_group 不应为 None");
        assert!(
            ng.mods.iter().any(|m| m.name == "ModA" && m.is_favorite),
            "深部 fav 变更生效"
        );
        // 行为证明：ModB 未被该批次触及（陈旧保留）——若走整组重扫，
        // scan_partial_path 会以磁盘为准重扫且删除传播会剔除 ModB
        assert!(
            ng.mods.iter().any(|m| m.name == "ModB"),
            "模组级 patch 不应触碰兄弟条目（证明未走整组重扫路径）"
        );
        // 缓存与 delta 一致
        let cached = mod_cache::MOD_CACHE.read().get(game, &mods_path).unwrap();
        let g1 = mod_cache::find_group_node(&cached, &group1).unwrap();
        assert!(
            g1.mods.iter().any(|m| m.name == "ModB"),
            "缓存同样保留兄弟（等其自身批次收敛）"
        );
    }

    /// C-3a 路由行为证明（二）：新增模组 patch 置组内**末位**（整组重扫会按排序规则
    /// 插入自然位），以此证明模组级路径被命中。
    #[test]
    fn route_incremental_batch_inserts_at_end_not_sorted_position() {
        use crate::core::mod_cache;
        
        use tempfile::TempDir;

        let _cache_guard = mod_cache::CACHE_WRITER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join(constants::MANAGED_FOLDER);
        std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let group1 = managed.join("group_1");
        std::fs::create_dir_all(group1.join("ZZZMod")).unwrap();
        std::fs::write(group1.join("ZZZMod").join("mod.ini"), "[Section]\nx=1\n").unwrap();

        let game = crate::models::enums::TargetGame::GenshinImpact;
        mod_cache::MOD_CACHE.write().set(
            game,
            &mods_path,
            crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
        );

        // 新增 AAAMod（自然序在 ZZZMod 之前；整组重扫会将其排到组首真实模组位）
        std::fs::create_dir_all(group1.join("AAAMod")).unwrap();
        std::fs::write(group1.join("AAAMod").join("mod.ini"), "[Section]\nx=1\n").unwrap();

        route_incremental_batch(game, &mods_path, &managed, &[group1.join("AAAMod")]);

        let cached = mod_cache::MOD_CACHE.read().get(game, &mods_path).unwrap();
        let g1 = mod_cache::find_group_node(&cached, &group1).unwrap();
        let names: Vec<&str> = g1.mods.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["None", "ZZZMod", "AAAMod"],
            "新增模组置于末位（模组级 patch 语义；排序位由下次组级扫描校正）"
        );
    }

    /// C-3a 混合批次聚合：组级根 + 模组级根各自产出 delta；
    /// 同组多个模组级根合并为**一次**组 delta。
    #[test]
    fn route_incremental_batch_mixed_and_aggregation() {
        use crate::core::mod_cache;
        
        use tempfile::TempDir;

        let _cache_guard = mod_cache::CACHE_WRITER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join(constants::MANAGED_FOLDER);
        std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        // group_1：ModA + ModB；group_2：ModC（组级根用）
        for (g, m) in [
            ("group_1", "ModA"),
            ("group_1", "ModB"),
            ("group_2", "ModC"),
        ] {
            std::fs::create_dir_all(managed.join(g).join(m)).unwrap();
            std::fs::write(managed.join(g).join(m).join("mod.ini"), "[Section]\nx=1\n").unwrap();
        }

        let game = crate::models::enums::TargetGame::GenshinImpact;
        mod_cache::MOD_CACHE.write().set(
            game,
            &mods_path,
            crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
        );

        // 混合批次：group_2（组级）+ group_1/ModA + group_1/ModB（同组两个模组级）
        std::fs::write(managed.join("group_1").join("ModA").join("fav"), "").unwrap();
        std::fs::write(managed.join("group_1").join("ModB").join("fav"), "").unwrap();
        std::fs::write(managed.join("group_2").join("ModC").join("fav"), "").unwrap();

        let deltas = route_incremental_batch(
            game,
            &mods_path,
            &managed,
            &[
                managed.join("group_2"),
                managed.join("group_1").join("ModA"),
                managed.join("group_1").join("ModB"),
            ],
        );

        // group_1 两个模组级根 → 合并为 1 个 delta；group_2 组级根 → 1 个 delta
        assert_eq!(deltas.len(), 2, "混合批次应聚合为两个组 delta");
        let g1_deltas = deltas
            .iter()
            .filter(|d| d.old_group_path.ends_with("group_1"))
            .count();
        assert_eq!(g1_deltas, 1, "同组两个模组级根应合并为一次组 delta");
        let d1 = deltas
            .iter()
            .find(|d| d.old_group_path.ends_with("group_1"))
            .unwrap();
        let ng = d1.new_group.as_ref().unwrap();
        assert!(
            ng.mods.iter().any(|m| m.name == "ModA" && m.is_favorite)
                && ng.mods.iter().any(|m| m.name == "ModB" && m.is_favorite),
            "同组批次的两个模组变更均生效于合并后的单个 delta"
        );
        assert_eq!(
            d1.old_mod_paths.len(),
            2,
            "旧状态为合并前（两模组均未 fav）"
        );
        // 缓存终态一致
        let cached = mod_cache::MOD_CACHE.read().get(game, &mods_path).unwrap();
        assert_eq!(cached.total_mods_count, 3);
        assert!(
            cached.mods.iter().filter(|m| m.is_favorite).count() == 3,
            "三个模组的 fav 状态均合并入缓存"
        );
    }

    /// C-3a 降级兜底：普通组内嵌套目录（轻扫不递归 NormalGroup——`Sub` 在缓存中是
    /// group_1 的模组条目而非分组，深层 `Sub/ModX` 无模组定位）的模组级根 →
    /// patch 未命中 → 降级组级重扫，与变更前行为一致（缓存不漏更新、不损坏）。
    #[test]
    fn route_incremental_batch_nested_normal_group_falls_back_to_group_rescan() {
        use crate::core::mod_cache;
        
        use tempfile::TempDir;

        let _cache_guard = mod_cache::CACHE_WRITER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join(constants::MANAGED_FOLDER);
        std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        // group_1/Sub/ModX：Sub 无 ini（轻扫中是 group_1 的模组条目），ModX 含 ini
        let group1 = managed.join("group_1");
        let mod_x = group1.join("Sub").join("ModX");
        std::fs::create_dir_all(&mod_x).unwrap();
        std::fs::write(mod_x.join("mod.ini"), "[Section]\nx=1\n").unwrap();

        let game = crate::models::enums::TargetGame::GenshinImpact;
        mod_cache::MOD_CACHE.write().set(
            game,
            &mods_path,
            crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
        );
        let before = mod_cache::MOD_CACHE.read().get(game, &mods_path).unwrap();
        assert!(
            before.groups.iter().any(|g| g.group_name == "group_1"),
            "前置：group_1 已入缓存"
        );

        // 深部变更（ModX 加 fav；模组级根指向 ModX）
        std::fs::write(mod_x.join("fav"), "").unwrap();
        let deltas = route_incremental_batch(game, &mods_path, &managed, &[mod_x]);

        // P1 差异过滤：嵌套深层变更（ModX/fav）在轻扫粒度下对缓存与 UI 不可见
        //（group_1/Sub 条目不受影响）→ 降级组级重扫产物与缓存一致 → 差异过滤
        // 拦截，不 emit delta（watcher 对嵌套路径「空转不惊动前端」的实证）。
        // 缓存侧照常合并（幂等），组节点健壮性断言保留。
        assert!(
            deltas.is_empty(),
            "嵌套深层变更的降级组级重扫与缓存一致，应被差异过滤: {:?}",
            deltas
        );
        let cached = mod_cache::MOD_CACHE.read().get(game, &mods_path).unwrap();
        let g1 = mod_cache::find_group_node(&cached, &group1).expect("缓存组节点应健在");
        assert!(
            g1.mods.iter().any(|m| m.name == "Sub"),
            "Sub 条目保留（轻扫粒度：嵌套内容对其不可见，与变更前行为一致）"
        );
        assert_eq!(cached.total_mods_count, 1, "计数不受影响");
    }

    /// C-3a 集成对齐（正确性金标准）：模组级 patch 后的缓存数据与整组重扫结果一致。
    ///
    /// fixture 覆盖普通组（一启用一禁用 + 唯一启用者 active）与互斥树（根模组 +
    /// 嵌套子分组模组）；深部变更（fav）走模组级路由后，缓存序列化产物应与
    /// 对同一磁盘状态的全量轻量扫描完全一致。
    #[test]
    fn route_incremental_batch_mod_level_parity_with_full_scan() {
        use crate::core::mod_cache;
        
        use tempfile::TempDir;

        let _cache_guard = mod_cache::CACHE_WRITER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join(constants::MANAGED_FOLDER);
        std::fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();

        // 普通组：ModA（唯一启用 → active）、DISABLED_ModB
        let group1 = managed.join("group_1");
        for m in ["ModA", "DISABLED_ModB"] {
            std::fs::create_dir_all(group1.join(m)).unwrap();
            std::fs::write(group1.join(m).join("mod.ini"), "[Section]\nx=1\n").unwrap();
        }
        // 互斥树：#M/{ModW, Sub/{ModX, ModY}}
        let mutex_root = managed.join("#M");
        std::fs::create_dir_all(mutex_root.join("ModW")).unwrap();
        std::fs::write(mutex_root.join("ModW").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        let sub = mutex_root.join("Sub");
        for m in ["ModX", "ModY"] {
            std::fs::create_dir_all(sub.join(m)).unwrap();
            std::fs::write(sub.join(m).join("mod.ini"), "[Section]\nx=1\n").unwrap();
        }

        let game = crate::models::enums::TargetGame::GenshinImpact;
        mod_cache::MOD_CACHE.write().set(
            game,
            &mods_path,
            crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap(),
        );

        // 深部变更（外部工具写入场景）：普通组 ModA 与互斥树深层 ModX 各加 fav
        std::fs::write(group1.join("ModA").join("fav"), "").unwrap();
        std::fs::write(sub.join("ModX").join("fav"), "").unwrap();

        let deltas = route_incremental_batch(
            game,
            &mods_path,
            &managed,
            &[group1.join("ModA"), sub.join("ModX")],
        );
        assert_eq!(deltas.len(), 2, "两个顶层组各一个 delta");

        // 金标准：缓存序列化产物与同一磁盘状态的全量轻量扫描完全一致
        let cached = mod_cache::MOD_CACHE.read().get(game, &mods_path).unwrap();
        let fresh = crate::core::mod_scan_light::scan_mods_light(&mods_path).unwrap();
        let cached_json = serde_json::to_value(&*cached).expect("缓存序列化失败");
        let fresh_json = serde_json::to_value(&fresh).expect("全量扫描序列化失败");
        assert_eq!(
            cached_json, fresh_json,
            "mod 级 patch 后缓存数据应与整组重扫结果一致（get_mods 口径）"
        );
    }
}
