/**
 * 缩略图 URL Composable（WebView 内存优化，2026-08-19）
 *
 * 功能：将原图路径异步解析为缩略图缓存文件的 webview URL（asset://），
 * 供 `<img>` 加载。配合后端 get_thumbnail 命令（分档位 JPEG 缓存：
 * 卡片档 240px / 头像档 64px），避免原图全尺寸解码位图撑爆 WebView 内存。
 *
 * 设计要点：
 * - **懒启用**：`enabled` 为 false（图片未进入视口）时不发起任何请求，
 *   与图片懒加载管线协同——只有进入视口的卡片才解析缩略图
 * - **档位参数**：`options.size`（'card' | 'avatar'，默认 'card'）决定请求档位
 *   与缓存键域，同一 composable 实例恒定档位（从 options 冻结为常量）
 * - **模块级缓存**：`档位:原图路径 → webviewUrl` 映射，同屏/组件复用不重复
 *   invoke（后端也缓存，此层只为省 IPC 往返）
 * - **竞态防护**：epoch 计数——路径变化/禁用/卸载后，过期响应不落地
 * - **失败回退**：缩略图生成失败时回退原图 URL（行为不劣化），回退结果不入缓存
 * - **路径切换清空**：复用卡片切换模组时立即清空 URL（显示骨架屏），
 *   新缩略图就绪后再显示，避免闪现旧图
 *
 * 使用方法：
 * ```ts
 * const enabled = computed(() => imageState.value !== 'idle')
 * const { url: thumbUrl } = useThumbnailUrl(
 *   computed(() => mod.value?.previewImagePath ?? ''),
 *   enabled,
 *   { size: 'card' },
 * )
 * ```
 */
import { ref, watch, onUnmounted, type Ref } from 'vue'
import { convertFileSrc } from '@tauri-apps/api/core'
import { getThumbnail } from '@/utils/tauri'
import { logger } from '@/utils/logger'

/** 缩略图档位占位符类型（与后端 get_thumbnail 的 size 参数对齐） */
export type ThumbSizeKey = 'card' | 'avatar'

/** 单个 composable 实例的档位配置  */
export interface UseThumbnailUrlOptions {
  /** 缩略图档位：'card'（默认，240px）| 'avatar'（64px） */
  size?: ThumbSizeKey
}

/** 模块级缓存：`档位:原图路径` → 缩略图 webview URL（进程生命周期，防重复 invoke） */
const thumbUrlCache = new Map<string, string>()

/**
 * 清空模块级缩略图 URL 缓存
 *
 * 窗口空闲回收模组数据（window-hidden-clear-data）时调用，释放全部
 * 原图路径→webview URL 的字符串映射；下次显示重新经后端缓存解析，开销极低。
 * 注意：Chromium 的 RSS 不会因 JS 堆释放立即回落（分配器惰性归还），
 * 前端内存的实质回落依赖 300s 空白页回收（JS 上下文整体销毁）。
 */
export function clearThumbnailUrlCache(): void {
  thumbUrlCache.clear()
}

/**
 * 异步解析原图路径对应的缩略图 webview URL
 *
 * @param originalPath 原图绝对路径（空串表示无图）
 * @param enabled 是否启用（进入视口后才为 true）
 * @param options 可选档位配置（size 默认为 'card'）
 * @returns `url`：缩略图 webview URL（未就绪/无图/禁用时为空串）
 */
export function useThumbnailUrl(
  originalPath: Ref<string>,
  enabled: Ref<boolean>,
  options?: UseThumbnailUrlOptions,
) {
  const sizeKey: ThumbSizeKey = options?.size ?? 'card'
  const url = ref('')
  /** 竞态世代计数器：路径变更/禁用/卸载时递增，使过期响应作废 */
  let epoch = 0

  async function resolve(path: string) {
    const current = ++epoch
    if (!path) {
      url.value = ''
      return
    }
    // 快路径：模块级缓存命中（同屏复用/滚动回看；缓存键含档位，档位独立）
    const cacheKey = `${sizeKey}:${path}`
    const cached = thumbUrlCache.get(cacheKey)
    if (cached) {
      url.value = cached
      return
    }
    try {
      const thumbPath = await getThumbnail(path, sizeKey)
      if (current !== epoch) return // 过期响应：期间路径已变/已禁用/已卸载
      const u = convertFileSrc(thumbPath)
      thumbUrlCache.set(cacheKey, u)
      url.value = u
    } catch (e) {
      if (current !== epoch) return
      // 失败回退原图（不劣化原有行为）；回退 URL 不入缓存（下次仍尝试缩略图）
      logger.warn('useThumbnailUrl', '缩略图生成失败，回退原图', e)
      try {
        url.value = convertFileSrc(path)
      } catch {
        url.value = ''
      }
    }
  }

  watch(
    [originalPath, enabled],
    ([path, en]) => {
      if (en) {
        // 路径切换：先清空显示骨架屏，异步就绪后再显示（避免闪现旧图）
        url.value = ''
        void resolve(path)
      } else {
        // 禁用（未进视口）：清空 URL 并使 in-flight 作废
        epoch++
        url.value = ''
      }
    },
    { immediate: true },
  )

  onUnmounted(() => {
    // 卸载：使 in-flight 请求作废
    epoch++
  })

  return { url }
}
