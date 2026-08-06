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
import type { ScanResult, UpdateResult, SaveCustomizationsResult, RestoredCount, RemoveModResult, HashConflictResult } from '../types'
import { useModsStore } from '@/stores/mods'
import { useSettingsStore } from '@/stores/settings'
import { logger } from './logger'

/**
 * 后端返回的路径不存在错误前缀标识
 * 捕获到带此前缀的错误时需执行：清除模组缓存 → 模组重读取
 */
export const ERR_PREFIX_PATH_NOT_FOUND = '[PATH_NOT_FOUND]'

/**
 * 检测错误是否为路径不存在类型，是则清除缓存并重读模组
 * 调用流程：打开文件资源管理器 → 路径不存在异常 → 清除模组缓存数据 → 模组重读取
 *
 * @param error invoke抛出的原始错误对象（含message属性）或字符串
 * @returns 若为路径不存在错误返回true（已处理完刷新），否则返回false（继续抛出原错误）
 */
export async function handlePathNotFoundError(error: unknown): Promise<boolean> {
  const msg = typeof error === 'string'
    ? error
    : (error as any)?.message ?? String(error ?? '')

  if (!msg.includes(ERR_PREFIX_PATH_NOT_FOUND)) {
    return false
  }
  logger.warn('tauri', 'Path not found while opening folder, refreshing mods data', msg)
  try {
    const modsStore = useModsStore()
    const settingsStore = useSettingsStore()
    await modsStore.stopWatching()
    modsStore.clearData()
    // clearData 后 currentGroup 会变成 null，因此需要用 settingsStore.currentModsPath 判断是否能加载
    if (settingsStore.currentModsPath) {
      await modsStore.startWatching()
      await modsStore.loadMods()
    }
  } catch (e) {
    logger.error('tauri', 'Failed to refresh mods after path-not-found error', e)
  }
  return true
}

/**
 * 统一 Tauri invoke 调用中间件
 *
 * 所有后端命令调用均通过此函数统一封装，提供以下保障：
 * - 记录调用日志（debug 级别）：命令名 + 参数摘要
 * - 捕获并记录错误（error 级别）：包含命令名和错误详情
 * - 自动处理路径不存在错误：当 options.suppressPathNotFound 为 true 时，
 *   自动调用 handlePathNotFoundError 执行缓存清除和模组重读取
 * - 统一错误格式：将字符串错误包装为 Error 对象
 *
 * @param cmd 后端命令名（snake_case，如 'get_settings'）
 * @param args 传给后端的参数对象（camelCase 键名）
 * @param options 可选配置：suppressPathNotFound=true 时自动处理路径不存在错误
 * @returns 后端命令的返回值
 * @throws 当后端返回错误且未被自动处理时，抛出 Error 对象
 */
async function safeInvoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
  options?: { suppressPathNotFound?: boolean }
): Promise<T> {
  logger.debug('Tauri', `invoke: ${cmd}`, args)
  try {
    return await invoke<T>(cmd, args)
  } catch (error) {
    const errObj = error instanceof Error ? error : new Error(String(error))
    logger.error('Tauri', `invoke failed: ${cmd}`, errObj)

    if (options?.suppressPathNotFound) {
      const handled = await handlePathNotFoundError(error)
      if (handled) {
        return undefined as unknown as T
      }
    }

    throw errObj
  }
}

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
  return safeInvoke('get_settings')
}

/**
 * 保存应用设置
 * 后端命令：save_settings
 * @param settings 要保存的设置对象
 */
export async function saveSettings(settings: any): Promise<void> {
  return safeInvoke('save_settings', { settings })
}

/**
 * 重置应用设置为默认值
 * 后端命令：reset_settings
 */
