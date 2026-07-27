<template>
  <div class="mod-grid-container">
    <SearchBar
      v-model:visible="searchVisible"
      v-model="searchQuery"
      :total-matches="totalMatches"
      :current-index="currentMatchIndex"
      @next="nextMatch"
      @prev="prevMatch"
      @close="onSearchClose"
    />

    <div ref="gridContentRef" class="grid-content" v-loading="loading" :element-loading-text="t('Loading...')">
      <div v-if="displayMods.length === 0 && !loading" class="empty-state">
        <el-icon :size="64" class="empty-icon"><FolderOpened /></el-icon>
        <p class="empty-text">{{ t('mods.noMods') }}</p>
        <p class="empty-hint">{{ t('Drag & Drop mod folders here to add mods to this group (1 folder = 1 mod).') }}</p>
      </div>
      <div v-else class="mod-grid-row">
        <ModCard
          v-for="(mod, index) in displayMods"
          :key="mod.modPath || index"
          :mod="mod"
          :group-index="mod._groupIndex || currentGroupIndex"
          :mod-index="mod._modIndex || index"
          :class="{ 'search-highlight': isModHighlighted(mod, index) }"
          :ref="el => setModRef(el, index)"
          @select="handleSelectMod"
        />
        <ModCard
          v-for="i in emptySlots"
          :key="'empty-' + i"
          :group-index="currentGroupIndex"
          :mod-index="-1"
          is-empty-slot
        />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
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

const gridContentRef = ref<HTMLElement | null>(null)
useDragScroll(gridContentRef)

const searchVisible = ref(false)
const searchQuery = ref('')
const currentMatchIndex = ref(0)
const modRefs = ref<(HTMLElement | null)[]>([])
const matchIndices = ref<number[]>([])

const loading = computed(() => modsStore.loading)
const showFavoritesOnly = computed(() => modsStore.showFavoritesOnly)
const currentGroupIndex = computed(() => modsStore.selectedGroupIndex)

const displayMods = computed<(ModData & { _groupIndex?: number; _modIndex?: number })[]>(() => {
  if (showFavoritesOnly.value) {
    return modsStore.mods
      .filter(m => m.isFavorite)
      .map((m, idx) => ({ ...m, _groupIndex: m.modIndex, _modIndex: idx }))
  }
  const group = modsStore.groups[currentGroupIndex.value]
  if (!group) return modsStore.filteredMods
  const modsInGroup = modsStore.mods.filter((_, idx) => {
    return group.mods.some(gm => gm.modPath === modsStore.mods[idx].modPath)
  })
  return modsInGroup
})

const emptySlots = computed(() => {
  const count = displayMods.value.length
  const slotsPerRow = 6
  const remainder = count % slotsPerRow
  if (remainder === 0) return 0
  return slotsPerRow - remainder
})

const totalMatches = computed(() => matchIndices.value.length)

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

  modsStore.groups.forEach((group) => {
    if (fuzzyMatch(group.groupName || '', query)) {
    }
  })

  matchIndices.value = matches
  if (currentMatchIndex.value >= matches.length) {
    currentMatchIndex.value = 0
  }
}

function isModHighlighted(_mod: ModData, index: number): boolean {
  if (!searchQuery.value.trim() || matchIndices.value.length === 0) return false
  return matchIndices.value[currentMatchIndex.value] === index
}

function setModRef(el: any, index: number) {
  if (el) {
    modRefs.value[index] = el.$el
  }
}

function scrollToMatch() {
  if (matchIndices.value.length === 0) return
  const idx = matchIndices.value[currentMatchIndex.value]
  const el = modRefs.value[idx]
  if (el && gridContentRef.value) {
    el.scrollIntoView({ behavior: 'smooth', block: 'center' })
  }
}

function nextMatch() {
  if (matchIndices.value.length === 0) return
  currentMatchIndex.value = (currentMatchIndex.value + 1) % matchIndices.value.length
  nextTick(scrollToMatch)
}

function prevMatch() {
  if (matchIndices.value.length === 0) return
  currentMatchIndex.value = (currentMatchIndex.value - 1 + matchIndices.value.length) % matchIndices.value.length
  nextTick(scrollToMatch)
}

function onSearchClose() {
  searchQuery.value = ''
  matchIndices.value = []
}

watch(searchQuery, updateMatches)
watch(displayMods, updateMatches, { deep: true })

function handleSelectMod(groupIndex: number, modIndex: number) {
  modsStore.selectModByIndex(groupIndex, modIndex)
}

function handleKeydown(e: KeyboardEvent) {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') {
    e.preventDefault()
    searchVisible.value = true
  }
}

onMounted(() => {
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
</style>
