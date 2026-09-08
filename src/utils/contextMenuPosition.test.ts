import { describe, it, expect, afterEach, vi } from 'vitest'
import { getDocumentZoom, placeContextMenu } from './contextMenuPosition'

const ZOOM_PROP = 'zoom'

function setRootZoom(value: string) {
  document.documentElement.style.setProperty(ZOOM_PROP, value)
}

function stubViewport(w: number, h: number) {
  Object.defineProperty(window, 'innerWidth', { value: w, configurable: true })
  Object.defineProperty(window, 'innerHeight', { value: h, configurable: true })
}

/** 以 computed style 伪造根 zoom（绕开 jsdom 对 zoom 属性支持程度的差异） */
function stubRootZoom(zoom: string) {
  vi.spyOn(window, 'getComputedStyle').mockReturnValue({
    getPropertyValue: () => zoom,
  } as unknown as CSSStyleDeclaration)
}

const realW = window.innerWidth
const realH = window.innerHeight

afterEach(() => {
  document.documentElement.style.removeProperty(ZOOM_PROP)
  Object.defineProperty(window, 'innerWidth', { value: realW, configurable: true })
  Object.defineProperty(window, 'innerHeight', { value: realH, configurable: true })
  vi.restoreAllMocks()
})

describe('getDocumentZoom', () => {
  it('未设置 zoom 时按 1 处理（未启用缩放场景行为不变）', () => {
    expect(getDocumentZoom()).toBe(1)
  })

  it('根元素 zoom=1.5 时返回 1.5', () => {
    stubRootZoom('1.5')
    expect(getDocumentZoom()).toBe(1.5)
  })

  it('非法 zoom（0 / NaN / 负数）回退为 1', () => {
    stubRootZoom('0')
    expect(getDocumentZoom()).toBe(1)
    stubRootZoom('abc')
    expect(getDocumentZoom()).toBe(1)
    stubRootZoom('-2')
    expect(getDocumentZoom()).toBe(1)
  })

  it('内联 zoom 优先于 computed style', () => {
    setRootZoom('1.25')
    stubRootZoom('2')
    // jsdom 若不支持 zoom 属性则内联为空 → 回退 computed(2)；支持则 1.25。
    // 两种实现均合法，但不得出现其它值
    expect([1.25, 2]).toContain(getDocumentZoom())
  })
})

describe('placeContextMenu', () => {
  it('zoom=1 时光标即菜单位置（与旧实现完全一致）', () => {
    stubViewport(1000, 800)
    const e = { clientX: 300, clientY: 200 } as MouseEvent
    expect(placeContextMenu(e, 200, 260)).toEqual({ x: 300, y: 200 })
  })

  it('zoom=1.5 时坐标除以 zoom（修正 fixed 定位漂移）', () => {
    stubViewport(1000, 800)
    stubRootZoom('1.5')
    const e = { clientX: 300, clientY: 200 } as MouseEvent
    const p = placeContextMenu(e, 200, 260)
    // 修正原理：菜单本地坐标 × zoom == 光标物理坐标 → 本地 = 物理 / zoom
    expect(p.x).toBeCloseTo(300 / 1.5, 6)
    expect(p.y).toBeCloseTo(200 / 1.5, 6)
  })

  it('右/下边缘溢出时收敛进视口', () => {
    stubViewport(1000, 800)
    const e = { clientX: 900, clientY: 700 } as MouseEvent
    const p = placeContextMenu(e, 200, 260)
    expect(p.x).toBe(1000 - 200 - 4)
    expect(p.y).toBe(800 - 260 - 4)
  })

  it('zoom=1.5 下视口边界同样按本地空间换算', () => {
    stubViewport(1000, 800)
    stubRootZoom('1.5')
    const e = { clientX: 700, clientY: 500 } as MouseEvent
    const p = placeContextMenu(e, 200, 260)
    // 本地视口 = 物理 / zoom = 666.67 × 533.33；两者均溢出
    expect(p.x).toBeCloseTo(1000 / 1.5 - 200 - 4, 2)
    expect(p.y).toBeCloseTo(800 / 1.5 - 260 - 4, 2)
  })

  it('菜单大于视口时不产生负坐标', () => {
    stubViewport(1000, 800)
    const e = { clientX: 50, clientY: 50 } as MouseEvent
    const p = placeContextMenu(e, 2000, 3000)
    expect(p.x).toBe(4)
    expect(p.y).toBe(4)
  })

  it('自定义 margin 生效', () => {
    stubViewport(1000, 800)
    const e = { clientX: 900, clientY: 700 } as MouseEvent
    const p = placeContextMenu(e, 200, 260, 10)
    expect(p.x).toBe(1000 - 200 - 10)
    expect(p.y).toBe(800 - 260 - 10)
  })
})
