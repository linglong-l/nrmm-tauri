//! 窗口生命周期分级回收（空闲内存优化，2026-08-23 修订为空白页占位方案）
//!
//! 窗口隐藏后 WebView（JS 堆 + DOM + 图片缓存）完整驻留。分级回收：
//! - 隐藏 120s：emit `window-hidden-clear-data`，前端清空模组数据（后端缓存/监控保留）
//! - 隐藏 300s：无待显示信息（无进行中用户可见操作 + 前端无未关闭模态框）时，
//!   **导航 WebView 至 about:blank 空白页占位**——窗口保留（事件监听器全部有效），
//!   JS 上下文整体销毁释放前端内存（预期 WebView 部分低至 ~40MB），
//!   同时清空后端 MOD_INI_CACHE（深扫解析结果常驻的大头）
//!
//! 方案沿革：曾采用「销毁窗口 + 运行时重建」，但热键/托盘 handler 均在主线程
//! 运行（global-hotkey WndProc 同步分发），而 wry 明文约束 build() 必须从独立
//! 线程调用——主线程重建必然死锁（实测主线程冻结连带托盘菜单失效），故弃用。
//!
//! 线程模型：计时线程使用 `std::thread::spawn`（项目硬约束：窗口事件回调、
//! 托盘回调禁止 tokio）。AppHandle 为 Send+Sync+Clone，跨线程持有安全。
//! `navigate` 底层走 send_user_message（主线程内联/非主线程投递），均不阻塞。

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 隐藏后清空前端模组数据的延迟（秒）
pub const HIDDEN_CLEAR_DATA_SECS: u64 = 120;
/// 隐藏后空白页回收 WebView 的延迟（秒）
pub const HIDDEN_DESTROY_SECS: u64 = 300;
/// 回收条件不满足（busy/模态框）时的重试间隔（秒）
const DESTROY_RETRY_SECS: u64 = 60;

/// 显隐世代计数器：每次隐藏或显示递增，陈旧计时线程据此静默退出
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// 进行中的用户可见操作数（update/导入/还原/保存/冲突检测）
static BUSY_OPS: AtomicUsize = AtomicUsize::new(0);
/// 前端是否存在未关闭的模态对话框（ElMessageBox）
static FRONTEND_PENDING: AtomicBool = AtomicBool::new(false);
/// WebView 是否被空白页回收过（前端冷启动时读取以跳过启动通知，读后清零）
static DESTROYED_FLAG: AtomicBool = AtomicBool::new(false);
/// 空白页回收前记录的主页面 URL（恢复导航依据）
///
/// 必须成功记录后才执行 blank 导航（记录失败则放弃本次回收），
/// 确保恢复路径永远有据可依。
static MAIN_PAGE_URL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// 用户显式请求退出标志：托盘/硬退出置位，未置位表示非用户来源的退出请求
static USER_QUIT_FLAG: AtomicBool = AtomicBool::new(false);

/// 窗口隐藏时调用：递增世代并启动分级回收计时线程
///
/// 同一时间只有一个有效计时线程；先前的线程因世代过期自动退出。
pub fn on_window_hidden(app: &AppHandle) {
    let gen = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::Builder::new()
        .name("window-lifecycle".into())
        .spawn(move || hidden_timer(app, gen))
        .map_err(|e| log::warn!("[lifecycle] 启动计时线程失败: {}", e))
        .ok();
}

/// 窗口显示时调用：递增世代，取消所有未决的清空/销毁计时
pub fn on_window_shown() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// 分级回收计时线程主体：T+120s 清数据 → T+300s 起（含重试）空白页回收
fn hidden_timer(app: AppHandle, gen: u64) {
    // 第一级：清空前端模组数据
    std::thread::sleep(Duration::from_secs(HIDDEN_CLEAR_DATA_SECS));
    if is_stale(gen, &app) {
        return;
    }
    log::info!(
        "[lifecycle] 隐藏 {}s，通知前端清空模组数据",
        HIDDEN_CLEAR_DATA_SECS
    );
    let _ = app.emit("window-hidden-clear-data", ());

    // 第二级：空白页回收 WebView（不足条件则每 DESTROY_RETRY_SECS 重试）
    std::thread::sleep(Duration::from_secs(
        HIDDEN_DESTROY_SECS - HIDDEN_CLEAR_DATA_SECS,
    ));
    loop {
        if is_stale(gen, &app) {
            return;
        }
        if can_destroy() {
            blank_hidden_webview(&app, gen);
            return;
        }
        log::info!(
            "[lifecycle] 隐藏 {}s 但存在待显示信息（busy_ops={} 或前端模态框未关闭），{}s 后重试",
            HIDDEN_DESTROY_SECS,
            BUSY_OPS.load(Ordering::SeqCst),
            DESTROY_RETRY_SECS
        );
        std::thread::sleep(Duration::from_secs(DESTROY_RETRY_SECS));
    }
}

