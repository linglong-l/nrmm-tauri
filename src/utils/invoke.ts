// 前端对 Tauri 后端命令的统一封装层。
// 所有跨进程调用（IPC）均通过本文件中的 invoke* 函数进行，便于集中维护与类型约束。
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import type {
  ModData,
  ModGroupData,
  AppSettings,
  ModsPathStatus,
  UpdateModDataResult,
  IniSyntaxError,
  IniFileData,
  WindowPosition,
  TrayMenuItem,
  CloudData,
  HotkeyKeyboard,
  HotkeyGamepad,
  HashConflictReport
} from '../types';
import { TargetGame } from '../types';
import { CONSTANTS } from './constants';

// 显式重新导出 HashConflictReport 类型以解决 vue-tsc 命名空间问题
export type { HashConflictReport };

/**
 * 将本地文件系统路径转换为 WebView 可访问的 URL。
 * 用于渲染本地图片等资源。
 * @param path 本地文件绝对路径
 * @returns WebView 可访问的 URL
 */
export function convertToAssetUrl(path: string | null | undefined): string {
  if (!path) return '';
  const url = convertFileSrc(path);
  return url;
}

/**
 * 加载指定游戏的全部 Mod 分组。
 * 对应后端命令：`load_mods`。
 * @param game 目标游戏（可选，不传则使用后端设置中的当前游戏）
 * @returns 分组数据数组
 */
export async function invokeLoadMods(game?: TargetGame): Promise<ModGroupData[]> {
  return invoke('load_mods', { game });
}

/**
 * 刷新 Mod 列表（重新读取文件系统），用于文件监听触发或手动刷新。
 * 对应后端命令：`refresh_mods`。
 * @param game 目标游戏（可选，不传则使用后端设置中的当前游戏）
 * @returns 最新分组数据数组
 */
export async function invokeRefreshMods(game?: TargetGame): Promise<ModGroupData[]> {
  return invoke('refresh_mods', { game });
}

/**
 * 刷新单个分组的模组列表。
 * 对应后端命令：`refresh_single_group`。
 * @param groupPath 分组目录路径
 * @returns 更新后的分组数据（仅包含最新的 mods，保留原有 children）
 */
export async function invokeRefreshSingleGroup(groupPath: string): Promise<ModGroupData> {
  return invoke('refresh_single_group', { groupPath });
}

/**
 * 加载指定路径的 INI 文件内容到后端缓存。
 * 对应后端命令：`load_ini`。
 * @param path INI 文件绝对路径
 */
export async function invokeLoadIni(path: string): Promise<void> {
  return invoke('load_ini', { path });
}

/**
 * 将后端缓存的 INI 内容保存到指定路径。
 * 对应后端命令：`save_ini`。
 * @param path 目标 INI 文件绝对路径
 */
export async function invokeSaveIni(path: string): Promise<void> {
  return invoke('save_ini', { path });
}

/**
 * 查找指定模组目录下的所有 .ini 文件。
 * 对应后端命令：`find_ini_files`。
 * @param path 模组目录绝对路径
 * @returns INI 文件绝对路径数组
 */
export async function invokeFindModIniFiles(path: string): Promise<string[]> {
  return invoke('find_ini_files', { path });
}

/**
 * 加载指定 INI 文件并返回完整数据结构。
 * 对应后端命令：`load_ini`。
 * @param path INI 文件绝对路径
 * @returns INI 文件数据结构
 */
export async function invokeLoadIniData(path: string): Promise<IniFileData> {
  return invoke('load_ini', { path });
}

/**
 * 启动对指定目录的文件监听。
 * 对应后端命令：`start_file_watcher`。
 * @param path 被监听目录绝对路径
 */
export async function invokeStartFileWatcher(path: string): Promise<void> {
  return invoke('start_file_watcher', { path });
}

/**
 * 停止当前正在运行的文件监听。
 * 对应后端命令：`stop_file_watcher`。
 */
export async function invokeStopFileWatcher(): Promise<void> {
  return invoke('stop_file_watcher');
}

/**
 * 注册全局键盘/手柄热键。
 * 对应后端命令：`register_hotkey`。
 * @param key 热键标识字符串
 */
export async function invokeRegisterHotkey(key: string): Promise<void> {
  return invoke('register_hotkey', { key });
}

/**
 * 注销已注册的全局热键。
 * 对应后端命令：`unregister_hotkey`。
 * @param key 热键标识字符串
 */
