<template>
  <div id="app" :class="{ 'no-transparency': !supportsTransparency }">
    <TitleBar />
    <div class="nav-container">
      <PillTabs v-model="activeTab" :tabs="tabs" />
    </div>
    <div class="main-content">
      <ModsView v-if="activeTab === 'mods'" />
      <KeybindsView v-else-if="activeTab === 'keybinds'" />
      <SettingsView v-else-if="activeTab === 'settings'" />
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import TitleBar from '@/components/common/TitleBar.vue'
import PillTabs from '@/components/nav/PillTabs.vue'
import ModsView from '@/views/ModsView.vue'
import KeybindsView from '@/views/KeybindsView.vue'
import SettingsView from '@/views/SettingsView.vue'
import { usePlatform } from '@/stores/platform'
import { useSettingsStore } from '@/stores/settings'

const { t } = useI18n()
const { platformInfo } = usePlatform()
const settingsStore = useSettingsStore()

const tabs = [
  { key: 'mods', label: t('Mods') },
  { key: 'keybinds', label: t('Keybinds') },
  { key: 'settings', label: t('Settings') }
]

const activeTab = ref('mods')
const supportsTransparency = ref(true)

onMounted(async () => {
  if (platformInfo.value) {
    supportsTransparency.value = platformInfo.value.transparencySupported
  }
  await settingsStore.load()
})
</script>

<style>
:root {
  --bg-primary: transparent;
  --bg-secondary: transparent;
  --bg-tertiary: transparent;
  --bg-card: transparent;
  --bg-card-hover: transparent;
  --text-primary: #e8e8e8;
  --text-secondary: #a0a0b0;
  --text-muted: #6c6c7c;
  --accent-primary: #4a9eff;
  --accent-success: #4ade80;
  --accent-warning: #fbbf24;
  --accent-danger: #f87171;
  --border-color: rgba(255, 255, 255, 0.1);
  --border-radius: 8px;
  background-color: transparent;
}

* {
  margin: 0;
  padding: 0;
  box-sizing: border-box;
}

html, body, #app {
  width: 100%;
  height: 100%;
  overflow: hidden;
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
  font-size: 14px;
  background-color: transparent;
  color: var(--text-primary);
  user-select: none;
}

body {
  background: transparent;
}

#app {
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

#app.no-transparency {
  background-color: #121212;
}

html.no-transparency, body.no-transparency {
  background-color: #121212;
}

.nav-container {
  display: flex;
  justify-content: center;
  padding: 8px 0;
  flex-shrink: 0;
}

.main-content {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.no-drag {
  -webkit-app-region: no-drag;
}

button {
  font-family: inherit;
}

::-webkit-scrollbar {
  display: none;
  width: 0;
  height: 0;
}

* {
  scrollbar-width: none;
  -ms-overflow-style: none;
}

img {
  -webkit-user-drag: none;
  user-drag: none;
}
</style>