export async function resetSettings(): Promise<any> {
  return safeInvoke('reset_settings')
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
  return safeInvoke<ScanResult>('get_mods', { game, modsPath })
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
  return safeInvoke<ScanResult>('refresh_mods', { game, modsPath })
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
 * @param groupPath 分组文件夹路径（用于分组级防抖）
 * @param modPath 模组文件夹路径
 */
export async function selectMod(
  game: string,
  modsPath: string,
  groupIndex: number,
  modIndex: number,
  isMutex: boolean,
  groupPath: string,
  modPath: string,
  cursorX?: number,
  cursorY?: number
): Promise<UpdateResult> {
  // 后端 select_mod 命令参数已重构为 SelectModArgs 结构体（修复 clippy::too_many_arguments），
  // Tauri 要求将结构体字段整体嵌套在 args 键下传递，故此处需包裹一层 args。
  const result = await safeInvoke<UpdateResult>('select_mod', {
    args: {
      game,
      modsPath,
      groupIndex,
      modIndex,
      isMutex,
      groupPath,
      modPath,
      cursorX: cursorX ?? null,
      cursorY: cursorY ?? null,
    },
  })
  return result
}

/**
 * 取消选中分组内模组
 * 后端命令：deselect_group_mod
 * @param game 目标游戏类型
 * @param modsPath 模组文件夹路径
 * @param groupIndex 分组索引
 */
export async function deselectGroupMod(game: string, modsPath: string, groupIndex: number): Promise<UpdateResult> {
  return safeInvoke<UpdateResult>('deselect_group_mod', { game, modsPath, groupIndex })
}

/**
 * 添加新分组
 * 后端命令：add_group
 * @param modsPath 模组文件夹路径
 * @param game 目标游戏类型
 * @param groupName 分组名称（可选，默认自动生成）
 */
export async function addGroup(modsPath: string, game: string, groupName?: string): Promise<any> {
  return safeInvoke('add_group', { modsPath, game, groupName })
}

/**
 * 删除分组
 * 后端命令：remove_group
 * @param groupPath 分组文件夹路径
 */
export async function removeGroup(groupPath: string): Promise<void> {
  return safeInvoke('remove_group', { groupPath })
}

/**
 * 移除模组（NRMM 对齐：移至 Mods/DISABLED_MANAGED_REMOVED/ + 还原 INI）
 * 后端命令：remove_mod
 *
 * 流程：
 * 1. 将模组文件夹移动至 Mods/DISABLED_MANAGED_REMOVED/（冲突时追加 _1、_2…）
 * 2. 还原 INI：移除 NRMM 管理注释、变量声明、条件表达式、if/endif 块
 * 3. 清除模组缓存
 *
 * @param modPath 模组文件夹路径
 * @returns 移除结果（包含移动后路径、INI 还原统计等）
 */
export async function removeMod(modPath: string): Promise<RemoveModResult> {
  return safeInvoke<RemoveModResult>('remove_mod', { modPath })
}

/**
 * 重命名模组
 * 后端命令：rename_mod
 * @param modPath 模组文件夹路径
 * @param newName 新名称
 * @returns 重命名后的模组名称
 */
export async function renameMod(modPath: string, newName: string): Promise<string> {
  return safeInvoke<string>('rename_mod', { modPath, newName })
}

/**
 * 重命名分组
 * 后端命令：rename_group
 * @param groupPath 分组文件夹路径
 * @param newName 新名称
 * @param isGroupXx 是否为 group_xx 格式的普通分组
 * @returns 重命名后的分组名称
 */
export async function renameGroup(groupPath: string, newName: string, isGroupXx: boolean): Promise<string> {
  return safeInvoke<string>('rename_group', { groupPath, newName, isGroupXx })
}

/**
 * 切换模组启用/禁用状态
 * 后端命令：toggle_mod_disabled
 * @param modPath 模组文件夹路径
 * @param enable true=启用, false=禁用
 * @param isMutex 是否为互斥组成员
 */
export async function toggleModDisabled(modPath: string, enable: boolean, isMutex: boolean): Promise<void> {
  return safeInvoke('toggle_mod_disabled', { modPath, enable, isMutex })
}

/**
 * 切换模组收藏状态
 * 后端命令：toggle_favorite
 * @param modPath 模组文件夹路径
 * @returns 切换后的收藏状态
 */
export async function toggleFavorite(modPath: string): Promise<boolean> {
  return safeInvoke<boolean>('toggle_favorite', { modPath })
}

/**
 * 检查模组是否已收藏
 * 后端命令：is_favorite
 * @param modPath 模组文件夹路径
 * @returns 是否已收藏
 */
export async function isFavorite(modPath: string): Promise<boolean> {
  return safeInvoke<boolean>('is_favorite', { modPath })
}

/**
 * 在文件管理器中打开模组文件夹
 * 后端命令：open_mod_folder
 * @param modPath 模组文件夹路径
 */
export async function openModFolder(modPath: string): Promise<void> {
  return safeInvoke('open_mod_folder', { modPath }, { suppressPathNotFound: true })
}

/**
 * 在文件管理器中打开分组文件夹
 * 后端命令：open_group_folder
 * @param groupPath 分组文件夹路径
 */
export async function openGroupFolder(groupPath: string): Promise<void> {
  return safeInvoke('open_group_folder', { groupPath }, { suppressPathNotFound: true })
}

/**
 * 恢复所有INI文件到原始状态
 * 后端命令：restore_all_inis
 * @param modsPath 模组文件夹路径
 * @returns 恢复统计结果
 */
export async function restoreAllInis(modsPath: string): Promise<RestoredCount> {
  return safeInvoke<RestoredCount>('restore_all_inis', { modsPath })
}

/**
 * Save Mod Customizations：保存用户自定义INI设置到d3dx_user.ini
 * 后端命令：save_customizations
 * @param game 目标游戏类型
 * @param modsPath 模组文件夹路径
 * @returns 保存结果
 */
export async function saveCustomizations(game: string, modsPath: string): Promise<SaveCustomizationsResult> {
  return safeInvoke<SaveCustomizationsResult>('save_customizations', { game, modsPath })
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
  return safeInvoke<number>('batch_toggle_mods', { modPaths, enable, isMutex })
}

/**
 * 检查模组路径状态
 * 后端命令：check_mods_path_status
 * @param game 目标游戏类型
 * @param modsPath 模组文件夹路径
 * @returns 路径状态：normal/empty/noAccess/notSet
 */
export async function checkModsPathStatus(game: string, modsPath: string): Promise<string> {
  return safeInvoke<string>('check_mods_path_status', { game, modsPath })
}

/**
 * 检查按键模拟支持状态
 * 后端命令：check_keypress_support
 *
 * Linux检查xdotool/ydotool可用性
 * macOS检查辅助功能权限
 */
export async function checkKeypressSupport(): Promise<void> {
  return safeInvoke('check_keypress_support')
}

/**
 * 检查游戏窗口是否在前台
 * 后端命令：is_game_foreground
 * @param game 目标游戏类型
 * @returns 是否在前台
 */
export async function isGameForeground(game: string): Promise<boolean> {
  return safeInvoke<boolean>('is_game_foreground', { game })
}

/**
 * 获取当前光标位置
 * 后端命令：get_cursor_position
 * @returns [x, y] 坐标元组
 */
export async function getCursorPosition(): Promise<[number, number]> {
  return safeInvoke<[number, number]>('get_cursor_position')
}

/**
 * 模拟按键选择分组
 * 后端命令：simulate_select_group
 */
export async function simulateSelectGroup(): Promise<void> {
  return safeInvoke('simulate_select_group')
}

/**
 * 模拟按键选择模组
 * 后端命令：simulate_select_mod
 */
export async function simulateSelectMod(): Promise<void> {
  return safeInvoke('simulate_select_mod')
}

/**
 * 模拟 F10 按键（3Dmigoto 重载快捷键）
 * 与 NRMM 的 simulateKeyF10() 对齐
 * @param game 目标游戏类型，传入时尝试定向发送到游戏窗口
 */
export async function simulateF10(game?: string): Promise<void> {
  const params: Record<string, unknown> = {}
  if (game) params.game = game
  await safeInvoke('simulate_f10', params)
}

/**
 * 显示窗口
 * 后端命令：show_window
 * @param windowName 窗口名称（默认为'main'）
 */
export async function showWindow(windowName: string): Promise<void> {
  return safeInvoke('show_window', { windowName })
}

/**
 * 隐藏窗口
 * 后端命令：hide_window
 * @param windowName 窗口名称
 */
export async function hideWindow(windowName: string): Promise<void> {
  return safeInvoke('hide_window', { windowName })
}

/**
 * 关闭窗口
 * 后端命令：close_window
 * @param windowName 窗口名称
 */
export async function closeWindow(windowName: string): Promise<void> {
  return safeInvoke('close_window', { windowName })
}

/**
 * 最小化窗口
 * 后端命令：minimize_window
 * @param windowName 窗口名称
 */
export async function minimizeWindow(windowName: string): Promise<void> {
  return safeInvoke('minimize_window', { windowName })
}

/**
 * 切换窗口最大化/还原状态
 * 后端命令：toggle_maximize
 * @param windowName 窗口名称
 */
export async function toggleMaximize(windowName: string): Promise<void> {
  return safeInvoke('toggle_maximize', { windowName })
}

/**
 * 重新注册全局热键
 * 后端命令：reregister_hotkeys
 *
 * 修改热键配置后调用，热键会即时生效无需重启
 * @param windowHotkey 可选的窗口切换热键，不传则从settings读取
 */
export async function reregisterHotkeys(windowHotkey?: string): Promise<void> {
  return safeInvoke('reregister_hotkeys', { windowHotkey })
}

/**
 * 注销所有全局热键
 * 后端命令：unregister_hotkeys
 */
export async function unregisterHotkeys(): Promise<void> {
  return safeInvoke('unregister_hotkeys')
}

/**
 * 启动文件监听器
 * 后端命令：start_file_watcher
 *
 * 监听Mods目录变化，自动刷新模组列表
 * @param modsPath 要监听的模组路径
 */
export async function startFileWatcher(modsPath: string): Promise<void> {
  return safeInvoke('start_file_watcher', { modsPath })
}

/**
 * 停止文件监听器
 * 后端命令：stop_file_watcher
 */
export async function stopFileWatcher(): Promise<void> {
  return safeInvoke('stop_file_watcher')
}

/**
 * 切换文件监听器到新路径
 * 后端命令：switch_file_watcher
 *
 * 切换游戏时调用，先停止旧路径监听再启动新路径
 * @param modsPath 新的模组路径
 */
export async function switchFileWatcher(modsPath: string): Promise<void> {
  return safeInvoke('switch_file_watcher', { modsPath })
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
  return safeInvoke<UpdateResult>('update_mod_data', { game, modsPath })
}

/**
 * 检测 hash 冲突（全量扫描，用户主动触发）
 * 后端命令：detect_hash_conflicts
 *
 * 扫描策略：
 * - NormalGroup（group_xx）：仅扫描当前选中模组的 INI
 * - MutexGroup（非 group_xx）：扫描所有启用模组的 INI
 *
 * @param modsPath 游戏 Mods 目录路径
 * @returns HashConflictResult 含冲突列表与扫描统计
 */
export async function detectHashConflicts(modsPath: string): Promise<HashConflictResult> {
  return safeInvoke<HashConflictResult>('detect_hash_conflicts', { modsPath })
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
  return safeInvoke<boolean>('is_supported_archive_cmd', { path })
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
  return safeInvoke('import_mod_auto_cmd', { archivePath, modsPath, password })
}

/**
 * 批量导入请求参数
 */
export interface ImportItemRequest {
  items: string[]
  targetGroupDir: string
  password?: string
}

/**
 * 批量导入模组（支持压缩包 zip/rar/7z 或已解压的目录混合导入）
 * 后端命令：import_item_cmd
 *
 * 导入时会自动暂停文件监控，完成后恢复
 * @param req 导入请求：items=文件/目录路径列表，targetGroupDir=目标分组目录，password=可选压缩包密码
 * @returns 每项的导入结果数组
 */
export async function importItems(req: ImportItemRequest): Promise<any[]> {
  return safeInvoke('import_item_cmd', { req })
}

/**
 * 检查应用更新
 * 后端命令：check_for_updates
 *
 * 从Gitee/GitHub Release检查最新版本
 * @returns 更新信息（有新版本时返回版本信息，无则返回null/false）
 */
export async function checkForUpdates(): Promise<any> {
  return safeInvoke('check_for_updates')
}

/**
 * 获取应用版本号
 * 后端命令：get_app_version
 * @returns 版本号字符串（如"1.0.0"）
 */
export async function getAppVersion(): Promise<string> {
  return safeInvoke<string>('get_app_version')
}

/**
 * 比较版本号
 * 后端命令：compare_versions
 * @param current 当前版本
 * @param latest 最新版本
 * @returns latest是否大于current
 */
export async function compareVersions(current: string, latest: string): Promise<boolean> {
  return safeInvoke<boolean>('compare_versions', { current, latest })
}

/**
 * 获取平台信息
 * 后端命令：get_platform_info
 *
 * 返回OS类型、按键模拟支持、前台检测支持等
 * @returns JSON序列化的PlatformInfo字符串
 */
export async function getPlatformInfo(): Promise<string> {
  return safeInvoke<string>('get_platform_info')
}

/**
 * 重置窗口位置到屏幕中央
 * 后端命令：reset_window_position
 * @param windowName 窗口名称，默认为'main'
 */
export async function resetWindowPosition(windowName = 'main'): Promise<void> {
  return safeInvoke('reset_window_position', { windowName })
}

/**
 * 切换目标游戏
 * 后端命令：switch_target_game
 *
 * 切换后会触发target-game-switched事件
 * @param game 目标游戏类型
 */
export async function switchTargetGame(game: any): Promise<void> {
  return safeInvoke('switch_target_game', { game })
}

/**
 * 强制退出应用
 * 后端命令：hard_quit_app
 *
 * 不保存状态直接退出进程
 */
export async function hardQuitApp(): Promise<void> {
  return safeInvoke('hard_quit_app')
}

/**
 * 校验子文件夹名称合法性
 * 后端命令：validate_subfolder_name
 * @param parentPath 父目录路径
 * @param folderName 待校验的文件夹名称
 * @returns [name: 清理后的名称, valid: 是否合法, message: 错误信息]
 */
export async function validateSubfolderName(parentPath: string, folderName: string): Promise<[string, boolean, string]> {
  return safeInvoke<[string, boolean, string]>('validate_subfolder_name', { parentPath, folderName })
}

/**
 * 创建子文件夹
 * 后端命令：create_subfolder
 * @param parentPath 父目录路径
 * @param folderName 文件夹名称（应先通过validateSubfolderName校验）
 */
export async function createSubfolder(parentPath: string, folderName: string): Promise<void> {
  return safeInvoke('create_subfolder', { parentPath, folderName })
}

/**
 * 获取当前前台窗口的进程名
 * 后端命令：get_foreground_process_name
 *
 * 用于窗口热键显示时自动检测前台游戏
 * @returns 进程名字符串（如 "StarRail.exe"）
 */
export async function getForegroundProcessName(): Promise<string> {
  return safeInvoke<string>('get_foreground_process_name')
}

/**
 * 检查模组缓存是否有效（未被文件监控标记失效且条目存在）
 * 后端命令：check_mod_cache_valid
 * @param game 目标游戏类型
 * @param modsPath 模组文件夹路径
 * @returns 缓存是否有效
 */
export async function checkModCacheValid(game: string, modsPath: string): Promise<boolean> {
  return safeInvoke<boolean>('check_mod_cache_valid', { game, modsPath })
}

/**
 * 检查文件监控是否正在运行
 * 后端命令：is_file_watcher_running
 * @returns 文件监控是否运行中
 */
export async function isFileWatcherRunning(): Promise<boolean> {
  return safeInvoke<boolean>('is_file_watcher_running')
}

/**
 * 获取当前文件监控的路径
 * 后端命令：current_watched_path
 * @returns 当前监控的路径，未运行则返回null
 */
export async function currentWatchedPath(): Promise<string | null> {
  return safeInvoke<string | null>('current_watched_path')
}

/**
 * 禁用指定分组下所有一级模组（叶子节点，含 .ini 的目录）
 * 子分组目录不受影响
 * 后端命令：disable_all_mods_in_group
 * @param groupPath 分组目录路径（绝对路径）
 * @returns 成功禁用的模组数量
 */
export async function disableAllModsInGroup(groupPath: string): Promise<number> {
  return safeInvoke<number>('disable_all_mods_in_group', { groupPath })
}

/**
 * 启用指定分组下所有一级禁用模组
 * 子分组目录不受影响
 * 后端命令：enable_all_mods_in_group
 * @param groupPath 分组目录路径（绝对路径）
 * @returns 成功启用的模组数量
 */
export async function enableAllModsInGroup(groupPath: string): Promise<number> {
  return safeInvoke<number>('enable_all_mods_in_group', { groupPath })
}

/**
 * 移除分组（NRMM 对齐：移至 Mods/_MANAGED_REMOVED_/ 目录）
 * - group_xx：直接移至 _MANAGED_REMOVED_
 * - 非group：先将一级子分组移至父级目录，再移自身至 _MANAGED_REMOVED_
 * 后端命令：remove_group_ex
 * @param groupPath 分组目录路径（绝对路径）
 * @param isGroupXx 是否为 group_xx 格式的普通分组
 */
export async function removeGroupEx(groupPath: string, isGroupXx: boolean): Promise<void> {
  return safeInvoke('remove_group_ex', { groupPath, isGroupXx })
}
