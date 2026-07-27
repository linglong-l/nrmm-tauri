import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { getMods, refreshMods, selectMod } from '../utils/tauri'
import { useSettingsStore } from './settings'
import type { ModGroupData, ModData } from '../types'
import { logger } from '../utils/logger'

export const useModsStore = defineStore('mods', () => {
  const groups = ref<ModGroupData[]>([])
  const mods = ref<ModData[]>([])
  const loading = ref(false)
  const selectedGroupIndex = ref<number>(0)
  const selectedModIndex = ref<number>(0)
  const searchQuery = ref('')
  const showFavoritesOnly = ref(false)

  const currentGroup = computed(() => groups.value[selectedGroupIndex.value] || null)

  const filteredMods = computed(() => {
    let result = mods.value
    if (searchQuery.value) {
      const q = searchQuery.value.toLowerCase()
      result = result.filter((m) => {
        const name = (m.modName || '').toLowerCase()
        return name.includes(q)
      })
    }
    if (showFavoritesOnly.value) {
      result = result.filter((m) => m.isFavorite)
    }
    return result
  })

  async function loadMods() {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    loading.value = true
    try {
      const result = await getMods(s.currentGame, s.currentModsPath)
      groups.value = result.groups || []
      mods.value = result.mods || []
    } catch (e) {
      logger.error('ModsStore', 'Failed to load mods', e)
    } finally {
      loading.value = false
    }
  }

  async function refresh() {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    loading.value = true
    try {
      await refreshMods(s.currentGame, s.currentModsPath)
      await loadMods()
    } catch (e) {
      logger.error('ModsStore', 'Failed to refresh mods', e)
    } finally {
      loading.value = false
    }
  }

  async function selectModByIndex(groupIdx: number, modIdx: number) {
    const s = useSettingsStore()
    if (!s.currentModsPath) return
    try {
      await selectMod(s.currentGame, s.currentModsPath, groupIdx, modIdx)
      selectedGroupIndex.value = groupIdx
      selectedModIndex.value = modIdx
      await loadMods()
    } catch (e) {
      logger.error('ModsStore', 'Failed to select mod', e)
    }
  }

  return {
    groups,
    mods,
    loading,
    selectedGroupIndex,
    selectedModIndex,
    searchQuery,
    showFavoritesOnly,
    currentGroup,
    filteredMods,
    loadMods,
    refresh,
    selectModByIndex,
  }
})
