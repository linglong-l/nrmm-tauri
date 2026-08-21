import { describe, it, expect } from 'vitest'
import { isBrowserRefreshKey } from './refreshGuard'

/** 构造 KeyboardEvent 的最小 stub */
function ke(key: string, opts: Partial<KeyboardEvent> = {}): KeyboardEvent {
  return { key, ctrlKey: false, metaKey: false, ...opts } as KeyboardEvent
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