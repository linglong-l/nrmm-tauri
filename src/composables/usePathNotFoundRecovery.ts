/**
 * 路径不存在错误的统一恢复流程
 *
 * F3 解耦（2026-09-18）：原实现在 src/utils/tauri.ts 内直接调用 stores，
 * 造成 utils → stores 反向依赖。恢复流程本质是「编排 stores」的组合层逻辑，
 * 归属 composables（依赖方向：composables → stores → utils）。
 *
 * 流程：停止文件监听 → 清空模组缓存 → 重新监听 → 重读模组。
 * 供 openModFolder / openGroupFolder 的 catch 分支在 isPathNotFoundError(e) 为真时调用。
 */
import { useModsStore } from '@/stores/mods'
import { useSettingsStore } from '@/stores/settings'
import { logger } from '@/utils/logger'

/**
 * 执行路径不存在错误后的数据恢复（fire-and-forget 安全：内部吞异常并记日志）
 */
export async function recoverFromPathNotFound(): Promise<void> {
  logger.warn('PathNotFoundRecovery', 'recovering mods data after path-not-found')
  try {
    const modsStore = useModsStore()
    const settingsStore = useSettingsStore()
    await modsStore.stopWatching()
    modsStore.clearData()
    // clearData 后 currentGroup 为 null，用 settingsStore.currentModsPath 判断能否重载
    if (settingsStore.currentModsPath) {
      await modsStore.startWatching()
      await modsStore.loadMods()
    }
  } catch (e) {
    logger.error('PathNotFoundRecovery', 'Failed to refresh mods after path-not-found error', e)
  }
}