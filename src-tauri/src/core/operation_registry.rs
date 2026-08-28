//! 重量级后台操作注册表：跟踪进行中的文件操作（更新模组数据 / 导入 / 导出等）
//!
//! 用途：用户主动退出时，需确认所有后台线程均已完成；若存在更新模组数据等长耗时
//! 操作，则取消退出并记录调用时间点，避免用户因等待过长而强制终止程序。
//!
//! 实现：进程内全局 `HashMap<操作名, 并发计数>`，提供 begin/end 与查询接口，并配套
//! `OperationGuard` RAII 守卫确保异常路径也能正确结束计数。
//!
//! 使用计数语义：`begin` 将同名计数 +1、`end` 将同名计数 -1，归零后移除。
//! 这样并发多个同名操作时，只有全部结束后 `has_active_operations()` 才返回 false，
//! 避免旧的"同名覆盖 + 首个 end 即删整条"逻辑导致退出保护提前失效。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

struct OpState {
    active: Mutex<HashMap<String, usize>>,
}

static STATE: OnceLock<OpState> = OnceLock::new();

fn state() -> &'static OpState {
    STATE.get_or_init(|| OpState {
        active: Mutex::new(HashMap::new()),
    })
}

/// 计数语义登记：begin +1 / end -1，归零移除——并发同名操作全程受退出保护。
pub fn begin_operation(name: &str) {
    let mut m = lock_or_recover(&state().active);
    *m.entry(name.to_string()).or_insert(0) += 1;
}

/// 标记一个重量级后台操作结束（同名计数 -1，归零后移除）。
pub fn end_operation(name: &str) {
    let mut m = lock_or_recover(&state().active);
    if let Some(c) = m.get_mut(name) {
        *c = c.saturating_sub(1);
        if *c == 0 {
            m.remove(name);
        }
    }
}

/// 当前是否有进行中的重量级操作。
pub fn has_active_operations() -> bool {
    !lock_or_recover(&state().active).is_empty()
}

/// 列出进行中的操作及其并发计数，便于退出排查时比对。
pub fn active_operations() -> Vec<(String, u64)> {
    lock_or_recover(&state().active)
        .iter()
        .map(|(k, v)| (k.clone(), *v as u64))
        .collect()
}

/// 锁中毒恢复（对齐全仓 lock_or_recover 风格）：持有锁的线程 panic 导致 Mutex
/// 中毒时恢复内部数据，避免静默跳过而丢失关键状态。
fn lock_or_recover(
    m: &Mutex<HashMap<String, usize>>,
) -> std::sync::MutexGuard<'_, HashMap<String, usize>> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// RAII 守卫：构造时 `begin_operation`，`Drop` 时 `end_operation`，确保异常路径也计数正确。
pub struct OperationGuard {
    name: String,
}

impl OperationGuard {
    /// 创建守卫并登记对应操作（同名计数 +1）。
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 本模块测试操控的是进程级全局状态，需串行执行以免相互干扰。
    static SERIAL: Mutex<()> = Mutex::new(());

    /// 让全局 active Mutex 中毒：持锁线程 panic 使 Mutex 标记 poison。
    fn poison_active() {
        let handle = std::thread::spawn(|| {
            let _guard = state().active.lock().unwrap();
            panic!("intentional poison for test");
        });
        assert!(handle.join().is_err(), "测试用辅助线程应 panic 而中毒");
    }

    #[test]
    fn concurrent_same_name_counting() {
        let _serial = SERIAL.lock().unwrap();

        // 并发两个同名 import：begin 两次（用独有前缀避免与其它模块并行测试登记的真实操作串扰计数）
        begin_operation("test_op_import");
        begin_operation("test_op_import");
        assert_eq!(active_count("test_op_import"), 2);

        // 仅第一个完成（end 一次），同名仍有在跑 → 退出保护应保持生效
        end_operation("test_op_import");
        assert_eq!(
            active_count("test_op_import"),
            1,
            "count=1 时退出保护不应提前失效"
        );

        // 第二个完成后再 end 一次 → 才归零移除
        end_operation("test_op_import");
        assert_eq!(active_count("test_op_import"), 0);
    }

    #[test]
    fn poisoned_mutex_recovers() {
        let _serial = SERIAL.lock().unwrap();

        poison_active();

        // 中毒后 begin / end / 查询仍应生效（lock_or_recover 恢复内部数据）
        // 注意：断言针对本测试独有 key，而非 has_active_operations() 全局空态——
        // 全量并行下 update_mod_data_test 等会真实登记 "update_mod_data" 守卫，
        // 使全局 map 无法保证为空。
        begin_operation("test_op_import");
        assert_eq!(active_count("test_op_import"), 1);
        end_operation("test_op_import");
        assert_eq!(active_count("test_op_import"), 0);

        begin_operation("test_op_export");
        begin_operation("test_op_export");
        assert_eq!(active_count("test_op_export"), 2);
        end_operation("test_op_export");
        assert_eq!(active_count("test_op_export"), 1);
        end_operation("test_op_export");
        assert_eq!(active_count("test_op_export"), 0);
    }

    /// 读取指定操作的当前并发计数（缺省为 0）；用 lock_or_recover 兼容中毒后的全局锁。
    fn active_count(name: &str) -> usize {
        lock_or_recover(&state().active)
            .get(name)
            .copied()
            .unwrap_or(0)
    }
}
