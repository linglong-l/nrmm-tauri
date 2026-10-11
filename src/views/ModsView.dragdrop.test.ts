/**
 * ModsView 拖拽导入测试（Tauri 原生拖放通道）
 *
 * 背景：Tauri v2 默认 dragDropEnabled=true，wry 会 SetAllowExternalDrop(false) 把系统文件
 * 拖放拦在 DOM 之外，HTML5 的 dragover/drop 在应用内一个都不触发；且 dataTransfer 里的
 * file.path 属 Electron 系专有，WebView2 下拿不到绝对路径。因此拖拽导入只能由
 * getCurrentWebview().onDragDropEvent 完成 —— 本组用例即钉住这条链路（含命中测试与守卫）。
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { mount, flushPromises, DOMWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { defineComponent, nextTick } from 'vue'
import type { ModGroupData } from '@/types'

/** 跨 mock 工厂共享的可变状态（vi.hoisted 保证先于 mock 工厂执行） */
const mockState = vi.hoisted(() => ({
  /** 原生拖放 handler（onDragDropEvent 注册的那一个） */
  dragDropHandlers: [] as Array<(event: { payload: unknown }) => void>,
  /** 原生拖放监听的注销标记 */
  dragDropUnlistened: false,
  settings: { currentModsPath: '', currentGame: '' },
  importItems: vi.fn(),
  refreshMods: vi.fn(),
  getMods: vi.fn(),
}))

vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (k: string) => k }),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
}))

vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: async (handler: (event: { payload: unknown }) => void) => {
      mockState.dragDropHandlers.push(handler)
      return () => {
        mockState.dragDropUnlistened = true
      }
    },
  }),
}))

