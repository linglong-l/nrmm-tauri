<template>
  <div class="mod-grid-container" @click="handleContainerClick">
    <!-- 搜索栏：支持模糊搜索、键盘导航 -->
    <SearchBar v-model:visible="searchVisible" :model-value="searchQuery" @update:model-value="updateSearchQuery" :total-matches="totalMatches"
      :current-index="currentMatchIndex" @next="nextMatch" @prev="prevMatch" @close="onSearchClose" :disabled="modsStore.isUpdatingModData" />

    <!-- 模组网格内容区：支持拖拽滚动 -->
    <div ref="gridContentRef" class="grid-content">
      <!-- B-2 状态链：加载错误 → 加载骨架 → 空态 → 虚拟/全量网格（错误优先） -->
      <div v-if="displayMods.length === 0 && modsStore.loadError" class="empty-state">
        <!-- 加载失败：显示错误信息+重试（C18，避免误导性的「无模组」提示） -->
        <div class="load-error-state">
          <p class="error-title">{{ t('Mods load failed') }}</p>
          <p class="error-detail">{{ modsStore.loadError }}</p>
          <el-button size="small" @click="modsStore.loadMods()">{{ t('Retry') }}</el-button>
        </div>
      </div>

      <!-- B-2 数据加载骨架：loading 且无数据时，渲染与真实卡片同尺寸（160×240）对齐的骨架网格 -->
      <div v-else-if="displayMods.length === 0 && loading" class="skeleton-grid" aria-busy="true">
        <div v-for="row in 3" :key="'sk-row-' + row" class="mod-grid-row skeleton-row">
          <div v-for="col in columnCount" :key="'sk-' + row + '-' + col" class="skeleton-card">
            <div class="sk-img skeleton-pulse"></div>
            <div class="sk-name skeleton-pulse"></div>
          </div>
        </div>
        <span class="visually-hidden">{{ t('Loading mods...') }}</span>
      </div>

      <!-- 空状态：无模组时显示提示 -->
      <div v-else-if="displayMods.length === 0 && !loading" class="empty-state">
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
        <!-- 可见行：行内仅渲染本行真实存在的卡片（A-2 行内列数剪裁，杜绝 undefined 空卡/错位行） -->
        <div
          v-for="rowIdx in visibleRowCount"
          :key="'row-' + (virtualStartRow + rowIdx - 1)"
          class="mod-grid-row virtual-row"
        >
          <ModCard
            v-for="colIdx in rowColCount(displayMods.length, columnCount, virtualStartRow + rowIdx - 1)"
            :key="'card-' + virtualCardIndex(virtualStartRow + rowIdx - 1, colIdx - 1)"
            :mod="getModAtRowCol(virtualStartRow + rowIdx - 1, colIdx - 1)"
            :isEmpty-slot="false"
            :mod-index="virtualCardIndex(virtualStartRow + rowIdx - 1, colIdx - 1)"
            :class="virtualCardClasses(virtualCardIndex(virtualStartRow + rowIdx - 1, colIdx - 1))"
            :ref="el => setModRef(el, virtualCardIndex(virtualStartRow + rowIdx - 1, colIdx - 1))"
            @select="handleHighlightMod"
            @activate="handleActivateMod"
          />
          <!-- A-2 对齐补位：行内真实卡之后的 CSS 占位（不渲染 ModCard），与全量模式空槽布局一致 -->
          <div
            v-for="i in (columnCount - rowColCount(displayMods.length, columnCount, virtualStartRow + rowIdx - 1))"
            :key="'empty-' + rowIdx + '-' + i"
            class="empty-slot-placeholder"
            aria-hidden="true"
          ></div>
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

    <!-- B-2 刷新遮罩：loading 且有旧数据时半透明视觉提醒，pointer-events:none 不拦截交互 -->
    <div v-if="loading && displayMods.length > 0" class="is-refreshing-mask" aria-hidden="true"></div>
  </div>
</template>

<!-- A-1/A-2 纯计算辅助函数：独立导出以便单测 -->
<script lang="ts">
/**
 * 计算某行实际应渲染的卡片数（A-2：行内列数剪裁，杜绝渲染越界空卡/错位行）
 * @param total 当前显示模组总数
 * @param cols 每行列数
 * @param row 行号（0-based）
 * @returns 该行应渲染的卡片数（首行满列 / 末行取余数 / 空行为 0）
 */
export function rowColCount(total: number, cols: number, row: number): number {
  if (cols <= 0) return 0
  const start = row * cols
  if (start >= total) return 0
  return Math.min(cols, total - start)
}

/**
 * 收敛滚动容器 scrollTop 至内容最大可滚位置（A-1：patch 缩短列表后防指向空区）
 * @param el 滚动容器 DOM（仅需 scrollHeight/clientHeight/scrollTop 三属性，便于纯函数单测）
 */