/// 世代是否已过期（期间发生过显隐）
///
/// 供计时线程各处（含 TOCTOU 二次校验）判断当前世代是否仍有效，
/// 单独抽取以便无窗口环境下单元测试世代校验逻辑。
fn generation_stale(gen: u64) -> bool {
    gen != GENERATION.load(Ordering::SeqCst)
}

/// 世代过期（期间发生过显隐）或窗口已可见 → 计时作废
fn is_stale(gen: u64, app: &AppHandle) -> bool {
    if generation_stale(gen) {
        return true;
    }
    app.get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// 回收条件：无进行中用户可见操作 且 前端无未关闭模态框
fn can_destroy() -> bool {
    BUSY_OPS.load(Ordering::SeqCst) == 0 && !FRONTEND_PENDING.load(Ordering::SeqCst)
}

/// 空白页回收：记录主页 URL → 导航 about:blank → 清空 INI 解析缓存。
///
/// - 主页 URL 必须先成功记录（恢复依据）；记录失败则放弃本次回收（下次重试）。
/// - `navigate` 仅触发导航不等待加载，任何线程调用均不阻塞。
/// - DESTROYED_FLAG 置位供 `was_window_recreated` 查询（前端冷启动跳过启动通知）
///   与 `restore_webview_if_blank` 恢复判定；标志清零由前端查询（读后清）完成。
fn blank_hidden_webview(app: &AppHandle, gen: u64) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    // 二次校验（TOCTOU 补丁）：检查与执行之间用户可能已唤起窗口——
    // 世代过期或窗口已可见则放弃本次回收（下轮重试）
    if generation_stale(gen) {
        return;
    }
    // 窗口已可见则放弃本次回收
    if win.is_visible().unwrap_or(true) {
        // 查询失败视为可见（保守）
        return;
    }
    // 1) 记录主页 URL（失败放弃，保证恢复有据）
    let main_url = match win.url() {
        Ok(u) => u.to_string(),
        Err(e) => {
            log::warn!("[lifecycle] 获取主页 URL 失败，放弃本次空白页回收: {}", e);
            return;
        }
    };
    if let Ok(mut slot) = MAIN_PAGE_URL.lock() {
        *slot = Some(main_url);
    }
    // 2) 导航空白页（销毁 JS 上下文，释放前端堆/位图）
    let blank = tauri::Url::parse("about:blank").expect("静态合法 URL");
    if let Err(e) = win.navigate(blank) {
        log::error!("[lifecycle] 导航空白页失败: {}", e);
        return;
    }
    DESTROYED_FLAG.store(true, Ordering::SeqCst);
    // 3) 一并清空后端深度解析 INI 缓存（空闲内存大头）
    let cleared = crate::core::mod_ini_cache::clear_ini_cache();
    log::info!(
        "[lifecycle] WebView 已导航空白页占位（隐藏超 {}s），INI 缓存清空 {} 条；唤起时恢复页面",
        HIDDEN_DESTROY_SECS,
        cleared
    );
}

/// 空白页占位恢复：若 WebView 处于回收态则导航回主页面（前端冷启动）。
///
/// # 参数
/// - `win`: 主窗口引用（调用方已确认存在）
///
/// # 返回
/// `true` 表示本次触发了恢复导航（页面加载中，调用方照常 show/focus）；
/// `false` 表示非回收态或恢复失败（失败时保持现状并记日志，下次唤起重试）。
///
/// 注意：不清零 DESTROYED_FLAG——由前端冷启动查询 `was_window_recreated`
/// （读后清零）完成；恢复导航失败时标志保持，下次唤起再次尝试恢复。
pub fn restore_webview_if_blank(win: &tauri::WebviewWindow) -> bool {
    if !DESTROYED_FLAG.load(Ordering::SeqCst) {
        return false;
    }
    let main_url = match MAIN_PAGE_URL.lock() {
        Ok(slot) => slot.clone(),
        Err(_) => None,
    };
    let Some(url_str) = main_url else {
        log::error!("[lifecycle] 回收态但主页 URL 记录缺失，无法恢复");
        return false;
    };
    match tauri::Url::parse(&url_str) {
        Ok(u) => match win.navigate(u) {
            Ok(()) => {
                log::info!("[lifecycle] WebView 从空白页恢复，导航回主页面");
                true
            }
            Err(e) => {
                log::error!("[lifecycle] 恢复导航失败: {}", e);
                false
            }
        },
        Err(e) => {
            log::error!("[lifecycle] 记录的主页 URL 无法解析（{}）: {}", url_str, e);
            false
        }
    }
}

/// 用户可见长操作的 RAII 计数守卫：构造 +1、Drop -1（panic 经 unwind 亦会 Drop）
pub struct BusyOpGuard;

