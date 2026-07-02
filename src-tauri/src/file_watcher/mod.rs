//! 文件监视器模块
//!
//! 该模块基于 `notify` 库监视指定目录的文件系统变化（创建/删除/修改），
//! 并通过 Tauri 事件系统向前端发送 `mods-directory-changed` 通知。
//!
//! 核心特性：
//! - 使用防抖（debounce）机制：在文件系统事件密集触发时，仅在最末次事件后
//!   等待 500ms 静默期才向前端发送通知，避免频繁刷新导致性能问题。
//! - 支持 Windows 长路径（`\\?\` 前缀），避免 MAX_PATH 限制。
//! - 自动过滤无关事件（如隐藏文件），仅关注模组相关路径的变化。
//! - 实现 `Drop` trait，确保资源释放时自动停止监视。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// 防抖等待时长（500ms）。
///
/// 在收到文件系统事件后，会等待该时长；若期间又有新事件到达，则重新计时。
/// 仅当连续 500ms 无新事件时，才向前端发送变更通知。
const DEBOUNCE_DURATION: Duration = Duration::from_millis(500);

/// Windows 路径长度阈值（260 字符，即 MAX_PATH）。
///
/// 当路径长度达到或超过该值时，会自动添加 `\\?\` 前缀以支持长路径。
const MAX_PATH: usize = 260;

/// 文件系统变更事件结构（通过 Tauri 事件发送给前端）。
///
/// 序列化为 camelCase JSON，前端通过 `mods-directory-changed` 事件监听。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModsChangedEvent {
    /// 被监视的目录路径。
    path: String,
    /// 变更类型字符串（当前固定为 `"modified"`）。
    change_type: String,
}

/// 文件监视器结构体。
///
/// 封装了底层 `notify` watcher、防抖任务句柄及运行状态标志。
/// 通过 `start_watching` / `stop_watching` 控制监视生命周期。
pub struct FileWatcher {
    /// 底层 `notify` 推荐 watcher 实例（运行时存在，停止时取出并 drop）。
    watcher: Option<RecommendedWatcher>,
    /// 当前监视的目录路径。
    watch_path: Option<PathBuf>,
    /// 运行状态标志（原子布尔，跨线程共享）。`true` 表示正在监视。
    is_running: Arc<AtomicBool>,
    /// 防抖任务的信号发送端（每次文件系统事件都会通过此通道发送一个 `()` 信号）。
    debounce_tx: Option<mpsc::Sender<()>>,
    /// 防抖异步任务的句柄（用于在停止时中止任务）。
    debounce_handle: Option<JoinHandle<()>>,
}

impl FileWatcher {
    /// 创建一个新的 `FileWatcher` 实例（未启动监视）。
    pub fn new() -> Self {
        Self {
            watcher: None,
            watch_path: None,
            is_running: Arc::new(AtomicBool::new(false)),
            debounce_tx: None,
            debounce_handle: None,
        }
    }

