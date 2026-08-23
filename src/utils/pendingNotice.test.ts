/**
 * pendingNotice 测试：模态框待处理状态上报（jsdom + MutationObserver）
 *
 * mock 掉 @/utils/tauri 的 setFrontendPendingInfo，验证：
 * - 动态加入含 class "el-overlay is-message-box" 的节点 → invoke(true)
 * - 移除该节点 → invoke(false)
 * - 幂等：重复 startPendingNoticeObserver 不重复观察
 * - setFrontendPendingInfo 的 mock 调用序列与 lastReported 状态
 */
import { describe, it, expect, vi, afterEach, type Mock } from 'vitest'

/** setFrontendPendingInfo 的 mock 引用 */
let mockSetPending: Mock

/**
 * resetModules + doMock 后动态导入模块，确保每个用例得到全新的闭包状态
 * （observer / lastReported 均为模块级单例，测试间须重置）
 */
async function loadPendingNotice() {
  vi.resetModules()
  mockSetPending = vi.fn().mockResolvedValue(undefined)
  vi.doMock('@/utils/tauri', () => ({
    setFrontendPendingInfo: mockSetPending,
  }))
  return await import('./pendingNotice')
}

afterEach(() => {
  document.body.innerHTML = ''
  vi.restoreAllMocks()
  vi.doUnmock('@/utils/tauri')
})

describe('pendingNotice MutationObserver', () => {
  it('加入含 "el-overlay is-message-box" 的节点 → invoke(true)，移除 → invoke(false)', async () => {
    const mod = await loadPendingNotice()
    mod.startPendingNoticeObserver()
    // 启动时先上报当前状态作为去重基准：无模态框 → false
    expect(mockSetPending).toHaveBeenCalledWith(false)
    mockSetPending.mockClear()

    const overlay = document.createElement('div')
    overlay.className = 'el-overlay is-message-box'
    document.body.appendChild(overlay)
    await vi.waitFor(() => {
      expect(mockSetPending).toHaveBeenCalledWith(true)
    })

    document.body.removeChild(overlay)
    await vi.waitFor(() => {
      expect(mockSetPending).toHaveBeenCalledWith(false)
    })

    // 调用序列：加入 → true，移除 → false
    expect(mockSetPending.mock.calls).toEqual([[true], [false]])
    expect(mod._lastReported()).toBe(false)
  })

  it('幂等：重复 startPendingNoticeObserver 只创建一个 observer', async () => {
    // 统计实际构造的 MutationObserver 实例数
    let constructCount = 0
    const OrigMO = globalThis.MutationObserver
    class TrackingMO extends OrigMO {
      constructor(cb: MutationCallback) {
        super(cb)
        constructCount++
      }
    }
    vi.stubGlobal('MutationObserver', TrackingMO)

    const mod = await loadPendingNotice()
    mod.startPendingNoticeObserver()
    mod.startPendingNoticeObserver()
    mod.startPendingNoticeObserver()

    expect(constructCount).toBe(1)
    vi.unstubAllGlobals()
  })

  it('状态未变化时不上报（去重），重复启动不产生额外调用', async () => {
    const mod = await loadPendingNotice()
    mod.startPendingNoticeObserver()
    mockSetPending.mockClear()

    // 加入节点触发一次 true
    const overlay = document.createElement('div')
    overlay.className = 'el-overlay is-message-box'
    document.body.appendChild(overlay)
    await vi.waitFor(() => {
      expect(mockSetPending).toHaveBeenCalledWith(true)
    })
    const callsAfterAdd = mockSetPending.mock.calls.length

    // 再次幂等启动：observer 已存在直接 return，不产生新增上报
    mod.startPendingNoticeObserver()
    expect(mockSetPending.mock.calls.length).toBe(callsAfterAdd)
    expect(mod._lastReported()).toBe(true)
  })
})