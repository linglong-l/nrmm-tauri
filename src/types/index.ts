/**
 * 单个 Mod 的数据结构，描述一个模组在文件系统与界面上的全部展示与状态信息。
 * 该接口是 Mod 列表的最小单元，会被 ModGroupData 包含。
 */
export interface ModData {
  /** Mod 的完整文件系统路径（绝对路径），用于唯一定位该 Mod */
  modPath: string;
  /** Mod 图标路径，若未找到合适的图标则为 null */
  iconPath: string | null;
  /** Mod 显示名称，通常取自文件夹名 */
  modName: string;
  /** 在所属分组中的真实索引位置（用于排序、选中状态记录） */
  realIndex: number;
  /** 是否为旧版本自动修复过的 Mod（兼容历史版本的标记） */
  isOldAutoFixed: boolean;
  /** 是否因语法错误被移除（被剔除出 Managed 列表的标记） */
  isSyntaxErrorRemoved: boolean;
  /** 是否为未优化的 Mod（提示用户可优化） */
  isUnoptimized: boolean;
  /** 是否使用了命名空间（影响 keybind 逻辑） */
  isNamespaced: boolean;
  /** 是否处于禁用状态（禁用后不会被加载到游戏中） */
  isDisabled: boolean;
  /** 收藏时间戳字符串，未收藏时为 null */
  favoriteDateTime: string | null;
}

/**
 * Mod 分组数据结构，一个分组包含若干个 Mod，并维护分组自身的元数据。
 * 分组通常对应游戏中的某个角色或场景，用户可在不同 Mod 之间切换选择。
 */
export interface ModGroupData {
  /** 分组文件夹的完整路径（绝对路径） */
  groupPath: string;
  /** 分组图标路径，未找到时为 null */
  iconPath: string | null;
  /** 分组显示名称 */
  groupName: string;
  /** 收藏时间戳字符串，未收藏时为 null */
  favoriteDateTime: string | null;
  /** 该分组下包含的全部 Mod 列表 */
  modsInGroup: ModData[];
  /** 分组在父级目录中的真实索引位置 */
  realIndex: number;
  /** 该分组中上一次选中的 Mod 索引（用于切换后恢复选中状态） */
  previousSelectedModOnGroup: number;
}

/**
 * 目标游戏枚举，标识当前所管理的 Mod 所属的游戏。
 * 切换不同游戏时会加载对应游戏的 Mods 目录与配置。
 */
export enum TargetGame {
  /** 未指定游戏（初始/默认状态） */
  none = 'none',
  /** 鸣潮 */
  Wuthering_Waves = 'Wuthering_Waves',
  /** 原神 */
  Genshin_Impact = 'Genshin_Impact',
  /** 崩坏：星穹铁道 */
  Honkai_Star_Rail = 'Honkai_Star_Rail',
  /** 绝区零 */
  Zenless_Zone_Zero = 'Zenless_Zone_Zero',
  /** 明日方舟：终末地 */
  Arknights_Endfield = 'Arknights_Endfield'
}

/**
 * 键盘快捷键枚举，定义可注册的全局键盘热键组合。
 * 均为 Alt + 字母 的组合，避免与游戏内按键冲突。
 */
export enum HotkeyKeyboard {
  /** Alt + W */
  altW = 'altW',
  /** Alt + S */
  altS = 'altS',
  /** Alt + A */
  altA = 'altA',
  /** Alt + D */
  altD = 'altD'
}

/**
 * 手柄快捷键枚举，定义可注册的手柄热键组合。
 * 多为左摇杆方向 + 按键的组合，以适配游戏中常见的手柄布局。
 */
export enum HotkeyGamepad {
  /** 不使用手柄热键 */
  none = 'none',
  /** 左摇杆下 + B */
  lsB = 'lsB',
  /** 左摇杆下 + A */
  lsA = 'lsA',
  /** 左摇杆下 + RB */
  lsRb = 'lsRb',
  /** Select + Start */
  selectStart = 'selectStart',
  /** 左摇杆 + 右摇杆 */
  lsRs = 'lsRs'
}

/**
 * 热键配置接口，同时包含键盘与手柄两部分热键设置。
 */
export interface HotkeyConfig {
  /** 键盘热键 */
  keyboard: HotkeyKeyboard;
  /** 手柄热键 */
  gamepad: HotkeyGamepad;
}

/**
 * Mods 路径校验状态枚举，描述对 Mods 目录合法性校验后的各种结果。
 * 用于在设置界面给用户明确的错误提示。
 */
