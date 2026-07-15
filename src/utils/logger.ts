/**
 * 前端统一日志工具模块
 *
 * 提供统一的日志输出接口，按级别区分 debug / info / warn / error。
 * - debug：开发环境输出，记录关键操作流程和变量状态，生产环境静默
 * - warn：输出警告原因和潜在影响
 * - error：包含完整错误堆栈、触发条件、环境参数及解决方案建议
 * - info：通用信息输出
 *
 * 使用 `createLogger(module)` 创建模块级 logger 实例，自动附加模块名和时间戳。
 */

const isProd = import.meta.env.PROD;

/** 格式化时间戳为 HH:MM:SS.ms */
function formatTimestamp(): string {
  const now = new Date();
  return `${String(now.getHours()).padStart(2, '0')}:${String(now.getMinutes()).padStart(2, '0')}:${String(now.getSeconds()).padStart(2, '0')}.${String(now.getMilliseconds()).padStart(3, '0')}`;
}

/** 安全序列化对象，避免循环引用和异常 */
function safeStringify(obj: unknown): string {
  try {
    return JSON.stringify(obj, null, 0);
  } catch {
    return String(obj);
  }
}

export interface LoggerErrorContext {
  /** 触发条件 */
  trigger?: string;
  /** 环境参数 */
  env?: Record<string, unknown>;
  /** 建议的解决方案 */
  suggestion?: string;
}

export interface LoggerWarnContext {
  /** 警告原因 */
  reason: string;
  /** 潜在影响 */
  impact?: string;
}

export interface LoggerInstance {
  debug(message: string, data?: Record<string, unknown>): void;
  info(message: string, data?: Record<string, unknown>): void;
  warn(message: string, context: LoggerWarnContext): void;
  error(message: string, error?: Error | unknown, context?: LoggerErrorContext): void;
}

/**
 * 创建模块级 logger 实例。
 *
 * 使用示例：
 * ```ts
 * const log = createLogger('HashConflict');
 * log.debug('Check started', { game: 'WW', modCount: 42 });
 * log.warn('Debounce cancelled', { reason: 'unmounted', impact: 'No further checks' });
 * log.error('Check failed', err, { trigger: 'GAME_SWITCHED', env: { game: 'WW' }, suggestion: 'Retry' });
 * ```
 */
export function createLogger(module: string): LoggerInstance {
  return {
    debug(message: string, data?: Record<string, unknown>) {
      if (!isProd) {
        const ts = formatTimestamp();
        const dataStr = data ? ` ${safeStringify(data)}` : '';
        console.debug(`[${ts}][${module}][DEBUG] ${message}${dataStr}`);
      }
    },

    info(message: string, data?: Record<string, unknown>) {
      const ts = formatTimestamp();
      const dataStr = data ? ` ${safeStringify(data)}` : '';
      console.info(`[${ts}][${module}][INFO] ${message}${dataStr}`);
    },

    warn(message: string, context: LoggerWarnContext) {
      const ts = formatTimestamp();
      const parts = [`[${ts}][${module}][WARN] ${message}`];
      parts.push(`  Reason: ${context.reason}`);
      if (context.impact) {
        parts.push(`  Impact: ${context.impact}`);
      }
      console.warn(parts.join('\n'));
    },

    error(message: string, error?: Error | unknown, context?: LoggerErrorContext) {
      const ts = formatTimestamp();
      const parts = [`[${ts}][${module}][ERROR] ${message}`];

      if (context?.trigger) {
        parts.push(`  Trigger: ${context.trigger}`);
      }
      if (context?.env) {
        parts.push(`  Env: ${safeStringify(context.env)}`);
      }

      if (error instanceof Error) {
        parts.push(`  Error: ${error.message}`);
        if (error.stack) {
          parts.push(`  Stack:\n${error.stack}`);
        }
      } else if (error !== undefined) {
        parts.push(`  Error: ${safeStringify(error)}`);
      }

      if (context?.suggestion) {
        parts.push(`  Suggestion: ${context.suggestion}`);
      }

      console.error(parts.join('\n'));
    }
  };
}

/**
 * 默认 logger 实例（无模块名）。
 * 用于向后兼容的简单场景。
 */
export const logger = createLogger('App');