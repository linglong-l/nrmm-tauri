<template>
  <div class="window-controls">
    <!-- 最小化按钮 -->
    <button class="win-btn minimize" @click="handleMinimize" :aria-label="t('common.minimizeA11y', 'Minimize window')" :title="t('common.minimize', 'Minimize')">
      <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><line x1="1" y1="5" x2="9" y2="5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
    </button>
    <!-- 最大化/还原按钮：根据状态切换图标 -->
    <button class="win-btn maximize" @click="handleMaximize" :aria-label="isMaximized ? t('common.restoreA11y', 'Restore window') : t('common.maximizeA11y', 'Maximize window')" :title="isMaximized ? t('common.restore', 'Restore') : t('common.maximize', 'Maximize')">
      <svg v-if="!isMaximized" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><rect x="1.5" y="1.5" width="7" height="7" stroke="currentColor" stroke-width="1.2" fill="none" rx="0.5"/></svg>
      <svg v-else width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><rect x="2.5" y="0.5" width="7" height="7" stroke="currentColor" stroke-width="1.2" fill="none" rx="0.5"/><path d="M2.5 2.5H9.5V9.5H2.5V2.5z" fill="currentColor" opacity="0.1"/><rect x="0.5" y="2.5" width="7" height="7" stroke="currentColor" stroke-width="1.2" fill="#121212" rx="0.5"/></svg>
    </button>
    <!-- 关闭按钮：悬停变红 -->
    <button class="win-btn close" @click="handleClose" :aria-label="t('common.closeA11y', 'Close window')" :title="t('common.close', 'Close')">
      <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/><line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
    </button>
  </div>
</template>

<script setup lang="ts">
/**
 * Windows风格窗口控制按钮组件
 * 提供最小化、最大化/还原、关闭三个按钮
 * 关闭按钮悬停时显示红色背景（Windows原生体验）
 */
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { minimizeWindow, toggleMaximize, closeWindow } from '@/utils/tauri'
import { logger } from '@/utils/logger'

const { t } = useI18n()

/** 窗口是否最大化状态 */
const isMaximized = ref(false)

/** 最小化窗口 */
async function handleMinimize() {
  try {
    await minimizeWindow('main')
  } catch (e) {
    logger.error('WindowControls', 'Failed to minimize window', e)
  }
}

/** 切换最大化/还原窗口 */
async function handleMaximize() {
  try {
    await toggleMaximize('main')
    isMaximized.value = !isMaximized.value
  } catch (e) {
    console.error('Failed to toggle maximize:', e)
  }
}

/** 关闭窗口 */
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
  transition: background-color 0.1s ease, color 0.1s ease;
  border-radius: 0;
}

.win-btn:hover {
  background: rgba(255, 255, 255, 0.08);
  color: #fff;
}

.win-btn:focus-visible {
  outline: 2px solid var(--accent-primary);
  outline-offset: -2px;
}

.win-btn.close:hover {
  background: #e81123;
  color: #fff;
}

.win-btn:active {
  background: rgba(255, 255, 255, 0.12);
}

.win-btn.close:active {
  background: #c4101d;
}
</style>
