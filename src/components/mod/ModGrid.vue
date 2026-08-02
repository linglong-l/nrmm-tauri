<template>
  <div class="mod-grid-container" @click="handleContainerClick">
    <!-- 搜索栏：支持模糊搜索、键盘导航 -->
    <SearchBar v-model:visible="searchVisible" v-model="searchQuery" :total-matches="totalMatches"
      :current-index="currentMatchIndex" @next="nextMatch" @prev="prevMatch" @close="onSearchClose" />

    <!-- 模组网格内容区：支持拖拽滚动 -->
    <div ref="gridContentRef" class="grid-content" v-loading="loading" :element-loading-text="t('Loading...')">
      <!-- 空状态：无模组时显示提示 -->
      <div v-if="displayMods.length === 0 && !loading" class="empty-state">
        <el-icon :size="64" class="empty-icon">
          <FolderOpened />
        </el-icon>
        <p class="empty-text">{{ t('mods.noMods') }}</p>
        <p class="empty-hint">{{ t('Drag & Drop mod folders here to add mods to this group (1 folder = 1 mod).') }}</p>
      </div>

      <!-- 虚拟行渲染模式（模组数 > 阈值） -->
      <template v-else-if="virtualEnabled">
        <!-- 上方占位：维持滚动条高度 -->
        <div :style="{ height: spacerTop + 'px' }" aria-hidden="true"></div>
        <!-- 可见行：每行渲染 columnCount 个卡片 -->
        <div
          v-for="rowIdx in visibleRowCount"
          :key="'row-' + (virtualStartRow + rowIdx - 1)"
          class="mod-grid-row virtual-row"
        >
          <ModCard
            v-for="colIdx in columnCount"
            :key="'card-' + ((virtualStartRow + rowIdx - 1) * columnCount + colIdx - 1)"
            :mod="getModAtRowCol(virtualStartRow + rowIdx - 1, colIdx - 1)"
            :mod-index="(virtualStartRow + rowIdx - 1) * columnCount + colIdx - 1"
            :class="{
              'search-highlight': isModHighlightedByIndex((virtualStartRow + rowIdx - 1) * columnCount + colIdx - 1) === 'active',
              'search-hit': isModHighlightedByIndex((virtualStartRow + rowIdx - 1) * columnCount + colIdx - 1) === 'hit'
            }"
            :ref="el => setModRef(el, (virtualStartRow + rowIdx - 1) * columnCount + colIdx - 1)"
            @select="handleHighlightMod"
            @activate="handleActivateMod"
          />
        </div>
        <!-- 下方占位：维持滚动条高度 -->
        <div :style="{ height: spacerBottom + 'px' }" aria-hidden="true"></div>
      </template>

      <!-- 全量渲染模式（模组数 ≤ 阈值） -->
      <div v-else class="mod-grid-row">
        <!-- 模组卡片：遍历displayMods渲染 -->
        <ModCard v-for="(mod, index) in displayMods" :key="mod.modPath || index" :mod="mod" :mod-index="index"
          :class="{ 'search-highlight': isModHighlighted(mod, index) === 'active', 'search-hit': isModHighlighted(mod, index) === 'hit' }" :ref="el => setModRef(el, index)"
          @select="handleHighlightMod" @activate="handleActivateMod" />
        <!-- 空槽位占位：仅占用网格宽度保持对齐，不渲染、不可点击、不可交互 -->
        <div v-for="i in emptySlots" :key="'empty-' + i" class="empty-slot-placeholder" aria-hidden="true"></div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 模组网格组件
 * 右侧主内容区，展示当前分组的模组卡片网格
 * 功能：
 * - 模糊搜索（子序列匹配算法）+ 键盘导航（Enter/Shift+Enter切换）
 * - Ctrl+F快捷键显示搜索栏
 * - 空槽位填充保持网格对齐
 * - 拖拽滚动
 * - 收藏过滤
 */
