//! 窗口生命周期分级回收（空闲内存优化，2026-08-21）
//!
//! 窗口隐藏后 WebView2（JS 堆 + 解码位图）完整驻留，空闲进程 ~250MB。
//! 分级回收：
//! - 隐藏 120s：emit `window-hidden-clear-data`，前端清空模组数据（后端缓存/监控保留）
//! - 隐藏 300s：无待显示信息（无进行中用户可见操作 + 前端无未关闭模态框）时
//!   销毁主窗口释放 WebView 全部内存，托盘/热键唤起时运行时重建
//!
//! 线程模型：计时线程使用 `std::thread::spawn`（项目硬约束：窗口事件回调、
//! 托盘回调禁止 tokio）。AppHandle 为 Send+Sync+Clone，跨线程持有安全。

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 隐藏后清空前端模组数据的延迟（秒）
pub const HIDDEN_CLEAR_DATA_SECS: u64 = 120;
/// 隐藏后销毁窗口的延迟（秒）
pub const HIDDEN_DESTROY_SECS: u64 = 300;
/// 销毁条件不满足（busy/模态框）时的重试间隔（秒）
const DESTROY_RETRY_SECS: u64 = 60;

/// 显隐世代计数器：每次隐藏或显示递增，陈旧计时线程据此静默退出
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// 进行中的用户可见操作数（update/导入/还原/保存/冲突检测）
static BUSY_OPS: AtomicUsize = AtomicUsize::new(0);
/// 前端是否存在未关闭的模态对话框（ElMessageBox）
static FRONTEND_PENDING: AtomicBool = AtomicBool::new(false);
/// 主窗口是否被运行时销毁过（前端启动时读取以跳过启动通知，读后清零）
static DESTROYED_FLAG: AtomicBool = AtomicBool::new(false);
/// 用户显式请求退出标志：托盘/硬退出置位，_un_置位表示窗口销毁触发的运行循环结束
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

/// 分级回收计时线程主体：T+120s 清数据 → T+300s 起（含重试）销毁
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

    // 第二级：销毁窗口（不足条件则每 DESTROY_RETRY_SECS 重试）
    std::thread::sleep(Duration::from_secs(
        HIDDEN_DESTROY_SECS - HIDDEN_CLEAR_DATA_SECS,
    ));
    loop {
        if is_stale(gen, &app) {
            return;
        }
        if can_destroy() {
            destroy_hidden_main_window(&app);
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

/// 世代过期（期间发生过显隐）或窗口已可见 → 计时作废
fn is_stale(gen: u64, app: &AppHandle) -> bool {
    if gen != GENERATION.load(Ordering::SeqCst) {
        return true;
    }
    app.get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// 销毁条件：无进行中用户可见操作 且 前端无未关闭模态框
fn can_destroy() -> bool {
    BUSY_OPS.load(Ordering::SeqCst) == 0 && !FRONTEND_PENDING.load(Ordering::SeqCst)
}

/// 销毁隐藏的主窗口：先持久化几何（排除 VISIBLE），再 destroy（绕过 CloseRequested 拦截）
fn destroy_hidden_main_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    use tauri_plugin_window_state::{AppHandleExt as _, StateFlags};
    // AppHandleExt 提供 save_window_state(flags)：保存所有已跟踪窗口状态
    let mut flags = StateFlags::all();
    flags.remove(StateFlags::VISIBLE); // 不把“隐藏”写入冷启动可见性
    if let Err(e) = app.save_window_state(flags) {
        log::warn!("[lifecycle] 保存窗口状态失败（继续销毁）: {}", e);
    }
    DESTROYED_FLAG.store(true, Ordering::SeqCst);
    if let Err(e) = win.destroy() {
        log::error!("[lifecycle] 销毁主窗口失败: {}", e);
        return;
    }
    log::info!(
        "[lifecycle] 主窗口已销毁（隐藏超 {}s），托盘/热键唤起时将重建",
        HIDDEN_DESTROY_SECS
    );
}

/// 确保主窗口存在：不存在（被运行时销毁）则按 tauri.conf.json 等价配置重建
///
/// 重建后经 window-state 插件恢复几何（不含 VISIBLE）；调用方负责 show/focus。
/// URL 使用显式前端入口 `WebviewUrl::App("index.html")`（`WebviewUrl::default()`
/// 亦解析为同一值），在 dev 模式解析到 devUrl、release 解析到打包资源。
pub fn ensure_main_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window("main") {
        return Some(w);
    }
    log::info!("[lifecycle] 主窗口不存在（运行时已销毁），开始重建");
    let built =
        tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
            .title("NRMM")
            .inner_size(960.0, 640.0)
            .min_inner_size(750.0, 500.0)
            .decorations(false)
            .transparent(true)
            .shadow(true)
            .center()
            .resizable(true)
            .focused(true)
            .visible(false)
            .build();
    match built {
        Ok(w) => {
            use tauri_plugin_window_state::{StateFlags, WindowExt};
            // WindowExt::restore_state(flags) 按该窗口自身 label 恢复几何（不含 VISIBLE）
            let mut flags = StateFlags::all();
            flags.remove(StateFlags::VISIBLE);
            if let Err(e) = w.restore_state(flags) {
                log::warn!("[lifecycle] 恢复窗口几何失败（使用居中默认）: {}", e);
            }
            Some(w)
        }
        Err(e) => {
            log::error!("[lifecycle] 重建主窗口失败: {}", e);
            None
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

/// 查询主窗口是否因隐藏超时被运行时销毁过（读后清零）
///
/// 用途：前端冷启动时读取，true 则跳过“启动成功”系统通知（避免每次唤醒弹通知）
#[tauri::command]
pub fn was_window_recreated() -> bool {
    DESTROYED_FLAG.swap(false, Ordering::SeqCst)
}

/// 标记用户显式请求退出（托盘/硬退出入口调用）
///
/// 用于在 `RunEvent::ExitRequested` 中区分退出来源：用户主动退出应放行，
/// 而"首个窗口被 lifecycle 销毁触发的运行循环结束"应被阻止（进程需驻留托盘等待重建）。
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
}
