/**
 * usePlatform.ts
 *
 * 平台环境信息 composable，封装后端 get_platform_info 命令调用结果。
 *
 * 核心用途：
 * - 检测当前是否运行在 Wayland/WSLg 等透明窗口不兼容环境；
 * - 为 App.vue 提供 `supportsTransparency` 标志，控制样式降级
 *   （强制不透明背景、禁用 backdrop-filter 毛玻璃、移除圆角）。
 *
 * 设计要点：
 * - 模块级单例缓存：首次调用后缓存结果，后续组件引用时直接复用，
 *   避免重复 IPC 调用；
 * - 容错：IPC 调用失败时回退到安全默认值（Windows/macOS 默认支持透明，
 *   Linux 默认保守禁用透明，确保最糟糕情况只是视觉降级而非白屏）。
 */
import { ref, computed } from 'vue';
import { invokeGetPlatformInfo } from '../utils/invoke';
import { createLogger } from '../utils/logger';
import type { PlatformInfo } from '../types';

const log = createLogger('usePlatform');

/** 模块级单例：Promise 确保整个应用生命周期只调用一次后端 */
let platformPromise: Promise<PlatformInfo> | null = null;

/** 模块级响应式状态 */
const platformInfo = ref<PlatformInfo | null>(null);

/** 是否正在加载平台信息 */
const loading = ref(true);

/**
 * 根据前端 navigator 信息做快速预判（作为 IPC 失败时的兜底）。
 * 注意：前端无法区分 Wayland/X11（WebView 不暴露该信息），
 *      所以 Linux 环境保守回退为"不支持透明"。
 */
function fallbackDetect(): PlatformInfo {
  const ua = navigator.userAgent.toLowerCase();
  const navPlatform = (navigator.platform || '').toLowerCase();

  let os: PlatformInfo['os'] = 'unknown';
  if (navPlatform.includes('mac') || ua.includes('mac')) os = 'macos';
  else if (navPlatform.includes('win') || ua.includes('win')) os = 'windows';
  else if (navPlatform.includes('linux') || ua.includes('linux')) os = 'linux';

  return {
    os,
    desktopSession: '',
    isWayland: false,
    isX11: false,
    isWslg: false,
    transparencySupported: os !== 'linux',
  };
}

/**
 * 初始化平台信息（应用启动时调用一次）。
 * 多次调用安全，内部只发起一次 IPC 请求。
 */
export async function initPlatformInfo(): Promise<PlatformInfo> {
  if (platformInfo.value) return platformInfo.value;
  if (platformPromise) return platformPromise;

  loading.value = true;
  platformPromise = (async () => {
    try {
      const result = await invokeGetPlatformInfo();
      if (result.ok) {
        platformInfo.value = result.data;
        if (!result.data.transparencySupported) {
          log.warn('Transparency effects disabled due to platform compatibility', {
            reason: `os=${result.data.os}, session=${result.data.desktopSession || 'n/a'}, wslg=${result.data.isWslg}`,
            impact: 'Window will use opaque background without backdrop-blur or rounded corners',
          });
        } else {
          log.debug(`Platform info loaded: os=${result.data.os}, transparency=supported`);
        }
        return result.data;
      }
      throw new Error(result.error);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      log.warn('Failed to get platform info from backend', {
        reason: msg,
        impact: 'Using frontend fallback detection; Linux environments will default to opaque background',
      });
      const fallback = fallbackDetect();
      platformInfo.value = fallback;
      return fallback;
    } finally {
      loading.value = false;
    }
  })();

  return platformPromise;
}

/**
 * 使用平台信息 composable。
 *
 * 返回响应式的平台信息，组件可通过 computed 或 watch 响应变化。
 * 若平台信息尚未加载，返回默认值（默认支持透明，保证 Windows/macOS 首帧正常渲染）。
 *
 * @example
 * ```ts
 * import { usePlatform } from '@/composables';
 * const { supportsTransparency, isWayland } = usePlatform();
 * ```
 */
export function usePlatform() {
  // 若尚未初始化，触发一次（异步，不阻塞）
  if (!platformPromise && !platformInfo.value) {
    void initPlatformInfo();
  }

  /** 当前环境是否支持透明窗口+毛玻璃（响应式 computed） */
  const supportsTransparency = computed<boolean>(
    () => platformInfo.value?.transparencySupported ?? true
  );

  /** 是否为 Wayland 会话（响应式 computed） */
  const isWayland = computed<boolean>(
    () => platformInfo.value?.isWayland ?? false
  );

  /** 是否为 WSLg 环境（响应式 computed） */
  const isWslg = computed<boolean>(
    () => platformInfo.value?.isWslg ?? false
  );

  /** 是否为 X11 会话（响应式 computed） */
  const isX11 = computed<boolean>(
    () => platformInfo.value?.isX11 ?? false
  );

  /** 当前操作系统（响应式 computed） */
  const os = computed<PlatformInfo['os']>(
    () => platformInfo.value?.os ?? 'unknown'
  );

  /** Linux 桌面会话类型（响应式 computed） */
  const desktopSession = computed<string>(
    () => platformInfo.value?.desktopSession ?? ''
  );

  return {
    /** 平台信息（加载完成后非 null） */
    platform: platformInfo,
    /** 是否仍在加载平台信息 */
    loading,
    /** 当前环境是否支持透明窗口+毛玻璃。Wayland/WSLg 返回 false，前端应使用不透明背景降级 */
    supportsTransparency,
    /** 便捷标志：是否为 Wayland 会话 */
    isWayland,
    /** 便捷标志：是否为 WSLg 环境 */
    isWslg,
    /** 便捷标志：是否为 X11 会话 */
    isX11,
    /** 便捷标志：当前操作系统 */
    os,
    /** 便捷标志：Linux 桌面会话类型 */
    desktopSession,
  };
}