import { ref, computed, onMounted, onUnmounted, nextTick, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { FolderOpened } from '@element-plus/icons-vue'
import ModCard from './ModCard.vue'
import SearchBar from '@/components/common/SearchBar.vue'
import { useModsStore } from '@/stores/mods'
import { useDragScroll } from '@/composables/useDragScroll'
import { useVirtualGrid } from '@/composables/useVirtualGrid'
import type { ModData } from '@/types'

const { t } = useI18n()
const modsStore = useModsStore()

/** 网格内容区DOM引用，用于拖拽滚动 */
const gridContentRef = ref<HTMLElement | null>(null)
/** 容器尺寸变化观察器，用于自动调整列数 */
let resizeObserver: ResizeObserver | null = null
useDragScroll(gridContentRef)

/**
 * 根据容器宽度计算每行可容纳的卡片数
 * 卡片宽度 160px，间距 12px
 * 公式：cols = floor((width + gap) / (cardWidth + gap))
 */
const columnCount = ref(5)

function updateColumnCount() {
  if (!gridContentRef.value) return
  const width = gridContentRef.value.clientWidth
  const cardWidth = 160
  const gap = 12
  const cols = Math.floor((width + gap) / (cardWidth + gap))
  columnCount.value = Math.max(1, Math.min(6, cols))
}

/** 虚拟行渲染 */
const totalCount = computed(() => displayMods.value.length)
const {
  startIndex: virtualStartIndex,
  endIndex: virtualEndIndex,
  spacerTop,
  spacerBottom,
  enabled: virtualEnabled
} = useVirtualGrid(totalCount, gridContentRef, { columnCount })

/** 虚拟行起始行号 */
const virtualStartRow = computed(() => Math.floor(virtualStartIndex.value / columnCount.value))
/** 可见行数 */
const visibleRowCount = computed(() => {
  const endRow = Math.ceil(virtualEndIndex.value / columnCount.value)
  return Math.max(0, endRow - virtualStartRow.value)
})

/** 从 store 读取全局搜索状态 */
const searchVisible = computed(() => modsStore.searchVisible)
const searchQuery = computed({
  get: () => modsStore.searchQuery,
  set: (v: string) => { modsStore.searchQuery = v }
})
const totalMatches = computed(() => modsStore.groupMatchPaths.length + modsStore.modMatchIndices.length)
const currentMatchIndex = computed(() => modsStore.currentGlobalMatchIndex)

/** 模组卡片DOM引用数组，用于滚动到匹配项 */
const modRefs = ref<(HTMLElement | null)[]>([])

/** 加载状态 */
const loading = computed(() => modsStore.loading)
/** 是否仅显示收藏 */
const showFavoritesOnly = computed(() => modsStore.showFavoritesOnly)

/**
 * 显示的模组列表
 * 收藏模式：显示所有收藏模组
 * 普通模式：显示当前分组的模组（使用currentGroupMods，MutexGroup递归收集子分组模组）
 */
const displayMods = computed<ModData[]>(() => {
  if (showFavoritesOnly.value) {
    return modsStore.mods.filter(m => m.isFavorite)
  }
  return modsStore.currentGroupMods
})

/**
 * 空槽位数量计算
 * 根据动态列数计算末尾需要填充多少空槽位保持对齐
 */
const emptySlots = computed(() => {
  const count = displayMods.value.length
  const slotsPerRow = columnCount.value
  const remainder = count % slotsPerRow
  if (remainder === 0) return 0
  return slotsPerRow - remainder
})

/**
 * 获取指定行列的模组数据
 * @param row 行号（0-based）
 * @param col 列号（0-based）
 * @returns 模组数据，越界返回 undefined
 */
function getModAtRowCol(row: number, col: number): ModData | undefined {
  const idx = row * columnCount.value + col
  return displayMods.value[idx]
}

/**
 * 通过 displayMods 索引判断模组卡片是否为搜索命中项
 * 用于虚拟行渲染模式（直接使用索引避免遍历）
 * 两种情况命中高亮：
 *   1. 当前全局聚焦索引正好指向该模组（强高亮发光边框）
 *   2. 该模组在命中列表中但非当前项（淡色标记边框）
 */
function isModHighlightedByIndex(displayIdx: number): 'active' | 'hit' | false {
  const q = searchQuery.value?.trim()
  if (!q) return false
  const mod = displayMods.value[displayIdx]
  if (!mod || !mod.modPath) return false
  const hit = modsStore.modMatchIndices.some(i => modsStore.mods[i]?.modPath === mod.modPath)
  if (!hit) return false
  const info = modsStore.getCurrentMatchInfo()
  if (!info.isGroup && info.modFlatIdx !== undefined) {
    const focusedMod = modsStore.mods[info.modFlatIdx]
    if (focusedMod?.modPath === mod.modPath) return 'active'
  }
  return 'hit'
}

/**
 * 当前模组卡片是否为搜索命中项
 * 两种情况命中高亮：
 *   1. 当前全局聚焦索引正好指向该模组（强高亮发光边框）
 *   2. 该模组在命中列表中但非当前项（淡色标记边框）
 */
function isModHighlighted(_mod: ModData, index: number): 'active' | 'hit' | false {
  const q = searchQuery.value?.trim()
  if (!q) return false

  const mod = displayMods.value[index]
  if (!mod || !mod.modPath) return false

  const hit = modsStore.modMatchIndices.some(i => modsStore.mods[i]?.modPath === mod.modPath)
  if (!hit) return false

  const info = modsStore.getCurrentMatchInfo()
  if (!info.isGroup && info.modFlatIdx !== undefined) {
    const focusedMod = modsStore.mods[info.modFlatIdx]
    if (focusedMod?.modPath === mod.modPath) return 'active'
  }
  return 'hit'
}

/**
 * 设置模组卡片DOM引用
 * 虚拟行渲染模式下 ref 可能稀疏，故使用 el.$el 安全获取 DOM 元素
 * @param el 组件实例
 * @param index 索引
 */
function setModRef(el: any, index: number) {
  if (el) {
    modRefs.value[index] = el.$el || el
  }
}

function scrollToMatch() {
  const info = modsStore.getCurrentMatchInfo()
  if (info.isGroup) {
    modsStore.selectGroupByPath(info.groupPath!)
    if (gridContentRef.value) gridContentRef.value.scrollTop = 0
    return
  }
  if (info.modFlatIdx !== undefined) {
    const focusedMod = modsStore.mods[info.modFlatIdx]
    if (!focusedMod) return
    const displayIdx = displayMods.value.findIndex(m => m.modPath === focusedMod.modPath)
    if (displayIdx < 0) {
      modsStore.selectGroupContainingMod(focusedMod.modPath)
      nextTick(() => {
        const idx2 = displayMods.value.findIndex(m => m.modPath === focusedMod.modPath)
        if (idx2 >= 0) scrollDisplayIndex(idx2)
      })
      return
    }
    scrollDisplayIndex(displayIdx)
  }
}
function scrollDisplayIndex(displayIdx: number) {
  const el = modRefs.value[displayIdx]
  if (el && gridContentRef.value) {
    el.scrollIntoView({ behavior: 'smooth', block: 'center' })
  }
}

function nextMatch() { modsStore.nextSearchMatch(); nextTick(scrollToMatch) }
function prevMatch() { modsStore.prevSearchMatch(); nextTick(scrollToMatch) }
function onSearchClose() { modsStore.setSearchVisible(false); modsStore.clearSearch() }

/**
 * 点击容器区域（非搜索框）时关闭搜索框
 * 搜索框自身点击通过 stopPropagation 阻止冒泡，不触发关闭
 */
function handleContainerClick() {
  if (modsStore.searchVisible) {
    modsStore.setSearchVisible(false)
    modsStore.clearSearch()
  }
}

/**
 * 单击高亮模组（仅更新UI选中状态，不写入INI）
 * @param modIndex 模组在当前显示列表中的索引
 */
function handleHighlightMod(modIndex: number) {
  modsStore.highlightMod(modIndex)
}

/**
 * 双击启用模组（调用后端写入INI）
 * @param modIndex 模组在当前显示列表中的索引
 */
function handleActivateMod(modIndex: number) {
  modsStore.activateModByIndex(modIndex)
}

/**
 * 全局键盘事件：Ctrl+F显示搜索栏
 * @param e 键盘事件
 */
function handleKeydown(e: KeyboardEvent) {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') {
    e.preventDefault()
    modsStore.setSearchVisible(true)
  }
}

