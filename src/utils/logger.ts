/**
 * 统一日志工具模块
 *
 * 设计原则：
 * - 统一格式化：[HH:MM:SS.mmm][LEVEL][Module] message，便于调试追踪
 * - Error对象自动提取message，避免直接打印Error对象导致控制台显示杂乱
 * - dev 模式日志经 IPC 转发后端（frontend_log 命令）写入统一日志文件，
 *   前后端日志时序合一，便于排查前后端链路异常
 *
 * 日志级别：
 * - DEBUG: 开发调试信息，release模式默认不显示
 * - INFO: 常规运行信息（如初始化完成、操作成功）
 * - WARN: 警告信息（非致命错误，如功能降级、回退处理）
 * - ERROR: 错误信息（功能失败、异常捕获）
 *
 * 级别过滤策略（与后端 Rust 端 log 初始化策略对齐）：
 * - dev 模式（npm run dev）：输出 DEBUG 及以上全部级别，且全部转发后端落盘
 * - prod 模式（npm run build）：仅输出 WARN 及以上级别（DEBUG/INFO 静默），
 *   WARN/ERROR 仍转发后端落盘（release 无终端窗口，落盘便于现场排查）
 * - 使用编译期常量 DEV_MODE 静态守卫，prod 构建时 DEBUG/INFO 分支被 tree-shaking 移除
 *
 * 转发契约（与后端 log_commands::frontend_log 对应）：
 * - fire-and-forget：invoke 不 await、失败静默（浏览器/无后端环境不抛错）
 * - 后端不返回写入成功/异常信息；单次 IPC 微秒级，不阻塞 UI
 */

import { DEV_MODE } from './env'
import { invoke } from '@tauri-apps/api/core'

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
function format(level: LogLevel, module: string, msg: string, ..._args: unknown[]): string {
  return `[${timestamp()}][${level}][${module}] ${msg}`
}

/**
 * 安全处理日志参数
 * 将Error对象转换为message字符串，避免控制台显示完整堆栈
 * @param args 原始参数数组
 * @returns 处理后的安全参数数组
 */
function safeArgs(args: unknown[]): unknown[] {
  return args.map(a => (a instanceof Error ? a.message : a))
}

/**
 * 转发日志到后端统一日志系统（fire-and-forget）
 *
 * - 不 await、不返回值：后端 log_commands::frontend_log 无返回，失败由后端管道
 *   自行降级/红字告警，前端不感知（日志丢失不阻断业务）
 * - 静默失败：Tauri 未就绪 / 纯浏览器环境（无后端 invoke）时 catch 忽略，
 *   不产生未处理 Promise rejection
 *
 * @param level 日志级别（大写字符串，与后端 Level 解析对齐）
 * @param module 前端模块名
 * @param msg 格式化后的消息
 */
function forwardToBackend(level: LogLevel, module: string, msg: string): void {
  invoke('frontend_log', { level, module, msg }).catch(() => {})
}

/**
 * 统一日志导出对象
 * 提供四个级别的日志方法，使用方式：logger.info('ModuleName', 'message', extraData)
 *
 * 级别过滤实现：
 * - debug/info 方法内部使用 `if (DEV_MODE)` 守卫
 * - prod 构建时 DEV_MODE 被静态替换为 false，整个 if 块被 tree-shaking 移除
 * - warn/error 始终输出（dev/prod 均保留）并转发后端落盘
 */
export const logger = {
  /**
   * 输出调试级别日志
   * 仅 dev 模式输出；prod 构建时整个方法体被 tree-shaking 移除
   * @param module 模块名
   * @param msg 日志消息
   * @param args 额外参数
   */
  debug(module: string, msg: string, ...args: unknown[]) {
    if (DEV_MODE) {
      console.debug(format('DEBUG', module, msg), ...safeArgs(args))
      forwardToBackend('DEBUG', module, msg)
    }
  },

  /**
   * 输出信息级别日志
   * 仅 dev 模式输出；prod 构建时整个方法体被 tree-shaking 移除
   * @param module 模块名
   * @param msg 日志消息
   * @param args 额外参数
   */
  info(module: string, msg: string, ...args: unknown[]) {
    if (DEV_MODE) {
      console.info(format('INFO', module, msg), ...safeArgs(args))
      forwardToBackend('INFO', module, msg)
    }
  },

  /**
   * 输出警告级别日志
   * dev/prod 均输出（prod 模式下用户打开 devtools 仍可见，便于排查），并转发后端落盘
   * @param module 模块名
   * @param msg 日志消息
   * @param args 额外参数
   */
  warn(module: string, msg: string, ...args: unknown[]) {
    console.warn(format('WARN', module, msg), ...safeArgs(args))
    forwardToBackend('WARN', module, msg)
  },

  /**
   * 输出错误级别日志
   * dev/prod 均输出（保证异常不丢失），并转发后端落盘
   * @param module 模块名
   * @param msg 日志消息
   * @param args 额外参数（通常为Error对象）
   */
  error(module: string, msg: string, ...args: unknown[]) {
    console.error(format('ERROR', module, msg), ...safeArgs(args))
    forwardToBackend('ERROR', module, msg)
  },
}