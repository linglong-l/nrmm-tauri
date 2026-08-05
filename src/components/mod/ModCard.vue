<template>
  <div
    class="mod-card"
    :class="{
      'is-selected': isSelected,
      'is-active': isActive,
      'is-disabled': !isNoneSlot && props.mod?.modDisabled,
      'is-empty': isNoneSlot,
      'is-error': hasError,
      'no-scale': contextMenuVisible
    }"
    @click="handleClick"
    @dblclick="handleDoubleClick"
    @contextmenu.prevent="onContextMenu"
  >
    <!-- 状态徽章：收藏⭐、禁用🔒、错误⚠️ -->
    <div v-if="!isNoneSlot && mod" class="card-badges">
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
      <div v-if="isNoneSlot" class="empty-slot">
        <el-icon :size="40"><Plus /></el-icon>
      </div>
      <!-- 模组预览图：懒加载，失败显示占位符 -->
      <template v-else-if="mod">
        <!-- 图片懒加载容器 -->
        <div ref="imageContainerRef" class="mod-image-wrapper">
          <!-- 无图片URL：直接显示占位符 -->
          <div v-if="!iconUrl" class="image-placeholder">
            <el-icon :size="48"><Picture /></el-icon>
          </div>
          <!-- 有图片URL：根据加载状态显示不同UI -->
          <template v-else>
            <!-- 加载中/未加载：骨架屏脉冲动画 -->
            <div v-if="imageState === 'idle' || imageState === 'loading'" class="image-placeholder skeleton-pulse">
              <el-icon :size="48"><Picture /></el-icon>
            </div>
            <!-- 隐式加载图片：visibility hidden 保持布局并触发浏览器加载 -->
            <img
              v-if="imageState === 'loading'"
              :src="iconUrl"
              class="mod-image mod-image-hidden"
              loading="eager"
              decoding="async"
              draggable="false"
              @load="onImageLoaded"
              @error="onImageError"
            />
            <!-- 加载成功：淡入显示 -->
            <img
              v-else-if="imageState === 'loaded'"
              :src="iconUrl"
              :alt="mod.modName"
              class="mod-image mod-image-fade-in"
              loading="lazy"
              decoding="async"
              draggable="false"
            />
            <!-- 加载失败：占位符 -->
            <div v-else class="image-placeholder">
              <el-icon :size="48"><Picture /></el-icon>
            </div>
          </template>
        </div>
      </template>
    </div>
    <!-- 模组名称 -->
    <div v-if="!isNoneSlot && mod" class="card-name" :title="mod.modName"><HighlightText :text="mod.modName" :spans="highlightSpans" /></div>
    <div v-else class="card-name empty-name">{{ t('common.emptySlot', '空槽位') }}</div>

    <!-- 右键菜单：Teleport到body，避免父元素transform导致fixed定位漂移 -->
    <Teleport to="body">
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
    </Teleport>
    <!-- 移除模组对话框（UI1确认 + UI2成功提示） -->
    <RemoveModDialog
      v-model="removeDialogVisible"
      :mod-name="removeDialogModName"
      :mod-path="removeDialogModPath"
      @removed="handleRemoved"
    />
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
import { convertFileSrc } from '@tauri-apps/api/core'
import type { ModData } from '@/types'
import { toggleModDisabled, toggleFavorite, renameMod, openModFolder, handlePathNotFoundError } from '@/utils/tauri'
import { useModsStore } from '@/stores/mods'
import { useImageLazyLoad } from '@/composables/useImageLazyLoad'
import HighlightText from '@/components/common/HighlightText.vue'
import RemoveModDialog from '@/components/mod/RemoveModDialog.vue'
import type { ImageLoadState } from '@/composables/useImageLazyLoad'
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
  /** 选中模组事件（单击，仅UI高亮） */
  (e: 'select', modIndex: number): void
  /** 启用模组事件（双击，写入INI） */
  (e: 'activate', modIndex: number): void
}>()

/** 模组数据计算属性 */
const mod = computed(() => props.mod)

