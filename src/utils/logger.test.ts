/**
 * 前端日志转发后端测试（fire-and-forget 契约，2026-09）。
 *
 * 覆盖：
 * - 各级别（dev 全量 + warn/error 无条件）均调用 invoke('frontend_log')
 * - 参数映射：level 大写 / module / msg
 * - 静默失败：invoke reject（浏览器/无后端）不产生未处理 rejection、不抛错
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

const invokeMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...(args as [])),
}))

import { logger } from './logger'

describe('logger 后端转发（frontend_log）', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    // 默认成功
    invokeMock.mockResolvedValue(undefined)
  })

  it('dev 下 debug/info 转发后端（与控制台双写）', () => {
    logger.debug('ModsStore', 'debug msg')
    logger.info('App', 'info msg')
    expect(invokeMock).toHaveBeenCalledTimes(2)
    expect(invokeMock).toHaveBeenNthCalledWith(1, 'frontend_log', {
      level: 'DEBUG',
      module: 'ModsStore',
      msg: 'debug msg',
    })
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'frontend_log', {
      level: 'INFO',
      module: 'App',
      msg: 'info msg',
    })
  })

  it('warn/error 无条件转发后端（release 落盘排查）', () => {
    logger.warn('Watcher', 'warn msg')
    logger.error('ModsStore', 'error msg')
    expect(invokeMock).toHaveBeenCalledTimes(2)
    expect(invokeMock).toHaveBeenNthCalledWith(1, 'frontend_log', {
      level: 'WARN',
      module: 'Watcher',
      msg: 'warn msg',
    })
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'frontend_log', {
      level: 'ERROR',
      module: 'ModsStore',
      msg: 'error msg',
    })
  })

  it('fire-and-forget 静默：invoke 拒绝（无后端）时不抛错、无未处理 rejection', async () => {
    invokeMock.mockRejectedValue(new Error('backend unavailable'))
    // 调用不返回 Promise（void），同步不抛
    expect(() => logger.error('App', 'boom')).not.toThrow()
    // 等待微任务：断言未产生未处理 rejection（vitest 对 unhandled rejection 会自动失败；
    // 此处仅确保调用后 0 个未捕获——invokeMock 被 catch 消费）
    await new Promise((r) => setTimeout(r, 0))
    expect(invokeMock).toHaveBeenCalledTimes(1)
  })
})