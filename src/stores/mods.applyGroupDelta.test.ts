/**
 * mods store 命令级分组增量测试（R1，mod-state-delta 方案阶段一）。
 *
 * 覆盖场景：
 * - applyGroupDelta 替换组：旧扁平条目剔除 + 新条目按最小旧索引原位插入 + 树节点原位替换
 * - applyGroupDelta 移除组（newGroup=null）：树节点移除 + 扁平剔除 + selectedGroupPath 回退
 * - None 空槽位不进扁平列表（含 oldModPaths 为空时从树节点递归收集的回退路径）
 * - 互斥组子分组递归收集（children 内模组进扁平；分组重命名 oldGroupPath ≠ 新路径）
 * - selectedModIndex 越界修正
 * - 纯本地 patch：不触发 refreshMods 全量 IPC + lastLocalPatchAt 回声抑制时间戳语义
 * - toggleFavoriteLocal：isFavorite 翻转（扁平与树两份独立实例）+ 组内近似重排（收藏前置不顶替 None、取消后移）
 *
 * 实现方式：与 mods.loadEpoch.test.ts 相同的 mock 基建（顶层 vi.mock 拦截
 * `../utils/tauri` 与 `./settings`），直接对 store 状态断言。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

/** 可变 settings 状态（被 mock 的 useSettingsStore 读取） */
const settingsState = { currentModsPath: '', currentGame: '' }

vi.mock('../utils/tauri', () => ({
  getMods: vi.fn(),
  refreshMods: (...args: unknown[]) => refreshModsMock(...(args as [])),
  selectMod: vi.fn(),
  switchFileWatcher: vi.fn(),
  stopFileWatcher: vi.fn(),
  updateModData: vi.fn(),
  deselectGroupMod: vi.fn(),
  disableAllModsInGroup: vi.fn(),
  syncNrmmUpdateFlag: vi.fn(),
}))
vi.mock('./settings', () => ({
  useSettingsStore: () => settingsState,
}))

// mock 声明需先于被导入模块（vi.mock 提升保证执行顺序，变量声明提升由 vitest 处理）
const refreshModsMock = vi.fn()

import { useModsStore } from './mods'
import type { ModData, ModGroupData, GroupDelta } from '@/types'

/** 构造最小模组数据（默认启用、未收藏） */
function makeMod(modPath: string, name: string, overrides: Partial<ModData> = {}): ModData {
  return {
    modPath,
    modName: name,
    name,
    modIni: null,
    isActive: false,
    isFavorite: false,
    isNamespaced: false,
    hasNonmanagedModsCrashlineFix: false,
    erroredLines: [],
    erroredPreexisting: [],
    missingEndif: [],
    namespaces: [],
    knownLibraries: [],
    duplicateLibraries: [],
    nonexistentLibraries: [],
    namespaceError: false,
    modDisabled: false,
    pathTooLong: false,
    modIndex: 0,
    isMutex: false,
    disabled: false,
    groupIndex: 0,
    ...overrides,
  }
}

/** 构造 None 空槽位（NormalGroup 组内首位占位） */
function makeNoneSlot(): ModData {
  return makeMod('None', 'None')
}

/** 构造最小分组数据 */
function makeGroup(groupPath: string, mods: ModData[], overrides: Partial<ModGroupData> = {}): ModGroupData {
  const name = groupPath.split('/').pop() || groupPath
  return {
    groupPath,
    groupName: name,
    name,
    groupId: 0,
    groupIndex: 0,
    mods,
    modCount: mods.filter((m) => m.name !== 'None').length,
    isActive: false,
    isFavorite: false,
    groupDisabled: false,
    groupType: 'normalGroup',
    hasChild: false,
    children: [],
    activeModIndex: -1,
    ...overrides,
  }
}