const highlightSpans = computed<[number, number][]>(() => {
  const name = mod.value?.modName || ''
  const q = modsStore.searchQuery?.trim()
  if (!q || !name) return []
  const { matched, spans } = modsStore.fuzzyMatchWithSpansSimple(name, q)
  if (!matched || !spans.length) return []
  return spans
})

/**
 * 是否为 None 空槽位
 * 两种情况：
 * 1. 父组件显式传入 isEmptySlot=true（网格对齐用的占位符）
 * 2. 模组数据为 None 槽位（name="None"，modPath="None"）
 */
const isNoneSlot = computed(() => {
  return props.isEmptySlot || (mod.value && (mod.value.name === 'None' || mod.value.modPath === 'None'))
})

/**
 * 模组图标URL：将后端返回的本地文件路径转换为webview可访问的asset:// URL
 * - 后端在扫描时自动查找 icon.png/preview.png 等图片，填入 previewImagePath
 * - convertFileSrc 将本地路径转换为 Tauri asset 协议 URL
 * - 无图标或非Tauri环境时返回空字符串，UI降级显示占位符
 */
const iconUrl = computed(() => {
  const p = mod.value?.previewImagePath
  if (!p) return ''
  try {
    return convertFileSrc(p)
  } catch {
    return ''
  }
})

/** 图片容器 DOM 引用 */
const imageContainerRef = ref<HTMLElement | null>(null)

/** 图片懒加载 composable */
const imageLazyLoad = useImageLazyLoad({
  rootMargin: '200px 0px 200px 0px',
  staggerDelay: 50
})

/** 当前图片加载状态 */
const imageState = computed<ImageLoadState>(() => {
  return imageLazyLoad.states.value.get(props.modIndex) || 'idle'
})

/** 图片加载成功回调 */
function onImageLoaded() {
  imageLazyLoad.markLoaded(props.modIndex)
}

/** 图片加载失败回调 */
function onImageError() {
  imageLazyLoad.markError(props.modIndex)
}

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
  if (isNoneSlot.value || !mod.value) return false
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
 * None 槽位在没有其他模组启用时显示为激活状态
 */
const isActive = computed(() => {
  if (isNoneSlot.value) {
    // None 槽位：如果 mod 存在且 isActive 为 true，则显示激活
    return mod.value?.isActive ?? false
  }
  if (!mod.value) return false
  return mod.value.isActive && !mod.value.modDisabled
})

/**
 * 是否有错误
 * 错误类型：INI解析错误行、命名空间冲突、路径过长、缺失endif
 */
const hasError = computed(() => {
  if (isNoneSlot.value || !mod.value) return false
  return mod.value.erroredLines.length > 0 ||
         mod.value.namespaceError ||
         mod.value.pathTooLong ||
         mod.value.missingEndif.length > 0
})

/** 右键菜单 */
function onContextMenu(e: MouseEvent) {
  if (isNoneSlot.value) return
  // 标记后续短暂时间内的左键为右键后残留点击，避免误选中
  // Teleport后误触概率已大幅降低，缩短时间窗到300ms
  ignoreNextClickUntil = performance.now() + 300
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
  // 关闭菜单后短暂忽略左键（teleport后已极短100ms即可）
  ignoreNextClickUntil = performance.now() + 100
}

/**
 * 右键菜单显示期间及刚关闭后，左键点击不触发选中，防止"飘逸"。
 * - 飘逸根因：用户右键→菜单弹出→左键点空白关闭菜单→这次click事件恰好落在卡片上→
 *   浏览器把它当普通左键点击卡片→select触发→选中状态跳动（看起来飘逸/误选中其他mod）
 */
let ignoreNextClickUntil = 0

/** 单击：不触发选中效果，仅关闭已打开的右键菜单（避免误选中） */
function handleClick() {
  if (isNoneSlot.value) return
  const now = performance.now()
  if (contextMenuVisible.value || now < ignoreNextClickUntil) {
    contextMenuVisible.value = false
    return
  }
  // 注意：单击不再 emit('select')，不触发任何效果
  // 如需选中高亮 → 请双击启用模组或通过右键菜单操作
}

