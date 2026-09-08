/**
 * mods store None 槽位取消选择链路测试（UI 先行 + 去 refresh + 回滚，2026-09-06 计划 Task 4）。
 *
 * 覆盖场景：
 * - L1 NormalGroup 取消选择：deselectGroupMod 挂起时同步调用后取消态立即呈现
 *   （selectedModIndex=0 / activeModIndex=0 / None 激活、其余失活）
 * - 去 refresh：成功后不再触发全量 refreshMods/getMods（后端缓存已定向 patch）
 * - 失败回滚：deselectGroupMod 异常时恢复操作前快照并冒泡
 * - MutexGroup 双击 None：乐观批量 patch（预测禁用路径）先行；成功零校正；失败反向回滚
 *
 * 实现方式：与 mods.loadEpoch.test.ts 相同的 mock 基建（顶层 vi.mock 拦截
 * `../utils/tauri` 与 `./settings`），element-plus 经 importOriginal 部分替换。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

/** 可变 settings 状态（被 mock 的 useSettingsStore 读取） */
const settingsState = { currentModsPath: '', currentGame: '' }

vi.mock('../utils/tauri', () => ({
  getMods: (...args: unknown[]) => getModsMock(...(args as [])),
  refreshMods: (...args: unknown[]) => refreshModsMock(...(args as [])),
  selectMod: vi.fn(),
  switchFileWatcher: vi.fn(),
  stopFileWatcher: vi.fn(),
  updateModData: vi.fn(),
  deselectGroupMod: (...args: unknown[]) => deselectGroupModMock(...(args as [])),
  disableAllModsInGroup: (...args: unknown[]) => disableAllModsInGroupMock(...(args as [])),
  syncNrmmUpdateFlag: vi.fn(),
}))
vi.mock('./settings', () => ({
  useSettingsStore: () => settingsState,
}))
vi.mock('element-plus', async (importOriginal) => ({
  ...(await importOriginal<typeof import('element-plus')>()),
  ElMessage: {
    error: vi.fn(),
    warning: vi.fn(),
    success: vi.fn(),
    info: vi.fn(),
  },
}))

// mock 声明需先于被导入模块（vi.mock 提升保证执行顺序，变量声明提升由 vitest 处理）
const getModsMock = vi.fn()
const refreshModsMock = vi.fn()
const deselectGroupModMock = vi.fn()
const disableAllModsInGroupMock = vi.fn()

import { useModsStore } from './mods'
import type { ModData, ModGroupData } from '@/types'

/** 构造最小模组数据 */
function makeMod(modPath: string, name: string, overrides: Partial<ModData> = {}): ModData {
  return {
    modPath,
    modName: name,
    name,
    isActive: false,
    isFavorite: false,
    isNamespaced: false,
    erroredLines: [],
    namespaces: [],
    modDisabled: false,
    modIndex: 0,
    disabled: false,
    groupIndex: 0,
    ...overrides,
  } as unknown as ModData
}

/** 构造最小分组（None 槽位 + 两个真实模组） */
function makeTestGroup(groupType: 'normalGroup' | 'mutexGroup'): {
  group: ModGroupData
  none: ModData
  modA: ModData
  modB: ModData
} {
  const none = makeMod('None', 'None')
  const modA = makeMod('/mods/_MANAGED_/group_1/ModA', 'ModA')
  const modB = makeMod('/mods/_MANAGED_/group_1/ModB', 'ModB')
  const group = {
    groupPath: '/mods/_MANAGED_/group_1',
    name: 'group_1',
    groupType,
    activeModIndex: 1,
    mods: [none, modA, modB],
    children: [],
  } as unknown as ModGroupData
  return { group, none, modA, modB }
}

/** 初始化 store 并装载测试分组（默认选中态：ModA 激活，selectedModIndex=1） */
function setupStore(groupType: 'normalGroup' | 'mutexGroup' = 'normalGroup') {
  const store = useModsStore()
  const ctx = makeTestGroup(groupType)
  ctx.modA.isActive = true
  store.groups = [ctx.group]
  store.selectedGroupPath = ctx.group.groupPath
  store.selectedModIndex = 1
  return { store, ...ctx }
}

