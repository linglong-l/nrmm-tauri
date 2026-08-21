/**
 * mods store 加载竞态防护测试（P1，2026-08-19 计划）。
 *
 * 覆盖场景（epoch 机制）：
 * - C1 并发 loadMods：慢的旧请求后完成不得覆盖新请求结果
 * - C2 clearData 使 in-flight 失效：切游戏后旧游戏响应不得落地
 * - C3 过期请求不复位 loading：新请求进行中 loading 不被旧请求 finally 提前清掉
 * - C4 refresh 与 loadMods 交错：后发起的 loadMods 结果生效
 *
 * 实现方式：顶层 vi.mock 拦截 `../utils/tauri`（getMods/refreshMods 为可控延迟 stub）
 * 与 `./settings`（由可变 settingsState 驱动），断言最终 store 状态与调用顺序无关。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

/** 可变 settings 状态（各用例赋值，被 mock 的 useSettingsStore 读取） */
const settingsState = { currentModsPath: '', currentGame: '' }

vi.mock('../utils/tauri', () => ({
  getMods: (...args: unknown[]) => getModsMock(...(args as [])),
  refreshMods: (...args: unknown[]) => refreshModsMock(...(args as [])),
  selectMod: vi.fn(),
  switchFileWatcher: vi.fn(),
  stopFileWatcher: vi.fn(),
  updateModData: vi.fn(),
  deselectGroupMod: vi.fn(),
  disableAllModsInGroup: vi.fn(),
}))
vi.mock('./settings', () => ({
  useSettingsStore: () => settingsState,
}))

// mock 声明需先于被导入模块（vi.mock 提升保证执行顺序，变量声明提升由 vitest 处理）
const getModsMock = vi.fn()
const refreshModsMock = vi.fn()

import { useModsStore } from './mods'
import type { ModGroupData } from '@/types'

/** 构造最小分组数据（不同 tag 标记便于断言来源） */
function fakeGroups(tag: string, count: number): ModGroupData[] {
  return Array.from({ length: count }, (_, i) => ({
    groupPath: `/${tag}/group_${i + 1}`,
    name: `${tag}-${i + 1}`,
    groupType: 'normalGroup',
    activeModIndex: -1,
    mods: [],
    children: [],
  })) as unknown as ModGroupData[]
}

describe('mods store 加载竞态防护（epoch）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    getModsMock.mockReset()
    refreshModsMock.mockReset()
    settingsState.currentModsPath = '/mods/GI'
    settingsState.currentGame = 'GI'
  })

  it('C1 旧请求后完成不得覆盖新请求结果', async () => {
    const store = useModsStore()

    // 请求1（旧）延迟 30ms 返回 1 组；请求2（新）立即返回 2 组
    getModsMock
      .mockImplementationOnce(
        () => new Promise((r) => setTimeout(() => r({ groups: fakeGroups('old', 1), mods: [] }), 30)),
      )
      .mockImplementationOnce(
        () => new Promise((r) => setTimeout(() => r({ groups: fakeGroups('new', 2), mods: [] }), 5)),
      )

    const p1 = store.loadMods()
    const p2 = store.loadMods()
    await Promise.all([p1, p2])

    // 旧请求（old-1）后完成但 epoch 已过期 → 不得落地；最终为新请求结果
    expect(store.groups.map((g) => g.name)).toEqual(['new-1', 'new-2'])
  })

  it('C2 clearData 使 in-flight 失效：旧游戏响应不得落地', async () => {
    const store = useModsStore()

    // 旧请求延迟完成
    getModsMock.mockImplementationOnce(
      () => new Promise((r) => setTimeout(() => r({ groups: fakeGroups('old', 1), mods: [] }), 20)),
    )
    const inflight = store.loadMods()

    // 切游戏：clearData 使 in-flight 失效
    store.clearData()
    await inflight

    expect(store.groups).toEqual([]) // 旧数据未落地
  })

  it('C3 过期请求不得提前复位 loading（新请求进行中）', async () => {
    const store = useModsStore()

    // 请求1（旧）先完成（10ms）；请求2（新）慢（40ms）
    getModsMock
      .mockImplementationOnce(
        () => new Promise((r) => setTimeout(() => r({ groups: fakeGroups('fast-old', 1), mods: [] }), 10)),
      )
      .mockImplementationOnce(
        () => new Promise((r) => setTimeout(() => r({ groups: fakeGroups('slow-new', 2), mods: [] }), 40)),
      )

    const p1 = store.loadMods()
    const p2 = store.loadMods()

    // 旧请求完成后、新请求完成前：loading 必须仍为 true
    await new Promise((r) => setTimeout(r, 25))
    expect(store.loading).toBe(true)

    await Promise.all([p1, p2])
    expect(store.loading).toBe(false)
    expect(store.groups.map((g) => g.name)).toEqual(['slow-new-1', 'slow-new-2'])
  })

  it('C4 refresh 与 loadMods 交错：后发起的 loadMods 结果生效', async () => {
    const store = useModsStore()

    refreshModsMock.mockImplementationOnce(
      () => new Promise((r) => setTimeout(() => r({ groups: fakeGroups('refresh', 3), mods: [] }), 30)),
    )
    getModsMock.mockImplementationOnce(
      () => new Promise((r) => setTimeout(() => r({ groups: fakeGroups('load', 1), mods: [] }), 5)),
    )

    const pr = store.refresh()
    const pl = store.loadMods()
    await Promise.all([pr, pl])

    // refresh（旧）后完成 → 过期作废；loadMods（新）结果生效
    expect(store.groups.map((g) => g.name)).toEqual(['load-1'])
  })
})
