import { defineStore } from 'pinia'
import { ref } from 'vue'
import { getSettings, saveSettings } from '../utils/tauri'
import { logger } from '../utils/logger'
import type { AppSettings, PlatformInfo, TargetGame } from '../types'

export const useSettingsStore = defineStore('settings', () => {
  const settings = ref<Partial<AppSettings>>({})
  const platformInfo = ref<PlatformInfo | null>(null)
  const currentGame = ref<TargetGame>('GenshinImpact')
  const currentModsPath = ref('')
  const loaded = ref(false)

  async function load() {
    try {
      settings.value = await getSettings()
      if (settings.value.targetGame) {
        currentGame.value = settings.value.targetGame
      }
      if (settings.value.gameModsPath && settings.value.targetGame) {
        currentModsPath.value = settings.value.gameModsPath[settings.value.targetGame] || ''
      }
      loaded.value = true
    } catch (e) {
      console.error('Failed to load settings:', e)
    }
  }

  async function save() {
    try {
      await saveSettings(settings.value)
    } catch (e) {
      logger.error('SettingsStore', 'Failed to save settings', e)
    }
  }

  function updateGame(game: TargetGame, path: string) {
    currentGame.value = game
    currentModsPath.value = path
  }

  return {
    settings,
    platformInfo,
    currentGame,
    currentModsPath,
    loaded,
    load,
    save,
    updateGame,
  }
})
