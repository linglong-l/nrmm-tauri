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
import { predictToggledPath, stripDisabledPrefix, isDisabledDir, isWatcherOnModsPath, isPathUnderDir } from './modPath'

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

/** 反斜杠常量：测试内以拼接构造 Windows 路径，避免源码散布转义 */
const BS = String.fromCharCode(92)
/** 把 `/` 风格路径转为 Windows 反斜杠风格 */
const win = (path: string): string => path.split('/').join(BS)

describe('isWatcherOnModsPath（监控路径归属判定）', () => {
  const mods = win('D:/Games/GG/Mod')
  const managed = win('D:/Games/GG/Mod/_MANAGED_')

  it('监控已挂在目标 _MANAGED_ 上 → true', () => {
    expect(isWatcherOnModsPath(managed, mods)).toBe(true)
  })

  it('分隔符形态与尾部多余分隔符不影响判定', () => {
    expect(isWatcherOnModsPath('D:/Games/GG/Mod/_MANAGED_', mods)).toBe(true)
    expect(isWatcherOnModsPath(managed, mods + BS)).toBe(true)
  })

  it('监控挂在别处 → false', () => {
    // 旧实现只截尾若干字符比较，会把另一个同样以 Mod 结尾的根误判为已就位
    expect(isWatcherOnModsPath(win('C:/Other/Mod/_MANAGED_'), mods)).toBe(false)
    expect(isWatcherOnModsPath(win('D:/Games/GG/Mod/_MANAGED_/group_1'), mods)).toBe(false)
    expect(isWatcherOnModsPath(win('D:/Games/GG/DISABLEDMod/_MANAGED_'), mods)).toBe(false)
  })

  it('未运行 / 空路径 → false', () => {
    expect(isWatcherOnModsPath(null, mods)).toBe(false)
    expect(isWatcherOnModsPath(undefined, mods)).toBe(false)
    expect(isWatcherOnModsPath(managed, '')).toBe(false)
  })
})

describe('isPathUnderDir（路径段边界归属）', () => {
  it('段名互为前缀时不误判', () => {
    expect(isPathUnderDir('/m/group_1/ModA', '/m/group_1')).toBe(true)
    expect(isPathUnderDir('/m/group_11/ModA', '/m/group_1')).toBe(false)
    expect(isPathUnderDir('/m/group_1_x/ModA', '/m/group_1')).toBe(false)
  })

  it('相等路径不算「之下」，分隔符混用仍可比', () => {
    expect(isPathUnderDir('/m/group_1', '/m/group_1')).toBe(false)
    expect(isPathUnderDir(win('/m/group_1/ModA'), '/m/group_1')).toBe(true)
    expect(isPathUnderDir('/m/group_1/ModA', win('/m/group_1'))).toBe(true)
    expect(isPathUnderDir(win('/m/group_11/ModA'), win('/m/group_1'))).toBe(false)
  })
})
