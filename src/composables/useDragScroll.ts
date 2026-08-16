/**
 * 鼠标拖拽滚动 Composable
 *
 * 功能：为指定元素添加鼠标拖拽滚动功能，类似触摸设备的滚动体验
 *
 * 使用方法：
 * ```vue
 * <script setup>
 * const scrollRef = ref<HTMLElement | null>(null)
 * const { isDragging } = useDragScroll(scrollRef)
 * </script>
 * <template>
 *   <div ref="scrollRef" style="overflow: auto; height: 400px;">
 *     <!-- 可滚动内容 -->
 *   </div>
 * </template>
 * ```
 *
 * 设计要点：
 * - 3px移动阈值：鼠标按下后移动超过3像素才判定为拖拽，避免干扰正常点击
 * - 点击阻断：拖拽结束后仅阻断滚动容器内部元素的一次click事件，防止误触；
 *   不对 teleport 弹层 / 外部组件的 click 做任何拦截，避免吞掉Element Plus等组件事件
 * - 交互元素排除：按钮、链接、输入框、表单控件、树节点等可交互元素上不触发拖拽
 * - 元素响应式：支持ref指向的DOM元素变化时自动重新绑定事件
 * - Pointer API：使用Pointer Events统一处理鼠标/触摸/笔输入
 * - 自动清理：组件卸载时自动移除事件监听器
 *
 * @param el 要启用拖拽滚动的DOM元素Ref
 * @returns { isDragging } 是否正在拖拽的响应式状态
 */
import { ref, onMounted, onUnmounted, watch, type Ref } from 'vue'

