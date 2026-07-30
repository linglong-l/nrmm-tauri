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
    <!-- 状态徽章：收藏⭐、禁用🔒、错误⚠️ -->
    <div v-if="!isEmptySlot && mod" class="card-badges">
      <span v-if="mod.isFavorite" class="badge favorite-badge">⭐</span>
      <span v-if="mod.modDisabled" class="badge disabled-badge">
        <el-icon><Lock /></el-icon>
      </span>
      <span v-if="hasError" class="badge error-badge">
        <el-icon><Warning /></el-icon>
      </span>
    </div>
    <!-- 卡片图片区域：100×140px竖版人像样式 -->
    <div class="card-image">
      <!-- 空槽位：虚线边框+加号 -->
      <div v-if="isEmptySlot" class="empty-slot">
        <el-icon :size="40"><Plus /></el-icon>
      </div>
      <!-- 模组预览图：懒加载，失败显示占位符 -->
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
    <!-- 模组名称 -->
    <div v-if="!isEmptySlot && mod" class="card-name" :title="mod.modName">
      {{ mod.modName }}
    </div>
    <div v-else class="card-name empty-name">{{ t('common.emptySlot', '空槽位') }}</div>

    <!-- 右键菜单：启用/禁用、收藏、重命名、打开文件夹、删除 -->
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
/**
 * 模组卡片组件
 * 竖版人像样式（100×140px图片区域）
 * 边框状态逻辑：
 * - 蓝色发光边框：当前选中模组
 * - 灰色边框：已启用模组（未选中）
 * - 红色边框：已禁用模组
 * - 橙色边框：有错误的模组
 * - 虚线边框：空槽位（用于网格对齐）
 * 支持：单击选中、双击确认启用、右键菜单
 */
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
  /** 模组数据（空槽位时无） */
  mod?: ModData
  /** 模组在当前显示列表中的索引 */
  modIndex: number
  /** 是否为空槽位（用于网格对齐） */
  isEmptySlot?: boolean
}>()

const emit = defineEmits<{
  /** 选中模组事件 */
  (e: 'select', modIndex: number): void
}>()

/** 模组数据计算属性 */
const mod = computed(() => props.mod)

/** 模组图标URL（预留，当前未实现自动图标） */
const iconUrl = ref('')
/** 右键菜单是否可见 */
const contextMenuVisible = ref(false)
/** 右键菜单位置X */
const contextMenuX = ref(0)
/** 右键菜单位置Y */
const contextMenuY = ref(0)

/**
 * 是否为当前选中模组
 * 条件：同分组（路径匹配）、同索引、非收藏模式、非空槽位
 */
const isSelected = computed(() => {
  if (props.isEmptySlot || !mod.value) return false
  const currentGroup = modsStore.currentGroup
  if (!currentGroup) return false
  // 检查模组是否属于当前选中分组（通过modPath前缀匹配）
  const modBelongsToCurrentGroup = mod.value.modPath.startsWith(currentGroup.groupPath)
  return modBelongsToCurrentGroup &&
         modsStore.selectedModIndex === props.modIndex &&
         !modsStore.showFavoritesOnly
})

/**
 * 模组是否激活（INI中已启用且未被完全禁用）
 */
const isActive = computed(() => {
  if (props.isEmptySlot || !mod.value) return false
  return mod.value.isActive && !mod.value.modDisabled
})

/**
 * 是否有错误
 * 错误类型：INI解析错误行、命名空间冲突、路径过长、缺失endif
 */
const hasError = computed(() => {
  if (props.isEmptySlot || !mod.value) return false
  return mod.value.erroredLines.length > 0 ||
         mod.value.namespaceError ||
         mod.value.pathTooLong ||
         mod.value.missingEndif.length > 0
})

/** 右键菜单 */
function onContextMenu(e: MouseEvent) {
  if (props.isEmptySlot) return
  // 标记后续一次左键为右键后残留点击，避免误选中（飘逸问题）
  ignoreNextClickUntil = performance.now() + 400
  // 菜单估算尺寸（min-width:160px，按6项约184x240px 保守计算）
  const MENU_ESTIMATED_W = 200
  const MENU_ESTIMATED_H = 260
  const vw = window.innerWidth
  const vh = window.innerHeight
  let x = e.clientX
  let y = e.clientY
  // 水平溢出：右边缘超出就左对齐到鼠标位置左侧
  if (x + MENU_ESTIMATED_W > vw - 4) {
    x = Math.max(4, x - MENU_ESTIMATED_W)
  }
  // 垂直溢出：下边缘超出就向上弹出
  if (y + MENU_ESTIMATED_H > vh - 4) {
    y = Math.max(4, y - MENU_ESTIMATED_H)
  }
  contextMenuX.value = x
  contextMenuY.value = y
  contextMenuVisible.value = true
}

/** 关闭右键菜单 */
function closeContextMenu() {
  contextMenuVisible.value = false
  // 关闭菜单后再忽略一次左键，避免左键关闭菜单时误点卡片
  ignoreNextClickUntil = performance.now() + 250
}

/**
 * 右键菜单显示期间及刚关闭后，左键点击不触发选中，防止"飘逸"。
 * - 飘逸根因：用户右键→菜单弹出→左键点空白关闭菜单→这次click事件恰好落在卡片上→
 *   浏览器把它当普通左键点击卡片→select触发→选中状态跳动（看起来飘逸/误选中其他mod）
 */
