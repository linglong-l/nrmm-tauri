/**
 * 组内模组排序前端镜像测试（与后端 mod_order_cmp / sort_mods_light 口径双向锁定）。
 *
 * 覆盖场景：
 * - 启用在前、禁用在后（disabled / modDisabled 双字段判定）
 * - 启用组内收藏优先；名称自然序
 * - None 空槽位恒为首位（mod_index 基准不漂移）
 * - reindexMods：None=0 递增重编号
 */
import { describe, it, expect } from 'vitest'
import { compareModsLikeLightScan, sortModsLikeLightScan, reindexMods, isNoneSlotMod, isModEnabled, naturalCompare } from './modSort'
import type { SortableMod } from './modSort'

function mod(name: string, overrides: Partial<SortableMod> = {}): SortableMod {
  return { name, modName: name, disabled: false, modDisabled: false, isFavorite: false, ...overrides }
}

describe('isModEnabled / isNoneSlotMod', () => {
  it('双字段任一禁用即视为禁用（与后端 !disabled && !modDisabled 同口径）', () => {
    expect(isModEnabled(mod('A'))).toBe(true)
    expect(isModEnabled(mod('A', { disabled: true }))).toBe(false)
    expect(isModEnabled(mod('A', { modDisabled: true }))).toBe(false)
  })

  it('None 空槽位判定（name 或 modPath）', () => {
    expect(isNoneSlotMod({ name: 'None' })).toBe(true)
    expect(isNoneSlotMod({ modPath: 'None', name: undefined })).toBe(true)
    expect(isNoneSlotMod({ name: 'ModA' })).toBe(false)
  })
})

describe('compareModsLikeLightScan（镜像后端 mod_order_cmp）', () => {
  it('启用优先于禁用（以字段为准，不看目录名前缀）', () => {
    expect(compareModsLikeLightScan(mod('Enabled'), mod('Disabled', { disabled: true }))).toBeLessThan(0)
    expect(compareModsLikeLightScan(mod('Disabled', { disabled: true }), mod('Enabled'))).toBeGreaterThan(0)
    // 目录名带 DISABLED 前缀但字段为 false → 仍按启用处理（后端派生只读字段）
    expect(compareModsLikeLightScan(mod('DISABLED_NamedButEnabled'), mod('Zebra', { disabled: true }))).toBeLessThan(0)
  })

  it('启用组内收藏优先', () => {
    const fav = mod('B', { isFavorite: true })
    const nonFav = mod('A')
    expect(compareModsLikeLightScan(fav, nonFav)).toBeLessThan(0)
  })

  it('同禁态同收藏态按名称自然序', () => {
    expect(compareModsLikeLightScan(mod('Alpha'), mod('Beta'))).toBeLessThan(0)
    expect(compareModsLikeLightScan(mod('Beta'), mod('Alpha'))).toBeGreaterThan(0)
    expect(compareModsLikeLightScan(mod('A'), mod('A'))).toBe(0)
  })
})

describe('sortModsLikeLightScan（容器就地排序，None 恒首）', () => {
  it('禁用模组沉底、启用模组前置，None 保持第 0 位', () => {
    const none = mod('None')
    const zDisabled = mod('Zeta', { disabled: true })
    const aEnabled = mod('Alpha')
    const mDisabled = mod('Mid', { modDisabled: true })
    const list = [none, zDisabled, aEnabled, mDisabled]
    sortModsLikeLightScan(list)
    expect(list.map((m) => m.name)).toEqual(['None', 'Alpha', 'Mid', 'Zeta'])
  })

  it('启用组内收藏优先：无收藏时名称序', () => {
    const list = [mod('Bravo'), mod('Charlie', { isFavorite: true }), mod('Alpha')]
    sortModsLikeLightScan(list)
    expect(list.map((m) => m.name)).toEqual(['Charlie', 'Alpha', 'Bravo'])
  })

  it('无 None 时（互斥组）纯排序不 panic', () => {
    const list = [mod('Zee', { disabled: true }), mod('Ally')]
    sortModsLikeLightScan(list)
    expect(list.map((m) => m.name)).toEqual(['Ally', 'Zee'])
  })
})

describe('reindexMods（mod_index 按 None=0 递增）', () => {
  it('重排后重编号，None 索引 0、真实模组从 1 起', () => {
    const none = mod('None')
    const a = mod('Alpha')
    const b = mod('Beta')
    const list = [b, none, a]
    sortModsLikeLightScan(list)
    reindexMods(list as (typeof list)[number][])
    expect(list.map((m) => (m as { modIndex?: number }).modIndex)).toEqual([0, 1, 2])
  })
})
describe('naturalCompare（镜像后端 mod_scanner::natural_compare）', () => {
  it('核心分歧用例：数字段按数值比较，而非 localeCompare 的字符比较', () => {
    // natural 序：mod2 < mod10（数值 2 < 10）
    expect(naturalCompare('mod2', 'mod10')).toBeLessThan(0)
    expect(naturalCompare('mod10', 'mod2')).toBeGreaterThan(0)
    // 反例：localeCompare 恰恰相反（字符 '1' < '2'），这正是修复前后端分叉的根因
    expect('mod10'.localeCompare('mod2')).toBeLessThan(0)
  })

  it('数字段数值相等时跳过该段继续比较（前导零：数值相等 → 退化为长度序）', () => {
    // 后端口径：'mod01' vs 'mod1' 数字段数值同为 1 → continue，随后一方耗尽 → 比码点总数
    // 故 mod01(5) > mod1(4)。此处刻意锁定该「非直觉」行为，避免日后被「顺手修好」而与后端分叉
    expect(naturalCompare('mod01', 'mod1')).toBeGreaterThan(0)
    expect(naturalCompare('mod1a', 'mod1b')).toBeLessThan(0)
  })

  it('大小写不敏感（后端先 to_lowercase）', () => {
    expect(naturalCompare('Alpha', 'alpha')).toBe(0)
    expect(naturalCompare('Beta', 'alpha')).toBeGreaterThan(0)
  })

  it('一方为前缀时按码点总数定序（后端 a_chars.len().cmp(b_chars.len())）', () => {
    expect(naturalCompare('mod', 'mod2')).toBeLessThan(0)
    expect(naturalCompare('a1', 'a')).toBeGreaterThan(0)
  })

  it('真实模组名（带小数点数字）与后端同序：4.3 早于 4.10', () => {
    expect(naturalCompare('女主-异界联动 4.3', '女主-异界联动 4.10')).toBeLessThan(0)
  })

  it('比较器整体透传：同禁态同收藏态下走自然序', () => {
    expect(compareModsLikeLightScan(mod('mod2'), mod('mod10'))).toBeLessThan(0)
    // 启用优先 / 收藏优先的键仍然优先于名称序
    expect(
      compareModsLikeLightScan(mod('zzz', { isFavorite: true }), mod('aaa', { isFavorite: false })),
    ).toBeLessThan(0)
    expect(
      compareModsLikeLightScan(mod('zzz'), mod('aaa', { disabled: true })),
    ).toBeLessThan(0)
  })
})
