import { ref, onMounted, onUnmounted, watch, type Ref } from 'vue'

export function useDragScroll(el: Ref<HTMLElement | null>) {
  const isDragging = ref(false)
  let startX = 0
  let startY = 0
  let startScrollLeft = 0
  let startScrollTop = 0
  let dragStarted = false
  let currentEl: HTMLElement | null = null

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return
    const target = e.target as HTMLElement
    if (target.closest('button, a, input, [role="button"], .no-drag-scroll')) return

    currentEl = el.value
    if (!currentEl) return

    isDragging.value = true
    dragStarted = false
    startX = e.clientX
    startY = e.clientY
    startScrollLeft = currentEl.scrollLeft
    startScrollTop = currentEl.scrollTop
    currentEl.setPointerCapture(e.pointerId)
    currentEl.style.cursor = 'grabbing'
    e.preventDefault()
  }

  function onPointerMove(e: PointerEvent) {
    if (!isDragging.value || !currentEl) return

    const deltaX = e.clientX - startX
    const deltaY = e.clientY - startY

    if (!dragStarted && (Math.abs(deltaX) > 3 || Math.abs(deltaY) > 3)) {
      dragStarted = true
    }

    if (dragStarted) {
      currentEl.scrollLeft = startScrollLeft - deltaX
      currentEl.scrollTop = startScrollTop - deltaY
      e.preventDefault()
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (!isDragging.value) return
    isDragging.value = false
    if (currentEl) {
      currentEl.releasePointerCapture(e.pointerId)
      currentEl.style.cursor = ''
    }
    if (dragStarted) {
      const clickBlocker = (ev: MouseEvent) => {
        ev.stopPropagation()
        ev.preventDefault()
        window.removeEventListener('click', clickBlocker, true)
      }
      window.addEventListener('click', clickBlocker, true)
      setTimeout(() => window.removeEventListener('click', clickBlocker, true), 100)
    }
    dragStarted = false
    currentEl = null
  }

  function attach(target: HTMLElement) {
    target.addEventListener('pointerdown', onPointerDown)
    target.addEventListener('pointermove', onPointerMove)
    target.addEventListener('pointerup', onPointerUp)
    target.addEventListener('pointercancel', onPointerUp)
  }

  function detach(target: HTMLElement) {
    target.removeEventListener('pointerdown', onPointerDown)
    target.removeEventListener('pointermove', onPointerMove)
    target.removeEventListener('pointerup', onPointerUp)
    target.removeEventListener('pointercancel', onPointerUp)
  }

  let attachedEl: HTMLElement | null = null

  onMounted(() => {
    if (el.value) {
      attach(el.value)
      attachedEl = el.value
    }
  })

  watch(el, (newEl, oldEl) => {
    if (oldEl && attachedEl === oldEl) {
      detach(oldEl)
    }
    if (newEl) {
      attach(newEl)
      attachedEl = newEl
    }
  })

  onUnmounted(() => {
    if (attachedEl) {
      detach(attachedEl)
    }
  })

  return { isDragging }
}