export async function invokeUnregisterHotkey(key: string): Promise<void> {
  return invoke('unregister_hotkey', { key });
}

/**
 * 显示主窗口。
 * 对应后端命令：`show_window`。
 */
export async function invokeShowWindow(): Promise<void> {
  return invoke('show_window');
}

/**
 * 隐藏主窗口。
 * 对应后端命令：`hide_window`。
 */
export async function invokeHideWindow(): Promise<void> {
  return invoke('hide_window');
}

/**
 * 切换主窗口显隐状态。
 * 对应后端命令：`toggle_window`。
 */
export async function invokeToggleWindow(): Promise<void> {
  return invoke('toggle_window');
}

/**
 * 初始化/设置系统托盘菜单。
 * 对应后端命令：`setup_tray`。
 */
export async function invokeSetupTray(): Promise<void> {
  return invoke('setup_tray');
}

/**
 * 查询指定进程是否正在运行。
 * 对应后端命令：`is_process_running`。
 * @param processName 进程名（含扩展名）
 * @returns 进程是否运行中
 */
export async function invokeIsProcessRunning(processName: string): Promise<boolean> {
  return invoke('is_process_running', { processName });
}

/**
 * 获取当前系统正在运行的进程名列表。
 * 对应后端命令：`get_process_list`。
 * @returns 进程名数组
 */
export async function invokeGetProcessList(): Promise<string[]> {
  return invoke('get_process_list');
}

/**
 * 读取持久化的应用设置。
 * 对应后端命令：`get_settings`。
 * @returns 应用设置对象
 */
export async function invokeGetSettings(): Promise<AppSettings> {
  return invoke('get_settings');
}

/**
 * 保存应用设置到持久化存储。
 * 对应后端命令：`save_settings`。
 * @param settings 应用设置对象
 */
export async function invokeSaveSettings(settings: AppSettings): Promise<void> {
  return invoke('save_settings', { settings });
}

/**
 * 从远端拉取云端数据并写入本地缓存。
 * 对应后端命令：`fetch_cloud_data`。
 */
export async function invokeFetchCloudData(): Promise<void> {
  return invoke('fetch_cloud_data');
}

/**
 * 将本地缓存与远端云端数据同步。
 * 对应后端命令：`sync_cloud_data`。
 */
export async function invokeSyncCloudData(): Promise<void> {
  return invoke('sync_cloud_data');
}

/**
 * 模拟单次按键。
 * 对应后端命令：`simulate_key_press`。
 * @param key 按键标识字符串
 */
export async function invokeSimulateKeyPress(key: string): Promise<void> {
  return invoke('simulate_key_press', { key });
}

/**
 * 模拟组合按键（如 Ctrl+C）。
 * 对应后端命令：`simulate_key_combination`。
 * @param keys 按键标识数组，按顺序按下
 */
export async function invokeSimulateKeyCombination(keys: string[]): Promise<void> {
  return invoke('simulate_key_combination', { keys });
}

/**
 * 校验指定游戏的 Mods 目录是否合法。
 * 对应后端命令：`validate_mods_path`。
 * @param path 待校验的 Mods 目录绝对路径
 * @param game 目标游戏
 * @returns 校验状态枚举值
 */
export async function invokeValidateModsPath(path: string, game: TargetGame): Promise<ModsPathStatus> {
  return invoke('validate_mods_path', { path, game });
}

/**
 * 获取指定游戏的全部 Mod 分组。
 * 对应后端命令：`get_mod_groups`。
 * @param game 目标游戏
 * @returns 分组数据数组
 */
export async function invokeGetModGroups(game: TargetGame): Promise<ModGroupData[]> {
  return invoke('get_mod_groups', { game });
}

/**
 * 获取指定分组下的全部 Mod。
 * 对应后端命令：`get_mods_in_group`。
 * @param groupPath 分组绝对路径
 * @returns Mod 数据数组
 */
export async function invokeGetModsInGroup(groupPath: string): Promise<ModData[]> {
  return invoke('get_mods_in_group', { groupPath });
}

/**
 * 更新/刷新指定游戏的 Mod 数据（重新扫描并重建缓存）。
 * 对应后端命令：`update_mod_data`。
 * @param game 目标游戏
 * @returns 操作结果（含成功标志、最新分组列表、可选错误信息）
 */
export async function invokeUpdateModData(game: TargetGame): Promise<UpdateModDataResult> {
  return invoke('update_mod_data', { game });
}

