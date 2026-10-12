<template>
  <div
    ref="dropZoneRef"
    class="mods-view"
    @dragover.prevent="handleDragOver"
    @dragleave.prevent="handleDragLeave"
    @drop.prevent="handleDrop"
  >
    <!-- 左侧分组面板 -->
    <GroupPanel />
    <!-- 右侧模组网格区域 -->
    <ModGrid />
    <!-- 更新模组数据提示条（仅Mods页面显示） -->
    <UpdateModDataReminder />
    <!-- dev 工具：移除模组对话框预览
         v-if="DEV_MODE" 守卫：prod 模式下 DEV_MODE 编译期为 false，组件不渲染、setup 不执行
         注意：Vue SFC 静态导入无法被 rollup tree-shaking 完全消除，组件代码会进入 prod 包
         但功能层面完全不可访问（约 12KB / gzip 4KB，对 Tauri 桌面安装包可忽略） -->
    <!-- <RemoveModDialogPreview v-if="DEV_MODE" /> -->
    <!-- 全页面加载遮罩：加载模组期间覆盖整个模组页，加载完成后淡出重渲染 -->
    <!-- 条件排除 isUpdatingModData：更新模组数据时由 UpdateModDataOverlay 独占显示，避免两遮罩冲突 -->
    <Transition name="overlay-fade">
      <div v-if="modsStore.loading && !modsStore.isUpdatingModData" class="full-loading-overlay">
        <div class="loading-box">
          <span class="loading-text">{{ t('Loading...') }}</span>
        </div>
      </div>
    </Transition>
    <!-- 拖拽文件时的覆盖提示层 -->
    <div v-if="isDragging" class="drag-overlay">
      <div class="drag-content">
        <el-icon :size="64"><UploadFilled /></el-icon>
        <p>{{ t('Drag & Drop mod files (zip/rar/7z) or folders here') }}</p>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { UploadFilled } from '@element-plus/icons-vue'
import GroupPanel from '@/components/mod/GroupPanel.vue'
import ModGrid from '@/components/mod/ModGrid.vue'
import UpdateModDataReminder from '@/components/common/UpdateModDataReminder.vue'
// 原 dev 工具组件 RemoveModDialogPreview 已在模板中注释（见第18行），若需恢复请同步重写 v-if="DEV_MODE" 守卫与 import
import { useModsStore } from '@/stores/mods'
import { useSettingsStore } from '@/stores/settings'
import { importItems, isFileWatcherRunning, currentWatchedPath } from '@/utils/tauri'
import { logger } from '@/utils/logger'
import { isWatcherOnModsPath } from '@/utils/modPath'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import type { ImportItemResult } from '@/utils/tauri'
import type { ManagedPartialUpdatePayload } from '@/types'
import { IpcEvent } from '@/events'

const { t } = useI18n()
const modsStore = useModsStore()
const settingsStore = useSettingsStore()

/**
 * 是否正在拖拽文件到窗口
 * 用于控制拖拽覆盖提示层的显示/隐藏
 */
const isDragging = ref(false)
/** 模组页拖放区根元素引用，原生拖放事件用它做落点命中测试 */
const dropZoneRef = ref<HTMLElement | null>(null)
/** 原生拖放事件（onDragDropEvent）的取消监听函数，组件卸载时调用 */
let unlistenDragDrop: (() => void) | null = null
/**
 * 托管文件夹变化事件（managed-folder-changed）的取消监听函数
 * 组件卸载时调用以移除 Tauri 事件监听，防止内存泄漏
 */
let unlistenManagedFolderChanged: (() => void) | null = null
/**
 * 托管目录增量事件（managed-partial-update）的取消监听函数
 * 组件卸载时调用以移除 Tauri 事件监听，防止内存泄漏
 */
