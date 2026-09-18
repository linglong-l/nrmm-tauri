//! 文件日志双输出模块（防阻塞设计）
//!
//! 提供 `DualWriter`，作为 `env_logger::Target::Pipe` 的输出目标，
//! 使所有 `log::xxx!` 调用同时写入每日日志文件和 stderr（异步排水）。
//!
//! # 日志路径
//! `<data_local_dir>/nrmm-rust/logs/YYYY-MM-DD.log`
//!
//! # 防阻塞设计（2026-08-18）
//! - **文件优先**：`write()` 先同步写文件（失败忽略），日志落盘不依赖 stderr 状态
//! - **stderr 异步排水**：stderr 写入投递到有界通道（`sync_channel(1024)`），
//!   由后台线程 "stderr-drain" 消费后调用 `platform::console::write_console_err`
//! - **阻塞隔离动机**：Windows 控制台 QuickEdit 选择模式下 `io::stderr()` 同步写会
//!   无限期阻塞，而 `env_logger` 全局 `Mutex` 会因单次阻塞写卡死全应用；异步排水把
//!   阻塞点移出日志调用方，通道满时 `try_send` 丢弃本条日志而非阻塞（取舍：stderr
//!   仅是调试辅助输出，丢弃远优于冻结 UI / 无界涨内存）
//! - **降级策略**：排水线程启动失败或接收端断开时，退化回同步直写
//!   `write_console_err`（仍绕过 `io::stderr()` 直连，由 console 模块保证不 panic）
//! - 目录或文件创建失败时降级为仅 stderr（`file = None`），不 panic
//!
//! 另提供 `install_panic_hook()`：进程级 panic hook，将 panic 信息经 `log::error!`
//! 落盘；debug 构建链式保留默认 hook 输出，release 构建不调用默认 hook
//! （项目硬约束：release 无控制台输出）。

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};

/// stderr 排水通道容量上限
///
/// 取舍：有界通道 + `try_send`，stderr 长时间阻塞（如 QuickEdit 选择模式）时
/// 丢弃新日志而非无界堆积内存；1024 条足以缓冲正常日志突发。
const STDERR_CHANNEL_CAPACITY: usize = 1024;

/// stderr 排水通道发送端（首次调用 `ensure_stderr_drain_started` 时懒创建）
static STDERR_TX: OnceLock<SyncSender<Arc<[u8]>>> = OnceLock::new();

/// stderr 排水通道接收端暂存槽类型（`Mutex` 保证排水线程启动时原子取走）
type StderrRxSlot = Mutex<Option<Receiver<Arc<[u8]>>>>;

/// stderr 排水通道接收端暂存（排水线程启动时取走，取走后恒为 `None`）
static STDERR_RX: OnceLock<StderrRxSlot> = OnceLock::new();

/// 排水线程是否已尝试启动（`ensure_stderr_drain_started` 幂等标记）
static DRAIN_THREAD_SPAWNED: AtomicBool = AtomicBool::new(false);

/// stderr 排水降级标记：排水线程启动失败或接收端断开时置位，
/// 此后 `DualWriter` 退化为在调用方线程直接调用 `write_console_err`
static DRAIN_DEGRADED: AtomicBool = AtomicBool::new(false);

/// 把日志投递到 stderr（异步排水或降级直写）
///
/// 投递路径优先级：
/// 1. 排水降级（线程不可用）→ 调用方线程直写 `write_console_err`
/// 2. 通道可用 → `try_send` 非阻塞投递；通道满丢弃本条，接收端断开则置降级标记并直写
/// 3. 通道未初始化（防御分支，`DualWriter::new` 已保证初始化）→ 直写
///
/// 任何路径均不阻塞、不 panic、不返回错误。
///
/// # 参数
/// - `buf`: 待写入 stderr 的字节缓冲区
fn dispatch_stderr(buf: &[u8]) {
    // 降级路径：排水线程不可用，退化为同步直写（console 模块保证不 panic）
    if DRAIN_DEGRADED.load(Ordering::Relaxed) {
        crate::platform::console::write_console_err(buf);
        return;
    }

    let Some(tx) = STDERR_TX.get() else {
        // 防御分支：通道未初始化（理论不可达，DualWriter::new 已 ensure），直写兜底
        crate::platform::console::write_console_err(buf);
        return;
    };

    match tx.try_send(Arc::from(buf)) {
        Ok(()) => {}
        // 通道满：丢弃本条日志（stderr 为辅助输出，丢弃优于阻塞调用方）
        Err(TrySendError::Full(_)) => {}
        // 接收端断开（排水线程已退出）：置降级标记，本条直写，后续走降级路径
        Err(TrySendError::Disconnected(_)) => {
            DRAIN_DEGRADED.store(true, Ordering::Relaxed);
            crate::platform::console::write_console_err(buf);
        }
    }
}