/**
 * 确保当前选中的模组卡片可见（滚动到视口内）
 * 边界条件：
 * - 无模组 / 无选中索引 → 忽略
 * - 选中索引越界 → 忽略
 * - 卡片已在可见区域 → 不滚动（避免抖动）
 */
function ensureSelectedCardVisible() {
  if (!gridContentRef.value) return
  const count = displayMods.value.length
  if (count === 0) return
  const idx = modsStore.selectedModIndex
  if (idx < 0 || idx >= count) return

  const cardEl = modRefs.value[idx] as HTMLElement | undefined
  if (!cardEl) {
    // DOM 尚未渲染（常见于刚切换分组或刚加载完），稍后重试一次
    nextTick(() => {
      const el = modRefs.value[idx] as HTMLElement | undefined
      if (el && gridContentRef.value) {
        scrollIntoViewIfNeeded(el, gridContentRef.value)
      }
    })
    return
  }
  scrollIntoViewIfNeeded(cardEl, gridContentRef.value)
}

/**
 * 仅当目标元素不在可见区域内时才滚动
 * @param target 目标DOM元素
 * @param container 滚动容器DOM元素
 */
function scrollIntoViewIfNeeded(target: HTMLElement, container: HTMLElement) {
  const containerRect = container.getBoundingClientRect()
  const targetRect = target.getBoundingClientRect()
  const isAbove = targetRect.top < containerRect.top
  const isBelow = targetRect.bottom > containerRect.bottom
  if (isAbove || isBelow) {
    target.scrollIntoView({ behavior: 'smooth', block: 'center' })
  }
}

