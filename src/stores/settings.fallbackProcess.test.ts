/**
 * settings store 进程名兜底回归：游戏标识的两种线上形态都必须命中兜底表。
 *
 * 背景（静默失配类缺陷）：`TargetGame` 的字面量写成 Rust `Debug` 形态（`GenshinImpact`），
 * 而后端 serde 以 `rename_all = "camelCase"` 上线（应用日志实测 `detected game: genshinImpact`）。
 * 兜底表曾按 Debug 形态精确建键 ⇒ 设置加载后 `currentGame` 变成 camel 形态，
 * 查表恒 `undefined`、`currentTargetProcess` 恒空串，前台游戏检测随之退化，且全程无报错。
 * 修法是把兜底表键与查表值都做小写归一（`Lowercase<TargetGame>`），本测试钉住这条口径。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

vi.mock('../utils/tauri', () => ({
  getSettings: vi.fn(),
  saveSettings: vi.fn(),
  setGameModsPath: vi.fn(),
  reregisterHotkeys: vi.fn(),
}))
vi.mock('../utils/logger', () => ({
  logger: { info: vi.fn(), warn: vi.fn(), error: vi.fn(), debug: vi.fn() },
}))

import { useSettingsStore } from './settings'
import type { TargetGame } from '@/types'

/** Debug 形态 ↔ 后端 serde camelCase 形态（后者以应用日志实测的 genshinImpact 为准） */
const FORM_PAIRS: Array<[TargetGame, string, string]> = [
  ['GenshinImpact', 'genshinImpact', 'GenshinImpact.exe'],
  ['HonkaiStarRail', 'honkaiStarRail', 'StarRail.exe'],
  ['Wuwa', 'wuwa', 'Client-Win64-Shipping.exe'],
  ['ZZZ', 'zzz', 'ZenlessZoneZero.exe'],
  ['HonkaiImpact3rd', 'honkaiImpact3rd', 'BH3.exe'],
  ['ArknightsEndfield', 'arknightsEndfield', 'Endfield.exe'],
]

describe('settings store 进程名兜底（与游戏标识形态无关）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('后端未下发 targetProcessPerGame 时，六个游戏的两种形态都命中兜底', () => {
    const store = useSettingsStore()
    store.settings = {}
    for (const [debugForm, camelForm, exe] of FORM_PAIRS) {
      store.currentGame = debugForm
      expect(store.currentTargetProcess, `Debug 形态 ${debugForm} 未命中兜底`).toBe(exe)
      store.currentGame = camelForm as TargetGame
      expect(store.currentTargetProcess, `camel 形态 ${camelForm} 未命中兜底`).toBe(exe)
    }
  })

  it('后端下发 targetProcessPerGame 时优先用后端值，兜底不覆盖', () => {
    const store = useSettingsStore()
    // 经 unknown 转：后端 serde 下发的是 camelCase 键，而 `Record<TargetGame, string>`
    // 的字面量键是 Debug 形态——TS 拒绝直转正是这条契约不一致的体现，测试按线上真实形态喂数据。
    store.settings = {
      targetProcessPerGame: { genshinImpact: 'CustomLauncher.exe' } as unknown as Record<TargetGame, string>,
    }
    store.currentGame = 'genshinImpact' as TargetGame
    expect(store.currentTargetProcess).toBe('CustomLauncher.exe')
  })

  it('后端表缺该游戏时仍回落到兜底（不得返回空串）', () => {
    const store = useSettingsStore()
    store.settings = {
      targetProcessPerGame: { genshinImpact: 'CustomLauncher.exe' } as unknown as Record<TargetGame, string>,
    }
    store.currentGame = 'zzz' as TargetGame
    expect(store.currentTargetProcess).toBe('ZenlessZoneZero.exe')
  })
})
