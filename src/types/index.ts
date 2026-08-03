/**
 * 类型定义模块
 * 定义应用中使用的所有数据结构、枚举类型和接口
 */

/** 支持的目标游戏类型 */
export type TargetGame = 'GenshinImpact' | 'HonkaiStarRail' | 'Wuwa' | 'ZZZ' | 'HonkaiImpact3rd' | 'ArknightsEndfield';

/** 模组分组类型：普通分组（group_xx 一级互斥槽位）、自定义并行组、互斥组（可嵌套） */
export type GroupType = 'normalGroup' | 'exclusiveSlot' | 'customParallel' | 'mutexGroup';

/** 模组路径状态：正常、空、无权限、未设置 */
export type ModsPathStatus = 'normal' | 'empty' | 'noAccess' | 'notSet';

/** 布局模式：网格、轮播、自动 */
export type LayoutMode = 'grid' | 'carousel' | 'automatic';

/** 排序方式：默认、字母顺序、最近模组、反向字母 */
export type SortingType = 'default' | 'alphabetical' | 'recentMod' | 'reverseAlphabetical';

/** 光标类型：普通、精确 */
export type CursorType = 'normal' | 'precision';

/** 通知级别：信息、警告、错误、成功 */
export type NotificationLevel = 'info' | 'warning' | 'error' | 'success';

/** 按键配置预设 */
export type KeybindProfile = 'GI' | 'HSR' | 'Wuwa' | 'ZZZ' | 'Hi3';

/**
 * 按键绑定数据
 * 描述INI文件中单个按键配置项
 */
export interface KeybindData {
  /** 按键键名，如 Key, Key2 等 */
  key: string;
  /** 按键值，如 VK_RETURN 等 */
  value: string;
  /** 所属配置段名 */
  section: string;
  /** 是否禁用 */
  disabled: boolean;
  /** 扩展名（用于KeybindCommands等特殊段） */
  extension: string;
}

/**
 * INI解析错误行信息
 */
export interface ErroredLines {
  /** 错误所在行号 */
  lineNumber: number;
  /** 错误行内容 */
  line: string;
  /** 错误类型编号 */
  errorType: number;
}

/**
 * 模组INI配置数据
 * 从模组的merged.ini中解析出的结构化数据
 */
export interface ModIniData {
  /** INI文件完整路径 */
  iniPath: string;
  /** 常规按键绑定列表 */
  keybinds: KeybindData[];
  /** 按键命令列表 */
  keybindCommands: KeybindData[];
  /** 常量定义列表 */
  constants: KeybindData[];
  /** 覆盖项列表 */
  overrides: KeybindData[];
  /** 命令列表 */
  commandLists: KeybindData[];
  /** 存在的配置段名称列表 */
  presentSections: string[];
  /** 相对于模组根目录的文件路径 */
  fileRelativePath: string;
}

/**
 * 模组数据
 * 描述单个模组的完整状态信息
 */
export interface ModData {
  /** 模组文件夹完整路径 */
  modPath: string;
  /** 模组显示名称 */
  modName: string;
  /** 显示名称（去掉DISABLED_前缀后的干净名称） */
  name?: string;
  /** 模组完整绝对路径 */
  fullPath?: string;
  /** 父文件夹路径 */
  parentFolder?: string;
  /** 预览图片本地路径（后端扫描时自动查找 icon.png/preview.png 等） */
  previewImagePath?: string;
  /** 解析后的INI配置数据，若无法解析则为null */
  modIni: ModIniData | null;
  /** 模组是否处于激活状态（INI中已启用） */
  isActive: boolean;
  /** 是否标记为收藏 */
  isFavorite: boolean;
  /** 是否使用命名空间隔离 */
  isNamespaced: boolean;
  /** 是否存在非托管模组崩溃修复标记 */
  hasNonmanagedModsCrashlineFix: boolean;
  /** 本次扫描发现的错误行列表 */
  erroredLines: ErroredLines[];
  /** 之前已存在的错误行列表 */
  erroredPreexisting: ErroredLines[];
  /** 缺失的endif指令列表 */
  missingEndif: string[];
  /** 使用的命名空间列表 */
  namespaces: string[];
  /** 已知依赖库列表 */
  knownLibraries: string[];
  /** 重复库项：[库名, 冲突路径列表] */
  duplicateLibraries: [string, string[]][];
  /** 不存在的引用库列表 */
  nonexistentLibraries: string[];
  /** 是否存在命名空间错误 */
  namespaceError: boolean;
  /** 模组是否被完全禁用 */
  modDisabled: boolean;
  /** 路径是否过长（Windows MAX_PATH限制） */
  pathTooLong: boolean;
  /** 模组在分组中的索引 */
  modIndex: number;
  /** 是否属于互斥组 */
  isMutex: boolean;
  /** 是否禁用（兼容字段） */
  disabled: boolean;
  /** 所属分组索引 */
  groupIndex: number;
}

/**
 * 模组分组数据
 * 描述一个分组文件夹及其包含的模组列表
 */
