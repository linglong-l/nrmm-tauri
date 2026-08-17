/**
 * 边界/回归组合测试（纯逻辑层，不触达 Tauri 后端）。
 *
 * 覆盖计划用例：
 * - F1/F2  escapeHtml：特殊字符 + 全角（NFKC 边界，全角字符不误转）
 * - F3/F4/F5 useVirtualGrid：超阈值启用虚拟行 / 低阈值全量 / 空列表不越界
 * - F6      useLoadingDots：定时器循环与停止
 * - F7/F8/F9 mods 文本规范化纯函数：NFKC 跨语言归一化 / 模糊搜索 / 超大输入
 *
 * 注：mods 的 normalizeText / fuzzyMatchWithSpansSimple 为 store 返回的纯函数，
 * 实例化 store（不调用 invoke）后直接调用，无真实后端副作用。
 */
import { describe, it, expect, beforeAll, afterAll, vi } from 'vitest'
import { defineComponent, computed, ref, h, type Ref } from 'vue'
import { mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { escapeHtml } from '@/utils/html'
import { useVirtualGrid, type VirtualGridReturn } from './useVirtualGrid'
import { useLoadingDots } from './useLoadingDots'
import { useModsStore } from '@/stores/mods'

// ---------- F1 / F2: escapeHtml ----------
describe('escapeHtml 边界', () => {
  it('F1 混合特殊字符 + 中文全部转义/保留', () => {
    expect(escapeHtml(`a&b<c>d"e'f 中文`)).toBe('a&amp;b&lt;c&gt;d&quot;e&#39;f 中文')
  })
  it('F2 全角字符（如全角小于号）不误转义', () => {
    // 全角 ＜＞ 不是 ASCII < >，不应转义为实体
    expect(escapeHtml('＜全角＞')).toBe('＜全角＞')
    expect(escapeHtml('a <b>')).toBe('a &lt;b&gt;')
  })
})

// ---------- F3 / F4 / F5: useVirtualGrid ----------
const VirtualHost = defineComponent({
  props: { total: { type: Number, required: true } },
  setup(props) {
    const el = ref<HTMLElement | null>(null)
    const total = computed(() => props.total)
    const grid = useVirtualGrid(total, el, { columnCount: 6 })
    return { el, grid }
  },
  render() {
    return h('div', { ref: 'el', class: 'host', style: { height: '600px', overflow: 'auto' } })
  },
})

type GridVm = {
  grid: VirtualGridReturn
  el: Ref<HTMLElement | null>
}

describe('useVirtualGrid 边界', () => {
  it('F3 超阈值(501)启用虚拟行，行数与区间正确', () => {
    const wrapper = mount(VirtualHost, { props: { total: 501 } })
    const g = (wrapper.vm as unknown as GridVm).grid
    expect(g.enabled.value).toBe(true)
    expect(g.totalRows.value).toBe(84) // ceil(501/6)
    expect(g.startIndex.value).toBe(0)
    // 初始 scrollTop=0 → endRow = buffer+可见；endIndex 至少渲染前 2 行
    expect(g.endIndex.value).toBeGreaterThan(0)
    expect(g.endIndex.value).toBeLessThanOrEqual(501)
  })
  it('F4 低阈值(20)跳过虚拟行，全量渲染', () => {
    const wrapper = mount(VirtualHost, { props: { total: 20 } })
    const g = (wrapper.vm as unknown as GridVm).grid
    expect(g.enabled.value).toBe(false)
    expect(g.endIndex.value).toBe(20)
  })
  it('F5 空列表(0)不越界', () => {
    const wrapper = mount(VirtualHost, { props: { total: 0 } })
    const g = (wrapper.vm as unknown as GridVm).grid
    expect(g.totalRows.value).toBe(0)
    expect(g.startIndex.value).toBe(0)
    expect(g.endIndex.value).toBe(0)
    expect(g.spacerBottom.value).toBe(0)
  })
})

// ---------- F6: useLoadingDots ----------
const LoadingHost = defineComponent({
  setup() {
    const d = useLoadingDots()
    return { d, start: () => d.startDots() }
  },
  render() {
    return h('div', {})
  },
})

describe('useLoadingDots', () => {
  beforeAll(() => vi.useFakeTimers())
  afterAll(() => vi.useRealTimers())

  it('F6 启动后按间隔循环，停止后不再增长', () => {
    const wrapper = mount(LoadingHost)
    const d = (wrapper.vm as unknown as { d: ReturnType<typeof useLoadingDots> }).d
    expect(d.dotsCount.value).toBe(1)
    wrapper.vm.start()
    vi.advanceTimersByTime(310)
    // 一个间隔后应处于 2..6 之间且非 1
    expect(d.dotsCount.value).not.toBe(1)
    expect(d.dotsCount.value).toBeGreaterThanOrEqual(1)
    expect(d.dotsCount.value).toBeLessThanOrEqual(6)
    d.stopDots()
    const after = d.dotsCount.value
    vi.advanceTimersByTime(1000)
    // 停止后不再变化
    expect(d.dotsCount.value).toBe(after)
  })
})

// ---------- F7 / F8 / F9: mods 文本纯函数 ----------
describe('mods 文本规范化纯函数', () => {
  let store: ReturnType<typeof useModsStore>
  beforeAll(() => {
    setActivePinia(createPinia())
    store = useModsStore()
  })

  it('F7 NFKC 归一化：全角字符 + 全角空格 + 大小写', () => {
    expect(store.normalizeText('　ＡＢＣ')).toBe('abc')
    expect(store.normalizeText('\u00A0foo\u202F')).toBe('foo')
    expect(store.normalizeText('MIXED \u3000 case')).toBe('mixedcase')
  })

  it('F8 跨语言模糊搜索命中（全角数字→半角）', () => {
    // normalizeText 做 NFKC，全角１２３ 归一化为 123，故断言按 123 命中
    const r = store.fuzzyMatchWithSpansSimple('Weapon１２３', '123')
    expect(r.matched).toBe(true)
    expect(r.spans.length).toBeGreaterThan(0)
  })

  it('F8b 未命中查询返回 matched=false', () => {
    const r = store.fuzzyMatchWithSpansSimple('HelloWorld', 'xyz')
    expect(r.matched).toBe(false)
  })

  it('F9 超大输入（1万字符 × 500 查询）可快速完成且正确', () => {
    const text = 'abc'.repeat(3334) // 10002 字符
    const query = 'a'.repeat(500)
    const start = performance.now()
    const r = store.fuzzyMatchWithSpansSimple(text, query)
    const ms = performance.now() - start
    expect(r.matched).toBe(true)
    // 宽松上限避免 CI 抖动；双指针线性复杂度应当远低于此
    expect(ms).toBeLessThan(2000)
  })
})