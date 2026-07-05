//! 任务队列模块
//!
//! 该模块提供基于任务类型的互斥执行与请求取消机制，确保：
//! 1. 同一类型的任务在同一时刻最多只有一个实例在运行；
//! 2. 当新的同类型任务请求到来时，立即取消之前未完成的任务，优先处理最新请求；
//! 3. 不同类型的任务之间相互独立，可并发执行（不相互阻塞）。
//!
//! 核心场景：NRMM 中存在多个可能长时间运行的后台任务（如 `load_mods`、`update_mod_data`），
//! 若前端重复触发或文件监视器自动触发，可能导致并发冲突或数据不一致。
//! 通过 `TaskQueue` 可以保证同一类型的任务互斥执行，并支持请求取消，
//! 确保最终返回的数据与用户最后一次选择的操作完全一致。
//!
//! 实现方式：
//! - 内部维护一个 `HashMap<String, AbortHandle>`，每种任务类型对应一个可取消的异步任务。
//! - 使用 `tokio::task::spawn` 和 `AbortHandle` 实现任务的异步执行与取消。
//! - 当新任务到来时，先取消同名的正在运行的任务，再启动新任务。
//! - 临界区仅覆盖 HashMap 的取消-启动操作（微秒级），不持有锁至任务完成，避免不同类型任务相互阻塞。

use std::collections::HashMap;
use std::future::Future;

use tokio::sync::{Mutex, oneshot};
use tokio::task;
use thiserror::Error;

/// 任务队列错误枚举。
#[derive(Debug, Error)]
pub enum TaskQueueError {
    /// 指定类型的任务已被取消（由于新的同类型任务请求到来）。
    /// 错误信息格式：`Task '{0}' was cancelled`。
    #[error("Task '{0}' was cancelled")]
    TaskCancelled(String),

    /// 任务执行失败。
    #[error("Task execution failed: {0}")]
    ExecutionError(String),
}

/// 任务队列结构体。
///
/// 通过按任务类型（字符串标识）管理任务的执行与取消，实现：
/// - 同一类型的任务互斥执行（新请求取消旧请求）；
/// - 不同类型的任务相互独立、可并发执行；
/// - 前端可追踪任务状态（进行中、已取消、已完成）。
///
/// 内部使用 `tokio::sync::Mutex`（异步互斥锁）保护任务映射表，`tokio::task::AbortHandle` 实现任务取消，
/// 适配 Tokio 异步运行时。锁的临界区仅覆盖 HashMap 操作，不持有至任务完成。
pub struct TaskQueue {
    /// 任务类型 -> AbortHandle 的映射表。
    /// 每个任务类型首次执行时会创建一个 `AbortHandle` 并缓存于此。
    /// 临界区仅覆盖对此映射表的读写，确保同类型任务取消-启动的原子性。
    tasks: Mutex<HashMap<String, task::AbortHandle>>,
}

impl TaskQueue {
    /// 创建一个新的 `TaskQueue` 实例（初始时无任何任务）。
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// 运行指定类型的任务，支持请求取消机制。
    ///
    /// 流程：
    /// 1. 获取任务映射表锁（临界区仅覆盖 HashMap 操作，微秒级），检查是否存在同名的正在运行的任务：
    ///    - 存在：调用 `abort()` 取消该任务。
    ///    - 不存在：直接继续。
    /// 2. 使用 `tokio::task::spawn` 启动新任务，并保存其 `AbortHandle`，随后释放映射表锁。
    /// 3. 等待任务完成或被取消（不持有任何锁）。
    /// 4. 从映射表中移除已完成或已取消的任务。
    /// 5. 返回任务结果或错误。
    ///
    /// 设计要点：
    /// - 锁的临界区仅覆盖 HashMap 的取消-启动操作，不持有至任务完成，因此不同类型的任务可并发执行。
    /// - 同类型任务的取消-启动原子性由 `tasks` 锁保证；任务执行期不持锁，新请求可随时取消旧任务。
    ///
    /// 参数：
    /// - `task_type`: 任务类型标识（如 `"load_mods"`、`"update_mod_data"`）。
    /// - `future`: 要执行的异步任务，返回 `Result<O, E>`。
    ///
    /// 返回：
    /// - `Ok(O)`：任务执行成功，返回其结果。
    /// - `Err(TaskQueueError::TaskCancelled)`：任务被取消（由于新的同类型任务请求到来）。
    /// - `Err(TaskQueueError::ExecutionError)`：任务执行失败。
    ///
    /// 注意：该函数会取消之前同名的正在运行的任务，适用于「最新请求优先」的场景。
    pub async fn run_task<F, O, E>(&self, task_type: &str, future: F) -> Result<O, TaskQueueError>
    where
        F: Future<Output = Result<O, E>> + Send + 'static,
        O: Send + 'static,
        E: std::fmt::Display + Send + 'static,
    {
        // 第一步：获取任务映射表锁，取消同名的正在运行的任务
        let mut tasks = self.tasks.lock().await;
        if let Some(handle) = tasks.remove(task_type) {
            handle.abort();
        }

        // 第二步：创建 channel 用于接收任务结果
        let (tx, rx) = oneshot::channel();

        // 第三步：启动新任务
        let join_handle = task::spawn(async move {
            let result = future.await;
            let _ = tx.send(result);
        });

        // 第四步：保存 AbortHandle，以便后续取消，随后释放映射表锁
        tasks.insert(task_type.to_string(), join_handle.abort_handle());
        drop(tasks);

        // 第五步：等待任务完成或被取消（不持有任何锁，允许其他类型任务并发执行）
        match rx.await {
            Ok(result) => {
                // 任务正常完成，从映射表中移除
                let mut tasks = self.tasks.lock().await;
                tasks.remove(task_type);

                match result {
                    Ok(output) => Ok(output),
                    Err(e) => Err(TaskQueueError::ExecutionError(e.to_string())),
                }
            }
            Err(_) => {
                // 任务被取消（channel 关闭），从映射表中移除
                let mut tasks = self.tasks.lock().await;
                tasks.remove(task_type);

                Err(TaskQueueError::TaskCancelled(task_type.to_string()))
            }
        }
    }

