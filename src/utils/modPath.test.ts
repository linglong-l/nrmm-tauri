/**
 * 模组路径预测纯函数测试（S5 口径：与后端 derive_toggled_path / strip_disabled_prefix 双向锁定）。
 *
 * 覆盖场景：
 * - 基本禁用/启用（DISABLED 前缀增删，保留原分隔符形态）
 * - 幂等：已禁用再禁用 / 已启用再启用 → 原路径
 * - 连续累积前缀剥离（DISABLEDDISABLEDMod → Mod）
 * - 分隔符容忍（DISABLED_ / DISABLED- / DISABLED空格 / 大小写混合）
 * - 路径无分隔符 / 反斜杠分隔符
 */
import { describe, it, expect } from 'vitest'
import { predictToggledPath, stripDisabledPrefix, isDisabledDir } from './modPath'

describe('isDisabledDir / stripDisabledPrefix（后端口径镜像）', () => {
  it('识别 DISABLED 前缀（含分隔符与大小写混合）', () => {
    expect(isDisabledDir('DISABLEDMod')).toBe(true)
    expect(isDisabledDir('DISABLED_Mod')).toBe(true)
    expect(isDisabledDir('disabled-mod')).toBe(true)
    expect(isDisabledDir('Disabled Mod')).toBe(true)
    expect(isDisabledDir('ModDISABLED')).toBe(false) // 非前缀
    expect(isDisabledDir('Mod')).toBe(false)
  })

  it('剥离连续累积前缀：DISABLEDDISABLEDMod → Mod', () => {
    expect(stripDisabledPrefix('DISABLEDDISABLEDMod')).toBe('Mod')
    expect(stripDisabledPrefix('DISABLED_DISABLED_Mod')).toBe('Mod')
    expect(stripDisabledPrefix('disabled Mod')).toBe('Mod')
  })

  it('无前缀 / 仅大小写混合非累积时不误删', () => {
    expect(stripDisabledPrefix('Mod')).toBe('Mod')
    expect(stripDisabledPrefix('DisabledStyle')).toBe('Style') // 单次可剥
    // 剥离后剩余不以精确全大写 DISABLED 开头 → 停止循环（不误删自然词）
    expect(stripDisabledPrefix('DISABLEDDisabledMod')).toBe('DisabledMod')
  })
})

describe('predictToggledPath（幂等 + 前缀增删）', () => {
  const p = (name: string) => `/m/_MANAGED_/group_1/${name}`

  it('禁用：加精确全大写 DISABLED 前缀', () => {
    expect(predictToggledPath(p('ModA'), true)).toBe(p('DISABLEDModA'))
  })

  it('禁用幂等：已禁用再禁用返回原路径', () => {
    expect(predictToggledPath(p('DISABLEDModA'), true)).toBe(p('DISABLEDModA'))
    expect(predictToggledPath(p('disabled_ModA'), true)).toBe(p('disabled_ModA'))
  })

  it('启用：剥离前缀恢复原名', () => {
    expect(predictToggledPath(p('DISABLEDModA'), false)).toBe(p('ModA'))
    expect(predictToggledPath(p('DISABLED_DISABLED_ModA'), false)).toBe(p('ModA'))
  })

  it('启用幂等：已启用返回原路径；剥后为空也返回原路径', () => {
    expect(predictToggledPath(p('ModA'), false)).toBe(p('ModA'))
    expect(predictToggledPath(p('DISABLED'), false)).toBe(p('DISABLED'))
  })

  it('反斜杠分隔符：保留原分隔符形态', () => {
    expect(predictToggledPath('\\m\\group_1\\ModA', true)).toBe('\\m\\group_1\\DISABLEDModA')
    expect(predictToggledPath('\\m\\group_1\\DISABLEDModA', false)).toBe('\\m\\group_1\\ModA')
  })

  it('无分隔符的纯目录名', () => {
    expect(predictToggledPath('ModA', true)).toBe('DISABLEDModA')
    expect(predictToggledPath('DISABLEDModA', false)).toBe('ModA')
  })

  it('与后端 toggle 往返一致：disable → enable 还原原路径', () => {
    const original = p('ModA')
    const disabled = predictToggledPath(original, true)
    expect(predictToggledPath(disabled, false)).toBe(original)
  })
})