/// stderr 排水线程主循环
///
/// 循环接收通道消息并调用 `write_console_err`；每次迭代独立 `catch_unwind`，
/// 单条日志写入 panic 不会退出循环（等效自动恢复），仅丢弃该条。
///
/// # 参数
/// - `rx`: 排水通道接收端（通道断开时 `recv` 返回 `Err`，线程自然退出）
fn stderr_drain_loop(rx: Receiver<Arc<[u8]>>) {
    while let Ok(buf) = rx.recv() {
        // write_console_err 契约保证不 panic，此处 catch_unwind 为纵深防御：
        // 即使意外 panic 也仅丢弃本条，排水线程继续服务
        let _ = std::panic::catch_unwind(|| {
            crate::platform::console::write_console_err(&buf);
        });
    }
}

/// 确保 stderr 排水设施已启动（幂等，可安全并发/重复调用）
///
/// 首次调用时创建有界通道并启动命名后台线程 "stderr-drain"；
/// 后续调用直接返回，不重复创建通道或线程。
///
/// # 降级策略
/// 线程启动失败时置位 `DRAIN_DEGRADED`，此后 stderr 写入退化为
/// 调用方线程直接调用 `write_console_err`（同步但绕过 `io::stderr()`，
/// 仍优于阻塞在系统 stderr 上）。
pub fn ensure_stderr_drain_started() {
    // 懒创建通道（OnceLock 保证仅创建一次）
    STDERR_TX.get_or_init(|| {
        let (tx, rx) = sync_channel(STDERR_CHANNEL_CAPACITY);
        let _ = STDERR_RX.set(Mutex::new(Some(rx)));
        tx
    });

    // 线程仅尝试启动一次（swap 的旧值为 true 表示已有并发调用者完成启动流程）
    if DRAIN_THREAD_SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }

    // 取出接收端交给排水线程（仅本次调用能取到 Some）
    let rx = STDERR_RX
        .get()
        .and_then(|m| m.lock().ok().and_then(|mut guard| guard.take()));

    let Some(rx) = rx else {
        // 接收端丢失或锁中毒：降级为直写
        DRAIN_DEGRADED.store(true, Ordering::Relaxed);
        return;
    };

    // 启动命名排水线程；失败则降级为直写
    if std::thread::Builder::new()
        .name("stderr-drain".to_string())
        .spawn(move || stderr_drain_loop(rx))
        .is_err()
    {
        DRAIN_DEGRADED.store(true, Ordering::Relaxed);
    }
}

/// 安装进程级 panic hook
///
/// panic 发生时通过 `log::error!` 记录 panic 消息、位置（file:line:column）
/// 与线程名，经由 `DualWriter` 落盘到当日日志文件。
///
/// # 构建差异
/// - debug（`debug_assertions`）：先 `take_hook()` 保留默认 hook，记录后链式调用，
///   开发者控制台仍可见完整默认输出
/// - release：不调用默认 hook（项目硬约束：release 无控制台输出），仅落盘
///
/// # 注意
/// hook 内部不得再 panic：仅使用 `downcast_ref`/`format!`/`log::error!` 等安全操作
pub fn install_panic_hook() {
    // debug 构建保留默认 hook 以链式调用；release 下该变量不存在，闭包不捕获
    #[cfg(debug_assertions)]
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        // 提取 panic 消息（payload 为 &str 或 String，其余类型给出占位描述）
        let payload = info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "<非字符串 panic payload>".to_string()
        };

        // 提取位置（file:line:column）
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<未知位置>".to_string());

        // 提取线程名（主线程为 "main"，未命名线程给出占位）
        let thread_name = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();

        // 经 DualWriter 落盘（文件优先 + stderr 异步排水，hook 内不会阻塞）
        log::error!(
            "[PANIC] 线程 '{}' 在 {} 发生 panic: {}",
            thread_name,
            location,
            msg
        );

        // debug 构建链式调用默认 hook（控制台可见标准 panic 输出）
        #[cfg(debug_assertions)]
        default_hook(info);
    }));
}

