<template>
  <div
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
import { listen } from '@tauri-apps/api/event'

const { t } = useI18n()
const modsStore = useModsStore()
const settingsStore = useSettingsStore()

/**
 * 是否正在拖拽文件到窗口
 * 用于控制拖拽覆盖提示层的显示/隐藏
 */
const isDragging = ref(false)
/**
 * 托管文件夹变化事件（managed-folder-changed）的取消监听函数
 * 组件卸载时调用以移除 Tauri 事件监听，防止内存泄漏
 */
let unlistenManagedFolderChanged: (() => void) | null = null
/**
 * 全局热键刷新事件（hotkey-refresh）的取消监听函数
 * 组件卸载时调用以移除 Tauri 事件监听，防止内存泄漏
 */
let unlistenHotkeyRefresh: (() => void) | null = null
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
 * 拖放事件处理：导入模组文件/目录
 *
 * 完整流程：
 * 1. 获取拖拽文件列表，提取每个文件的路径（优先使用 file.path，回退 webkitRelativePath）
 * 2. 校验：必须已选择游戏路径（currentModsPath）且已选中分组（groupPath），否则 ElMessage 警告
 * 3. 显示加载中提示（ElMessage 持久化 info 提示）
 * 4. 调用后端 importItems（import_item_cmd 命令）批量导入，支持 zip/rar/7z 压缩包和已解压目录混合导入
 * 5. 导入完成后检查结果：有失败项则显示错误提示，全部成功则显示成功提示
 * 6. 关闭加载提示，调用 modsStore.refresh() 刷新模组列表
 *
 * @param e DragEvent 拖拽事件对象，通过 e.dataTransfer.files 获取拖拽文件列表
 */
async function handleDrop(e: DragEvent) {
  isDragging.value = false
  const files = e.dataTransfer?.files
  if (!files || files.length === 0) return

  if (!settingsStore.currentModsPath) {
    ElMessage.warning(t('Please select a game first.'))
    return
  }

  const group = modsStore.currentGroup
  if (!group || !group.groupPath) {
    ElMessage.warning(t('Please select a group first'))
    return
  }

  const paths: string[] = []
  for (let i = 0; i < files.length; i++) {
    const file = files[i]
    const f = file as any
    const p = f.path || f.webkitRelativePath
    if (p) paths.push(p)
  }

  if (paths.length === 0) {
    ElMessage.warning(t('No valid paths found'))
    return
  }

  const loading: any = ElMessage({
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

    const hasError = results.some((r: any) => r?.ExtractFailed || (typeof r === 'object' && r.message && !r.mod_path))
    if (hasError) {
      ElMessage.error(t('Failed to add mods'))
    } else {
      ElMessage.success(t('Mods added successfully'))
    }
  } catch (err: any) {
    logger.error('ModsView', 'Import failed', err)
    ElMessage.error(t('Failed to add mods') + ': ' + (err?.message || String(err)))
  } finally {
    loading.close()
  }

  await modsStore.refresh()
}

onMounted(async () => {
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
      const expectedManaged = `${settingsStore.currentModsPath.replace(/[\\/]$/, '')}\\${'_MANAGED_'}`.replace(/\//g, '\\')
      const actualNormalized = watchedPath?.replace(/\//g, '\\') ?? ''
      if (!actualNormalized.endsWith(expectedManaged.slice(-12))) {
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
    unlistenManagedFolderChanged = await listen('managed-folder-changed', () => {
      // 防抖处理：合并短时间内的连续文件变更事件
      // 避免批量操作（如互斥组目录重命名）触发多次全量 refresh，导致耗时飙升
      if (refreshTimer) clearTimeout(refreshTimer)
      refreshTimer = setTimeout(() => {
        modsStore.refresh()
        refreshTimer = null
      }, REFRESH_DEBOUNCE_MS)
    })
  } catch (e) {
    logger.info('ModsView', 'Event listeners not available (dev mode)')
  }

  try {
    unlistenHotkeyRefresh = await listen('hotkey-refresh', () => {
      logger.debug('ModsView', 'Hotkey refresh triggered')
      modsStore.refresh()
    })
  } catch (e) {
    logger.info('ModsView', 'Hotkey refresh listener failed (dev mode)')
  }
})

onUnmounted(async () => {
  // Phase 2 策略：组件卸载时仅清理事件监听，不停止文件监控
  // 文件监控由 App.vue 的 applyDetectedGameSwitch 或根组件卸载时统一管理
  // 这样在切换 Tab 时文件监控仍保持运行，数据缓存有效，切回时无需重新加载
  unlistenManagedFolderChanged?.()
  unlistenHotkeyRefresh?.()
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
