//! 全局常量定义模块
//!
//! 定义整个应用中使用的常量值，包括：
//! - 特殊目录和文件名前缀/后缀
//! - 标记文件名（收藏、强制启用、命名空间等）
//! - INI 相关常量（扩展名、备份后缀、条件段前缀等）
//! - 图标文件扩展名和优先级
//! - 文件监控防抖时间
//! - 平台相关的 7z 可执行文件名

/// NRMM 管理的模组根目录名，所有模组都存放在此目录下
pub const MANAGED_FOLDER: &str = "_MANAGED_";

/// 按键监听文件名（用于游戏内按键检测）
pub const KEYPRESS_FILENAME: &str = "nrmm_keypress.txt";

/// NRMM 自动生成的 include INI 文件名
pub const INCLUDE_FILENAME: &str = "nrmm_include.ini";

/// 禁用模组的目录名前缀（重命名目录实现启用/禁用）
pub const DISABLED_PREFIX: &str = "DISABLED_";

/// 收藏标记文件名：存在此空文件表示模组被收藏
pub const FAV_MARKER: &str = "fav";

/// 命名空间标记文件名：存在表示模组使用了 namespace
pub const NAMESPACED_MARKER: &str = "modnamespaced";

/// 强制启用标记文件名：存在表示模组强制启用（跳过崩溃行检查）
pub const MODFORCED_MARKER: &str = "modforced";

/// 选中模组索引标记文件名：存储分组内当前选中的模组索引
pub const SELECTED_INDEX_FILE: &str = "selectedindex";

/// INI 文件备份扩展名：update_mod_data 前自动备份原文件
pub const BACKUP_EXTENSION: &str = "ini_managed_backup";

/// NRMM 在 INI 中添加的禁用标记注释
pub const DISABLED_BY_NRMM: &str = "DISABLED_BY_NRMM";

/// "无模组" 空槽位的索引值（始终为 -1）
pub const NOMOD_INDEX: i32 = -1;

/// 普通分组目录前缀：group_1, group_2, ... 等
pub const MOD_GROUP_FILE_PREFIX: &str = "group_";

/// 模组管理器槽位变量前缀（注入到 d3dx.ini 中）
pub const SLOT_VAR_PREFIX: &str = "$modmanageragl";

/// 当前选中槽位 ID 变量名（注入到 d3dx.ini）
pub const MANAGED_SLOT_ID_VAR: &str = "$managed_slot_id";

/// 当前激活分组 ID 变量名
pub const ACTIVE_GROUP_VAR: &str = "$active_group_id";

/// 当前激活槽位变量名
pub const ACTIVE_SLOT_VAR: &str = "$active_slot";

/// 分组 ID 变量名
pub const GROUP_ID_VAR: &str = "$group_id";

/// 禁用 CRC32 修复标记文件名
pub const DISABLE_CRC_FIX_FILE: &str = "DisableCRC32Fix.ini";

/// INI 文件扩展名
pub const INI_EXTENSION: &str = ".ini";

/// Windows 平台 7-Zip 可执行文件名
#[cfg(target_os = "windows")]
pub const SEVEN_Z_EXECUTABLE: &str = "7z.exe";

/// Linux 平台 7-Zip 可执行文件名
#[cfg(target_os = "linux")]
pub const SEVEN_Z_EXECUTABLE: &str = "7zz";

/// macOS 平台 7-Zip 可执行文件名
#[cfg(target_os = "macos")]
pub const SEVEN_Z_EXECUTABLE: &str = "7zz";

/// 临时解压目录名（在系统临时目录下）
pub const TEMP_EXTRACT_DIR: &str = "nrmm_extract";

/// 支持的图标文件扩展名列表
pub const ICON_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif"];

/// 图标文件名优先级列表：按顺序查找，第一个找到的作为模组图标
/// 优先级：icon.* > preview.* > .jasm_cover.png > cover.*
pub const ICON_NAME_PRIORITY: &[&str] = &[
    "icon.png", "icon.jpg", "icon.jpeg", "icon.webp", "icon.bmp", "icon.gif",
    "preview.png", "preview.jpg", "preview.jpeg", ".jasm_cover.png", "cover.png", "cover.jpg"
];

/// 3Dmigoto INI 条件段前缀列表（这些段需要注入槽位条件）
/// Key: 按键触发段
/// KeyPress: 按键按下段
/// TextureOverride: 纹理覆盖段
/// ShaderOverride: 着色器覆盖段
/// CommandList: 命令列表段
pub const CONDITIONAL_SECTION_PREFIXES: &[&str] = &[
    "Key", "KeyPress", "TextureOverride", "ShaderOverride", "CommandList"
];

/// 非条件段列表（这些段不需要注入槽位条件）
/// Present: 呈现段
/// CustomShader: 自定义着色器
/// String: 字符串常量
/// Constants: 数值常量
pub const NON_CONDITIONAL_SECTIONS: &[&str] = &[
    "Present", "CustomShader", "String", "Constants"
];

/// 3Dmigoto 覆盖资源键名列表（用于检测可能的崩溃行）
/// vb/vib/ib: 顶点/索引缓冲区
/// ps-t/vs-t/cs-t: 纹理采样器（像素/顶点/计算着色器）
/// ps-/vs-/cs-: 着色器资源
/// o0-o7: 渲染目标
/// u0-u7: 无序访问视图
pub const OVERRIDE_KEYS: &[&str] = &[
    "vb0", "vb1", "ib", "ps-t0", "ps-t1", "ps-t2", "ps-t3", "ps-t4", "ps-t5", "ps-t6", "ps-t7",
    "vs-t0", "vs-t1", "vs-t2", "vs-t3", "vs-t4", "vs-t5", "vs-t6", "vs-t7",
    "ps-", "vs-", "cs-",
    "o0", "o1", "o2", "o3", "o4", "o5", "o6", "o7",
    "u0", "u1", "u2", "u3", "u4", "u5", "u6", "u7"
];

/// 可能导致游戏崩溃的行模式（这些行会被自动注释掉）
pub const CRASH_LINE_PATTERNS: &[&str] = &[
    "drawindexed", "drawindexed = auto", "draw = ", "ib = ", "vb0 = "
];

/// Windows 最大路径长度限制（用于路径过长检测）
pub const MAX_PATH: usize = 260;

/// 文件监控防抖时间（毫秒）：文件变化后等待此时间再触发刷新，避免频繁事件
pub const FILE_WATCHER_DEBOUNCE_MS: u64 = 500;

/// 前台窗口轮询间隔（毫秒）：检测游戏是否在前台
pub const FOREGROUND_POLL_INTERVAL_MS: u64 = 1000;