export function useDragScroll(el: Ref<HTMLElement | null>) {
  /** 是否正在拖拽中（响应式状态，可用于UI反馈） */
  const isDragging = ref(false)
  /** 拖拽起始X坐标 */
  let startX = 0
  /** 拖拽起始Y坐标 */
  let startY = 0
  /** 拖拽起始时的水平滚动位置 */
  let startScrollLeft = 0
  /** 拖拽起始时的垂直滚动位置 */
  let startScrollTop = 0
  /** 是否已超过阈值判定为有效拖拽（用于区分点击和拖拽） */
  let dragStarted = false
  /** 当前绑定事件的DOM元素引用 */
  let currentEl: HTMLElement | null = null
  /** clickBlocker 的 setTimeout 清理ID，用于组件卸载时提前清理 */
  let cleanupTimer: ReturnType<typeof setTimeout> | null = null
  /** 当前注册的 clickBlocker 引用，用于组件卸载时移除 window 监听器 */
  let currentClickBlocker: ((ev: MouseEvent) => void) | null = null

  /**
   * 指针按下事件处理
   * 仅响应鼠标左键，排除可交互元素
   * 记录起始位置，不捕获指针（避免干扰原生点击事件）
   * 采用"放开鼠标选择"策略：移动超过阈值才启动拖拽，未移动则放行点击
   */
  function onPointerDown(e: PointerEvent) {
    // 仅响应鼠标左键（button === 0）
    if (e.button !== 0) return
    const target = e.target as HTMLElement

    // 排除所有可交互元素和Element Plus组件
    // 注意：不排除 .group-item / .group-avatar / .group-name
    // 这些元素覆盖左侧导航栏全部区域，采用"放开鼠标选择"策略区分点击和拖拽
    if (target.closest(
      'button, a, input, textarea, select, option, label, ' +
      '[role="button"], [role="combobox"], [role="listbox"], [role="option"], ' +
      '[role="slider"], [role="switch"], [role="checkbox"], [role="radio"], [role="menuitem"], [role="treeitem"], ' +
      '.el-select, .el-select__wrapper, .el-select-dropdown, .el-cascader, .el-date-editor, ' +
      '.el-checkbox, .el-radio, .el-switch, .el-slider, .el-input, .el-input__wrapper, .el-textarea, ' +
      '.el-button, .el-popper, .el-picker-panel, .el-dialog, .el-message-box, .el-dropdown, ' +
      '.allow-context-menu, .allow-text-select, .no-drag-scroll'
    )) return

    currentEl = el.value
    if (!currentEl) return

    isDragging.value = true
    dragStarted = false
    startX = e.clientX
    startY = e.clientY
    startScrollLeft = currentEl.scrollLeft
    startScrollTop = currentEl.scrollTop
    // 不捕获指针，避免干扰点击事件
    // 使用 document 级监听器确保指针移出元素仍能接收事件
    document.addEventListener('pointermove', onDocPointerMove)
    document.addEventListener('pointerup', onDocPointerUp)
    document.addEventListener('pointercancel', onDocPointerUp)
  }

  /** 文档级指针移动事件处理 */
  function onDocPointerMove(e: PointerEvent) {
    if (!isDragging.value || !currentEl) return

    const deltaX = e.clientX - startX
    const deltaY = e.clientY - startY

    // 3px阈值：移动超过3像素才认为是有效拖拽，避免点击时微小抖动被误判
    if (!dragStarted && (Math.abs(deltaX) > 3 || Math.abs(deltaY) > 3)) {
      dragStarted = true
      // 超过阈值时立即更改光标
      currentEl.style.cursor = 'grabbing'
    }

    // 有效拖拽时更新滚动位置（反向滚动：鼠标向下拖，内容向上滚）
    if (dragStarted) {
      currentEl.scrollLeft = startScrollLeft - deltaX
      currentEl.scrollTop = startScrollTop - deltaY
      if (e.cancelable) e.preventDefault()
    }
  }

  /**
   * 文档级指针抬起/取消事件处理
   * 结束拖拽状态，清理文档级监听器
   * 若发生了有效拖拽，阻断滚动容器内部的一次click事件
   * 若未发生拖拽（纯点击），放行click事件，让分组选择等逻辑正常执行
   */
  function onDocPointerUp(_e: PointerEvent) {
    // 清理文档级监听器
    document.removeEventListener('pointermove', onDocPointerMove)
    document.removeEventListener('pointerup', onDocPointerUp)
    document.removeEventListener('pointercancel', onDocPointerUp)

    if (!isDragging.value) return
    isDragging.value = false
    const container = currentEl
    if (container) {
      // 恢复默认光标
      container.style.cursor = ''
    }
    // 若发生了有效拖拽，阻断"容器内部"接下来的一次click事件
    // 未拖拽（纯点击）时放行，让分组选择等逻辑正常执行
    if (dragStarted && container) {
      const containerRef = container
      /**
       * 点击阻断器（只对当前滚动容器内部的真实点击生效）
       * 使用capture: true确保在目标元素处理前拦截
       * 对于不在container内的点击（例如 teleport 到<body>下的el-popper弹层）一律放行
       */
      const clickBlocker = (ev: MouseEvent) => {
        const t = ev.target as HTMLElement
        if (
          containerRef.contains(t) &&
          !t.closest('.el-popper, .el-select-dropdown, .el-dialog, .el-message-box, .el-dropdown-menu')
        ) {
          ev.stopPropagation()
          ev.preventDefault()
        }
        window.removeEventListener('click', clickBlocker, true)
      }
      currentClickBlocker = clickBlocker
      window.addEventListener('click', clickBlocker, true)
      // 100ms后自动清理，防止阻断正常点击
      cleanupTimer = setTimeout(() => {
        window.removeEventListener('click', clickBlocker, true)
        cleanupTimer = null
        currentClickBlocker = null
      }, 100)
    }
    dragStarted = false
    currentEl = null
  }

  /**
   * 为目标元素绑定拖拽滚动事件
   * @param target 要绑定事件的DOM元素
   */
  function attach(target: HTMLElement) {
    target.addEventListener('pointerdown', onPointerDown)
    // pointermove/pointerup/pointercancel 使用 document 级监听器
  }

  /**
   * 从目标元素移除拖拽滚动事件
   * @param target 要移除事件的DOM元素
   */
  function detach(target: HTMLElement) {
    target.removeEventListener('pointerdown', onPointerDown)
    // 清理可能残留的文档级监听器
    document.removeEventListener('pointermove', onDocPointerMove)
    document.removeEventListener('pointerup', onDocPointerUp)
    document.removeEventListener('pointercancel', onDocPointerUp)
  }

  /** 记录已绑定事件的元素，用于watch和unmount时清理 */
  let attachedEl: HTMLElement | null = null

  // 组件挂载时绑定事件
  onMounted(() => {
    if (el.value) {
      attach(el.value)
      attachedEl = el.value
    }
  })

  // 监听el引用变化：元素变化时重新绑定事件（如v-if切换）
  watch(el, (newEl, oldEl) => {
    if (oldEl && attachedEl === oldEl) {
      detach(oldEl)
    }
    if (newEl) {
      attach(newEl)
      attachedEl = newEl
    }
  })

  // 组件卸载时清理事件监听器，防止内存泄漏
  onUnmounted(() => {
    if (attachedEl) {
      detach(attachedEl)
    }
    if (cleanupTimer !== null) {
      clearTimeout(cleanupTimer)
      cleanupTimer = null
    }
    if (currentClickBlocker) {
      window.removeEventListener('click', currentClickBlocker, true)
      currentClickBlocker = null
    }
  })

  return { isDragging }
}
