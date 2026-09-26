//! 文件监控器 —— FileWatcher 主体域
//!
//! 从 file_watcher.rs 拆出（2026-09-19 剩余巨石拆分 PA-2）。
//! 承载 `FileWatcher` 结构体及其驻留/暂停/启停/切换方法，含 notify 回调、
//! 独立防抖线程与增量/全量更新管线编排。
//! 事件环形缓冲/暂停守卫见 [`crate::core::watcher_ring`]，增量根分类/批次路由见
//! [`crate::core::watcher_incremental`]，事件发射见 [`crate::core::watcher_emit`]。

use crate::core::constants;
use crate::core::watcher_emit::{emit_folder_changed, emit_managed_partial_update};
use crate::core::watcher_incremental::{
    classify_overflow_roots, consolidate_incremental_roots, is_generation_stale,
    route_incremental_batch, scan_group_roots,
};
use crate::core::watcher_ring::{backoff_quiet_period, EventRingBuffer, WATCHER_PAUSED};
use crate::models::mod_data::GroupDelta;
use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::AppHandle;

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
    ///    事件平息静默期到期（FILE_WATCHER_DEBOUNCE_MS，溢出退避时延长）→ drain 环形队列 → 增量/分层处理
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
                        // 临时/备份/隐藏文件 + selectedindex（命令内联维护）不触发缓存对账
                        if is_ignored_watcher_path(&p, &backup_suffix, &manager_backup_suffix) {
                            continue;
                        }
                        if let Some(parent) = p.parent() {
                            if let Ok(mut ring) = ring_cb.lock() {
                                ring.push(parent.to_path_buf());
                            }
                            pushed = true;
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
                            // 事件平息静默期：距最后事件 FILE_WATCHER_DEBOUNCE_MS 才处理批次；
                            // 溢出退避：连续溢出轮数逐轮 +1s（独立阶梯，封顶 FILE_WATCHER_MAX_BACKOFF_MS），
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
                                    let mut cache_guard =
                                        crate::core::mod_cache_core::MOD_CACHE.write();
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
                                    let mut cache_guard =
                                        crate::core::mod_cache_core::MOD_CACHE.write();
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

/// 事件路径是否应被监控管线忽略（不触发增量/全量缓存对账）。
///
/// 除临时（.tmp）/ 备份（NRMM 注入备份 + INI 启动备份）/ 隐藏文件外，
/// `selectedindex` 亦被忽略：该文件由 select/deselect 命令原子写 + 缓存定向
/// patch（`ModCache::patch_selected_index`）内联维护，watcher 对它的共鸣
/// （整组轻扫 + `subtree_replace` 深拷贝合并）属冗余工作——即选择交互高延迟
/// 的来源（单次选择 ≈1s 轻扫 + ~7s 合并）。外部对 `selectedindex` 的修改由
/// 下次自然全量扫描时按磁盘重读对账，不依赖事件共鸣。
fn is_ignored_watcher_path(p: &Path, backup_suffix: &str, manager_backup_suffix: &str) -> bool {
    let path_str = p.to_string_lossy();
    let file_name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    path_str.contains(".tmp")
        || path_str.ends_with(backup_suffix)
        || path_str.ends_with(manager_backup_suffix)
        || file_name.starts_with('.')
        || file_name == constants::SELECTED_INDEX_FILE
}

#[cfg(test)]
mod tests {
    use super::is_ignored_watcher_path;
    use std::path::Path;

    const BACKUP: &str = ".ini_managed_backup";
    const MANAGER_BACKUP: &str = ".ini_manager_backup";

    #[test]
    fn selectedindex_file_is_ignored() {
        // select/deselect 命令内联维护该文件，watcher 不得因它触发扫描/合并（选择高延迟根因）
        let selected = Path::new(r"D:\Games\Mods\_MANAGED_\group_1\selectedindex");
        assert!(is_ignored_watcher_path(selected, BACKUP, MANAGER_BACKUP));
    }

    #[test]
    fn normal_ini_and_dirs_still_trigger() {
        let ini = Path::new(r"D:\Games\Mods\_MANAGED_\group_1\some.ini");
        assert!(!is_ignored_watcher_path(ini, BACKUP, MANAGER_BACKUP));
        let dir = Path::new(r"D:\Games\Mods\_MANAGED_\group_2");
        assert!(!is_ignored_watcher_path(dir, BACKUP, MANAGER_BACKUP));
    }

    #[test]
    fn tmp_backup_hidden_still_ignored() {
        let tmp = Path::new(r"D:\Games\Mods\_MANAGED_\group_1\x.tmp");
        assert!(is_ignored_watcher_path(tmp, BACKUP, MANAGER_BACKUP));
        let backup = Path::new(r"D:\Games\Mods\_MANAGED_\group_1\a.mod.ini.ini_managed_backup");
        assert!(is_ignored_watcher_path(backup, BACKUP, MANAGER_BACKUP));
        let mbackup = Path::new(r"D:\Games\Mods\_MANAGED_\group_1\a.ini.ini_manager_backup");
        assert!(is_ignored_watcher_path(mbackup, BACKUP, MANAGER_BACKUP));
        let hidden = Path::new(r"D:\Games\Mods\_MANAGED_\group_1\.hidden");
        assert!(is_ignored_watcher_path(hidden, BACKUP, MANAGER_BACKUP));
    }
}