// ===== 日志时区（2026-09-08 时区不匹配修复）=====
//
// 根因：env_logger 默认格式的时间戳恒为 UTC（如 `2026-09-08T06:51:01Z`），
// 而日志文件名 / 启动分隔线（open_daily_log）等均为本地时区 → 东八区日志行头
// 比文件名慢 8 小时（时区不匹配）。env_logger 默认格式不支持改时区，
// 因此经 `init_logger` 自定义 format 输出 `format_log_timestamp`。
//
// 时区来源：默认跟随系统本地时区；配置文件 settings.json 的 `logTimezone`
// 字段可显式指定（system / UTC / 固定偏移如 UTC+8、+08:00），由
// settings_store::init_settings（重启生效）与 save_settings（UI 保存即时生效）调用
// `apply_log_timezone` 应用。

/// 时区模式：跟随系统本地时区（默认）
const TZ_MODE_SYSTEM: u8 = 0;
/// 时区模式：固定 UTC
const TZ_MODE_UTC: u8 = 1;
/// 时区模式：固定偏移（偏移秒数存于 [`LOG_TZ_OFFSET_SECS`]）
const TZ_MODE_FIXED: u8 = 2;

/// 当前时区模式（`apply_log_timezone` 写入，format 闭包逐条读取）
static LOG_TZ_MODE: AtomicU8 = AtomicU8::new(TZ_MODE_SYSTEM);

/// 固定偏移模式的偏移秒数（东八区 = 8 * 3600）
static LOG_TZ_OFFSET_SECS: AtomicI32 = AtomicI32::new(0);

/// 最近一次应用的原始配置串（变更检测：`save_settings` 高频调用防刷屏）
static LOG_TZ_LAST_APPLIED: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

/// 解析固定偏移字符串为偏移秒数
///
/// 支持形式（`UTC` / `GMT` 前缀大小写不敏感）：`+8`、`+08:00`、`-05:30`、`UTC+8`、`GMT-5:30`。
/// 不支持 IANA 时区名（如 Asia/Shanghai，避免引入 chrono-tz 依赖）与秒级偏移。
///
/// # 返回
/// 偏移秒数（东区为正）；非偏移形式返回 `None`（由调用方回退 system）
fn parse_fixed_offset(s: &str) -> Option<i32> {
    let t = s.trim();
    // to_ascii_uppercase 逐字节转换，长度与原串一致 → 按长度回切原串安全
    let upper = t.to_ascii_uppercase();
    let t = if let Some(rest) = upper.strip_prefix("UTC") {
        &t[(upper.len() - rest.len())..]
    } else if let Some(rest) = upper.strip_prefix("GMT") {
        &t[(upper.len() - rest.len())..]
    } else {
        t
    };
    let t = t.trim();

    let (sign, rest) = match t.as_bytes().first()? {
        b'+' => (1, &t[1..]),
        b'-' => (-1, &t[1..]),
        _ => return None,
    };
    let mut parts = rest.split(':');
    let hours: i32 = parts.next()?.trim().parse().ok()?;
    let minutes: i32 = match parts.next() {
        Some(v) => v.trim().parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None; // ±HH:MM:SS 不支持
    }
    if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
        return None;
    }
    Some(sign * (hours * 3600 + minutes * 60))
}

