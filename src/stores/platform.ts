/**
 * 平台信息模块
 *
 * 职责：
 * - 检测当前操作系统类型（Windows/macOS/Linux）
 * - 获取平台功能支持状态（按键模拟、前台窗口检测等）
 * - 提供平台信息的单例访问
 *
 * 注意：initPlatform()使用单例Promise防止重复初始化
 */
import { ref } from 'vue'
import { getPlatformInfo } from '../utils/tauri'
import { logger } from '../utils/logger'
import type { PlatformInfo } from '../types'

/** 平台信息响应式引用 */
const platformInfo = ref<PlatformInfo | null>(null)
/** 平台信息是否已初始化完成 */
const initialized = ref(false)
/** 初始化Promise单例，防止并发初始化 */
let _initPromise: Promise<void> | null = null

/**
 * 初始化平台信息
 *
 * 调用后端get_platform_info命令获取：
 * - 操作系统类型
 * - 会话类型（Linux下x11/wayland）
 * - 按键模拟是否支持
 * - 前台窗口检测是否支持
 *
 * 失败时默认使用Windows配置作为回退
 */
export async function initPlatform() {
  if (_initPromise) return _initPromise
  _initPromise = (async () => {
    try {
      const raw = await getPlatformInfo()
      platformInfo.value = raw as unknown as PlatformInfo
      initialized.value = true
    } catch (e) {
      logger.error('Platform', 'Failed to get platform info, defaulting to Windows', e)
      // 失败回退：默认Windows配置，假设所有功能可用
      platformInfo.value = {
        os: 'windows',
        sessionType: null,
        keypressSupported: true,
        keypressError: null,
        foregroundDetectionSupported: true,
      }
      initialized.value = true
    }
  })()
  return _initPromise
}

/**
 * 使用平台信息的Composable
 * @returns 包含platformInfo和initialized的对象
 */
export function usePlatform() {
  return { platformInfo, initialized }
}
