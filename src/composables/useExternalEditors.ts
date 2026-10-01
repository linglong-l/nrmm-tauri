/**
 * 外部代码编辑器探测 Composable
 *
 * 对齐 NRMM v3.2.0：Dart 在 App 启动时 findExternalCodeEditors()；
 * 移植端按需懒触发（首次右键菜单打开时探测），结果模块级缓存全局共享。
 * 探测失败静默降级为空列表（菜单不展示编辑器项，不阻塞其他操作）。
 */
import { ref } from 'vue'
import { detectExternalEditors } from '@/utils/tauri'
import type { ExternalEditorInfo } from '@/types'
import { logger } from '@/utils/logger'

/** 模块级共享：已探测编辑器列表（跨所有 ModCard 实例共享同一份） */
const externalEditors = ref<ExternalEditorInfo[]>([])

let detected = false
let detecting: Promise<void> | null = null

/** 确保已探测（幂等；失败也视为已探测，避免每次右键重复请求） */
export async function ensureEditorsDetected(): Promise<void> {
  if (detected) return
  if (!detecting) {
    detecting = detectExternalEditors()
      .then((list) => {
        externalEditors.value = list ?? []
        detected = true
      })
      .catch((e) => {
        logger.warn('ExternalEditors', 'detect failed', e)
        externalEditors.value = []
        detected = true
      })
      .finally(() => {
        detecting = null
      })
  }
  return detecting
}

/** 获取共享的已探测编辑器列表 */
export function useExternalEditors() {
  return { externalEditors, ensureEditorsDetected }
}
