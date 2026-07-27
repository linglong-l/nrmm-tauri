<template>
  <div class="group-panel">
    <div class="groups-header">
      <span class="groups-title">Groups</span>
    </div>
    <div ref="groupListRef" class="group-list" @contextmenu.prevent="onContextMenuEmpty">
      <div
        v-for="(group, index) in groups"
        :key="group.groupId || index"
        class="group-item"
        :class="{
          active: selectedIndex === index && !showFavoritesOnly,
          disabled: group.groupDisabled
        }"
        @click="selectGroup(index)"
        @contextmenu.prevent="onContextMenu($event, group, index)"
      >
        <div class="group-avatar">
          <span class="avatar-text">{{ getGroupInitial(group.groupName, index) }}</span>
        </div>
        <span class="group-name" :title="group.groupName">{{ group.groupName }}</span>
      </div>
    </div>

    <div
      v-if="contextMenuVisible"
      class="context-menu"
      :style="{ top: contextMenuY + 'px', left: contextMenuX + 'px' }"
      @click.stop
    >
      <div class="menu-item" @click="handleAddGroup">
        <el-icon><Plus /></el-icon>
        {{ t('Add group') }}
      </div>
      <div class="menu-divider" v-if="contextGroup"></div>
      <div v-if="contextGroup" class="menu-item" @click="handleRename">
        <el-icon><Edit /></el-icon>
        {{ t('Rename') }}
      </div>
      <div v-if="contextGroup" class="menu-item" @click="handleOpenFolder">
        <el-icon><FolderOpened /></el-icon>
        {{ t('Open in File Explorer') }}
      </div>
      <div class="menu-divider" v-if="contextGroup"></div>
      <div v-if="contextGroup" class="menu-item danger" @click="handleDelete">
        <el-icon><Delete /></el-icon>
        {{ t('Remove group') }}
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { Plus, Edit, Delete, FolderOpened } from '@element-plus/icons-vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import type { ModGroupData } from '@/types'
import { addGroup, renameGroup, removeGroup, openGroupFolder } from '@/utils/tauri'
import { useModsStore } from '@/stores/mods'
import { useSettingsStore } from '@/stores/settings'
import { useDragScroll } from '@/composables/useDragScroll'

const { t } = useI18n()
const modsStore = useModsStore()
const settingsStore = useSettingsStore()

const groupListRef = ref<HTMLElement | null>(null)
useDragScroll(groupListRef)

const groups = computed(() => modsStore.groups)
const selectedIndex = computed(() => modsStore.selectedGroupIndex)
const showFavoritesOnly = computed(() => modsStore.showFavoritesOnly)

const contextMenuVisible = ref(false)
const contextMenuX = ref(0)
const contextMenuY = ref(0)
const contextGroup = ref<ModGroupData | null>(null)
const contextGroupIndex = ref(-1)

function getGroupInitial(name: string, index: number): string {
  if (name && name.length > 0) {
    return name.charAt(0).toUpperCase()
  }
  return String(index + 1)
}

function selectGroup(index: number) {
  modsStore.selectedGroupIndex = index
  modsStore.showFavoritesOnly = false
  modsStore.selectedModIndex = 0
}

function onContextMenu(e: MouseEvent, group: ModGroupData, index: number) {
  contextGroup.value = group
  contextGroupIndex.value = index
  contextMenuX.value = e.clientX
  contextMenuY.value = e.clientY
  contextMenuVisible.value = true
}

function onContextMenuEmpty(e: MouseEvent) {
  contextGroup.value = null
  contextGroupIndex.value = -1
  contextMenuX.value = e.clientX
  contextMenuY.value = e.clientY
  contextMenuVisible.value = true
}

function closeContextMenu() {
  contextMenuVisible.value = false
  contextGroup.value = null
}

