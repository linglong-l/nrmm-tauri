//! 文件系统监控模块
//!
//! 使用 notify 库监控 _MANAGED_ 目录的文件变化，实现：
//! - 递归监控整个模组目录
//! - 防抖机制：300ms 内的多次变化合并为一次事件（通过独立线程 + mpsc channel 实现）
//! - 增量更新：通过 `IncrementalUpdater` 收集变更路径 → consolidate → 局部重扫 → subtree_replace 写入缓存，避免全量刷新
//! - 暂停/恢复功能：程序自身写文件时暂停监控避免循环触发
//! - 过滤无关事件：临时文件、备份文件、隐藏文件不触发刷新
//! - 缓存失效：文件变化后自动失效模组缓存并通知前端刷新

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use once_cell::sync::Lazy;
use crate::core::constants;

/// 全局文件监控暂停标志
///
/// 当 NRMM 自身需要写标记文件（如 groupname、selectedindex、fav 等）时，
/// 设置为 `true` 避免触发文件变化事件导致循环刷新，写完后恢复为 `false`。
/// 跨线程可见，使用 `Ordering::SeqCst` 保证顺序一致性 —— 所有线程看到的内存操作顺序完全相同。
/// 使用 `AtomicBool` 保证线程安全和无锁访问。
pub static WATCHER_PAUSED: Lazy<AtomicBool> = Lazy::new(|| AtomicBool::new(false));

/// 文件监控器结构体
///
/// 封装 notify 的 `RecommendedWatcher`，配合独立的防抖线程处理事件。
/// 使用 mpsc channel 在 watcher 回调和防抖线程间传递事件。
/// 通过 `IncrementalUpdater` 实现增量更新，避免全量重扫。
pub struct FileWatcher {
    /// notify 库的文件监控器实例。
    /// `Option` 允许 `stop_watching` 时通过 `take` 释放底层 OS 资源。
    watcher: Option<RecommendedWatcher>,
    /// 当前被监控的 `_MANAGED_` 目录路径（完整路径，如 `.../Mods/_MANAGED_`）。
    watched_path: Option<PathBuf>,
    /// 实例级别的暂停标志，独立于全局 `WATCHER_PAUSED`。
    /// 防抖线程同时检查两者，任一为 `true` 则跳过事件触发。
    paused: Arc<AtomicBool>,
    /// mpsc channel 的发送端，watcher 回调线程通过此端发送事件。
    /// 保持所有权防止 channel 关闭（drop 后接收端收到 `Disconnected`）。
    _tx: Option<mpsc::Sender<notify::Event>>,
    /// 防抖处理线程的句柄，停止监控时需 `join` 等待线程正常退出，避免资源泄漏。
    _debounce_thread: Option<std::thread::JoinHandle<()>>,
    /// 增量更新器，使用 `Arc<Mutex<IncrementalUpdater>>` 供防抖线程共享访问。
    /// 收集路径变化 → consolidate → 局部重扫 → subtree_replace 写入缓存。
    updater: Option<Arc<Mutex<crate::core::incremental_updater::IncrementalUpdater>>>,
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
            updater: None,
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
    /// 3. **创建 mpsc channel**：watcher 回调线程发送事件，防抖线程接收
    /// 4. **初始化 notify watcher**：使用 `recommended_watcher` 递归监控整个目录
    /// 5. **启动防抖线程**：独立线程通过 `recv_timeout(50ms)` 轮询接收事件，
    ///    收集路径 → consolidate → 局部重扫 → subtree_replace 更新缓存 → emit 前端事件
    /// 6. **创建增量更新器**：`IncrementalUpdater` 实例由 `Arc<Mutex<>>` 包裹，供防抖线程共享访问
    ///
    /// # 防抖 & 增量更新管线（防抖线程内部）
    /// - `recv_timeout(50ms)` 轮询：每次收到相关事件调用 `collect()` 记录变更路径
    /// - 超时后检查 `is_ready()`（自上次 collect 后已过 300ms 防抖期）
    /// - 就绪后：检查暂停标志 → `consolidate()` 合并根路径 → `scan_partial_path()` 局部重扫
    ///   → `subtree_replace()` 替换缓存子树 → emit `managed-folder-changed` 和 `managed-partial-update`
    /// - 窗口隐藏时仅合并缓存不发送事件，避免不必要的前端渲染
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

        // 创建消息通道：watcher 回调线程发送事件，防抖线程接收事件
        let (tx, rx) = mpsc::channel();