export enum ModsPathStatus {
  /** 路径不存在 */
  invalidNotExist = 'invalidNotExist',
  /** 路径存在但不是合法的 Mods 文件夹 */
  invalidNotModsFolder = 'invalidNotModsFolder',
  /** 缺少 d3dx.ini 配置文件 */
  invalidMissingD3dx = 'invalidMissingD3dx',
  /** 缺少必要的 DLL 文件 */
  invalidMissingDll = 'invalidMissingDll',
  /** 缺少 _MANAGED_ 管理文件夹 */
  invalidWithoutManagedFolder = 'invalidWithoutManagedFolder',
  /** 缺少前置必备文件 */
  invalidWithoutPrerequisiteFiles = 'invalidWithoutPrerequisiteFiles',
  /** Mods 框架版本过旧 */
  invalidOutdated = 'invalidOutdated',
  /** 路径合法且可用 */
  valid = 'valid'
}

/**
 * 界面布局模式枚举，控制 Mod 列表的展示方式。
 * Auto 模式会根据窗口尺寸自动在 Carousel 与 Grid 间切换。
 */
export enum LayoutMode {
  /** 自动布局，依据窗口尺寸自动选择 Carousel 或 Grid */
  Auto = 0,
  /** 轮播布局，单行展示，左右切换 */
  Carousel = 1,
  /** 网格布局，多行多列展示 */
  Grid = 2
}

/**
 * 分组排序方式枚举，控制 Mod 分组在列表中的排列顺序。
 */
export enum SortGroupMethod {
  /** 按索引排序（保持文件系统顺序） */
  ByIndex = 0,
  /** 按名称排序（字母序） */
  ByName = 1
}

/**
 * Mod 快捷键绑定信息，用于 keybinds 标签页中展示某个 Mod 与热键的对应关系。
 */
export interface ModKeybindInfo {
  /** 关联的 Mod 数据 */
  modData: ModData;
  /** 所属分组名称 */
  groupName: string;
  /** 所属目标游戏 */
  targetGame: TargetGame;
  /** 是否为休闲风格（影响展示样式） */
  isCasualStyle: boolean;
  /** 是否为 INI 文件类型 Mod */
  isIniFile: boolean;
}

/**
 * 应用全局设置接口，包含所有可持久化的用户配置项。
 * 该结构对应后端 settings 文件，并用于 settings 标签页的双向绑定。
 */
export interface AppSettings {
  /** 键盘热键 */
  hotkeyKeyboard: HotkeyKeyboard;
  /** 手柄热键 */
  hotkeyGamepad: HotkeyGamepad;
  /** 鸣潮目标进程名 */
  targetProcessWuwa: string;
  /** 原神目标进程名 */
  targetProcessGenshin: string;
  /** 崩坏：星穹铁道目标进程名 */
  targetProcessHsr: string;
  /** 绝区零目标进程名 */
  targetProcessZzz: string;
  /** 明日方舟：终末地目标进程名 */
  targetProcessEndfield: string;
  /** 整体界面缩放比例 */
  overallScale: number;
  /** 背景透明度（0-1） */
  bgTransparency: number;
  /** 界面布局模式 */
  layoutMode: LayoutMode;
  /** 界面语言代码 */
  language: string;
  /** 是否自动生成分组文件夹图标 */
  isAutoGenerateFolderIcon: boolean;
  /** 是否自动置顶窗口 */
  isAutoPinWindow: boolean;
  /** 游戏外切换时是否显示菜单 */
  showMenuWhenTogglingOutsideGame: boolean;
  /** 是否通过模拟按键方式触发 keybind */
  keybindSimulateKeypress: boolean;
  /** 分组排序方式 */
  sortGroupMethod: SortGroupMethod;
  /** 上次保存的窗口宽度 */
  savedWindowWidth: number;
  /** 上次保存的窗口高度 */
  savedWindowHeight: number;
  /** 上次保存的窗口 X 坐标，未保存时为 null */
  savedWindowX: number | null;
  /** 上次保存的窗口 Y 坐标，未保存时为 null */
  savedWindowY: number | null;
  /** 主题名称 */
  theme: string;
  /** 鸣潮的 Mods 目录路径 */
  modsPathWuwa: string;
  /** 原神的 Mods 目录路径 */
  modsPathGenshin: string;
  /** 崩坏：星穹铁道的 Mods 目录路径 */
  modsPathHsr: string;
  /** 绝区零的 Mods 目录路径 */
  modsPathZzz: string;
  /** 明日方舟：终末地的 Mods 目录路径 */
  modsPathEndfield: string;
}

