/**
 * ModGrid 专项测试（A-1 / A-2 / B-2）
 *
 * 覆盖场景：
 * - A-1 clampScrollTop 纯函数（超界收敛 / 未超界不变 / 无溢出归 0）
 * - A-1 watch 联动：displayMods 变化且 loading=false → virtualRecalculate 被调用
 * - A-2 rowColCount 纯函数（首行满列 / 末行余数 / 空行为 0）
 * - A-2 虚拟行剪裁：非整行模组数只渲染真实卡片，不产出多余空卡
 * - B-2 骨架分支：loading 空数据 → 渲染骨架网格、无空态文案
 * - B-2 遮罩分支：loading 且有旧数据 → 半透明遮罩存在
 * - B-2 空态分支：非 loading 空数据 → 空态（回归）
 *
 * 实现方式：Mock 子组件/组合式（useDragScroll/useVirtualGrid/useModsStore/ModCard/
 * SearchBar/vue-i18n/@element-plus），挂载真实 ModGrid 后驱动 store 状态断言渲染。
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { reactive, defineComponent } from 'vue'

/** useVirtualGrid 的受控返回值（vi.hoisted 保证先于 mock 工厂执行） */
const virtualControls = vi.hoisted(() => ({
  enabled: { value: true },
  startIndex: { value: 0 },
  endIndex: { value: 0 },
  spacerTop: { value: 0 },
  spacerBottom: { value: 0 },
  recalculate: vi.fn(),
}))

/** jsdom 无真实布局，clientWidth 恒为 0 会把 columnCount 压到 1；固定为 900 使列数为 5，便于验证行内剪裁 */
const originalClientWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth')

/** 共享 store（beforeEach 中重建 reactive 实例并赋值） */
let store: Record<string, unknown>

vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (k: string): string => k }),
}))

vi.mock('@element-plus/icons-vue', () => ({
  FolderOpened: defineComponent({ name: 'FolderOpened', template: '<i />' }),
}))

vi.mock('@/components/mod/ModCard.vue', () => ({
  default: defineComponent({
    name: 'ModCard',
    props: ['mod', 'isEmptySlot', 'modIndex'],
    emits: ['activate', 'select'],
    // data-idx 便于测试定位指定索引卡片；click 触发激活事件（R4 用例）
    template: '<div class="mod-card-stub" :data-idx="modIndex" @click="$emit(\'activate\', modIndex)" /><span class="selectable" :data-idx="modIndex" @click="$emit(\'select\', modIndex)" />',
  }),
}))

vi.mock('@/components/common/SearchBar.vue', () => ({
  default: { name: 'SearchBar', template: '<div />' },
}))

vi.mock('@/composables/useDragScroll', () => ({
  useDragScroll: vi.fn(),
}))

vi.mock('@/composables/useVirtualGrid', () => ({
  useVirtualGrid: () => ({
    startIndex: virtualControls.startIndex,
    endIndex: virtualControls.endIndex,
    spacerTop: virtualControls.spacerTop,
    spacerBottom: virtualControls.spacerBottom,
    enabled: virtualControls.enabled,
    recalculate: virtualControls.recalculate,
  }),
}))

vi.mock('@/stores/mods', () => ({
  useModsStore: () => store,
}))

/** el-icon / el-button 替身（测试环境无自动注册） */
const ElIconStub = defineComponent({ name: 'ElIcon', template: '<span><slot /></span>' })
const ElButtonStub = defineComponent({ name: 'ElButton', template: '<button><slot /></button>' })

function createStore() {
  return reactive({
    loading: false,
    loadError: null as string | null,
    selectedGroupPath: '',
    showFavoritesOnly: false,
    mods: [] as unknown[],
    currentGroupMods: [] as unknown[],
    selectedModIndex: -1,
    searchVisible: false,
    searchQuery: '',
    groupMatchPaths: [] as string[],
    modMatchIndices: [] as number[],
    currentGlobalMatchIndex: -1,
    loadMods: vi.fn(),
    highlightMod: vi.fn(),
    activateModByIndex: vi.fn(),
    setSearchVisible: vi.fn(),
    clearSearch: vi.fn(),
    nextSearchMatch: vi.fn(),
    prevSearchMatch: vi.fn(),
    getCurrentMatchInfo: () => ({ isGroup: false }),
    selectGroupByPath: vi.fn(),
    selectGroupContainingMod: vi.fn(),
  })
}