describe('mods store 命令级分组增量（applyGroupDelta）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    refreshModsMock.mockReset()
    settingsState.currentModsPath = '/mods/GI'
    settingsState.currentGame = 'GI'
  })

  it('R1-1 替换组：旧扁平条目剔除、新条目按最小旧索引原位插入、树节点原位替换', () => {
    const store = useModsStore()
    const other1 = makeMod('/g/B/modB1', 'modB1')
    const other2 = makeMod('/g/B/modB2', 'modB2')
    const oldA1 = makeMod('/g/A/modA1', 'modA1')
    const oldA2 = makeMod('/g/A/modA2', 'modA2')
    const groupA = makeGroup('/g/A', [makeNoneSlot(), oldA1, oldA2])
    const groupB = makeGroup('/g/B', [makeNoneSlot(), other1, other2])
    store.groups = [groupB, groupA]
    store.mods = [other1, other2, oldA1, oldA2]

    // 禁用场景：modA1 目录加 DISABLED_ 前缀 → modPath 变化，整组以后端新状态替换
    const newA1 = makeMod('/g/A/DISABLED_modA1', 'DISABLED_modA1', { modDisabled: true })
    const newA2 = makeMod('/g/A/modA2', 'modA2')
    const newGroupA = makeGroup('/g/A', [makeNoneSlot(), newA1, newA2])
    const delta: GroupDelta = {
      oldGroupPath: '/g/A',
      newGroup: newGroupA,
      oldModPaths: ['/g/A/modA1', '/g/A/modA2'],
    }

    store.applyGroupDelta(delta)

    // 扁平：旧条目剔除，新条目插入在首个旧条目位置（groupB 两个条目之后）
    expect(store.mods.map((m) => m.modPath)).toEqual([
      '/g/B/modB1',
      '/g/B/modB2',
      '/g/A/DISABLED_modA1',
      '/g/A/modA2',
    ])
    // 树：节点被原位替换为 delta.newGroup（同一对象实例，父数组位置不变）
    expect(store.groups[0]).toBe(groupB)
    expect(store.groups[1]).toBe(newGroupA)
  })

  it('R1-2 移除组（newGroup=null）：树节点移除、扁平剔除、selectedGroupPath 回退第一个分组', () => {
    const store = useModsStore()
    const a1 = makeMod('/g/A/modA1', 'modA1')
    const a2 = makeMod('/g/A/modA2', 'modA2')
    const b1 = makeMod('/g/B/modB1', 'modB1')
    const groupA = makeGroup('/g/A', [makeNoneSlot(), a1, a2])
    const groupB = makeGroup('/g/B', [makeNoneSlot(), b1])
    store.groups = [groupA, groupB]
    store.mods = [a1, a2, b1]
    store.selectedGroupPath = '/g/A'

    store.applyGroupDelta({ oldGroupPath: '/g/A', newGroup: null, oldModPaths: ['/g/A/modA1', '/g/A/modA2'] })

    expect(store.groups).toEqual([groupB])
    expect(store.mods.map((m) => m.modPath)).toEqual(['/g/B/modB1'])
    // 被移除组正是当前选中组 → 回退到第一个分组
    expect(store.selectedGroupPath).toBe('/g/B')
  })

  it('R1-3 None 空槽位不进扁平列表（oldModPaths 为空时从树节点收集亦排除 None）', () => {
    const store = useModsStore()
    const a1 = makeMod('/g/A/modA1', 'modA1')
    const groupA = makeGroup('/g/A', [makeNoneSlot(), a1])
    store.groups = [groupA]
    store.mods = [a1]

    const newGroupA = makeGroup('/g/A', [makeNoneSlot(), makeMod('/g/A/modA1', 'modA1')])

    // oldModPaths 为空 → 回退从旧树节点递归收集（排除 None 槽位）
    store.applyGroupDelta({ oldGroupPath: '/g/A', newGroup: newGroupA, oldModPaths: [] })

    // 扁平列表不含 None（后端 scan_mods_light 扁平列表语义）
    expect(store.mods.map((m) => m.modPath)).toEqual(['/g/A/modA1'])
    expect(store.mods.some((m) => m.modPath === 'None' || m.name === 'None')).toBe(false)
    // 树节点保留 None 槽位（组内槽位选择展示依赖）
    expect(store.groups[0].mods[0].name).toBe('None')
  })

  it('R1-4 互斥组重命名：递归收集 children 子分组模组进扁平，树整组替换（新旧 groupPath 不同）', () => {
    const store = useModsStore()
    const directMod = makeMod('/m/root/modD1', 'modD1')
    const childMod = makeMod('/m/root/child/modC1', 'modC1')
    const child = makeGroup('/m/root/child', [childMod], { groupType: 'mutexGroup' })
    const root = makeGroup('/m/root', [directMod], { groupType: 'mutexGroup', children: [child], hasChild: true })
    store.groups = [root]
    store.mods = [directMod, childMod]

    const newChild = makeGroup('/m/root2/child', [makeMod('/m/root2/child/modC1', 'modC1')], { groupType: 'mutexGroup' })
    const newRoot = makeGroup('/m/root2', [makeMod('/m/root2/modD1', 'modD1')], {
      groupType: 'mutexGroup',
      children: [newChild],
      hasChild: true,
    })

    store.applyGroupDelta({
      oldGroupPath: '/m/root',
      newGroup: newRoot,
      oldModPaths: ['/m/root/modD1', '/m/root/child/modC1'],
    })

    // 扁平：子分组模组同样被剔除并以新路径插入
    expect(store.mods.map((m) => m.modPath)).toEqual(['/m/root2/modD1', '/m/root2/child/modC1'])
    expect(store.groups[0]).toBe(newRoot)
  })

  it('R1-5 选中索引越界修正：新组条目变少时 selectedModIndex 收敛到最后有效索引', () => {
    const store = useModsStore()
    const a1 = makeMod('/g/A/modA1', 'modA1')
    const a2 = makeMod('/g/A/modA2', 'modA2')
    const a3 = makeMod('/g/A/modA3', 'modA3')
    const groupA = makeGroup('/g/A', [makeNoneSlot(), a1, a2, a3])
    store.groups = [groupA]
    store.mods = [a1, a2, a3]
    store.selectedGroupPath = '/g/A'
    store.selectedModIndex = 3 // 指向 a3（含 None 槽位的组内索引）

    // 移除模组后新组仅剩 None + modA1（组内长度 2）
    const newGroupA = makeGroup('/g/A', [makeNoneSlot(), makeMod('/g/A/modA1', 'modA1')])
    store.applyGroupDelta({
      oldGroupPath: '/g/A',
      newGroup: newGroupA,
      oldModPaths: ['/g/A/modA1', '/g/A/modA2', '/g/A/modA3'],
    })

    expect(store.selectedModIndex).toBe(1)
  })

  it('R1-6 纯本地 patch：不触发 refreshMods 全量 IPC，并置位 lastLocalPatchAt 回声抑制时间戳', () => {
    const store = useModsStore()
    const a1 = makeMod('/g/A/modA1', 'modA1')
    store.groups = [makeGroup('/g/A', [makeNoneSlot(), a1])]
    store.mods = [a1]

    // 初始未发生过本地 patch
    expect(store.lastLocalPatchAt).toBe(0)

    const before = Date.now()
    store.applyGroupDelta({ oldGroupPath: '/g/A', newGroup: null, oldModPaths: ['/g/A/modA1'] })
    const after = Date.now()

    // 零全量 refresh：未发起任何 refreshMods IPC
    expect(refreshModsMock).not.toHaveBeenCalled()
    // 时间戳落在 patch 调用时间窗内
    expect(store.lastLocalPatchAt).toBeGreaterThanOrEqual(before)
    expect(store.lastLocalPatchAt).toBeLessThanOrEqual(after)
    // ModsView 回声抑制判定依据：patch 后短窗口内为真（回声跳过）
    expect(Date.now() - store.lastLocalPatchAt < 2000).toBe(true)
    // 对照：从未 patch（时间戳为 0）时判定为假（外部真实变更不受抑制）
    expect(Date.now() - 0 < 2000).toBe(false)
  })
})

