/**
 * useModContextMenu 启停/收藏链路乐观更新测试（UI 先行 + 回滚 + 校正，2026-09-06 计划 Task 3）。
 *
 * 覆盖场景：
 * - L1 UI 先行：toggleModDisabled 挂起时同步调用后 toggleModLocal 已按预测路径 patch
 * - 成功一致：预测与后端返回一致 → 无二次 patch，markNeedUpdate + 成功通知
 * - 成功校正：后端返回路径与预测不一致 → 以权威值二次 patch
 * - 失败回滚：toggleModLocal 反向恢复原路径与原禁用态 + ElMessage.error，markNeedUpdate 不置位
 * - 收藏乐观：toggleFavoriteLocal 先行翻转；失败回翻
 *
 * 实现方式：顶层 vi.mock 拦截 '@/utils/tauri'、'@/stores/mods'、'@/stores/settings'，
 * element-plus 经 importOriginal 部分替换以捕获 ElMessage 调用。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { ref } from 'vue'
import type { ModData } from '@/types'

vi.mock('@/utils/tauri', () => ({
  toggleModDisabled: (...args: unknown[]) => toggleModDisabledMock(...(args as [])),
  toggleFavorite: (...args: unknown[]) => toggleFavoriteMock(...(args as [])),
  openModFolder: vi.fn(),
  isPathNotFoundError: (e: unknown) => String(e).includes('[PATH_NOT_FOUND]'),
  deselectGroupMod: vi.fn(),
}))
vi.mock('./usePathNotFoundRecovery', () => ({
  recoverFromPathNotFound: vi.fn(),
}))
vi.mock('@/stores/mods', () => ({
  useModsStore: () => modsStoreMock,
}))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: () => settingsState,
}))
vi.mock('element-plus', async (importOriginal) => ({
  ...(await importOriginal<typeof import('element-plus')>()),
  ElMessage: {
    error: vi.fn(),
    warning: vi.fn(),
    success: vi.fn(),
    info: vi.fn(),
  },
}))

// mock 声明需先于被导入模块（vi.mock 提升保证执行顺序，变量声明提升由 vitest 处理）
const toggleModDisabledMock = vi.fn()
const toggleFavoriteMock = vi.fn()
const modsStoreMock = {
  toggleModLocal: vi.fn(),
  toggleFavoriteLocal: vi.fn(),
  applyGroupDelta: vi.fn(),
  markNeedUpdate: vi.fn(),
  isUpdatingModData: false,
  setKeybindTargetMod: vi.fn(),
}
const settingsState = { currentModsPath: '/mods/GI', currentGame: 'GI' }

import { ElMessage } from 'element-plus'
import { useModContextMenu } from './useModContextMenu'

/** 构造最小模组数据 */
function makeMod(overrides: Partial<ModData> = {}): ModData {
  return {
    modPath: '/mods/_MANAGED_/group_1/ModA',
    modName: 'ModA',
    name: 'ModA',
    isActive: false,
    isFavorite: false,
    isMutex: false,
    modDisabled: false,
    groupIndex: 1,
    ...overrides,
  } as unknown as ModData
}

/** 构造被测 composable（t 直接回退文案，不依赖 i18n） */
function setup(mod: ModData) {
  const modRef = ref<ModData | null | undefined>(mod)
  const ctx = useModContextMenu(
    modRef,
    ref(1),
    ref(false),
    vi.fn(),
    modsStoreMock as never,
    (key: string, fallback?: string) => fallback ?? key,
  )
  return ctx
}

