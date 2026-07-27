/**
 * 统一日志工具
 *
 * 仅输出至浏览器控制台，不包含任何网络传输逻辑，不收集用户信息。
 * 格式化：[HH:MM:SS.mmm][LEVEL][Module] message
 */

type LogLevel = 'DEBUG' | 'INFO' | 'WARN' | 'ERROR'

function timestamp(): string {
  const d = new Date()
  const hh = String(d.getHours()).padStart(2, '0')
  const mm = String(d.getMinutes()).padStart(2, '0')
  const ss = String(d.getSeconds()).padStart(2, '0')
  const ms = String(d.getMilliseconds()).padStart(3, '0')
  return `${hh}:${mm}:${ss}.${ms}`
}

function format(level: LogLevel, module: string, msg: string, ..._args: any[]): string {
  return `[${timestamp()}][${level}][${module}] ${msg}`
}

function safeArgs(args: any[]): any[] {
  return args.map(a => (a instanceof Error ? a.message : a))
}

export const logger = {
  debug(module: string, msg: string, ...args: any[]) {
    console.debug(format('DEBUG', module, msg), ...safeArgs(args))
  },

  info(module: string, msg: string, ...args: any[]) {
    console.info(format('INFO', module, msg), ...safeArgs(args))
  },

  warn(module: string, msg: string, ...args: any[]) {
    console.warn(format('WARN', module, msg), ...safeArgs(args))
  },

  error(module: string, msg: string, ...args: any[]) {
    console.error(format('ERROR', module, msg), ...safeArgs(args))
  },
}
