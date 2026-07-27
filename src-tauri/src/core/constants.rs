pub const MANAGED_FOLDER: &str = "_MANAGED_";
pub const KEYPRESS_FILENAME: &str = "nrmm_keypress.txt";
pub const INCLUDE_FILENAME: &str = "nrmm_include.ini";
pub const DISABLED_PREFIX: &str = "DISABLED_";
pub const FAV_MARKER: &str = "fav";
pub const NAMESPACED_MARKER: &str = "modnamespaced";
pub const MODFORCED_MARKER: &str = "modforced";
pub const SELECTED_INDEX_FILE: &str = "selectedindex";
pub const BACKUP_EXTENSION: &str = "ini_managed_backup";
pub const DISABLED_BY_NRMM: &str = "DISABLED_BY_NRMM";
pub const NOMOD_INDEX: i32 = -1;
pub const MOD_GROUP_FILE_PREFIX: &str = "group_";
pub const SLOT_VAR_PREFIX: &str = "$modmanageragl";
pub const MANAGED_SLOT_ID_VAR: &str = "$managed_slot_id";
pub const ACTIVE_GROUP_VAR: &str = "$active_group_id";
pub const ACTIVE_SLOT_VAR: &str = "$active_slot";
pub const GROUP_ID_VAR: &str = "$group_id";
pub const DISABLE_CRC_FIX_FILE: &str = "DisableCRC32Fix.ini";
pub const INI_EXTENSION: &str = ".ini";

#[cfg(target_os = "windows")]
pub const SEVEN_Z_EXECUTABLE: &str = "7z.exe";
#[cfg(target_os = "linux")]
pub const SEVEN_Z_EXECUTABLE: &str = "7zz";
#[cfg(target_os = "macos")]
pub const SEVEN_Z_EXECUTABLE: &str = "7zz";

pub const TEMP_EXTRACT_DIR: &str = "nrmm_extract";

pub const ICON_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif"];

pub const ICON_NAME_PRIORITY: &[&str] = &[
    "icon.png", "icon.jpg", "icon.jpeg", "icon.webp", "icon.bmp", "icon.gif",
    "preview.png", "preview.jpg", "preview.jpeg", ".jasm_cover.png", "cover.png", "cover.jpg"
];

pub const CONDITIONAL_SECTION_PREFIXES: &[&str] = &[
    "Key", "KeyPress", "TextureOverride", "ShaderOverride", "CommandList"
];

pub const NON_CONDITIONAL_SECTIONS: &[&str] = &[
    "Present", "CustomShader", "String", "Constants"
];

pub const OVERRIDE_KEYS: &[&str] = &[
    "vb0", "vb1", "ib", "ps-t0", "ps-t1", "ps-t2", "ps-t3", "ps-t4", "ps-t5", "ps-t6", "ps-t7",
    "vs-t0", "vs-t1", "vs-t2", "vs-t3", "vs-t4", "vs-t5", "vs-t6", "vs-t7",
    "ps-", "vs-", "cs-",
    "o0", "o1", "o2", "o3", "o4", "o5", "o6", "o7",
    "u0", "u1", "u2", "u3", "u4", "u5", "u6", "u7"
];

pub const CRASH_LINE_PATTERNS: &[&str] = &[
    "drawindexed", "drawindexed = auto", "draw = ", "ib = ", "vb0 = "
];

pub const MAX_PATH: usize = 260;
pub const FILE_WATCHER_DEBOUNCE_MS: u64 = 500;
pub const FOREGROUND_POLL_INTERVAL_MS: u64 = 1000;
