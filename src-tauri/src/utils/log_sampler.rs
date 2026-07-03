//! 日志采样器模块
//!
//! 提供基于时间窗口的日志采样机制，对高频同类日志仅输出首次，抑制后续重复输出。
//!
//! # 设计目的
//! 文件监听等场景下，单次用户操作可能触发数十甚至上百条相同类型的日志，
//! 全量输出既会污染日志文件，又会拖慢 IO。采样器以 `key` 为粒度，
//! 在指定时间窗口（默认 1 秒）内只允许首次日志通过，后续重复日志被静默抑制。
//!
//! # 线程安全
//! 内部使用 `std::sync::Mutex`（而非 `tokio::sync::Mutex`）保护映射表，
//! 因为日志采样的临界区极短，无需跨 `.await` 持锁，标准互斥锁性能更优。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 默认采样窗口（1 秒）。
///
/// 同一 key 在此窗口内的重复日志将被抑制，超过窗口期后才允许再次输出。
const DEFAULT_WINDOW: Duration = Duration::from_secs(1);

/// 日志采样器：基于时间窗口对同类日志进行采样。
///
/// 以 `key` 为粒度记录每个日志类别的上次输出时间，
/// 在 [`DEFAULT_WINDOW`]（或自定义窗口）内仅允许首次输出通过。
///
/// # 设计目的
/// 抑制高频同类日志，避免日志风暴；同时保证不同 key 之间互不影响。
pub struct LogSampler {
    /// `key -> 上次输出时间` 的映射表，受互斥锁保护以保证线程安全。
    last_log: Mutex<HashMap<String, Instant>>,
}

impl LogSampler {
    /// 创建一个新的空日志采样器。
    ///
    /// # 返回值
    /// 返回一个内部映射表为空的 [`LogSampler`] 实例。
    pub fn new() -> Self {
        Self {
            last_log: Mutex::new(HashMap::new()),
        }
    }

    /// 检查指定 key 的日志是否应该输出。
    ///
    /// # 工作原理
    /// - 若该 key 首次出现，或距上次输出时间超过 [`DEFAULT_WINDOW`]，返回 `true` 并更新时间戳；
    /// - 若仍在窗口期内，返回 `false`，且不更新时间戳（保持原窗口边界）。
    ///
    /// # 参数
    /// - `key`: 日志类别标识（如 `"file_event"`）。不同 key 独立计数，互不影响。
    ///
    /// # 返回值
    /// - `true`: 当前日志应当输出；
    /// - `false`: 当前日志被抑制。
    ///
    /// # 设计目的
    /// 在调用方仅需一行 `if sampler.should_log("xxx") { ... }` 即可完成采样判断，
    /// 隐藏内部时间戳管理与并发同步细节。
    pub fn should_log(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut map = self
            .last_log
            .lock()
            .expect("LogSampler mutex poisoned; sampling state unavailable");

        match map.get(key) {
            // 该 key 之前输出过：检查是否已超出窗口期
            Some(last) => {
                if now.duration_since(*last) >= DEFAULT_WINDOW {
                    map.insert(key.to_string(), now);
                    true
                } else {
                    false
                }
            }
            // 该 key 首次出现：允许输出并记录时间戳
            None => {
                map.insert(key.to_string(), now);
                true
            }
        }
    }
}

impl Default for LogSampler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    /// 首次调用 `should_log` 应返回 `true`。
    #[test]
    fn test_first_log_allows() {
        let sampler = LogSampler::new();
        assert!(
            sampler.should_log("file_event"),
            "首次调用 should_log 应当返回 true"
        );
    }

    /// 窗口期内第二次调用 `should_log` 应返回 `false`（被抑制）。
    #[test]
    fn test_second_log_within_window_suppressed() {
        let sampler = LogSampler::new();
        assert!(sampler.should_log("file_event"), "首次调用应返回 true");
        assert!(
            !sampler.should_log("file_event"),
            "窗口期内第二次调用应返回 false"
        );
    }

    /// 超过窗口期后调用 `should_log` 应返回 `true`（重新允许输出）。
    #[test]
    fn test_log_after_window_allows() {
        let sampler = LogSampler::new();
        assert!(sampler.should_log("file_event"), "首次调用应返回 true");
        // 等待超过默认窗口期（1 秒）后再调用
        sleep(Duration::from_millis(1100));
        assert!(
            sampler.should_log("file_event"),
            "超过窗口期后调用应返回 true"
        );
    }

    /// 不同 key 之间应独立计数，互不影响。
    #[test]
    fn test_different_keys_independent() {
        let sampler = LogSampler::new();
        assert!(sampler.should_log("key_a"), "key_a 首次调用应返回 true");
        assert!(
            sampler.should_log("key_b"),
            "key_b 首次调用应返回 true，不受 key_a 影响"
        );
        assert!(
            !sampler.should_log("key_a"),
            "key_a 在窗口期内第二次调用应返回 false"
        );
        assert!(
            !sampler.should_log("key_b"),
            "key_b 在窗口期内第二次调用应返回 false"
        );
    }
}