let unlistenManagedPartialUpdate: (() => void) | null = null
/**
 * managed-folder-changed 事件防抖定时器句柄
 *
 * 文件系统可能在短时间内连续触发多次变更事件（例如批量导入、
 * 互斥组重命名目录等场景）。若无防抖，每次事件都会触发一次
 * modsStore.refresh() 全量轻量扫描，导致 300s+ 级别的耗时峰值。
 * 此处通过 500ms 防抖窗口合并连续事件，仅保留最后一次刷新。
 */
let refreshTimer: ReturnType<typeof setTimeout> | null = null
/** 防抖延迟：500ms（覆盖文件系统连续事件的最小间隔） */
const REFRESH_DEBOUNCE_MS = 500

function handleDragOver(e: DragEvent) {
  if (e.dataTransfer?.types.includes('Files')) {
    isDragging.value = true
  }
}

function handleDragLeave(e: DragEvent) {
  const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
  if (
    e.clientX <= rect.left ||
    e.clientX >= rect.right ||
    e.clientY <= rect.top ||
    e.clientY >= rect.bottom
  ) {
    isDragging.value = false
  }
}

/**
 * 导入拖放落地的模组文件/目录（原生通道与 HTML5 兜底通道共用）
 *
 * 完整流程：
 * 1. 校验落地路径列表非空（真实绝对路径由原生拖放事件给出）
 * 2. 校验：必须已选择游戏路径（currentModsPath）且已选中分组（groupPath），否则 ElMessage 警告
 * 3. 显示加载中提示（ElMessage 持久化 info 提示）
 * 4. 调用后端 importItems（import_item_cmd 命令）批量导入，支持 zip/rar/7z 压缩包和已解压目录混合导入
 * 5. 导入完成后检查结果：有失败项则显示错误提示，全部成功则显示成功提示
 * 6. 关闭加载提示，调用 modsStore.refresh() 刷新模组列表
 *
 * @param paths 落地项的绝对路径列表（压缩包或已解压目录）
 */
async function importDroppedPaths(paths: string[]) {
  // 更新模组数据期间禁止导入/拖放：update 正在批量改写 _MANAGED_，导入写入会产生中间态竞态
  if (modsStore.isUpdatingModData) {
    ElMessage.warning(t('Updating mod data, please wait'))
    return
  }

  if (!settingsStore.currentModsPath) {
    ElMessage.warning(t('Please select a game first.'))
    return
  }

  const group = modsStore.currentGroup
  if (!group || !group.groupPath) {
    ElMessage.warning(t('Please select a group first'))
    return
  }

  const loading = ElMessage({
    message: t('Loading...'),
    duration: 0,
    type: 'info',
    icon: undefined,
    customClass: 'el-message--loading',
  })
  loading.close = loading.close || (() => {})

  try {
    const results = await importItems({
      items: paths,
      targetGroupDir: group.groupPath,
    })

    const hasError = results.some((r: ImportItemResult) => r.ExtractFailed || (r.message && !r.mod_path))
    if (hasError) {
      ElMessage.error(t('Failed to add mods'))
    } else {
      ElMessage.success(t('Mods added successfully'))
    }
  } catch (err) {
    logger.error('ModsView', 'Import failed', err)
    const msg = err instanceof Error ? err.message : String(err)
    ElMessage.error(t('Failed to add mods') + ': ' + msg)
  } finally {
    loading.close()
  }

  await modsStore.refresh()
}

/**
 * HTML5 拖放通道（仅浏览器预览等非 Tauri 环境可达）
 *
 * Tauri 运行时下 wry 会 SetAllowExternalDrop(false) 拦截系统文件拖放（见
 * SettingsView.setupTauriDragDrop 的同款坑注释），此处的 dragover/drop 不会触发，
 * 且 dataTransfer 里的 file.path 只在 Electron 系存在，本来也拿不到绝对路径。
 */
async function handleDrop(e: DragEvent) {
  isDragging.value = false
  const files = e.dataTransfer?.files
  if (!files || files.length === 0) return

  const paths: string[] = []
  for (let i = 0; i < files.length; i++) {
    const f = files[i] as File & { path?: string }
    const p = f.path || f.webkitRelativePath
    if (p) paths.push(p)
  }

  if (paths.length === 0) {
    ElMessage.warning(t('No valid paths found'))
    return
  }

  await importDroppedPaths(paths)
}

