import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HighlightText from './HighlightText.vue'

describe('HighlightText', () => {
  it('should render plain text without spans', () => {
    const wrapper = mount(HighlightText, {
      props: { text: 'Hello World' },
    })
    expect(wrapper.text()).toBe('Hello World')
    expect(wrapper.find('mark').exists()).toBe(false)
  })

  it('should render plain text with empty spans', () => {
    const wrapper = mount(HighlightText, {
      props: { text: 'Hello World', spans: [] },
    })
    expect(wrapper.text()).toBe('Hello World')
    expect(wrapper.find('mark').exists()).toBe(false)
  })

  it('should highlight matched spans with mark tag', () => {
    const wrapper = mount(HighlightText, {
      props: { text: 'Hello World', spans: [[0, 5]] },
    })
    const marks = wrapper.findAll('mark')
    expect(marks).toHaveLength(1)
    expect(marks[0].text()).toBe('Hello')
    expect(wrapper.text()).toBe('Hello World')
  })

  it('should highlight multiple spans', () => {
    const wrapper = mount(HighlightText, {
      props: { text: 'Hello World', spans: [[0, 5], [6, 11]] },
    })
    const marks = wrapper.findAll('mark')
    expect(marks).toHaveLength(2)
    expect(marks[0].text()).toBe('Hello')
    expect(marks[1].text()).toBe('World')
  })

  it('should handle span in the middle of text', () => {
    const wrapper = mount(HighlightText, {
      props: { text: 'Hello World', spans: [[6, 11]] },
    })
    const marks = wrapper.findAll('mark')
    expect(marks).toHaveLength(1)
    expect(marks[0].text()).toBe('World')
    expect(wrapper.text()).toBe('Hello World')
  })

  it('should handle empty text', () => {
    const wrapper = mount(HighlightText, {
      props: { text: '' },
    })
    expect(wrapper.text()).toBe('')
    expect(wrapper.find('mark').exists()).toBe(false)
  })

  it('should apply highlight-mark class to mark elements', () => {
    const wrapper = mount(HighlightText, {
      props: { text: 'Hello', spans: [[0, 5]] },
    })
    expect(wrapper.find('mark').classes()).toContain('highlight-mark')
  })

  it('should not render HTML tags from text content (XSS protection)', () => {
    const wrapper = mount(HighlightText, {
      props: { text: '<script>alert(1)</script>', spans: [] },
    })
    expect(wrapper.find('script').exists()).toBe(false)
    expect(wrapper.text()).toBe('<script>alert(1)</script>')
  })
})