/**
 * 切换指定 Mod 的收藏状态。
 * 对应后端命令：`toggle_mod_favorite`。
 * @param modPath Mod 绝对路径
 * @returns 切换后是否处于收藏状态
 */
export async function invokeToggleModFavorite(modPath: string): Promise<boolean> {
  return invoke('toggle_mod_favorite', { modPath });
}

/**
 * 切换指定分组的收藏状态。
 * 对应后端命令：`toggle_group_favorite`。
 * @param groupPath 分组绝对路径
 * @returns 切换后是否处于收藏状态
 */
export async function invokeToggleGroupFavorite(groupPath: string): Promise<boolean> {
  return invoke('toggle_group_favorite', { groupPath });
}

/**
 * 在指定位置新增一个分组。
 * 对应后端命令：`add_group`。
 * @param groupName 新分组的显示名称
 * @param targetGroupPath 目标分组路径（可选）。指定后新分组将与该分组处于同一目录层级
 * @returns 新建分组在列表中的索引，失败时为 null
 */
export async function invokeAddGroup(groupName: string, targetGroupPath?: string): Promise<number | null> {
  return invoke('add_group', { groupName, targetGroupPath });
}

/**
 * 删除指定分组。
 * 对应后端命令：`remove_group`。
 * @param groupPath 分组绝对路径
 * @returns 是否删除成功
 */
export async function invokeRemoveGroup(groupPath: string): Promise<boolean> {
  return invoke('remove_group', { groupPath });
}

/**
 * 移除单个模组（先还原再移动到 DISABLED_MANAGED_REMOVED）。
 * 对应后端命令：`remove_mod`。
 */
export async function invokeRemoveMod(modPath: string): Promise<void> {
  return invoke('remove_mod', { modPath });
}

/**
 * 重命名指定分组。
 * 对应后端命令：`rename_group`。
 * @param groupPath 分组绝对路径
 * @param newName 新分组名称
 * @returns 是否重命名成功
 */
export async function invokeRenameGroup(groupPath: string, newName: string): Promise<boolean> {
  return invoke('rename_group', { groupPath, newName });
}

/**
 * 重命名指定模组。
 * 对应后端命令：`rename_mod`。
 * @param modPath 模组绝对路径
 * @param newName 新模组名称（不含 DISABLED 前缀）
 * @returns 是否重命名成功
 */
export async function invokeRenameMod(modPath: string, newName: string): Promise<boolean> {
  return invoke('rename_mod', { modPath, newName });
}

/**
 * 按给定顺序重排分组。
 * 对应后端命令：`reorder_groups`。
 * @param groupPaths 分组路径的有序数组
 * @returns 是否重排成功
 */
export async function invokeReorderGroups(groupPaths: string[]): Promise<boolean> {
  return invoke('reorder_groups', { groupPaths });
}

/**
 * 在指定游戏中按关键字搜索 Mod。
 * 对应后端命令：`search_mods`。
 * @param keyword 搜索关键字
 * @param game 目标游戏
 * @returns 命中的 Mod 数据数组
 */
export async function invokeSearchMods(keyword: string, game: TargetGame): Promise<ModData[]> {
  return invoke('search_mods', { keyword, game });
}

/**
 * 切换指定 Mod 的禁用状态。
 * 对应后端命令：`toggle_mod_disabled`。
 * @param modPath Mod 绝对路径
 * @returns 切换后是否处于禁用状态
 */
export async function invokeToggleModDisabled(modPath: string): Promise<boolean> {
  return invoke('toggle_mod_disabled', { modPath });
}

/**
 * 切换树节点（# 目录）下 Mod 的禁用状态（互斥模式）。
 * 启用时会先禁用同 # 目录下所有其他 Mod，再启用目标 Mod。
 * 禁用时直接禁用目标 Mod。
 * 对应后端命令：`toggle_tree_node_mod_disabled`。
 * @param modPath Mod 绝对路径
 * @returns [新模组路径, 切换后是否处于禁用状态]
 */
export async function invokeToggleTreeNodeModDisabled(modPath: string): Promise<[string, boolean]> {
  return invoke('toggle_tree_node_mod_disabled', { modPath });
}

/**
 * 安全禁用树节点（# 目录）下的指定 Mod 目录（仅添加 DISABLED 前缀，不切换）。
 * 若目标已处于禁用状态，则直接返回原路径。
 * 对应后端命令：`disable_tree_node_mod`。
 * @param modPath Mod 绝对路径
 * @returns 操作后的新模组路径
 */