        let tx_clone = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res| {
            // watcher 回调在 notify 内部线程，只负责发送事件到 channel
            if let Ok(event) = res {
                let _ = tx_clone.send(event);
            }
        })?;

        // 递归监控 _MANAGED_ 目录下所有文件和子目录
        watcher.watch(&managed_path, RecursiveMode::Recursive)?;

        let managed_path_clone = managed_path.clone();
        let debounce_duration = Duration::from_millis(constants::FILE_WATCHER_DEBOUNCE_MS);
        let paused_clone = self.paused.clone();

        let updater = Arc::new(Mutex::new(
            crate::core::incremental_updater::IncrementalUpdater::new(debounce_duration.as_millis() as u64),
        ));
        let updater_clone = updater.clone();

        // 启动防抖线程：消费 channel 事件，实现防抖 + 增量更新管线
        // 管线流程：collect（收集变更路径）→ consolidate（合并根路径）→ scan_partial_path（局部重扫）
        // → subtree_replace（替换缓存子树）→ emit（通知前端）
        let debounce_thread = std::thread::spawn(move || {
            loop {
                // 50ms 超时轮询：收到事件立即处理，超时后检查防抖就绪状态
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(event) => {
                        use notify::EventKind;
                        // 仅关注 Create/Remove/Modify 事件，忽略 Access 等元数据事件
                        let should_trigger = matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
                        );
                        if !should_trigger {
                            continue;
                        }
                        let backup_suffix = format!(".{}", constants::BACKUP_EXTENSION);
                        // 过滤临时文件（.tmp）、备份文件、隐藏文件（以 . 开头）
                        for p in event.paths {
                            let path_str = p.to_string_lossy();
                            let file_name = p
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();
                            let relevant = !path_str.contains(".tmp")
                                && !path_str.ends_with(&backup_suffix)
                                && !file_name.starts_with('.');
                            if relevant {
                                // 收集相关变更路径到 IncrementalUpdater
                                if let Ok(mut u) = updater_clone.lock() {
                                    u.collect(p.clone());
                                }
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // 超时：检查是否过了防抖期（自上次 collect 后已过 300ms）
                        let ready = updater_clone
                            .lock()
                            .map(|u| u.is_ready())
                            .unwrap_or(false);
                        if ready {
                            // 检查暂停标志（全局 + 实例级），任一为 true 则跳过触发
                            let paused_global = WATCHER_PAUSED.load(Ordering::SeqCst);
                            let paused_local = paused_clone.load(Ordering::SeqCst);
                            if paused_global || paused_local {
                                if let Ok(mut u) = updater_clone.lock() {
                                    u.reset();
                                }
                                continue;
                            }

                            // consolidate：将收集到的路径合并为根路径（如从多个子文件合并到其父目录）
                            let consolidated = {
                                let mut u = updater_clone.lock().unwrap();
                                let paths = u.consolidate(&managed_path_clone);
                                u.reset();
                                paths
                            };

                            log::info!(
                                "[Incremental] consolidated {} root path(s) for refresh",
                                consolidated.len()
                            );

                            let current_game =
                                crate::config::settings_store::get_settings().target_game;
                            let mods_path_buf = managed_path_clone
                                .parent()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_else(|| managed_path_clone.clone());

                            // 对每个合并后的根路径执行局部重扫，并 subtree_replace 到缓存
                            {
                                let mut cache_guard =
                                    crate::core::mod_cache::MOD_CACHE.write();
                                for sub in &consolidated {
                                    match crate::core::mod_scanner::scan_partial_path(
                                        &mods_path_buf,
                                        sub,
                                    ) {
                                        Ok(partial) => {
                                            cache_guard.subtree_replace(
                                                current_game,
                                                &mods_path_buf,
                                                partial,
                                            );
                                        }
                                        Err(e) => log::error!(
                                            "[Incremental] scan_partial_path failed for {}: {}",
                                            sub.display(),
                                            e
                                        ),
                                    }
                                }
                            }

                            let window_visible = app_handle
                                .get_webview_window("main")
                                .and_then(|w| w.is_visible().ok())
                                .unwrap_or(false);

                            // 窗口可见时 emit 前端事件通知刷新；窗口隐藏时仅合并缓存（静默更新）
                            if window_visible {
                                // emit managed-folder-changed：通知前端扫描目录变化
                                let _ = app_handle.emit(
                                    "managed-folder-changed",
                                    managed_path_clone.as_os_str(),
                                );
                                // emit managed-partial-update：携带增量更新详情（consolidatedRoots + game）
                                let payload = serde_json::json!({
                                    "consolidatedRoots": consolidated,
                                    "game": format!("{:?}", current_game),
                                });
                                let _ = app_handle.emit("managed-partial-update", payload);
                            } else {
                                log::info!("[Incremental] window hidden -> merged cache silently");
                            }
                        }
                    }
                    // channel 发送端已关闭（stop_watching 时 drop tx），线程退出
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        self.watcher = Some(watcher);
        self._tx = Some(tx);
        self._debounce_thread = Some(debounce_thread);
        self.watched_path = Some(managed_path);
        self.updater = Some(updater);
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
        self.updater = None;
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
    pub fn switch_watched_path(&mut self, app_handle: AppHandle, game_mods_path: &Path) -> Result<()> {
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
pub fn is_file_watcher_running(
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
) -> bool {
    watcher.lock().map(|w| w.is_running()).unwrap_or(false)
}

/// 获取当前监控的 `_MANAGED_` 路径（Tauri 命令）
///
/// 前端调用 `currentWatchedPath` 触发，供前端对比是否需要切换监控路径。
/// 例如用户切换游戏时，前端调用此命令获取当前路径并与新路径比较。
#[tauri::command]
pub fn current_watched_path(
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
) -> Option<String> {
    watcher.lock().ok().and_then(|w| {
        w.watched_path().map(|p| p.to_string_lossy().into_owned())
    })
}
