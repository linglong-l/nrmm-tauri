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
    <!-- 拖拽文件时的覆盖提示层 -->
    <div v-if="isDragging" class="drag-overlay">
      <div class="drag-content">
        <el-icon :size="64"><UploadFilled /></el-icon>
        <p>{{ t('Drag & Drop mod folders here to add mods to this group (1 folder = 1 mod).') }}</p>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 模组管理页面
 * 提供分组导航、模组网格展示、拖拽导入模组功能
 * 生命周期内管理文件监听器和模组数据刷新
 */
import { ref, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { UploadFilled } from '@element-plus/icons-vue'
import { ElMessage } from 'element-plus'
import GroupPanel from '@/components/mod/GroupPanel.vue'
import ModGrid from '@/components/mod/ModGrid.vue'
import UpdateModDataReminder from '@/components/common/UpdateModDataReminder.vue'
import { useModsStore } from '@/stores/mods'
import { useSettingsStore } from '@/stores/settings'
import { importModAuto, isSupportedArchive } from '@/utils/tauri'
import { logger } from '@/utils/logger'
import { listen } from '@tauri-apps/api/event'

const { t } = useI18n()
const modsStore = useModsStore()
const settingsStore = useSettingsStore()

/** 是否正在拖拽文件到窗口 */
const isDragging = ref(false)
/** 托管文件夹变化事件取消监听函数 */
let unlistenManagedFolderChanged: (() => void) | null = null

/**
 * 拖拽进入事件处理
 * 检测是否为文件拖拽以显示拖拽提示
 */
function handleDragOver(e: DragEvent) {
  if (e.dataTransfer?.types.includes('Files')) {
    isDragging.value = true
  }
}

/**
 * 拖拽离开事件处理
 * 当鼠标离开组件边界时隐藏拖拽提示
 */
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
 * 拖放事件处理
 * 支持拖放压缩包自动导入模组
 * 支持的格式：zip/7z/rar等（由后端isSupportedArchive判断）
 */
async function handleDrop(e: DragEvent) {
  isDragging.value = false
  const files = e.dataTransfer?.files
  if (!files || files.length === 0) return

  // 未选择游戏路径时提示用户先选择游戏
  if (!settingsStore.currentModsPath) {
    ElMessage.warning(t('Please select a game first.'))
    return
  }

  // 遍历拖入的文件，逐个尝试导入
  for (let i = 0; i < files.length; i++) {
    const file = files[i]
    const path = (file as any).path
    if (!path) continue

    try {
      const supported = await isSupportedArchive(path)
      if (supported) {
        await importModAuto(path, settingsStore.currentModsPath)
        ElMessage.success(t('Mods added successfully'))
      }
    } catch (err: any) {
      logger.error('ModsView', 'Import failed', err)
      ElMessage.error(t('Failed to add mods') + ': ' + (err?.message || err))
    }
  }

  // 导入完成后刷新模组列表
  await modsStore.refresh()
}

onMounted(async () => {
  // 如果已选择模组路径，启动文件监听并加载模组列表
  if (settingsStore.currentModsPath) {
    await modsStore.startWatching()
    await modsStore.loadMods()
  }

  // 监听后端托管文件夹变化事件
  // 事件名：managed-folder-changed
  // 处理逻辑：外部文件系统变化时自动刷新模组列表
  try {
    unlistenManagedFolderChanged = await listen('managed-folder-changed', () => {
      modsStore.refresh()
    })
  } catch (e) {
    logger.info('ModsView', 'Event listeners not available (dev mode)')
  }
})

onUnmounted(async () => {
  // 清理事件监听器和文件监视器
  unlistenManagedFolderChanged?.()
  await modsStore.stopWatching()
  modsStore.clearData()
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

.drag-content {
  text-align: center;
  color: var(--accent-primary);
}

.drag-content p {
  margin-top: 16px;
  font-size: 14px;
  max-width: 300px;
  line-height: 1.6;
}
</style>
