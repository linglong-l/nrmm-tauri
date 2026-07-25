// 全局常量与展示用映射表集合。
// 本文件集中维护窗口尺寸、文件命名约定、默认设置、云端 URL 以及各类枚举到可读名称的映射。
import { TargetGame, HotkeyKeyboard, HotkeyGamepad, LayoutMode, SortGroupMethod, ModsPathStatus, type LanguageOption } from '../types';

/**
 * 应用全局常量集合。
 * 包含窗口默认尺寸、文件名约定、图像缓存宽度、默认设置以及云端资源 URL 等内容。
 */
export const CONSTANTS = {
  /** 窗口最小宽度（像素），低于该值时禁止缩放 */
  minWindowWidth: 750,
  /** 窗口最小高度（像素），低于该值时禁止缩放 */
  minWindowHeight: 412,
  /** 窗口默认宽度（像素） */
  defaultWindowWidth: 800,
  /** 窗口默认高度（像素） */
  defaultWindowHeight: 600,
  /** Managed 文件夹名称，所有受管理的 Mod 数据均存放于此 */
  managedFolderName: '_MANAGED_',
  /** Managed 文件夹被备份时的扩展名 */
  managedBackupExtension: 'ini_managed_backup',
  /** 已被移除（禁用）的 Managed Mod 存放目录名 */
  managedRemovedFolderName: 'DISABLED_MANAGED_REMOVED',
  /** 旧版本（1.3.x）使用的 Managed 目录名，仅用于兼容迁移 */
  oldManagedFolderName: 'V1_3_x_MANAGED-DO_NOT_EDIT_COPY_MOVE_CUT',
  /** 更早期版本使用的 Managed 目录名，仅用于兼容迁移 */
  anotherOldManagedFolderName: 'MANAGED-DO_NOT_EDIT_COPY_MOVE_CUT',
  /** "无" 槽位对应的图标文件名 */
  noneSlotIconFileName: 'icon_none.png',
  /**
   * Mod 图标候选文件名列表（按优先级顺序匹配）。
   * 同时支持英文与中文（预览）命名，覆盖 png/jpg/jpeg/webp 多种格式。
   */
  modIconFilenames: [
    'icon.png', 'preview.png', '预览.png', '.jasm_cover.png', 'cover.png',
    'preview.jpg', '预览.jpg', '.jasm_cover.jpg', 'cover.jpg',
    'preview.jpeg', '预览.jpeg', '.jasm_cover.jpeg', 'cover.jpeg',
    'preview.webp', '预览.webp', '.jasm_cover.webp', 'cover.webp'
  ],
  /** keybind 模式下用于触发按键模拟的临时文件名 */
  nrmmKeypressFileName: 'nrmm_keypress.txt',
  /** NRMM 引用 INI 文件名（用于将 Mod 注入到游戏框架） */
  nrmmIncluderFileName: 'nrmm_include.ini',
  /** 分组管理元数据文件名（记录分组与 Mod 的关系） */
  managerGroupFileName: 'manager_group.ini',
  /** 本程序自身的进程名（用于自检，避免重复启动等） */
  thisProcessName: 'nrmm-rust.exe',
  /** 资源管理器视图下图像缓存宽度（像素） */
  explorerViewImageCacheWidth: 192,
  /** 分组图标缓存宽度（像素） */
  groupImageCacheWidth: 192,
  /** Mod 图标缓存宽度（像素） */
  modImageCacheWidth: 320,
  /** 单个 Managed 目录下允许的最大分组数量 */
  maxGroupCount: 500,
  /** 默认键盘热键 */
  defaultHotkeyKeyboard: HotkeyKeyboard.altW,
  /** 默认手柄热键（none 表示不启用） */
  defaultHotkeyGamepad: HotkeyGamepad.none,
  /** 默认整体缩放比例 */
  defaultOverallScale: 1.0,
  /** 默认背景透明度（0-1） */
  defaultBgTransparency: 0.85,
  /** 默认布局模式 */
  defaultLayoutMode: LayoutMode.Auto,
  /** 默认界面语言代码 */
  defaultLanguage: 'en',
  /** 默认主题名称 */
  defaultTheme: 'dark',
  /** 默认是否自动生成分组文件夹图标 */
  defaultAutoGenerateFolderIcon: true,
  /** 默认不自动置顶窗口（普通优先级） */
  defaultAutoPinWindow: false,
  /** 默认游戏外切换时是否显示菜单 */
  defaultShowMenuWhenTogglingOutsideGame: false,
  /** 默认是否通过模拟按键方式触发 keybind */
  defaultKeybindSimulateKeypress: false,
  /** 默认分组排序方式 */
  defaultSortGroupMethod: SortGroupMethod.ByIndex,
  /** 各游戏默认目标进程名映射 */
  defaultTargetProcesses: {
    /** 鸣潮默认目标进程名 */
    wuwa: 'Wuthering Waves.exe',
    /** 原神默认目标进程名 */
    genshin: 'GenshinImpact.exe',
    /** 崩坏：星穹铁道默认目标进程名 */
    hsr: 'StarRail.exe',
    /** 绝区零默认目标进程名 */
    zzz: 'ZenlessZoneZero.exe',
    /** 明日方舟：终末地默认目标进程名 */
    endfield: 'Endfield-Win64-Shipping.exe'
  },
  /**
   * 云端资源 URL 集合。
   * 包含支持/教程/联系入口的图标与链接、各游戏公告、自动图标 JSON、已知模组库等远端地址。
   */
  cloudDataUrls: {
    /** 支持入口常态图标 URL */
    supportIcon: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/icon_support.png',
    /** 支持入口悬停图标 URL */
    supportIconOnHover: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/icon_support_onhover.png',
    /** 教程入口常态图标 URL */
    tutorialIcon: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/icon_tutorial.png',
    /** 教程入口悬停图标 URL */
    tutorialIconOnHover: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/icon_tutorial_onhover.png',
    /** 联系入口常态图标 URL */
    contactIcon: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/icon_contact.png',
    /** 联系入口悬停图标 URL */
    contactIconOnHover: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/icon_contact_onhover.png',
    /** 支持入口跳转链接（远程 txt 文本） */
    supportLink: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/link_support.txt',
    /** 教程入口跳转链接（远程 txt 文本） */
    tutorialLink: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/link_tutorial.txt',
    /** 联系入口跳转链接（远程 txt 文本） */
    contactLink: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/link_contact.txt',
    /** 鸣潮公告文本 URL */
    messageWuwa: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/message_wuwa.txt',
    /** 原神公告文本 URL */
    messageGenshin: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/message_genshin.txt',
    /** 崩坏：星穹铁道公告文本 URL */
    messageHsr: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/message_hsr.txt',
    /** 绝区零公告文本 URL */
    messageZzz: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/message_zzz.txt',
    /** 明日方舟：终末地公告文本 URL */
    messageEndfield: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/message_endfield.txt',
    /** 鸣潮自动图标 JSON URL */
    autoIconWuwa: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/auto_icon/wuwa/_icon.json',
    /** 原神自动图标 JSON URL */
    autoIconGenshin: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/auto_icon/genshin/_icon.json',
    /** 崩坏：星穹铁道自动图标 JSON URL */
    autoIconHsr: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/auto_icon/hsr/_icon.json',
    /** 绝区零自动图标 JSON URL */
    autoIconZzz: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/auto_icon/zzz/_icon.json',
    /** 明日方舟：终末地自动图标 JSON URL */
    autoIconEndfield: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/auto_icon/endfield/_icon.json',
    /** 自动图标说明页面 URL（GitHub 目录） */
    autoIconInfo: 'https://github.com/Aglglg/No-Reload-Mod-Manager/tree/main/assets/cloud_data/auto_icon',
    /** 已知通用模组库 JSON URL（用于自动识别与提示） */
    updatedKnownModdingLibs: 'https://raw.githubusercontent.com/Aglglg/No-Reload-Mod-Manager/refs/heads/main/assets/cloud_data/common_modding_lib/common_modding_lib.json',
    /** 虚拟按键代码参考文档 URL（供用户配置按键时查阅） */
    validKeysExample: 'https://learn.microsoft.com/en-us/windows/win32/inputdev/virtual-key-codes'
  },
  /**
   * 内置已知模组库映射表。
   * 键为识别字符串（通常出现在 INI 中），值为面向用户的展示名称。
   * 当本地数据未与云端同步时作为兜底使用。
   */
  knownModdingLibraries: {
    'rabbitfx': 'RabbitFx',
    'gimiv8': 'GIMIv8',
    'gimi': 'GIMI',
    'global\\healthbar': 'GIMI HealthBar Library',
    'global\\offset': 'GIMI Offset Library',
    'global\\orfix': 'ORFix',
    'global\\region': 'GIMI Region Library',
    'global\\tracking': 'GIMI Tracking Library',
    'texfx': 'TexFx',
    'srmi': 'SRMI',
    'srmiv1': 'SRMIv1',
    'wwmiv1': 'WWMIv1',
    'zzmiv1': 'ZZMIv1',
    'zzmi': 'ZZMI',
    'slotfix': 'SlotFix',
    'healthbar': 'HealthBar',
    'efmiv1': 'EFMIv1'
  } as Record<string, string>
};

