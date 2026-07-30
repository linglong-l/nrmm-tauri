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

  /**
   * 指针按下事件处理
   * 仅响应鼠标左键，排除可交互元素
   * 捕获指针并记录起始位置和滚动状态
   */
  function onPointerDown(e: PointerEvent) {
    // 仅响应鼠标左键（button === 0）
    if (e.button !== 0) return
    const target = e.target as HTMLElement

    // 排除所有可交互元素和Element Plus组件
    // 关键点：.group-item / .expand-btn / .group-avatar 是左侧导航栏树节点的核心容器，
    // 这里的点击必须触发选中/展开，不能被拖拽滚动拦截
    if (target.closest(
      'button, a, input, textarea, select, option, label, ' +
      '[role="button"], [role="combobox"], [role="listbox"], [role="option"], ' +
      '[role="slider"], [role="switch"], [role="checkbox"], [role="radio"], [role="menuitem"], [role="treeitem"], ' +
      '.el-select, .el-select__wrapper, .el-select-dropdown, .el-cascader, .el-date-editor, ' +
      '.el-checkbox, .el-radio, .el-switch, .el-slider, .el-input, .el-input__wrapper, .el-textarea, ' +
      '.el-button, .el-popper, .el-picker-panel, .el-dialog, .el-message-box, .el-dropdown, ' +
      '.group-item, .expand-btn, .group-avatar, .group-tree-node, ' +
      '.mod-card, .allow-context-menu, .allow-text-select, .no-drag-scroll'
    )) return

    currentEl = el.value
    if (!currentEl) return

    isDragging.value = true
    dragStarted = false
    startX = e.clientX
    startY = e.clientY
    startScrollLeft = currentEl.scrollLeft
    startScrollTop = currentEl.scrollTop
    // 捕获指针，确保指针移出元素外仍能接收事件
    try { currentEl.setPointerCapture(e.pointerId) } catch (_) {}
    // 更改光标样式为抓取中
    currentEl.style.cursor = 'grabbing'
    e.preventDefault()
  }

  /**
   * 指针移动事件处理
   * 更新滚动位置，3px阈值判定
   */
  function onPointerMove(e: PointerEvent) {
    if (!isDragging.value || !currentEl) return

    const deltaX = e.clientX - startX
    const deltaY = e.clientY - startY

    // 3px阈值：移动超过3像素才认为是有效拖拽，避免点击时微小抖动被误判
    if (!dragStarted && (Math.abs(deltaX) > 3 || Math.abs(deltaY) > 3)) {
      dragStarted = true
    }

    // 有效拖拽时更新滚动位置（反向滚动：鼠标向下拖，内容向上滚）
    if (dragStarted) {
      currentEl.scrollLeft = startScrollLeft - deltaX
      currentEl.scrollTop = startScrollTop - deltaY
      if (e.cancelable) e.preventDefault()
    }
  }

  /**
   * 指针抬起/取消事件处理
   * 结束拖拽状态，释放指针捕获
   * 若发生了有效拖拽，仅阻断滚动容器内部元素的一次click事件，防止误触
   * 关键点：不对teleport弹层/外部组件做任何拦截（否则Element Plus下拉/对话框等点击失效）
   */
  function onPointerUp(e: PointerEvent) {
    if (!isDragging.value) return
    isDragging.value = false
    const container = currentEl
    if (container) {
      // 释放指针捕获
      try { container.releasePointerCapture(e.pointerId) } catch (_) {}
      // 恢复默认光标
      container.style.cursor = ''
    }
    // 若发生了有效拖拽，阻断"容器内部"接下来的一次click事件
    // 这是因为拖拽结束时的mouseup会触发click，导致误点卡片/按钮
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
      window.addEventListener('click', clickBlocker, true)
      // 100ms后自动清理，防止阻断正常点击
      setTimeout(() => window.removeEventListener('click', clickBlocker, true), 100)
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
    target.addEventListener('pointermove', onPointerMove)
    target.addEventListener('pointerup', onPointerUp)
    // pointercancel处理触摸中断等异常情况
    target.addEventListener('pointercancel', onPointerUp)
  }

  /**
   * 从目标元素移除拖拽滚动事件
   * @param target 要移除事件的DOM元素
   */
  function detach(target: HTMLElement) {
    target.removeEventListener('pointerdown', onPointerDown)
    target.removeEventListener('pointermove', onPointerMove)
    target.removeEventListener('pointerup', onPointerUp)
    target.removeEventListener('pointercancel', onPointerUp)
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
  })

  return { isDragging }
}