vi.mock('@/utils/tauri', () => ({
  importItems: (...args: unknown[]) => mockState.importItems(...(args as [])),
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

/** el-icon 的极简替身（测试环境无自动导入插件，模板里的 el-icon 须显式提供） */
const ElIconStub = defineComponent({ name: 'ElIcon', template: '<i><slot /></i>' })

/** ElMessage 替身：可调用（返回带 close 的实例）+ warning/error/success 三个静态方法 */
type ElMessageMock = {
  (...args: unknown[]): { close: () => void }
  warning: ReturnType<typeof vi.fn>
  error: ReturnType<typeof vi.fn>
  success: ReturnType<typeof vi.fn>
}
let elMessage: ElMessageMock

/** 构造最小分组（拖放守卫只看 groupPath） */
function makeGroup(groupPath: string): ModGroupData {
  return { groupPath, mods: [], children: [] } as unknown as ModGroupData
}

/**
 * 触发原生拖放事件
 * @param payload wry 侧透传的 DragDropEvent payload（enter/over/leave/drop）
 */
function fireDragDrop(payload: unknown): void {
  mockState.dragDropHandlers.forEach((h) => h({ payload }))
}

/** 挂载 ModsView，并把根元素矩形固定为 (0,0)-(400,300) 逻辑像素 */
async function mountView() {
  const wrapper = mount(ModsView, { global: { components: { 'el-icon': ElIconStub } } })
  await flushPromises()
  ;(wrapper.element as HTMLElement).getBoundingClientRect = () =>
    ({ left: 0, top: 0, right: 400, bottom: 300, width: 400, height: 300, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect
  return wrapper
}

/** 预置一次可导入的最小上下文：已选游戏路径 + 已选分组 */
function seedImportContext() {
  const store = useModsStore()
  store.groups = [makeGroup('/mods/GI/_MANAGED_/A')]
  store.selectedGroupPath = '/mods/GI/_MANAGED_/A'
  mockState.settings.currentModsPath = '/mods/GI'
  mockState.settings.currentGame = 'GenshinImpact'
  return store
}

describe('ModsView 拖拽导入（原生拖放通道）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    mockState.dragDropHandlers.length = 0
    mockState.dragDropUnlistened = false
    mockState.settings.currentModsPath = ''
    mockState.settings.currentGame = ''
    mockState.importItems.mockReset().mockResolvedValue([])
    mockState.refreshMods.mockReset().mockResolvedValue({ groups: [], mods: [] })
    mockState.getMods.mockReset().mockResolvedValue({ groups: [], mods: [] })

    const close = vi.fn()
    elMessage = Object.assign(vi.fn(() => ({ close })), {
      warning: vi.fn(),
      error: vi.fn(),
      success: vi.fn(),
    }) as ElMessageMock
    vi.stubGlobal('ElMessage', elMessage)
    vi.stubGlobal('devicePixelRatio', 1)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('D-1 原生 drop 把绝对路径交给 import_item_cmd，并带上当前分组目录', async () => {
    const wrapper = await mountView()
    seedImportContext()

    fireDragDrop({ type: 'drop', position: { x: 100, y: 100 }, paths: ['D:/dl/mod.zip', 'D:/dl/modDir'] })
    await flushPromises()

    expect(mockState.importItems).toHaveBeenCalledTimes(1)
    expect(mockState.importItems).toHaveBeenCalledWith({
      items: ['D:/dl/mod.zip', 'D:/dl/modDir'],
      targetGroupDir: '/mods/GI/_MANAGED_/A',
    })
    expect(elMessage.success).toHaveBeenCalled()
    wrapper.unmount()
  })

  it('D-2 enter/over 弹覆盖层，leave 与 drop 收起', async () => {
    const wrapper = await mountView()
    const overlay = (): DOMWrapper<Element> => wrapper.find('.drag-overlay')
    expect(overlay().exists()).toBe(false)

    fireDragDrop({ type: 'enter', position: { x: 10, y: 10 } })
    await nextTick()
    expect(overlay().exists()).toBe(true)

    fireDragDrop({ type: 'leave' })
    await nextTick()
    expect(overlay().exists()).toBe(false)

    fireDragDrop({ type: 'over', position: { x: 10, y: 10 } })
    await nextTick()
    expect(overlay().exists()).toBe(true)

    fireDragDrop({ type: 'drop', position: { x: 10, y: 10 }, paths: [] })
    await nextTick()
    expect(overlay().exists()).toBe(false)
    wrapper.unmount()
  })

  it('D-3 落点命中测试按 devicePixelRatio 换算：区内导入，区外不导入', async () => {
    const wrapper = await mountView()
    seedImportContext()
    vi.stubGlobal('devicePixelRatio', 2)

    // 矩形为 (0,0)-(400,300) 逻辑像素：物理 (700,500) → 逻辑 (350,250) 命中
    fireDragDrop({ type: 'drop', position: { x: 700, y: 500 }, paths: ['D:/dl/inside.zip'] })
    await flushPromises()
    expect(mockState.importItems).toHaveBeenCalledTimes(1)

    // 物理 (900,100) → 逻辑 (450,50) 越界：不得导入
    mockState.importItems.mockClear()
    fireDragDrop({ type: 'drop', position: { x: 900, y: 100 }, paths: ['D:/dl/outside.zip'] })
    await flushPromises()
    expect(mockState.importItems).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('D-4 守卫：未选分组 / 更新模组数据期间均不导入且给出可见提示', async () => {
    const wrapper = await mountView()
    const store = seedImportContext()

    // (a) 未选中分组
    store.selectedGroupPath = ''
    store.groups = []
    fireDragDrop({ type: 'drop', position: { x: 10, y: 10 }, paths: ['D:/dl/mod.zip'] })
    await flushPromises()
    expect(mockState.importItems).not.toHaveBeenCalled()
    expect(elMessage.warning).toHaveBeenCalledWith('Please select a group first')

    // (b) 更新模组数据期间（此提示走 i18n key，不再用裸中文当 key）
    store.groups = [makeGroup('/mods/GI/_MANAGED_/A')]
    store.selectedGroupPath = '/mods/GI/_MANAGED_/A'
    store.isUpdatingModData = true
    fireDragDrop({ type: 'drop', position: { x: 10, y: 10 }, paths: ['D:/dl/mod.zip'] })
    await flushPromises()
    expect(mockState.importItems).not.toHaveBeenCalled()
    expect(elMessage.warning).toHaveBeenCalledWith('Updating mod data, please wait')
    wrapper.unmount()
  })

  it('D-5 卸载时注销原生拖放监听', async () => {
    const wrapper = await mountView()
    expect(mockState.dragDropHandlers.length).toBe(1)

    wrapper.unmount()
    expect(mockState.dragDropUnlistened).toBe(true)

    // 注销后残留 handler 不再被注册方持有，重复卸载不应抛错
    mockState.dragDropHandlers.length = 0
    wrapper.unmount()
  })

  it('D-6 HTML5 兜底通道拿不到绝对路径时明确报错，而不是静默无反应', async () => {
    const wrapper = await mountView()
    seedImportContext()

    // 浏览器预览场景：File 无 path 属性（Electron 系专有）
    const file = new File(['x'], 'mod.zip')
    await wrapper.find('.mods-view').trigger('drop', { dataTransfer: { files: [file] } })
    await flushPromises()

    expect(mockState.importItems).not.toHaveBeenCalled()
    expect(elMessage.warning).toHaveBeenCalledWith('No valid paths found')
    wrapper.unmount()
  })
})