export interface ModGroupData {
  /** 分组文件夹完整路径 */
  groupPath: string;
  /** 分组名称（原始目录名） */
  groupName: string;
  /** 分组显示名称（去掉DISABLED_前缀，或从groupname文件读取） */
  name?: string;
  /** 分组唯一ID */
  groupId: number;
  /** 分组在列表中的索引 */
  groupIndex: number;
  /** 分组内包含的模组列表 */
  mods: ModData[];
  /** 模组数量 */
  modCount: number;
  /** 分组是否处于激活状态 */
  isActive: boolean;
  /** 是否标记为收藏 */
  isFavorite: boolean;
  /** 分组是否被禁用 */
  groupDisabled: boolean;
  /** 分组类型 */
  groupType: GroupType;
  /** 是否包含子分组 */
  hasChild: boolean;
  /** 子分组列表 */
  children: ModGroupData[];
  /** 子分组列表（后端字段别名） */
  childGroups?: ModGroupData[];
  /** 当前激活的模组索引 */
  activeModIndex: number;
  /** 预览图片本地路径（后端扫描时自动查找 icon.png/preview.png 等） */
  previewImagePath?: string;
  /** 模组完整绝对路径 */
  fullPath?: string;
  /** 模组路径列表（后端字段） */
  modPaths?: string[];
  /**
   * UI层使用：是否是虚拟"Groups"根节点，
   * 用于把NormalGroup / ExclusiveSlot / CustomParallel收拢到一个虚拟父节点下展示。
   * 虚拟节点不会触发真实groupPath的选择、右键菜单、重命名/删除等操作。
   */
  isVirtualRoot?: boolean;
}

/**
 * 键盘热键配置
 */
export interface HotkeyKeyboard {
  /** 第一修饰键（如Ctrl, Alt等） */
  mod1: string;
  /** 第二修饰键 */
  mod2: string;
  /** 下一个按键 */
  keyNext: string;
  /** 上一个按键 */
  keyPrev: string;
  /** 隐藏窗口按键 */
  keyHide: string;
  /** 选择按键 */
  keySelect: string;
  /** 取消按键 */
  keyCancel: string;
  /** 向上滚动按键 */
  keyScrollup: string;
  /** 向下滚动按键 */
  keyScrolldown: string;
}

/**
 * 手柄热键配置
 */
export interface HotkeyGamepad {
  /** 方向键上 */
  dpadUp: boolean;
  /** 方向键下 */
  dpadDown: boolean;
  /** 方向键左 */
  dpadLeft: boolean;
  /** 方向键右 */
  dpadRight: boolean;
  /** 下一个按钮 */
  buttonNext: string;
  /** 上一个按钮 */
  buttonPrev: string;
  /** 隐藏按钮 */
  buttonHide: string;
  /** 选择按钮 */
  buttonSelect: string;
  /** 取消按钮 */
  buttonCancel: string;
}

/**
 * 应用程序完整设置
 */
export interface AppSettings {
  /** 当前目标游戏 */
  targetGame: TargetGame;
  /** 键盘热键配置 */
  hotkey: HotkeyKeyboard;
  /** 手柄热键配置 */
  gamepadHotkey: HotkeyGamepad;
  /** 窗口切换快捷键（键盘） */
  windowHotkey: string;
  /** 窗口切换快捷键（手柄） */
  gamepadHotkeyToggle: string;
  /** 搜索快捷键 */
  searchHotkey: string;
  /** 各游戏的模组路径映射 */
  gameModsPath: Record<TargetGame, string>;
  /** 各游戏的目标进程名映射 */
  targetProcessPerGame: Record<TargetGame, string>;
  /** 各游戏的按键配置预设 */
  gameProfile: Record<string, KeybindProfile>;
  /** 界面缩放比例 (0.6-2.0) */
  interfaceScale: number;
  /** 背景透明度 (0-1) */
  bgTransparency: number;
  /** 是否启用动态背景 */
  dynamicBackground: boolean;
  /** 模组分组布局模式 */
  modGroupingMode: LayoutMode;
  /** 模组排序方式 */
  modsSortingType: SortingType;
  /** 是否反向排序 */
  reverseSort: boolean;
  /** 光标类型 */
  cursorType: CursorType;
  /** 界面语言 */
  language: string;
  /** 是否深色模式 */
  darkMode: boolean;
  /** 是否自动生成分组文件夹图标 */
  autoFolderIcon: boolean;
  /** 是否自动设置优先级索引 */
  autoPriorityIndex: boolean;
  /** 启动时是否检查更新 */
  checkUpdateOnStart: boolean;
  /** 热键激活时是否自动置顶窗口 */
  autoTopWindow: boolean;
  /** 窗口是否全屏 */
  isWindowFullscreen: boolean;
  /** 是否在屏幕上显示按键提示 */
  showKeypressOnScreen: boolean;
  /** 选择时是否模拟按键 */
  simulateKeyOnSelection: boolean;
  /** 是否使用精确热键模式 */
  usePreciseHotkey: boolean;
  /** 是否交换取消按键绑定 */
  swapCancelKeybind: boolean;
  /** 热键时是否始终显示菜单 */
  alwaysShowMenuOnHotkey: boolean;
  /** 热键是否仅在3DMigoto运行时生效 */
  hotkeyOnlyInMigoto: boolean;
  /** 文件夹图标黑名单列表 */
  folderIconBlacklist: string[];
  /** 禁用的键盘输入列表 */
  disabledKbInputs: string[];
  /** 禁用的手柄输入列表 */
  disabledGamepadInputs: string[];
  /** 各游戏上次选中的模组索引 */
  selectedModIndex: Record<string, number>;
  /** 各游戏上次选中的分组索引 */
  selectedGroupIndex: Record<string, number>;
  /** 是否启用键盘热键 */
  enabledKb: boolean;
  /** 是否启用手柄热键 */
  enabledGamepad: boolean;
  /** 是否显示有错误的模组 */
  showErroredMods: boolean;
  /** 是否仅显示收藏模组 */
  showFavoritesOnly: boolean;
  /** 是否检查命名空间冲突 */
  checkNamespaceConflict: boolean;
}

