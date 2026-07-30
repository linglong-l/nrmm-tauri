/**
 * Tauri后端命令封装层
 *
 * 所有前端与Rust后端的交互都通过此模块统一封装
 * 使用@tauri-apps/api/core的invoke调用后端命令
 *
 * 命名约定：
 * - 函数名使用camelCase，对应后端snake_case命令名
 * - 参数名与后端Rust函数参数保持一致
 */
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { revealItemInDir } from '@tauri-apps/plugin-opener'
import type { ScanResult, UpdateResult, SaveCustomizationsResult, RestoredCount } from '../types'

/**
 * 打开文件夹选择对话框
 * 后端命令：N/A（使用Tauri dialog插件）
 * @param defaultPath 对话框默认打开路径
 * @returns 选择的文件夹路径，取消则返回null
 */
export async function selectFolder(defaultPath?: string): Promise<string | null> {
  return open({
    directory: true,
    multiple: false,
    defaultPath,
    title: '选择模组路径',
  }) as Promise<string | null>
}

/**
 * 获取应用设置
 * 后端命令：get_settings
 * @returns 应用设置对象
 */
export async function getSettings(): Promise<any> {
  return invoke('get_settings')
}

/**
 * 保存应用设置
 * 后端命令：save_settings
 * @param settings 要保存的设置对象
 */
export async function saveSettings(settings: any): Promise<void> {
  return invoke('save_settings', { settings })
}

/**
 * 重置应用设置为默认值
 * 后端命令：reset_settings
 */
export async function resetSettings(): Promise<any> {
  return invoke('reset_settings')
}

/**
 * 加载模组列表（轻量扫描）
 * 后端命令：get_mods
 *
 * 轻量扫描：仅读取目录结构和基础元数据，不深度解析INI
 * @param game 目标游戏类型
 * @param modsPath 模组文件夹路径
 * @returns 扫描结果（分组列表+模组列表）
 */
export async function getMods(game: string, modsPath: string): Promise<ScanResult> {
  return invoke('get_mods', { game, modsPath })
}

/**
 * 刷新模组列表（轻量扫描，文件变化后调用）
 * 后端命令：refresh_mods
 *
 * 相比getMods更轻量，复用后端缓存
 * @param game 目标游戏类型
 * @param modsPath 模组文件夹路径
 * @returns 扫描结果
 */
export async function refreshMods(game: string, modsPath: string): Promise<ScanResult> {
  return invoke('refresh_mods', { game, modsPath })
}

/**
 * 选中模组（写入INI，处理互斥组逻辑）
 * 后端命令：select_mod
 *
 * 互斥组逻辑：如果是互斥组，会自动禁用同组其他模组
 * @param game 目标游戏类型
 * @param modsPath 模组文件夹路径
 * @param groupIndex 分组索引
 * @param modIndex 模组索引
 * @param isMutex 是否为互斥组
 * @param modPath 模组文件夹路径
 */
export async function selectMod(game: string, modsPath: string, groupIndex: number, modIndex: number, isMutex: boolean, modPath: string): Promise<any> {
  return invoke('select_mod', { game, modsPath, groupIndex, modIndex, isMutex, modPath })
}

/**
 * 取消选中分组内模组
 * 后端命令：deselect_group_mod
 * @param game 目标游戏类型
 * @param modsPath 模组文件夹路径
 * @param groupIndex 分组索引
 */
export async function deselectGroupMod(game: string, modsPath: string, groupIndex: number): Promise<any> {
  return invoke('deselect_group_mod', { game, modsPath, groupIndex })
}

/**
 * 添加新分组
 * 后端命令：add_group
 * @param modsPath 模组文件夹路径
 * @param game 目标游戏类型
 * @param groupName 分组名称（可选，默认自动生成）
 */
export async function addGroup(modsPath: string, game: string, groupName?: string): Promise<any> {
  return invoke('add_group', { modsPath, game, groupName })
}

