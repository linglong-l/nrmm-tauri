//! 任务队列模块
//!
//! 该模块提供基于任务类型的互斥执行机制，确保同一类型的任务在同一时刻最多只有一个实例在运行。
//!
//! 核心场景：NRMM 中存在多个可能长时间运行的后台任务（如 `load_mods`、`update_mod_data`），
//! 若前端重复触发或文件监视器自动触发，可能导致并发冲突（如同时修改 INI 文件）。
//! 通过 `TaskQueue` 可以保证同一类型的任务全局单一执行，避免竞态条件。
//!
//! 实现方式：
//! - 内部维护一个 `HashMap<String, Arc<Mutex<()>>>`，每种任务类型对应一个独立的互斥锁。
//! - 执行任务时使用 `try_lock` 而非 `lock`：若锁已被占用则立即返回 `TaskAlreadyRunning` 错误，
//!   不会阻塞等待，从而避免任务堆积。

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use tokio::sync::Mutex;
use thiserror::Error;

/// 任务队列错误枚举。
///
/// 当尝试运行一个已在执行中的同类型任务时返回 `TaskAlreadyRunning`，
/// 携带任务类型名称以便前端展示友好的提示信息。
#[derive(Debug, Error)]
pub enum TaskQueueError {
    /// 指定类型的任务已在运行中。
    /// 错误信息格式：`Task of type '{0}' is already running`。
    #[error("Task of type '{0}' is already running")]
    TaskAlreadyRunning(String),
}

/// 任务队列结构体。
///
/// 通过按任务类型（字符串标识）分别加锁的方式，实现细粒度的并发控制：
/// 不同类型的任务可并行执行，同一类型的任务互斥执行。
///
/// 内部使用 `tokio::sync::Mutex`（异步互斥锁），适配 Tokio 异步运行时。
pub struct TaskQueue {
    /// 任务类型 -> 对应互斥锁的映射表。
    /// 每个任务类型首次执行时会创建一个 `Arc<Mutex<()>>` 并缓存于此。
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl TaskQueue {
    /// 创建一个新的 `TaskQueue` 实例（初始时无任何锁）。
    pub fn new() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// 运行指定类型的任务，保证同类型任务全局单一执行。
    ///
    /// 流程：
    /// 1. 获取或创建 `task_type` 对应的互斥锁（`Arc<Mutex<()>>`）。
    /// 2. 使用 `try_lock` 尝试获取锁：
    ///    - 成功：执行传入的 `future`，完成后释放锁并返回结果。
    ///    - 失败（锁已被占用）：立即返回 `TaskAlreadyRunning` 错误，不阻塞等待。
    ///
    /// 参数：
    /// - `task_type`: 任务类型标识（如 `"load_mods"`、`"update_mod_data"`）。
    /// - `future`: 要执行的异步任务。
    ///
    /// 返回：
    /// - `Ok(O)`：任务执行成功，返回其结果。
    /// - `Err(TaskQueueError::TaskAlreadyRunning)`：同类型任务已在运行中。
    ///
    /// 限制条件：使用 `try_lock` 而非阻塞式 `lock`，因此不会排队等待，
    /// 适用于「重复触发时直接拒绝」的场景。
    pub async fn run_task<F, O>(&self, task_type: &str, future: F) -> Result<O, TaskQueueError>
    where
        F: Future<Output = O>,
    {
        // 第一步：获取（或创建）该任务类型对应的互斥锁
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(task_type.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        // 第二步：尝试获取锁（非阻塞），失败则说明同类型任务正在运行
        let guard = match lock.try_lock() {
            Ok(guard) => guard,
            Err(_) => return Err(TaskQueueError::TaskAlreadyRunning(task_type.to_string())),
        };

        // 第三步：执行任务并释放锁
        let result = future.await;
        drop(guard);
        Ok(result)
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}