let ignoreNextClickUntil = 0

/** 单击：选中模组（仅更新选中状态，不写入INI） */
function handleClick() {
  if (props.isEmptySlot) return
  const now = performance.now()
  if (contextMenuVisible.value || now < ignoreNextClickUntil) {
    contextMenuVisible.value = false
    return
  }
  emit('select', props.modIndex)
}

/** 双击：确认选中模组（调用selectModByIndex写入INI，处理互斥组逻辑） */
function handleDoubleClick() {
  if (props.isEmptySlot) return
  modsStore.selectModByIndex(props.modIndex)
}

/**
 * 切换模组启用/禁用状态
 * 调用后端toggle_mod_disabled命令，传入isMutex参数处理互斥组逻辑
 */
async function handleToggleEnabled() {
  if (!mod.value) return
  closeContextMenu()
  try {
    await toggleModDisabled(mod.value.modPath, mod.value.modDisabled, mod.value.isMutex)
    await modsStore.refresh()
    ElMessage.success(mod.value.modDisabled ? t('Enabled') : t('Disabled'))
  } catch (e: any) {
    ElMessage.error(t('Failed to enable mod') + ': ' + (e?.message || e))
  }
}

/** 切换收藏状态 */
async function handleToggleFavorite() {
  if (!mod.value) return
  closeContextMenu()
  try {
    await toggleFavorite(mod.value.modPath)
    await modsStore.refresh()
  } catch (e: any) {
    logger.error('ModCard', 'Failed to toggle favorite', e)
  }
}

/** 重命名模组 */
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
      await modsStore.refresh()
      ElMessage.success(t('Mod renamed successfully'))
    }
  } catch (e) {
  }
}

/** 在文件管理器中打开模组文件夹 */
async function handleOpenFolder() {
  if (!mod.value) return
  closeContextMenu()
  try {
    await openModFolder(mod.value.modPath)
  } catch (e: any) {
    ElMessage.error(t('Failed to open mod folder.') + ': ' + (e?.message || e))
  }
}

/**
 * 删除模组（移入回收站）
 * 弹出确认对话框，确认后调用后端remove_mod命令
 */
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
    await modsStore.refresh()
    ElMessage.success(t('Mod removed successfully'))
  } catch (e) {
  }
}

/** 点击外部区域关闭右键菜单 */
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

.mod-card:focus-visible {
  outline: 2px solid var(--accent-primary);
  outline-offset: 2px;
  border-radius: 8px;
}

.card-image {
  width: 100px;
  height: 140px;
  border-radius: 8px;
  overflow: hidden;
  position: relative;
  border: 3px solid #555;
  background: rgba(255, 255, 255, 0.03);
  transition: border-color 0.2s ease, box-shadow 0.2s ease, opacity 0.2s ease;
}

/* 选中状态：蓝色发光边框 */
.mod-card.is-selected .card-image {
  border-color: #4a9eff;
  box-shadow: 0 0 16px rgba(74, 158, 255, 0.6), 0 0 32px rgba(74, 158, 255, 0.3);
}

/* 启用状态（未选中）：灰色边框 */
.mod-card.is-active:not(.is-selected) .card-image {
  border-color: #888;
}

.mod-card.is-active.is-selected .card-image {
  border-color: #4a9eff;
  box-shadow: 0 0 16px rgba(74, 158, 255, 0.6), 0 0 32px rgba(74, 158, 255, 0.3);
}

/* 禁用状态：红色边框 */
.mod-card.is-disabled .card-image {
  border-color: #e74c3c;
  opacity: 0.7;
}

/* 空槽位：虚线边框 */
.mod-card.is-empty .card-image {
  border: 3px dashed #555;
  background: transparent;
}

/* 错误状态：橙色边框 */
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
  background: rgba(255, 193, 7, 0.16);
  color: #ffc107;
}

.disabled-badge {
  background: rgba(231, 76, 60, 0.16);
  color: #fff;
}

.error-badge {
  background: rgba(243, 156, 18, 0.16);
  color: #fff;
}

.card-name {
  font-size: 11px;
  color: rgba(255, 255, 255, 0.75);
  text-align: center;
  width: 100%;
  margin-top: 6px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  padding: 0 2px;
}

.mod-card.is-selected .card-name {
  color: #fff;
}

.empty-name {
  color: rgba(255, 255, 255, 0.3);
}

.context-menu {
  position: fixed;
  background: rgba(30, 30, 30, 0.96);
  backdrop-filter: blur(20px) saturate(180%);
  -webkit-backdrop-filter: blur(20px) saturate(180%);
  border: 1px solid rgba(255, 255, 255, 0.10);
  border-radius: 8px;
  padding: 4px 0;
  min-width: 160px;
  z-index: 9999;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
}

.menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  font-size: 13px;
  color: #eee;
  cursor: pointer;
  transition: background-color 0.1s ease;
}

.menu-item:hover {
  background: rgba(74, 158, 255, 0.16);
}

.menu-item.danger {
  color: #e74c3c;
}

.menu-item.danger:hover {
  background: rgba(231, 76, 60, 0.12);
}

.menu-divider {
  height: 1px;
  background: rgba(255, 255, 255, 0.08);
  margin: 4px 0;
}
</style>
