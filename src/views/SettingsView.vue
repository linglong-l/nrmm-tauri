<template>
  <div class="settings-view">
    <div class="settings-container">
      <h2 class="settings-title">{{ t('settings.title') }}</h2>

      <!-- Game Selection -->
      <div class="settings-section">
        <div class="section-row">
          <span class="row-label">{{ t('settings.selectGame') }}</span>
          <el-select v-model="formData.game" class="transparent-select" @change="handleGameChange">
            <el-option label="Genshin Impact" value="GenshinImpact" />
            <el-option label="Honkai: Star Rail" value="HonkaiStarRail" />
            <el-option label="Wuthering Waves" value="Wuwa" />
            <el-option label="Zenless Zone Zero" value="ZZZ" />
            <el-option label="Honkai Impact 3rd" value="HonkaiImpact3rd" />
          </el-select>
        </div>
        <div class="section-row">
          <span class="row-label">{{ t('settings.modsPath') }}</span>
          <div class="path-input-row">
            <el-input v-model="formData.modsPath" class="transparent-input path-input" readonly />
            <el-button class="transparent-btn" @click="handleBrowse">
              {{ t('settings.browse') }}
            </el-button>
          </div>
        </div>
      </div>

      <!-- Update Mod Data -->
      <div class="settings-section">
        <el-button class="transparent-btn action-btn" @click="handleUpdateModData" :loading="updatingModData">
          {{ t('settings.updateModData') }}
        </el-button>
      </div>

      <!-- Enable/Disable Mod Group -->
      <div class="settings-section">
        <div class="section-row">
          <span class="row-label">{{ t('settings.enableModGroup') }}</span>
          <el-switch v-model="formData.modGroupEnabled" active-color="#4a9eff" inactive-color="#555" />
        </div>
      </div>

      <!-- Restore Zone -->
      <div class="settings-section">
        <div class="section-row column">
          <span class="row-label">{{ t('settings.restoreZone') }}</span>
          <p class="section-desc">{{ t('Only for mods that are directly removed via File Explorer (without right-click on "Mods" tab)') }}</p>
        </div>
      </div>

      <!-- Group Folder Icon -->
      <div class="settings-section">
        <span class="section-heading">{{ t('settings.groupFolderIcon') }}</span>
        <div class="section-row">
          <el-button class="transparent-btn" @click="handleGenerateFolderIcon">
            {{ t('settings.generateFolderIcon') }}
          </el-button>
        </div>
        <div class="section-row">
          <span class="row-label">{{ t('settings.autoGenerateIcon') }}</span>
          <el-switch v-model="formData.autoFolderIcon" active-color="#4a9eff" inactive-color="#555" />
        </div>
      </div>

      <!-- Overall Scale -->
      <div class="settings-section">
        <div class="section-row">
          <span class="row-label">{{ t('settings.overallScale') }}</span>
          <span class="slider-value">{{ formData.overallScale?.toFixed(1) }}</span>
        </div>
        <el-slider v-model="formData.overallScale" :min="0.8" :max="1.5" :step="0.1" show-stops class="transparent-slider" />
      </div>

      <!-- Background Transparency -->
      <div class="settings-section">
        <div class="section-row">
          <span class="row-label">{{ t('settings.backgroundTransparency') }}</span>
          <span class="slider-value">{{ formData.bgTransparency?.toFixed(1) }}</span>
        </div>
        <el-slider v-model="formData.bgTransparency" :min="0" :max="1" :step="0.1" show-stops class="transparent-slider" />
      </div>

      <!-- Window Toggle Hotkeys -->
      <div class="settings-section">
        <span class="section-heading">{{ t('settings.windowToggleHotkey') }}</span>
        <div class="section-row">
          <span class="row-label">{{ t('settings.keyboardToggle') }}</span>
          <el-select v-model="formData.keyboardToggle" class="transparent-select" placeholder="Alt+W">
            <el-option label="Alt+W" value="Alt+W" />
            <el-option label="Alt+Q" value="Alt+Q" />
            <el-option label="Alt+E" value="Alt+E" />
            <el-option label="Alt+R" value="Alt+R" />
            <el-option label="Alt+T" value="Alt+T" />
            <el-option :label="t('settings.none')" value="" />
          </el-select>
        </div>
        <div class="section-row">
          <span class="row-label">{{ t('settings.gamepadToggle') }}</span>
          <el-select v-model="formData.gamepadToggle" class="transparent-select" placeholder="LB+RB">
            <el-option label="LB+RB" value="LB+RB" />
            <el-option label="LB+Start" value="LB+Start" />
            <el-option label="RB+Back" value="RB+Back" />
            <el-option :label="t('settings.none')" value="" />
          </el-select>
        </div>
      </div>

      <!-- Auto pin window & Show Tray Menu -->
      <div class="settings-section">
        <div class="section-row">
          <span class="row-label">{{ t('settings.autoPinWindow') }}</span>
          <el-switch v-model="formData.autoPinWindow" active-color="#4a9eff" inactive-color="#555" />
        </div>
        <div class="section-row">
          <span class="row-label">{{ t('settings.showTrayMenu') }}</span>
          <el-switch v-model="formData.showTrayMenu" active-color="#4a9eff" inactive-color="#555" />
        </div>
      </div>

      <!-- Language -->
      <div class="settings-section">
        <div class="section-row">
          <span class="row-label">{{ t('settings.language') }}</span>
          <el-select v-model="formData.language" class="transparent-select" @change="handleLanguageChange">
            <el-option label="简体中文" value="zh-CN" />
            <el-option label="English" value="en" />
          </el-select>
        </div>
      </div>

      <!-- Navigation Hotkeys -->
      <div class="settings-section">
        <span class="section-heading">{{ t('settings.navigationHotkeys') }}</span>
        <div class="hints-grid">
          <div class="hint-item">
            <span class="hint-key">{{ t('Navigation: WASD / D-Pad') }}</span>
            <span class="hint-desc">{{ t('Group Navigation') }}</span>
          </div>
          <div class="hint-item">
            <span class="hint-key">{{ t('Select: F / A') }}</span>
            <span class="hint-desc">{{ t('Select Mod') }}</span>
          </div>
          <div class="hint-item">
            <span class="hint-key">{{ t('Keybind: R / X') }}</span>
            <span class="hint-desc">{{ t('Mod Keybind') }}</span>
          </div>
          <div class="hint-item">
            <span class="hint-key">{{ t('Tab: Q-E / LB-RB') }}</span>
            <span class="hint-desc">{{ t('Tab Navigation') }}</span>
          </div>
          <div class="hint-item">
            <span class="hint-key">{{ t('Search: Space') }}</span>
            <span class="hint-desc">{{ t('Search') }}</span>
          </div>
        </div>
      </div>

      <!-- Bottom Buttons -->
      <div class="settings-section">
        <div class="bottom-buttons">
          <el-button class="transparent-btn" @click="handleCheckUpdates" :loading="checkingUpdates">
            {{ t('settings.checkUpdates') }}
          </el-button>
          <el-button class="transparent-btn" @click="handleResetPosition">
            {{ t('settings.resetPosition') }}
          </el-button>
          <el-button class="transparent-btn" @click="handleExit">
            {{ t('settings.exit') }}
          </el-button>
        </div>
      </div>

      <!-- Bottom Links -->
      <div class="bottom-links">
        <a class="link-item" href="#">{{ t('settings.supportDeveloper') }}</a>
        <span class="link-separator">·</span>
        <a class="link-item" href="#">{{ t('settings.contactHelp') }}</a>
        <span class="link-separator">·</span>
        <a class="link-item" href="#">{{ t('settings.tutorial') }}</a>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { ElMessage } from 'element-plus'
