<template>
  <div
    class="mod-card"
    :class="{
      'is-selected': isSelected,
      'is-active': isActive,
      'is-disabled': !isEmptySlot && props.mod?.modDisabled,
      'is-empty': isEmptySlot,
      'is-error': hasError
    }"
    @click="handleClick"
    @dblclick="handleDoubleClick"
    @contextmenu.prevent="onContextMenu"
  >
    <div v-if="!isEmptySlot && mod" class="card-badges">
      <span v-if="mod.isFavorite" class="badge favorite-badge">⭐</span>
      <span v-if="mod.modDisabled" class="badge disabled-badge">
        <el-icon><Lock /></el-icon>
      </span>
      <span v-if="hasError" class="badge error-badge">
        <el-icon><Warning /></el-icon>
      </span>
    </div>
    <div class="card-image">
      <div v-if="isEmptySlot" class="empty-slot">
        <el-icon :size="40"><Plus /></el-icon>
      </div>
      <template v-else-if="mod">
        <el-image :src="iconUrl" fit="cover" class="mod-image" :alt="mod.modName">
          <template #placeholder>
            <div class="image-placeholder">
              <el-icon :size="48"><Picture /></el-icon>
            </div>
          </template>
          <template #error>
            <div class="image-placeholder">
              <el-icon :size="48"><Picture /></el-icon>
            </div>
          </template>
        </el-image>
      </template>
    </div>
    <div v-if="!isEmptySlot && mod" class="card-name" :title="mod.modName">
      {{ mod.modName }}
    </div>
    <div v-else class="card-name empty-name">空槽位</div>

    <div
      v-if="contextMenuVisible"
      class="context-menu"
      :style="{ top: contextMenuY + 'px', left: contextMenuX + 'px' }"
      @click.stop
    >
      <div v-if="mod" class="menu-item" @click="handleToggleEnabled">
        <el-icon><Switch /></el-icon>
        {{ mod.modDisabled ? t('Enable mod') : t('Disable mod completely') }}
      </div>
      <div v-if="mod" class="menu-item" @click="handleToggleFavorite">
        <el-icon><Star /></el-icon>
        {{ mod.isFavorite ? t('Unfavorite') : t('Favorite') }}
      </div>
      <div class="menu-divider"></div>
      <div class="menu-item" @click="handleRename">
        <el-icon><Edit /></el-icon>
        {{ t('Rename') }}
      </div>
      <div class="menu-item" @click="handleOpenFolder">
        <el-icon><FolderOpened /></el-icon>
        {{ t('Open in File Explorer') }}
      </div>
      <div class="menu-divider"></div>
      <div class="menu-item danger" @click="handleDelete">
        <el-icon><Delete /></el-icon>
        {{ t('Remove mod') }}
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { Lock, Warning, Picture, Switch, Star, Edit, Delete, FolderOpened, Plus } from '@element-plus/icons-vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import type { ModData } from '@/types'
import { toggleModDisabled, toggleFavorite, renameMod, removeMod, openModFolder } from '@/utils/tauri'
import { useModsStore } from '@/stores/mods'
import { logger } from '@/utils/logger'

const { t } = useI18n()
const modsStore = useModsStore()

const props = defineProps<{
  mod?: ModData
  groupIndex: number
  modIndex: number
  isEmptySlot?: boolean
}>()

const emit = defineEmits<{
  (e: 'select', groupIndex: number, modIndex: number): void
}>()

const mod = computed(() => props.mod)

const iconUrl = ref('')
const contextMenuVisible = ref(false)
const contextMenuX = ref(0)
const contextMenuY = ref(0)

const isSelected = computed(() => {
  if (props.isEmptySlot || !mod.value) return false
  return modsStore.selectedGroupIndex === props.groupIndex &&
         modsStore.selectedModIndex === props.modIndex &&
         !modsStore.showFavoritesOnly
})

const isActive = computed(() => {
  if (props.isEmptySlot || !mod.value) return false
  return mod.value.isActive && !mod.value.modDisabled
})

const hasError = computed(() => {
  if (props.isEmptySlot || !mod.value) return false
  return mod.value.erroredLines.length > 0 ||
         mod.value.namespaceError ||
         mod.value.pathTooLong ||
         mod.value.missingEndif.length > 0
})

function handleClick() {
  if (props.isEmptySlot) return
  emit('select', props.groupIndex, props.modIndex)
}

function handleDoubleClick() {
  if (props.isEmptySlot) return
  modsStore.selectModByIndex(props.groupIndex, props.modIndex)
}

function onContextMenu(e: MouseEvent) {
  if (props.isEmptySlot) return
  contextMenuX.value = e.clientX
  contextMenuY.value = e.clientY
  contextMenuVisible.value = true
}