describe('useModContextMenu 启停链路乐观更新', () => {
  beforeEach(() => {
    toggleModDisabledMock.mockReset()
    toggleFavoriteMock.mockReset()
    modsStoreMock.toggleModLocal.mockClear()
    modsStoreMock.toggleFavoriteLocal.mockClear()
    modsStoreMock.applyGroupDelta.mockClear()
    modsStoreMock.markNeedUpdate.mockClear()
    vi.mocked(ElMessage.error).mockClear()
    vi.mocked(ElMessage.success).mockClear()
    vi.mocked(ElMessage.warning).mockClear()
  })

  it('L1 toggleModDisabled 挂起时同步调用后已按预测路径本地 patch', async () => {
    const mod = makeMod({ modDisabled: false })
    const ctx = setup(mod)
    let resolveToggle: (v: unknown) => void = () => {}
    toggleModDisabledMock.mockReturnValueOnce(
      new Promise((r) => (resolveToggle = r)),
    )

    // 不 await：调用后同步断言 UI 已先行（预测路径 = 加 DISABLED 前缀）
    const pending = ctx.handleToggleEnabled()
    expect(modsStoreMock.toggleModLocal).toHaveBeenCalledTimes(1)
    expect(modsStoreMock.toggleModLocal).toHaveBeenCalledWith(
      '/mods/_MANAGED_/group_1/ModA',
      '/mods/_MANAGED_/group_1/DISABLEDModA',
      true,
    )

    resolveToggle({
      newPath: '/mods/_MANAGED_/group_1/DISABLEDModA',
      disabled: true,
      degraded: false,
      groupDelta: null,
    })
    await pending
    // 预测与后端一致 → 无二次 patch；通知成功
    expect(modsStoreMock.toggleModLocal).toHaveBeenCalledTimes(1)
    expect(modsStoreMock.markNeedUpdate).toHaveBeenCalledWith(1)
    expect(ElMessage.success).toHaveBeenCalledTimes(1)
  })

  it('预测与返回一致 → 无二次 patch（幂等口径）', async () => {
    const mod = makeMod({
      modPath: '/mods/_MANAGED_/group_1/DISABLEDModA',
      modDisabled: true,
    })
    const ctx = setup(mod)
    // 启用：预测剥离前缀；后端返回同一路径
    toggleModDisabledMock.mockResolvedValueOnce({
      newPath: '/mods/_MANAGED_/group_1/ModA',
      disabled: false,
      degraded: false,
      groupDelta: null,
    })

    await ctx.handleToggleEnabled()

    // 第一次：乐观 patch（旧路径 → 预测启用路径）
    expect(modsStoreMock.toggleModLocal).toHaveBeenNthCalledWith(
      1,
      '/mods/_MANAGED_/group_1/DISABLEDModA',
      '/mods/_MANAGED_/group_1/ModA',
      false,
    )
    // 预测与返回一致 → 无第二次 patch
    expect(modsStoreMock.toggleModLocal).toHaveBeenCalledTimes(1)
  })

  it('预测与返回不一致 → 权威值校正二次 patch', async () => {
    const mod = makeMod({ modDisabled: false })
    const ctx = setup(mod)
    toggleModDisabledMock.mockResolvedValueOnce({
      newPath: '/mods/_MANAGED_/group_1/DISABLED_ModA_v2', // 竞态：目录已被外部改名
      disabled: true,
      degraded: false,
      groupDelta: null,
    })

    await ctx.handleToggleEnabled()

    expect(modsStoreMock.toggleModLocal).toHaveBeenNthCalledWith(
      1,
      '/mods/_MANAGED_/group_1/ModA',
      '/mods/_MANAGED_/group_1/DISABLEDModA',
      true,
    )
    expect(modsStoreMock.toggleModLocal).toHaveBeenNthCalledWith(
      2,
      '/mods/_MANAGED_/group_1/DISABLEDModA',
      '/mods/_MANAGED_/group_1/DISABLED_ModA_v2',
      true,
    )
  })

  it('失败回滚：恢复原路径与原禁用态 + 告知用户，markNeedUpdate 不置位', async () => {
    const mod = makeMod({ modDisabled: false })
    const ctx = setup(mod)
    toggleModDisabledMock.mockRejectedValueOnce(new Error('rename failed'))

    await ctx.handleToggleEnabled()

    expect(modsStoreMock.toggleModLocal).toHaveBeenNthCalledWith(
      1,
      '/mods/_MANAGED_/group_1/ModA',
      '/mods/_MANAGED_/group_1/DISABLEDModA',
      true,
    )
    // 回滚：预测路径 → 原路径 + 原禁用态
    expect(modsStoreMock.toggleModLocal).toHaveBeenNthCalledWith(
      2,
      '/mods/_MANAGED_/group_1/DISABLEDModA',
      '/mods/_MANAGED_/group_1/ModA',
      false,
    )
    expect(modsStoreMock.markNeedUpdate).not.toHaveBeenCalled()
    expect(ElMessage.error).toHaveBeenCalledTimes(1)
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('rename failed'))
  })

  it('收藏乐观：先行翻转；失败回翻', async () => {
    const mod = makeMod({ isFavorite: false })
    const ctx = setup(mod)

    // 成功：后端返回 true 与乐观值一致 → 不校正
    toggleFavoriteMock.mockResolvedValueOnce(true)
    await ctx.handleToggleFavorite()
    expect(modsStoreMock.toggleFavoriteLocal).toHaveBeenCalledTimes(1)
    expect(modsStoreMock.toggleFavoriteLocal).toHaveBeenCalledWith(
      '/mods/_MANAGED_/group_1/ModA',
      true,
    )

    // 失败：回翻为原收藏态
    modsStoreMock.toggleFavoriteLocal.mockClear()
    toggleFavoriteMock.mockRejectedValueOnce(new Error('io error'))
    await ctx.handleToggleFavorite()
    expect(modsStoreMock.toggleFavoriteLocal).toHaveBeenCalledTimes(2)
    expect(modsStoreMock.toggleFavoriteLocal).toHaveBeenNthCalledWith(
      1,
      '/mods/_MANAGED_/group_1/ModA',
      true,
    )
    expect(modsStoreMock.toggleFavoriteLocal).toHaveBeenNthCalledWith(
      2,
      '/mods/_MANAGED_/group_1/ModA',
      false,
    )
  })
})