/**
 * 目标游戏到完整名称的映射，用于界面展示。
 * 注意：此映射为英文默认值，实际展示时应优先使用 i18n 翻译。
 */
export const GAME_NAMES: Record<TargetGame, string> = {
  [TargetGame.none]: 'None',
  [TargetGame.Wuthering_Waves]: 'Wuthering Waves',
  [TargetGame.Genshin_Impact]: 'Genshin Impact',
  [TargetGame.Honkai_Star_Rail]: 'Honkai: Star Rail',
  [TargetGame.Zenless_Zone_Zero]: 'Zenless Zone Zero',
  [TargetGame.Arknights_Endfield]: 'Arknights: Endfield'
};

/**
 * 获取游戏名称的 i18n 翻译键。
 * 用于在模板中直接使用 $t() 进行翻译。
 * @param game - 目标游戏枚举值
 * @returns i18n 翻译键（如 "game.wuwa", "game.hsr"）
 */
export function getGameNameKey(game: TargetGame): string {
  const name = GAME_SHORT_NAMES[game];
  return `game.${name ?? 'none'}`;
}

/**
 * 目标游戏到短标识的映射，用于内部存储与路径构造（如 wuwa/genshin/hsr/zzz/endfield）。
 */
export const GAME_SHORT_NAMES: Record<TargetGame, string> = {
  [TargetGame.none]: 'none',
  [TargetGame.Wuthering_Waves]: 'wuwa',
  [TargetGame.Genshin_Impact]: 'genshin',
  [TargetGame.Honkai_Star_Rail]: 'hsr',
  [TargetGame.Zenless_Zone_Zero]: 'zzz',
  [TargetGame.Arknights_Endfield]: 'endfield'
};

