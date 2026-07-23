//! 全局应用状态容器模块。
//!
//! 定义 [`AppState`]，作为 Tauri 应用在整个生命周期内共享的全局状态。
//! 通过 `tauri::Builder::manage` 注入后，所有命令均可通过
//! `app.state::<AppState>()` 获取只读引用（内部使用锁实现可变性）。
//!
//! ## 并发安全策略
//! 所有字段均使用 [`std::sync::Arc`] 包裹以实现共享所有权，内部可变性分别由：
//! - [`parking_lot::RwLock`]：适合读多写少的字段（如 `settings`、`cloud_data`）；
//! - [`parking_lot::Mutex`]：适合读写均衡或需要独占访问的字段（如 `file_watcher`）；
//! - 字段自身的内部同步（如 `Arc<ModManager>`、`Arc<TaskQueue>` 等无锁结构）。
//!
//! 使用 `parking_lot` 而非 `std::sync` 的原因：性能更优、不会中毒、API 更友好。

use std::sync::Arc;

use parking_lot::Mutex;
use parking_lot::RwLock;

use crate::mod_manager::ModManager;
use crate::ini_handler::IniHandler;
use crate::file_watcher::FileWatcher;
use crate::hotkey::HotkeyManager;
use crate::window_manager::WindowManager;
use crate::tray::TrayManager;
use crate::settings::Settings;
use crate::cloud_data::CloudData;
use crate::keypress_simulator::KeypressSimulator;
use crate::task_queue::TaskQueue;

/// 应用全局状态容器。
///
/// 持有各子系统的单例句柄，通过 `Arc` + 锁实现跨线程安全共享。
/// 在 `lib.rs::run()` 中构造并通过 `tauri::Builder::manage` 注入。
///
/// # 字段说明
/// 各字段对应一个子系统，详见字段级注释。
pub struct AppState {
    /// Mod 管理器：负责 Mod 列表加载、分组、收藏等业务。
    /// 内部自行管理可变性，因此外层仅用 `Arc` 包裹（无锁）。
    pub mod_manager: Arc<ModManager>,

    /// INI 文件处理器：负责 INI 读写与语法检查。
    /// 内部自行管理可变性，外层仅用 `Arc` 包裹。
    pub ini_handler: Arc<IniHandler>,

    /// 文件系统监听器：监听 Mods 目录变更。
    /// 启动/停止等操作需要独占访问，使用 `Mutex` 保护。
    pub file_watcher: Arc<Mutex<FileWatcher>>,

    /// 热键管理器：负责全局快捷键的注册/注销。
    /// 当前实现为无状态结构（仅承载方法），标记 `dead_code` 是因为
    /// 实际调用通过类方法而非实例字段进行。
    #[allow(dead_code)]
    pub hotkey_manager: Arc<HotkeyManager>,

    /// 窗口管理器：封装主窗口的显示/隐藏/尺寸/置顶等操作。
    /// 同样为无状态结构，标记 `dead_code` 原因同上。
    #[allow(dead_code)]
    pub window_manager: Arc<WindowManager>,

    /// 托盘管理器：封装系统托盘菜单与图标事件处理。
    /// 同样为无状态结构，标记 `dead_code` 原因同上。
    #[allow(dead_code)]
    pub tray_manager: Arc<TrayManager>,

    /// 用户设置：读多写少，使用 `RwLock` 允许多读单写。
    /// 设置变更后会异步落盘到 `settings.json`。
    pub settings: Arc<RwLock<Settings>>,

    /// 云端数据缓存：读多写少，使用 `RwLock` 保护。
    pub cloud_data: Arc<RwLock<CloudData>>,

    /// 按键模拟器：用于在游戏内模拟键盘/鼠标输入。
    /// 内部自行管理可变性，外层仅用 `Arc` 包裹。
    pub keypress_simulator: Arc<KeypressSimulator>,

    /// 后台任务队列：串行处理需要排队的异步任务（避免并发冲突）。
    /// 内部自行管理可变性，外层仅用 `Arc` 包裹。
    pub task_queue: Arc<TaskQueue>,
}

impl AppState {
    /// 构造一个全新的 `AppState`，所有子系统均使用各自的默认初始化。
    ///
    /// # 参数
    /// 无。
    ///
    /// # 返回值
    /// 返回填充完毕的 [`AppState`] 实例。
    ///
    /// # 业务逻辑
    /// 在应用启动时被 `lib.rs::run()` 调用一次，构造后立即注入 Tauri 状态管理。
    /// 此处不加载任何持久化数据，设置/窗口状态等会在 `setup` 回调中单独加载。
    pub fn new() -> Self {
        Self {
            mod_manager: Arc::new(ModManager::new()),
            ini_handler: Arc::new(IniHandler::new()),
            file_watcher: Arc::new(Mutex::new(FileWatcher::new())),
            hotkey_manager: Arc::new(HotkeyManager::new()),
            window_manager: Arc::new(WindowManager::new()),
            tray_manager: Arc::new(TrayManager::new()),
            settings: Arc::new(RwLock::new(Settings::new())),
            cloud_data: Arc::new(RwLock::new(CloudData::new())),
            keypress_simulator: Arc::new(KeypressSimulator::new()),
            task_queue: Arc::new(TaskQueue::new()),
        }
    }
}

impl Default for AppState {
    /// 默认实现等价于 [`AppState::new`]，便于在测试或派生场景下使用。
    fn default() -> Self {
        Self::new()
    }
}