/// 应用配置文件中的日志时区设置（settings.json 的 `logTimezone` 字段）
///
/// 支持值（不区分大小写）：
/// - `""` / `system` / `local`：系统本地时区（默认）
/// - `UTC`：固定 UTC
/// - 固定偏移：`UTC+8`、`+08:00`、`-05:30` 等（见 [`parse_fixed_offset`]）
///
/// 无法识别的值回退系统本地时区并告警；同一配置重复应用时静默跳过
/// （`save_settings` 每次设置保存都会调用，防刷屏）。
pub fn apply_log_timezone(config: &str) {
    let cfg = config.trim();
    let lower = cfg.to_ascii_lowercase();
    let (mode, offset_secs, desc, invalid) =
        if lower.is_empty() || lower == "system" || lower == "local" {
            (
                TZ_MODE_SYSTEM,
                0,
                "system（系统本地时区）".to_string(),
                None,
            )
        } else if lower == "utc" {
            (TZ_MODE_UTC, 0, "UTC".to_string(), None)
        } else if let Some(secs) = parse_fixed_offset(cfg) {
            let abs = secs.unsigned_abs();
            (
                TZ_MODE_FIXED,
                secs,
                format!("UTC{}{:02}:{:02}", if secs < 0 { '-' } else { '+' }, abs / 3600, (abs % 3600) / 60),
                None,
            )
        } else {
            (
                TZ_MODE_SYSTEM,
                0,
                "system（系统本地时区）".to_string(),
                Some(cfg.to_string()),
            )
        };

    // 变更检测：同一配置重复应用时静默跳过（防止设置保存刷屏）
    {
        let mut last = LOG_TZ_LAST_APPLIED.lock();
        if last.as_deref() == Some(cfg) {
            return;
        }
        *last = Some(cfg.to_string());
    }

    // 先写偏移再写模式（缩小 FIXED 模式读到旧偏移的窗口；单条日志受影响的概率可忽略）
    LOG_TZ_OFFSET_SECS.store(offset_secs, Ordering::Relaxed);
    LOG_TZ_MODE.store(mode, Ordering::Relaxed);

    match invalid {
        Some(bad) => log::warn!(
            "[file_logger] logTimezone 配置无法识别：{:?}，回退 system（支持：system / UTC / 固定偏移如 UTC+8、+08:00）",
            bad
        ),
        None => log::info!("[file_logger] 日志时区已应用：{}", desc),
    }
}

/// 获取当前时刻（按已应用的日志时区），日志行时间戳与日志文件名统一使用
///
/// 注意：`DualWriter::new` 在 logger 初始化时运行，早于 settings 加载，
/// 因此启动首个日志文件的文件名恒为系统本地日期（时区配置自次日起生效）。
///
/// # 返回
/// 带固定偏移的当前时刻（System 模式下偏移为系统本地当前偏移）
pub fn log_now() -> chrono::DateTime<chrono::FixedOffset> {
    match LOG_TZ_MODE.load(Ordering::Relaxed) {
        TZ_MODE_UTC => chrono::Utc::now().with_timezone(
            &chrono::FixedOffset::east_opt(0).expect("east_opt(0) 恒有效"),
        ),
        TZ_MODE_FIXED => {
            let secs = LOG_TZ_OFFSET_SECS.load(Ordering::Relaxed);
            let off = chrono::FixedOffset::east_opt(secs)
                .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).expect("east_opt(0) 恒有效"));
            chrono::Utc::now().with_timezone(&off)
        }
        _ => {
            let local = chrono::Local::now();
            local.with_timezone(local.offset())
        }
    }
}

/// 格式化当前日志时间戳：`YYYY-MM-DD HH:MM:SS.mmm±HH:MM`
///
/// 恒带偏移量后缀（如 `+08:00`），使任意时区配置下的日志均可无歧义对时。
pub fn format_log_timestamp() -> String {
    log_now().format("%Y-%m-%d %H:%M:%S%.3f%:z").to_string()
}

/// 初始化全局日志系统（env_logger → DualWriter 双输出，自定义时区时间戳）
///
/// 供 `lib.rs::run` 调用（debug=Debug / release=Info），替代 env_logger 默认格式：
/// 默认格式时间戳恒为 UTC（时区不匹配的根因），自定义格式输出
/// [`format_log_timestamp`]（默认本地时区，可经 settings.json `logTimezone` 覆盖），
/// 行样式与默认格式保持一致：`[<时间> <级别> <target>] <消息>`。
///
/// # Panics
/// 与 env_logger `init()` 一致：全局 logger 已被设置时 panic（进程内仅调用一次）。
pub fn init_logger(level: log::LevelFilter) {
    env_logger::Builder::new()
        .filter_level(level)
        .format(|buf, record| {
            writeln!(
                buf,
                "[{} {:<5} {}] {}",
                format_log_timestamp(),
                record.level(),
                record.target(),
                record.args()
            )
        })
        .target(env_logger::Target::Pipe(Box::new(DualWriter::new())))
        .init();
}