/**
 * 键盘热键枚举到可读名称的映射，用于设置界面下拉框展示。
 */
export const HOTKEY_KEYBOARD_NAMES: Record<HotkeyKeyboard, string> = {
  [HotkeyKeyboard.none]: 'None',
  [HotkeyKeyboard.altA]: 'Alt + A',
  [HotkeyKeyboard.altB]: 'Alt + B',
  [HotkeyKeyboard.altC]: 'Alt + C',
  [HotkeyKeyboard.altD]: 'Alt + D',
  [HotkeyKeyboard.altE]: 'Alt + E',
  [HotkeyKeyboard.altF]: 'Alt + F',
  [HotkeyKeyboard.altG]: 'Alt + G',
  [HotkeyKeyboard.altH]: 'Alt + H',
  [HotkeyKeyboard.altI]: 'Alt + I',
  [HotkeyKeyboard.altJ]: 'Alt + J',
  [HotkeyKeyboard.altK]: 'Alt + K',
  [HotkeyKeyboard.altL]: 'Alt + L',
  [HotkeyKeyboard.altM]: 'Alt + M',
  [HotkeyKeyboard.altN]: 'Alt + N',
  [HotkeyKeyboard.altO]: 'Alt + O',
  [HotkeyKeyboard.altP]: 'Alt + P',
  [HotkeyKeyboard.altQ]: 'Alt + Q',
  [HotkeyKeyboard.altR]: 'Alt + R',
  [HotkeyKeyboard.altS]: 'Alt + S',
  [HotkeyKeyboard.altT]: 'Alt + T',
  [HotkeyKeyboard.altU]: 'Alt + U',
  [HotkeyKeyboard.altV]: 'Alt + V',
  [HotkeyKeyboard.altW]: 'Alt + W',
  [HotkeyKeyboard.altX]: 'Alt + X',
  [HotkeyKeyboard.altY]: 'Alt + Y',
  [HotkeyKeyboard.altZ]: 'Alt + Z'
};