async function handleAddGroup() {
  closeContextMenu()
  if (!settingsStore.currentModsPath) {
    ElMessage.warning(t('Please select a group first'))
    return
  }

  try {
    const { value } = await ElMessageBox.prompt(
      t('Group Name'),
      t('Add group'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        inputValidator: (val) => {
          if (val && val.trim().length > 0) return true
          return true
        }
      }
    )
    const groupName = value || undefined
    await addGroup(settingsStore.currentModsPath, settingsStore.currentGame, groupName)
    await modsStore.loadMods()
    ElMessage.success(t('Group added successfully'))
  } catch (e: any) {
    if (e !== 'cancel' && e !== 'close') {
      ElMessage.error(t('Failed to add group') + ': ' + (e?.message || e))
    }
  }
}

async function handleRename() {
  if (!contextGroup.value) return
  closeContextMenu()
  try {
    const { value } = await ElMessageBox.prompt(
      t('Group Name'),
      t('Rename'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        inputValue: contextGroup.value.groupName,
        inputValidator: (val) => {
          if (!val || !val.trim()) return t('Group name cannot be empty')
          return true
        }
      }
    )
    if (value && contextGroup.value.groupPath) {
      await renameGroup(contextGroup.value.groupPath, value.trim())
      await modsStore.loadMods()
      ElMessage.success(t('Group renamed successfully'))
    }
  } catch (e) {
  }
}

async function handleOpenFolder() {
  if (!contextGroup.value) return
  closeContextMenu()
  try {
    await openGroupFolder(contextGroup.value.groupPath)
  } catch (e: any) {
    ElMessage.error(t('Failed to open folder') + ': ' + (e?.message || e))
  }
}

async function handleDelete() {
  if (!contextGroup.value) return
  closeContextMenu()
  try {
    await ElMessageBox.confirm(
      t('Removing group will make the mods inside the group can be used again without mod manager.'),
      t('Warning'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        type: 'warning'
      }
    )
    if (contextGroup.value.groupPath) {
      await removeGroup(contextGroup.value.groupPath)
      await modsStore.loadMods()
      ElMessage.success(t('Group removed successfully'))
    }
  } catch (e) {
  }
}

function handleClickOutside() {
  closeContextMenu()
}

onMounted(() => {
  document.addEventListener('click', handleClickOutside)
})

onUnmounted(() => {
  document.removeEventListener('click', handleClickOutside)
})
</script>

<style scoped>
.group-panel {
  width: 72px;
  min-width: 72px;
  height: 100%;
  background: transparent;
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 8px 0;
  gap: 4px;
}

.groups-header {
  width: 100%;
  padding: 8px 0;
  display: flex;
  justify-content: center;
  flex-shrink: 0;
}

.groups-title {
  font-size: 10px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.4);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  writing-mode: vertical-rl;
  text-orientation: mixed;
}

.group-list {
  flex: 1;
  width: 100%;
  overflow-y: auto;
  overflow-x: hidden;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 4px 0;
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.group-list::-webkit-scrollbar {
  display: none;
}

.group-item {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  cursor: pointer;
  padding: 4px;
  border-radius: 8px;
  transition: all 0.15s ease;
  width: 60px;
}

.group-item:hover {
  background: transparent;
}

.group-item.active {
  background: transparent;
}

.group-item.disabled {
  opacity: 0.4;
}

.group-avatar {
  width: 48px;
  height: 48px;
  border-radius: 50%;
  background: transparent;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 2px solid transparent;
  transition: all 0.2s ease;
}

.group-item.active .group-avatar {
  border-color: #4a9eff;
  box-shadow: 0 0 12px rgba(74, 158, 255, 0.5);
}

.avatar-text {
  font-size: 18px;
  font-weight: 600;
  color: #ccc;
}

.group-item.active .avatar-text {
  color: #fff;
}

.group-name {
  font-size: 10px;
  color: rgba(255, 255, 255, 0.6);
  text-align: center;
  width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.group-item.active .group-name {
  color: #fff;
}

.context-menu {
  position: fixed;
  background: transparent;
  border: 1px solid #444;
  border-radius: 8px;
  padding: 4px 0;
  min-width: 160px;
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
