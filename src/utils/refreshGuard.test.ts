import { describe, it, expect } from 'vitest'
import { isBrowserRefreshKey, isLightRescanKey, isFullRescanKey } from './refreshGuard'

/** 构造 KeyboardEvent 的最小 stub */
function ke(key: string, opts: Partial<KeyboardEvent> = {}): KeyboardEvent {
  return { key, ctrlKey: false, metaKey: false, shiftKey: false, ...opts } as KeyboardEvent
}

describe('isBrowserRefreshKey', () => {
  it('F5（大小写）命中', () => {
    expect(isBrowserRefreshKey(ke('F5'))).toBe(true)
    expect(isBrowserRefreshKey(ke('f5'))).toBe(true)
  })

  it('Ctrl+R / Meta+R 命中', () => {
    expect(isBrowserRefreshKey(ke('r', { ctrlKey: true }))).toBe(true)
    expect(isBrowserRefreshKey(ke('R', { metaKey: true }))).toBe(true)
  })

  it('普通 R / 其他键命中为 false（输入框等场景由调用方过滤）', () => {
    expect(isBrowserRefreshKey(ke('r'))).toBe(false)
    expect(isBrowserRefreshKey(ke('a', { ctrlKey: true }))).toBe(false)
    expect(isBrowserRefreshKey(ke('Enter'))).toBe(false)
  })
})

describe('应用内重扫快捷键（窗口内绑定）', () => {
  it('isLightRescanKey：Ctrl/Meta+R 且无 Shift', () => {
    expect(isLightRescanKey(ke('r', { ctrlKey: true }))).toBe(true)
    expect(isLightRescanKey(ke('R', { metaKey: true }))).toBe(true)
  })

  it('isFullRescanKey：Ctrl/Meta+Shift+R', () => {
    expect(isFullRescanKey(ke('r', { ctrlKey: true, shiftKey: true }))).toBe(true)
    expect(isFullRescanKey(ke('R', { metaKey: true, shiftKey: true }))).toBe(true)
  })

  it('互斥：带 Shift 不是轻量；不带 Shift 不是全量', () => {
    expect(isLightRescanKey(ke('r', { ctrlKey: true, shiftKey: true }))).toBe(false)
    expect(isFullRescanKey(ke('r', { ctrlKey: true }))).toBe(false)
  })

  it('无修饰键 / 其他键均不命中', () => {
    expect(isLightRescanKey(ke('r'))).toBe(false)
    expect(isFullRescanKey(ke('r'))).toBe(false)
    expect(isLightRescanKey(ke('a', { ctrlKey: true }))).toBe(false)
    expect(isFullRescanKey(ke('F5'))).toBe(false)
  })
})