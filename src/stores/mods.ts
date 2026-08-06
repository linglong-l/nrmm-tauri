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
import { ref, computed, reactive, watch } from 'vue'
import { getMods, refreshMods, selectMod, switchFileWatcher, stopFileWatcher, updateModData as tauriUpdateModData, deselectGroupMod, disableAllModsInGroup } from '../utils/tauri'
import { useSettingsStore } from './settings'
import type { ModGroupData, ModData, TargetGame, UpdateResult } from '../types'
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
  /**
   * 按键绑定页的目标模组（对齐 NRMM modKeybindProvider）
   * - 当右键菜单「按键切换」设置该值时，selectedMod 计算属性优先返回此值
   * - KeybindsView 离开或 ModsView onMounted 时清空，避免跨分组残留
   */
  const keybindTargetMod = ref<ModData | null>(null)
  /** 搜索关键词 */
  const searchQuery = ref('')
  /** 是否仅显示收藏模组 */
  const showFavoritesOnly = ref(false)
  /** 按游戏存储是否需要更新模组数据（提示条显示状态） */
  const needUpdatePerGame = ref<Record<TargetGame, boolean>>({} as Record<TargetGame, boolean>)
  /** 按分组索引存储是否需要更新模组数据（分组级标记，用于增量更新） */
  const needUpdatePerGroup = ref<Record<number, boolean>>({})
  /** 是否需要用户手动重载（updateModData 结果中返回的 need_reload_manual 标记） */
  const needReloadManual = ref(false)
  /** 是否正在执行模组激活操作（双击防抖） */
  const isActivating = ref(false)
  /** 是否正在执行重量级更新模组数据（全局状态锁，true时阻止其他操作） */
  const isUpdatingModData = ref(false)
  /** 更新模组数据结果（保存最近一次的 UpdateResult） */
  const updateResult = ref<UpdateResult | null>(null)
  /** 分组级选择操作防抖：key=groupPath，value=Date.now() 时间戳（ms） */
  const lastSelectionTime = reactive<Record<string, number>>({})
  /** 防抖阈值：3000ms（3秒内同一分组不重复选择） */
  const SELECTION_DEBOUNCE_MS = 3000

  // ========== 全局搜索状态（Ctrl+F） ==========
  /** 搜索栏是否可见 */
  const searchVisible = ref(false)
  /** 命中的模组索引（在 mods.value 扁平列表中的下标） */
  const modMatchIndices = ref<number[]>([])
  /** 命中的分组 groupPath 列表 */
  const groupMatchPaths = ref<string[]>([])
  /** 当前聚焦命中项的全局索引（用于 Enter/Shift+Enter 导航），-1 表示无选中 */
  const currentGlobalMatchIndex = ref(-1)
  /** 需要自动展开的分组路径集合（命中分组的所有祖先路径） */
  const autoExpandGroupPaths = ref<Set<string>>(new Set())

  /**
   * 文本归一化：Unicode NFKC + 大小写 + 空白统一
   */
  function normalizeText(str: string): string {
    if (!str) return ''
    return str
      .normalize('NFKC')
      .replace(/\s|\u00A0|\u202F/g, '')
      .toLowerCase()
  }

  /**
   * 简化正确版本：找到 query 每个字符在 text 中按顺序出现的原始下标
   * 将连续命中的下标合并为 [start, end) 区间
   */
  function fuzzyMatchWithSpansSimple(text: string, query: string): { matched: boolean; spans: [number, number][] } {
    if (!query) return { matched: false, spans: [] }
    const normText = normalizeText(text)
    const normQuery = normalizeText(query)
    if (!normText || !normQuery) return { matched: false, spans: [] }

    // Step 1: 构造 textIdxToNormIdx 映射
    const textIdxToNormIdx: number[] = []
    let normPos = 0
    for (let i = 0; i < text.length; i++) {
      const origChar = text[i]
      const normChar = normalizeText(origChar)
      if (normChar) {
        textIdxToNormIdx.push(normPos)
        normPos += normChar.length
      } else {
        textIdxToNormIdx.push(-1)
      }
    }

    // Step 2: 在 normText 上双指针，记录命中的 normText 下标数组
    const hitNormIndices: number[] = []
    let q = 0
    for (let n = 0; n < normText.length && q < normQuery.length; n++) {
      if (normText[n] === normQuery[q]) {
        hitNormIndices.push(n)
        q++
      }
    }
    if (q < normQuery.length) return { matched: false, spans: [] }

    // Step 3: 将命中的 normText 下标映射回原 text 下标
    const hitTextIndices: number[] = []
    for (const hitNorm of hitNormIndices) {
      for (let ti = 0; ti < textIdxToNormIdx.length; ti++) {
        if (textIdxToNormIdx[ti] !== -1 && textIdxToNormIdx[ti] <= hitNorm && hitNorm < textIdxToNormIdx[ti] + 1) {
          hitTextIndices.push(ti)
          break
        }
      }
    }

    // Step 4: 合并连续下标为 [start, end) 区间
    const spans: [number, number][] = []
    if (hitTextIndices.length === 0) return { matched: true, spans: [] }
    let runStart = hitTextIndices[0]
    let prev = runStart
    for (let i = 1; i < hitTextIndices.length; i++) {
      const cur = hitTextIndices[i]
      if (cur === prev + 1) {
        prev = cur
      } else {
        spans.push([runStart, prev + 1])
        runStart = cur
        prev = cur
      }
    }
    spans.push([runStart, prev + 1])
    return { matched: true, spans }
  }

  /**
   * 递归遍历分组树，收集命中搜索词的分组路径及需要自动展开的祖先路径
   */
  function collectGroupMatchesRecursive(
    groupList: ModGroupData[],
    q: string,
    matchedPaths: Set<string>,
    expandPaths: Set<string>,
    ancestors: string[] = []
  ): void {
    for (const g of groupList) {
      const name = g.name || g.groupName || ''
      const { matched } = fuzzyMatchWithSpansSimple(name, q)
      if (matched) {
        matchedPaths.add(g.groupPath)
        // 命中分组 → 所有祖先都需要自动展开
        for (const anc of ancestors) expandPaths.add(anc)
      }
      if (g.children && g.children.length > 0) {
        collectGroupMatchesRecursive(
          g.children,
          q,
          matchedPaths,
          expandPaths,
          [...ancestors, g.groupPath]
        )
        // 如果子树中有命中 → 当前节点也需要展开并保留
        const subtreeHasMatch = g.children.some(
          (c) => matchedPaths.has(c.groupPath) || expandPaths.has(c.groupPath)
        )
        if (subtreeHasMatch) {
          for (const anc of ancestors) expandPaths.add(anc)
          expandPaths.add(g.groupPath)
        }
      }
    }
  }

  /**
   * 全局重新计算搜索命中
   * - 分组：递归匹配所有层级分组名
   * - 模组：仅搜索当前选中分组内的模组（未选中分组的模组不搜索）
   * - 自动展开：命中分组及其所有祖先分组
   */
  function updateSearchMatches() {
    const q = searchQuery.value.trim()
    if (!q) {
      groupMatchPaths.value = []
      modMatchIndices.value = []
      autoExpandGroupPaths.value = new Set()
      currentGlobalMatchIndex.value = -1
      return
    }

    // Step 1: 分组搜索（递归遍历所有分组树）
    const matchedGroupSet = new Set<string>()
    const expandSet = new Set<string>()
    collectGroupMatchesRecursive(groups.value, q, matchedGroupSet, expandSet)
    groupMatchPaths.value = Array.from(matchedGroupSet)
    autoExpandGroupPaths.value = expandSet

    // Step 2: 模组搜索（仅在当前选中分组内搜索，结果保存为 mods 扁平列表全局索引）
    const flatMods = mods.value
    const modPathToFlatIdx = new Map<string, number>()
    flatMods.forEach((m, i) => {
      if (m.modPath) modPathToFlatIdx.set(m.modPath, i)
    })
    const currentMods = currentGroupMods.value
    const hitFlatIndices: number[] = []
    for (const m of currentMods) {
      const name = m.modName || m.name || ''
      const { matched } = fuzzyMatchWithSpansSimple(name, q)
      if (matched && m.modPath) {
        const flatIdx = modPathToFlatIdx.get(m.modPath)
        if (flatIdx !== undefined) hitFlatIndices.push(flatIdx)
      }
    }
    modMatchIndices.value = hitFlatIndices

    currentGlobalMatchIndex.value =
      groupMatchPaths.value.length + hitFlatIndices.length > 0 ? 0 : -1
  }

  /** 显示/隐藏搜索栏 */
  function setSearchVisible(v: boolean) {
    searchVisible.value = v
    if (v) {
      updateSearchMatches()
    } else {
      modMatchIndices.value = []
      groupMatchPaths.value = []
      autoExpandGroupPaths.value = new Set()
      currentGlobalMatchIndex.value = -1
    }
  }

  /** 清空搜索词 */
  function clearSearch() {
    searchQuery.value = ''
    updateSearchMatches()
  }

  /** 下一个命中项 */
  function nextSearchMatch() {
    const total = groupMatchPaths.value.length + modMatchIndices.value.length
    if (total === 0) return
    if (currentGlobalMatchIndex.value < 0) currentGlobalMatchIndex.value = 0
    else currentGlobalMatchIndex.value = (currentGlobalMatchIndex.value + 1) % total
  }

  /** 上一个命中项 */
  function prevSearchMatch() {
    const total = groupMatchPaths.value.length + modMatchIndices.value.length
    if (total === 0) return
    if (currentGlobalMatchIndex.value < 0) currentGlobalMatchIndex.value = total - 1
    else currentGlobalMatchIndex.value = (currentGlobalMatchIndex.value - 1 + total) % total
  }

  /**
   * 判断当前命中项是否为分组
   */
  function getCurrentMatchInfo(): { isGroup: boolean; groupPath?: string; modFlatIdx?: number } {
    const i = currentGlobalMatchIndex.value
    if (i < 0) return { isGroup: false }
    const gCount = groupMatchPaths.value.length
    if (i < gCount) {
      return { isGroup: true, groupPath: groupMatchPaths.value[i] }
    }
    return { isGroup: false, modFlatIdx: modMatchIndices.value[i - gCount] }
  }

  /**
   * 判断某分组是否在搜索命中分组列表中
   */
  function isGroupMatch(groupPath: string): boolean {
    if (!searchQuery.value.trim()) return false
    return groupMatchPaths.value.includes(groupPath)
  }

  /**
   * 工具：判断某分组是否命中模组
   */
  function getGroupModDisplayHitIndices(groupMods: ModData[], currentMods?: ModData[]): number[] {
    if (!searchQuery.value.trim()) return []
    const displayList = currentMods || groupMods
    const flatHitPaths = new Set(
      modMatchIndices.value.map(i => mods.value[i]?.modPath).filter(Boolean)
    )
    const result: number[] = []
    displayList.forEach((m, idx) => {
      if (m.modPath && flatHitPaths.has(m.modPath)) result.push(idx)
    })
    return result
  }

  /**
   * 按路径选中分组
   */
  function selectGroupByPath(path: string) {
    const g = findGroupByPathInList(groups.value, path)
    if (g) selectGroup(g)
  }

  /**
   * 递归在分组树中查找包含指定模组的分组
   */
  function findGroupContainingModInList(groupList: ModGroupData[], modPath: string): ModGroupData | null {
    for (const g of groupList) {
      for (const m of g.mods) {
        if (m.modPath === modPath) return g
      }
      if (g.children && g.children.length > 0) {
        const found = findGroupContainingModInList(g.children, modPath)
        if (found) return found
      }
    }
    return null
  }

  /**
   * 选中包含指定模组的分组
   */
  function selectGroupContainingMod(modPath: string) {
    const g = findGroupContainingModInList(groups.value, modPath)
    if (g) selectGroup(g)
  }

  // 搜索词变化 → 重算命中（150ms 防抖，避免快速输入时频繁触发）
  let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null
  watch(searchQuery, () => {
    if (searchDebounceTimer) clearTimeout(searchDebounceTimer)
    searchDebounceTimer = setTimeout(() => {
      updateSearchMatches()
    }, 150)
  })
  // 分组/模组数据变化 → 重算命中
  watch([groups, mods], () => {
    if (searchVisible.value && searchQuery.value.trim()) {
      updateSearchMatches()
    }
  })

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

  /** 当前选中的分组对象（通过路径递归查找） */
  const currentGroup = computed<ModGroupData | null>(() => {
    if (!selectedGroupPath.value) return groups.value[0] || null
    return findGroupByPathInList(groups.value, selectedGroupPath.value)
  })

  /** 当前选中的模组对象（显示用：如果是子分组，从子分组mods中取）
   *  优先返回按键绑定页显式设置的目标模组（keybindTargetMod），对齐 NRMM modKeybindProvider
   */
  const selectedMod = computed<ModData | null>(() => {
    if (keybindTargetMod.value) return keybindTargetMod.value
    const g = currentGroup.value
    if (!g) return null
    return g.mods[selectedModIndex.value] || null
  })
  /**
   * 设置按键绑定页的目标模组（右键菜单「按键切换」时调用）
   * @param mod 要在 Keybinds 页显示的模组对象，传 null 清空
   */
  function setKeybindTargetMod(mod: ModData | null) {
    keybindTargetMod.value = mod
  }
  /**
   * 清空按键绑定页的目标模组（离开 KeybindsView 或切回 ModsView 时调用）
   */
  function clearKeybindTargetMod() {
    keybindTargetMod.value = null
  }

  /** 当前分组显示的模组列表（所有分组类型均仅返回自身直接模组，不递归子分组） */
  const currentGroupMods = computed<ModData[]>(() => {
    const g = currentGroup.value
    if (!g) return []
    // 所有分组类型统一仅返回自身直接模组列表，子分组模组需点击子分组才显示
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
    needReloadManual.value = false
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

      // 恢复上次选中的分组：路径有效则保留，无效则回退到第一个分组
      // 配合 clearData 不重置 selectedGroupPath，实现窗口隐藏/显示时保留分组选择
      if (groups.value.length > 0) {
        const savedPath = selectedGroupPath.value
        const found = savedPath ? findGroupByPathInList(groups.value, savedPath) : null
        restoreGroupSelection(found || groups.value[0])
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
    logger.debug('ModsStore', 'activateModByIndex', { modIdx })
    if (isUpdatingModData.value) return
    if (isActivating.value) return
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    const group = currentGroup.value
    if (!group) return
    const displayedMods = currentGroupMods.value
    if (!Number.isSafeInteger(modIdx) || modIdx < 0 || modIdx >= displayedMods.length) {
      logger.warn('ModsStore', 'activateModByIndex out of range', { modIdx, len: displayedMods.length })
      return
    }

    const groupPath = group.groupPath
    const now = Date.now()
    const lastTs = lastSelectionTime[groupPath] ?? 0
    if (now - lastTs < SELECTION_DEBOUNCE_MS) {
      logger.debug('mods', `[activateModByIndex] debounced: groupPath=${groupPath}, elapsed=${now - lastTs}ms`)
      return
    }
    lastSelectionTime[groupPath] = now

    isActivating.value = true
    try {
      const isMutex = group.groupType === 'mutexGroup'
      const mod = displayedMods[modIdx]
      const modPath = mod?.modPath || ''
      // 使用 group_xx 目录编号（如 group_1 → 1）作为后端 group_index，
      // 而非 groups 数组下标。后端 switch_mod 与按键模拟的 active_group_id
      // 均以该编号为准；若存在互斥组或编号不连续（删除过分组），数组下标会与编号错位，
      // 导致游戏内 [KeyMod] 的 condition（$group_id == active_group_id）无法命中。
      const groupIdx = group.groupIndex

      logger.debug('ModsStore', '[activateModByIndex] calling selectMod', {
        game: s.currentGame,
        modsPath: s.currentModsPath,
        groupIndex: groupIdx,
        modIndex: modIdx,
        isMutex,
        groupPath: group.groupPath,
        modPath,
      })

      // 调用后端select_mod命令处理INI写入和互斥逻辑，取得写入磁盘后的最终索引
      const result = await selectMod(
        s.currentGame,
        s.currentModsPath,
        groupIdx,
        modIdx,
        isMutex,
        group.groupPath,
        modPath,
        undefined, // cursorX - pass undefined for now (no coordinate calculation yet)
        undefined, // cursorY - pass undefined for now (no coordinate calculation yet)
      )
      logger.debug('ModsStore', '[activateModByIndex] selectMod result', {
        selectedModIndex: result?.selectedModIndex,
      })
      // 注意：模组选择不触发 markNeedUpdate，因为选择不涉及模组数据修改
      // 仅在启用/禁用模组时才需要提醒用户更新模组数据

      // 选择写入磁盘的最终索引：优先使用后端返回值，否则回退到前端传入值
      const retSelIdx: number =
        result && typeof result.selectedModIndex === 'number' ? result.selectedModIndex : modIdx
      // 更新前端选中状态
      selectedModIndex.value = retSelIdx
      // 同步记录到分组索引映射（对应 Flutter 的 previousSelectedModOnGroup）
      const gPath = group.groupPath
      if (!gPath.startsWith('__') && gPath !== '__groups__') {
        selectedModIndicesByGroup[gPath] = retSelIdx
      }
      // 关键同步：在 groups.value 整棵树上定位该分组，更新其 activeModIndex
      // 保证下一次点击该分组时，UI 依然显示为后端写入的选中索引
      if (group.groupType === 'normalGroup') {
        const groupPathMatch = group.groupPath
        const found = (function syncRec(list: ModGroupData[]): boolean {
          for (const g of list) {
            if (g.groupPath === groupPathMatch) {
              g.activeModIndex = retSelIdx
              return true
            }
            if (g.children && g.children.length > 0 && syncRec(g.children)) return true
          }
          return false
        })(groups.value)
        if (!found) {
          logger.warn('ModsStore', 'activeModIndex sync failed: group not found in tree', {
            groupPath: groupPathMatch,
            retSelIdx,
          })
        }
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
      // 向调用方冒泡异常，保证调用方能感知到失败
      throw e
    } finally {
      isActivating.value = false
    }
  }

  /**
   * 双击 None 空槽位时的统一处理入口
   * - NormalGroup（group_xx） → 取消分组选中（写入 selectedindex=0，不选择模组）
   * - MutexGroup（非 group_xx）→ 禁用该分组下所有一级模组（添加 DISABLED_ 前缀）
   *
   * 虚拟节点（__all__ / __fav__ / __groups__）不执行任何操作（兜底 guard）。
   *
   * @param groupType  当前分组类型（仅 normalGroup / mutexGroup 有行为）
   * @param groupIndex group_xx 编号（NormalGroup 写 selectedindex 用）
   * @param groupPath  分组目录绝对路径（MutexGroup 批量禁用用；NormalGroup 也传入用于日志）
   */
  async function deselectOrDisableNoneSlot(
    groupType: 'normalGroup' | 'mutexGroup',
    groupIndex: number,
    groupPath: string,
  ) {
    // 虚拟节点兜底：仅 normalGroup / mutexGroup 才实际操作
    if (groupType !== 'normalGroup' && groupType !== 'mutexGroup') {
      logger.warn('mods', 'deselectOrDisableNoneSlot called on virtual group, skip', { groupType })
      return
    }
    if (isUpdatingModData.value || isActivating.value) return
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    isActivating.value = true
    try {
      if (groupType === 'normalGroup') {
        logger.debug('mods', 'deselectOrDisableNoneSlot → deselectGroupMod', { groupIndex, groupPath })
        await deselectGroupMod(s.currentGame, s.currentModsPath, groupIndex)
        // 同步前端选中索引到 0（None 槽位），减少 refresh 前的视觉闪动
        selectedModIndex.value = 0
        const path = selectedGroupPath.value
        if (path && !path.startsWith('__') && path !== '__groups__') {
          selectedModIndicesByGroup[path] = 0
        }
      } else {
        logger.debug('mods', 'deselectOrDisableNoneSlot → disableAllModsInGroup', { groupPath })
        const n = await disableAllModsInGroup(groupPath)
        logger.info('mods', `Disabled ${n} mods in mutex group ${groupPath}`)
      }
      await refresh()
    } catch (e: any) {
      logger.error('mods', 'deselectOrDisableNoneSlot failed', e)
      throw e
    } finally {
      isActivating.value = false
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
   *
   * 注意：保留 selectedGroupPath，配合 loadMods 末尾的恢复逻辑实现窗口隐藏/显示时保留分组选择。
   * - 同一游戏窗口隐藏/显示：selectedGroupPath 保留，loadMods 检查路径仍有效后恢复选中状态
   * - 切换游戏：旧 selectedGroupPath 在新游戏 groups 中找不到，loadMods 自动回退到第一个分组
   * - 路径不存在刷新：保留选择，loadMods 后若分组仍存在则恢复，否则回退到第一个
   */
  function clearData() {
    groups.value = []
    mods.value = []
    loading.value = false
    // 保留 selectedGroupPath：由 loadMods 的恢复逻辑判断是否仍有效
    selectedModIndex.value = 0
    searchQuery.value = ''
    showFavoritesOnly.value = false
    // 清空分组选中记录（loadMods 会用后端 activeModIndex 重新初始化）
    for (const key of Object.keys(selectedModIndicesByGroup)) {
      delete selectedModIndicesByGroup[key]
    }
  }

  /**
   * 恢复分组选中状态（selectedGroupPath + selectedModIndex）
   *
   * 优先级：selectedModIndicesByGroup（会话内用户选择） > group.activeModIndex（后端持久化） > 0（默认）
   * 虚拟节点路径（__all__/__fav__/__groups__）固定为 0
   *
   * @param group 目标分组对象
   */
  function restoreGroupSelection(group: ModGroupData) {
    selectedGroupPath.value = group.groupPath
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
   * 统一全量策略：始终调用全量 update_mod_data，不再使用分组增量更新。
   * 原因：全量 update_mod_data 已验证 <10s，而分组增量更新在多分组标记时
   * 会导致 N×全量扫描的 300s+ 性能问题。
   *
   * needUpdatePerGroup 仅用于提示条显示逻辑，不再用于选择更新路径。
   * 更新完成后清除所有标记和提醒条。
   *
   * @returns Promise<UpdateResult | null> 更新完成后自动调用 loadMods() 刷新前端数据；失败时抛出异常
   */
  async function updateModData() {
    logger.debug('ModsStore', 'updateModData started')
    const s = useSettingsStore()
    if (!s.currentModsPath) return null
    isUpdatingModData.value = true
    loading.value = true
    try {
      const lastResult = await tauriUpdateModData(s.currentGame, s.currentModsPath)
      needReloadManual.value = lastResult.needReloadManual
      updateResult.value = lastResult

      await loadMods()
      clearNeedUpdate()
      logger.debug('ModsStore', 'updateModData completed', { result: lastResult })
      return lastResult
    } catch (e) {
      logger.error('ModsStore', 'Failed to update mod data', e)
      throw e
    } finally {
      loading.value = false
      isUpdatingModData.value = false
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
  /**
   * Global no search hit. When true, group tree stays fully visible
   * instead of being cleared to empty.
   */
  const globalNoHit = computed(() => {
    const q = searchQuery.value.trim()
    if (!q) return false
    return groupMatchPaths.value.length === 0 && modMatchIndices.value.length === 0
  })

  function hasData(): boolean {
    return groups.value.length > 0 || mods.value.length > 0
  }

  return {
    groups,
    mods,
    loading,
    selectedGroupPath,
    selectedModIndex,
    searchQuery,
    showFavoritesOnly,
    needUpdate,
    needUpdatePerGame,
    needUpdatePerGroup,
    needReloadManual,
    isUpdatingModData,
    updateResult,
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
    deselectOrDisableNoneSlot,
    setKeybindTargetMod,
    clearKeybindTargetMod,
    clearData,
    startWatching,
    stopWatching,
    updateModData,
    markNeedUpdate,
    clearNeedUpdate,
    findGroupByPathInList,
    hasData,
    globalNoHit,
    // ========== 搜索导出 ==========
    searchVisible,
    modMatchIndices,
    groupMatchPaths,
    currentGlobalMatchIndex,
    autoExpandGroupPaths,
    updateSearchMatches,
    setSearchVisible,
    clearSearch,
    nextSearchMatch,
    prevSearchMatch,
    getCurrentMatchInfo,
    isGroupMatch,
    getGroupModDisplayHitIndices,
    fuzzyMatchWithSpansSimple,
    normalizeText,
    selectGroupByPath,
    selectGroupContainingMod,
  }
})
