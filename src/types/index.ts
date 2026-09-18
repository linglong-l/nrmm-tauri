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
 * 按键切换（Keybind Simulation）单个按键条目
 * 对应后端 `KeybindEntry`（NRMM `KeyData`）：
 * `key = <combo>`（主键）或 `back = <combo>`（反向键）
 */
export interface KeybindEntry {
  /** true = `key =`，false = `back =` */
  isMainKey: boolean;
  /** combo 原始值（如 `no_modifiers 5`） */
  value: string;
  /** 原始 INI 中的行号（0-based，写回定位用） */
  lineIndex: number;
}

/**
 * 按键切换：单个 `[KeyXXX]` 段（XXX 为段别名，如 Top/Shoulder）
 */
export interface KeybindSection {
  /** 去 `Key` 前缀后的段名（如 `Top`） */
  section: string;
  /** 段头是否带 `;+;` 前缀（NRMM 禁用标记） */
  disabled: boolean;
  /** 段头行号（0-based，写回定位用） */
  sectionLineIndex: number;
  /** 段内按键条目（key 与 back 混合，保持原始顺序） */
  keys: KeybindEntry[];
}

/**
 * 按键切换：一个 INI 文件的按键段集合
 */
export interface KeybindFile {
  /** INI 文件绝对路径 */
  filePath: string;
  /** 该文件的按键段列表 */
  sections: KeybindSection[];
}

/**
 * 保存按键切换的结果
 */
