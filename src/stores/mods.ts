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
import { ref, computed, reactive, watch, shallowRef, triggerRef, nextTick } from 'vue'
import { ElMessage } from 'element-plus'
import { getMods, refreshMods, selectMod, switchFileWatcher, stopFileWatcher, updateModData as tauriUpdateModData, deselectGroupMod, disableAllModsInGroup, syncNrmmUpdateFlag } from '../utils/tauri'
import { predictToggledPath } from '../utils/modPath'
import { deriveGroupActiveState } from '../utils/groupActiveState'
import { normalizeText, fuzzyMatchWithSpansSimple, collectGroupMatchesRecursive } from '../utils/modSearch'
import { sortModsLikeLightScan, reindexMods } from '../utils/modSort'
import { useSettingsStore } from './settings'
import type { ModGroupData, ModData, TargetGame, UpdateResult, GroupDelta, InvalidUtf8Entry } from '../types'
import { logger } from '../utils/logger'
import { formatInvalidUtf8Message } from '../utils/decodePathName'

export const useModsStore = defineStore('mods', () => {
  /**
   * 加载世代计数器（竞态防护，2026-08-19）
   *
   * loadMods/refresh 入口各自取号（++loadEpoch），await 返回后若发现自己的编号已非最新，
   * 说明期间发生了新的加载/清空（如快速切换游戏、窗口显隐重入），本请求结果作废：
   * 不落地数据、不复位 loading——避免「旧请求后完成覆盖新数据」「clearData 后旧结果落地」
   * 两类数据加载异常（多入口：App.vue window-shown / target-game-switched、
   * ModsView onMounted、updateModData 内部等）。
   * clearData 亦使所有 in-flight 请求失效（切游戏时旧游戏数据不得落地）。
   */
  let loadEpoch = 0
  /**
   * 模组分组列表（顶层：NormalGroup + MutexGroup根节点）
   *
   * shallowRef（2026-08-19 性能优化）：万级模组（实测 14415）场景下深层 reactive 的
   * proxy 化内存与开销显著；本 store 的更新模式为「整体替换赋值」（loadMods/refresh），
   * 天然兼容 shallowRef。仅有的两处树内字段 mutate（activateModByIndex 的
   * activeModIndex / isActive 乐观更新）显式调用 triggerRef 触发更新。
   */
  const groups = shallowRef<ModGroupData[]>([])
  /** 所有模组的扁平列表（包含所有分组的模组），shallowRef 理由同上（整体替换赋值） */
  const mods = shallowRef<ModData[]>([])
  /**
   * 树内本地 mutate 版本戳（2026-09-06 启停 UI 渲染断链修复）。
   *
   * shallowRef 语义下，树内对象/数组的**就地 mutate**（启用禁用标志、isActive、
   * 组内重排等）不会改变 `groups.value` / `currentGroup` 的引用——即使显式
   * `triggerRef`，computed 链（currentGroup → currentGroupMods → displayMods）
   * 因「值引用相等」不向下传播，UI 直到整树替换（切页/重扫）才刷新。
   * 本地 mutate 后自增该戳 + 受影响容器替换新数组引用，强制 stats 链重算并拿到
   * 新引用（ModCard v-for 依赖数组引用），实现乐观 UI 即时渲染。
   */
  const modTreeVersion = ref(0)
  /** 是否正在加载模组数据 */
  const loading = ref(false)
  /** 最近一次加载失败的错误信息（C18：非竞态过期时置位；success/clearData 时清空） */
  const loadError = ref<string | null>(null)
  /**
   * 非法 UTF-8 路径跳过通知（App.vue watch 后 ElMessage 提示）。
   *
   * loadMods/refresh/updateModData 扫描返回 invalidUtf8Count>0 时置位，
   * 提示用户「有文件名因编码无法识别被跳过」；为 null 表示无跳过（不提示）。
   * clearData/下一次无跳过加载时自动清空（防重复弹窗）。
   */
  const invalidUtf8Notice = ref<string | null>(null)
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
  /**
   * 后端 `select_mod` 防抖窗口为 500ms；命中防抖后前端以 trailing-edge 重发，
   * 延迟须略大于该窗口，否则重发仍会落在窗口内被再次拦截。
   */
  const SELECTION_DEBOUNCE_RETRY_MS = 600
  /** 待执行的防抖 trailing-edge 重试定时器（同一时刻最多一个，新意图覆盖旧意图） */
  let pendingActivateRetry: ReturnType<typeof setTimeout> | null = null
  /** 是否正在执行重量级更新模组数据（全局状态锁，true时阻止其他操作） */
  const isUpdatingModData = ref(false)
  /** 更新模组数据结果（保存最近一次的 UpdateResult） */
  const updateResult = ref<UpdateResult | null>(null)
  /**
   * 更新期间发生 clearData/切游戏（epoch 竞态）后的待重载标志。
   * 记录更新发起时的游戏 id：window-shown 在同游戏恢复时强制 loadMods 并清 needUpdate；
   * 若期间已切到其他游戏则仅丢弃标志（新游戏有自己的加载流程），避免误清新游戏 needUpdate。
   * null = 无待重载。
   */
  const pendingReloadAfterRecreate = ref<string | null>(null)
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
   * 最近一次本地 patch 时间戳（R1 命令级 Delta）
   * ModsView folder-changed 回声抑制依据：命令 patch 后短窗口内的 watcher 回声事件
   * 跳过全量 refresh（缓存已由命令内局部重扫合并，批次处理为幂等重复）
   */
  const lastLocalPatchAt = ref(0)

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
    // 消费树版本戳：本地 mutate（启停/收藏/选择/取消选择）后重算，绕开 shallowRef
    // 引用相等导致的断链（详见 modTreeVersion 注释）
    void modTreeVersion.value
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
    needUpdatePerGame.value[s.currentGame] = true
    // 分组级标记（用于增量更新）
    if (groupIndex !== undefined) {
      needUpdatePerGroup.value = {
        ...needUpdatePerGroup.value,
        [groupIndex]: true
      }
    }
    // 双向同步：把「需要更新模组数据」写为 true 到原版 NRMM 持久化标记，
    // 与原版 NRMM showUpdateModSnackbar 行为一致（fire-and-forget，不阻断主流程）
    void syncNrmmUpdateFlag(s.currentGame, true)
  }

  /**
   * 清除当前游戏的需要更新状态
   * 在updateModData成功完成后或用户关闭提示条时调用
   */
  function clearNeedUpdate() {
    const s = useSettingsStore()
    needUpdatePerGame.value[s.currentGame] = false
    // 清除所有分组级标记
    needUpdatePerGroup.value = {}
    needReloadManual.value = false
    // 双向同步：清除原版 NRMM 持久化标记，避免其残留「未更新」提示
    // （与原版 NRMM manageMods 完成行为一致，fire-and-forget）
    void syncNrmmUpdateFlag(s.currentGame, false)
  }

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
    const epoch = ++loadEpoch
    loading.value = true
    try {
      const result = await getMods(s.currentGame, s.currentModsPath)
      // 竞态防护：await 期间若有新加载/清空（epoch 变化），本结果作废不落地
      if (epoch !== loadEpoch) return
      groups.value = result.groups || []
      mods.value = result.mods || []
      // C18：加载成功后清空失败反馈
      loadError.value = null
      // 非法 UTF-8 路径提示：扫描有跳过时置位（App.vue watch 弹窗），无跳过时清空
      invalidUtf8Notice.value = applyInvalidUtf8Notice(result.invalidUtf8Count ?? 0, result.invalidUtf8Entries)

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
      // 过期请求的异常静默（新请求已接管）；当前请求的异常记录日志
      if (epoch === loadEpoch) {
        logger.error('ModsStore', 'Failed to load mods', e)
        // C18：非竞态过期时提供可见失败反馈；路径不存在类错误走既有 handlePathNotFoundError 恢复流程，不弹错误提示
        const msg = e instanceof Error ? e.message : String(e)
        loadError.value = msg.includes('[PATH_NOT_FOUND]') ? null : msg
      }
    } finally {
      // 仅当前请求可复位 loading：过期请求提前复位会导致新请求进行中加载态消失
      if (epoch === loadEpoch) {
        loading.value = false
      }
    }
  }

  /**
   * 将后端扫描结果中的非法 UTF-8 跳统计转为通知文本（无跳返回 null 不再提示）
   *
   * @param count 跳过总数
   * @param entries 明细（含父路径与保真字节名）
   * @returns 通知文本；count<=0 或无明细时返回 null
   */
  function applyInvalidUtf8Notice(
    count: number,
    entries: InvalidUtf8Entry[] | undefined,
  ): string | null {
    const list = entries ?? []
    if (count <= 0 || list.length === 0) return null
    return formatInvalidUtf8Message(count, list)
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
    const epoch = ++loadEpoch
    loading.value = true
    try {
      const result = await refreshMods(s.currentGame, s.currentModsPath)
      // 竞态防护：await 期间若有新加载/清空（epoch 变化），本结果作废不落地
      if (epoch !== loadEpoch) return
      groups.value = result.groups || []
      mods.value = result.mods || []
      loadError.value = null
      // 非法 UTF-8 路径提示：刷新同样上报（文件变化后可能新出现非法条目）
      invalidUtf8Notice.value = applyInvalidUtf8Notice(result.invalidUtf8Count ?? 0, result.invalidUtf8Entries)

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
      // 过期请求的异常静默（新请求已接管）；当前请求的异常记录日志
      if (epoch === loadEpoch) {
        logger.error('ModsStore', 'Failed to refresh mods', e)
        // C18：与 loadMods 同口径（refresh 由 watcher/导入触发，静默失败无感知；PATH_NOT_FOUND 走既有恢复流程）
        const msg = e instanceof Error ? e.message : String(e)
        loadError.value = msg.includes('[PATH_NOT_FOUND]') ? null : msg
      }
    } finally {
      // 仅当前请求可复位 loading（同 loadMods 的竞态防护）
      if (epoch === loadEpoch) {
        loading.value = false
      }
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

  // ========== 命令级分组增量（R1：mod-state-delta 方案，替代操作后全量 refresh） ==========

  /**
   * 递归收集组节点内全部模组对象（含 children 子分组），排除 None 空槽位
   * None 仅存在于树内组列表首位（槽位选择展示），扁平 mods 列表不含 None
   * （与后端 scan_mods_light 的扁平列表语义一致）
   */
  function collectGroupModsRecursive(group: ModGroupData): ModData[] {
    const out: ModData[] = []
    for (const m of group.mods) {
      if (m.modPath && m.modPath !== 'None' && m.name !== 'None') out.push(m)
    }
    for (const child of group.children || []) {
      out.push(...collectGroupModsRecursive(child))
    }
    return out
  }

  /**
   * 在分组树中定位指定路径的组节点及其所在数组（顶层 groups 或父级 children）
   * @returns 节点、所在数组与数组内下标；未找到返回 null
   */
  function findGroupSlot(
    groupList: ModGroupData[],
    path: string
  ): { node: ModGroupData; list: ModGroupData[]; index: number } | null {
    for (let i = 0; i < groupList.length; i++) {
      const g = groupList[i]
      if (g.groupPath === path) return { node: g, list: groupList, index: i }
      if (g.children && g.children.length > 0) {
        const found = findGroupSlot(g.children, path)
        if (found) return found
      }
    }
    return null
  }

  /**
   * 分组树中是否存在指定路径的分组（顶层 + 互斥组 children 递归）
   *
   * R2：ModsView 消费 managed-partial-update 前的可 patch 判定——
   * 树中无此组（外部新建分组、树尚未加载等）时增量无法原地 patch，
   * 调用方应跳过 patch 改走全量 refresh 兜底
   *
   * @param groupPath 分组路径（树定位键）
   * @returns 存在返回 true
   */
  function hasGroup(groupPath: string): boolean {
    return findGroupSlot(groups.value, groupPath) !== null
  }

  /**
   * 应用命令级分组增量（R1 命中路径：toggle/remove/rename/批量组操作）
   *
   * 命令返回受影响分组的完整新状态（后端已局部重扫并排序），本地原地 patch，
   * 零全量 refresh：
   * 1. 树中按 oldGroupPath 定位组节点（顶层 + children 递归）
   * 2. 收集该节点旧扁平条目路径（递归 mods，排除 None）→ 优先用 delta.oldModPaths
   * 3. 扁平 mods：记录旧条目最小索引 → 移除全部旧条目 → newGroup 存在时在该索引 splice 插入新条目
   * 4. 树：newGroup 存在 → 父数组原位替换节点；null → 从父数组移除节点
   * 5. 选中态修正：selectedGroupPath 失效（指向被移除/重命名组）→ 回退第一个分组；
   *    selectedModIndex 越界修正（同 refresh() 既有逻辑）
   * 6. 搜索激活时重算命中（splice 导致 modMatchIndices 扁平索引漂移，重算保证一致，仅搜索激活时付出）
   * 7. 记录 lastLocalPatchAt（ModsView folder-changed 回声抑制依据）
   *
   * @param delta 命令返回的分组增量（newGroup=null 表示分组已整体移除）
   */
  function applyGroupDelta(delta: GroupDelta): void {
    // 0. 替换前捕获「当前选中模组路径」：delta 携带后端权威序（natural 序），
    //    替换后组内卡片会重排；若只按整数下标搬运 selectedModIndex，选中高亮会落到
    //    别的卡片上（用户反馈的「返回数据后重排序 → 选错」）。此处对齐 toggleModLocal 口径：
    //    按 modPath 记录 → 替换后按路径重定位。
    const deltaGPath = selectedGroupPath.value
    const deltaTrackingPath =
      selectedModIndex.value > 0 && currentGroup.value
        ? currentGroup.value.mods[selectedModIndex.value]?.modPath
        : undefined

    // 1. 树中定位旧组节点（含其所在父数组，供原位替换/移除）
    const slot = findGroupSlot(groups.value, delta.oldGroupPath)
    if (!slot) {
      logger.warn('ModsStore', 'applyGroupDelta: group not found in tree, only flat mods patched', {
        oldGroupPath: delta.oldGroupPath,
      })
    }

    // 2. 该分组旧扁平条目路径：优先用 delta.oldModPaths；为空时从树节点递归收集（排除 None）
    const oldPaths =
      delta.oldModPaths.length > 0
        ? delta.oldModPaths
        : slot
          ? collectGroupModsRecursive(slot.node).map((m) => m.modPath)
          : []

    // 3. 扁平 mods：剔除全部旧条目并记录最小索引；newGroup 存在时在该索引原位插入新条目
    const oldPathSet = new Set(oldPaths)
    let insertIdx = -1
    const kept: ModData[] = []
    for (let i = 0; i < mods.value.length; i++) {
      const m = mods.value[i]
      if (m.modPath && oldPathSet.has(m.modPath)) {
        if (insertIdx === -1) insertIdx = i
      } else {
        kept.push(m)
      }
    }
    if (delta.newGroup) {
      const newEntries = collectGroupModsRecursive(delta.newGroup)
      // 旧条目未在扁平中找到（如空分组）→ 追加到尾部
      if (insertIdx === -1) insertIdx = kept.length
      kept.splice(insertIdx, 0, ...newEntries)
    }
    mods.value = kept

    // 4. 树：原位替换 / 移除节点（shallowRef 数组 mutate → 显式 triggerRef）
    if (slot) {
      if (delta.newGroup) {
        slot.list[slot.index] = delta.newGroup
      } else {
        slot.list.splice(slot.index, 1)
      }
      triggerRef(groups)
    }

    // 5. 选中态修正（与 refresh() 既有逻辑对齐）
    if (selectedGroupPath.value) {
      const found = findGroupByPathInList(groups.value, selectedGroupPath.value)
      if (!found && groups.value.length > 0) {
        selectedGroupPath.value = groups.value[0].groupPath
      }
    } else if (groups.value.length > 0) {
      selectedGroupPath.value = groups.value[0].groupPath
    }
    // 5b. 按路径重定位选中位次（权威 delta 重排后的兜底修正）
    //    先于越界钳制执行：重排后旧下标通常仍「合法」但已指向别的模组，
    //    仅靠越界钳制无法发现，必须以路径为准。
    if (deltaTrackingPath !== undefined && deltaGPath) {
      const g = findGroupByPathInList(groups.value, deltaGPath)
      if (g) {
        const idx = g.mods.findIndex((m) => m.modPath === deltaTrackingPath)
        if (idx >= 0 && idx !== selectedModIndex.value) {
          selectedModIndex.value = idx
          if (!deltaGPath.startsWith('__') && deltaGPath !== '__groups__') {
            selectedModIndicesByGroup[deltaGPath] = idx
          }
          logger.debug('ModsStore', 'applyGroupDelta: relocated selected index by path', {
            from: deltaTrackingPath,
            to: idx,
            groupPath: deltaGPath,
          })
        } else if (idx < 0) {
          logger.warn('ModsStore', 'applyGroupDelta: selected mod vanished after delta', {
            trackingPath: deltaTrackingPath,
            groupPath: deltaGPath,
          })
        }
      }
    }
    const currentMods = currentGroupMods.value
    if (selectedModIndex.value >= currentMods.length) {
      selectedModIndex.value = Math.max(0, currentMods.length - 1)
      const curPath = selectedGroupPath.value
      if (curPath && !curPath.startsWith('__') && curPath !== '__groups__') {
        selectedModIndicesByGroup[curPath] = selectedModIndex.value
      }
    }

    // 6. 搜索激活时重算命中（扁平索引随 splice 漂移，重算保证一致）
    if (searchVisible.value && searchQuery.value.trim()) {
      updateSearchMatches()
    }

    // 7. 记录本地 patch 时间戳（watcher 回声抑制依据）
    lastLocalPatchAt.value = Date.now()
  }

  /**
   * 收藏本地翻转（toggleFavorite 命令返回 bool 后调用；A2 变体——0 IPC 零全量）
   *
   * 扁平与树内同路径条目为两个独立对象实例（后端 ScanResult 独立序列化），需分别翻转；
   * 组内位置近似重排（收藏前置/取消后移；精确序由后续增量/全量刷新恢复）。
   *
   * @param modPath 模组路径（扁平与树的共同定位键）
   * @param isFav 命令返回的切换后收藏状态
   */
  function toggleFavoriteLocal(modPath: string, isFav: boolean): void {
    // 扁平列表：翻转 isFavorite（shallowRef 内对象 mutate → 换新引用强制收藏列表重渲染）
    const flat = mods.value.find((m) => m.modPath === modPath)
    let flatChanged = false
    if (flat) {
      flat.isFavorite = isFav
      flatChanged = true
    }
    if (flatChanged) mods.value = [...mods.value]
    // 树：定位直接包含该模组的分组节点，翻转并做组内近似重排（换新数组引用驱动渲染）
    const container = findGroupContainingModInList(groups.value, modPath)
    if (container) {
      const idx = container.mods.findIndex((m) => m.modPath === modPath)
      if (idx >= 0) {
        const next = [...container.mods]
        const [moved] = next.splice(idx, 1)
        moved.isFavorite = isFav
        if (isFav) {
          // 收藏前置：插到首位之后（None 空槽位恒为组内第 0 位，mod_index 以 None=0 为基准，不得顶替）
          const noneFirst =
            next[0] && (next[0].name === 'None' || next[0].modPath === 'None')
          next.splice(noneFirst ? 1 : 0, 0, moved)
        } else {
          // 取消收藏后移（近似序，精确序由后续增量/全量刷新恢复）
          next.push(moved)
        }
        container.mods = next
        reindexMods(container.mods)
      }
    }
    modTreeVersion.value++
    triggerRef(mods)
    triggerRef(groups)
    // fav 标记文件的 watcher 回声同样需要抑制
    lastLocalPatchAt.value = Date.now()
  }

  /**
   * 启用/禁用本地 patch（零扫描路径：toggle_mod_disabled 命令仅目录重命名后返回路径对，
   * 前端原位更新新路径与禁用标志，不触发任何扫描/重载/IPC）。
   *
   * 扁平与树内同路径条目为两个独立对象实例，需分别更新；`disabled` 兼容字段与
   * `modDisabled` 双字段同步（后端扫描/派生同源，分叉会造成 UI 显示/排序/激活态口径错位）。
   * 变更容器按轻量扫描口径就地重排（启用在前→收藏在前→名称序），并将当前选中位次
   * 按 modPath 重定位，使 UI 展示序与「下次全量轻扫」收敛一致。
   *
   * @param oldPath 重命名前绝对路径（扁平/树的共同定位键）
   * @param newPath 重命名后绝对路径
   * @param disabled 操作后的禁用状态
   */
  function toggleModLocal(oldPath: string, newPath: string, disabled: boolean): void {
    // 重排前捕获当前选中位次指向的模组路径（用于重排后按路径重定位；None=0 无需）
    const curGroup = currentGroup.value
    const selectGPath = curGroup?.groupPath ?? ''
    const selectTrackingPath =
      selectedModIndex.value > 0 && curGroup
        ? curGroup.mods[selectedModIndex.value]?.modPath
        : undefined
    const patch = (m: ModData) => {
      if (m.modPath === oldPath) {
        m.modPath = newPath
        m.disabled = disabled
        m.modDisabled = disabled
      }
    }
    let flatChanged = false
    for (const m of mods.value) {
      if (m.modPath === oldPath) {
        patch(m)
        flatChanged = true
      }
    }
    if (flatChanged) {
      // 换新数组引用：shallowRef 整体替换以强制扁平链（收藏列表等）重渲染
      const flatCopy = [...mods.value]
      sortModsLikeLightScan(flatCopy)
      mods.value = flatCopy
    }
    let treeChanged = false
    const walk = (list: ModGroupData[]) => {
      for (const g of list) {
        let gChanged = false
        for (const m of g.mods) {
          if (m.modPath === oldPath) {
            patch(m)
            gChanged = true
          }
        }
        if (gChanged) {
          // 组内容器换新数组引用 + 排序/重编号：驱动 currentGroupMods 拿到新引用并发起重渲染
          const sorted = [...g.mods]
          sortModsLikeLightScan(sorted)
          reindexMods(sorted)
          g.mods = sorted
          g.activeModIndex = g.mods.findIndex((m) => m.isActive)
          treeChanged = true
        }
        walk(g.children || [])
      }
    }
    walk(groups.value)
    // 重定位当前选中位次（组树受影响时）：patch 已改写目标模组路径，
    // 按「patch 后新路径」找回重排后的位次（用旧路径匹配会因改名而失配）
    if (treeChanged && selectTrackingPath !== undefined) {
      const g = findGroupByPathInList(groups.value, selectGPath)
      if (g) {
        const targetPath = selectTrackingPath === oldPath ? newPath : selectTrackingPath
        const idx = g.mods.findIndex((m) => m.modPath === targetPath)
        if (idx >= 0 && idx !== selectedModIndex.value) {
          selectedModIndex.value = idx
          if (selectGPath && !selectGPath.startsWith('__') && selectGPath !== '__groups__') {
            selectedModIndicesByGroup[selectGPath] = idx
          }
        }
      }
    }
    // shallowRef：树内字段 mutate 需显式触发更新；版本戳自增强制渲染链重算（引用已换新）
    modTreeVersion.value++
    triggerRef(mods)
    triggerRef(groups)
    // 目录重命名的 watcher 回声同样需要抑制（避免快照泄漏为一次无操作 refresh）
    lastLocalPatchAt.value = Date.now()
  }

  /**
   * 批量启停本地 patch（零扫描路径：disable/enable_all_mods_in_group 返回路径对列表，
   * 前端逐条更新 + 容器重排 + 选中位次重定位，不触发任何扫描/重载）。
   */
  function batchToggleModLocal(pairs: { oldPath: string; newPath: string; disabled: boolean }[]): void {
    const curGroup = currentGroup.value
    const selectGPath = curGroup?.groupPath ?? ''
    const selectTrackingPath =
      selectedModIndex.value > 0 && curGroup
        ? curGroup.mods[selectedModIndex.value]?.modPath
        : undefined
    const patch = (m: ModData) => {
      const hit = pairs.find((p) => m.modPath === p.oldPath)
      if (hit) {
        m.modPath = hit.newPath
        m.disabled = hit.disabled
        m.modDisabled = hit.disabled
      }
    }
    let flatChanged = false
    for (const m of mods.value) {
      if (pairs.some((p) => m.modPath === p.oldPath)) {
        patch(m)
        flatChanged = true
      }
    }
    if (flatChanged) {
      const flatCopy = [...mods.value]
      sortModsLikeLightScan(flatCopy)
      mods.value = flatCopy
    }
    let treeChanged = false
    const walk = (list: ModGroupData[]) => {
      for (const g of list) {
        let gChanged = false
        for (const m of g.mods) {
          if (pairs.some((p) => m.modPath === p.oldPath)) {
            patch(m)
            gChanged = true
          }
        }
        if (gChanged) {
          // 组内容器换新数组引用（同 toggleModLocal：驱动 currentGroupMods 重渲染）
          const sorted = [...g.mods]
          sortModsLikeLightScan(sorted)
          reindexMods(sorted)
          g.mods = sorted
          g.activeModIndex = g.mods.findIndex((m) => m.isActive)
          treeChanged = true
        }
        walk(g.children || [])
      }
    }
    walk(groups.value)
    if (treeChanged && selectTrackingPath !== undefined) {
      const g = findGroupByPathInList(groups.value, selectGPath)
      if (g) {
        // 批量 patch 后按新路径重定位（与 toggleModLocal 同口径）
        const hit = pairs.find((p) => p.oldPath === selectTrackingPath)
        const targetPath = hit ? hit.newPath : selectTrackingPath
        const idx = g.mods.findIndex((m) => m.modPath === targetPath)
        if (idx >= 0 && idx !== selectedModIndex.value) {
          selectedModIndex.value = idx
          if (selectGPath && !selectGPath.startsWith('__') && selectGPath !== '__groups__') {
            selectedModIndicesByGroup[selectGPath] = idx
          }
        }
      }
    }
    modTreeVersion.value++
    triggerRef(mods)
    triggerRef(groups)
    if (pairs.length > 0) lastLocalPatchAt.value = Date.now()
  }

  /**
   * 移除模组本地 patch（零扫描路径：remove_mod 命令移动 + INI 还原后，
   * 前端就地剔除卡片，不触发任何扫描/重载/IPC）。
   *
   * 扁平与树内同路径条目为两个独立对象实例，需分别剔除；若被移除模组为当前选中项，
   * 选中索引回退（对齐 applyGroupDelta 的选中态修正语义）。
   */
  function removeModLocal(modPath: string): void {
    // 扁平剔除
    mods.value = mods.value.filter((m) => m.modPath !== modPath)
    // 树内剔除（深层 children 递归；空组保留）
    const walk = (list: ModGroupData[]): boolean => {
      let removed = false
      for (const g of list) {
        const before = g.mods.length
        g.mods = g.mods.filter((m) => m.modPath !== modPath)
        if (g.mods.length !== before) removed = true
        if (walk(g.children || [])) removed = true
      }
      return removed
    }
    walk(groups.value)
    // 树容器已换新引用（filter 结果）；版本戳强制 currentGroupMods 重算（shallowRef 断链修复）
    modTreeVersion.value++
    triggerRef(mods)
    triggerRef(groups)
    // 选中态修正：被移除模组为当前选中时回退
    if (currentGroup.value) {
      const cur = currentGroupMods.value
      if (selectedModIndex.value >= cur.length) {
        selectedModIndex.value = Math.max(0, cur.length - 1)
        const p = selectedGroupPath.value
        if (p && !p.startsWith('__') && p !== '__groups__') {
          selectedModIndicesByGroup[p] = selectedModIndex.value
        }
      }
    }
    // 目录移动的 watcher 回声同样需要抑制
    lastLocalPatchAt.value = Date.now()
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
   * @param modPath 模组绝对路径（唯一标识，定位模组的首选依据）
   * @param isRetry 内部参数：是否为「防抖 trailing-edge 重试」的重发调用（防重试死循环）
   */
  async function activateModByIndex(modIdx: number, modPath?: string, isRetry = false) {
    logger.debug('ModsStore', 'activateModByIndex', { modIdx, modPath, isRetry })
    if (isUpdatingModData.value) return
    if (isActivating.value) return
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    const group = currentGroup.value
    if (!group) return
    const displayedMods = currentGroupMods.value
    // 优先以 modPath（唯一标识）定位模组，规避收藏模式下
    // displayMods（无 None、0-based）与 currentGroupMods（含 None 槽位）下标错位：
    // 用收藏列表下标直接取 currentGroupMods 会选错模组，甚至取到 None 槽位（误传 mod_index=0）。
    let mod: ModData | undefined
    if (modPath) {
      mod = displayedMods.find(m => m.modPath === modPath)
    } else if (Number.isSafeInteger(modIdx) && modIdx >= 0 && modIdx < displayedMods.length) {
      mod = displayedMods[modIdx]
    }
    if (!mod) {
      logger.warn('ModsStore', 'activateModByIndex mod not found', { modIdx, modPath, len: displayedMods.length })
      return
    }

    isActivating.value = true
    // === [selection-latency] 端到端打点（线性增长调试） ===
    // t0=入口 → t1=乐观更新应用完 → (异步)渲染 flush → t2=selectMod 返回 → t3=校正完成。
    // 用户用不同模组数量的分组对比各分段耗时，验证「延迟与模组数量线性增长」假设。
    const latT0 = performance.now()
    let latT1 = 0
    let latT2 = 0
    let latT3 = 0
    let latRenderFlush: number | null = null
    let latSummaryDone = false
    // 全树递归模组总数（含嵌套子组）：setActiveRecursive / collectActiveRecursive 均为 O(全树) 遍历
    let latTreeMods = 0
    const latCtx: Record<string, unknown> = {
      groupModsCount: group.mods.length, // 组第一层模组数（不含 None 槽位）
      displayedModsCount: displayedMods.length, // 当前视图列表长度（含 None 槽位）
      flatModsCount: mods.value.length, // 扁平列表总数（收藏/搜索视图高亮同步范围）
      groupPath: group.groupPath,
      isRetry,
    }
    // t3 与渲染 flush 两端都齐才打一条完整汇总（渲染 flush 通常先到，汇总在 t3 处发出）
    const latTryEmitSummary = () => {
      if (latSummaryDone || latRenderFlush === null || latT3 === 0) return
      latSummaryDone = true
      logger.debug('ModsStore', '[selection-latency] end-to-end timing', {
        t0Ms: Math.round(latT0),
        totalMs: Math.round(latT3 - latT0),
        uiOptimisticMs: Math.round(latT1 - latT0),
        uiRenderFlushedMs: Math.round(latRenderFlush - latT0),
        backendMs: Math.round(latT2 - latT1),
        correctMs: Math.round(latT3 - latT2),
        groupTreeModsTotal: latTreeMods,
        ...latCtx,
      })
    }
    // 乐观回滚函数（try 内赋值，catch 可见；模块严格模式下块内函数声明不外提）
    let rollbackOptimistic: () => void = () => {}
    try {
      const isMutex = group.groupType === 'mutexGroup'
      // 后端 mod_index 必须以「当前分组真实列表（含 None 槽位）」中的下标为准，
      // 与后端扫描器的 mod_index / $managed_slot_id 同基准（None=0，真实模组从 1 起）
      const realModIdx = displayedMods.indexOf(mod)
      const modPath2 = mod.modPath || ''
      latCtx.realModIdx = realModIdx
      latCtx.modPath = modPath2
      // 使用 group_xx 目录编号（如 group_1 → 1）作为后端 group_index，
      // 而非 groups 数组下标。后端 switch_mod 与按键模拟的 active_group_id
      // 均以该编号为准；若存在互斥组或编号不连续（删除过分组），数组下标会与编号错位，
      // 导致游戏内 [KeyMod] 的 condition（$group_id == active_group_id）无法命中。
      const groupIdx = group.groupIndex

      // 诊断日志：追踪 realModIdx 的来源与上下文，用于排查鼠标模拟 X 轴异常
      // 详见 .trae/specs/diagnose-mouse-sim-x-axis/spec.md
      logger.debug('ModsStore', '[activateModByIndex] mod_index source diagnostic', {
        displayedModsLength: displayedMods.length,
        modPath: modPath2,
        realModIdx,
        groupIdx,
        groupPath: group.groupPath,
      })
      if (realModIdx < 0 || realModIdx >= displayedMods.length) {
        logger.warn('ModsStore', '[activateModByIndex] realModIdx out of range', {
          realModIdx,
          displayedModsLength: displayedMods.length,
          modPath: modPath2,
          groupIdx,
          groupPath: group.groupPath,
        })
      }

      // === UI 先行（乐观更新）：后端调用前即呈现目标选中态，消除秒级点击延迟 ===
      // 后端 mod_index 归一化口径：None 槽位占 0，真实模组从 1 起（误传 0 时按 1 处理）
      const normalizedIdx = realModIdx === 0 ? 1 : realModIdx
      const gPath = group.groupPath
      const gPathUsable = !gPath.startsWith('__') && gPath !== '__groups__'
      // 快照（回滚基准）：选中索引 / 分组索引映射 / 树节点 activeModIndex / 组内每模组 isActive
      const prevSelectedIdx = selectedModIndex.value
      const hadGroupIdxEntry = gPathUsable && Object.prototype.hasOwnProperty.call(selectedModIndicesByGroup, gPath)
      const prevGroupIdxVal = selectedModIndicesByGroup[gPath]
      const found = findGroupByPathInList(groups.value, gPath)
      const prevActiveIdx = found ? found.activeModIndex : undefined
      const prevActiveByPath = new Map<string, boolean>()
      function collectActiveRecursive(g: ModGroupData) {
        for (const m of g.mods) prevActiveByPath.set(m.modPath, m.isActive)
        for (const child of g.children) collectActiveRecursive(child)
      }
      collectActiveRecursive(group)
      // 扁平列表快照（独立实例，供回滚恢复；收藏/搜索结果视图的高亮直接读扁平 isActive）
      for (const fm of mods.value) {
        const prev = prevActiveByPath.get(fm.modPath)
        if (prev === undefined) prevActiveByPath.set(fm.modPath, fm.isActive)
      }
      // 递归更新分组树中所有模组的isActive状态（互斥组和普通组都是互斥语义：
      // 同一时间只有一个模组为active）
      function setActiveRecursive(g: ModGroupData, targetPath: string) {
        for (const m of g.mods) {
          m.isActive = m.modPath === targetPath
        }
        for (const child of g.children) {
          setActiveRecursive(child, targetPath)
        }
      }
      // 扁平列表同步（收藏/搜索视图高亮与组视图一致）
      function setActiveFlat(targetPath: string) {
        for (const m of mods.value) {
          m.isActive = m.modPath === targetPath
        }
      }
      // 乐观回滚：失败/防抖拒绝时完整恢复操作前快照（含全部派生字段）
      rollbackOptimistic = () => {
        selectedModIndex.value = prevSelectedIdx
        if (hadGroupIdxEntry) {
          selectedModIndicesByGroup[gPath] = prevGroupIdxVal
        } else if (gPathUsable) {
          delete selectedModIndicesByGroup[gPath]
        }
        if (!isMutex && found && prevActiveIdx !== undefined) {
          found.activeModIndex = prevActiveIdx
        }
        function restoreActiveRecursive(g: ModGroupData) {
          for (const m of g.mods) {
            const prev = prevActiveByPath.get(m.modPath)
            if (prev !== undefined) m.isActive = prev
          }
          for (const child of g.children) restoreActiveRecursive(child)
        }
        restoreActiveRecursive(group)
        // 扁平恢复（镜像树恢复）
        for (const m of mods.value) {
          const prev = prevActiveByPath.get(m.modPath)
          if (prev !== undefined) m.isActive = prev
        }
        mods.value = [...mods.value]
        // 换新数组引用 + 版本戳：驱动 currentGroupMods 重算并重渲染（shallowRef 断链修复）
        group.mods = [...group.mods]
        modTreeVersion.value++
        // shallowRef：树内字段 mutate 需显式触发更新
        triggerRef(mods)
        triggerRef(groups)
      }

      // 应用乐观态：选中索引 / 分组映射 / 树节点 activeModIndex（非互斥）/ isActive
      selectedModIndex.value = normalizedIdx
      if (gPathUsable) {
        selectedModIndicesByGroup[gPath] = normalizedIdx
      }
      if (!isMutex && found) {
        found.activeModIndex = normalizedIdx
      }
      setActiveRecursive(group, modPath2)
      setActiveFlat(modPath2)
      // 换新数组引用 + 版本戳：乐观选中态即时渲染（shallowRef 断链修复）
      group.mods = [...group.mods]
      mods.value = [...mods.value]
      modTreeVersion.value++
      // shallowRef：树内模组 isActive 乐观更新需显式触发 UI 刷新
      triggerRef(mods)
      triggerRef(groups)

      // [selection-latency] 乐观更新应用完成（含快照收集 / 递归 isActive / 数组复制）
      latT1 = performance.now()
      // 渲染 flush 异步打点：Vue 批量更新提交、UI 实际可呈现的时刻（不阻塞主流程）
      void nextTick().then(() => {
        latRenderFlush = performance.now()
        latTryEmitSummary()
      })

      logger.debug('ModsStore', '[activateModByIndex] calling selectMod', {
        game: s.currentGame,
        modsPath: s.currentModsPath,
        groupIndex: groupIdx,
        modIndex: realModIdx,
        isMutex,
        groupPath: group.groupPath,
        modPath: modPath2,
      })

      // 调用后端select_mod命令处理selectedindex写入和互斥逻辑（UI 已先行，不再阻塞呈现）
      const result = await selectMod(
        s.currentGame,
        s.currentModsPath,
        groupIdx,
        realModIdx,
        isMutex,
        group.groupPath,
        modPath2,
        undefined, // cursorX - pass undefined for now (no coordinate calculation yet)
        undefined, // cursorY - pass undefined for now (no coordinate calculation yet)
      )
      // [selection-latency] IPC 返回：后端 select_mod 全程耗时（含锁等待 / 直写 / patch / 按键模拟）
      latT2 = performance.now()
      logger.debug('ModsStore', '[activateModByIndex] selectMod result', {
        selectedModIndex: result?.selectedModIndex,
      })
      // 注意：模组选择不触发 markNeedUpdate，因为选择不涉及模组数据修改
      // 仅在启用/禁用模组时才需要提醒用户更新模组数据

      // 成功：以后端写入 selectedindex 的返回值为准无条件重断言 UI（防索引一致时并发回退残留旧选中态）
      const retSelIdx = result?.selectedModIndex ?? realModIdx
      const confirmPath = (group.mods[retSelIdx]?.modPath) || modPath2
      selectedModIndex.value = retSelIdx
      if (gPathUsable) selectedModIndicesByGroup[gPath] = retSelIdx
      if (!isMutex && found) found.activeModIndex = retSelIdx
      setActiveRecursive(group, confirmPath)
      setActiveFlat(confirmPath)
      group.mods = [...group.mods]
      modTreeVersion.value++
      triggerRef(mods)
      triggerRef(groups)
      ElMessage.success('已选择模组')

      // [selection-latency] 校正完成 → 打端到端汇总（若渲染 flush 已先到）
      latT3 = performance.now()
      latTryEmitSummary()
    } catch (e: unknown) {
      const errMsg = typeof e === 'string' ? e : (e instanceof Error ? e.message : String(e))
      // 防抖命中（P1 修复）：回滚本次乐观变更后不再静默丢弃最后一次选择意图。
      // 原实现直接 return，用户双击后「点了没反应」且无任何提示；改为
      // trailing-edge 重发一次——延迟略大于后端防抖窗口（500ms）后重放，
      // 保证同组快速连选 A→B 时 B 最终一定生效。仅重试一次，避免抖动循环。
      if (errMsg === 'debounced') {
        rollbackOptimistic()
        if (isRetry) {
          logger.warn('ModsStore', 'activateModByIndex debounced again on retry, giving up', {
            modIdx,
            modPath,
          })
          return
        }
        logger.debug('ModsStore', 'activateModByIndex debounced → schedule trailing-edge retry', {
          modIdx,
          modPath,
          delayMs: SELECTION_DEBOUNCE_RETRY_MS,
        })
        if (pendingActivateRetry !== null) clearTimeout(pendingActivateRetry)
        // 记录意图发生时的分组：定时器触发时若用户已切到别的分组，则丢弃这次重试，
        // 避免用「旧分组的下标/路径」在新分组上选中错误的模组（无 modPath 的调用方
        // 会退化为按 modIdx 定位，跨分组即错位）。
        const retryGroupPath = group.groupPath
        pendingActivateRetry = setTimeout(() => {
          pendingActivateRetry = null
          if (currentGroup.value?.groupPath !== retryGroupPath) {
            logger.warn('ModsStore', 'activateModByIndex retry dropped: group changed', {
              from: retryGroupPath,
              to: currentGroup.value?.groupPath,
            })
            return
          }
          void activateModByIndex(modIdx, modPath, true)
        }, SELECTION_DEBOUNCE_RETRY_MS)
        return
      }
      // 其它错误：回滚快照并告知用户（fire-and-forget），保持异常冒泡供调用方感知
      rollbackOptimistic()
      logger.error('ModsStore', 'Failed to activate mod', e)
      ElMessage.error(`选择模组失败：${errMsg}`)
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
   * 所有非 mutexGroup 类型（normalGroup/exclusiveSlot/customParallel）均按 NormalGroup 处理。
   * 虚拟节点（__ 开头）不执行任何操作（兜底 guard）。
   *
   * @param groupType  当前分组类型
   * @param groupIndex group_xx 编号（NormalGroup 写 selectedindex 用）
   * @param groupPath  分组目录绝对路径（MutexGroup 批量禁用用；NormalGroup 也传入用于日志）
   */
  async function deselectOrDisableNoneSlot(
    groupType: string,
    groupIndex: number,
    groupPath: string,
  ) {
    const isMutex = groupType === 'mutexGroup'
    if (groupType.startsWith('__') || groupType === '__groups__') {
      logger.warn('mods', 'deselectOrDisableNoneSlot called on virtual group, skip', { groupType })
      return
    }
    if (isUpdatingModData.value || isActivating.value) return
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    isActivating.value = true
    // 乐观回滚函数（try 内赋值，catch 可见；模块严格模式下块内函数声明不外提）
    let rollbackOptimistic: () => void = () => {}
    try {
      if (!isMutex) {
        logger.debug('mods', 'deselectOrDisableNoneSlot → deselectGroupMod', { groupIndex, groupPath })
        // === UI 先行（乐观更新）：取消选择立即呈现，不等后端 ===
        const gPathUsable = !groupPath.startsWith('__') && groupPath !== '__groups__'
        const prevSelectedIdx = selectedModIndex.value
        const hadGroupIdxEntry = gPathUsable && Object.prototype.hasOwnProperty.call(selectedModIndicesByGroup, groupPath)
        const prevGroupIdxVal = selectedModIndicesByGroup[groupPath]
        const found = findGroupByPathInList(groups.value, groupPath)
        const prevActiveIdx = found ? found.activeModIndex : undefined
        const prevActiveByPath = new Map<string, boolean>()
        if (found) {
          for (const m of found.mods) prevActiveByPath.set(m.modPath, m.isActive)
        }
        rollbackOptimistic = () => {
          selectedModIndex.value = prevSelectedIdx
          if (hadGroupIdxEntry) {
            selectedModIndicesByGroup[groupPath] = prevGroupIdxVal
          } else if (gPathUsable) {
            delete selectedModIndicesByGroup[groupPath]
          }
          if (found && prevActiveIdx !== undefined) {
            found.activeModIndex = prevActiveIdx
          }
          if (found) {
            for (const m of found.mods) {
              const prev = prevActiveByPath.get(m.modPath)
              if (prev !== undefined) m.isActive = prev
            }
            // 换新数组引用 + 版本戳：驱动渲染链重算（shallowRef 断链修复）
            found.mods = [...found.mods]
          }
          modTreeVersion.value++
          triggerRef(groups)
        }
        // 应用乐观态：选中索引置 0；组内激活态按后端派生口径（单一真源镜像）同步
        selectedModIndex.value = 0
        if (gPathUsable) {
          selectedModIndicesByGroup[groupPath] = 0
        }
        if (found) {
          found.activeModIndex = deriveGroupActiveState(found.mods, 0)
          // 换新数组引用 + 版本戳：取消选择态即时渲染（shallowRef 断链修复）
          found.mods = [...found.mods]
          modTreeVersion.value++
          triggerRef(groups)
        }

        // 直写快路径（后端 O(1) 写 selectedindex=0 + 缓存定向 patch）；
        // 成功后不再全量 refresh：UI 已本地一致，回声被 WatcherGuard 抑制，对账走 watcher 兜底
        await deselectGroupMod(s.currentGame, s.currentModsPath, groupIndex, groupPath)
      } else {
        logger.debug('mods', 'deselectOrDisableNoneSlot → disableAllModsInGroup', { groupPath })
        // === UI 先行（乐观更新）：预测禁用 pairs 并本地批量 patch ===
        const found = findGroupByPathInList(groups.value, groupPath)
        const groupMods = found ? found.mods : []
        const optimisticPairs: { oldPath: string; newPath: string; disabled: boolean }[] = []
        for (const m of groupMods) {
          if (m.name === 'None' || m.modPath === 'None') continue
          if (m.modDisabled) continue // 已禁用跳过（后端幂等口径）
          optimisticPairs.push({ oldPath: m.modPath, newPath: predictToggledPath(m.modPath, true), disabled: true })
        }
        if (optimisticPairs.length > 0) {
          batchToggleModLocal(optimisticPairs)
        }
        rollbackOptimistic = () => {
          if (optimisticPairs.length > 0) {
            batchToggleModLocal(
              optimisticPairs.map((p) => ({ oldPath: p.newPath, newPath: p.oldPath, disabled: false })),
            )
          }
        }

        const batch = await disableAllModsInGroup(groupPath)
        if (batch.degraded) {
          // 异常降级：组增量原地 patch（磁盘真相覆盖乐观态）
          applyGroupDelta(batch.delta)
        } else {
          // 零扫描校正：后端权威路径与预测不一致的条目二次 patch（同口径幂等下通常一致）
          const predictedByOld = new Map(optimisticPairs.map((p) => [p.oldPath, p.newPath]))
          for (const t of batch.toggled) {
            const predicted = predictedByOld.get(t.oldPath)
            if (predicted !== undefined && predicted !== t.newPath) {
              toggleModLocal(predicted, t.newPath, true)
            } else if (predicted === undefined) {
              toggleModLocal(t.oldPath, t.newPath, true)
            }
          }
        }
        logger.info('mods', `Disabled ${batch.changed} mods in mutex group ${groupPath}`)
      }
    } catch (e: unknown) {
      // 失败：回滚乐观变更后冒泡（错误提示由调用方 ModCard 统一弹出，避免双重 toast）
      rollbackOptimistic()
      logger.error('mods', 'deselectOrDisableNoneSlot failed', e)
      throw e
    } finally {
      isActivating.value = false
    }
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
    // 使所有 in-flight 加载/刷新失效：切游戏后旧游戏的响应不得落地（竞态防护）
    loadEpoch++
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
    // 清空按键绑定页目标模组（切游戏后旧模组不残留，对齐 NRMM setModDir 的游戏匹配校验）
    keybindTargetMod.value = null
    // C18：清空加载失败反馈（clearData 后旧错误不残留）
    loadError.value = null
    // 清空非法 UTF-8 通知（切游戏/清数据后旧提示不残留）
    invalidUtf8Notice.value = null
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
    const startEpoch = loadEpoch // 防护起点：await 期间可能发生 clearData（窗口隐藏清空）/切游戏
    isUpdatingModData.value = true
    loading.value = true
    try {
      const lastResult = await tauriUpdateModData(s.currentGame, s.currentModsPath)
      // 主 INI（游戏根 d3dx.ini）缺失：模组处理完成但游戏侧不会加载管理段，显式警告（对齐修复目标，
      // 不再静默跳过让用户误以为「更新未生效」）
      if (lastResult.mainIniStatus === 'MissingMainIni') {
        ElMessage.warning('未找到游戏根主 INI（d3dx.ini），游戏内模组管理不会生效，请确认游戏根目录包含 d3dx.ini')
      }
      // 竞态防护（C14）：更新期间数据已被清空/切换（epoch 变化）→ 不回灌加载（刚释放的内存不再拉回）。
      // 更新本身已成功，仍需清除 needUpdate 标记（否则提醒条残留造成「更新没生效」假象）。
      if (startEpoch !== loadEpoch) {
        clearNeedUpdate()
        return lastResult
      }
      needReloadManual.value = lastResult.needReloadManual
      updateResult.value = lastResult
      // 非法 UTF-8 路径提示：深扫跳过项直接上报（update_mod_data 是全量深扫，最全）
      invalidUtf8Notice.value = applyInvalidUtf8Notice(lastResult.invalidUtf8Count ?? 0, lastResult.invalidUtf8Entries)

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
    loadError,
    invalidUtf8Notice,
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
    pendingReloadAfterRecreate,
    currentGroup,
    currentGroupMods,
    selectedMod,
    keybindTargetMod,
    selectGroup,
    loadMods,
    refresh,
    // ========== 命令级分组增量（R1） ==========
    lastLocalPatchAt,
    applyGroupDelta,
    hasGroup,
    toggleFavoriteLocal,
    toggleModLocal,
    batchToggleModLocal,
    removeModLocal,
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
