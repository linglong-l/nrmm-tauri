/**
 * 全局错误处理器模块。
 *
 * 提供三层前端错误兜底，所有错误仅记录日志，不触发重启：
 * 1. Vue 应用级错误处理器
 * 2. 未处理的 Promise 拒绝
 * 3. 全局 JS 运行时错误
 */
import type { App as VueApp } from 'vue';
import { createLogger } from './logger';

const log = createLogger('ErrorBoundary');

/**
 * 设置 Vue 应用级错误处理器。
 *
 * 捕获 Vue 组件渲染、侦听器、生命周期钩子中的错误。
 * 仅记录日志，不触发重启。
 *
 * @param app Vue 应用实例
 */
export function setupVueErrorHandler(app: VueApp): void {
  app.config.errorHandler = (err: unknown, _instance: unknown, info: string) => {
    log.error('[Vue error]', err, { trigger: info });
  };
}

/**
 * 设置全局运行时错误处理器。
 *
 * 覆盖：
 * - `window.addEventListener('unhandledrejection')`：未处理的 Promise 拒绝
 * - `window.onerror`：同步 JS 错误
 *
 * 在 App.vue 的 onMounted 中调用。
 */
export function setupGlobalErrorHandlers(): void {
  // 1. 未处理的 Promise 拒绝
  window.addEventListener('unhandledrejection', (event: PromiseRejectionEvent) => {
    log.error('[Unhandled rejection]', event.reason);
  });

  // 2. 全局 JS 运行时错误
  window.onerror = (_message: string | Event, _source?: string, _lineno?: number, _colno?: number, error?: Error | null) => {
    log.error('[Global error]', error);
  };
}