export async function invokeDisableTreeNodeMod(modPath: string): Promise<string> {
  return invoke('disable_tree_node_mod', { modPath });
}

/**
 * 切换 # 目录分组的启用/禁用状态。
 * 对应后端命令：`toggle_tree_node_group_disabled`。
 * @param groupPath 分组绝对路径
 * @returns 切换后的禁用状态（true = 已禁用，false = 已启用）
 */
export async function invokeToggleTreeNodeGroupDisabled(groupPath: string): Promise<boolean> {
  return invoke('toggle_tree_node_group_disabled', { groupPath });
}

/**
 * 设置指定分组的当前选中 Mod 索引。
 * 对应后端命令：`set_selected_mod`。
 * @param groupPath 分组绝对路径
 * @param index 选中的 Mod 索引
 */
export async function invokeSetSelectedMod(groupPath: string, index: number): Promise<void> {
  return invoke('set_selected_mod', { groupPath, index });
}

/**
 * 读取指定分组的当前选中 Mod 索引。
 * 对应后端命令：`get_selected_mod`。
 * @param groupPath 分组绝对路径
 * @param modsCount 该分组下 Mod 数量（用于越界保护）
 * @returns 当前选中的 Mod 索引
 */
export async function invokeGetSelectedMod(groupPath: string, modsCount: number): Promise<number> {
  return invoke('get_selected_mod', { groupPath, modsCount });
}

/**
 * 设置指定 Managed 目录的当前选中分组索引。
 * 对应后端命令：`set_selected_group`。
 * @param managedPath Managed 目录绝对路径
 * @param index 选中的分组索引
 */
export async function invokeSetSelectedGroup(managedPath: string, index: number): Promise<void> {
  return invoke('set_selected_group', { managedPath, index });
}

/**
 * 读取指定 Managed 目录的当前选中分组索引。
 * 对应后端命令：`get_selected_group`。
 * @param managedPath Managed 目录绝对路径
 * @param groupCount 该目录下分组数量（用于越界保护）
 * @returns 当前选中的分组索引
 */
export async function invokeGetSelectedGroup(managedPath: string, groupCount: number): Promise<number> {
  return invoke('get_selected_group', { managedPath, groupCount });
}

/**
 * 获取指定 INI 文件的语法错误列表。
 * 对应后端命令：`get_ini_syntax_errors`。
 * @param iniPath INI 文件绝对路径
 * @returns 语法错误信息数组（无错误时为空数组）
 */
export async function invokeGetIniSyntaxErrors(iniPath: string): Promise<IniSyntaxError[]> {
  return invoke('get_ini_syntax_errors', { iniPath });
}

/**
 * 尝试自动修复指定 INI 文件的语法错误。
 * 对应后端命令：`fix_ini_syntax_errors`。
 * @param iniPath INI 文件绝对路径
 * @returns 是否修复成功
 */
export async function invokeFixIniSyntaxErrors(iniPath: string): Promise<boolean> {
  return invoke('fix_ini_syntax_errors', { iniPath });
}

/**
 * 获取主窗口的当前位置与尺寸。
 * 对应后端命令：`get_window_position`。
 * @returns 窗口位置与尺寸信息
 */
export async function invokeGetWindowPosition(): Promise<WindowPosition> {
  return invoke('get_window_position');
}

/**
 * 设置主窗口位置。
 * 对应后端命令：`set_window_position`。
 * @param x 窗口左上角 X 坐标
 * @param y 窗口左上角 Y 坐标
 */
export async function invokeSetWindowPosition(x: number, y: number): Promise<void> {
  return invoke('set_window_position', { x, y });
}

/**
 * 设置主窗口尺寸。
 * 对应后端命令：`set_window_size`。
 * @param width 窗口宽度
 * @param height 窗口高度
 */
export async function invokeSetWindowSize(width: number, height: number): Promise<void> {
  return invoke('set_window_size', { width, height });
}

/**
 * 设置主窗口是否置顶。
 * 对应后端命令：`pin_window`。
 * @param pinned 是否置顶
 */
export async function invokePinWindow(pinned: boolean): Promise<void> {
  return invoke('pin_window', { pinned });
}

/**
 * 查询主窗口是否处于置顶状态。
 * 对应后端命令：`is_window_pinned`。
 * @returns 是否置顶
 */
export async function invokeIsWindowPinned(): Promise<boolean> {
  return invoke('is_window_pinned');
}