/** 构造最小模组数据（仅需 modPath 即可渲染/计数） */
function makeMod(path: string): unknown {
  return { modPath: path, modName: path.split('/').pop() }
}

async function mountGrid() {
  const wrapper = mount((await import('./ModGrid.vue')).default, {
    global: {
      components: {
        'el-icon': ElIconStub,
        'el-button': ElButtonStub,
      },
    },
  })
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  vi.clearAllMocks()
  virtualControls.enabled.value = true
  virtualControls.startIndex.value = 0
  virtualControls.endIndex.value = 0
  // 固定容器宽度，使 updateColumnCount 得到 5 列（(900+12)/(160+12)=5.3 → 5）
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
    configurable: true,
    get: () => 900,
  })
  // jsdom 无 ResizeObserver，ModGrid onMounted 需要
  globalThis.ResizeObserver = class {
    observe() {}
    disconnect() {}
    unobserve() {}
  } as unknown as typeof ResizeObserver
  store = createStore()
})

afterEach(() => {
  if (originalClientWidth) {
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', originalClientWidth)
  } else {
    delete (HTMLElement.prototype as { clientWidth?: number }).clientWidth
  }
  delete (globalThis as { ResizeObserver?: unknown }).ResizeObserver
})

import { rowColCount, clampScrollTop } from './ModGrid.vue'

describe('A-2 rowColCount 行内列数剪裁（纯函数）', () => {
  it('首行满列', () => {
    expect(rowColCount(7, 5, 0)).toBe(5)
  })
  it('末行取余数', () => {
    expect(rowColCount(7, 5, 1)).toBe(2)
  })
  it('空行（start 越界）返回 0', () => {
    expect(rowColCount(5, 5, 1)).toBe(0)
  })
  it('总数为 0 返回 0', () => {
    expect(rowColCount(0, 5, 0)).toBe(0)
  })
  it('cols 非法（<=0）返回 0', () => {
    expect(rowColCount(7, 0, 0)).toBe(0)
  })
})

describe('A-1 clampScrollTop 滚动收敛（纯函数）', () => {
  it('scrollTop 超界 => 收敛到最大可滚位置', () => {
    const el = { scrollHeight: 100, clientHeight: 50, scrollTop: 80 }
    clampScrollTop(el)
    expect(el.scrollTop).toBe(50)
  })
  it('scrollTop 未超界 => 保持不变', () => {
    const el = { scrollHeight: 100, clientHeight: 50, scrollTop: 30 }
    clampScrollTop(el)
    expect(el.scrollTop).toBe(30)
  })
  it('无溢出（无滚动条）=> 归 0', () => {
    const el = { scrollHeight: 50, clientHeight: 50, scrollTop: 10 }
    clampScrollTop(el)
    expect(el.scrollTop).toBe(0)
  })
})

describe('A-1 watch 联动重算', () => {
  it('displayMods 长度变化且 loading=false => 触发 virtualRecalculate', async () => {
    store.currentGroupMods = [makeMod('/g/A/m1'), makeMod('/g/A/m2'), makeMod('/g/A/m3')]
    await mountGrid()
    virtualControls.recalculate.mockClear()

    // 命令级 patch 缩短列表（移除一个模组）→ 长度变化
    store.currentGroupMods = [makeMod('/g/A/m1'), makeMod('/g/A/m2')]
    await flushPromises()

    expect(virtualControls.recalculate).toHaveBeenCalled()
  })

  it('loading=true 时列表变化 => 不重算（沿用既有条件）', async () => {
    store.loading = true
    store.currentGroupMods = [makeMod('/g/A/m1'), makeMod('/g/A/m2')]
    await mountGrid()
    virtualControls.recalculate.mockClear()

    store.currentGroupMods = [makeMod('/g/A/m1')]
    await flushPromises()

    expect(virtualControls.recalculate).not.toHaveBeenCalled()
  })
})

describe('A-2 虚拟行剪裁：不渲染多余空卡', () => {
  it('非整行模组数（7 个，5 列）× 虚拟模式 => 恰好渲染 7 张真实卡，无多余空卡', async () => {
    const mods = Array.from({ length: 7 }, (_, i) => makeMod(`/g/A/m${i + 1}`))
    store.currentGroupMods = mods
    virtualControls.startIndex.value = 0
    virtualControls.endIndex.value = 7
    const wrapper = await mountGrid()

    expect(wrapper.findAll('.mod-card-stub')).toHaveLength(7)
  })
})