/// 双输出写入器：每日日志文件（同步优先）+ stderr（异步排水）
///
/// 作为 `env_logger::Target::Pipe` 的目标，使所有 `log::xxx!` 调用
/// 同时输出到文件和 stderr。`env_logger` 内部已通过 `Mutex` 序列化写入，
/// 因此 `DualWriter` 的 `Write` 实现无需额外同步原语。
///
/// # 字段
/// - `file`: 日志文件句柄，`None` 表示文件不可用（降级为仅 stderr）
/// - `file_err_notified`: 文件写入已失败并红字告警（避免每行都刷屏）
pub struct DualWriter {
    /// 日志文件句柄，None 表示文件不可用（降级为仅 stderr）
    file: Option<File>,
    /// 文件写入失败已红字告警过（置位后静默，防刷屏）
    file_err_notified: bool,
}

impl DualWriter {
    /// 创建双输出写入器
    ///
    /// 日志路径: `<data_local_dir>/nrmm-rust/logs/YYYY-MM-DD.log`
    ///
    /// # 降级策略
    /// 目录或文件创建失败时 `file = None`，仅输出到 stderr，不影响主流程
    ///
    /// # 返回
    /// 初始化完成的 `DualWriter` 实例（同时确保 stderr 排水线程已启动）
    pub fn new() -> Self {
        ensure_stderr_drain_started();
        let file = Self::open_daily_log();
        Self {
            file,
            file_err_notified: false,
        }
    }

    /// 打开当日日志文件
    ///
    /// 构建路径 `<data_local_dir>/nrmm-rust/logs/YYYY-MM-DD.log`，
    /// 创建目录（若不存在），以追加模式打开文件。
    ///
    /// # 返回
    /// - `Ok(file)`: 文件打开成功
    /// - `Err(())`: 路径获取、目录创建或文件打开失败
    fn open_daily_log() -> Option<File> {
        // 获取用户本地数据目录
        let data_dir = dirs::data_local_dir()?;
        let log_dir: PathBuf = data_dir.join("nrmm-rust").join("logs");

        // 创建日志目录（若不存在）
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            crate::platform::console::write_console_red(
                format!("[file_logger] 创建日志目录失败 {:?}: {}\n", log_dir, e).as_bytes(),
            );
            return None;
        }

        // 按日期命名日志文件（与行内时间戳同一时区来源：跟随 logTimezone 配置；
        // 启动首文件早于 settings 加载，恒为系统本地日期，见 log_now 文档）
        let date_str = log_now().format("%Y-%m-%d").to_string();
        let log_path = log_dir.join(format!("{}.log", date_str));

        // 以追加模式打开文件
        match OpenOptions::new().create(true).append(true).open(&log_path) {
            Ok(mut f) => {
                // 写入分隔标记，标识新的一次启动
                let _ = writeln!(
                    f,
                    "\n--- NRMM 启动 {} ---",
                    log_now().format("%Y-%m-%d %H:%M:%S")
                );
                Some(f)
            }
            Err(e) => {
                crate::platform::console::write_console_red(
                    format!("[file_logger] 打开日志文件失败 {:?}: {}\n", log_path, e).as_bytes(),
                );
                None
            }
        }
    }
}