onMounted(() => {
  // 监听全局Ctrl+F快捷键
  window.addEventListener('keydown', handleKeydown)
  // 计算初始列数
  updateColumnCount()
  // 监听容器尺寸变化，自动调整列数
  if (gridContentRef.value) {
    resizeObserver = new ResizeObserver(() => {
      updateColumnCount()
    })
    resizeObserver.observe(gridContentRef.value)
  }
  // 首次挂载：确保选中卡片可见
  nextTick(ensureSelectedCardVisible)
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleKeydown)
  // 清理 ResizeObserver
  if (resizeObserver) {
    resizeObserver.disconnect()
    resizeObserver = null
  }
})

/**
 * 数据加载完成后 → 确保选中卡片可见
 * 监听 loading 从 true → false 的切换（loadMods / refresh 完成）
 */
watch(loading, (newVal, oldVal) => {
  if (oldVal === true && newVal === false) {
    nextTick(ensureSelectedCardVisible)
  }
})

/**
 * 显示模组列表或选中分组变化后 → 确保选中卡片可见
 * 触发场景：分组切换（displayMods 重建）、收藏模式切换、模组数量变化
 */
watch(
  () => [displayMods.value.length, modsStore.selectedGroupPath, modsStore.showFavoritesOnly],
  () => {
    if (!loading.value) {
      nextTick(ensureSelectedCardVisible)
    }
  }
)
</script>

<style scoped>
.mod-grid-container {
  flex: 1;
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  position: relative;
}

.grid-content {
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 0 16px 16px;
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.grid-content::-webkit-scrollbar {
  display: none;
}

.mod-grid-row {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  padding-top: 8px;
}

/* 虚拟行模式：每行固定 6 个卡片，不换行，无顶部 padding（spacerTop 已包含偏移） */
.mod-grid-row.virtual-row {
  flex-wrap: nowrap;
  padding-top: 0;
}

.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  min-height: 300px;
  color: var(--text-muted);
}

.empty-icon {
  opacity: 0.4;
  margin-bottom: 16px;
}

.empty-text {
  font-size: 16px;
  font-weight: 500;
  margin: 0 0 8px;
  color: var(--text-secondary);
}

.empty-hint {
  font-size: 13px;
  text-align: center;
  max-width: 400px;
  margin: 0;
  line-height: 1.5;
}

.search-highlight {
  border: 2px solid #f5c35a !important;
  box-shadow: 0 0 14px rgba(245, 195, 90, 0.6);
  border-radius: 8px;
  transition: all 0.2s ease;
}
.search-hit {
  border: 2px dashed rgba(245, 195, 90, 0.45);
  border-radius: 8px;
  transition: all 0.2s ease;
}

/*
 * 空槽位占位：保持网格对齐但不渲染可见元素、不响应任何点击/选中。
 * 宽度与 ModCard (160px) 一致，不可见元素仅用于对齐布局。
 */
.empty-slot-placeholder {
  flex: 0 0 auto;
  width: 160px;
  height: 0;
  min-height: 0;
  pointer-events: none;
  visibility: hidden;
}
</style>