    /// 检查指定类型的任务是否正在运行。
    ///
    /// 参数：
    /// - `task_type`: 任务类型标识。
    ///
    /// 返回：
    /// - `true`：任务正在运行。
    /// - `false`：任务未在运行。
    ///
    /// 当前未暴露到命令层，保留用于后续任务状态查询。
    #[allow(dead_code)]
    pub async fn is_running(&self, task_type: &str) -> bool {
        let tasks = self.tasks.lock().await;
        tasks.contains_key(task_type)
    }

    /// 取消指定类型的正在运行的任务。
    ///
    /// 参数：
    /// - `task_type`: 任务类型标识。
    ///
    /// 返回：
    /// - `true`：任务已成功取消。
    /// - `false`：任务不存在或未在运行。
    ///
    /// 当前未暴露到命令层，保留用于后续任务取消场景。
    #[allow(dead_code)]
    pub async fn cancel_task(&self, task_type: &str) -> bool {
        let mut tasks = self.tasks.lock().await;
        if let Some(handle) = tasks.remove(task_type) {
            handle.abort();
            true
        } else {
            false
        }
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_run_task_success() {
        let queue = TaskQueue::new();

        let result = queue.run_task("test", async { Ok::<_, String>("success") }).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        assert!(!queue.is_running("test").await);
    }

    #[tokio::test]
    async fn test_run_task_failure() {
        let queue = TaskQueue::new();

        let result = queue.run_task("test", async { Err::<String, String>("failed".to_string()) }).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TaskQueueError::ExecutionError(e) => assert_eq!(e, "failed"),
            _ => panic!("Unexpected error type"),
        }
        assert!(!queue.is_running("test").await);
    }

    #[tokio::test]
    async fn test_task_cancellation() {
        let queue = std::sync::Arc::new(TaskQueue::new());

        // 启动第一个任务（长时间运行）
        let q1 = queue.clone();
        let first_task = tokio::spawn(async move {
            q1.run_task("test", async {
                sleep(Duration::from_millis(100)).await;
                Ok::<_, String>("first")
            }).await
        });

        // 等待一小段时间确保第一个任务已启动
        sleep(Duration::from_millis(10)).await;
        assert!(queue.is_running("test").await);

        // 启动第二个任务（会取消第一个任务）
        let q2 = queue.clone();
        let second_task = tokio::spawn(async move {
            q2.run_task("test", async { Ok::<_, String>("second") }).await
        });

        // 等待第二个任务完成
        let second_result = second_task.await.unwrap();
        assert!(second_result.is_ok());
        assert_eq!(second_result.unwrap(), "second");

        // 等待第一个任务完成并检查是否被取消
        let first_result = first_task.await.unwrap();
        assert!(first_result.is_err());
        match first_result.unwrap_err() {
            TaskQueueError::TaskCancelled(t) => assert_eq!(t, "test"),
            _ => panic!("Unexpected error type"),
        }

        // 确保任务已清理
        assert!(!queue.is_running("test").await);
    }

    #[tokio::test]
    async fn test_concurrent_different_tasks() {
        let queue = std::sync::Arc::new(TaskQueue::new());

        // 启动两个不同类型的任务
        let q1 = queue.clone();
        let task1 = tokio::spawn(async move {
            q1.run_task("task1", async {
                sleep(Duration::from_millis(50)).await;
                Ok::<_, String>("task1_result")
            }).await
        });

        let q2 = queue.clone();
        let task2 = tokio::spawn(async move {
            q2.run_task("task2", async {
                sleep(Duration::from_millis(50)).await;
                Ok::<_, String>("task2_result")
            }).await
        });

        // 两个任务应该都能成功完成
        let result1 = task1.await.unwrap();
        let result2 = task2.await.unwrap();

        assert!(result1.is_ok());
        assert_eq!(result1.unwrap(), "task1_result");
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap(), "task2_result");
    }

    #[tokio::test]
    async fn test_concurrent_different_tasks_run_in_parallel() {
        let queue = std::sync::Arc::new(TaskQueue::new());

        let start = std::time::Instant::now();

        let q1 = queue.clone();
        let task1 = tokio::spawn(async move {
            q1.run_task("task1", async {
                sleep(Duration::from_millis(100)).await;
                Ok::<_, String>("task1_result")
            }).await
        });

        let q2 = queue.clone();
        let task2 = tokio::spawn(async move {
            q2.run_task("task2", async {
                sleep(Duration::from_millis(100)).await;
                Ok::<_, String>("task2_result")
            }).await
        });

        let result1 = task1.await.unwrap();
        let result2 = task2.await.unwrap();

        let elapsed = start.elapsed();

        assert!(result1.is_ok());
        assert!(result2.is_ok());
        // 如果是串行执行，需要 200ms；并发执行只需 ~100ms
        assert!(
            elapsed < Duration::from_millis(180),
            "Tasks should run concurrently, but took {:?}",
            elapsed
        );
    }
}