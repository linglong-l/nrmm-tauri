<template>
  <div class="keybinds-view">
    <div class="keybinds-container">
      <h2 class="keybinds-title">{{ t('Keybinds') }}</h2>

      <div class="status-card" :class="keypressStatusClass">
        <el-icon :size="24" class="status-icon">
          <WarningFilled v-if="!keypressSupported" />
          <CircleCheckFilled v-else />
        </el-icon>
        <div class="status-content">
          <h3 class="status-title">
            {{ keypressSupported ? 'Keypress simulation available' : 'Keypress simulation not available' }}
          </h3>
          <p class="status-desc">
            {{ keypressSupported ? 'Hotkeys will work in supported games.' : keypressHint }}
          </p>
        </div>
      </div>

      <div class="coming-soon-card">
        <el-icon :size="48" class="coming-soon-icon"><Setting /></el-icon>
        <h3>热键配置</h3>
        <p>Coming soon</p>
        <p class="hint">{{ t('Click keybind to simulate keypress') }}</p>
      </div>

      <div class="keybind-hints">
        <h3 class="section-title">{{ t('Navigation Hotkeys') }}</h3>
        <div class="hints-grid">
          <div class="hint-item">
            <span class="key">{{ t('Navigation: WASD / D-Pad') }}</span>
            <span class="desc">在分组和模组间导航</span>
          </div>
          <div class="hint-item">
            <span class="key">{{ t('Select: F / A') }}</span>
            <span class="desc">选择模组</span>
          </div>
          <div class="hint-item">
            <span class="key">{{ t('Keybind: R / X') }}</span>
            <span class="desc">按键切换</span>
          </div>
          <div class="hint-item">
            <span class="key">{{ t('Tab: Q-E / LB-RB') }}</span>
            <span class="desc">切换标签页</span>
          </div>
          <div class="hint-item">
            <span class="key">{{ t('Search: Space') }}</span>
            <span class="desc">搜索</span>
          </div>
          <div class="hint-item">
            <span class="key">Alt + S</span>
            <span class="desc">聚焦搜索框</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { WarningFilled, CircleCheckFilled, Setting } from '@element-plus/icons-vue'
import { checkKeypressSupport } from '@/utils/tauri'
import { usePlatform } from '@/stores/platform'

const { t } = useI18n()
const { platformInfo } = usePlatform()

const keypressSupported = ref(true)
const checkingSupport = ref(false)

const keypressStatusClass = computed(() => ({
  'status-error': !keypressSupported.value,
  'status-success': keypressSupported.value
}))

const keypressHint = computed(() => {
  if (!platformInfo.value) return ''
  const os = platformInfo.value.os
  if (os === 'linux') {
    return 'Please install xdotool or ydotool for keypress simulation support.'
  }
  if (os === 'macos') {
    return 'Please grant accessibility permissions for keypress simulation.'
  }
  return 'Keypress simulation requires additional setup.'
})

onMounted(async () => {
  checkingSupport.value = true
  try {
    await checkKeypressSupport()
    keypressSupported.value = true
  } catch (e) {
    keypressSupported.value = false
  } finally {
    checkingSupport.value = false
  }
})
</script>

<style scoped>
.keybinds-view {
  flex: 1;
  overflow-y: auto;
  padding: 24px;
}

.keybinds-container {
  max-width: 600px;
  margin: 0 auto;
}

.keybinds-title {
  font-size: 24px;
  font-weight: 600;
  color: var(--text-primary);
  margin: 0 0 24px;
}

.status-card {
  display: flex;
  align-items: flex-start;
  gap: 16px;
  padding: 20px;
  border-radius: var(--border-radius);
  border: 1px solid;
  margin-bottom: 24px;
}

.status-card.status-success {
  background: transparent;
  border-color: var(--accent-success);
}

.status-card.status-error {
  background: transparent;
  border-color: var(--accent-danger);
}

.status-icon {
  flex-shrink: 0;
  margin-top: 2px;
}

.status-success .status-icon {
  color: var(--accent-success);
}

.status-error .status-icon {
  color: var(--accent-danger);
}

.status-content {
  flex: 1;
}

.status-title {
  margin: 0 0 8px;
  font-size: 15px;
  font-weight: 600;
  color: var(--text-primary);
}

.status-desc {
  margin: 0;
  font-size: 13px;
  color: var(--text-secondary);
  line-height: 1.5;
}

.coming-soon-card {
  background-color: transparent;
  border: 1px solid var(--border-color);
  border-radius: var(--border-radius);
  padding: 48px 24px;
  text-align: center;
  margin-bottom: 24px;
}

.coming-soon-icon {
  color: var(--text-muted);
  margin-bottom: 16px;
}

.coming-soon-card h3 {
  margin: 0 0 8px;
  font-size: 18px;
  font-weight: 600;
  color: var(--text-primary);
}

.coming-soon-card p {
  margin: 0 0 8px;
  color: var(--text-secondary);
  font-size: 14px;
}

.coming-soon-card .hint {
  font-size: 12px;
  color: var(--text-muted);
  margin-top: 16px;
}

.keybind-hints {
  border: 1px solid var(--border-color);
  border-radius: var(--border-radius);
  padding: 24px;
}

.section-title {
  font-size: 16px;
  font-weight: 600;
  color: var(--text-primary);
  margin: 0 0 16px;
}

.hints-grid {
  display: grid;
  gap: 12px;
}

.hint-item {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 12px;
  border-radius: 6px;
}

.key {
  font-family: monospace;
  font-size: 13px;
  color: var(--accent-primary);
  font-weight: 500;
}

.desc {
  font-size: 13px;
  color: var(--text-secondary);
}
</style>