export function clampScrollTop(el: { scrollHeight: number; clientHeight: number; scrollTop: number }): void {
  const maxScroll = Math.max(0, el.scrollHeight - el.clientHeight)
  if (el.scrollTop > maxScroll) el.scrollTop = maxScroll
}
</script>

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
import { ref, computed, onMounted, onUnmounted, nextTick, watch, inject } from 'vue'
import type { Ref, ComponentPublicInstance } from 'vue'
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

/**
 * 模组页焦点令牌（由 App.vue provide）
 * 窗口显示或切回模组页时自增，触发选中模组卡片滚动到可视范围
 */
const modsFocusTick = inject<Ref<number>>('modsFocusTick', ref(0))

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
/** 卡片尺寸与网格间距（需与 CSS .mod-card 宽度及网格 gap 保持一致） */
const CARD_WIDTH = 160
const CARD_GAP = 12
const MIN_COLUMNS = 1
const MAX_COLUMNS = 6
/** 虚拟行高度与顶部偏移（需与 useVirtualGrid 默认 rowHeight=252 / topOffset=8 保持一致） */
const VIRTUAL_ROW_HEIGHT = 252
const VIRTUAL_TOP_OFFSET = 8
const columnCount = ref(5)

function updateColumnCount() {
  if (!gridContentRef.value) return
  const width = gridContentRef.value.clientWidth
  const cols = Math.floor((width + CARD_GAP) / (CARD_WIDTH + CARD_GAP))
  columnCount.value = Math.max(MIN_COLUMNS, Math.min(MAX_COLUMNS, cols))
}