/**
 * 删除分组
 * 后端命令：remove_group
 * @param groupPath 分组文件夹路径
 */
export async function removeGroup(groupPath: string): Promise<void> {
  return invoke('remove_group', { groupPath })
}

/**
 * 删除模组（移入回收站）
 * 后端命令：remove_mod
 * @param modPath 模组文件夹路径
 */
export async function removeMod(modPath: string): Promise<void> {
  return invoke('remove_mod', { modPath })
}

/**
 * 重命名模组
 * 后端命令：rename_mod
 * @param modPath 模组文件夹路径
 * @param newName 新名称
 * @returns 重命名后的模组名称
 */
export async function renameMod(modPath: string, newName: string): Promise<string> {
  return invoke('rename_mod', { modPath, newName })
}

/**
 * 重命名分组
 * 后端命令：rename_group
 * @param groupPath 分组文件夹路径
 * @param newName 新名称
 * @returns 重命名后的分组名称
 */
export async function renameGroup(groupPath: string, newName: string): Promise<string> {
  return invoke('rename_group', { groupPath, newName })
}

/**
 * 切换模组启用/禁用状态
 * 后端命令：toggle_mod_disabled
 * @param modPath 模组文件夹路径
 * @param enable true=启用, false=禁用
 * @param isMutex 是否为互斥组成员
 */
export async function toggleModDisabled(modPath: string, enable: boolean, isMutex: boolean): Promise<void> {
  return invoke('toggle_mod_disabled', { modPath, enable, isMutex })
}

/**
 * 切换模组收藏状态
 * 后端命令：toggle_favorite
 * @param modPath 模组文件夹路径
 * @returns 切换后的收藏状态
 */
export async function toggleFavorite(modPath: string): Promise<boolean> {
  return invoke('toggle_favorite', { modPath })
}

/**
 * 检查模组是否已收藏
 * 后端命令：is_favorite
 * @param modPath 模组文件夹路径
 * @returns 是否已收藏
 */
export async function isFavorite(modPath: string): Promise<boolean> {
  return invoke('is_favorite', { modPath })
}

/**
 * 在文件管理器中打开模组文件夹
 * 使用tauri-plugin-opener的revealItemInDir在文件管理器中显示并定位文件夹
 * @param modPath 模组文件夹路径
 */
export async function openModFolder(modPath: string): Promise<void> {
  return revealItemInDir(modPath)
}

/**
 * 在文件管理器中打开分组文件夹
 * 使用tauri-plugin-opener的revealItemInDir在文件管理器中显示并定位文件夹
 * @param groupPath 分组文件夹路径
 */
export async function openGroupFolder(groupPath: string): Promise<void> {
  return revealItemInDir(groupPath)
}

/**
 * 恢复所有INI文件到原始状态
 * 后端命令：restore_all_inis
 * @param modsPath 模组文件夹路径
 * @returns 恢复统计结果
 */
export async function restoreAllInis(modsPath: string): Promise<RestoredCount> {
  return invoke('restore_all_inis', { modsPath })
}

/**
 * Save Mod Customizations：保存用户自定义INI设置到d3dx_user.ini
 * 后端命令：save_customizations
 * @param game 目标游戏类型
 * @param modsPath 模组文件夹路径
 * @returns 保存结果
 */
export async function saveCustomizations(game: string, modsPath: string): Promise<SaveCustomizationsResult> {
  return invoke('save_customizations', { game, modsPath })
}

/**
 * 批量切换模组启用/禁用状态
 * 后端命令：batch_toggle_mods
 * @param modPaths 模组路径列表
 * @param enable true=启用, false=禁用
 * @param isMutex 是否为互斥组成员
 * @returns 成功处理的模组数量
 */
export async function batchToggleMods(modPaths: string[], enable: boolean, isMutex: boolean): Promise<number> {
  return invoke('batch_toggle_mods', { modPaths, enable, isMutex })
}