/** 双击：确认启用模组（调用后端写入INI，自动同步选中状态） */
function handleDoubleClick() {
  logger.debug('ModCard', 'onDoubleClick activateModByIndex', { modIndex: props.modIndex, modName: mod.value?.name })
  if (isNoneSlot.value) return
  // 同步切换左侧导航栏对应分组为选中状态
  if (mod.value) {
    const modPath = mod.value.modPath
    // 从模组路径中提取分组路径（向上查找 _MANAGED_ 目录）
    const managedIndex = modPath.indexOf('_MANAGED_')
    if (managedIndex !== -1) {
      // 分组路径 = _MANAGED_ 目录 + 后续路径
      const groupPath = modPath.substring(managedIndex)
      // 查找并选中对应分组
      modsStore.selectGroupByPath(groupPath)
    }
  }
  // 双击直接触发启用（activate）= 右键菜单中的「启用」效果
  emit('activate', props.modIndex)
}

/**
 * 切换模组启用/禁用状态
 * 调用后端toggle_mod_disabled命令，传入isMutex参数处理互斥组逻辑
 */
async function handleToggleEnabled() {
  if (!mod.value) return
  closeContextMenu()
  // 操作前捕获原禁用状态，用于生成操作后的提示文案（refresh 后会更新为操作后状态，不能直接拿来判断）
  const wasDisabled = mod.value.modDisabled
  try {
    await toggleModDisabled(mod.value.modPath, mod.value.modDisabled, mod.value.isMutex)
    // 标记需要更新模组数据（仅NormalGroup操作）
    if (!mod.value.isMutex) {
      modsStore.markNeedUpdate(mod.value.groupIndex)
    }
    await modsStore.refresh()
    ElMessage.success(wasDisabled ? t('Enabled') : t('Disabled'))
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
    // 路径不存在错误 → 清除缓存+重读模组（自动处理，不弹错误提示）
    const handled = await handlePathNotFoundError(e)
    if (!handled) {
      ElMessage.error(t('Failed to open mod folder.') + ': ' + (e?.message || e))
    }
  }
}

/**
 * 移除模组对话框显隐状态
 * 由 RemoveModDialog 组件通过 v-model 双向绑定
 */
const removeDialogVisible = ref(false)
/** 待移除模组名称（传入对话框显示） */
const removeDialogModName = ref('')
/** 待移除模组路径（传入对话框调用后端） */
const removeDialogModPath = ref('')

/**
 * 移除模组（NRMM 对齐：移至 DISABLED_MANAGED_REMOVED + 还原 INI）
 * 打开自定义确认对话框（UI1），后续流程由 RemoveModDialog 组件接管：
 * - 用户确认 → 调用后端 remove_mod → 显示成功提示（UI2）
 * - UI2 确认 → 触发 handleRemoved 重读模组
 */
function handleDelete() {
  if (!mod.value) return
  closeContextMenu()
  removeDialogModName.value = mod.value.modName
  removeDialogModPath.value = mod.value.modPath
  removeDialogVisible.value = true
}

/**
 * 移除模组成功后回调（由 RemoveModDialog 的 UI2 确认按钮触发）
 * 执行模组重读取以反映文件夹移动后的目录结构变化
 */
async function handleRemoved() {
  try {
    await modsStore.refresh()
  } catch (e) {
    logger.error('ModCard', 'refresh after remove failed', e)
  }
}

/** 点击外部区域关闭右键菜单 */
function handleClickOutside() {
  closeContextMenu()
}

onMounted(() => {
  document.addEventListener('click', handleClickOutside)
  // 挂载时观察图片容器，触发懒加载
  if (imageContainerRef.value && iconUrl.value) {
    imageLazyLoad.observeElement(props.modIndex, imageContainerRef.value)
  }
})

onUnmounted(() => {
  document.removeEventListener('click', handleClickOutside)
  // 卸载时取消观察图片容器
  if (imageContainerRef.value) {
    imageLazyLoad.unobserveElement(imageContainerRef.value)
  }
})
</script>

<style scoped>
.mod-card {
  width: 160px;
  height: 240px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  cursor: pointer;
  transition: transform 0.15s ease;
  position: relative;
}

