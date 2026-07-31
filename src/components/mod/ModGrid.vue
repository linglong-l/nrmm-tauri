<template>
  <div class="mod-grid-container">
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
      <!-- 模组卡片网格：每行6个，末尾填充空槽位对齐 -->
      <div v-else class="mod-grid-row">
        <!-- 模组卡片：遍历displayMods渲染 -->
        <ModCard v-for="(mod, index) in displayMods" :key="mod.modPath || index" :mod="mod" :mod-index="index"
          :class="{ 'search-highlight': isModHighlighted(mod, index) }" :ref="el => setModRef(el, index)"
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
import { ref, computed, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { FolderOpened } from '@element-plus/icons-vue'
import ModCard from './ModCard.vue'
import SearchBar from '@/components/common/SearchBar.vue'
import { useModsStore } from '@/stores/mods'
import { useDragScroll } from '@/composables/useDragScroll'
import type { ModData } from '@/types'

const { t } = useI18n()
const modsStore = useModsStore()

/** 网格内容区DOM引用，用于拖拽滚动 */
const gridContentRef = ref<HTMLElement | null>(null)
useDragScroll(gridContentRef)

/** 搜索栏是否可见 */
const searchVisible = ref(false)
/** 搜索关键词 */
const searchQuery = ref('')
/** 当前匹配项索引 */
const currentMatchIndex = ref(0)
/** 模组卡片DOM引用数组，用于滚动到匹配项 */
const modRefs = ref<(HTMLElement | null)[]>([])
/** 搜索匹配的模组索引列表 */
const matchIndices = ref<number[]>([])

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
 * 每行6个卡片，计算末尾需要填充多少空槽位保持对齐
 */
const emptySlots = computed(() => {
  const count = displayMods.value.length
  const slotsPerRow = 6
  const remainder = count % slotsPerRow
  if (remainder === 0) return 0
  return slotsPerRow - remainder
})

/** 总匹配数 */
const totalMatches = computed(() => matchIndices.value.length)

/**
 * 子序列模糊匹配算法
 * 判断query中的字符是否按顺序出现在text中（不要求连续）
 * 例如："abc" 匹配 "aabbcc"、"axbxc"，但不匹配 "acb"
 * @param text 待匹配文本（模组名）
 * @param query 搜索关键词
 * @returns 是否匹配
 */
function fuzzyMatch(text: string, query: string): boolean {
  if (!query) return false
  const lowerText = text.toLowerCase()
  const lowerQuery = query.toLowerCase()
  let queryIdx = 0
  for (let i = 0; i < lowerText.length && queryIdx < lowerQuery.length; i++) {
    if (lowerText[i] === lowerQuery[queryIdx]) {
      queryIdx++
    }
  }
  return queryIdx === lowerQuery.length
}

/**
 * 更新搜索匹配结果
 * 遍历displayMods，用fuzzyMatch找出所有匹配项的索引
 */
function updateMatches() {
  if (!searchQuery.value.trim()) {
    matchIndices.value = []
    currentMatchIndex.value = 0
    return
  }
  const query = searchQuery.value.toLowerCase()
  const matches: number[] = []

  displayMods.value.forEach((mod, idx) => {
    if (fuzzyMatch(mod.modName || '', query)) {
      matches.push(idx)
    }
  })

  // 预留：分组名称匹配逻辑
  modsStore.groups.forEach((group) => {
    if (fuzzyMatch(group.groupName || '', query)) {
    }
  })

  matchIndices.value = matches
  if (currentMatchIndex.value >= matches.length) {
    currentMatchIndex.value = 0
  }
}

/**
 * 判断模组是否为当前高亮的搜索匹配项
 * @param _mod 模组数据
 * @param index 模组在displayMods中的索引
 * @returns 是否高亮
 */
function isModHighlighted(_mod: ModData, index: number): boolean {
  if (!searchQuery.value.trim() || matchIndices.value.length === 0) return false
  return matchIndices.value[currentMatchIndex.value] === index
}

/**
 * 设置模组卡片DOM引用
 * @param el 组件实例
 * @param index 索引
 */
function setModRef(el: any, index: number) {
  if (el) {
    modRefs.value[index] = el.$el
  }
}

/** 滚动到当前匹配的模组卡片 */
function scrollToMatch() {
  if (matchIndices.value.length === 0) return
  const idx = matchIndices.value[currentMatchIndex.value]
  const el = modRefs.value[idx]
  if (el && gridContentRef.value) {
    el.scrollIntoView({ behavior: 'smooth', block: 'center' })
  }
}

/** 切换到下一个匹配项（循环） */
function nextMatch() {
  if (matchIndices.value.length === 0) return
  currentMatchIndex.value = (currentMatchIndex.value + 1) % matchIndices.value.length
  nextTick(scrollToMatch)
}

/** 切换到上一个匹配项（循环） */
function prevMatch() {
  if (matchIndices.value.length === 0) return
  currentMatchIndex.value = (currentMatchIndex.value - 1 + matchIndices.value.length) % matchIndices.value.length
  nextTick(scrollToMatch)
}

/** 关闭搜索栏并清空搜索词 */
function onSearchClose() {
  searchQuery.value = ''
  matchIndices.value = []
}

/** 监听搜索词和模组列表变化，更新匹配结果 */
watch(searchQuery, updateMatches)
watch(displayMods, updateMatches)

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
    searchVisible.value = true
  }
}

onMounted(() => {
  // 监听全局Ctrl+F快捷键
  window.addEventListener('keydown', handleKeydown)
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleKeydown)
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
  border-radius: 8px;
}

/*
 * 空槽位占位：保持网格对齐但不渲染可见元素、不响应任何点击/选中。
 * 宽度与 ModCard (120px) 一致，margin-top 与真实卡片首行 padding-top 对齐保持行高。
 */
.empty-slot-placeholder {
  flex: 0 0 auto;
  width: 120px;
  height: 0;
  min-height: 0;
  pointer-events: none;
  visibility: hidden;
}
</style>
