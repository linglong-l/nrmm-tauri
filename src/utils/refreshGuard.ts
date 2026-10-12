/**
 * 刷新键判定（WebView 整页 reload 内存优化）
 *
 * WebView2 聚焦时按 F5 / Ctrl+R 会触发浏览器原生整页 reload——旧文档 JS 堆与解码位图
 * 惰性回收叠加新文档全量重建，实测内存峰值破 1000MB。因此这些键一律拦截，
 * 「刷新」只走数据层、永不重载页面：
 * - F5、Ctrl/Meta+R（不含 Shift）→ 轻量重扫 `loadMods`
 * - Ctrl/Meta+Shift+R → 全量重扫 `refresh`
 *
 * 全局热键侧不含 F5（后端不注册、也不 emit `hotkey-refresh`），此处只管窗口内绑定。
 */

/**
 * 判定是否应用内「轻量重扫」快捷键（F5，或 Ctrl/Meta+R 且不含 Shift）。
 *
 * 窗口内绑定：拦截整页 reload 后改为数据层轻量重扫（loadMods：
 * 后端缓存有效则零扫描，失效则轻量扫描）。
 * 输入框/文本域等可编辑场景由调用方（main.ts）过滤。
 */
export function isLightRescanKey(e: KeyboardEvent): boolean {
  const key = e.key?.toLowerCase()
  if (key === 'f5') return true
  return key === 'r' && (e.ctrlKey || e.metaKey) && !e.shiftKey
}

/**
 * 判定是否应用内「全量重扫」快捷键（Ctrl/Meta+Shift+R，窗口内绑定）。
 *
 * 语义：强制后端全量轻扫并覆盖缓存与前端数据（refresh），与浏览器强刷逃生门
 * 解耦——Ctrl+Shift+R 不再放行整页 reload，改走数据层全量重扫。
 */
export function isFullRescanKey(e: KeyboardEvent): boolean {
  const key = e.key?.toLowerCase()
  return key === 'r' && (e.ctrlKey || e.metaKey) && !!e.shiftKey
}