/**
 * mods store 启停本地 patch 测试（双字段同步 + 容器重排 + 选中位次重定位）。
 *
 * 覆盖场景（2026-09-06 启停异常回归）：
 * - toggleModLocal 双字段同步：disabled 与 modDisabled 均更新（消除 UI/派生口径分叉）
 * - 启用后组内排序立即更新：禁用模组沉底、启用模组前置（无需等重扫）
 * - 重排后 selectedModIndex 按 modPath 重定位（高亮不错位）
 * - 扁平列表同步重排
 * - batchToggleModLocal 批量同语义
 * - 回滚路径（新路径→旧路径）同样生效
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

const settingsState = { currentModsPath: '', currentGame: '' }

vi.mock('../utils/tauri', () => ({
  getMods: vi.fn(),
  refreshMods: vi.fn(),
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

import { useModsStore } from './mods'
import type { ModData, ModGroupData } from '@/types'

function makeMod(modPath: string, name: string, overrides: Partial<ModData> = {}): ModData {
  return {
    modPath,
    modName: name,
    name,
    isActive: false,
    isFavorite: false,
    modDisabled: false,
    disabled: false,
    modIndex: 0,
    groupIndex: 1,
    ...overrides,
  } as unknown as ModData
}

/**
 * 构造组序 None / ModA / ModB（全部启用）的分组。
 * 排序镜像口径：启用按名称自然序，A < B。
 * 注意：扁平与树内同路径条目为两个独立对象实例（镜像后端 ScanResult 独立序列化），
 * toggleModLocal 需分别 patch——测试必须用独立实例，共享实例会使树侧匹配失效。
 */
function setupStore() {
  const store = useModsStore()
  const none = makeMod('None', 'None')
  const modA = makeMod('/mods/_MANAGED_/group_1/ModA', 'ModA')
  const modB = makeMod('/mods/_MANAGED_/group_1/ModB', 'ModB')
  const group = {
    groupPath: '/mods/_MANAGED_/group_1',
    name: 'group_1',
    groupType: 'normalGroup',
    activeModIndex: 1,
    mods: [none, modA, modB],
    children: [],
  } as unknown as ModGroupData
  store.groups = [group]
  store.selectedGroupPath = group.groupPath
  store.selectedModIndex = 1 // 当前选中 ModA
  // 扁平列表：独立实例（与后端一致），None 不进扁平
  store.mods = [
    makeMod('/mods/_MANAGED_/group_1/ModA', 'ModA', { modIndex: 1 }),
    makeMod('/mods/_MANAGED_/group_1/ModB', 'ModB', { modIndex: 2 }),
  ]
  return { store, group, none, modA, modB }
}

