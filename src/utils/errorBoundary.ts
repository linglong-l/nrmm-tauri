/**
 * 全局错误处理器模块。
 *
 * 提供三层前端错误兜底：
 * 1. Vue 应用级错误处理器（在 main.ts 中通过 app.config.errorHandler 注册）
 * 2. 未处理的 Promise 拒绝（window.onunhandledrejection）
 * 3. 全局 JS 运行时错误（window.onerror）
 *
 * 所有错误均记录到日志，并尝试调用后端 `restart_application` 命令重启应用。
 * 重启失败时静默忽略（进程已处于不可恢复状态）。
 */
import type { App as VueApp } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { createLogger } from './logger';

const log = createLogger('ErrorBoundary');

/**
 * 尝试重启应用（调用后端命令）。
 * 使用 setTimeout 确保错误日志先写入再重启。
 */
function restartApp(): void {
  setTimeout(() => {
    invoke('restart_application').catch(() => {
      // 重启命令失败，应用已处于不可恢复状态，静默忽略
    });
  }, 100);
}

/**
 * 设置 Vue 应用级错误处理器。
 *
 * 捕获 Vue 组件渲染、侦听器、生命周期钩子中的错误。
 * 当前端发生未知异常时，记录日志并尝试重启应用。
 *
 * @param app Vue 应用实例，在 main.ts 中创建
 */
export function setupVueErrorHandler(app: VueApp): void {
  app.config.errorHandler = (err: unknown, _instance: unknown, info: string) => {
    log.error('[Vue error]', err, { trigger: info });
    log.error('[ErrorBoundary] Will restart application due to Vue error');
    restartApp();
  };
}

/**
 * 设置全局运行时错误处理器。
 *
 * 覆盖：
 * - `window.onerror`：同步 JS 错误
 * - `window.addEventListener('unhandledrejection')`：未处理的 Promise 拒绝
 *
 * 在 App.vue 的 onMounted 中调用。
 */
export function setupGlobalErrorHandlers(): void {
  // 1. 未处理的 Promise 拒绝
  window.addEventListener('unhandledrejection', (event: PromiseRejectionEvent) => {
    log.error('[Unhandled rejection]', event.reason);
    log.error('[ErrorBoundary] Will restart application due to unhandled rejection');
    restartApp();
  });

  // 2. 全局 JS 运行时错误
  window.onerror = (_message: string | Event, _source?: string, _lineno?: number, _colno?: number, error?: Error | null) => {
    log.error('[Global error]', error);
    log.error('[ErrorBoundary] Will restart application due to global error');
    restartApp();
  };
}