impl BusyOpGuard {
    pub fn new() -> Self {
        BUSY_OPS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Default for BusyOpGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BusyOpGuard {
    fn drop(&mut self) {
        BUSY_OPS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 前端上报模态对话框存在状态（销毁门禁条件之一）
#[tauri::command]
pub fn set_frontend_pending_info(pending: bool) {
    FRONTEND_PENDING.store(pending, Ordering::SeqCst);
}

/// 查询 WebView 是否因隐藏超时被空白页回收过（读后清零）
///
/// 用途：前端冷启动（空白页恢复）时读取，true 则跳过“启动成功”系统通知
/// （避免每次唤醒弹通知）
#[tauri::command]
pub fn was_window_recreated() -> bool {
    DESTROYED_FLAG.swap(false, Ordering::SeqCst)
}

/// 标记用户显式请求退出（托盘/硬退出入口调用）
///
/// 用于在 `RunEvent::ExitRequested` 中区分退出来源：用户主动退出应放行，
/// 非用户来源的退出请求则阻止（进程驻留托盘；空白页方案下窗口不销毁，
/// 此防护保留作兜底）。
pub fn mark_user_quit() {
    USER_QUIT_FLAG.store(true, Ordering::SeqCst);
}

/// 判断 RunEvent::ExitRequested 是否应被阻止。
///
/// # 规则
/// - 用户显式退出（USER_QUIT_FLAG=true）→ 放行（返回 false）
/// - 非用户退出（窗口销毁触发、run 循环空转）→ 阻止（返回 true）
///
/// 调用后清空 USER_QUIT_FLAG，避免一次 quit 后污染后续判定。
pub fn should_prevent_exit_on_windows_closed() -> bool {
    !USER_QUIT_FLAG.swap(false, Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// can_destroy 四象限：(busy, pending) 组合门禁
    #[test]
    fn can_destroy_four_quadrants() {
        // (false, false) → 可销毁
        BUSY_OPS.store(0, Ordering::SeqCst);
        FRONTEND_PENDING.store(false, Ordering::SeqCst);
        assert!(can_destroy());

        // (true, false) → 有进行中操作，不可销毁
        BUSY_OPS.store(1, Ordering::SeqCst);
        FRONTEND_PENDING.store(false, Ordering::SeqCst);
        assert!(!can_destroy());

        // (false, true) → 前端有未关闭模态框，不可销毁
        BUSY_OPS.store(0, Ordering::SeqCst);
        FRONTEND_PENDING.store(true, Ordering::SeqCst);
        assert!(!can_destroy());

        // (true, true) → 均有，不可销毁
        BUSY_OPS.store(1, Ordering::SeqCst);
        FRONTEND_PENDING.store(true, Ordering::SeqCst);
        assert!(!can_destroy());

        BUSY_OPS.store(0, Ordering::SeqCst);
        FRONTEND_PENDING.store(false, Ordering::SeqCst);
        assert!(can_destroy());
    }

    /// BusyOpGuard RAII：构造+1，Drop-1
    #[test]
    fn busy_op_guard_raii() {
        BUSY_OPS.store(0, Ordering::SeqCst);
        {
            let _guard = BusyOpGuard::new();
            assert_eq!(BUSY_OPS.load(Ordering::SeqCst), 1);
        }
        assert_eq!(BUSY_OPS.load(Ordering::SeqCst), 0);
    }

    /// 销毁阶段必须晚于清数据阶段（编译期校验，避免调整常量破坏分级顺序）
    #[test]
    fn constants_ordering() {
        const _: () = assert!(HIDDEN_DESTROY_SECS > HIDDEN_CLEAR_DATA_SECS);
        const _: () = assert!(HIDDEN_CLEAR_DATA_SECS > 0);
        const _: () = assert!(DESTROY_RETRY_SECS > 0);
    }

    /// 用户显式退出放行：mark_user_quit 后 should_prevent_exit_on_windows_closed 返回 false
    #[test]
    fn user_quit_allows_exit() {
        USER_QUIT_FLAG.store(false, Ordering::SeqCst);
        // 未标记用户退出（窗口销毁触发）→ 应阻止
        assert!(should_prevent_exit_on_windows_closed());
        // 标记用户退出 → 应放行
        mark_user_quit();
        assert!(!should_prevent_exit_on_windows_closed());
        // 状态已清空，下次（非用户退出）再阻止
        assert!(should_prevent_exit_on_windows_closed());
    }

    /// TOCTOU 世代校验：世代一致才允许回收，显隐递增后旧世代视为过期
    #[test]
    fn generation_stale_tracks_generation() {
        let base = GENERATION.load(Ordering::SeqCst);
        // 与当前世代一致 → 未过期（可执行二次校验后的导航）
        assert!(!generation_stale(base));
        // 显隐使世代递增，旧世代视为过期 → 放弃本次回收（下一轮重试）
        GENERATION.store(base + 1, Ordering::SeqCst);
        assert!(generation_stale(base));
        // 新世代不再过期
        assert!(!generation_stale(base + 1));
        // 清理，避免污染其它测试
        GENERATION.store(base, Ordering::SeqCst);
    }
}
