//! 文件系统监控模块
//!
//! 使用 notify 库监控 _MANAGED_ 目录的文件变化，实现：
//! - 递归监控整个模组目录
//! - 防抖机制：300ms 内的多次变化合并为一次事件（通过独立线程 + mpsc channel 实现）
//! - 增量更新：通过 `IncrementalUpdater` 收集变更路径 → consolidate → 局部重扫 → subtree_replace 写入缓存，避免全量刷新
//! - 暂停/恢复功能：程序自身写文件时暂停监控避免循环触发
//! - 过滤无关事件：临时文件、备份文件、隐藏文件不触发刷新
//! - 缓存失效：文件变化后自动失效模组缓存并通知前端刷新

use crate::core::constants;
use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use once_cell::sync::Lazy;
use rayon::prelude::*;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// 事件环形队列容量（2026-08-19 用户指定）。
///
/// 设计意图：正常人工操作（增删改少量模组）产生的脏目录数远小于 120，走精确增量路径；
/// 风暴场景（INI 备份/更新模组数据写入数万文件，目录数远超容量）必然溢出，
/// 快速转入分层全量降级路径（按顶层 group/mutex 分类全量），避免逐目录精确处理
/// 数千至上万个根路径的串行开销。
const RING_CAPACITY: usize = 120;

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
}

