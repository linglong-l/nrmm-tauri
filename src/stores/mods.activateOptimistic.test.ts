/**
 * mods store 选择链路乐观更新测试（UI 先行 + 快照回滚，2026-09-06 计划 Task 2）。
 *
 * 覆盖场景：
 * - L1 UI 先行：selectMod 挂起（pending Promise）时，同步调用后选中态立即呈现
 *   （selectedModIndex / activeModIndex / 组内 isActive），不等待后端返回
 * - S2 防抖回滚：后端拒绝 'debounced' 时静默完整回滚乐观变更（无错误提示）
 * - 错误回滚：后端异常时回滚快照 + ElMessage.error 告知 + 异常冒泡 + 状态复位
 * - 返回值校正：后端返回索引与乐观值不一致时以后端权威值校正
 * - 并发守卫：selectMod 在途时再次点击为 no-op（selectMod 仅调用一次）
 *
 * 实现方式：与 mods.loadEpoch.test.ts 相同的 mock 基建（顶层 vi.mock 拦截
 * `../utils/tauri` 与 `./settings`），element-plus 经 importOriginal 部分替换
 * 以捕获 ElMessage 调用。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

/** 可变 settings 状态（被 mock 的 useSettingsStore 读取） */
const settingsState = { currentModsPath: '', currentGame: '' }