/**
 * 检查模组路径状态
 * 后端命令：check_mods_path_status
 * @param game 目标游戏类型
 * @param modsPath 模组文件夹路径
 * @returns 路径状态：normal/empty/noAccess/notSet
 */
export async function checkModsPathStatus(game: string, modsPath: string): Promise<string> {
  return invoke('check_mods_path_status', { game, modsPath })
}

/**
 * 检查按键模拟支持状态
 * 后端命令：check_keypress_support
 *
 * Linux检查xdotool/ydotool可用性
 * macOS检查辅助功能权限
 */
export async function checkKeypressSupport(): Promise<void> {
  return invoke('check_keypress_support')
}

/**
 * 检查游戏窗口是否在前台
 * 后端命令：is_game_foreground
 * @param game 目标游戏类型
 * @returns 是否在前台
 */
export async function isGameForeground(game: string): Promise<boolean> {
  return invoke('is_game_foreground', { game })
}

/**
 * 获取当前光标位置
 * 后端命令：get_cursor_position
 * @returns [x, y] 坐标元组
 */
export async function getCursorPosition(): Promise<[number, number]> {
  return invoke('get_cursor_position')
}

/**
 * 模拟按键选择分组
 * 后端命令：simulate_select_group
 */
export async function simulateSelectGroup(): Promise<void> {
  return invoke('simulate_select_group')
}

/**
 * 模拟按键选择模组
 * 后端命令：simulate_select_mod
 */
export async function simulateSelectMod(): Promise<void> {
  return invoke('simulate_select_mod')
}

/**
 * 显示窗口
 * 后端命令：show_window
 * @param windowName 窗口名称（默认为'main'）
 */
export async function showWindow(windowName: string): Promise<void> {
  return invoke('show_window', { windowName })
}

/**
 * 隐藏窗口
 * 后端命令：hide_window
 * @param windowName 窗口名称
 */
export async function hideWindow(windowName: string): Promise<void> {
  return invoke('hide_window', { windowName })
}

/**
 * 关闭窗口
 * 后端命令：close_window
 * @param windowName 窗口名称
 */
export async function closeWindow(windowName: string): Promise<void> {
  return invoke('close_window', { windowName })
}

/**
 * 最小化窗口
 * 后端命令：minimize_window
 * @param windowName 窗口名称
 */
export async function minimizeWindow(windowName: string): Promise<void> {
  return invoke('minimize_window', { windowName })
}

/**
 * 切换窗口最大化/还原状态
 * 后端命令：toggle_maximize
 * @param windowName 窗口名称
 */
export async function toggleMaximize(windowName: string): Promise<void> {
  return invoke('toggle_maximize', { windowName })
}

/**
 * 重新注册全局热键
 * 后端命令：reregister_hotkeys
 *
 * 修改热键配置后调用，热键会即时生效无需重启
 */
export async function reregisterHotkeys(): Promise<void> {
  return invoke('reregister_hotkeys')
}

/**
 * 注销所有全局热键
 * 后端命令：unregister_hotkeys
 */
export async function unregisterHotkeys(): Promise<void> {
  return invoke('unregister_hotkeys')
}

/**
 * 启动文件监听器
 * 后端命令：start_file_watcher
 *
 * 监听Mods目录变化，自动刷新模组列表
 * @param modsPath 要监听的模组路径
 */
export async function startFileWatcher(modsPath: string): Promise<void> {
  return invoke('start_file_watcher', { modsPath })
}

/**
 * 停止文件监听器
 * 后端命令：stop_file_watcher
 */
export async function stopFileWatcher(): Promise<void> {
  return invoke('stop_file_watcher')
}

/**
 * 切换文件监听器到新路径
 * 后端命令：switch_file_watcher
 *
 * 切换游戏时调用，先停止旧路径监听再启动新路径
 * @param modsPath 新的模组路径
 */
export async function switchFileWatcher(modsPath: string): Promise<void> {
  return invoke('switch_file_watcher', { modsPath })
}

