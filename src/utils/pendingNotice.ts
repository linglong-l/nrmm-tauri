/**
 * 模态框待处理状态监视器
 *
 * 通过 MutationObserver 监视 body 上 Element Plus 模态框（message-box）的挂载/移除，
 * 实时将「是否有待处理模态框」上报后端。供后端窗口空闲回收决策：隐藏窗口后，
 * 仅当无待处理模态框时才清空模组数据，避免毁掉用户尚未确认的弹窗状态。
 */
import { setFrontendPendingInfo } from './tauri'
import { logger } from './logger'

/**
 * 检测 body 上是否存在已打开的模态框（Element Plus message-box overlay）
 * 两种选择器互为兜底：is-message-box 覆盖直接挂载到 body 的渲染，嵌套选择器
 * 覆盖 overlay 内再包裹一层 message-box 的场景。
 * @returns 是否存在待处理模态框
 */
export function hasOpenMessageBox(): boolean {
  return !!(
    document.querySelector('.el-overlay.is-message-box') ||
    document.querySelector('.el-overlay .el-message-box')
  )
}

/** 模块级去重标记：仅当待处理状态发生变化时才上报，避免频繁调用后端 */
let lastReported: boolean | null = null

/** 已启动的 MutationObserver 实例（幂等防重复 observe/监听的前提） */
let observer: MutationObserver | null = null

/**
 * 上报当前待处理状态（状态未变化时跳过）
 * @param pending 当前是否存在待处理模态框
 */
function report(pending: boolean): void {
  if (lastReported === pending) return
  lastReported = pending
  setFrontendPendingInfo(pending).catch((e) => {
    // safeInvoke 内部已记录 error；此处兜底避免未捕获的 promise 拒绝
    logger.warn('pendingNotice', 'setFrontendPendingInfo failed', e)
  })
}

/**
 * 幂等启动 body 的 MutationObserver 监听
 * - 已存在 observer 则直接返回，不重复 observe
 * - 监听 body 的 childList 变化（子树），模态框增删都会触发
 * - 启动时先上报一次当前状态，确保后端拿到基线值
 */
export function startPendingNoticeObserver(): void {
  if (observer) return
  observer = new MutationObserver(() => {
    report(hasOpenMessageBox())
  })
  observer.observe(document.body, { childList: true, subtree: true })
  report(hasOpenMessageBox())
}

/**
 * 测试辅助：读取当前的上报去重状态
 */
export function _lastReported(): boolean | null {
  return lastReported
}