describe('mods store 收藏本地翻转（toggleFavoriteLocal）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    refreshModsMock.mockReset()
    settingsState.currentModsPath = '/mods/GI'
    settingsState.currentGame = 'GI'
  })

  it('R1-7 isFavorite 翻转：扁平与树内同路径条目同步翻转（两处为不同对象实例）', () => {
    const store = useModsStore()
    // 后端 ScanResult 的 groups 与 mods 独立序列化 → 前端为两个不同对象实例
    const treeMod = makeMod('/g/A/modA1', 'modA1', { isFavorite: false })
    const flatMod = makeMod('/g/A/modA1', 'modA1', { isFavorite: false })
    const otherTree = makeMod('/g/A/modA2', 'modA2')
    const otherFlat = makeMod('/g/A/modA2', 'modA2')
    store.groups = [makeGroup('/g/A', [makeNoneSlot(), treeMod, otherTree])]
    store.mods = [flatMod, otherFlat]

    store.toggleFavoriteLocal('/g/A/modA1', true)

    expect(store.mods.find((m) => m.modPath === '/g/A/modA1')?.isFavorite).toBe(true)
    const g = store.groups.find((x) => x.groupPath === '/g/A')
    expect(g).toBeDefined()
    expect(g!.mods.find((m) => m.modPath === '/g/A/modA1')?.isFavorite).toBe(true)
  })

  it('R1-8 组内近似重排：收藏前置（不顶替 None 槽位）、取消收藏后移', () => {
    const store = useModsStore()
    const m1 = makeMod('/g/A/m1', 'm1')
    const m2 = makeMod('/g/A/m2', 'm2')
    const m3 = makeMod('/g/A/m3', 'm3')
    store.groups = [makeGroup('/g/A', [makeNoneSlot(), m1, m2, m3])]
    store.mods = [m1, m2, m3]

    store.toggleFavoriteLocal('/g/A/m3', true)
    // None 恒为首位（组内 mod_index 以 None=0 为基准）；收藏的 m3 前置到 None 之后
    expect(store.groups[0].mods.map((m) => m.modPath)).toEqual(['None', '/g/A/m3', '/g/A/m1', '/g/A/m2'])

    store.toggleFavoriteLocal('/g/A/m3', false)
    // 取消收藏 → 后移到组尾
    expect(store.groups[0].mods.map((m) => m.modPath)).toEqual(['None', '/g/A/m1', '/g/A/m2', '/g/A/m3'])
  })

  it('R1-9 toggleFavoriteLocal 亦置位 lastLocalPatchAt（fav 文件 watcher 回声同样需要抑制）', () => {
    const store = useModsStore()
    const m1 = makeMod('/g/A/m1', 'm1')
    store.groups = [makeGroup('/g/A', [makeNoneSlot(), m1])]
    store.mods = [m1]

    const before = Date.now()
    store.toggleFavoriteLocal('/g/A/m1', true)

    expect(refreshModsMock).not.toHaveBeenCalled()
    expect(store.lastLocalPatchAt).toBeGreaterThanOrEqual(before)
  })
})
