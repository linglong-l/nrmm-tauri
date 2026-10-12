import { describe, it, expect } from 'vitest'
import { isLightRescanKey, isFullRescanKey } from './refreshGuard'

/** 构造 KeyboardEvent 的最小 stub */
function ke(key: string, opts: Partial<KeyboardEvent> = {}): KeyboardEvent {
  return { key, ctrlKey: false, metaKey: false, shiftKey: false, ...opts } as KeyboardEvent
}

describe('应用内重扫快捷键（窗口内绑定）', () => {
  it('isLightRescanKey：Ctrl/Meta+R 且无 Shift', () => {
    expect(isLightRescanKey(ke('r', { ctrlKey: true }))).toBe(true)
    expect(isLightRescanKey(ke('R', { metaKey: true }))).toBe(true)
  })

  it('isLightRescanKey：F5 与 Ctrl+R 同义（大小写都命中）', () => {
    expect(isLightRescanKey(ke('F5'))).toBe(true)
    expect(isLightRescanKey(ke('f5'))).toBe(true)
  })

  it('isFullRescanKey：Ctrl/Meta+Shift+R', () => {
    expect(isFullRescanKey(ke('r', { ctrlKey: true, shiftKey: true }))).toBe(true)
    expect(isFullRescanKey(ke('R', { metaKey: true, shiftKey: true }))).toBe(true)
  })

  it('互斥：带 Shift 不是轻量；不带 Shift 不是全量；F5 只走轻量', () => {
    expect(isLightRescanKey(ke('r', { ctrlKey: true, shiftKey: true }))).toBe(false)
    expect(isFullRescanKey(ke('r', { ctrlKey: true }))).toBe(false)
    expect(isFullRescanKey(ke('F5'))).toBe(false)
  })

  it('无修饰键 / 其他键均不命中', () => {
    expect(isLightRescanKey(ke('r'))).toBe(false)
    expect(isFullRescanKey(ke('r'))).toBe(false)
    expect(isLightRescanKey(ke('a', { ctrlKey: true }))).toBe(false)
    expect(isLightRescanKey(ke('F6'))).toBe(false)
    expect(isLightRescanKey(ke('Enter'))).toBe(false)
  })
})
