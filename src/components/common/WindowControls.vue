<template>
  <div class="window-controls">
    <button class="win-btn minimize" @click="handleMinimize" title="Minimize">
      <svg width="10" height="10" viewBox="0 0 10 10"><line x1="1" y1="5" x2="9" y2="5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
    </button>
    <button class="win-btn maximize" @click="handleMaximize" :title="isMaximized ? 'Restore' : 'Maximize'">
      <svg v-if="!isMaximized" width="10" height="10" viewBox="0 0 10 10"><rect x="1.5" y="1.5" width="7" height="7" stroke="currentColor" stroke-width="1.2" fill="none" rx="0.5"/></svg>
      <svg v-else width="10" height="10" viewBox="0 0 10 10"><rect x="2.5" y="0.5" width="7" height="7" stroke="currentColor" stroke-width="1.2" fill="none" rx="0.5"/><path d="M2.5 2.5H9.5V9.5H2.5V2.5z" fill="currentColor" opacity="0.1"/><rect x="0.5" y="2.5" width="7" height="7" stroke="currentColor" stroke-width="1.2" fill="#121212" rx="0.5"/></svg>
    </button>
    <button class="win-btn close" @click="handleClose" title="Close">
      <svg width="10" height="10" viewBox="0 0 10 10"><line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/><line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
    </button>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { minimizeWindow, toggleMaximize, closeWindow } from '@/utils/tauri'
import { logger } from '@/utils/logger'

const isMaximized = ref(false)

async function handleMinimize() {
  try {
    await minimizeWindow('main')
  } catch (e) {
    logger.error('WindowControls', 'Failed to minimize window', e)
  }
}

async function handleMaximize() {
  try {
    await toggleMaximize('main')
    isMaximized.value = !isMaximized.value
  } catch (e) {
    console.error('Failed to toggle maximize:', e)
  }
}

async function handleClose() {
  try {
    await closeWindow('main')
  } catch (e) {
    logger.error('WindowControls', 'Failed to close window', e)
  }
}
</script>

<style scoped>
.window-controls {
  display: flex;
  gap: 0;
}

.win-btn {
  width: 46px;
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: none;
  color: rgba(255, 255, 255, 0.7);
  cursor: pointer;
  transition: all 0.1s ease;
  border-radius: 0;
}

.win-btn:hover {
  background: transparent;
  color: #fff;
}

.win-btn.close:hover {
  background: transparent;
  color: #fff;
}

.win-btn:active {
  background: transparent;
}
</style>