/**
 * 设置系统托盘菜单内容。
 * 对应后端命令：`set_tray_menu`。
 * @param items 托盘菜单项数组
 */
export async function invokeSetTrayMenu(items: TrayMenuItem[]): Promise<void> {
  return invoke('set_tray_menu', { items });
}

/**
 * 设置系统托盘图标悬停提示。
 * 对应后端命令：`set_tray_tooltip`。
 * @param tooltip 提示文本
 */
export async function invokeSetTrayTooltip(tooltip: string): Promise<void> {
  return invoke('set_tray_tooltip', { tooltip });
}

/**
 * 获取已缓存的云端数据。
 * 对应后端命令：`get_cloud_data`。
 * @returns 云端数据聚合对象
 */
export async function invokeGetCloudData(): Promise<CloudData> {
  return invoke('get_cloud_data');
}

/**
 * 获取指定游戏的 Mods 目录路径。
 * 对应后端命令：`get_mods_path`。
 * @param game 目标游戏
 * @returns Mods 目录绝对路径
 */
export async function invokeGetModsPath(game: TargetGame): Promise<string> {
  return invoke('get_mods_path', { game });
}

/**
 * 设置指定游戏的 Mods 目录路径。
 * 对应后端命令：`set_mods_path`。
 * @param game 目标游戏
 * @param path Mods 目录绝对路径
 */
export async function invokeSetModsPath(game: TargetGame, path: string): Promise<void> {
  return invoke('set_mods_path', { game, path });
}

/**
 * 弹出系统目录选择对话框并返回所选目录。
 * 对应后端命令：`select_directory`。
 * @returns 选中目录绝对路径，用户取消时为 null
 */
export async function invokeSelectDirectory(): Promise<string | null> {
  return invoke('select_directory');
}

/**
 * 在系统默认程序中打开指定路径（文件或目录）。
 * 对应后端命令：`open_path`。
 * @param path 待打开路径
 */
export async function invokeOpenPath(path: string): Promise<void> {
  return invoke('open_path', { path });
}

/**
 * 打开指定游戏的 Mods 根目录。
 * 对应后端命令：`open_mod_folder`。
 * @param game 目标游戏
 */
export async function invokeOpenModFolder(game: TargetGame): Promise<void> {
  return invoke('open_mod_folder', { game });
}

/**
 * 构造一份带默认值的 AppSettings 对象。
 * 默认值来源于 CONSTANTS 中对应的 default* 字段；
 * 窗口坐标默认为 null（表示由系统决定），各游戏 Mods 路径默认为空字符串（需用户后续配置）。
 * @returns 默认应用设置对象
 */
export function getDefaultSettings(): AppSettings {
  return {
    hotkeyKeyboard: CONSTANTS.defaultHotkeyKeyboard as HotkeyKeyboard,
    hotkeyGamepad: CONSTANTS.defaultHotkeyGamepad as HotkeyGamepad,
    searchHotkey: 'altF',
    targetProcessWuwa: CONSTANTS.defaultTargetProcesses.wuwa,
    targetProcessGenshin: CONSTANTS.defaultTargetProcesses.genshin,
    targetProcessHsr: CONSTANTS.defaultTargetProcesses.hsr,
    targetProcessZzz: CONSTANTS.defaultTargetProcesses.zzz,
    targetProcessEndfield: CONSTANTS.defaultTargetProcesses.endfield,
    overallScale: CONSTANTS.defaultOverallScale,
    bgTransparency: CONSTANTS.defaultBgTransparency,
    layoutMode: CONSTANTS.defaultLayoutMode,
    language: CONSTANTS.defaultLanguage,
    isAutoGenerateFolderIcon: CONSTANTS.defaultAutoGenerateFolderIcon,
    isAutoPinWindow: CONSTANTS.defaultAutoPinWindow,
    showMenuWhenTogglingOutsideGame: CONSTANTS.defaultShowMenuWhenTogglingOutsideGame,
    keybindSimulateKeypress: CONSTANTS.defaultKeybindSimulateKeypress,
    sortGroupMethod: CONSTANTS.defaultSortGroupMethod,
    savedWindowWidth: CONSTANTS.defaultWindowWidth,
    savedWindowHeight: CONSTANTS.defaultWindowHeight,
    savedWindowX: null,
    savedWindowY: null,
    theme: CONSTANTS.defaultTheme,
    targetGame: TargetGame.Wuthering_Waves,
    modsPathWuwa: '',
    modsPathGenshin: '',
    modsPathHsr: '',
    modsPathZzz: '',
    modsPathEndfield: ''
  };
}

