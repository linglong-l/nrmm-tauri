/**
 * 虚拟行渲染 Composable
 *
 * 功能：根据滚动位置计算可见行范围，仅渲染可见行 + 缓冲行的卡片，减少 DOM 节点数量
 *
 * 使用方法：
 * ```vue
 * <script setup>
 * const scrollRef = ref<HTMLElement | null>(null)
 * const totalCount = computed(() => items.value.length)
 * const { startIndex, endIndex, spacerTop, spacerBottom, enabled } = useVirtualGrid(totalCount, scrollRef)
 * </script>
 * ```
 *
 * 设计要点：
 * - 以行为单位计算可见范围：每行固定 6 个卡片，行高 = 卡片高度(240px) + 行间距(12px) = 252px
 * - 第一行顶部偏移 8px（原 .mod-grid-row 的 padding-top）
 * - 上下各缓冲 2 行，防止快速滚动时出现空白
 * - 模组总数 ≤ 30 时不启用虚拟渲染（全量渲染，避免小数据量时的额外开销）
 * - 使用 requestAnimationFrame 节流滚动事件，确保 60fps+
 * - 通过占位 div 维持滚动条高度，确保滚动体验一致
 *
 * @param totalCount 总卡片数的计算属性
 * @param scrollContainer 滚动容器的 DOM 引用
 * @param options 配置项（行高、列数、缓冲行数、启用阈值、顶部偏移）
 * @returns 虚拟渲染相关状态和方法
 */
import { ref, computed, onMounted, onUnmounted, type Ref, type ComputedRef } from 'vue'

export interface VirtualGridOptions {
  /** 行高（卡片高度 + 行间距），默认 252px */
  rowHeight?: number
  /** 每行卡片数，支持响应式 Ref，默认 6 */
  columnCount?: number | Ref<number>
  /** 上下缓冲行数，默认 2 */
  bufferRows?: number
  /** 启用虚拟渲染的阈值（总卡片数），默认 30 */
  enableThreshold?: number
  /** 第一行顶部偏移，默认 8px */
  topOffset?: number
}

export interface VirtualGridReturn {
  /** 可见卡片起始索引（在全量列表中的位置） */
  startIndex: ComputedRef<number>
  /** 可见卡片结束索引（不含） */
  endIndex: ComputedRef<number>
  /** 上方占位高度 */
  spacerTop: ComputedRef<number>
  /** 下方占位高度 */
  spacerBottom: ComputedRef<number>
  /** 是否启用虚拟行渲染 */
  enabled: ComputedRef<boolean>
  /** 总行数 */
  totalRows: ComputedRef<number>
  /** 强制重新计算可见范围 */
  recalculate: () => void
}

export function useVirtualGrid(
  totalCount: ComputedRef<number>,
  scrollContainer: Ref<HTMLElement | null>,
  options: VirtualGridOptions = {}
): VirtualGridReturn {
  const {
    rowHeight = 252,
    bufferRows = 2,
    enableThreshold = 30,
    topOffset = 8
  } = options

  /**
   * 每行卡片数，支持响应式 Ref
   * 非 Ref 值会被包装为 Ref，确保所有 computed 都能响应变化
   */
  const rawColCount = options.columnCount ?? 6
  const columnCount = typeof rawColCount === 'number' ? ref(rawColCount) : rawColCount

  /** 当前滚动位置 */
  const scrollTop = ref(0)
  /** 容器可视高度 */
  const containerHeight = ref(0)

  /** 总行数 */
  const totalRows = computed(() => Math.ceil(totalCount.value / columnCount.value))

  /** 是否启用虚拟渲染：总卡片数超过阈值时启用 */
  const enabled = computed(() => totalCount.value > enableThreshold)

  /** 视口可见起始行（不含缓冲） */
  const visibleStartRow = computed(() => {
    if (!enabled.value) return 0
    return Math.max(0, Math.floor((scrollTop.value - topOffset) / rowHeight))
  })

  /** 视口可见结束行（不含缓冲） */
  const visibleEndRow = computed(() => {
    if (!enabled.value) return totalRows.value
    return Math.min(
      totalRows.value,
      Math.ceil((scrollTop.value - topOffset + containerHeight.value) / rowHeight)
    )
  })

  /** 含缓冲的起始行 */
  const startRow = computed(() => Math.max(0, visibleStartRow.value - bufferRows))
  /** 含缓冲的结束行 */
  const endRow = computed(() => Math.min(totalRows.value, visibleEndRow.value + bufferRows))

  /** 可见卡片起始索引 */
  const startIndex = computed(() => startRow.value * columnCount.value)
  /** 可见卡片结束索引（不含） */
  const endIndex = computed(() => Math.min(totalCount.value, endRow.value * columnCount.value))

  /** 上方占位高度：第一行偏移 + 起始行之前的行高总和 */
  const spacerTop = computed(() => {
    if (!enabled.value) return 0
    return topOffset + startRow.value * rowHeight
  })

  /** 下方占位高度：结束行之后的行高总和 */
  const spacerBottom = computed(() => {
    if (!enabled.value) return 0
    return (totalRows.value - endRow.value) * rowHeight
  })

  /**
   * 更新滚动位置和容器高度
   */
  function onScroll() {
    if (!scrollContainer.value) return
    scrollTop.value = scrollContainer.value.scrollTop
    containerHeight.value = scrollContainer.value.clientHeight
  }

  /** 强制重新计算可见范围 */
  function recalculate() {
    onScroll()
  }

  /** requestAnimationFrame ID，用于节流 */
  let rafId: number | null = null

  /**
   * 节流版滚动事件处理
   * 使用 requestAnimationFrame 确保滚动计算与浏览器渲染帧同步
   */
  function onScrollThrottled() {
    if (rafId !== null) return
    rafId = requestAnimationFrame(() => {
      rafId = null
      onScroll()
    })
  }

  onMounted(() => {
    if (scrollContainer.value) {
      scrollContainer.value.addEventListener('scroll', onScrollThrottled, { passive: true })
      onScroll()
    }
  })

  onUnmounted(() => {
    if (scrollContainer.value) {
      scrollContainer.value.removeEventListener('scroll', onScrollThrottled)
    }
    if (rafId !== null) {
      cancelAnimationFrame(rafId)
      rafId = null
    }
  })

  return {
    startIndex,
    endIndex,
    spacerTop,
    spacerBottom,
    enabled,
    totalRows,
    recalculate
  }
}