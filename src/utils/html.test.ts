import { describe, it, expect } from 'vitest'
import { escapeHtml } from './html'

describe('escapeHtml', () => {
  it('should return normal text unchanged', () => {
    expect(escapeHtml('Hello World')).toBe('Hello World')
  })

  it('should escape ampersand', () => {
    expect(escapeHtml('a & b')).toBe('a &amp; b')
  })

  it('should escape less-than sign', () => {
    expect(escapeHtml('a < b')).toBe('a &lt; b')
  })

  it('should escape greater-than sign', () => {
    expect(escapeHtml('a > b')).toBe('a &gt; b')
  })

  it('should escape double quote', () => {
    expect(escapeHtml('say "hello"')).toBe('say &quot;hello&quot;')
  })

  it('should escape single quote', () => {
    expect(escapeHtml("it's")).toBe('it&#39;s')
  })

  it('should escape all special characters in one string', () => {
    expect(escapeHtml('<script>alert("xss")</script>')).toBe(
      '&lt;script&gt;alert(&quot;xss&quot;)&lt;/script&gt;'
    )
  })

  it('should handle empty string', () => {
    expect(escapeHtml('')).toBe('')
  })

  it('should handle string with no special characters', () => {
    expect(escapeHtml('12345')).toBe('12345')
  })

  it('should handle Unicode characters', () => {
    expect(escapeHtml('你好世界')).toBe('你好世界')
  })

  it('should handle mixed Unicode and special characters', () => {
    expect(escapeHtml('你好 <world>')).toBe('你好 &lt;world&gt;')
  })

  it('should handle multiple ampersands', () => {
    expect(escapeHtml('&&&')).toBe('&amp;&amp;&amp;')
  })
})