/**
 * 手柄热键枚举到可读名称的映射，用于设置界面下拉框展示。
 */
export const HOTKEY_GAMEPAD_NAMES: Record<HotkeyGamepad, string> = {
  [HotkeyGamepad.none]: 'None',
  [HotkeyGamepad.lsB]: 'Left Stick + B',
  [HotkeyGamepad.lsA]: 'Left Stick + A',
  [HotkeyGamepad.lsRb]: 'Left Stick + RB',
  [HotkeyGamepad.selectStart]: 'Select + Start',
  [HotkeyGamepad.lsRs]: 'Left Stick + Right Stick'
};

/**
 * 布局模式枚举到可读名称的映射，用于设置界面下拉框展示。
 */
export const LAYOUT_MODE_NAMES: Record<LayoutMode, string> = {
  [LayoutMode.Auto]: 'Auto',
  [LayoutMode.Carousel]: 'List',
  [LayoutMode.Grid]: 'Grid'
};

/**
 * 分组排序方式枚举到可读名称的映射，用于设置界面下拉框展示。
 */
export const SORT_GROUP_METHOD_NAMES: Record<SortGroupMethod, string> = {
  [SortGroupMethod.ByIndex]: 'By Index',
  [SortGroupMethod.ByName]: 'By Name'
};

/**
 * Mods 路径校验状态到用户可读描述的映射，用于在校验失败时给出明确的提示文案。
 */
export const MODS_PATH_STATUS_DESCRIPTIONS: Record<ModsPathStatus, string> = {
  [ModsPathStatus.invalidNotExist]: 'Mods folder does not exist',
  [ModsPathStatus.invalidNotModsFolder]: 'Not a valid mods folder',
  [ModsPathStatus.invalidMissingD3dx]: 'Missing d3dx file',
  [ModsPathStatus.invalidMissingDll]: 'Missing required DLL',
  [ModsPathStatus.invalidWithoutManagedFolder]: 'Missing _MANAGED_ folder',
  [ModsPathStatus.invalidWithoutPrerequisiteFiles]: 'Missing prerequisite files',
  [ModsPathStatus.invalidOutdated]: 'Outdated version',
  [ModsPathStatus.valid]: 'Valid mods folder'
};

/**
 * 应用支持的语言选项列表，用于设置界面语言下拉框。
 * 同时包含语言代码、英文名与原生名（用于本地化展示）。
 */
export const SUPPORTED_LANGUAGES: LanguageOption[] = [
  { code: 'en', name: 'English', nativeName: 'English' },
  { code: 'ru', name: 'Russian', nativeName: 'Русский' },
  { code: 'id', name: 'Indonesian', nativeName: 'Bahasa Indonesia' },
  { code: 'zh-CN', name: 'Simplified Chinese', nativeName: '简体中文' },
  { code: 'zh-TW', name: 'Traditional Chinese', nativeName: '繁體中文' }
];

/** 主界面顶部标签页的有序列表（与 TabType 联合类型一一对应）。 */
export const TABS = ['mods', 'keybinds', 'settings'] as const;

/**
 * 通知自动关闭时长预设（毫秒）。
 * - short：短时提示
 * - medium：中等时长
 * - long：较长时长
 * - persistent：常驻不自动关闭
 */
export const NOTIFICATION_DURATION = {
  short: 2000,
  medium: 4000,
  long: 6000,
  persistent: 0
};
