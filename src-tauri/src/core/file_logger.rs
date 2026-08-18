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
use std::sync::atomic::{AtomicBool, Ordering};
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

/// 双输出写入器：每日日志文件（同步优先）+ stderr（异步排水）
///
/// 作为 `env_logger::Target::Pipe` 的目标，使所有 `log::xxx!` 调用
/// 同时输出到文件和 stderr。`env_logger` 内部已通过 `Mutex` 序列化写入，
/// 因此 `DualWriter` 的 `Write` 实现无需额外同步原语。
///
/// # 字段
/// - `file`: 日志文件句柄，`None` 表示文件不可用（降级为仅 stderr）
pub struct DualWriter {
    /// 日志文件句柄，None 表示文件不可用（降级为仅 stderr）
    file: Option<File>,
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
        Self { file }
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
            eprintln!(
                "[file_logger] Failed to create log dir {:?}: {}",
                log_dir, e
            );
            return None;
        }

        // 按日期命名日志文件
        let date_str = chrono::Local::now().format("%Y-%m-%d").to_string();
        let log_path = log_dir.join(format!("{}.log", date_str));

        // 以追加模式打开文件
        match OpenOptions::new().create(true).append(true).open(&log_path) {
            Ok(mut f) => {
                // 写入分隔标记，标识新的一次启动
                let _ = writeln!(
                    f,
                    "\n--- NRMM 启动 {} ---",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
                );
                Some(f)
            }
            Err(e) => {
                eprintln!(
                    "[file_logger] Failed to open log file {:?}: {}",
                    log_path, e
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
        // 先写文件（失败忽略：文件落盘失败时至少 stderr 排水仍可送达）
        if let Some(file) = self.file.as_mut() {
            let _ = file.write_all(buf);
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

        let mut writer = DualWriter { file: Some(file) };
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
        let mut writer = DualWriter { file: None };
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
        let mut writer = DualWriter { file: None };
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
}
