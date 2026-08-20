//! 重量级后台操作注册表：跟踪进行中的文件操作（更新模组数据 / 导入 / 导出等）
//!
//! 用途：用户主动退出时，需确认所有后台线程均已完成；若存在更新模组数据等长耗时
//! 操作，则取消退出并记录调用时间点，避免用户因等待过长而强制终止程序。
//!
//! 实现：进程内全局 `HashMap<操作名, 开始时刻>`，提供 begin/end 与查询接口，并配套
//! `OperationGuard` RAII 守卫确保异常路径也能正确结束计数。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct OpState {
    active: Mutex<HashMap<String, Instant>>,
}

static STATE: OnceLock<OpState> = OnceLock::new();

fn state() -> &'static OpState {
    STATE.get_or_init(|| OpState {
        active: Mutex::new(HashMap::new()),
    })
}

/// 标记一个重量级后台操作开始（按名称去重计数）。
pub fn begin_operation(name: &str) {
    if let Ok(mut m) = state().active.lock() {
        m.insert(name.to_string(), Instant::now());
    }
}

/// 标记一个重量级后台操作结束。
pub fn end_operation(name: &str) {
    if let Ok(mut m) = state().active.lock() {
        m.remove(name);
    }
}

/// 当前是否有进行中的重量级操作。
pub fn has_active_operations() -> bool {
    state()
        .active
        .lock()
        .map(|m| !m.is_empty())
        .unwrap_or(false)
}

/// 列出进行中的操作及其已运行时长（秒），便于退出排查时比对时间点。
pub fn active_operations() -> Vec<(String, u64)> {
    match state().active.lock() {
        Ok(m) => m
            .iter()
            .map(|(k, v)| (k.clone(), v.elapsed().as_secs()))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// RAII 守卫：构造时 `begin_operation`，`Drop` 时 `end_operation`，确保异常路径也计数正确。
pub struct OperationGuard {
    name: String,
}

impl OperationGuard {
    /// 创建守卫并开始计时对应操作。
    pub fn new(name: &str) -> Self {
        begin_operation(name);
        OperationGuard {
            name: name.to_string(),
        }
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        end_operation(&self.name);
    }
}