/* 悬停：轻微放大 + 白色边框提示当前位置 */
.mod-card:hover:not(.no-scale):not(.is-empty) {
  transform: scale(1.02);
}
.mod-card:hover:not(.no-scale):not(.is-empty) .card-image {
  border-color: #ffffff;
}

/* 已启用且选中：蓝色边框 + 轻微光晕（降低阴影半径和尺寸，减少 GPU 负担） */
.mod-card.is-active.is-selected .card-image {
  border-color: #4a9eff;
  box-shadow: 0 0 8px rgba(74, 158, 255, 0.28);
}
/* 优先级保证：启用并选中的蓝色边框不应被 hover 的白色覆盖 */
.mod-card.is-active.is-selected:hover .card-image {
  border-color: #4a9eff;
  box-shadow: 0 0 8px rgba(74, 158, 255, 0.28);
}

.mod-card:active:not(.no-scale) {
  transform: scale(0.96);
}

/* 右键菜单打开时禁用:active缩放，防止transform导致已teleport的fixed菜单视觉跳动 */
.mod-card.no-scale:active {
  transform: none;
}

.mod-card:focus-visible {
  outline: 2px solid var(--accent-primary);
  outline-offset: 2px;
  border-radius: 8px;
}

.card-image {
  width: 100%;
  height: 100%;
  border-radius: 8px;
  overflow: hidden;
  position: relative;
  border: 3px solid #555;
  background: rgba(255, 255, 255, 0.03);
  transition: border-color 0.2s ease, box-shadow 0.2s ease, opacity 0.2s ease;
}

/* 启用（未选中）：淡蓝色边框提示已启用状态 */
.mod-card.is-active:not(.is-selected) .card-image {
  border-color: rgba(74, 158, 255, 0.5);
  box-shadow: none;
}
/* 启用未选中 + hover：保持淡蓝优先于白色 */
.mod-card.is-active:not(.is-selected):hover .card-image {
  border-color: rgba(74, 158, 255, 0.75);
}

/* 禁用状态：红色边框（优先级高于 hover） */
.mod-card.is-disabled .card-image {
  border-color: #e74c3c;
  opacity: 0.7;
}
.mod-card.is-disabled:hover .card-image {
  border-color: #e74c3c;
  opacity: 0.8;
}

/* 空槽位：虚线边框 */
.mod-card.is-empty .card-image {
  border: 3px dashed #555;
  background: transparent;
}

/* 空槽位激活状态：淡蓝色实线边框（高于 hover 白色） */
.mod-card.is-empty.is-active .card-image {
  border: 3px solid rgba(74, 158, 255, 0.5);
  background: transparent;
}
.mod-card.is-empty.is-active:hover .card-image {
  border: 3px solid rgba(74, 158, 255, 0.75);
}

/* 错误状态：橙色边框（优先级高于 hover 白色） */
.mod-card.is-error .card-image {
  border-color: #f39c12;
}
.mod-card.is-error:hover .card-image {
  border-color: #f5b25a;
}

.mod-image {
  width: 100%;
  height: 100%;
  object-fit: cover;
  border-radius: 8px;
  display: block;
}

/* 图片懒加载容器：需要 relative 定位以容纳绝对定位的隐藏图片 */
.mod-image-wrapper {
  width: 100%;
  height: 100%;
  position: relative;
}

/* 骨架屏脉冲动画 */
.skeleton-pulse {
  animation: skeleton-pulse 1.5s ease-in-out infinite;
}

@keyframes skeleton-pulse {
  0%, 100% { opacity: 0.3; }
  50% { opacity: 0.6; }
}

/* 图片淡入动画 */
.mod-image-fade-in {
  animation: fade-in 0.3s ease-out;
}

@keyframes fade-in {
  from { opacity: 0; }
  to { opacity: 1; }
}

/* 加载中隐藏图片：visibility hidden 保持布局并触发浏览器加载 */
.mod-image-hidden {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  visibility: hidden;
  pointer-events: none;
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
  z-index: 10000;
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