impl Default for DualWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for DualWriter {
    /// 将数据写入每日日志文件（同步优先）并投递 stderr 排水通道（异步）
    ///
    /// 写入顺序与容错：
    /// 1. **文件优先**：同步写文件，失败忽略（落盘不依赖 stderr 状态）
    /// 2. **stderr 异步投递**：非阻塞 `try_send` 到排水通道，满则丢弃、
    ///    断开则降级直写（见 `dispatch_stderr`）
    /// 3. 始终返回 `Ok(buf.len())`，任何 stderr/文件失败都不向
    ///    `env_logger` 传播错误（避免日志管道中断）
    ///
    /// # 参数
    /// - `buf`: 待写入的字节缓冲区
    ///
    /// # 返回
    /// - `Ok(len)`: 恒为 `buf.len()`（写入语义按全部成功处理）
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // 先写文件（失败红字告警一次：磁盘异常需醒目可见，之后降级仅 stderr 防刷屏）
        if let Some(file) = self.file.as_mut() {
            if let Err(e) = file.write_all(buf) {
                if !self.file_err_notified {
                    self.file_err_notified = true;
                    crate::platform::console::write_console_red(
                        format!("[file_logger] 日志文件写入失败（降级仅终端）: {}\n", e).as_bytes(),
                    );
                }
            }
        }

        // 再异步投递 stderr（不阻塞、失败静默）
        dispatch_stderr(buf);