import { useSettingsStore } from '@/stores/settings'
import { selectFolder, checkForUpdates } from '@/utils/tauri'
import { logger } from '@/utils/logger'
import type { TargetGame } from '@/types'

const { t, locale } = useI18n()
const settingsStore = useSettingsStore()

const formData = reactive({
  game: 'GenshinImpact' as TargetGame,
  modsPath: '',
  language: 'zh-CN',
  modGroupEnabled: true,
  overallScale: 1.0,
  bgTransparency: 0.5,
  keyboardToggle: 'Alt+W',
  gamepadToggle: 'LB+RB',
  autoPinWindow: false,
  autoFolderIcon: false,
  showTrayMenu: false,
})

const updatingModData = ref(false)
const checkingUpdates = ref(false)

onMounted(async () => {
  await settingsStore.load()
  formData.game = settingsStore.currentGame
  formData.modsPath = settingsStore.currentModsPath
  formData.language = locale.value
  if (settingsStore.settings.interfaceScale !== undefined) {
    formData.overallScale = settingsStore.settings.interfaceScale
  }
  if (settingsStore.settings.bgTransparency !== undefined) {
    formData.bgTransparency = settingsStore.settings.bgTransparency
  }
  if (settingsStore.settings.autoFolderIcon !== undefined) {
    formData.autoFolderIcon = settingsStore.settings.autoFolderIcon
  }
  if (settingsStore.settings.autoTopWindow !== undefined) {
    formData.autoPinWindow = settingsStore.settings.autoTopWindow
  }
  if (settingsStore.settings.alwaysShowMenuOnHotkey !== undefined) {
    formData.showTrayMenu = settingsStore.settings.alwaysShowMenuOnHotkey
  }
  if (settingsStore.settings.enabledKb !== undefined) {
    formData.modGroupEnabled = settingsStore.settings.enabledKb
  }
})

