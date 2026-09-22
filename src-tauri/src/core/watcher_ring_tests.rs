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

/// 静默期计算（R2 + 解耦）：非溢出（0 轮）用普通静默期 base；连续溢出 1/2/3 轮 →
/// 1000/2000/3000ms 的**独立**退避阶梯（与 base 解耦，调小 base 不削弱溢出保护）；
/// 3 轮起封顶 FILE_WATCHER_MAX_BACKOFF_MS
#[test]
fn backoff_quiet_period_levels() {
    let base = Duration::from_millis(constants::FILE_WATCHER_DEBOUNCE_MS);
    assert_eq!(
        base.as_millis(),
        200,
        "普通静默期基线应为 200ms（2026-09-20 由 1000ms 下调：交互式单次操作的尾部等待）"
    );
    // 无溢出 → 普通静默期
    assert_eq!(backoff_quiet_period(base, 0), base);
    // 连续溢出 1/2/3 轮 → 独立退避阶梯（与 base 无关）
    assert_eq!(backoff_quiet_period(base, 1), Duration::from_millis(1000));
    assert_eq!(backoff_quiet_period(base, 2), Duration::from_millis(2000));
    assert_eq!(backoff_quiet_period(base, 3), Duration::from_millis(3000));
    assert_eq!(backoff_quiet_period(base, 9), Duration::from_millis(3000));
    // 解耦验证：base 取值不影响溢出退避阶梯（调小普通静默期不削弱溢出首次保护）
    let tiny_base = Duration::from_millis(50);
    assert_eq!(
        backoff_quiet_period(tiny_base, 1),
        Duration::from_millis(1000)
    );
    assert_eq!(
        backoff_quiet_period(tiny_base, 2),
        Duration::from_millis(2000)
    );
    assert_eq!(
        backoff_quiet_period(tiny_base, 3),
        Duration::from_millis(3000)
    );
    // 封顶为绝对上限：更大轮数不越 FILE_WATCHER_MAX_BACKOFF_MS
    assert_eq!(
        backoff_quiet_period(base, 5),
        Duration::from_millis(constants::FILE_WATCHER_MAX_BACKOFF_MS)
    );
}

/// EventRingBuffer 连续溢出计数：溢出批次 drain 递增、非溢出批次清零（退避依据）
#[test]
fn ring_buffer_consecutive_overflow_counter() {
    assert_eq!(RING_CAPACITY, 256, "R2 容量规格：120 → 256");
    let mut ring = EventRingBuffer::new();
    // 填满 + 1 → 溢出批次
    let fill = |ring: &mut EventRingBuffer| {
        for i in 0..RING_CAPACITY {
            ring.push(PathBuf::from(format!("/mods/_MANAGED_/g{i}/Mod")));
        }
        ring.push(PathBuf::from("/mods/_MANAGED_/extra/Mod"));
    };
    fill(&mut ring);
    let (_, o) = ring.drain();
    assert!(o);
    assert_eq!(ring.consecutive_overflows, 1, "首次溢出后计数为 1");
    // 连续再溢出两轮
    fill(&mut ring);
    ring.drain();
    fill(&mut ring);
    ring.drain();
    assert_eq!(ring.consecutive_overflows, 3, "连续溢出 3 轮计数为 3");
    // 非溢出批次 → 清零
    ring.push(PathBuf::from("/mods/_MANAGED_/g0/Mod"));
    let (_, o2) = ring.drain();
    assert!(!o2);
    assert_eq!(ring.consecutive_overflows, 0, "非溢出批次清零退避");
}
