/**
 * 组内激活态派生前端镜像测试（与后端 derive_group_active_state 口径双向锁定）。
 *
 * 覆盖场景：
 * - 多启用（常态）→ 回退 selectedindex 定位；selectedindex=0 → None 激活
 * - 单启用 → 该模组恒激活（selectedindex 不影响）
 * - 零启用 → None 激活，返回 0
 * - selectedindex 指向禁用模组 → fallback 第一个启用模组
 */
import { describe, it, expect } from 'vitest'
import { deriveGroupActiveState } from './groupActiveState'
import type { ActiveStateMod } from './groupActiveState'

function mod(name: string, disabled = false): ActiveStateMod {
  return { name, disabled, isActive: false }
}

describe('deriveGroupActiveState（后端口径镜像）', () => {
  it('多启用 + selectedindex=0 → None 激活、真实模组全失活、返回 0', () => {
    const mods = [mod('None'), mod('ModA'), mod('ModB')]
    expect(deriveGroupActiveState(mods, 0)).toBe(0)
    expect(mods[0].isActive).toBe(true)
    expect(mods[1].isActive).toBe(false)
    expect(mods[2].isActive).toBe(false)
  })

  it('多启用 + selectedindex=N → 下标 N 模组激活', () => {
    const mods = [mod('None'), mod('ModA'), mod('ModB')]
    expect(deriveGroupActiveState(mods, 2)).toBe(2)
    expect(mods[1].isActive).toBe(false)
    expect(mods[2].isActive).toBe(true)
  })

  it('单启用 → 该模组恒激活（selectedindex 不影响）', () => {
    const mods = [mod('None'), mod('ModA')]
    expect(deriveGroupActiveState(mods, 0)).toBe(1)
    expect(mods[0].isActive).toBe(false)
    expect(mods[1].isActive).toBe(true)
  })

  it('零启用 → None 激活，返回 0', () => {
    const mods = [mod('None'), mod('ModA', true)]
    expect(deriveGroupActiveState(mods, 1)).toBe(0)
    expect(mods[0].isActive).toBe(true)
    expect(mods[1].isActive).toBe(false)
  })

  it('selectedindex 指向禁用模组 → fallback 第一个启用模组', () => {
    const mods = [mod('None'), mod('ModA', true), mod('ModB')]
    // selectedindex=1 指向禁用的 ModA → fallback 到 ModB（下标 2）
    expect(deriveGroupActiveState(mods, 1)).toBe(2)
    expect(mods[1].isActive).toBe(false)
    expect(mods[2].isActive).toBe(true)
  })
})