vi.mock('../utils/tauri', () => ({
  getMods: vi.fn(),
  refreshMods: vi.fn(),
  selectMod: (...args: unknown[]) => selectModMock(...(args as [])),
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
vi.mock('element-plus', async (importOriginal) => ({
  ...(await importOriginal<typeof import('element-plus')>()),
  ElMessage: {
    error: vi.fn(),
    warning: vi.fn(),
    success: vi.fn(),
  },
}))

// mock 声明需先于被导入模块（vi.mock 提升保证执行顺序，变量声明提升由 vitest 处理）
const selectModMock = vi.fn()

import { useModsStore } from './mods'
import { ElMessage } from 'element-plus'
import type { ModData, ModGroupData, UpdateResult } from '@/types'

/** 构造最小更新结果 */
function fakeUpdateResult(selectedModIndex?: number): UpdateResult {
  return {
    totalGroups: 0,
    totalMods: 0,
    enabledMods: 0,
    disabledMods: 0,
    processedMods: 0,
    errors: [],
    needReloadManual: false,
    ...(selectedModIndex !== undefined ? { selectedModIndex } : {}),
  } as UpdateResult
}

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

/** 构造 None 空槽位 */
function makeNoneSlot(): ModData {
  return makeMod('None', 'None')
}

/**
 * 构造最小分组树：/mods/_MANAGED_/group_1（None 槽位 + ModA + ModB）
 */
function makeTestGroup(): { group: ModGroupData; modA: ModData; modB: ModData } {
  const none = makeNoneSlot()
  const modA = makeMod('/mods/_MANAGED_/group_1/ModA', 'ModA')
  const modB = makeMod('/mods/_MANAGED_/group_1/ModB', 'ModB')
  const group = {
    groupPath: '/mods/_MANAGED_/group_1',
    name: 'group_1',
    groupType: 'normalGroup',
    activeModIndex: -1,
    mods: [none, modA, modB],
    children: [],
  } as unknown as ModGroupData
  return { group, modA, modB }
}

/** 初始化 store 并装载测试分组（返回 store 与分组内关键对象） */
function setupStore() {
  const store = useModsStore()
  const { group, modA, modB } = makeTestGroup()
  store.groups = [group]
  store.selectedGroupPath = group.groupPath
  return { store, group, modA, modB }
}

describe('mods store 选择链路乐观更新（activateModByIndex UI 先行）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    selectModMock.mockReset()
    vi.mocked(ElMessage.error).mockClear()
    vi.mocked(ElMessage.success).mockClear()
    settingsState.currentModsPath = '/mods/GI'
    settingsState.currentGame = 'GI'
  })

  it('L1 selectMod 挂起时同步调用后选中态立即呈现（不等待后端）', async () => {
    const { store, group, modA, modB } = setupStore()
    let resolveSelect: (v: UpdateResult) => void = () => {}
    selectModMock.mockReturnValueOnce(
      new Promise<UpdateResult>((r) => (resolveSelect = r)),
    )

    // 不 await：调用后同步断言 UI 已先行呈现目标选中态
    const pending = store.activateModByIndex(0, modA.modPath)
    expect(store.selectedModIndex).toBe(1)
    expect(group.activeModIndex).toBe(1)
    expect(modA.isActive).toBe(true)
    expect(modB.isActive).toBe(false)
    expect(group.mods[0].isActive).toBe(false) // None 槽位失活

    // 后端完成后返回一致值：状态保持稳定
    resolveSelect(fakeUpdateResult(1))
    await pending
    expect(store.selectedModIndex).toBe(1)
    expect(group.activeModIndex).toBe(1)
    expect(modA.isActive).toBe(true)
  })

  it('扁平列表 isActive 同步：挂起时扁平条目立即更新（收藏/搜索视图高亮即时）', async () => {
    const { store, modA, modB } = setupStore()
    // 扁平列表使用与树独立的实例（镜像后端序列化）；预置 ModB 为当前激活
    const flatA = { ...modA, modPath: modA.modPath, isActive: false }
    const flatB = { ...modB, modPath: modB.modPath, isActive: true }
    store.mods = [flatA, flatB]
    const flatBefore = store.mods
    let resolveSelect: (v: UpdateResult) => void = () => {}
    selectModMock.mockReturnValueOnce(
      new Promise<UpdateResult>((r) => (resolveSelect = r)),
    )

    const pending = store.activateModByIndex(0, modA.modPath)
    // 乐观：扁平 ModA 激活、ModB 失活（树侧同步）
    expect(flatA.isActive).toBe(true)
    expect(flatB.isActive).toBe(false)
    expect(store.mods).not.toBe(flatBefore) // 换新引用驱动收集视图渲染

    resolveSelect(fakeUpdateResult(1))
    await pending
    expect(flatA.isActive).toBe(true)
  })

  it('S2 防抖拒绝（debounced）时静默完整回滚乐观变更', async () => {
    const { store, group, modA, modB } = setupStore()
    // 操作前快照态：ModB 为选中（索引 2）
    store.selectedModIndex = 2
    group.activeModIndex = 2
    modB.isActive = true

    selectModMock.mockRejectedValueOnce('debounced')
    await store.activateModByIndex(0, modA.modPath)

    // 回滚：恢复操作前快照（选中索引 / activeModIndex / 组内 isActive）
    expect(store.selectedModIndex).toBe(2)
    expect(group.activeModIndex).toBe(2)
    expect(modB.isActive).toBe(true)
    expect(modA.isActive).toBe(false)
    // 防抖静默：不弹错误提示
    expect(ElMessage.error).not.toHaveBeenCalled()
  })

  it('后端异常时回滚快照 + ElMessage.error 告知 + 异常冒泡', async () => {
    const { store, group, modA, modB } = setupStore()
    store.selectedModIndex = 2
    group.activeModIndex = 2
    modB.isActive = true

    selectModMock.mockRejectedValueOnce(new Error('disk full'))
    await expect(
      store.activateModByIndex(0, modA.modPath),
    ).rejects.toThrow('disk full')

    // 回滚完整
    expect(store.selectedModIndex).toBe(2)
    expect(group.activeModIndex).toBe(2)
    expect(modB.isActive).toBe(true)
    expect(modA.isActive).toBe(false)
    // 告知用户（fire-and-forget）
    expect(ElMessage.error).toHaveBeenCalledTimes(1)
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('disk full'))
  })

  it('后端返回索引与乐观值不一致时以后端权威值校正', async () => {
    const { store, group, modA } = setupStore()
    // 后端最终写入 selectedindex=3（与乐观值 1 不一致）
    selectModMock.mockResolvedValueOnce(fakeUpdateResult(3))

    await store.activateModByIndex(0, modA.modPath)

    expect(store.selectedModIndex).toBe(3)
    expect(group.activeModIndex).toBe(3)
  })

  it('成功后无条件重断言 UI：并发回退（索引一致时）也被后端确认恢复', async () => {
    const { store, group, modA } = setupStore()
    let resolveSelect: (v: UpdateResult) => void = () => {}
    selectModMock.mockReturnValueOnce(
      new Promise<UpdateResult>((r) => (resolveSelect = r)),
    )

    const pending = store.activateModByIndex(0, modA.modPath)
    expect(store.selectedModIndex).toBe(1)
    expect(modA.isActive).toBe(true)

    // 模拟乐观更新后、后端返回前被并发 refresh/增量 patch 回退（旧选中态残留）
    group.mods = [...group.mods]
    for (const m of group.mods) m.isActive = false
    modA.isActive = false
    group.activeModIndex = -1

    // 后端写入 selectedindex=1 成功返回（与乐观值一致 → 旧实现走空分支不恢复）
    resolveSelect(fakeUpdateResult(1))
    await pending

    expect(store.selectedModIndex).toBe(1)
    expect(group.activeModIndex).toBe(1)
    expect(modA.isActive).toBe(true)
  })

  it('成功通知：写入成功后发出（乐观 UI 更新之后）', async () => {
    const { store, modA } = setupStore()
    selectModMock.mockResolvedValueOnce(fakeUpdateResult(1))

    await store.activateModByIndex(0, modA.modPath)

    expect(vi.mocked(ElMessage.success)).toHaveBeenCalledTimes(1)
  })

  it('并发守卫：selectMod 在途时再次点击为 no-op', async () => {
    const { store, modA } = setupStore()
    let resolveSelect: (v: UpdateResult) => void = () => {}
    selectModMock.mockReturnValueOnce(
      new Promise<UpdateResult>((r) => (resolveSelect = r)),
    )

    const pending1 = store.activateModByIndex(0, modA.modPath)
    const pending2 = store.activateModByIndex(0, modA.modPath)
    await pending2

    // 第二次点击被 isActivating 守卫拦截：后端仅收到一次调用
    expect(selectModMock).toHaveBeenCalledTimes(1)

    resolveSelect(fakeUpdateResult(1))
    await pending1
    expect(store.selectedModIndex).toBe(1)
  })
})