describe('B-2 骨架 / 遮罩 / 空态 分支', () => {
  it('loading 且空数据 => 渲染骨架网格，无空态文案', async () => {
    store.loading = true
    store.currentGroupMods = []
    const wrapper = await mountGrid()

    expect(wrapper.find('.skeleton-grid').exists()).toBe(true)
    // 3 行 × 默认 5 列
    expect(wrapper.findAll('.skeleton-card')).toHaveLength(15)
    expect(wrapper.find('.empty-state').exists()).toBe(false)
  })

  it('loading 且有旧数据 => 半透明刷新遮罩存在，无骨架', async () => {
    store.loading = true
    store.currentGroupMods = [makeMod('/g/A/m1'), makeMod('/g/A/m2'), makeMod('/g/A/m3')]
    virtualControls.startIndex.value = 0
    virtualControls.endIndex.value = 3
    const wrapper = await mountGrid()

    expect(wrapper.find('.is-refreshing-mask').exists()).toBe(true)
    expect(wrapper.find('.skeleton-grid').exists()).toBe(false)
  })

  it('非 loading 且空数据 => 空态（回归）', async () => {
    store.loading = false
    store.currentGroupMods = []
    const wrapper = await mountGrid()

    expect(wrapper.find('.empty-state').exists()).toBe(true)
    expect(wrapper.find('.skeleton-grid').exists()).toBe(false)
  })
})

describe('A 组交叉影响补充（6.2：R1/R2/R4/R8）', () => {
  /** 9 卡 / 5 列：行0 = idx 0-4，行1（末行余数）= idx 5-8（4 张） */
  function nineMods() {
    return Array.from({ length: 9 }, (_, i) => makeMod(`/g/A/m${i + 1}`))
  }

  it('R1 虚拟末行对齐：真实卡后补 CSS 占位（与全量模式空槽布局一致）', async () => {
    store.currentGroupMods = nineMods()
    virtualControls.startIndex.value = 0
    virtualControls.endIndex.value = 10 // 覆盖 2 行
    const wrapper = await mountGrid()

    // 9 张真实卡（不渲染 undefined 卡）+ 末行补位 5-4 = 1 个
    expect(wrapper.findAll('.mod-card-stub')).toHaveLength(9)
    expect(wrapper.findAll('.empty-slot-placeholder')).toHaveLength(1)
  })

  it('R2 末行命中前置：余数行真实卡全部渲染且索引连续（0..8，无 undefined 卡）', async () => {
    store.currentGroupMods = nineMods()
    virtualControls.startIndex.value = 0
    virtualControls.endIndex.value = 10
    const wrapper = await mountGrid()

    // 最后一行（行1）应恰好渲染 rowColCount(9,5,1)=4 张真实卡
    const rows = wrapper.findAll('.mod-grid-row.virtual-row')
    expect(rows).toHaveLength(2)
    expect(rows[1].findAll('.mod-card-stub')).toHaveLength(4)
    // 全部真实卡的 data-idx 覆盖 0..8（索引连续、无缺失/越界）
    const idxs = wrapper
      .findAll('.mod-card-stub')
      .map((c) => Number(c.attributes('data-idx')))
      .sort((a, b) => a - b)
    expect(idxs).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8])
  })

  it('R4 末行激活索引正确：点击行1 第 4 张（display idx 8）=> activateModByIndex(8, 对应 modPath)', async () => {
    store.currentGroupMods = nineMods()
    virtualControls.startIndex.value = 0
    virtualControls.endIndex.value = 10
    const wrapper = await mountGrid()

    await wrapper.find('[data-idx="8"]').trigger('click')
    await flushPromises()

    expect(store.activateModByIndex).toHaveBeenCalledWith(8, '/g/A/m9')
  })

  it('R8 重算有界：连续 5 次列表长度变化 => virtualRecalculate 恰触发 5 次（无放大）', async () => {
    store.currentGroupMods = []
    await mountGrid()
    virtualControls.recalculate.mockClear()

    for (let n = 1; n <= 5; n++) {
      store.currentGroupMods = Array.from({ length: n }, (_, i) => makeMod(`/g/A/m${i + 1}`))
      await flushPromises()
    }

    expect(virtualControls.recalculate).toHaveBeenCalledTimes(5)
  })
})