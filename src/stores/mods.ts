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
import { ref, computed, reactive } from 'vue'
import { getMods, refreshMods, selectMod, switchFileWatcher, stopFileWatcher, updateModData as tauriUpdateModData, updateGroupModData } from '../utils/tauri'
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
  /**
   * 每个分组独立记录的上次选中模组索引（对应 Flutter 版本的 previousSelectedModOnGroup）
   * key 为 groupPath（目录绝对路径），value 为该分组在 currentGroupMods 中的模组索引
   * 虚拟节点路径（__all__/__fav__/__groups__）不记录
   */
  const selectedModIndicesByGroup: Record<string, number> = reactive({})
  /** 搜索关键词 */
  const searchQuery = ref('')
  /** 是否仅显示收藏模组 */
  const showFavoritesOnly = ref(false)
  /** 按游戏存储是否需要更新模组数据（提示条显示状态） */
  const needUpdatePerGame = ref<Record<TargetGame, boolean>>({} as Record<TargetGame, boolean>)
  /** 按分组索引存储是否需要更新模组数据（分组级标记，用于增量更新） */
  const needUpdatePerGroup = ref<Record<number, boolean>>({})

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
   * 递归查找目标分组所在的顶层根分组索引
   * 
   * @param groupList 分组列表
   * @param path 目标分组路径
   * @param topLevelIndex 当前顶层索引（递归时传递，用于定位根分组）
   * @returns 顶层根分组索引，未找到返回-1
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
    console.debug(group.groupPath, result)
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
   * 仅在非互斥组（普通group_xx分组）的模组操作后调用
   * @param groupIndex 可选的分组索引，传入时记录分组级标记用于增量更新
   */
  function markNeedUpdate(groupIndex?: number) {
    const s = useSettingsStore()
    // 游戏级标记
    needUpdatePerGame.value = {
      ...needUpdatePerGame.value,
      [s.currentGame]: true
    }
    // 分组级标记（用于增量更新）
    if (groupIndex !== undefined) {
      needUpdatePerGroup.value = {
        ...needUpdatePerGroup.value,
        [groupIndex]: true
      }
    }
  }

  /**
   * 清除当前游戏的需要更新状态
   * 在updateModData成功完成后或用户关闭提示条时调用
   */
  function clearNeedUpdate() {
    const s = useSettingsStore()
    needUpdatePerGame.value = {
      ...needUpdatePerGame.value,
      [s.currentGame]: false
    }
    // 清除所有分组级标记
    needUpdatePerGroup.value = {}
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
    // 切换分组前，先保存当前分组的选中索引（仅对真实分组，不保存虚拟节点）
    const prevPath = selectedGroupPath.value
    if (prevPath && !prevPath.startsWith('__') && prevPath !== '__groups__') {
      selectedModIndicesByGroup[prevPath] = selectedModIndex.value
    }

    selectedGroupPath.value = group.groupPath
    showFavoritesOnly.value = false

    // 恢复目标分组的选中索引：优先使用内存记录，其次使用后端返回的 activeModIndex，最后默认 0
    const path = group.groupPath
    if (path.startsWith('__') || path === '__groups__') {
      selectedModIndex.value = 0
    } else if (selectedModIndicesByGroup[path] !== undefined) {
      selectedModIndex.value = selectedModIndicesByGroup[path]
    } else if (group.activeModIndex >= 0) {
      selectedModIndex.value = group.activeModIndex
    } else {
      selectedModIndex.value = 0
    }
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
   *
   * @returns Promise<void> 加载完成后，groups 和 mods 状态已更新；加载失败时由内部 catch 记录日志，不抛出异常
   */
  async function loadMods() {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    loading.value = true
    try {
      const result = await getMods(s.currentGame, s.currentModsPath)
      groups.value = result.groups || []
      mods.value = result.mods || []

      // 清空之前的分组选中记录，用后端返回的 activeModIndex 初始化
      for (const key of Object.keys(selectedModIndicesByGroup)) {
        delete selectedModIndicesByGroup[key]
      }
      for (const g of groups.value) {
        initGroupSelectedIndex(g)
      }

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

      // 根据当前选中分组设置 selectedModIndex
      const curGroup = currentGroup.value
      if (curGroup) {
        const path = curGroup.groupPath
        if (path.startsWith('__') || path === '__groups__') {
          selectedModIndex.value = 0
        } else if (selectedModIndicesByGroup[path] !== undefined) {
          selectedModIndex.value = selectedModIndicesByGroup[path]
        } else {
          selectedModIndex.value = 0
        }
      }
    } catch (e) {
      logger.error('ModsStore', 'Failed to load mods', e)
    } finally {
      loading.value = false
    }
  }

  /**
   * 递归初始化分组及其子分组的选中索引记录
   * 仅对 normalGroup 使用 activeModIndex 初始化
   */
  function initGroupSelectedIndex(group: ModGroupData) {
    if (group.groupType === 'normalGroup' && group.activeModIndex >= 0) {
      selectedModIndicesByGroup[group.groupPath] = group.activeModIndex
    }
    if (group.children && group.children.length > 0) {
      for (const child of group.children) {
        initGroupSelectedIndex(child)
      }
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
   *
   * @returns Promise<void> 刷新完成后，groups 和 mods 状态已增量更新；选中分组路径自动校验，失效时回退到第一个分组
   */
  async function refresh() {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    loading.value = true
    try {
      const result = await refreshMods(s.currentGame, s.currentModsPath)
      groups.value = result.groups || []
      mods.value = result.mods || []

      // 更新选中索引记录：保留会话内的用户选择，用新的 activeModIndex 补充未记录的分组
      for (const g of groups.value) {
        refreshGroupSelectedIndex(g)
      }

      // 刷新后检查选中分组是否存在
      if (selectedGroupPath.value) {
        const found = findGroupByPathInList(groups.value, selectedGroupPath.value)
        if (!found && groups.value.length > 0) {
          selectedGroupPath.value = groups.value[0].groupPath
        }
      } else if (groups.value.length > 0) {
        selectedGroupPath.value = groups.value[0].groupPath
      }

      // 确保当前选中索引在有效范围内（防止模组被删除后越界）
      const currentMods = currentGroupMods.value
      if (selectedModIndex.value >= currentMods.length) {
        selectedModIndex.value = Math.max(0, currentMods.length - 1)
        const curPath = selectedGroupPath.value
        if (curPath && !curPath.startsWith('__') && curPath !== '__groups__') {
          selectedModIndicesByGroup[curPath] = selectedModIndex.value
        }
      }
    } catch (e) {
      logger.error('ModsStore', 'Failed to refresh mods', e)
    } finally {
      loading.value = false
    }
  }

  /**
   * 递归刷新分组选中索引记录
   * 保留已有记录（用户会话内选择优先），仅补充未记录的 normalGroup 初始值
   */
  function refreshGroupSelectedIndex(group: ModGroupData) {
    if (group.groupType === 'normalGroup' && selectedModIndicesByGroup[group.groupPath] === undefined && group.activeModIndex >= 0) {
      selectedModIndicesByGroup[group.groupPath] = group.activeModIndex
    }
    if (group.children && group.children.length > 0) {
      for (const child of group.children) {
        refreshGroupSelectedIndex(child)
      }
    }
  }

  /**
   * 高亮选中模组（仅UI，不写入INI、不刷新、不弹提示）
   * 用于单击卡片时的视觉反馈
   * @param modIdx 模组在当前显示列表中的索引
   */
  function highlightMod(modIdx: number) {
    selectedModIndex.value = modIdx
    const g = currentGroup.value
    if (g) {
      const path = g.groupPath
      if (!path.startsWith('__') && path !== '__groups__') {
        selectedModIndicesByGroup[path] = modIdx
      }
    }
  }

  /**
   * 启用/选中模组（写入INI，处理互斥组逻辑，乐观更新UI）
   *
   * 核心业务逻辑：
   * - 调用后端select_mod命令写入selectedindex文件和INI配置
   * - 互斥组(mutexGroup)：选中一个模组时自动禁用同组其他模组
   * - 乐观更新：不刷新列表，直接在本地更新isActive状态
   * - 不触发"需要更新模组数据"提示条
   *
   * @param modIdx 模组在当前显示列表中的索引
   */
  async function activateModByIndex(modIdx: number) {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    const group = currentGroup.value
    if (!group) return

    try {
      const isMutex = group.groupType === 'mutexGroup'
      // 从当前显示的模组列表中获取目标模组（MutexGroup使用递归收集后的列表）
      const displayedMods = currentGroupMods.value
      const mod = displayedMods[modIdx]
      const modPath = mod?.modPath || ''
      const groupIdx = selectedGroupRootIndex.value

      // 调用后端select_mod命令处理INI写入和互斥逻辑
      await selectMod(s.currentGame, s.currentModsPath, groupIdx, modIdx, isMutex, modPath)
      // 标记需要更新模组数据（仅NormalGroup操作）
      if (!isMutex) {
        markNeedUpdate(groupIdx)
      }
      // 更新前端选中状态
      selectedModIndex.value = modIdx
      // 同步记录到分组索引映射（对应 Flutter 的 previousSelectedModOnGroup）
      const gPath = group.groupPath
      if (!gPath.startsWith('__') && gPath !== '__groups__') {
        selectedModIndicesByGroup[gPath] = modIdx
      }
      // 乐观更新：递归更新分组树中所有模组的isActive状态
      // 互斥组和普通组都是互斥语义：同一时间只有一个模组为active
      function setActiveRecursive(g: ModGroupData, targetPath: string) {
        for (const m of g.mods) {
          m.isActive = m.modPath === targetPath
        }
        for (const child of g.children) {
          setActiveRecursive(child, targetPath)
        }
      }
      setActiveRecursive(group, modPath)
    } catch (e) {
      logger.error('ModsStore', 'Failed to activate mod', e)
    }
  }

  /**
   * 选中模组（写入INI，处理互斥组逻辑）
   * @deprecated 请使用 activateModByIndex 代替（双击启用）或 highlightMod（单击高亮）
   */
  async function selectModByIndex(modIdx: number) {
    await activateModByIndex(modIdx)
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
    // 清空分组选中记录
    for (const key of Object.keys(selectedModIndicesByGroup)) {
      delete selectedModIndicesByGroup[key]
    }
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
   * 更新策略：
   * - 当 needUpdatePerGroup 中存在分组标记时 → 仅更新指定分组（分组增量更新）
   * - 当 needUpdatePerGroup 为空（无分组标记）→ 执行全量更新（update_mod_data）
   * - 更新完成后清除所有标记和提醒条
   *
   * @returns Promise<void> 更新完成后自动调用 loadMods() 刷新前端数据；失败时抛出异常
   */
  async function updateModData() {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    loading.value = true
    try {
      const groupIndices = Object.keys(needUpdatePerGroup.value)
        .filter(k => needUpdatePerGroup.value[Number(k)])
        .map(Number)

      if (groupIndices.length > 0) {
        // 有分组标记 → 分组增量更新
        for (const groupIndex of groupIndices) {
          await updateGroupModData(s.currentGame, s.currentModsPath, groupIndex)
        }
      } else {
        // 无分组标记 → 全量更新
        await tauriUpdateModData(s.currentGame, s.currentModsPath)
      }

      // 更新完成后刷新前端数据并清除所有标记
      await loadMods()
      clearNeedUpdate()
    } catch (e) {
      logger.error('ModsStore', 'Failed to update mod data', e)
      throw e
    } finally {
      loading.value = false
    }
  }

  /**
   * 判断缓存中是否已有模组数据
   *
   * Phase 4 新增：用于页面切换时的 cache 判断
   * 在 ModsView.onMounted 中优先检查此方法，缓存不为空时跳过 loadMods 调用，避免重复加载
   * 在 App.vue window-shown 监听中配合 checkModCacheValid 后端命令一起判断缓存有效性
   *
   * @returns {boolean} groups 或 mods 数组非空时返回 true
   */
  function hasData(): boolean {
    return groups.value.length > 0 || mods.value.length > 0
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
    needUpdatePerGroup,
    currentGroup,
    currentGroupMods,
    selectedMod,
    filteredMods,
    selectGroup,
    loadMods,
    refresh,
    selectModByIndex,
    highlightMod,
    activateModByIndex,
    clearData,
    startWatching,
    stopWatching,
    updateModData,
    markNeedUpdate,
    clearNeedUpdate,
    findGroupByPathInList,
    hasData,
  }
})