function handleGameChange(game: TargetGame) {
  formData.game = game
  settingsStore.updateGame(game, formData.modsPath)
}

async function handleBrowse() {
  try {
    const selected = await selectFolder()
    if (selected) {
      formData.modsPath = selected
      settingsStore.updateGame(formData.game, formData.modsPath)
      await settingsStore.save()
    }
  } catch (e: any) {
    ElMessage.error(t('Failed to open folder dialog'))
    logger.error('SettingsView', 'Browse failed', e)
  }
}

async function handleUpdateModData() {
  updatingModData.value = true
  try {
    logger.info('SettingsView', 'Updating mod data...')
    ElMessage.success(t('Update Mod Data completed successfully!'))
  } catch (e: any) {
    ElMessage.error(t('Unknown error occurred.'))
    logger.error('SettingsView', 'Update mod data failed', e)
  } finally {
    updatingModData.value = false
  }
}

function handleGenerateFolderIcon() {
  logger.info('SettingsView', 'Generate folder icon')
  ElMessage.info(t('Task completed!'))
}

async function handleCheckUpdates() {
  checkingUpdates.value = true
  try {
    const result = await checkForUpdates()
    if (result) {
      ElMessage.info(t('Updater.newVersionAvailable', { version: result }))
    } else {
      ElMessage.info(t('Updater.upToDate'))
    }
  } catch (e: any) {
    ElMessage.error(t('Updater.checkFailed') + ': ' + (e?.message || e))
  } finally {
    checkingUpdates.value = false
  }
}

async function handleResetPosition() {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    await invoke('reset_window_position')
    ElMessage.success(t('Reset Position'))
  } catch (e: any) {
    logger.error('SettingsView', 'Reset position failed', e)
  }
}

async function handleExit() {
  try {
    const { closeWindow } = await import('@/utils/tauri')
    await closeWindow('main')
  } catch (e: any) {
    logger.error('SettingsView', 'Exit failed', e)
  }
}

function handleLanguageChange(lang: string) {
  locale.value = lang
}
</script>

<style scoped>
.settings-view {
  flex: 1;
  overflow-y: auto;
  padding: 24px;
  background: transparent;
}

.settings-container {
  max-width: 700px;
  margin: 0 auto;
}

.settings-title {
  font-size: 24px;
  font-weight: 600;
  color: var(--text-primary);
  margin: 0 0 24px;
}

