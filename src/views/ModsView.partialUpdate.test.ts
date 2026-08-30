/**
 * ModsView managed-partial-update 监听联动测试（R2，watcher 增量消费）。
 *
 * 覆盖场景：
 * - partial-update 携带 GroupDelta 列表 → 逐组 applyGroupDelta 原地 patch（树+扁平+零全量 IPC）
 * - 回声抑制链：folder-changed 先到（建 500ms 防抖 timer）→ partial-update 后到（patch 置位
 *   lastLocalPatchAt）→ timer 到期时复查时间戳 < 2000ms 跳过全量 refresh
 * - 时间戳过期后的 folder-changed（真实外部变更）→ 正常全量 refresh
 * - 树外分组（外部新建分组，无法原地 patch）→ 不 patch，folder-changed 防抖 timer 走全量兜底
 * - 空 deltas（如整组目录被删的失效批次）→ 不 patch，全量兜底
 * - 卸载时 unlisten 清理
 *
 * 实现方式：与 mods.applyGroupDelta.test.ts 相同的 mock 基建（vi.mock 拦截
 * '@tauri-apps/api/event' / '@/utils/tauri' / '@/stores/settings' 并 stub 子组件），
 * 挂载真实 ModsView 后直接触发捕获的事件 handler。
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { defineComponent, nextTick } from 'vue'

/** 跨 mock 工厂共享的可变状态（vi.hoisted 保证先于 mock 工厂执行） */
const mockState = vi.hoisted(() => ({
  /** 捕获的事件 handler（按事件名） */
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
  /** 被 mock 的 settings 状态（引用透传给 useSettingsStore） */
  settings: { currentModsPath: '', currentGame: '' },
  refreshMods: vi.fn(),
  getMods: vi.fn(),
}))

vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (k: string) => k }),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    mockState.listeners.set(name, handler)
    return () => {
      mockState.listeners.delete(name)
    }
  }),
}))

vi.mock('@/utils/tauri', () => ({
  importItems: vi.fn(),
  isFileWatcherRunning: vi.fn(async () => false),
  currentWatchedPath: vi.fn(async () => null),
  getMods: (...args: unknown[]) => mockState.getMods(...(args as [])),
  refreshMods: (...args: unknown[]) => mockState.refreshMods(...(args as [])),
  selectMod: vi.fn(),
  switchFileWatcher: vi.fn(),
  stopFileWatcher: vi.fn(),
  updateModData: vi.fn(),
  deselectGroupMod: vi.fn(),
  disableAllModsInGroup: vi.fn(),
  syncNrmmUpdateFlag: vi.fn(),
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: () => mockState.settings,
}))

vi.mock('@/components/mod/GroupPanel.vue', () => ({
  default: { name: 'GroupPanel', template: '<div />' },
}))
vi.mock('@/components/mod/ModGrid.vue', () => ({
  default: { name: 'ModGrid', template: '<div />' },
}))
vi.mock('@/components/common/UpdateModDataReminder.vue', () => ({
  default: { name: 'UpdateModDataReminder', template: '<div />' },
}))

import ModsView from './ModsView.vue'
import { useModsStore } from '@/stores/mods'
import type { ModData, ModGroupData, GroupDelta, ManagedPartialUpdatePayload } from '@/types'

/** el-icon 的极简替身（测试环境无 unplugin-vue-components 自动注册，
 *  模板内 el-icon 的 resolveComponent 会被编译器提升到 render 前奏，须显式提供） */
const ElIconStub = defineComponent({ name: 'ElIcon', template: '<i><slot /></i>' })

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

/** 触发已捕获的监听事件（模拟 Tauri 事件到达） */
function fire(name: string, payload: unknown): void {
  mockState.listeners.get(name)?.({ payload })
}

/** 挂载 ModsView 并等待 onMounted 内的 listen 注册完成 */
async function mountView() {
  const wrapper = mount(ModsView, {
    global: {
      components: { 'el-icon': ElIconStub },
    },
  })
  await flushPromises()
  return wrapper
}

/** 预置 store：A/B 两组 + 扁平列表 */
function seedStore() {
  const store = useModsStore()
  const a1 = makeMod('/g/A/modA1', 'modA1')
  const b1 = makeMod('/g/B/modB1', 'modB1')
  store.groups = [makeGroup('/g/A', [makeNoneSlot(), a1]), makeGroup('/g/B', [makeNoneSlot(), b1])]
  store.mods = [a1, b1]
  store.selectedGroupPath = '/g/A'
  return store
}