impl EventRingBuffer {
    /// 创建空缓冲
    fn new() -> Self {
        EventRingBuffer {
            queue: VecDeque::with_capacity(RING_CAPACITY),
            dedup: HashSet::with_capacity(RING_CAPACITY),
            overflow: false,
            last_push_at: None,
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

    /// 防抖是否到期：队列非空且距最近一次 push 已超过防抖时长
    ///
    /// # 参数
    /// - `debounce`: 防抖时长（300ms）
    fn is_ready(&self, debounce: Duration) -> bool {
        match self.last_push_at {
            None => false,
            Some(t) => !self.queue.is_empty() && t.elapsed() >= debounce,
        }
    }

    /// 取走全部数据并复位（返回脏目录列表与是否曾溢出）
    ///
    /// # 返回
    /// `(脏目录列表, 溢出标志)`；调用后缓冲回到初始空状态
    fn drain(&mut self) -> (Vec<PathBuf>, bool) {
        let dirs: Vec<PathBuf> = self.queue.drain(..).collect();
        self.dedup.clear();
        let overflow = self.overflow;
        self.overflow = false;
        self.last_push_at = None;
        (dirs, overflow)
    }
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
                    if crate::core::mod_scanner::is_normal_group_dir(&name).is_some() {
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

/// 处理完成后通知前端：窗口可见时 emit `managed-folder-changed` +
/// `managed-partial-update`（携带扫描根列表）；窗口隐藏时静默（缓存已更新）。
///
/// # 参数
/// - `app_handle`: Tauri 应用句柄
/// - `managed_path`: `_MANAGED_` 目录路径
/// - `game`: 当前游戏（payload 字段）
/// - `roots`: 本轮处理的扫描根路径列表
fn emit_folder_changed(
    app_handle: &AppHandle,
    managed_path: &Path,
    game: crate::models::enums::TargetGame,
    roots: &[PathBuf],
) {
    let window_visible = app_handle
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if window_visible {
        // emit managed-folder-changed：通知前端扫描目录变化
        let _ = app_handle.emit("managed-folder-changed", managed_path.as_os_str());
        // emit managed-partial-update：携带增量更新详情（consolidatedRoots + game）
        let payload = serde_json::json!({
            "consolidatedRoots": roots,
            "game": format!("{:?}", game),
        });
        let _ = app_handle.emit("managed-partial-update", payload);
    } else {
        log::info!("[Incremental] window hidden -> merged cache silently");
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
    /// 防抖处理线程的句柄，停止监控时需 `join` 等待线程正常退出，避免资源泄漏。
    _debounce_thread: Option<std::thread::JoinHandle<()>>,
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
    ///    300ms 防抖到期 → drain 环形队列 → 增量/分层处理 → emit 前端事件
    ///
    /// # 防抖 & 增量/分层更新管线（防抖线程内部）
    /// - 未溢出（≤120 脏目录）：`consolidate()` 合并根路径 → rayon 并行
    ///   `scan_partial_path()` 局部重扫 → 短锁 `subtree_replace()` 替换缓存子树
    /// - 溢出（队列曾满）：按顶层 group/mutex 分层全量降级（见 [`classify_overflow_roots`]），
    ///   混合场景走 `invalidate_game` + 前端全量 loadMods 兜底
    /// - emit `managed-folder-changed` 和 `managed-partial-update`；窗口隐藏时仅更新缓存
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

                    // 队列非空：50ms 超时轮询（继续接收新事件 + 检测 300ms 防抖到期）
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(()) => {}
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let ready = ring_debounce
                                .lock()
                                .map(|r| r.is_ready(debounce_duration))
                                .unwrap_or(false);
                            if !ready {
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

                            // 检查暂停标志（全局引用计数 + 实例级），任一为 true 则丢弃本轮
                            let paused_global = WATCHER_PAUSED.load(Ordering::Acquire) > 0;
                            let paused_local = paused_clone.load(Ordering::SeqCst);
                            if paused_global || paused_local {
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
                                emit_folder_changed(
                                    &app_handle,
                                    &managed_path_clone,
                                    current_game,
                                    &[],
                                );
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

                            // rayon 并行局部重扫（纯读磁盘+解析，锁外执行）；
                            // 单个根失败仅跳过该根（记日志），不中断其余
                            let scan_results: Vec<(PathBuf, crate::core::mod_scanner::ScanResult)> =
                                scan_roots
                                    .par_iter()
                                    .filter_map(|sub| {
                                        // 收敛根已不存在（整组目录被删除）：scan_partial_path 会退化为
                                        // 全量扫描，且缓存会残留已删分组——使该游戏缓存失效兜底
                                        if !sub.exists() {
                                            log::info!(
                                                "[Incremental] scan root no longer exists, invalidating game cache: {:?}",
                                                sub
                                            );
                                            let mut cache_guard =
                                                crate::core::mod_cache::MOD_CACHE.write();
                                            cache_guard.invalidate_game(current_game);
                                            return None;
                                        }
                                        match crate::core::mod_scanner::scan_partial_path(
                                            &mods_path_buf,
                                            sub,
                                        ) {
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
                                    .collect();

                            // 短锁替换：并行扫描完成后逐个短时持写锁替换缓存子树
                            // （写锁持有从原"全程数十秒"降为"每次替换毫秒级"，get_mods 读阻塞大幅减少）
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

                            emit_folder_changed(
                                &app_handle,
                                &managed_path_clone,
                                current_game,
                                &scan_roots,
                            );
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
        self.paused.store(false, Ordering::SeqCst);

        Ok(())
    }

    /// 停止监控并清理所有资源
    ///
    /// 1. drop `watcher`（`Option::take` → 释放 notify 资源，自动调用 `unwatch` 停止 OS 级别监控）
    /// 2. drop `_tx`（关闭 channel 发送端，防抖线程 `recv_timeout` 收到 `Disconnected` 退出循环）
    /// 3. `join` 防抖线程（`JoinHandle::join` 等待线程正常退出，避免资源泄漏和僵尸线程）
    /// 4. 清空 `watched_path`、`updater`，重置实例级暂停标志
    pub fn stop_watching(&mut self) {
        // drop watcher 会自动调用 unwatch
        self.watcher = None;
        // drop tx 会导致 rx 端收到 Disconnected，防抖线程退出循环
        self._tx = None;
        // 等待防抖线程结束，避免资源泄漏
        if let Some(handle) = self._debounce_thread.take() {
            let _ = handle.join();
        }
        self.watched_path = None;
        self.paused.store(false, Ordering::SeqCst);
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

/// 开始文件监控 Tauri 命令
///
/// 前端调用 `startFileWatcher` 触发。
/// 涉及 IO 操作（创建目录）和线程 join（`stop_watching`），使用 `spawn_blocking` 避免阻塞 async 运行时。
///
/// # Errors
/// - 传入路径无效或目录创建失败
/// - notify watcher 初始化失败
#[tauri::command]
pub async fn start_file_watcher(
    app_handle: AppHandle,
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
    mods_path: String,
) -> Result<(), String> {
    let watcher_arc = watcher.inner().clone();
    let mods_path_buf = PathBuf::from(mods_path);
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let mut w = watcher_arc.lock().map_err(|e| e.to_string())?;
        w.stop_watching();
        w.start_watching(app_handle, &mods_path_buf)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 停止文件监控 Tauri 命令
///
/// 前端调用 `stopFileWatcher` 触发。
/// `thread::join` 可能阻塞，使用 `spawn_blocking` 避免阻塞 async 运行时。
///
/// # Errors
/// - 锁获取失败（Mutex  poisoned）
/// - 线程 join 失败
#[tauri::command]
pub async fn stop_file_watcher(
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
) -> Result<(), String> {
    let watcher_arc = watcher.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let mut w = watcher_arc.lock().map_err(|e| e.to_string())?;
        w.stop_watching();
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 切换监控路径 Tauri 命令
///
/// 前端调用 `switchFileWatcher` 触发。
/// 涉及 IO 和线程操作，使用 `spawn_blocking`。
///
/// # Errors
/// - 同 `start_watching`：目录创建失败或 watcher 初始化失败
#[tauri::command]
pub async fn switch_file_watcher(
    app_handle: AppHandle,
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
    mods_path: String,
) -> Result<(), String> {
    let watcher_arc = watcher.inner().clone();
    let mods_path_buf = PathBuf::from(mods_path);
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let mut w = watcher_arc.lock().map_err(|e| e.to_string())?;
        w.switch_watched_path(app_handle, &mods_path_buf)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 暂停文件监控 Tauri 命令
///
/// 前端调用 `pauseFileWatcher` 触发。
/// 仅设置原子 bool，轻量操作保持同步，无需 `spawn_blocking`。
#[tauri::command]
pub fn pause_file_watcher(
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
) -> Result<(), String> {
    let w = watcher.lock().map_err(|e| e.to_string())?;
    w.pause();
    Ok(())
}

/// 恢复文件监控 Tauri 命令
///
/// 前端调用 `resumeFileWatcher` 触发。
/// 仅设置原子 bool，轻量操作保持同步，无需 `spawn_blocking`。
#[tauri::command]
pub fn resume_file_watcher(
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
) -> Result<(), String> {
    let w = watcher.lock().map_err(|e| e.to_string())?;
    w.resume();
    Ok(())
}

/// 检查文件监控是否正在运行（Tauri 命令）
///
/// 前端调用 `isFileWatcherRunning` 触发，用于 `window-shown` 时判断监控是否需要重启。
/// 例如窗口从隐藏到显示时，如果监控已停止则需要重新启动。
#[tauri::command]
pub fn is_file_watcher_running(watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>) -> bool {
    watcher.lock().map(|w| w.is_running()).unwrap_or(false)
}

/// 获取当前监控的 `_MANAGED_` 路径（Tauri 命令）
///
/// 前端调用 `currentWatchedPath` 触发，供前端对比是否需要切换监控路径。
/// 例如用户切换游戏时，前端调用此命令获取当前路径并与新路径比较。
#[tauri::command]
pub fn current_watched_path(watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>) -> Option<String> {
    watcher
        .lock()
        .ok()
        .and_then(|w| w.watched_path().map(|p| p.to_string_lossy().into_owned()))
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
}