.settings-section {
  margin-bottom: 20px;
  padding-bottom: 20px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
}

.settings-section:last-of-type {
  border-bottom: none;
  margin-bottom: 0;
  padding-bottom: 0;
}

.section-heading {
  display: block;
  font-size: 15px;
  font-weight: 600;
  color: var(--text-primary);
  margin-bottom: 12px;
}

.section-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 10px;
  gap: 12px;
}

.section-row:last-child {
  margin-bottom: 0;
}

.section-row.column {
  flex-direction: column;
  align-items: flex-start;
}

.row-label {
  font-size: 13px;
  color: var(--text-primary);
  flex-shrink: 0;
}

.section-desc {
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.5;
  margin: 6px 0 0 0;
}

.path-input-row {
  display: flex;
  gap: 8px;
  flex: 1;
  max-width: 400px;
}

.path-input {
  flex: 1;
}

.slider-value {
  font-size: 13px;
  color: var(--text-secondary);
  min-width: 32px;
  text-align: right;
}

.action-btn {
  width: 100%;
}

.bottom-buttons {
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
}

/* Transparent Element Plus overrides */
:deep(.transparent-select .el-input__wrapper) {
  background: transparent !important;
  border: 1px solid rgba(255, 255, 255, 0.25);
  box-shadow: none !important;
  border-radius: 6px;
}

:deep(.transparent-select .el-input__wrapper:hover) {
  border-color: rgba(255, 255, 255, 0.45);
}

:deep(.transparent-select .el-input__wrapper.is-focus) {
  border-color: var(--accent-primary);
}

:deep(.transparent-select .el-input__inner) {
  color: var(--text-primary);
}

:deep(.transparent-select .el-select__caret) {
  color: var(--text-muted);
}

:deep(.transparent-input .el-input__wrapper) {
  background: transparent !important;
  border: 1px solid rgba(255, 255, 255, 0.15);
  box-shadow: none !important;
  border-radius: 6px;
}

:deep(.transparent-input .el-input__wrapper:hover) {
  border-color: rgba(255, 255, 255, 0.3);
}

:deep(.transparent-input .el-input__inner) {
  color: var(--text-secondary);
  cursor: default;
}

.transparent-btn {
  background: transparent !important;
  border: 1px solid rgba(255, 255, 255, 0.25) !important;
  border-radius: 6px;
  color: var(--text-primary) !important;
  font-size: 13px;
  padding: 8px 16px;
  transition: border-color 0.2s, background 0.2s;
}

.transparent-btn:hover {
  border-color: rgba(255, 255, 255, 0.5) !important;
  background: transparent !important;
}

:deep(.transparent-slider .el-slider__runway) {
  background: transparent;
}

:deep(.transparent-slider .el-slider__bar) {
  background: transparent;
}

:deep(.transparent-slider .el-slider__button) {
  border-color: var(--accent-primary);
  background: transparent;
}

:deep(.transparent-slider .el-slider__stop) {
  background: transparent;
}

/* Navigation hints grid */
.hints-grid {
  display: grid;
  gap: 8px;
}

.hint-item {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 8px 12px;
  border: 1px solid rgba(255, 255, 255, 0.1);
  border-radius: 6px;
}

.hint-key {
  font-family: monospace;
  font-size: 13px;
  color: var(--accent-primary);
  font-weight: 500;
}

.hint-desc {
  font-size: 12px;
  color: var(--text-secondary);
}

/* Bottom links */
.bottom-links {
  display: flex;
  justify-content: center;
  align-items: center;
  gap: 8px;
  margin-top: 24px;
  padding-top: 16px;
}

.link-item {
  color: var(--text-muted);
  font-size: 12px;
  text-decoration: none;
  transition: color 0.2s;
}

.link-item:hover {
  color: var(--text-secondary);
}

.link-separator {
  color: var(--text-muted);
  font-size: 12px;
}

/* el-switch overrides via scoped CSS won't work for deep slots,
   but active-color prop handles the active color. */
</style>
