/**
 * 模组状态管理Store
 *
 * 核心概念：
 * - 轻量扫描(loadMods/refresh)：快速读取目录结构和基础元数据，不深度解析INI内容
 * - 重量级更新(updateModData)：完整解析所有INI文件，检测/修复错误，处理互斥组逻辑
 * - 互斥组(mutexGroup)：同一分组内同时只能启用一个模组（类似单选按钮）
 * - 缓存策略：groups/mods数据保存在内存中，通过文件监听器自动刷新
 * - 生命周期：页面挂载时启动监听+加载，卸载时停止监听+清空数据
 * - 树状导航：groups是顶层列表（NormalGroup + MutexGroup根节点），子分组通过children递归访问
 */
import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { getMods, refreshMods, selectMod, switchFileWatcher, stopFileWatcher, updateModData as tauriUpdateModData } from '../utils/tauri'
import { useSettingsStore } from './settings'
import type { ModGroupData, ModData, TargetGame } from '../types'
import { logger } from '../utils/logger'

export const useModsStore = defineStore('mods', () => {
  /** 模组分组列表（顶层：NormalGroup + MutexGroup根节点） */
  const groups = ref<ModGroupData[]>([])
  /** 所有模组的扁平列表（包含所有分组的模组） */
  const mods = ref<ModData[]>([])
  /** 是否正在加载模组数据 */
  const loading = ref(false)
  /** 当前选中的分组路径（用于树状导航，唯一标识） */
  const selectedGroupPath = ref<string>('')
  /** 当前选中的模组索引（在分组内） */
  const selectedModIndex = ref<number>(0)
  /** 搜索关键词 */
  const searchQuery = ref('')
  /** 是否仅显示收藏模组 */
  const showFavoritesOnly = ref(false)
  /** 按游戏存储是否需要更新模组数据（提示条显示状态） */
  const needUpdatePerGame = ref<Record<TargetGame, boolean>>({} as Record<TargetGame, boolean>)

  /**
   * 递归在分组树中查找指定路径的分组
   * @param groupList 分组列表（顶层或子分组列表）
   * @param path 目标分组路径
   * @returns 找到的分组或null
   */
  function findGroupByPathInList(groupList: ModGroupData[], path: string): ModGroupData | null {
    for (const g of groupList) {
      if (g.groupPath === path) {
        return g
      }
      if (g.children && g.children.length > 0) {
        const found = findGroupByPathInList(g.children, path)
        if (found) return found
      }
    }
    return null
  }

  /**
   * 递归查找分组所在的顶层根分组索引
   * @param groupList 分组列表
   * @param path 目标分组路径
   * @param topLevelIndex 当前顶层索引
   * @returns 顶层根分组索引，-1表示未找到
   */
  function findRootGroupIndex(groupList: ModGroupData[], path: string, topLevelIndex: number = -1): number {
    for (let i = 0; i < groupList.length; i++) {
      const g = groupList[i]
      const currentTopIndex = topLevelIndex === -1 ? i : topLevelIndex
      if (g.groupPath === path) {
        return currentTopIndex
      }
      if (g.children && g.children.length > 0) {
        const found = findRootGroupIndex(g.children, path, currentTopIndex)
        if (found >= 0) return found
      }
    }
    return -1
  }

  /**
   * 递归获取分组及其所有子分组的模组列表（用于MutexGroup显示）
   * @param group 分组
   * @returns 该分组及其所有子分组的模组
   */
  function collectModsRecursive(group: ModGroupData): ModData[] {
    let result = [...group.mods]
    if (group.children && group.children.length > 0) {
      for (const child of group.children) {
        result = result.concat(collectModsRecursive(child))
      }
    }
    return result
  }

  /** 当前选中的分组对象（通过路径递归查找） */
  const currentGroup = computed<ModGroupData | null>(() => {
    if (!selectedGroupPath.value) return groups.value[0] || null
    return findGroupByPathInList(groups.value, selectedGroupPath.value)
  })

  /** 当前选中分组所在的顶层根分组索引（用于后端select_mod调用） */
  const selectedGroupRootIndex = computed<number>(() => {
    if (!selectedGroupPath.value) return 0
    return findRootGroupIndex(groups.value, selectedGroupPath.value)
  })

  /** 当前选中的模组对象（显示用：如果是子分组，从子分组mods中取） */
  const selectedMod = computed<ModData | null>(() => {
    const g = currentGroup.value
    if (!g) return null
    return g.mods[selectedModIndex.value] || null
  })

  /** 当前分组显示的模组列表（MutexGroup包含子分组模组，NormalGroup仅直接子模组） */
  const currentGroupMods = computed<ModData[]>(() => {
    const g = currentGroup.value
    if (!g) return []
    // MutexGroup递归收集所有子分组模组；NormalGroup（groupType=customParallel/exclusiveSlot）仅直接子模组
    if (g.groupType === 'mutexGroup') {
      return collectModsRecursive(g)
    }
    return g.mods
  })

  /** 当前游戏是否需要更新模组数据（控制提示条显示） */
  const needUpdate = computed<boolean>(() => {
    const s = useSettingsStore()
    return needUpdatePerGame.value[s.currentGame] || false
  })

  /**
   * 标记当前游戏需要更新模组数据
   * 仅在非互斥组（普通group_int分组）的模组启用/禁用操作后调用
   * @param game 目标游戏
   */
  function markNeedUpdate(game?: TargetGame) {
    const s = useSettingsStore()
    const targetGame = game || s.currentGame
    needUpdatePerGame.value = {
      ...needUpdatePerGame.value,
      [targetGame]: true
    }
  }

  /**
   * 清除当前游戏的需要更新状态
   * 在updateModData成功完成后或用户关闭提示条时调用
   * @param game 目标游戏
   */
  function clearNeedUpdate(game?: TargetGame) {
    const s = useSettingsStore()
    const targetGame = game || s.currentGame
    needUpdatePerGame.value = {
      ...needUpdatePerGame.value,
      [targetGame]: false
    }
  }

  /**
   * 过滤后的模组列表（用于搜索和收藏过滤）
   * 注意：此过滤仅作用于扁平mods列表，不影响分组视图
   */
  const filteredMods = computed(() => {
    let result = mods.value
    if (searchQuery.value) {
      const q = searchQuery.value.toLowerCase()
      result = result.filter((m) => {
        const name = (m.modName || '').toLowerCase()
        return name.includes(q)
      })
    }
    if (showFavoritesOnly.value) {
      result = result.filter((m) => m.isFavorite)
    }
    return result
  })

  /**
   * 选中分组
   * @param group 要选中的分组对象
   */
  function selectGroup(group: ModGroupData) {
    selectedGroupPath.value = group.groupPath
    selectedModIndex.value = 0
    showFavoritesOnly.value = false
  }

  /**
   * 加载模组列表（轻量扫描）
   *
   * 与refresh的区别：
   * - loadMods：首次加载或切换游戏时调用，完整重建分组树
   * - refresh：文件变化后调用，增量更新，性能更优
   *
   * 轻量扫描特点：
   * - 仅读取目录结构和modname/selectedindex文件
   * - 不深度解析merged.ini内容
   * - 快速响应，UI立即显示
   */
  async function loadMods() {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    loading.value = true
    try {
      const result = await getMods(s.currentGame, s.currentModsPath)
      groups.value = result.groups || []
      mods.value = result.mods || []
      // 默认选中第一个分组
      if (groups.value.length > 0 && !selectedGroupPath.value) {
        selectedGroupPath.value = groups.value[0].groupPath
      } else if (selectedGroupPath.value) {
        // 检查之前选中的分组是否还存在，不存在则选第一个
        const found = findGroupByPathInList(groups.value, selectedGroupPath.value)
        if (!found && groups.value.length > 0) {
          selectedGroupPath.value = groups.value[0].groupPath
        }
      }
    } catch (e) {
      logger.error('ModsStore', 'Failed to load mods', e)
    } finally {
      loading.value = false
    }
  }

  /**
   * 刷新模组列表（轻量扫描，用于文件变化后）
   *
   * 触发场景：
   * - 文件监听器检测到Mods目录变化
   * - 拖拽导入模组完成后
   * - 增删改模组/分组操作后
   *
   * 相比loadMods更轻量，复用后端缓存
   */
  async function refresh() {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    loading.value = true
    try {
      const result = await refreshMods(s.currentGame, s.currentModsPath)
      groups.value = result.groups || []
      mods.value = result.mods || []
      // 刷新后检查选中分组是否存在
      if (selectedGroupPath.value) {
        const found = findGroupByPathInList(groups.value, selectedGroupPath.value)
        if (!found && groups.value.length > 0) {
          selectedGroupPath.value = groups.value[0].groupPath
        }
      } else if (groups.value.length > 0) {
        selectedGroupPath.value = groups.value[0].groupPath
      }
    } catch (e) {
      logger.error('ModsStore', 'Failed to refresh mods', e)
    } finally {
      loading.value = false
    }
  }

  /**
   * 选中模组（写入INI，处理互斥组逻辑）
   *
   * 核心业务逻辑：
   * - 互斥组(mutexGroup)：选中一个模组时自动禁用同组其他模组（基于路径）
   * - 并行组：允许多个模组同时启用（基于顶层分组索引）
   * - 写入selectedindex文件和INI配置
   * - 操作完成后自动refresh刷新UI状态
   *
   * @param modIdx 模组在当前分组内的索引
   */
  async function selectModByIndex(modIdx: number) {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    const group = currentGroup.value
    if (!group) return

    try {
      const isMutex = group.groupType === 'mutexGroup'
      const mod = group.mods[modIdx]
      const modPath = mod?.modPath || ''
      const groupIdx = selectedGroupRootIndex.value

      // 调用后端select_mod命令处理INI写入和互斥逻辑
      await selectMod(s.currentGame, s.currentModsPath, groupIdx, modIdx, isMutex, modPath)
      // 更新前端选中状态
      selectedModIndex.value = modIdx
      // 非互斥组（普通group_int分组）操作后标记需要更新模组数据
      if (!isMutex) {
        markNeedUpdate()
      }
      // 刷新以获取最新状态（互斥组可能改变了其他模组的isActive）
      await refresh()
    } catch (e) {
      logger.error('ModsStore', 'Failed to select mod', e)
    }
  }

  /**
   * 清空所有模组数据
   * 用于切换游戏或页面卸载时重置状态
   */
  function clearData() {
    groups.value = []
    mods.value = []
    loading.value = false
    selectedGroupPath.value = ''
    selectedModIndex.value = 0
    searchQuery.value = ''
    showFavoritesOnly.value = false
  }

  /**
   * 启动文件监听器
   * 监听Mods目录变化，自动触发refresh
   */
  async function startWatching() {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    try {
      await switchFileWatcher(s.currentModsPath)
    } catch (e) {
      logger.warn('ModsStore', 'Failed to start file watcher', e)
    }
  }

  /** 停止文件监听器 */
  async function stopWatching() {
    try {
      await stopFileWatcher()
    } catch (e) {
      logger.warn('ModsStore', 'Failed to stop file watcher', e)
    }
  }

  /**
   * 更新模组数据（重量级操作）
   *
   * 与轻量扫描(loadMods/refresh)的区别：
   * 1. 深度解析：完整解析所有merged.ini文件内容
   * 2. 错误修复：自动检测并修复missingEndif、namespace冲突等错误
   * 3. 互斥组处理：重新计算互斥组内模组的启用/禁用状态
   * 4. 耗时较长：可能需要数秒，有loading状态提示
   * 5. 触发时机：用户手动点击"Update Mod Data"按钮
   *
   * 适用场景：
   * - 手动在文件管理器中增删改模组后
   * - 模组出现错误需要修复时
   * - 首次配置游戏路径后
   */
  async function updateModData() {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    loading.value = true
    try {
      await tauriUpdateModData(s.currentGame, s.currentModsPath)
      // 重量级更新后需要完整加载以获取最新解析结果
      await loadMods()
      // 更新成功后清除当前游戏的needUpdate状态
      clearNeedUpdate()
    } catch (e) {
      logger.error('ModsStore', 'Failed to update mod data', e)
      throw e
    } finally {
      loading.value = false
    }
  }

  return {
    groups,
    mods,
    loading,
    selectedGroupPath,
    selectedGroupRootIndex,
    selectedModIndex,
    searchQuery,
    showFavoritesOnly,
    needUpdate,
    needUpdatePerGame,
    currentGroup,
    currentGroupMods,
    selectedMod,
    filteredMods,
    selectGroup,
    loadMods,
    refresh,
    selectModByIndex,
    clearData,
    startWatching,
    stopWatching,
    updateModData,
    markNeedUpdate,
    clearNeedUpdate,
    findGroupByPathInList,
  }
})
