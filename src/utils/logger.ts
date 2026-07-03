/**
 * 前端统一日志工具模块
 *
 * 提供统一的日志输出接口，按级别区分 debug / info / warn / error。
 * 在生产环境下静默 debug 级别日志，避免向控制台输出过多调试信息；
 * 其他级别在所有环境下正常输出，便于排查问题。
 */

/**
 * 是否为生产环境。
 * 基于 Vite 注入的 `import.meta.env.PROD` 静态判断，
 * 用于在构建时决定是否静默 debug 级别日志。
 */
const isProd = import.meta.env.PROD;

/**
 * 前端统一日志单例对象。
 *
 * - debug：开发环境输出到 `console.debug` 并带 `[debug]` 前缀，生产环境静默；
 * - info：输出到 `console.info`；
 * - warn：输出到 `console.warn`；
 * - error：所有环境均输出到 `console.error`。
 */
export const logger = {
  /**
   * 输出 debug 级别日志。
   * 仅在开发环境（非生产环境）下输出到 `console.debug`，并附加 `[debug]` 前缀；
   * 生产环境下不产生任何输出。
   * @param args 日志参数，可变参数
   */
  debug(...args: unknown[]) {
    if (!isProd) {
      console.debug('[debug]', ...args);
    }
  },

  /**
   * 输出 info 级别日志。
   * @param args 日志参数，可变参数
   */
  info(...args: unknown[]) {
    console.info(...args);
  },

  /**
   * 输出 warn 级别日志。
   * @param args 日志参数，可变参数
   */
  warn(...args: unknown[]) {
    console.warn(...args);
  },

  /**
   * 输出 error 级别日志。
   * 在所有环境下均输出到 `console.error`，确保异常信息不丢失。
   * @param args 日志参数，可变参数
   */
  error(...args: unknown[]) {
    console.error(...args);
  }
};
