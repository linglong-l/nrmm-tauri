/**
 * 刷新拦截判定（WebView F5 内存优化，2026-08-21）
 *
 * 应用注册了全局热键 F5（后端 emit hotkey-refresh → modsStore.refresh()），
 * 但 WebView2 聚焦时按 F5/Ctrl+R 会触发浏览器原生整页 reload——旧文档 JS 堆
 * 与解码位图惰性回收 + 新文档全量重建，实测内存峰值破 1000MB。
 * 拦截这两个按键，让"刷新"只走数据层 refresh（不重载页面）。
 */

/**
 * 判定键盘事件是否为需要拦截的浏览器刷新键（F5 / Ctrl+R / Meta+R）
 *
 * 输入框/文本域等可编辑场景与 Ctrl+Shift+R 强刷逃生门由调用方（main.ts）
 * 自行过滤，此处只做纯按键组合判定，保持函数单一职责、便于单测。
 *
 * @param e 键盘事件
 * @returns true 表示应拦截（阻止整页 reload），false 表示放行
 */
export function isBrowserRefreshKey(e: KeyboardEvent): boolean {
  const key = e.key?.toLowerCase()
  if (key === 'f5') return true
  // Ctrl+R（含 macOS Cmd+R：metaKey）
  if (key === 'r' && (e.ctrlKey || e.metaKey)) return true
  return false
}