/**
 * 平台信息
 * 描述当前操作系统及功能支持状态
 */
export interface PlatformInfo {
  /** 操作系统类型：windows, macos, linux */
  os: string;
  /** 会话类型（Linux下x11/wayland） */
  sessionType: string | null;
  /** 按键模拟是否受支持 */
  keypressSupported: boolean;
  /** 按键模拟错误信息 */
  keypressError: string | null;
  /** 前台窗口检测是否受支持 */
  foregroundDetectionSupported: boolean;
}

/**
 * 平台依赖状态
 * 描述Linux/macOS下外部依赖工具的可用性
 */
export interface PlatformDepsStatus {
  /** xdotool是否可用（Linux X11按键模拟） */
  xdotoolAvailable: boolean;
  /** ydotool是否可用（Linux Wayland按键模拟） */
  ydotoolAvailable: boolean;
  /** libxtst是否可用（X11测试扩展） */
  libxtstAvailable: boolean;
  /** macOS辅助功能权限是否已授予 */
  hasAccessibilityPermission: boolean;
}

/**
 * 云端链接项
 */
export interface CloudLink {
  /** 链接名称 */
  name: string;
  /** 链接URL */
  url: string;
  /** 链接图标（可选） */
  icon?: string;
}

/**
 * 云端消息
 */
export interface CloudMessage {
  /** 消息标题 */
  title: string;
  /** 消息内容 */
  content: string;
  /** 消息级别 */
  level: 'info' | 'warning' | 'error' | 'success';
  /** 消息日期（可选） */
  date?: string;
}

/**
 * 云端同步数据
 * 包含从云端拉取的链接、消息和自动图标配置
 */
export interface CloudData {
  /** 外部链接列表 */
  links: CloudLink[];
  /** 公告消息列表 */
  messages: CloudMessage[];
  /** 自动图标映射：模组名 -> 图标URL */
  autoIcons: Record<string, string>;
  /** 已知库映射：库名 -> 所属模组名列表 */
  knownLibraries: Record<string, string[]>;
}

/**
 * Tauri命令调用通用结果
 * @template T 返回数据类型
 */
export interface InvokeResult<T> {
  /** 调用是否成功 */
  success: boolean;
  /** 返回数据（成功时） */
  data?: T;
  /** 错误信息（失败时） */
  error?: string;
}

/**
 * 模组扫描结果
 * 轻量扫描返回的分组和模组列表
 */
export interface ScanResult {
  /** 分组列表 */
  groups: ModGroupData[];
  /** 所有模组列表（扁平结构） */
  mods: ModData[];
}

/**
 * 重量级更新结果
 * 执行完整INI解析和修复后返回的统计信息
 */
export interface UpdateResult {
  /** 处理的总分组数 */
  totalGroups: number;
  /** 处理的总模组数 */
  totalMods: number;
  /** 启用的模组数 */
  enabledMods: number;
  /** 禁用的模组数 */
  disabledMods: number;
  /** 实际处理的模组数 */
  processedMods: number;
  /** 发现的错误行列表 */
  errors: ErroredLines[];
  /** 是否需要用户手动重载 */
  needReloadManual: boolean;
  /** switch_mod成功时返回写入的选中索引（NormalGroup）；其他操作无此字段（undefined） */
  selectedModIndex?: number;
  /** 是否检测到标准 XXMI/3DMigoto 环境（仅 updateModData 结果中有效） */
  isStandardXxmi?: boolean;
}

/**
 * Save Customizations操作结果
 */
export interface SaveCustomizationsResult {
  /** 是否成功 */
  success: boolean;
  /** 结果消息 */
  message: string;
}

/**
 * INI恢复操作统计结果
 */
export interface RestoredCount {
  /** 成功恢复的文件数 */
  restored: number;
  /** 恢复失败的文件数 */
  failed: number;
}
