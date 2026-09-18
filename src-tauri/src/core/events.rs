//! 前后端 IPC 事件契约（唯一登记处）
//!
//! 所有 `emit` 调用必须引用本模块常量，禁止裸字符串
//! （由 `tests/arch_gate.rs` F5 门禁全文扫描强制）。
//! 前端镜像：`src/events.ts` 的 `IpcEvent` 对象；
//! 两侧集合相等性由 F5 集合校验测试保证（新增事件须双侧同步登记）。

/// 目标游戏切换（payload: TargetGame 字符串；发射点：settings_commands / tray）
pub const TARGET_GAME_SWITCHED: &str = "target-game-switched";
/// 主窗口已显示
pub const WINDOW_SHOWN: &str = "window-shown";
/// 主窗口已隐藏
pub const WINDOW_HIDDEN: &str = "window-hidden";
/// 主窗口隐藏且需清理数据
pub const WINDOW_HIDDEN_CLEAR_DATA: &str = "window-hidden-clear-data";
/// 显示主窗口并切换到检测到的游戏（payload: TargetGame）
pub const WINDOW_SHOW_WITH_GAME: &str = "window-show-with-game";
/// 热键触发模组列表刷新
pub const HOTKEY_REFRESH: &str = "hotkey-refresh";
/// `_MANAGED_` 目录发生变化（全量刷新兜底信号）
pub const MANAGED_FOLDER_CHANGED: &str = "managed-folder-changed";
/// `_MANAGED_` 目录增量更新（payload: GroupDelta 批次）
pub const MANAGED_PARTIAL_UPDATE: &str = "managed-partial-update";
/// 游戏外快捷键触发：显示游戏选择菜单（payload: { x, y, action }）
pub const SHOW_GAME_SELECT: &str = "show-game-select";

/// 已登记事件全集（F5 集合校验基准；新增事件须同步登记于此与 src/events.ts）
pub const ALL: &[&str] = &[
    TARGET_GAME_SWITCHED,
    WINDOW_SHOWN,
    WINDOW_HIDDEN,
    WINDOW_HIDDEN_CLEAR_DATA,
    WINDOW_SHOW_WITH_GAME,
    HOTKEY_REFRESH,
    MANAGED_FOLDER_CHANGED,
    MANAGED_PARTIAL_UPDATE,
    SHOW_GAME_SELECT,
];