/**
 * 落点是否落在模组页拖放区内
 *
 * payload.position 为窗口内物理像素，getBoundingClientRect 返回 CSS 逻辑像素，
 * 高 DPI（dpr>1）下须先除以 devicePixelRatio，否则区域下半部分会越界无法命中。
 */
function isInsideDropZone(x: number, y: number): boolean {
  const el = dropZoneRef.value
  if (!el) return false
  const rect = el.getBoundingClientRect()
  const dpr = window.devicePixelRatio || 1
  const cx = x / dpr
  const cy = y / dpr
  return cx >= rect.left && cx <= rect.right && cy >= rect.top && cy <= rect.bottom
}

/**
 * 注册 Tauri 原生拖放事件监听 —— 拖拽导入在 Tauri 下唯一可用通道
 *
 * enter/over 驱动覆盖层显隐，drop 取 payload.paths（真实绝对路径）走 importDroppedPaths。
 * 非 Tauri 环境注册失败即跳过，由 HTML5 handleDrop 兜底。
 */
async function setupTauriDragDrop() {
  try {
    unlistenDragDrop = await getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload
      if (payload.type === 'enter' || payload.type === 'over') {
        isDragging.value = isInsideDropZone(payload.position.x, payload.position.y)
        return
      }
      if (payload.type === 'leave') {
        isDragging.value = false
        return
      }
      if (payload.type === 'drop') {
        isDragging.value = false
        if (isInsideDropZone(payload.position.x, payload.position.y) && payload.paths.length > 0) {
          void importDroppedPaths(payload.paths)
        }
      }
    })
  } catch (e) {
    logger.warn('ModsView', 'Tauri onDragDropEvent unavailable, fallback to HTML5 drag events', e)
  }
}