/**
 * 云端数据聚合接口，包含云端下发的链接、消息、自动图标和已知模组库映射。
 * 通过 fetch_cloud_data 拉取并缓存到本地。
 */
export interface CloudData {
  /** 支持入口、教程入口、联系入口的图标与链接 */
  links: CloudLinks;
  /** 各游戏的公告消息 */
  messages: CloudMessages;
  /** 各游戏的自动图标数据 */
  autoIcons: AutoIconData;
  /** 已知模组库的键值映射（键为识别字符串，值为展示名称） */
  knownModLibraries: Record<string, string>;
}

/**
 * 云端链接数据，包含三个入口（支持/教程/联系）的常态与悬停图标及跳转链接。
 */
export interface CloudLinks {
  /** 支持入口常态图标 URL */
  supportIcon: string;
  /** 支持入口悬停图标 URL */
  supportIconOnHover: string;
  /** 支持入口跳转链接 */
  supportLink: string;
  /** 教程入口常态图标 URL */
  tutorialIcon: string;
  /** 教程入口悬停图标 URL */
  tutorialIconOnHover: string;
  /** 教程入口跳转链接 */
  tutorialLink: string;
  /** 联系入口常态图标 URL */
  contactIcon: string;
  /** 联系入口悬停图标 URL */
  contactIconOnHover: string;
  /** 联系入口跳转链接 */
  contactLink: string;
}

/**
 * 云端公告消息集合，按游戏区分。每条为一段纯文本。
 */
export interface CloudMessages {
  /** 鸣潮公告 */
  wuwa: string;
  /** 原神公告 */
  genshin: string;
  /** 崩坏：星穹铁道公告 */
  hsr: string;
  /** 绝区零公告 */
  zzz: string;
  /** 明日方舟：终末地公告 */
  endfield: string;
}

/**
 * 自动图标数据集合，按游戏分组。用于在生成分组图标时按名称匹配远程图标。
 */
export interface AutoIconData {
  /** 鸣潮自动图标列表 */
  wuwa: AutoIconEntry[];
  /** 原神自动图标列表 */
  genshin: AutoIconEntry[];
  /** 崩坏：星穹铁道自动图标列表 */
  hsr: AutoIconEntry[];
  /** 绝区零自动图标列表 */
  zzz: AutoIconEntry[];
  /** 明日方舟：终末地自动图标列表 */
  endfield: AutoIconEntry[];
}

/**
 * 单个自动图标条目，包含匹配名称与图标 URL。
 */
export interface AutoIconEntry {
  /** 用于匹配 Mod/分组名称的字符串 */
  name: string;
  /** 图标远程 URL */
  iconUrl: string;
}

/**
 * 调用 update_mod_data 后端命令的返回结果。
 * 用于描述刷新/更新操作是否成功，以及最新分组列表与可能的错误信息。
 */
export interface UpdateModDataResult {
  /** 操作是否成功 */
  success: boolean;
  /** 最新分组数据列表（成功时返回） */
  groups: ModGroupData[];
  /** 失败时的错误信息（可选） */
  errorMessage?: string;
}

/**
 * INI 语法错误信息，记录某条错误所在的文件位置与具体内容。
 */
export interface IniSyntaxError {
  /** 错误所在行号（1-based） */
  lineNumber: number;
  /** 错误行原文 */
  lineContent: string;
  /** 错误描述信息 */
  errorMessage: string;
  /** 出错的 INI 文件路径 */
  filePath: string;
}

/**
 * Mod 压缩包归档信息，描述待安装或预览的压缩包内容元数据。
 */
export interface ModArchiveInfo {
  /** 压缩包完整路径 */
  archivePath: string;
  /** 压缩包文件名 */
  archiveName: string;
  /** 压缩包内文件数量 */
  fileCount: number;
  /** 压缩包总大小（字节） */
  totalSize: number;
  /** Mod 名称（从压缩包内元数据读取，可能为 null） */
  modName: string | null;
  /** Mod 作者（从压缩包内元数据读取，可能为 null） */
  modAuthor: string | null;
  /** Mod 版本（从压缩包内元数据读取，可能为 null） */
  modVersion: string | null;
}

/**
 * 文件监听事件类型，描述被监听目录中发生的文件系统变化类型。
 */