/**
 * 从指定路径添加 Mod（复制到目标分组目录）。
 * 对应后端命令：`add_mods`。
 * @param sourcePaths 源文件/目录路径列表
 * @param targetGroupPath 目标分组目录路径
 * @returns 是否添加成功
 */
export async function invokeAddMods(sourcePaths: string[], targetGroupPath: string): Promise<boolean> {
  return invoke('add_mods', { sourcePaths, targetGroupPath });
}

/**
 * 使用BFS算法查找指定路径下的所有.ini文件。
 * 对应后端命令：`find_ini_files`。
 * @param path 起始路径（文件或目录）
 * @returns .ini文件路径列表
 */
export async function invokeFindIniFiles(path: string): Promise<string[]> {
  return invoke('find_ini_files', { path });
}

/**
 * 处理.ini文件，移除xxmi专属ini语句。
 * 对应后端命令：`process_ini_files`。
 * @param paths .ini文件路径列表
 * @returns 是否处理成功
 */
export async function invokeProcessIniFiles(paths: string[]): Promise<boolean> {
  return invoke('process_ini_files', { paths });
}

/**
 * 验证压缩文件的有效性。
 * 对应后端命令：`validate_archive_file`。
 * @param path 文件路径
 * @returns (是否有效, 文件类型字符串: "zip"/"7z"/"rar"/"unknown")
 */
export async function invokeValidateArchiveFile(path: string): Promise<[boolean, string]> {
  return invoke('validate_archive_file', { path });
}

/**
 * 使用BFS算法递归查找目录下所有文件。
 * 对应后端命令：`find_all_files`。
 * @param path 起始目录路径
 * @returns 目录下所有文件的路径列表
 */
export async function invokeFindAllFiles(path: string): Promise<string[]> {
  return invoke('find_all_files', { path });
}

/**
 * 解压压缩文件到指定目录（自动识别文件类型）。
 * 对应后端命令：`extract_archive`。
 * @param file_path 压缩文件路径
 * @param dest_dir 目标目录路径
 * @returns 是否解压成功
 */
export async function invokeExtractArchive(file_path: string, dest_dir: string): Promise<boolean> {
  return invoke('extract_archive', { file_path, dest_dir });
}

/**
 * 导出单个模组为7z压缩文件。
 * 对应后端命令：`export_mod`。
 * @param mod_path 模组目录路径
 * @param dest_dir 目标目录路径
 * @returns 导出文件的完整路径
 */
export async function invokeExportMod(mod_path: string, dest_dir: string): Promise<string> {
  return invoke('export_mod', { mod_path, dest_dir });
}

/**
 * 导出分组模组为7z压缩文件（保持目录结构）。
 * 对应后端命令：`export_group`。
 * @param group_path 分组目录路径
 * @param dest_dir 目标目录路径
 * @returns 导出文件的完整路径
 */
export async function invokeExportGroup(group_path: string, dest_dir: string): Promise<string> {
  return invoke('export_group', { group_path, dest_dir });
}

/**
 * 在系统默认浏览器中打开指定 URL。
 * 对应后端命令：`open_url`。
 * @param url 要打开的 URL（必须包含协议，如 `https://`）
 */
export async function invokeOpenUrl(url: string): Promise<void> {
  return invoke('open_url', { url });
}

/** 创建桌面快捷方式（Linux 为 .desktop，Windows 为 .lnk）。 */
export async function invokeCreateDesktopIcon(name?: string): Promise<void> {
  return invoke('create_desktop_icon', { name });
}

/**
 * 独立执行 Hash 冲突检测。
 *
 * 调用后端的 `check_hash_conflicts` Tauri 命令：
 * - 通过 `TaskQueue` 任务类型 `"check_hash_conflicts"` 互斥执行。
 * - 同类型并发时新请求会取消旧请求。
 * - 与 `update_mod_data` 互不阻塞（不同任务类型）。
 *
 * @returns `HashConflictReport`（包含 `enabledModHashes` 与 `conflicts` 字段）。
 *          若任务被取消则返回 rejected Promise，错误信息以 `Task 'check_hash_conflicts' was cancelled` 开头。
 */
export async function invokeCheckHashConflicts(): Promise<HashConflictReport> {
  return invoke<HashConflictReport>('check_hash_conflicts');
}