onMounted(async () => {
  // 拖拽导入：Tauri 下系统文件拖放只能由原生通道收到，挂载即注册
  await setupTauriDragDrop()

  // Phase 4 策略：条件化加载
  // 1. 检查文件监控运行状态，未运行则启动，已运行但路径不对则切换
  // 2. 检查 modsStore.hasData()：缓存不为空则跳过 loadMods（避免重复加载），
  //    缓存为空（首次加载或切换游戏后）才调用 loadMods 完整加载
  if (settingsStore.currentModsPath) {
    const watcherRunning = await isFileWatcherRunning()
    if (!watcherRunning) {
      await modsStore.startWatching()
    } else {
      const watchedPath = await currentWatchedPath()
      if (!isWatcherOnModsPath(watchedPath, settingsStore.currentModsPath)) {
        await modsStore.startWatching()
      }
    }

    if (!modsStore.hasData()) {
      await modsStore.loadMods()
    } else {
      logger.info('ModsView', 'cache already loaded -> skip loadMods')
    }
  }

  try {
    unlistenManagedFolderChanged = await listen(IpcEvent.ManagedFolderChanged, () => {
      // 回声抑制（R1）：命令 patch 后短窗口内的 watcher 回声事件跳过（缓存已由命令内局部重扫合并，
      // 批次处理为幂等重复）；外部真实变更不受影响（窗口外）
      if (Date.now() - modsStore.lastLocalPatchAt < 2000) {
        logger.debug('ModsView', 'folder-changed 为命令回声，跳过全量 refresh')
        return
      }
      // 防抖处理：合并短时间内的连续文件变更事件
      // 避免批量操作（如互斥组目录重命名）触发多次全量 refresh，导致耗时飙升
      if (refreshTimer) clearTimeout(refreshTimer)
      refreshTimer = setTimeout(() => {
        // 回声抑制复查（R2）：后端双事件顺序为 folder-changed 先、partial-update 后，
        // 事件到达时增量尚未 patch（lastLocalPatchAt 未置位），须在到期时复查——
        // 增量已 patch 的批次跳过全量 refresh，全量路径仅余兜底（溢出/失效批次）
        if (Date.now() - modsStore.lastLocalPatchAt < 2000) {
          logger.debug('ModsView', 'folder-changed 防抖到期，增量已 patch，跳过全量 refresh')
          refreshTimer = null
          return
        }
        modsStore.refresh()
        refreshTimer = null
      }, REFRESH_DEBOUNCE_MS)
    })
  } catch (e) {
    logger.info('ModsView', 'Event listeners not available (dev mode)')
  }

  try {
    unlistenManagedPartialUpdate = await listen<ManagedPartialUpdatePayload>(
      IpcEvent.ManagedPartialUpdate,
      (event) => {
        const { deltas } = event.payload
        if (!Array.isArray(deltas) || deltas.length === 0) return
        // R2：增量事件直接携带分组 delta，逐组原地 patch（复用 R1 基建；applyGroupDelta 置
        // lastLocalPatchAt → 已到达的 folder-changed 500ms 防抖 timer 到期时被回声抑制跳过
        // ——全量 refresh 仅剩兜底路径）
        // 树中不存在的组（外部新建分组、树未加载）无法原地 patch → 跳过 patch 且不置位
        // 时间戳，保留 folder-changed 已建的防抖 timer 走全量 refresh 兜底
        const unpatchable = deltas.some((d) => d.newGroup !== null && !modsStore.hasGroup(d.oldGroupPath))
        if (unpatchable) {
          logger.debug('ModsView', 'managed-partial-update 含树外分组，转全量 refresh 兜底', {
            groups: deltas.length,
          })
          return
        }
        for (const delta of deltas) modsStore.applyGroupDelta(delta)
        logger.debug('ModsView', 'managed-partial-update 增量 patch', { groups: deltas.length })
      }
    )
  } catch (e) {
    logger.info('ModsView', 'managed-partial-update listener failed (dev mode)')
  }
})

onUnmounted(async () => {
  // Phase 2 策略：组件卸载时仅清理事件监听，不停止文件监控
  // 文件监控由 App.vue 的 applyDetectedGameSwitch 或根组件卸载时统一管理
  // 这样在切换 Tab 时文件监控仍保持运行，数据缓存有效，切回时无需重新加载
  unlistenManagedFolderChanged?.()
  unlistenManagedPartialUpdate?.()
  unlistenDragDrop?.()
  // 清理防抖定时器，避免组件卸载后残留定时器触发已销毁的 store
  if (refreshTimer) {
    clearTimeout(refreshTimer)
    refreshTimer = null
  }
})
</script>

<style scoped>
.mods-view {
  display: flex;
  flex: 1;
  height: 100%;
  position: relative;
  overflow: hidden;
}

.drag-overlay {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(74, 158, 255, 0.08);
  backdrop-filter: blur(4px);
  -webkit-backdrop-filter: blur(4px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
  border: 3px dashed var(--accent-primary);
  border-radius: var(--border-radius);
  margin: 8px;
  pointer-events: none;
}

/* 全页面加载遮罩：覆盖整个模组页（含左侧分组面板），风格与更新模组弹窗统一 */
.full-loading-overlay {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  z-index: 200;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  pointer-events: all;
}

.loading-box {
  background: #1e1e1e;
  border-radius: 8px;
  padding: 24px 36px;
  min-width: 200px;
  text-align: center;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
}

.loading-text {
  color: #fff;
  font-size: 14px;
}

.overlay-fade-enter-active,
.overlay-fade-leave-active {
  transition: opacity 0.18s ease;
}
.overlay-fade-enter-from,
.overlay-fade-leave-to {
  opacity: 0;
}

.drag-content {
  text-align: center;
  color: var(--accent-primary);
}

.drag-content p {
  margin-top: 16px;
  font-size: 14px;
  max-width: 360px;
  line-height: 1.6;
}
</style>