    /// 启动对指定目录的递归监视。
    ///
    /// 流程：
    /// 1. 若当前已在监视，先停止旧实例。
    /// 2. 校验路径存在且为目录。
    /// 3. 将路径转换为长路径形式（Windows 下处理 MAX_PATH 限制）。
    /// 4. 创建防抖信号通道，并启动防抖异步任务。
    /// 5. 创建底层 `notify` watcher，注册回调函数处理文件系统事件。
    /// 6. 开始递归监视目标目录。
    ///
    /// 防抖机制说明：
    /// - 文件系统事件触发时，回调函数通过 `debounce_tx` 发送一个 `()` 信号。
    /// - 防抖任务收到信号后，进入 500ms 静默等待；期间若再收到信号则重置计时。
    /// - 静默期结束后，向前端发送 `mods-directory-changed` 事件。
    ///
    /// 参数：
    /// - `path`: 待监视的目录路径。
    /// - `app_handle`: Tauri 应用句柄（用于发送事件给前端）。
    ///
    /// 错误：路径不存在、非目录、创建 watcher 失败或注册监视失败时返回 `anyhow::Error`。
    pub fn start_watching(&mut self, path: &str, app_handle: AppHandle) -> Result<()> {
        // 若已在监视，先停止
        if self.is_running.load(Ordering::SeqCst) {
            warn!("File watcher is already running, stopping first");
            self.stop_watching()?;
        }

        let path = Path::new(path);
        if !path.exists() {
            anyhow::bail!("Path does not exist: {:?}", path);
        }
        if !path.is_dir() {
            anyhow::bail!("Path is not a directory: {:?}", path);
        }

        // 转换为长路径形式（Windows 专用）
        let watch_path = Self::to_long_path(path);
        let is_running = self.is_running.clone();
        // 通道容量为 1：使用 try_send 语义，事件丢失也无妨（防抖会合并处理）
        let (debounce_tx, debounce_rx) = mpsc::channel::<()>(1);

        let app_handle_clone = app_handle.clone();
        let watch_path_clone = watch_path.clone();
        let is_running_clone = is_running.clone();

        // 启动防抖异步任务
        let debounce_handle = tokio::spawn(Self::debounce_task(
            debounce_rx,
            app_handle_clone,
            watch_path_clone,
            is_running_clone,
        ));

        let debounce_tx_clone = debounce_tx.clone();
        let is_running_clone = is_running.clone();

        // 创建底层 notify watcher，注册事件回调
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            // 已停止时直接忽略事件
            if !is_running_clone.load(Ordering::SeqCst) {
                return;
            }

            match res {
                Ok(event) => {
                    // 仅处理与模组相关的事件
                    if Self::is_relevant_event(&event) {
                        debug!("File system event: {:?}", event);
                        let tx = debounce_tx_clone.clone();
                        // 异步发送防抖信号，不阻塞 watcher 线程
                        tokio::spawn(async move {
                            let _ = tx.send(()).await;
                        });
                    }
                }
                Err(e) => {
                    error!("File watcher error: {}", e);
                }
            }
        })
        .context("Failed to create file watcher")?;

        // 开始递归监视目录
        watcher
            .watch(&watch_path, RecursiveMode::Recursive)
            .with_context(|| format!("Failed to watch path: {:?}", watch_path))?;

        self.watcher = Some(watcher);
        self.watch_path = Some(watch_path.clone());
        self.is_running.store(true, Ordering::SeqCst);
        self.debounce_tx = Some(debounce_tx);
        self.debounce_handle = Some(debounce_handle);

        info!("File watcher started for path: {:?}", watch_path);
        Ok(())
    }

    /// 停止文件监视。
    ///
    /// 流程：
    /// 1. 将运行标志置为 `false`，使回调函数忽略后续事件。
    /// 2. 取消对目录的监视并 drop watcher。
    /// 3. 关闭防抖信号通道（drop sender）。
    /// 4. 中止防抖异步任务。
    /// 5. 清空保存的路径。
    ///
    /// 若当前未在监视，则静默返回 `Ok(())`。
    pub fn stop_watching(&mut self) -> Result<()> {
        if !self.is_running.load(Ordering::SeqCst) {
            debug!("File watcher is not running");
            return Ok(());
        }

        // 标记为已停止，使回调函数忽略后续事件
        self.is_running.store(false, Ordering::SeqCst);

        // 取消监视并释放 watcher
        if let Some(mut watcher) = self.watcher.take() {
            if let Some(path) = &self.watch_path {
                if let Err(e) = watcher.unwatch(path) {
                    warn!("Failed to unwatch path: {:?}, error: {}", path, e);
                }
            }
            drop(watcher);
        }

        // 关闭防抖信号通道
        if let Some(tx) = self.debounce_tx.take() {
            drop(tx);
        }

        // 中止防抖异步任务
        if let Some(handle) = self.debounce_handle.take() {
            handle.abort();
        }

        self.watch_path = None;

        info!("File watcher stopped");
        Ok(())
    }

    /// 查询当前是否正在监视。
    pub fn is_watching(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    /// 判断文件系统事件是否值得关注。
    ///
    /// 条件：
    /// 1. 事件类型为 Create / Remove / Modify / Any 之一。
    /// 2. 事件涉及的路径中至少有一个与模组相关（见 `is_mod_related_path`）。
    fn is_relevant_event(event: &Event) -> bool {
        matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_) | EventKind::Any
        ) && event.paths.iter().any(|p| Self::is_mod_related_path(p))
    }

    /// 判断路径是否与模组相关。
    ///
    /// 规则：
    /// - `.ini` 文件 → 相关
    /// - 目录 → 相关
    /// - 以 `.` 开头的隐藏文件 → 不相关
    /// - 其他文件 → 相关
    fn is_mod_related_path(path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("ini") {
                return true;
            }
        }

        if path.is_dir() {
            return true;
        }

        // 排除隐藏文件（如 .favorite、.DS_Store 等）
        if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
            if file_name.starts_with('.') {
                return false;
            }
        }

        true
    }

    /// 防抖异步任务主体。
    ///
    /// 流程：
    /// 1. 等待接收信号；收到后进入防抖循环。
    /// 2. 防抖循环：每收到一个新信号就重置 500ms 计时器；
    ///    连续 500ms 无新信号时跳出循环。
    /// 3. 检查运行状态，若已停止则退出任务。
    /// 4. 构造 `ModsChangedEvent` 并通过 `app_handle.emit` 发送给前端。
    /// 5. 通道关闭（sender 被 drop）时退出任务。
    ///
    /// 参数：
    /// - `rx`: 防抖信号接收端。
    /// - `app_handle`: Tauri 应用句柄。
    /// - `watch_path`: 被监视的目录路径。
    /// - `is_running`: 运行状态标志（共享）。
    async fn debounce_task(
        mut rx: mpsc::Receiver<()>,
        app_handle: AppHandle,
        watch_path: PathBuf,
        is_running: Arc<AtomicBool>,
    ) {
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(_) => {
                            // 防抖内循环：持续重置计时器直到静默 500ms
                            loop {
                                tokio::select! {
                                    _ = tokio::time::sleep(DEBOUNCE_DURATION) => {
                                        // 静默期结束，跳出防抖循环
                                        break;
                                    }
                                    _ = rx.recv() => {
                                        // 收到新信号，重置计时器
                                        debug!("Debounce reset due to new event");
                                    }
                                }
                            }

                            // 再次检查运行状态，避免在停止后发送事件
                            if !is_running.load(Ordering::SeqCst) {
                                break;
                            }

                            let event = ModsChangedEvent {
                                path: watch_path.to_string_lossy().to_string(),
                                change_type: "modified".to_string(),
                            };

                            // 通过 Tauri 事件系统发送通知给前端
                            match app_handle.emit("mods-directory-changed", &event) {
                                Ok(_) => {
                                    debug!("Sent mods-directory-changed event");
                                }
                                Err(e) => {
                                    error!("Failed to emit mods-directory-changed event: {}", e);
                                }
                            }
                        }
                        None => {
                            // 通道关闭（sender 被 drop），退出任务
                            debug!("Debounce channel closed, exiting debounce task");
                            break;
                        }
                    }
                }
            }
        }
    }

    /// 将路径转换为 Windows 长路径形式（添加 `\\?\` 前缀）。
    ///
    /// Windows API 默认限制路径长度为 MAX_PATH（260 字符）。
    /// 通过添加 `\\?\` 前缀可以绕过该限制，支持长达 32767 字符的路径。
    ///
    /// 仅在 Windows 平台且路径长度达到 MAX_PATH 时转换；其他情况原样返回。
    fn to_long_path(path: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let path_str = path.to_string_lossy();
            if path_str.len() >= MAX_PATH && !path_str.starts_with("\\\\?\\") {
                if let Ok(canonical) = path.canonicalize() {
                    let canonical_str = canonical.to_string_lossy();
                    if !canonical_str.starts_with("\\\\?\\") {
                        return PathBuf::from(format!("\\\\?\\{}", canonical_str));
                    }
                    return canonical;
                }
                
                let mut long_path = PathBuf::from("\\\\?\\");
                long_path.push(path);
                return long_path;
            }
        }
        path.to_path_buf()
    }
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// 析构时自动停止监视，确保资源释放。
impl Drop for FileWatcher {
    fn drop(&mut self) {
        if let Err(e) = self.stop_watching() {
            warn!("Error stopping file watcher on drop: {}", e);
        }
    }
}
