//! 文件监控器 —— 事件环形缓冲 / 溢出退避 / 暂停守卫域
//!
//! 从 file_watcher.rs 拆出（2026-09-19 剩余巨石拆分 PA-2）。
//! 承载 notify 事件环形队列（目录级聚合 + 溢出标志）、溢出退避静默期计算，
//! 以及全局暂停计数（`WATCHER_PAUSED`）与 RAII 守卫（`WatcherGuard`）。
//! 增量根分类 / 批次路由见 [`crate::core::watcher_incremental`]，事件发射见
//! [`crate::core::watcher_emit`]，FileWatcher 主体见 [`crate::core::watcher_core`]。

use crate::core::constants;
use once_cell::sync::Lazy;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// 事件环形队列容量（2026-08-19 用户指定；R2 由 120 上调至 256）。
///
/// 设计意图：正常人工操作（增删改少量模组）产生的脏目录数远小于容量，走精确增量路径；
/// 风暴场景（INI 备份/更新模组数据写入数万文件，目录数远超容量）必然溢出，
/// 快速转入分层全量降级路径（按顶层 group/mutex 分类全量），避免逐目录精确处理
/// 数千至上万个根路径的串行开销。R2 配合事件平息静默期 + 溢出退避
/// （连续溢出逐轮 +1s 封顶 3s），风暴批次合并更大、溢出率进一步下降。
const RING_CAPACITY: usize = 256;

/// notify 事件环形缓冲：目录级聚合 + 溢出标志（notify 回调线程生产，防抖线程消费）。
///
/// - **目录级去重**：入队单元是事件文件的**父目录**（同目录多次文件事件合并为 1 条），
///   `dedup` 集合使重复目录直接跳过；
/// - **有界**：容量 [`RING_CAPACITY`]；满时置 `overflow` 并**丢弃后续新目录**——
///   溢出路径的全量扫描天然覆盖被丢弃目录，语义安全（宁全量不漏扫），且不阻塞 notify 回调线程；
/// - mpsc channel 仅承载**唤醒信号**（事件本体走本缓冲），空闲时防抖线程可无限期阻塞。
pub(crate) struct EventRingBuffer {
    /// 脏目录队列（FIFO）
    queue: VecDeque<PathBuf>,
    /// 已在队列中的目录集合（入队去重，O(1) 判重）
    dedup: HashSet<PathBuf>,
    /// 溢出标志：队列曾满，本轮处理需走分层全量降级
    overflow: bool,
    /// 最近一次 push 时刻（防抖窗口起点）
    last_push_at: Option<Instant>,
    /// 连续溢出批次数（溢出退避依据；非溢出批次 drain 后清零）
    pub(crate) consecutive_overflows: u32,
}

impl EventRingBuffer {
    /// 创建空缓冲
    pub(crate) fn new() -> Self {
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
    pub(crate) fn push(&mut self, dir: PathBuf) {
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
    pub(crate) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// 防抖是否到期：队列非空且距最近一次 push 已超过静默期时长
    ///
    /// # 参数
    /// - `debounce`: 静默期时长（事件平息 [`constants::FILE_WATCHER_DEBOUNCE_MS`]；
    ///   溢出退避时由调用方传入 [`backoff_quiet_period`] 计算后的延长值）
    pub(crate) fn is_ready(&self, debounce: Duration) -> bool {
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
    pub(crate) fn drain(&mut self) -> (Vec<PathBuf>, bool) {
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

/// 静默期计算：非溢出批次使用普通静默期 `base`；溢出批次切换到**独立**退避阶梯
/// `FILE_WATCHER_OVERFLOW_BACKOFF_BASE_MS + min(连续溢出轮数 - 1, 2) × 1000ms`，
/// 绝对上限 [`constants::FILE_WATCHER_MAX_BACKOFF_MS`]。
///
/// 连续溢出 1/2/3 轮 → 1000/2000/3000ms；退避拉长静默期使风暴合并更大批次，
/// 降低 ring 溢出频率；非溢出批次（计数清零）回到 base。
///
/// 退避阶梯与 `base` 解耦（2026-09-20）：溢出意味着事件风暴已填满事件环，需要足够长的
/// 静默期压制；若退避以普通静默期为基数，调小普通静默期会同步削弱溢出首次保护。
pub(crate) fn backoff_quiet_period(base: Duration, consecutive_overflows: u32) -> Duration {
    if consecutive_overflows == 0 {
        return base;
    }
    let step_ms = u64::from(std::cmp::min(consecutive_overflows.saturating_sub(1), 2) * 1000);
    let quiet =
        Duration::from_millis(constants::FILE_WATCHER_OVERFLOW_BACKOFF_BASE_MS + step_ms);
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

#[cfg(test)]
#[path = "watcher_ring_tests.rs"]
mod tests;
