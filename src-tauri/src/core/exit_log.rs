//! 程序退出记录：记录每次退出的触发方式与时间点
//!
//! 覆盖四种退出类型：
//! 1. `user_quit` —— 用户主动点击退出按钮（经后台操作检查后真正退出）
//! 2. `user_quit_cancelled` —— 用户点击退出但被后台操作取消（仅记录，未退出）
//! 3. `system_signal` —— 系统发送的退出信号（Ctrl+C / SIGTERM 等）
//! 4. `forced` —— 强制终止（SIGKILL / 任务管理器结束 / 崩溃），启动时通过未干净退出标记推断补记
//!
//! 记录落盘于 `<data_local>/nrmm-rust/exit_log.jsonl`（追加 JSON 单行），同时维护
//! `run_state.json` 标记上次是否干净退出，用于跨启动推断强制终止。

use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// 退出触发方式分类
#[derive(Debug, Clone, Copy, Serialize)]
pub enum ExitReason {
    /// 用户主动点击退出按钮（经后台操作检查后真正退出）
    UserQuit,
    /// 用户点击退出但被后台操作取消（仅记录，未退出）
    UserQuitCancelled,
    /// 系统发送的退出信号（Ctrl+C / SIGTERM 等）
    SystemSignal,
    /// 强制终止（SIGKILL / 任务管理器结束 / 崩溃），启动时通过未干净退出标记推断
    Forced,
}

#[derive(Debug, Serialize)]
struct ExitRecord {
    ts: String,
    epoch_secs: u64,
    reason: &'static str,
    detail: String,
}

fn log_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|p| p.join("nrmm-rust"))
}

fn now_iso() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%.3f%z")
        .to_string()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn reason_str(reason: ExitReason) -> &'static str {
    match reason {
        ExitReason::UserQuit => "user_quit",
        ExitReason::UserQuitCancelled => "user_quit_cancelled",
        ExitReason::SystemSignal => "system_signal",
        ExitReason::Forced => "forced",
    }
}

/// 记录一次退出事件到 `<data_local>/nrmm-rust/exit_log.jsonl`（追加，JSON 单行）。
///
/// 任何一层（目录创建失败、序列化失败、文件打开失败）均静默降级，不影响主流程。
pub fn record_exit(reason: ExitReason, detail: &str) {
    let dir = match log_dir() {
        Some(d) => d,
        None => return,
    };
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("exit_log.jsonl");
    let rec = ExitRecord {
        ts: now_iso(),
        epoch_secs: now_secs(),
        reason: reason_str(reason),
        detail: detail.to_string(),
    };
    if let Ok(json) = serde_json::to_string(&rec) {
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
            let mut w = BufWriter::new(f);
            let _ = writeln!(w, "{}", json);
            let _ = w.flush();
        }
    }
}

/// 运行态标记文件名。
const RUN_STATE_FILE: &str = "run_state.json";

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct RunState {
    last_start_secs: u64,
    last_exit_clean: bool,
}

/// 应用启动时刻调用：检测上一次是否「未干净退出」（强制终止），若是则补记一条
/// `forced` 退出记录；随后写入本次启动标记（`last_exit_clean=false`）。
pub fn mark_app_started() {
    let dir = match log_dir() {
        Some(d) => d,
        None => return,
    };
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(RUN_STATE_FILE);
    if let Ok(data) = fs::read_to_string(&path) {
        if let Ok(state) = serde_json::from_str::<RunState>(&data) {
            if !state.last_exit_clean {
                let detail = format!(
                    "上次运行（启动于 epoch={}）未记录到干净退出，推断为强制终止/崩溃",
                    state.last_start_secs
                );
                record_exit(ExitReason::Forced, &detail);
            }
        }
    }
    let new_state = RunState {
        last_start_secs: now_secs(),
        last_exit_clean: false,
    };
    if let Ok(json) = serde_json::to_string(&new_state) {
        let _ = fs::write(&path, json);
    }
}

/// 应用干净退出时刻调用：将运行态标记为「已干净退出」，并补记对应退出类型。
///
/// 在 `app.exit` 之前调用，确保下次启动不会误判为强制终止。
pub fn mark_clean_exit(reason: ExitReason) {
    let dir = match log_dir() {
        Some(d) => d,
        None => return,
    };
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(RUN_STATE_FILE);
    if let Ok(data) = fs::read_to_string(&path) {
        if let Ok(mut state) = serde_json::from_str::<RunState>(&data) {
            state.last_exit_clean = true;
            if let Ok(json) = serde_json::to_string(&state) {
                let _ = fs::write(&path, json);
            }
        }
    }
    record_exit(reason, "");
}