describe('ModsView managed-partial-update 增量消费（R2）', () => {
  beforeEach(() => {
    // 仅 fake 定时器与 Date（回声抑制的时间窗依赖 Date.now）
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] })
    setActivePinia(createPinia())
    mockState.listeners.clear()
    mockState.settings.currentModsPath = ''
    mockState.settings.currentGame = ''
    mockState.refreshMods.mockReset().mockResolvedValue({ groups: [], mods: [] })
    mockState.getMods.mockReset().mockResolvedValue({ groups: [], mods: [] })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('R2-1 partial-update 携带 deltas 逐组原地 patch：树节点替换 + 扁平更新 + 零全量 IPC + 置位 lastLocalPatchAt', async () => {
    const store = await (async () => {
      const s = seedStore()
      await mountView()
      return s
    })()

    // 外部变更：/g/A 的 modA1 被 modA2 替换（后端局部重扫后的整组新状态）
    const newGroupA = makeGroup('/g/A', [makeNoneSlot(), makeMod('/g/A/modA2', 'modA2')])
    const payload: ManagedPartialUpdatePayload = {
      game: 'GenshinImpact',
      deltas: [
        {
          oldGroupPath: '/g/A',
          newGroup: newGroupA,
          oldModPaths: ['/g/A/modA1'],
        } satisfies GroupDelta,
      ],
    }
    fire('managed-partial-update', payload)
    await nextTick()

    // 树：A 组原位替换为新节点，B 组不受影响
    expect(store.groups[0].mods.map((m) => m.modPath)).toEqual(['None', '/g/A/modA2'])
    expect(store.groups[1].mods.map((m) => m.modPath)).toEqual(['None', '/g/B/modB1'])
    // 扁平：旧条目剔除、新条目原位插入
    expect(store.mods.map((m) => m.modPath)).toEqual(['/g/A/modA2', '/g/B/modB1'])
    // 选中组仍存在 → 不回退（R1 已处理，回归确认）
    expect(store.selectedGroupPath).toBe('/g/A')
    // 纯本地 patch：无全量 IPC + 回声抑制时间戳置位
    expect(mockState.refreshMods).not.toHaveBeenCalled()
    expect(store.lastLocalPatchAt).toBeGreaterThan(0)
  })

  it('R2-2 回声抑制链：folder-changed 先到建 timer → partial-update patch → timer 到期跳过全量 refresh；时间戳过期后恢复', async () => {
    const store = await (async () => {
      const s = seedStore()
      await mountView()
      return s
    })()
    // refresh 需要游戏路径（对照路径走真实 refresh 断言用）
    mockState.settings.currentModsPath = '/mods/GI'
    mockState.settings.currentGame = 'GenshinImpact'

    // 后端双事件顺序：folder-changed 先（防抖 timer 先建）
    fire('managed-folder-changed', 'D:/mods/_MANAGED_')
    // partial-update 后到（patch 置位 lastLocalPatchAt）
    const newGroupA = makeGroup('/g/A', [makeNoneSlot(), makeMod('/g/A/modA2', 'modA2')])
    fire('managed-partial-update', {
      game: 'GenshinImpact',
      deltas: [{ oldGroupPath: '/g/A', newGroup: newGroupA, oldModPaths: ['/g/A/modA1'] }],
    })
    await nextTick()
    expect(store.mods.map((m) => m.modPath)).toEqual(['/g/A/modA2', '/g/B/modB1'])

    // timer 到期（+500ms）：lastLocalPatchAt < 2000ms → 抑制全量 refresh
    vi.advanceTimersByTime(500)
    expect(mockState.refreshMods).not.toHaveBeenCalled()

    // 对照：时间戳过期（>2000ms）后的 folder-changed（真实外部变更）→ 正常全量 refresh
    vi.advanceTimersByTime(2500)
    fire('managed-folder-changed', 'D:/mods/_MANAGED_')
    vi.advanceTimersByTime(500)
    expect(mockState.refreshMods).toHaveBeenCalledTimes(1)
    await flushPromises()
  })

  it('R2-3 树外分组（外部新建分组）无法原地 patch → 不 patch，folder-changed 防抖 timer 走全量兜底', async () => {
    const store = await (async () => {
      const s = seedStore()
      await mountView()
      return s
    })()
    mockState.settings.currentModsPath = '/mods/GI'
    mockState.settings.currentGame = 'GenshinImpact'

    fire('managed-folder-changed', 'D:/mods/_MANAGED_')
    fire('managed-partial-update', {
      game: 'GenshinImpact',
      deltas: [
        {
          oldGroupPath: '/g/NEW',
          newGroup: makeGroup('/g/NEW', [makeNoneSlot(), makeMod('/g/NEW/modN1', 'modN1')]),
          oldModPaths: [],
        },
      ],
    })
    await nextTick()

    // 未 patch：树/扁平/时间戳均不变（保留 folder-changed 已建 timer 的兜底语义）
    expect(store.groups.map((g) => g.groupPath)).toEqual(['/g/A', '/g/B'])
    expect(store.mods.map((m) => m.modPath)).toEqual(['/g/A/modA1', '/g/B/modB1'])
    expect(store.lastLocalPatchAt).toBe(0)

    vi.advanceTimersByTime(500)
    expect(mockState.refreshMods).toHaveBeenCalledTimes(1)
    await flushPromises()
  })

  it('R2-4 空 deltas（如整组目录被删的失效批次）→ 不 patch，全量兜底', async () => {
    const store = await (async () => {
      const s = seedStore()
      await mountView()
      return s
    })()
    mockState.settings.currentModsPath = '/mods/GI'
    mockState.settings.currentGame = 'GenshinImpact'

    fire('managed-folder-changed', 'D:/mods/_MANAGED_')
    fire('managed-partial-update', { game: 'GenshinImpact', deltas: [] })
    await nextTick()

    expect(store.lastLocalPatchAt).toBe(0)
    vi.advanceTimersByTime(500)
    expect(mockState.refreshMods).toHaveBeenCalledTimes(1)
    await flushPromises()
  })

  it('R2-5 卸载时清理 managed-partial-update 监听（防内存泄漏）', async () => {
    await seedStore()
    const wrapper = await mountView()
    expect(mockState.listeners.has('managed-partial-update')).toBe(true)

    wrapper.unmount()
    expect(mockState.listeners.has('managed-partial-update')).toBe(false)
    expect(mockState.listeners.has('managed-folder-changed')).toBe(false)
  })
})