export type FileWatcherEventType = 'create' | 'modify' | 'delete' | 'rename';

/**
 * 文件监听事件，由后端 file_watcher 推送到前端，描述一次具体变化。
 */
export interface FileWatcherEvent {
  /** 事件类型 */
  type: FileWatcherEventType;
  /** 受影响的文件/目录路径 */
  path: string;
  /** 是否为目录变化 */
  isDirectory: boolean;
  /** 事件发生时间戳（毫秒） */
  timestamp: number;
}

/**
 * 窗口位置与尺寸信息，用于窗口状态的保存与恢复。
 */
export interface WindowPosition {
  /** 窗口左上角 X 坐标 */
  x: number;
  /** 窗口左上角 Y 坐标 */
  y: number;
  /** 窗口宽度 */
  width: number;
  /** 窗口高度 */
  height: number;
}

/**
 * 系统托盘菜单项结构，支持普通项、分隔符、子菜单以及勾选状态。
 */
export interface TrayMenuItem {
  /** 菜单项唯一标识 */
  id: string;
  /** 菜单项显示文本 */
  label: string;
  /** 菜单项图标路径（可选） */
  icon?: string;
  /** 是否启用（可选，默认启用） */
  enabled?: boolean;
  /** 是否勾选（可选，用于切换型菜单项） */
  checked?: boolean;
  /** 是否为分隔符（可选） */
  separator?: boolean;
  /** 子菜单项列表（可选） */
  submenu?: TrayMenuItem[];
}

/**
 * 主界面顶部标签页类型，控制当前展示的功能页。
 */
export type TabType = 'keybinds' | 'mods' | 'settings';

/**
 * 右键上下文菜单状态，记录菜单是否可见、位置及携带的数据。
 */
export interface ContextMenuState {
  /** 菜单是否可见 */
  visible: boolean;
  /** 菜单左上角 X 坐标（屏幕坐标） */
  x: number;
  /** 菜单左上角 Y 坐标（屏幕坐标） */
  y: number;
  /** 菜单类型标识（用于区分不同右键场景） */
  menuType: string | null;
  /** 菜单附带数据，类型随 menuType 变化 */
  data: unknown;
}

/**
 * 各类对话框的开关状态集合，统一管理弹窗显隐。
 */
export interface DialogStates {
  /** 设置对话框 */
  settings: boolean;
  /** 关于对话框 */
  about: boolean;
  /** 添加 Mod 对话框 */
  addMod: boolean;
  /** 添加分组对话框 */
  addGroup: boolean;
  /** 编辑 Mod 对话框 */
  editMod: boolean;
  /** 编辑分组对话框 */
  editGroup: boolean;
  /** 确认删除对话框 */
  confirmDelete: boolean;
  /** Mod 信息对话框 */
  modInfo: boolean;
}

/**
 * 通知消息结构，用于界面右上角通知系统。
 */
export interface Notification {
  /** 通知唯一 ID */
  id: string;
  /** 通知类型 */
  type: 'success' | 'error' | 'warning' | 'info';
  /** 通知标题 */
  title: string;
  /** 通知正文 */
  message: string;
  /** 自动关闭时长（毫秒），0 表示不自动关闭 */
  duration: number;
  /** 创建时间戳（毫秒） */
  timestamp: number;
}

/**
 * 热键运行时状态，记录热键注册与触发情况。
 */
export interface HotkeyState {
  /** 热键是否已成功注册 */
  isRegistered: boolean;
  /** 热键当前是否启用 */
  isEnabled: boolean;
  /** 最近一次按下的键（字符串描述），未触发时为 null */
  lastPressed: string | null;
  /** 最近一次按下的时间戳（毫秒），未触发时为 null */
  lastPressedTime: number | null;
}

/**
 * 语言选项结构，用于设置界面语言下拉框。
 */
export interface LanguageOption {
  /** 语言代码（如 zh-CN） */
  code: string;
  /** 语言英文名称 */
  name: string;
  /** 语言原生名称（如 简体中文） */
  nativeName: string;
}

/**
 * Mod 搜索结果，包含匹配的 Mod、所属分组以及匹配评分与命中的字段列表。
 */
export interface ModSearchResult {
  /** 匹配到的 Mod 数据 */
  mod: ModData;
  /** 该 Mod 所属的分组 */
  group: ModGroupData;
  /** 匹配评分（数值越高匹配度越高） */
  matchScore: number;
  /** 命中的字段名称列表（如 modName、groupName 等） */
  matchedFields: string[];
}