describe('mods store None 槽位取消选择（NormalGroup 直写 + 乐观更新）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    getModsMock.mockReset()
    refreshModsMock.mockReset()
    deselectGroupModMock.mockReset()
    disableAllModsInGroupMock.mockReset()
    settingsState.currentModsPath = '/mods/GI'
    settingsState.currentGame = 'GI'
  })

  it('L1 deselectGroupMod 挂起时同步调用后取消态立即呈现', async () => {
    const { store, group, none, modA } = setupStore()
    let resolveDeselect: (v: unknown) => void = () => {}
    deselectGroupModMock.mockReturnValueOnce(
      new Promise((r) => (resolveDeselect = r)),
    )

    // 不 await：调用后同步断言 UI 已先行呈现取消选择态
    const pending = store.deselectOrDisableNoneSlot('normalGroup', 1, group.groupPath)
    expect(store.selectedModIndex).toBe(0)
    expect(group.activeModIndex).toBe(0)
    expect(none.isActive).toBe(true)
    expect(modA.isActive).toBe(false)

    resolveDeselect({ selectedModIndex: 0 })
    await pending
    // 状态稳定
    expect(store.selectedModIndex).toBe(0)
  })

  it('成功后不再触发全量 refresh（refreshMods/getMods 零调用）', async () => {
    const { store, group } = setupStore()
    deselectGroupModMock.mockResolvedValueOnce({ selectedModIndex: 0 })

    await store.deselectOrDisableNoneSlot('normalGroup', 1, group.groupPath)

    expect(refreshModsMock).not.toHaveBeenCalled()
    expect(getModsMock).not.toHaveBeenCalled()
    // 直写路径参数：groupPath 一并传给后端（快路径守卫依据）
    expect(deselectGroupModMock).toHaveBeenCalledWith('GI', '/mods/GI', 1, group.groupPath)
  })

  it('失败回滚：恢复选中索引与组内激活态并冒泡', async () => {
    const { store, group, none, modA } = setupStore()
    deselectGroupModMock.mockRejectedValueOnce(new Error('write failed'))

    await expect(
      store.deselectOrDisableNoneSlot('normalGroup', 1, group.groupPath),
    ).rejects.toThrow('write failed')

    expect(store.selectedModIndex).toBe(1)
    expect(group.activeModIndex).toBe(1)
    expect(modA.isActive).toBe(true)
    expect(none.isActive).toBe(false)
  })
})

describe('mods store None 槽位批量禁用（MutexGroup 乐观批量）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    getModsMock.mockReset()
    refreshModsMock.mockReset()
    deselectGroupModMock.mockReset()
    disableAllModsInGroupMock.mockReset()
    settingsState.currentModsPath = '/mods/GI'
    settingsState.currentGame = 'GI'
  })

  it('L1 乐观批量先行：预测禁用路径立即本地 patch；成功零校正零 refresh', async () => {
    const { store, group, modA, modB } = setupStore('mutexGroup')
    let resolveBatch: (v: unknown) => void = () => {}
    disableAllModsInGroupMock.mockReturnValueOnce(
      new Promise((r) => (resolveBatch = r)),
    )

    // 不 await：调用后同步断言乐观批量已生效（路径预测 + 禁用态）
    const pending = store.deselectOrDisableNoneSlot('mutexGroup', 1, group.groupPath)
    expect(modA.modPath).toBe('/mods/_MANAGED_/group_1/DISABLEDModA')
    expect(modA.modDisabled).toBe(true)
    expect(modB.modPath).toBe('/mods/_MANAGED_/group_1/DISABLEDModB')
    expect(modB.modDisabled).toBe(true)

    // 后端权威返回与预测一致 → 零校正
    resolveBatch({
      changed: 2,
      toggled: [
        { oldPath: '/mods/_MANAGED_/group_1/ModA', newPath: '/mods/_MANAGED_/group_1/DISABLEDModA' },
        { oldPath: '/mods/_MANAGED_/group_1/ModB', newPath: '/mods/_MANAGED_/group_1/DISABLEDModB' },
      ],
      degraded: false,
      delta: null,
    })
    await pending

    expect(modA.modPath).toBe('/mods/_MANAGED_/group_1/DISABLEDModA')
    expect(refreshModsMock).not.toHaveBeenCalled()
    expect(getModsMock).not.toHaveBeenCalled()
  })

  it('失败反向回滚：恢复原路径与启用态并冒泡', async () => {
    const { store, group, modA, modB } = setupStore('mutexGroup')
    disableAllModsInGroupMock.mockRejectedValueOnce(new Error('rename denied'))

    await expect(
      store.deselectOrDisableNoneSlot('mutexGroup', 1, group.groupPath),
    ).rejects.toThrow('rename denied')

    expect(modA.modPath).toBe('/mods/_MANAGED_/group_1/ModA')
    expect(modA.modDisabled).toBe(false)
    expect(modB.modPath).toBe('/mods/_MANAGED_/group_1/ModB')
    expect(modB.modDisabled).toBe(false)
  })
})
