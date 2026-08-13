import { ref, onBeforeUnmount } from 'vue'

/** 加载动画省略号循环上限（0..6 个点） */
export const MAX_LOADING_DOTS = 6
/** 省略号切换间隔（ms） */
export const DOTS_INTERVAL_MS = 300

/**
 * 加载省略号动画状态机
 *
 * 统一管理各对话框的 "..." 循环动画（启动 / 停止 / 卸载清理），
 * 消除 UpdateModDataOverlay / HashConflictOverlay / RemoveModDialog 三处重复实现。
 *
 * - `dotsCount`：模板 `v-for` 渲染省略号用（数值 1..6 循环）
 * - `startDots` / `stopDots`：控制定时器
 * - 组件卸载时自动 `stopDots()`，避免定时器泄漏（等价原各自 `onBeforeUnmount`）
 */
export function useLoadingDots() {
  const dotsCount = ref(1)
  let dotsTimer: ReturnType<typeof setInterval> | null = null

  function startDots() {
    stopDots()
    dotsTimer = setInterval(() => {
      dotsCount.value = (dotsCount.value % MAX_LOADING_DOTS) + 1
    }, DOTS_INTERVAL_MS)
  }

  function stopDots() {
    if (dotsTimer) {
      clearInterval(dotsTimer)
      dotsTimer = null
    }
  }

  onBeforeUnmount(stopDots)

  return { dotsCount, startDots, stopDots }
}
