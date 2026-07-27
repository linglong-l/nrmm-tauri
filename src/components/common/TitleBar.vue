<template>
  <div class="title-bar" :class="{ 'no-transparency': !transparencySupported }">
    <div class="title-bar-drag"></div>
    <div class="title-bar-content">
      <div class="title-bar-left no-drag">
        <div class="window-controls macos" v-if="isMac">
          <button class="mac-btn close" @click="handleClose">
            <svg width="12" height="12" viewBox="0 0 12 12"><circle cx="6" cy="6" r="5" fill="#ff5f57"/><path d="M4 4l4 4M8 4l-4 4" stroke="#4d0000" stroke-width="1.2" stroke-linecap="round"/></svg>
          </button>
          <button class="mac-btn minimize" @click="handleMinimize">
            <svg width="12" height="12" viewBox="0 0 12 12"><circle cx="6" cy="6" r="5" fill="#febc2e"/><path d="M3.5 6h5" stroke="#5a3900" stroke-width="1.2" stroke-linecap="round"/></svg>
          </button>
          <button class="mac-btn maximize" @click="handleMaximize">
            <svg width="12" height="12" viewBox="0 0 12 12"><circle cx="6" cy="6" r="5" fill="#28c840"/><path d="M4 4v4h4V4H4z" fill="#004400"/></svg>
          </button>
        </div>
        <img src="@/assets/images/app-icon-32.png" class="app-icon" alt="NRMM" v-if="!isMac" />
        <span class="app-title" v-if="!isMac">NRMM</span>
      </div>
      <div class="title-bar-center"></div>
      <div class="title-bar-right no-drag" v-if="!isMac">
        <WindowControls />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { minimizeWindow, toggleMaximize, closeWindow } from '@/utils/tauri'
import { usePlatform } from '@/stores/platform'
import { logger } from '@/utils/logger'
import WindowControls from './WindowControls.vue'

const { platformInfo } = usePlatform()
const isMaximized = ref(false)
const transparencySupported = ref(true)

const isMac = computed(() => platformInfo.value?.os === 'macos')

onMounted(() => {
  if (platformInfo.value) {
    transparencySupported.value = platformInfo.value.transparencySupported
  }
})

async function handleMinimize() {
  try {
    await minimizeWindow('main')
  } catch (e) {
    logger.error('TitleBar', 'Failed to minimize window', e)
  }
}

async function handleMaximize() {
  try {
    await toggleMaximize('main')
    isMaximized.value = !isMaximized.value
  } catch (e) {
    logger.error('TitleBar', 'Failed to toggle maximize', e)
  }
}

async function handleClose() {
  try {
    await closeWindow('main')
  } catch (e) {
    logger.error('TitleBar', 'Failed to close window', e)
  }
}
</script>

<style scoped>
.title-bar {
  height: 40px;
  background: transparent;
  display: flex;
  align-items: center;
  position: relative;
  user-select: none;
  flex-shrink: 0;
}

.title-bar.no-transparency {
  background: #121212;
}

.title-bar-drag {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  height: 100%;
  -webkit-app-region: drag;
}

.title-bar-content {
  position: relative;
  z-index: 1;
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 12px;
}

.title-bar-left {
  display: flex;
  align-items: center;
  gap: 8px;
}

.app-icon {
  width: 20px;
  height: 20px;
}

.app-title {
  font-size: 13px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.9);
  letter-spacing: 0.5px;
}

.title-bar-center {
  flex: 1;
}

.title-bar-right {
  display: flex;
  align-items: center;
  gap: 4px;
}

.window-controls.macos {
  display: flex;
  gap: 8px;
}

.mac-btn {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  border: none;
  padding: 0;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  background: transparent;
}

.mac-btn:hover svg path,
.mac-btn:hover svg rect {
  opacity: 1;
}

.mac-btn svg path,
.mac-btn svg rect {
  opacity: 0;
  transition: opacity 0.1s;
}

.no-drag {
  -webkit-app-region: no-drag;
}
</style>