/**
 * 更新模组数据（重量级操作）
 * 后端命令：update_mod_data
 *
 * 重量级操作：完整解析所有INI，检测/修复错误，处理互斥组
 * 相比轻量扫描(getMods/refreshMods)耗时更长
 * @param game 目标游戏类型
 * @param modsPath 模组文件夹路径
 * @returns 更新结果统计
 */
export async function updateModData(game: string, modsPath: string): Promise<UpdateResult> {
  return invoke('update_mod_data', { game, modsPath })
}

/**
 * 检查压缩包格式是否支持
 * 后端命令：is_supported_archive_cmd
 *
 * 支持格式：zip/rar/7z
 * @param path 压缩包路径
 * @returns 是否支持
 */
export async function isSupportedArchive(path: string): Promise<boolean> {
  return invoke('is_supported_archive_cmd', { path })
}

/**
 * 自动导入模组压缩包
 * 后端命令：import_mod_auto_cmd
 *
 * 自动解压并安装到当前选中分组
 * @param archivePath 压缩包路径
 * @param modsPath 模组根路径
 * @param password 压缩包密码（可选）
 */
export async function importModAuto(archivePath: string, modsPath: string, password?: string): Promise<any> {
  return invoke('import_mod_auto_cmd', { archivePath, modsPath, password })
}

/**
 * 检查应用更新
 * 后端命令：check_for_updates
 *
 * 从Gitee/GitHub Release检查最新版本
 * @returns 更新信息（有新版本时返回版本信息，无则返回null/false）
 */
export async function checkForUpdates(): Promise<any> {
  return invoke('check_for_updates')
}

/**
 * 获取应用版本号
 * 后端命令：get_app_version
 * @returns 版本号字符串（如"1.0.0"）
 */
export async function getAppVersion(): Promise<string> {
  return invoke('get_app_version')
}

/**
 * 比较版本号
 * 后端命令：compare_versions
 * @param current 当前版本
 * @param latest 最新版本
 * @returns latest是否大于current
 */
export async function compareVersions(current: string, latest: string): Promise<boolean> {
  return invoke('compare_versions', { current, latest })
}

/**
 * 获取平台信息
 * 后端命令：get_platform_info
 *
 * 返回OS类型、按键模拟支持、前台检测支持等
 * @returns JSON序列化的PlatformInfo字符串
 */
export async function getPlatformInfo(): Promise<string> {
  return invoke('get_platform_info')
}

/**
 * 重置窗口位置到屏幕中央
 * 后端命令：reset_window_position
 * @param windowName 窗口名称，默认为'main'
 */
export async function resetWindowPosition(windowName = 'main'): Promise<void> {
  return invoke('reset_window_position', { windowName })
}

/**
 * 切换目标游戏
 * 后端命令：switch_target_game
 *
 * 切换后会触发target-game-switched事件
 * @param game 目标游戏类型
 */
export async function switchTargetGame(game: any): Promise<void> {
  return invoke('switch_target_game', { game })
}

/**
 * 强制退出应用
 * 后端命令：hard_quit_app
 *
 * 不保存状态直接退出进程
 */
export async function hardQuitApp(): Promise<void> {
  return invoke('hard_quit_app')
}

/**
 * 校验子文件夹名称合法性
 * 后端命令：validate_subfolder_name
 * @param parentPath 父目录路径
 * @param folderName 待校验的文件夹名称
 * @returns [name: 清理后的名称, valid: 是否合法, message: 错误信息]
 */
export async function validateSubfolderName(parentPath: string, folderName: string): Promise<[string, boolean, string]> {
  return invoke('validate_subfolder_name', { parentPath, folderName })
}

/**
 * 创建子文件夹
 * 后端命令：create_subfolder
 * @param parentPath 父目录路径
 * @param folderName 文件夹名称（应先通过validateSubfolderName校验）
 */
export async function createSubfolder(parentPath: string, folderName: string): Promise<void> {
  return invoke('create_subfolder', { parentPath, folderName })
}