describe('mods store toggleModLocal 双字段 + 重排 + 重定位', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    settingsState.currentModsPath = '/mods/GI'
    settingsState.currentGame = 'GI'
  })

  it('双字段同步更新（disabled 与 modDisabled）', () => {
    const { store, group } = setupStore()
    store.toggleModLocal('/mods/_MANAGED_/group_1/ModA', '/mods/_MANAGED_/group_1/DISABLED_ModA', true)
    const a = group.mods.find((m) => m.modName === 'ModA')
    expect(a?.modPath).toBe('/mods/_MANAGED_/group_1/DISABLED_ModA')
    expect(a?.modDisabled).toBe(true)
    expect(a?.disabled).toBe(true)
    // 反向：启用回来，双字段同步复位
    store.toggleModLocal('/mods/_MANAGED_/group_1/DISABLED_ModA', '/mods/_MANAGED_/group_1/ModA', false)
    const a2 = group.mods.find((m) => m.modName === 'ModA')
    expect(a2?.modDisabled).toBe(false)
    expect(a2?.disabled).toBe(false)
  })

  it('禁用后组内排序立即更新：ModA 沉底到禁用段（启用段保留 ModB）', () => {
    const { store, group } = setupStore()
    store.toggleModLocal('/mods/_MANAGED_/group_1/ModA', '/mods/_MANAGED_/group_1/DISABLED_ModA', true)
    const order = group.mods.map((m) => m.modName)
    expect(order[0]).toBe('None')
    expect(order).toEqual(['None', 'ModB', 'ModA']) // 启用段 ModB 在前、禁用段 ModA 沉底
  })

  it('启用后组内排序立即更新：原禁用模组前置回启用段', () => {
    const { store, group, modB } = setupStore()
    // 先禁用 ModB（排序后 ModB 沉底），再启用它 → 应回到启用段
    store.toggleModLocal('/mods/_MANAGED_/group_1/ModB', '/mods/_MANAGED_/group_1/DISABLED_ModB', true)
    expect(group.mods.map((m) => m.modName)).toEqual(['None', 'ModA', 'ModB'])
    store.toggleModLocal('/mods/_MANAGED_/group_1/DISABLED_ModB', '/mods/_MANAGED_/group_1/ModB', false)
    expect(group.mods.map((m) => m.modName)).toEqual(['None', 'ModA', 'ModB'])
    expect(modB.modDisabled).toBe(false)
  })

  it('重排后 selectedModIndex 按 modPath 重定位（高亮跟随原选中模组）', () => {
    const { store, group } = setupStore()
    // 当前选中 ModA（索引 1）。禁用 ModA 后 ModA 沉底 → 选中位次应重定位到沉底位置
    store.toggleModLocal('/mods/_MANAGED_/group_1/ModA', '/mods/_MANAGED_/group_1/DISABLED_ModA', true)
    const newIdx = group.mods.findIndex((m) => m.modPath === '/mods/_MANAGED_/group_1/DISABLED_ModA')
    expect(newIdx).toBe(2)
    expect(store.selectedModIndex).toBe(newIdx)
  })

  it('扁平列表同步重排（禁用后平铺序：启用在前、禁用在后）', () => {
    const { store } = setupStore()
    store.toggleModLocal('/mods/_MANAGED_/group_1/ModA', '/mods/_MANAGED_/group_1/DISABLED_ModA', true)
    const flat = store.mods.map((m) => m.modName)
    expect(flat).toEqual(['ModB', 'ModA'])
  })

  it('渲染断链回归：本地 patch 后 currentGroupMods 数组引用变化（v-for 依赖引用重渲染）', () => {
    const { store } = setupStore()
    const before = store.currentGroupMods
    store.toggleModLocal('/mods/_MANAGED_/group_1/ModA', '/mods/_MANAGED_/group_1/DISABLED_ModA', true)
    const after = store.currentGroupMods
    expect(after).not.toBe(before)
    expect(after.map((m) => m.modName)).toEqual(['None', 'ModB', 'ModA'])
    // 排序结果立即可读（不再依赖切页/重扫）
    expect(after[1]?.modName).toBe('ModB')
    expect(after[2]?.modName).toBe('ModA')
  })

  it('回滚路径（新路径→旧路径）对称生效', () => {
    const { store, group } = setupStore()
    // 乐观禁用后回滚：DISABLED_ModA → ModA（恢复启用）
    store.toggleModLocal('/mods/_MANAGED_/group_1/ModA', '/mods/_MANAGED_/group_1/DISABLED_ModA', true)
    store.toggleModLocal('/mods/_MANAGED_/group_1/DISABLED_ModA', '/mods/_MANAGED_/group_1/ModA', false)
    expect(group.mods.map((m) => m.modName)).toEqual(['None', 'ModA', 'ModB'])
    expect(store.selectedModIndex).toBe(1) // 回滚后选中位次恢复
  })
})

describe('mods store batchToggleModLocal（批量双字段 + 重排）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    settingsState.currentModsPath = '/mods/GI'
    settingsState.currentGame = 'GI'
  })

  it('批量禁用：双字段同步 + 沉底 + mod_index 重编号', () => {
    const { store, group, modA, modB } = setupStore()
    store.batchToggleModLocal([
      { oldPath: '/mods/_MANAGED_/group_1/ModA', newPath: '/mods/_MANAGED_/group_1/DISABLED_ModA', disabled: true },
      { oldPath: '/mods/_MANAGED_/group_1/ModB', newPath: '/mods/_MANAGED_/group_1/DISABLED_ModB', disabled: true },
    ])
    expect(modA.modDisabled).toBe(true)
    expect(modA.disabled).toBe(true)
    expect(modB.modDisabled).toBe(true)
    expect(modB.disabled).toBe(true)
    // 全部禁用 → 名称自然序（None 恒首）
    expect(group.mods.map((m) => m.modName)).toEqual(['None', 'ModA', 'ModB'])
    // mod_index 重编号：None=0，从 1 递增
    expect(group.mods.map((m) => m.modIndex)).toEqual([0, 1, 2])
  })
})