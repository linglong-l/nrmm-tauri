/**
 * 统一日志工具模块
 *
 * 设计原则：
 * - 仅输出至浏览器控制台，不包含任何网络传输逻辑，不收集用户隐私信息
 * - 统一格式化：[HH:MM:SS.mmm][LEVEL][Module] message，便于调试追踪
 * - Error对象自动提取message，避免直接打印Error对象导致控制台显示杂乱
 *
 * 日志级别：
 * - DEBUG: 开发调试信息，release模式默认不显示
 * - INFO: 常规运行信息（如初始化完成、操作成功）
 * - WARN: 警告信息（非致命错误，如功能降级、回退处理）
 * - ERROR: 错误信息（功能失败、异常捕获）
 */

/** 日志级别类型定义 */
type LogLevel = 'DEBUG' | 'INFO' | 'WARN' | 'ERROR'

/**
 * 生成带毫秒精度的时间戳
 * @returns 格式化时间字符串 HH:MM:SS.mmm
 */
function timestamp(): string {
  const d = new Date()
  const hh = String(d.getHours()).padStart(2, '0')
  const mm = String(d.getMinutes()).padStart(2, '0')
  const ss = String(d.getSeconds()).padStart(2, '0')
  const ms = String(d.getMilliseconds()).padStart(3, '0')
  return `${hh}:${mm}:${ss}.${ms}`
}

/**
 * 格式化日志消息
 * @param level 日志级别
 * @param module 模块名称（如'ModsStore'、'App'）
 * @param msg 日志消息内容
 * @param _args 额外参数（当前未使用，预留扩展）
 * @returns 格式化后的日志字符串
 */
function format(level: LogLevel, module: string, msg: string, ..._args: any[]): string {
  return `[${timestamp()}][${level}][${module}] ${msg}`
}

/**
 * 安全处理日志参数
 * 将Error对象转换为message字符串，避免控制台显示完整堆栈
 * @param args 原始参数数组
 * @returns 处理后的安全参数数组
 */
function safeArgs(args: any[]): any[] {
  return args.map(a => (a instanceof Error ? a.message : a))
}

/**
 * 统一日志导出对象
 * 提供四个级别的日志方法，使用方式：logger.info('ModuleName', 'message', extraData)
 */
export const logger = {
  /**
   * 输出调试级别日志
   * @param module 模块名
   * @param msg 日志消息
   * @param args 额外参数
   */
  debug(module: string, msg: string, ...args: any[]) {
    console.debug(format('DEBUG', module, msg), ...safeArgs(args))
  },

  /**
   * 输出信息级别日志
   * @param module 模块名
   * @param msg 日志消息
   * @param args 额外参数
   */
  info(module: string, msg: string, ...args: any[]) {
    console.info(format('INFO', module, msg), ...safeArgs(args))
  },

  /**
   * 输出警告级别日志
   * @param module 模块名
   * @param msg 日志消息
   * @param args 额外参数
   */
  warn(module: string, msg: string, ...args: any[]) {
    console.warn(format('WARN', module, msg), ...safeArgs(args))
  },

  /**
   * 输出错误级别日志
   * @param module 模块名
   * @param msg 日志消息
   * @param args 额外参数（通常为Error对象）
   */
  error(module: string, msg: string, ...args: any[]) {
    console.error(format('ERROR', module, msg), ...safeArgs(args))
  },
}
