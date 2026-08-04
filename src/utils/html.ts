/**
 * HTML 工具函数模块
 *
 * 提供 HTML 特殊字符转义等安全工具函数，供前端组件统一使用
 */

/**
 * HTML 特殊字符转义
 *
 * 将字符串中的 HTML 特殊字符（& < > " '）转换为对应的 HTML 实体，
 * 防止 XSS 攻击。用于在将用户内容插入 HTML 上下文前的安全处理。
 *
 * @param s 待转义的原始字符串
 * @returns 转义后的安全字符串，特殊字符已替换为 HTML 实体
 *
 * @example
 * escapeHtml('<script>alert("xss")</script>')
 * // => '&lt;script&gt;alert(&quot;xss&quot;)&lt;/script&gt;'
 */
export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
  }[c] as string))
}
