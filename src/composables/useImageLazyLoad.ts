/**
 * 图片懒加载 Composable
 *
 * 功能：基于 IntersectionObserver 实现图片自上而下懒加载，管理加载状态
 *
 * 使用方法：
 * ```vue
 * <script setup>
 * const { states, observeElement, markLoaded, markError } = useImageLazyLoad()
 * const imageState = computed(() => states.value.get(modIndex) || 'idle')
 * onMounted(() => { if (containerRef.value) observeElement(modIndex, containerRef.value) })
 * </script>
 * ```
 *
 * 设计要点：
 * - IntersectionObserver rootMargin: '200px 0px 200px 0px'（进入视口前 200px 预加载）
 * - 加载状态流程：idle → loading → loaded | error
 * - 自上而下加载：进入视口的卡片按索引升序排序，依次触发加载，50ms 交错延迟避免并发过多
 * - 每个元素仅观察一次，进入视口后立即取消观察
 * - 使用 dataset.lazyIndex 存储索引与元素的映射
 *
 * @param options 配置项（rootMargin、交错延迟）
 * @returns 图片懒加载相关状态和方法
 */
import { ref, onMounted, onUnmounted, type Ref } from 'vue'

/** 图片加载状态 */
export type ImageLoadState = 'idle' | 'loading' | 'loaded' | 'error'

export interface ImageLazyLoadOptions {
  /** IntersectionObserver rootMargin，默认 200px 提前加载 */
  rootMargin?: string
  /** 图片加载交错延迟（ms），默认 50ms，避免瞬间并发过多 */
  staggerDelay?: number
}

export interface ImageLazyLoadReturn {
  /** 图片加载状态映射：key 为模组索引，value 为状态 */
  states: Ref<Map<number, ImageLoadState>>
  /** 标记开始加载（状态 → loading） */
  markLoading: (index: number) => void
  /** 标记加载成功（状态 → loaded） */
  markLoaded: (index: number) => void
  /** 标记加载失败（状态 → error） */
  markError: (index: number) => void
  /** 观察元素：当元素进入视口时触发加载 */
  observeElement: (index: number, element: HTMLElement) => void
  /** 取消观察 */
  unobserveElement: (element: HTMLElement) => void
  /** 清理所有观察器和待处理队列 */
  cleanup: () => void
}

export function useImageLazyLoad(
  options: ImageLazyLoadOptions = {}
): ImageLazyLoadReturn {
  const { rootMargin = '200px 0px 200px 0px', staggerDelay = 50 } = options

  /** 图片加载状态映射 */
  const states = ref<Map<number, ImageLoadState>>(new Map())
  /** IntersectionObserver 实例 */
  let observer: IntersectionObserver | null = null
  /** 待加载队列：按索引排序确保自上而下 */
  const pendingQueue: { index: number; element: HTMLElement }[] = []
  /** 队列处理定时器 */
  let processTimer: ReturnType<typeof setTimeout> | null = null

  /**
   * 获取指定索引的加载状态
   * @param index 模组索引
   * @returns 加载状态，未记录时返回 'idle'
   */
  function getState(index: number): ImageLoadState {
    return states.value.get(index) || 'idle'
  }

  /**
   * 设置指定索引的加载状态
   * @param index 模组索引
   * @param state 新状态
   */
  function setState(index: number, state: ImageLoadState) {
    const newMap = new Map(states.value)
    newMap.set(index, state)
    states.value = newMap
  }

  /** 标记开始加载 */
  function markLoading(index: number) {
    setState(index, 'loading')
  }

  /** 标记加载成功 */
  function markLoaded(index: number) {
    setState(index, 'loaded')
  }

  /** 标记加载失败 */
  function markError(index: number) {
    setState(index, 'error')
  }

  /**
   * 处理待加载队列
   * 按索引升序排序后依次触发加载，实现自上而下效果
   * 每次处理后若还有待处理项，延迟 staggerDelay ms 继续
   */
  function processQueue() {
    if (pendingQueue.length === 0) {
      processTimer = null
      return
    }
    // 按索引升序排序（索引小 = DOM 上方，先加载）
    pendingQueue.sort((a, b) => a.index - b.index)
    const item = pendingQueue.shift()!
    if (getState(item.index) === 'idle') {
      markLoading(item.index)
    }
    if (pendingQueue.length > 0) {
      processTimer = setTimeout(processQueue, staggerDelay)
    } else {
      processTimer = null
    }
  }

  /**
   * 开始观察元素
   * 元素进入视口时加入待加载队列
   * @param index 模组索引
   * @param element 要观察的 DOM 元素
   */
  function observeElement(index: number, element: HTMLElement) {
    if (!observer) return
    observer.observe(element)
    // 通过 dataset 存储索引，供 IntersectionObserver 回调使用
    element.dataset.lazyIndex = String(index)
  }

  /**
   * 取消观察元素
   * @param element 要取消观察的 DOM 元素
   */
  function unobserveElement(element: HTMLElement) {
    observer?.unobserve(element)
  }

  /**
   * 清理所有资源
   * 断开观察器、清除定时器、清空队列和状态
   */
  function cleanup() {
    observer?.disconnect()
    observer = null
    if (processTimer !== null) {
      clearTimeout(processTimer)
      processTimer = null
    }
    pendingQueue.length = 0
    states.value = new Map()
  }

  onMounted(() => {
    observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            const el = entry.target as HTMLElement
            const idx = Number(el.dataset.lazyIndex)
            if (!Number.isNaN(idx) && getState(idx) === 'idle') {
              pendingQueue.push({ index: idx, element: el })
            }
            // 每个元素仅观察一次，进入视口后取消观察
            observer?.unobserve(el)
          }
        }
        // 启动队列处理（如果尚未启动）
        if (pendingQueue.length > 0 && processTimer === null) {
          processQueue()
        }
      },
      { rootMargin }
    )
  })

  onUnmounted(() => {
    cleanup()
  })

  return {
    states,
    markLoading,
    markLoaded,
    markError,
    observeElement,
    unobserveElement,
    cleanup
  }
}