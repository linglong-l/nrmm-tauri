<template>
  <div
    class="mods-view"
    @dragover.prevent="handleDragOver"
    @dragleave.prevent="handleDragLeave"
    @drop.prevent="handleDrop"
  >
    <GroupPanel />
    <ModGrid />
    <div v-if="isDragging" class="drag-overlay">
      <div class="drag-content">
        <el-icon :size="64"><UploadFilled /></el-icon>
        <p>{{ t('Drag & Drop mod folders here to add mods to this group (1 folder = 1 mod).') }}</p>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { UploadFilled } from '@element-plus/icons-vue'
import { ElMessage } from 'element-plus'
import GroupPanel from '@/components/mod/GroupPanel.vue'
import ModGrid from '@/components/mod/ModGrid.vue'
import { useModsStore } from '@/stores/mods'
import { useSettingsStore } from '@/stores/settings'
import { importModAuto, isSupportedArchive } from '@/utils/tauri'
import { logger } from '@/utils/logger'
import { listen } from '@tauri-apps/api/event'

const { t } = useI18n()
const modsStore = useModsStore()
const settingsStore = useSettingsStore()

const isDragging = ref(false)
let unlistenManagedFolderChanged: (() => void) | null = null
let unlistenHotkeyRefresh: (() => void) | null = null
let unlistenTrayRefresh: (() => void) | null = null

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

async function handleDrop(e: DragEvent) {
  isDragging.value = false
  const files = e.dataTransfer?.files
  if (!files || files.length === 0) return

  if (!settingsStore.currentModsPath) {
    ElMessage.warning(t('Please select a game first.'))
    return
  }

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

  await modsStore.refresh()
}

onMounted(async () => {
  await settingsStore.load()
  await modsStore.loadMods()

  try {
    unlistenManagedFolderChanged = await listen('managed-folder-changed', () => {
      modsStore.refresh()
    })
    unlistenHotkeyRefresh = await listen('hotkey-refresh', () => {
      modsStore.refresh()
    })
    unlistenTrayRefresh = await listen('tray-refresh', () => {
      modsStore.refresh()
    })
  } catch (e) {
    logger.info('ModsView', 'Event listeners not available (dev mode)')
  }
})

onUnmounted(() => {
  unlistenManagedFolderChanged?.()
  unlistenHotkeyRefresh?.()
  unlistenTrayRefresh?.()
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
  background: transparent;
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
