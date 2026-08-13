//! 内置资源文件模块
//!
//! 使用 include_bytes! 宏在编译时将资源文件嵌入到二进制中：
//! - TEMPLATE_GROUP/TEMPLATE_MANAGER_GROUP: 3Dmigoto INI 模板
//! - LISTEN_KEYPRESS_*: 按键监听相关配置
//! - LINKS_JSON/MESSAGES_JSON: 云端链接和消息的默认数据
//! - AUTO_ICONS_JSON: 自动图标配置
//! - KNOWN_LIBRARIES_JSON: 已知库列表（用于冲突检测）

/// 普通分组 INI 模板
pub const TEMPLATE_GROUP: &[u8] = include_bytes!("template_group.txt");
/// 管理分组 INI 模板
pub const TEMPLATE_MANAGER_GROUP: &[u8] = include_bytes!("template_manager_group.txt");
/// 前台按键监听配置
pub const LISTEN_KEYPRESS_MANAGER: &[u8] = include_bytes!("listen_keypress_manager.txt");
/// 支持额外前台窗口的按键监听配置（DLL 支持 additional_foreground_window）
pub const LISTEN_KEYPRESS_ADDITIONAL_WINDOW: &[u8] = include_bytes!("listen_keypress_additional_window.txt");
/// 后台按键监听配置
pub const LISTEN_KEYPRESS_EVEN_ON_BACKGROUND: &[u8] = include_bytes!("listen_keypress_even_on_background.txt");

/// 默认云端链接数据
pub const LINKS_JSON: &[u8] = include_bytes!("data/links.json");
/// 默认云端消息数据
pub const MESSAGES_JSON: &[u8] = include_bytes!("data/messages.json");
/// 自动图标配置数据
pub const AUTO_ICONS_JSON: &[u8] = include_bytes!("data/auto_icons.json");
/// 已知库列表（用于重复定义检测）
pub const KNOWN_LIBRARIES_JSON: &[u8] = include_bytes!("data/known_libraries.json");