function closeContextMenu() {
  contextMenuVisible.value = false
}

async function handleToggleEnabled() {
  if (!mod.value) return
  closeContextMenu()
  try {
    await toggleModDisabled(mod.value.modPath, mod.value.modDisabled)
    await modsStore.loadMods()
    ElMessage.success(mod.value.modDisabled ? t('Enabled') : t('Disabled'))
  } catch (e: any) {
    ElMessage.error(t('Failed to enable mod') + ': ' + (e?.message || e))
  }
}

async function handleToggleFavorite() {
  if (!mod.value) return
  closeContextMenu()
  try {
    await toggleFavorite(mod.value.modPath)
    await modsStore.loadMods()
  } catch (e: any) {
    logger.error('ModCard', 'Failed to toggle favorite', e)
  }
}

async function handleRename() {
  if (!mod.value) return
  closeContextMenu()
  try {
    const { value } = await ElMessageBox.prompt(
      t('Mod Name'),
      t('Rename'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        inputValue: mod.value.modName,
        inputValidator: (val) => {
          if (!val || !val.trim()) return t('Mod name cannot be empty')
          return true
        }
      }
    )
    if (value) {
      await renameMod(mod.value.modPath, value.trim())
      await modsStore.loadMods()
      ElMessage.success(t('Mod renamed successfully'))
    }
  } catch (e) {
  }
}

async function handleOpenFolder() {
  if (!mod.value) return
  closeContextMenu()
  try {
    await openModFolder(mod.value.modPath)
  } catch (e: any) {
    ElMessage.error(t('Failed to open mod folder.') + ': ' + (e?.message || e))
  }
}

async function handleDelete() {
  if (!mod.value) return
  closeContextMenu()
  try {
    await ElMessageBox.confirm(
      t('Removing mod will move it to restore zone.'),
      t('Warning'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        type: 'warning'
      }
    )
    await removeMod(mod.value.modPath)
    await modsStore.loadMods()
    ElMessage.success(t('Mod removed successfully'))
  } catch (e) {
  }
}

function handleClickOutside() {
  closeContextMenu()
}

onMounted(() => {
  if (mod.value?.modPath) {
  }
  document.addEventListener('click', handleClickOutside)
})

onUnmounted(() => {
  document.removeEventListener('click', handleClickOutside)
})
</script>

<style scoped>
.mod-card {
  width: 120px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  cursor: pointer;
  transition: transform 0.15s ease;
  position: relative;
}

.mod-card:active {
  transform: scale(0.96);
}

.card-image {
  width: 100px;
  height: 140px;
  border-radius: 8px;
  overflow: hidden;
  position: relative;
  border: 3px solid #555;
  background: transparent;
  transition: all 0.2s ease;
}

.mod-card.is-selected .card-image {
  border-color: #4a9eff;
  box-shadow: 0 0 16px rgba(74, 158, 255, 0.6), 0 0 32px rgba(74, 158, 255, 0.3);
}

.mod-card.is-active:not(.is-selected) .card-image {
  border-color: #888;
}

.mod-card.is-active.is-selected .card-image {
  border-color: #4a9eff;
  box-shadow: 0 0 16px rgba(74, 158, 255, 0.6), 0 0 32px rgba(74, 158, 255, 0.3);
}

.mod-card.is-disabled .card-image {
  border-color: #e74c3c;
  opacity: 0.7;
}

.mod-card.is-empty .card-image {
  border: 3px dashed #555;
  background: transparent;
}

.mod-card.is-error .card-image {
  border-color: #f39c12;
}

.mod-image {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.image-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #555;
}

.empty-slot {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #444;
}

.card-badges {
  position: absolute;
  top: -4px;
  right: -4px;
  display: flex;
  gap: 4px;
  z-index: 2;
}

.badge {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 11px;
  backdrop-filter: blur(4px);
}

.favorite-badge {
  background: transparent;
}

.disabled-badge {
  background: transparent;
  color: white;
}

.error-badge {
  background: transparent;
  color: white;
}

.card-name {
  margin-top: 8px;
  font-size: 12px;
  color: #ccc;
  text-align: center;
  width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  padding: 0 4px;
}

.empty-name {
  color: #555;
}

.context-menu {
  position: fixed;
  background: transparent;
  border: 1px solid #444;
  border-radius: 8px;
  padding: 4px 0;
  min-width: 180px;
  z-index: 9999;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
}

.menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  font-size: 13px;
  color: #eee;
  cursor: pointer;
  transition: background 0.1s;
}

.menu-item:hover {
  background: transparent;
}

.menu-item.danger {
  color: #e74c3c;
}

.menu-divider {
  height: 1px;
  background: transparent;
  margin: 4px 0;
}
</style>
