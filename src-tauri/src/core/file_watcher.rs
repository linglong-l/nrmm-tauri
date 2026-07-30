//! 文件系统监控模块
//!
//! 使用 notify 库监控 _MANAGED_ 目录的文件变化，实现：
//! - 递归监控整个模组目录
//! - 防抖机制：500ms 内的多次变化合并为一次事件
//! - 暂停/恢复功能：程序自身写文件时暂停监控避免循环触发
//! - 过滤无关事件：临时文件、备份文件、隐藏文件不触发刷新
//! - 缓存失效：文件变化后自动失效模组缓存并通知前端刷新

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use once_cell::sync::Lazy;
use crate::core::constants;

/// 全局文件监控暂停标志
/// 当 NRMM 自身需要写标记文件（如 groupname、selectedindex、fav 等）时，
/// 设置为 true 避免触发文件变化事件导致循环刷新，写完后恢复为 false。
/// 使用 AtomicBool 保证线程安全和无锁访问。
pub static WATCHER_PAUSED: Lazy<AtomicBool> = Lazy::new(|| AtomicBool::new(false));

/// 文件监控器结构体
///
/// 封装 notify 的 RecommendedWatcher，配合独立的防抖线程处理事件。
/// 使用 mpsc channel 在 watcher 回调和防抖线程间传递事件。
pub struct FileWatcher {
    /// notify 库的文件监控器实例，Option 允许停止监控时释放资源
    watcher: Option<RecommendedWatcher>,
    /// 当前被监控的目录路径
    watched_path: Option<PathBuf>,
    /// 实例级别的暂停标志（独立于全局 WATCHER_PAUSED）
    paused: Arc<AtomicBool>,
    /// 事件发送端，保持 channel 打开状态
    _tx: Option<mpsc::Sender<notify::Event>>,
    /// 防抖处理线程句柄，停止监控时需要 join 等待线程退出
    _debounce_thread: Option<std::thread::JoinHandle<()>>,
}

impl FileWatcher {
    /// 创建新的空 FileWatcher 实例
    ///
    /// 此时尚未开始监控任何目录，需要调用 start_watching() 启动监控。
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
    /// 设置 paused 标志为 true，防抖线程检测到此标志后会跳过事件触发。
    /// 注意：watcher 本身仍在接收事件，只是不处理。
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// 恢复文件监控事件处理
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    /// 开始监控指定游戏模组目录下的 _MANAGED_ 文件夹
    ///
    /// # 实现细节
    /// 1. 先停止已有监控（如果存在）
    /// 2. 确保 _MANAGED_ 目录存在
    /// 3. 创建 mpsc channel 传递事件
    /// 4. 创建 notify watcher，递归监控目录
    /// 5. 启动独立的防抖线程：接收事件 → 等待 500ms 无新事件 → 触发刷新
    ///
    /// # 防抖逻辑
    /// - 每次收到相关事件重置 last_event_time
    /// - 轮询超时（100ms）时检查是否过了防抖期
    /// - 过了防抖期且未暂停则：失效缓存 + 发送前端事件
    ///
    /// # 参数
    /// - `app_handle`: Tauri 应用句柄，用于发送前端事件
    /// - `game_mods_path`: 游戏 Mods 目录路径（会自动拼接 _MANAGED_）
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
        // 启动防抖线程：消费 channel 事件，实现防抖逻辑
        let debounce_thread = std::thread::spawn(move || {
            let mut last_event_time: Option<Instant> = None;
            let mut pending = false;

            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => {
                        use notify::EventKind;
                        // 只关心创建、删除、修改事件
                        let should_trigger = matches!(
                            event.kind,
                            EventKind::Create(_)
                                | EventKind::Remove(_)
                                | EventKind::Modify(_)
                        );

                        if should_trigger {
                            let backup_suffix = format!(".{}", constants::BACKUP_EXTENSION);
                            // 过滤掉不相关的文件变化：
                            // - .tmp 临时文件（原子写入时产生）
                            // - .ini_managed_backup 备份文件
                            // - 以 . 开头的隐藏文件
                            let relevant = event.paths.iter().any(|p| {
                                let path_str = p.to_string_lossy();
                                let file_name = p.file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                !path_str.contains(".tmp")
                                    && !path_str.ends_with(&backup_suffix)
                                    && !file_name.starts_with('.')
                            });
                            if relevant {
                                last_event_time = Some(Instant::now());
                                pending = true;
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // 轮询超时：检查是否有等待中的事件且防抖期已过
                        if let Some(t) = last_event_time {
                            if pending && t.elapsed() >= debounce_duration {
                                // 双重暂停检查：全局暂停标志 + 实例暂停标志
                                if WATCHER_PAUSED.load(Ordering::SeqCst) {
                                    log::debug!("[FileWatcher] Skipping trigger - globally paused");
                                } else if paused_clone.load(Ordering::SeqCst) {
                                    log::debug!("[FileWatcher] Skipping trigger - watcher paused");
                                } else {
                                    log::info!("[FileWatcher] Change detected, invalidating cache");
                                    // 失效对应路径前缀的缓存
                                    {
                                        let mut cache = crate::core::mod_cache::MOD_CACHE.write();
                                        cache.invalidate_by_prefix(&managed_path_clone);
                                    }
                                    // 通知前端刷新模组列表
                                    let _ = app_handle.emit("managed-folder-changed", &managed_path_clone);
                                }
                                pending = false;
                                last_event_time = None;
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        // channel 发送端已释放，退出线程
                        break;
                    }
                }
            }
        });

        self.watcher = Some(watcher);
        self._tx = Some(tx);
        self._debounce_thread = Some(debounce_thread);
        self.watched_path = Some(managed_path);
        self.paused.store(false, Ordering::SeqCst);

        Ok(())
    }

    /// 停止监控并清理所有资源
    ///
    /// 1. drop watcher（停止 OS 级别的监控）
    /// 2. drop tx（关闭 channel，防抖线程会收到 Disconnected 退出）
    /// 3. join 防抖线程（等待其正常退出）
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

    /// 切换监控路径（先停止再启动）
    ///
    /// # 参数
    /// - `app_handle`: Tauri 应用句柄
    /// - `game_mods_path`: 新的游戏 Mods 目录路径
    pub fn switch_watched_path(&mut self, app_handle: AppHandle, game_mods_path: &Path) -> Result<()> {
        self.stop_watching();
        self.start_watching(app_handle, game_mods_path)
    }
}

/// 开始文件监控 Tauri 命令
///
/// 涉及 IO 操作（创建目录）和线程 join（stop_watching），使用 spawn_blocking 避免阻塞 async 运行时。
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
/// thread::join 可能阻塞，使用 spawn_blocking。
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
/// 涉及 IO 和线程操作，使用 spawn_blocking。
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
/// 仅设置原子 bool，轻量操作保持同步。
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
/// 仅设置原子 bool，轻量操作保持同步。
#[tauri::command]
pub fn resume_file_watcher(
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
) -> Result<(), String> {
    let w = watcher.lock().map_err(|e| e.to_string())?;
    w.resume();
    Ok(())
}