/** 虚拟行渲染 */
const totalCount = computed(() => displayMods.value.length)
const {
  startIndex: virtualStartIndex,
  endIndex: virtualEndIndex,
  spacerTop,
  spacerBottom,
  enabled: virtualEnabled,
  recalculate: virtualRecalculate
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
const searchQuery = computed(() => modsStore.searchQuery)
/** 更新搜索关键词（直接写入 store，避免 computed setter 副作用） */
function updateSearchQuery(v: string) { modsStore.searchQuery = v }
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
 * 虚拟行渲染模式下，根据已减 1 的 0-based 行列基址计算 displayMods 扁平索引
 * 提取自模板中重复 4+ 次的索引表达式，集中维护避免错位
 */
function virtualCardIndex(rowBase: number, colBase: number): number {
  return rowBase * columnCount.value + colBase
}

/**
 * 虚拟行渲染模式下单个卡片的搜索高亮 class（'active' 强高亮 / 'hit' 淡色）
 * 仅调用一次 isModHighlightedByIndex（内部 .some() 一次），替代模板内 2 次独立调用
 */
function virtualCardClasses(idx: number): Record<string, boolean> {
  const status = isModHighlightedByIndex(idx)
  return {
    'search-highlight': status === 'active',
    'search-hit': status === 'hit',
  }
}

/**
 * 设置模组卡片DOM引用
 * 虚拟行渲染模式下 ref 可能稀疏，故使用 el.$el 安全获取 DOM 元素
 * @param el 组件实例
 * @param index 索引
 */
function setModRef(el: Element | ComponentPublicInstance | null, index: number) {
  if (!el) {
    // 卸载回调：清理槽位，防止虚拟滚动 detached DOM 被 modRefs 持有（C7 内存泄漏）
    modRefs.value[index] = null
    return
  }
  // 组件实例取其根 DOM（$el），原生元素直接使用；统一收窄为 HTMLElement
  const domEl: HTMLElement = '$el' in el ? (el.$el as HTMLElement) : (el as HTMLElement)
  modRefs.value[index] = domEl
}

function scrollToMatch() {
  const info = modsStore.getCurrentMatchInfo()
  if (info.isGroup && info.groupPath) {
    modsStore.selectGroupByPath(info.groupPath)
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
  // 同时传递 modPath 用于定位：收藏模式下 displayMods（无 None、0-based）与
  // currentGroupMods（含 None 槽位、真实模组 1-based）下标错位，
  // 仅传下标会选错模组/误传 mod_index=0；以 modPath 定位可保证后端拿到真实分组下标。
  const mod = displayMods.value[modIndex]
  modsStore.activateModByIndex(modIndex, mod?.modPath)
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
    // 虚拟滚动模式下，选中模组卡片可能尚未渲染（不在当前可见行内）。
    // 此时无法 scrollIntoView，需先将容器滚动到目标行触发虚拟行渲染，再精确定位。
    if (virtualEnabled.value) {
      scrollToSelectedRow(idx)
      return
    }
    // 全量渲染下 DOM 尚未渲染（常见于刚切换分组或刚加载完），稍后重试一次
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
 * 虚拟滚动模式下，将容器滚动到选中模组所在行，触发虚拟行渲染出该卡片
 * 行高与顶部偏移需与 useVirtualGrid 默认值保持一致（252px / 8px）
 * @param idx 选中模组索引
 */
function scrollToSelectedRow(idx: number) {
  if (!gridContentRef.value) return
  const rowHeight = VIRTUAL_ROW_HEIGHT
  const topOffset = VIRTUAL_TOP_OFFSET
  const targetRow = Math.floor(idx / columnCount.value)
  const targetTop = topOffset + targetRow * rowHeight
  const containerHeight = gridContentRef.value.clientHeight
  // 目标行垂直居中于视口
  const targetScrollTop = Math.max(0, targetTop - (containerHeight - rowHeight) / 2)
  gridContentRef.value.scrollTop = targetScrollTop
  // 同步刷新虚拟网格可见范围，触发虚拟行按新 scrollTop 重渲染
  virtualRecalculate()
  // 等待虚拟行渲染完成后，若卡片已渲染则精确定位到居中
  nextTick(() => {
    const el = modRefs.value[idx] as HTMLElement | undefined
    if (el && gridContentRef.value) {
      scrollIntoViewIfNeeded(el, gridContentRef.value)
    }
  })
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
 * 显示模组列表或选中分组变化后 → 收敛滚动+重算可见范围 → 确保选中卡片可见
 * 触发场景：分组切换（displayMods 重建）、收藏模式切换、模组数量变化
 *
 * A-1 异常渲染修复：列表变化（命令 patch/分组建树/收藏切换/refresh）后先收敛
 * scrollTop（防 patch 缩短列表后指向空区）再重算可见范围，杜绝渲染不该渲染的行/空卡。
 */
watch(
  () => [displayMods.value.length, modsStore.selectedGroupPath, modsStore.showFavoritesOnly],
  () => {
    if (!loading.value) {
      clampScrollToContent()
      virtualRecalculate()
      nextTick(ensureSelectedCardVisible)
    }
  }
)

/**
 * A-1：收敛 gridContent 的 scrollTop 至内容最大可滚位置
 * 当命令级 patch 缩短列表（删除模组/移除组/取消收藏）后，scrollTop 可能超过新列表
 * 最大可滚位置，若不收敛，重算后的可见范围仍指向空区。
 */
function clampScrollToContent() {
  const el = gridContentRef.value
  if (el) clampScrollTop(el)
}

/**
 * 焦点回归（窗口显示 / 切回模组页）→ 确保选中模组卡片可见
 */
watch(modsFocusTick, () => {
  if (!loading.value) {
    nextTick(ensureSelectedCardVisible)
  }
})
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

/* 加载失败状态（C18）：样式对齐现有空态 */
.load-error-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  color: var(--text-muted);
}

.error-title {
  font-size: 16px;
  font-weight: 500;
  margin: 0;
  color: var(--danger, #f56c6c);
}

.error-detail {
  font-size: 13px;
  text-align: center;
  max-width: 400px;
  margin: 0;
  line-height: 1.5;
  word-break: break-all;
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

/* B-2 数据加载骨架：与真实 ModCard 同尺寸（160×240）对齐，纯 CSS，复用 skeleton-pulse 呼吸动画 */
.skeleton-grid {
  height: 100%;
  overflow: hidden;
}

.skeleton-row {
  /* 复用 .mod-grid-row 的 flex 包裹与间隙布局（160×240 卡片 + 12px gap） */
}

.skeleton-card {
  flex: 0 0 auto;
  width: 160px;
  height: 240px;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.05);
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

.sk-img {
  width: 100%;
  height: 200px;
  background: rgba(255, 255, 255, 0.08);
  border-radius: 8px 8px 0 0;
}

.sk-name {
  margin: 8px;
  height: 20px;
  border-radius: 4px;
  background: rgba(255, 255, 255, 0.08);
}

/* 骨架呼吸动画（作用域内定义，因 ModCard 的 .skeleton-pulse 是 scoped 无法复用） */
.skeleton-pulse {
  animation: skeleton-pulse 1.5s ease-in-out infinite;
}

@keyframes skeleton-pulse {
  0%, 100% { opacity: 0.3; }
  50% { opacity: 0.6; }
}

/* 无障碍隐藏文本（供屏幕阅读器读取"正在加载模组"） */
.visually-hidden {
  position: absolute;
  width: 1px;
  height: 1px;
  margin: -1px;
  padding: 0;
  overflow: hidden;
  clip: rect(0 0 0 0);
  white-space: nowrap;
  border: 0;
}

/* B-2 刷新遮罩：loading 且有旧数据时的半透明视觉提醒，pointer-events:none 不拦截交互 */
.is-refreshing-mask {
  position: absolute;
  inset: 0;
  background: rgba(18, 18, 18, 0.4);
  pointer-events: none;
  transition: opacity 0.2s ease;
}
</style>