        Ok(buf.len())
    }

    /// 刷新日志文件缓冲区
    ///
    /// stderr 不再同步 flush：排水线程写后自行处理，调用方无需等待。
    ///
    /// # 返回
    /// - `Ok(())`: 恒成功（文件 flush 失败忽略）
    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 DualWriter 文件优先写入：内容必须落盘且 write 恒返回 Ok(len)
    #[test]
    fn test_dual_writer_file_priority() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("test.log");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap();

        let mut writer = DualWriter {
            file: Some(file),
            file_err_notified: false,
        };
        let msg = b"[TEST] file priority message\n";
        let result = writer.write(msg);
        assert_eq!(result.unwrap(), msg.len());
        writer.flush().unwrap();

        // 文件内容包含写入的消息（文件优先落盘）
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("[TEST] file priority message"));
    }

    /// 测试 stderr 投递失败不影响 write 返回 Ok(buf.len())
    ///
    /// file = None 且强制走降级直写路径，验证无论 stderr 路径状态如何，
    /// write 恒返回 Ok(len)（stderr 失败永不向 env_logger 传播错误）。
    #[test]
    fn test_dual_writer_stderr_failure_returns_ok() {
        // 场景 1：降级标记置位（排水线程不可用的兜底路径）
        let prev = DRAIN_DEGRADED.swap(true, Ordering::Relaxed);
        let mut writer = DualWriter {
            file: None,
            file_err_notified: false,
        };
        let msg = b"[TEST] degraded stderr path\n";
        assert_eq!(writer.write(msg).unwrap(), msg.len());
        DRAIN_DEGRADED.store(prev, Ordering::Relaxed);

        // 场景 2：通道正常路径（若其他测试已 ensure 则为已初始化状态）
        ensure_stderr_drain_started();
        let msg2 = b"[TEST] normal channel path\n";
        assert_eq!(writer.write(msg2).unwrap(), msg2.len());
    }

    /// 测试 DualWriter 在无文件时仍可写入
    #[test]
    fn test_dual_writer_with_none_file() {
        let mut writer = DualWriter {
            file: None,
            file_err_notified: false,
        };
        let result = writer.write_all(b"test message\n");
        assert!(result.is_ok());
        let result = writer.flush();
        assert!(result.is_ok());
    }

    /// 测试 DualWriter::new() 不 panic
    ///
    /// 即使日志目录创建失败，DualWriter 也应正常构造（file=None）
    #[test]
    fn test_dual_writer_new_does_not_panic() {
        let writer = DualWriter::new();
        // 不断言 file 是否 Some，因为取决于运行环境
        let _ = writer;
    }

    /// 测试 DualWriter 实现了 Write + Send
    ///
    /// env_logger 的 Target::Pipe 要求 Box<dyn Write + Send>
    #[test]
    fn test_dual_writer_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<DualWriter>();
    }

    /// 测试 ensure_stderr_drain_started 幂等：多次调用不重复建线程、不 panic
    #[test]
    fn test_ensure_stderr_drain_started_idempotent() {
        ensure_stderr_drain_started();
        ensure_stderr_drain_started();
        ensure_stderr_drain_started();

        // 通道已创建
        assert!(STDERR_TX.get().is_some());
        // 线程启动流程已执行（无论成功与否仅一次）
        assert!(DRAIN_THREAD_SPAWNED.load(Ordering::SeqCst));
        // 接收端已被排水线程取走（证明线程启动成功且进入消费循环）
        let rx_taken = STDERR_RX
            .get()
            .and_then(|m| m.lock().ok().map(|g| g.is_none()))
            .unwrap_or(false);
        assert!(rx_taken, "排水线程应已取走接收端并进入消费循环");
        // 启动成功则未降级
        if rx_taken {
            assert!(!DRAIN_DEGRADED.load(Ordering::Relaxed));
        }
    }

    /// 测试排水通道投递不阻塞：通道正常状态下 try_send 立即返回
    #[test]
    fn test_drain_channel_try_send_nonblocking() {
        ensure_stderr_drain_started();
        let tx = STDERR_TX.get().expect("通道应已初始化");
        // 投递一条测试日志（排水线程异步消费，try_send 恒非阻塞）
        let result = tx.try_send(Arc::from(b"[TEST] drain alive probe\n".as_slice()));
        // 通道可能已满（极端情况），但断言不应断开
        assert!(
            !matches!(result, Err(TrySendError::Disconnected(_))),
            "排水通道不应断开"
        );
    }

    // ===== 日志时区测试 =====

    /// 时区全局状态的测试互斥锁：触碰 LOG_TZ_* 的测试必须持锁串行执行，
    /// 避免并行测试互相覆写全局时区模式
    static TZ_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    /// parse_fixed_offset 变体解析回归（纯函数，不触碰全局状态）
    #[test]
    fn test_parse_fixed_offset_variants() {
        assert_eq!(parse_fixed_offset("+8"), Some(8 * 3600));
        assert_eq!(parse_fixed_offset("+08:00"), Some(8 * 3600));
        assert_eq!(parse_fixed_offset("UTC+8"), Some(8 * 3600));
        assert_eq!(parse_fixed_offset("utc+9:30"), Some(9 * 3600 + 30 * 60));
        assert_eq!(parse_fixed_offset("GMT-5:30"), Some(-5 * 3600 - 30 * 60));
        assert_eq!(parse_fixed_offset("+09:30"), Some(9 * 3600 + 30 * 60));
        assert_eq!(parse_fixed_offset("-0"), Some(0));
        // 非法形式 → None（由 apply_log_timezone 回退 system）
        assert_eq!(parse_fixed_offset("Asia/Shanghai"), None);
        assert_eq!(parse_fixed_offset("25:00"), None);
        assert_eq!(parse_fixed_offset("+08:00:00"), None);
        assert_eq!(parse_fixed_offset("abc"), None);
        assert_eq!(parse_fixed_offset(""), None);
    }

    /// apply_log_timezone 应用与 format_log_timestamp 后缀回归：
    /// UTC / 固定偏移模式的时间戳必须带对应偏移后缀；
    /// 非法值回退 system（偏移后缀 = 本地时区当前偏移）
    #[test]
    fn test_apply_log_timezone_and_format_suffix() {
        let _guard = TZ_TEST_LOCK.lock();

        // UTC 模式
        apply_log_timezone("UTC");
        assert!(
            format_log_timestamp().ends_with("+00:00"),
            "UTC 模式时间戳应以 +00:00 结尾: {}",
            format_log_timestamp()
        );

        // 固定偏移模式（UTC 前缀 + 大小写混合 + 分钟偏移）
        apply_log_timezone("utc+9:30");
        assert!(
            format_log_timestamp().ends_with("+09:30"),
            "UTC+9:30 模式时间戳应以 +09:30 结尾: {}",
            format_log_timestamp()
        );

        // 非法值回退 system：偏移后缀应等于本地时区当前偏移（偏移在测试期间恒定，跨秒比较安全）
        apply_log_timezone("definitely-not-a-zone");
        let local_suffix = chrono::Local::now().format("%:z").to_string();
        assert!(
            format_log_timestamp().ends_with(&local_suffix),
            "回退 system 后偏移应与本地时区一致: {} vs {}",
            format_log_timestamp(),
            local_suffix
        );

        // 恢复默认 system（避免影响其它测试对全局状态的假设）
        apply_log_timezone("system");
    }
}