export interface SaveKeybindResult {
  /** 是否存在重复段名（大小写不敏感 + namespace 后缀） */
  duplicateSections: boolean;
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
  /** 当前激活的模组索引 */
  activeModIndex: number;
  /** 预览图片本地路径（后端扫描时自动查找 icon.png/preview.png 等） */
  previewImagePath?: string;
  /** 模组完整绝对路径 */
  fullPath?: string;
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
  /** 窗口是否透明化（Windows/其它平台默认 true，Linux 默认 false） */
  windowTransparent: boolean;
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
  /** 是否在启动时打开控制台窗口查看日志（默认 false，重启后生效） */
  showConsoleOnStart: boolean;
  /** 日志时间戳时区：system=系统本地（默认）/ UTC / 固定偏移（如 UTC+8、+08:00）；仅配置文件生效，无 UI 项 */
  logTimezone: string;
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
 * 被跳过的非法 UTF-8 路径条目（后端 fs_walk 上报）
 *
 * 文件名含非 UTF-8 字节（Windows 孤代理项 / Linux 原始字节）时，后端着跳越，
 * 该条目仅用于前端提示用户「发现无法识别的文件/目录名称」。
 */
export interface InvalidUtf8Entry {
  /** 所在父目录的显示路径（恒为合法 UTF-8，可直接展示） */
  parentDisplay: string;
  /** 非法文件名的保真字节：`u16le:<base64>`（Windows）或 `raw:<base64>`（Linux），前端解码后展示 */
  nameBytes: string;
  /** 条目类型：0=目录 1=文件 2=图片候选 3=符号链接 */
  kind: number;
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
  /** 因文件名非 UTF-8 被跳过的条目总数（无则省略） */
  invalidUtf8Count?: number;
  /** 被跳过的非法条目明细（无则省略） */
  invalidUtf8Entries?: InvalidUtf8Entry[];
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
  /** 实际写盘（INI 内容有变化）的 INI 计数 */
  writtenMods?: number;
  /** 因内容未变化跳过写盘（幂等快速路径）的 INI 计数 */
  skippedMods?: number;
  /** 发现的错误行列表 */
  errors: ErroredLines[];
  /** 是否需要用户手动重载 */
  needReloadManual: boolean;
  /** switch_mod成功时返回写入的选中索引（NormalGroup）；其他操作无此字段（undefined） */
  selectedModIndex?: number;
  /** 是否检测到标准 XXMI/3DMigoto 环境（仅 updateModData 结果中有效） */
  isStandardXxmi?: boolean;
  /** 主 INI（游戏根 d3dx.ini）处理状态：'Resolved'｜'MissingMainIni'（仅 updateModData 结果中有效；旧前端无该字段时按 Resolved 处理） */
  mainIniStatus?: 'Resolved' | 'MissingMainIni';
  /** ORFix/TexFx 检测结果（仅 updateModData 结果中有效） */
  orfixDetection?: OrfixDetection;
  /** 因文件名非 UTF-8 被跳过的条目总数（update_mod_data 深扫；无则省略） */
  invalidUtf8Count?: number;
  /** 被跳过的非法条目明细（无则省略） */
  invalidUtf8Entries?: InvalidUtf8Entry[];
}

/**
 * 命令级分组增量（R1：命令级 Delta 更新）
 *
 * 状态操作命令（toggle/rename/remove/批量组操作）返回受影响分组的新状态，
 * 前端原地 patch 进 store，避免全量 refresh IPC。
 * 对应后端 GroupDelta（serde camelCase 已由后端保证）。
 */
export interface GroupDelta {
  /** 旧分组路径（树定位键；分组重命名时与 newGroup.groupPath 不同） */
  oldGroupPath: string;
  /** 新分组节点（含 mods/children/统计，后端已排序）；null = 分组已整体移除 */
  newGroup: ModGroupData | null;
  /** 该分组旧的扁平 modPath 列表（用于从扁平 mods 剔除） */
  oldModPaths: string[];
}

/** 批量启停路径对（零扫描：模组目录重命名前后） */
export interface PathPair {
  /** 重命名前绝对路径 */
  oldPath: string;
  /** 重命名后绝对路径 */
  newPath: string;
}

/**
 * 批量组操作命令级增量（disable_all_mods_in_group / enable_all_mods_in_group）
 * 零扫描路径：toggled 携带路径对，degraded=false，前端逐条本地 patch；
 * 异常降级：degraded=true，delta 为增量扫描后的分组完整新状态（整体 patch + 告知用户）
 */
export interface BatchGroupDelta {
  /** 实际发生状态变化的模组数量 */
  changed: number;
  /** 零扫描路径：被切换状态的路径对列表（degraded=false 时消费） */
  toggled: PathPair[];
  /** 是否降级为增量扫描（true 时消费 delta 并告知用户异常） */
  degraded: boolean;
  /** 受影响分组的增量（degraded=true 时消费） */
  delta: GroupDelta;
}

/** 单模组启用/禁用结果（toggle_mod_disabled：零扫描路径） */
export interface ToggleModResult {
  /** 操作后模组目录绝对路径 */
  newPath: string;
  /** 操作后的禁用状态 */
  disabled: boolean;
  /** 是否降级为增量扫描（true 时消费 groupDelta 并告知用户异常） */
  degraded: boolean;
  /** 异常降级时的受影响分组增量 */
  groupDelta: GroupDelta;
}

/**
 * watcher 增量事件 payload（R2：managed-partial-update 直接携带分组增量列表）
 *
 * 后端 watcher 未溢出批次局部重扫合并缓存后推送；前端零额外 IPC 往返，
 * 对 deltas 逐组 applyGroupDelta 原地 patch。溢出/全量兜底分支不发此事件
 * （仅 managed-folder-changed → 前端全量 refresh 兜底）。
 */
export interface ManagedPartialUpdatePayload {
  /** 当前游戏（后端 TargetGame 的 Debug 字符串形态） */
  game: string;
  /** 受影响分组的增量列表（多 root 批次可能含多个组；空列表 = 失效批次，走兜底） */
  deltas: GroupDelta[];
}

/**
 * 移除模组结果（后端 RemoveModResult）
 *
 * 对应后端 `crate::core::mod_manager::RemoveModResult`
 * 字段采用 camelCase 命名（Rust 端通过 serde rename_all 自动转换）
 */
export interface RemoveModResult {
  /** 模组名称 */
  modName: string;
  /** 移动后的目标路径（Mods/DISABLED_MANAGED_REMOVED/ 下） */
  movedTo: string;
  /** INI 还原是否成功（true=全部成功，false=部分或全部失败） */
  restored: boolean;
  /** 还原过程中处理的 INI 文件数 */
  iniCount: number;
  /** 还原过程中失败的文件数 */
  failedCount: number;
  /** 受影响分组的增量（R1：前端原地 patch，替代全量 refresh） */
  groupDelta: GroupDelta;
  /** 是否降级为增量扫描（缓存剔除未命中；true 时消费 groupDelta 并告知用户异常） */
  degraded?: boolean;
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

/**
 * 还原模组 INI 结果（还原区功能）
 *
 * 对应后端 `crate::core::mod_manager::RestoreManagedResult`
 */
export interface RestoreManagedResult {
  /** 被还原的目录路径 */
  path: string;
  /** 处理的 INI 文件数 */
  iniCount: number;
  /** 还原失败的文件数 */
  failedCount: number;
  /** 是否全部还原成功 */
  success: boolean;
}

/**
 * Hash 冲突单条详情（一个模组使用该 hash 的一个或多个 INI 文件）
 *
 * 对应后端 `crate::models::mod_data::HashConflictEntry`
 */
export interface HashConflictEntry {
  /** 模组名称 */
  modName: string;
  /** 模组目录路径（绝对路径） */
  modPath: string;
  /** 包含该 hash 的 INI 文件绝对路径列表 */
  iniVec: string[];
}

/**
 * Hash 冲突信息（一个 hash 被多个模组使用）
 *
 * 对应后端 `crate::models::mod_data::HashConflict`
 */
export interface HashConflict {
  /** 冲突的 hash 值（如 "0x12345678"） */
  hash: string;
  /** 使用该 hash 的模组详情列表（每个模组一条，含其全部冲突 INI） */
  entries: HashConflictEntry[];
}

/**
 * Hash 冲突检测结果
 *
 * 对应后端 `crate::models::mod_data::HashConflictResult`
 */
export interface HashConflictResult {
  /** 所有冲突列表 */
  conflicts: HashConflict[];
  /** 扫描的模组总数 */
  scannedMods: number;
  /** 扫描的 hash 总数 */
  scannedHashes: number;
}

/**
 * 库在模组内（ORFix/TexFx 应在 Mods 根目录而非模组内）
 *
 * 对应后端 `crate::models::mod_data::LibInMod`
 */
export interface LibInMod {
  /** 模组名称 */
  modName: string;
  /** 模组路径 */
  modPath: string;
  /** 检测到的库名称列表（如 ["ORFix", "TexFx"]） */
  libNames: string[];
}

/**
 * 重复声明库（同一已知库被多个模组声明）
 *
 * 对应后端 `crate::models::mod_data::DuplicateLib`
 */
export interface DuplicateLib {
  /** 库名称（如 "ORFix"） */
  libName: string;
  /** 声明该库的模组名称列表 */
  modNames: string[];
}

/**
 * 引用未声明的库（run = 引用了已知库但全局未声明）
 *
 * 对应后端 `crate::models::mod_data::NonExistentLib`
 */
export interface NonExistentLib {
  /** 被引用的库名称（如 "ORFix"） */
  libName: string;
  /** 引用该库的模组名称列表 */
  modNames: string[];
}

/**
 * ORFix/TexFx 检测结果
 *
 * 对应后端 `crate::models::mod_data::OrfixDetection`
 * 由 update_mod_data 在修改 INI 之前使用原始内容检测得到
 */
export interface OrfixDetection {
  /** 库在模组内（模组声明了已知库命名空间） */
  libsInMods: LibInMod[];
  /** 重复声明（同一已知库命名空间被多个 INI 声明） */
  duplicateLibs: DuplicateLib[];
  /** 引用未声明（run = 引用了已知库但全局未声明） */
  nonexistentLibs: NonExistentLib[];
  /** 是否检测到任何 ORFix/TexFx 异常 */
  hasDetection: boolean;
}

/**
 * 更新模组数据遮罩状态
 * 由 UpdateModDataOverlay 组件内部状态机使用，亦作为 provide/inject 契约的一部分
 */
export type OverlayState = 'loading' | 'completed' | 'error';

/**
 * 更新模组数据遮罩控制器
 * App.vue 通过 provide('updateOverlay', ...) 提供，SettingsView / UpdateModDataReminder
 * 等组件通过 inject<OverlayController>('updateOverlay') 消费，替代原先的 `any` 类型。
 */
export interface OverlayController {
  /** 显示遮罩：status 为状态机值，data 携带 result / error / durationMs 等可选载荷 */
  show: (status: OverlayState, data?: { result?: UpdateResult | null; error?: string; durationMs?: number }) => void;
  /** 隐藏遮罩 */
  hide: () => void;
}
