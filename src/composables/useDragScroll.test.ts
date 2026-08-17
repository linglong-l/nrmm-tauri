/**
 * useDragScroll 单元测试
 *
 * 验证核心行为：
 * - 在普通元素上按下并移动超过阈值 → 更新容器 scrollTop（拖拽生效）
 * - 命中默认排除选择器（button/a/input 等）→ 不触发拖拽
 * - 命中自定义 excludeSelector → 不触发拖拽（P3 新增能力，用于替代 Settings/Keybinds 内联实现）
 */
import { describe, it, expect, afterEach } from 'vitest'
import { defineComponent, ref, h } from 'vue'
import { mount, type VueWrapper } from '@vue/test-utils'
import { useDragScroll } from './useDragScroll'

/** 测试宿主组件：暴露容器 ref 并复用 useDragScroll */
const ScrollHost = defineComponent({
  props: {
    excludeSelector: { type: String, default: '' },
  },
  setup(props) {
    const scrollEl = ref<HTMLElement | null>(null)
    useDragScroll(scrollEl, { excludeSelector: props.excludeSelector })
    return { scrollEl }
  },
  render() {
    return h(
      'div',
      { class: 'scrollbox', ref: 'scrollEl' },
      [
        h('div', { class: 'plain-target' }, 'plain'),
        h('span', { class: 'custom-exclude' }, 'excluded'),
        h('button', { class: 'btn-target' }, 'button'),
      ]
    )
  },
})

/** 派发 PointerEvent 的工具 */
function dispatchPointer(target: EventTarget, type: string, init = {}) {
  target.dispatchEvent(new PointerEvent(type, { bubbles: true, cancelable: true, button: 0, ...init }))
}

/**
 * 触发一次拖拽：在 target 上按下，然后在文档上移动（超过 3px 阈值）
 * @param wrapper 测试组件
 * @param target target 事件源
 * @returns 容器元素
 */
function performDrag(wrapper: VueWrapper, target: Element) {
  const el = wrapper.find('.scrollbox').element
  el.scrollTop = 0
  dispatchPointer(target, 'pointerdown', { clientX: 10, clientY: 10 })
  dispatchPointer(document, 'pointermove', { clientX: 10, clientY: 40 })
  dispatchPointer(document, 'pointerup')
  return el
}

afterEach(() => {
  // 清理可能残留的 document 级监听器
  document.removeEventListener('pointermove', () => {})
})

describe('useDragScroll', () => {
  it('在普通元素上拖拽时更新容器 scrollTop', () => {
    const wrapper = mount(ScrollHost)
    const el = performDrag(wrapper, wrapper.find('.plain-target').element)
    // 向下拖拽 → 内容上滚 → scrollTop 变小（jsdom 不 clamp 负值）；断言已从初始 0 变化
    expect(el.scrollTop).not.toBe(0)
  })

  it('命中默认排除选择器（button）时不触发拖拽', () => {
    const wrapper = mount(ScrollHost)
    const el = performDrag(wrapper, wrapper.find('.btn-target').element)
    expect(el.scrollTop).toBe(0)
  })

  it('命中自定义 excludeSelector 时不触发拖拽（用于替代内联实现的自定义排除）', () => {
    const wrapper = mount(ScrollHost, {
      props: { excludeSelector: '.custom-exclude' },
    })
    const el = performDrag(wrapper, wrapper.find('.custom-exclude').element)
    expect(el.scrollTop).toBe(0)
  })

  it('未配置 excludeSelector 时，非默认排除元素可正常拖拽', () => {
    const wrapper = mount(ScrollHost)
    const el = performDrag(wrapper, wrapper.find('.custom-exclude').element)
    expect(el.scrollTop).not.toBe(0)
  })
})