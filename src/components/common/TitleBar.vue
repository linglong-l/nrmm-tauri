<template>
  <div class="title-bar">
    <!-- 拖拽区域：覆盖整个标题栏用于窗口拖动 -->
    <div class="title-bar-drag"></div>
    <div class="title-bar-content">
      <div class="title-bar-left no-drag">
        <!-- macOS风格窗口控制按钮（红黄绿三色圆点） -->
        <div class="window-controls macos" v-if="isMac">
          <button class="mac-btn close" @click="handleClose" :aria-label="t('common.close')">
            <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><circle cx="6" cy="6" r="5" fill="#ff5f57"/><path d="M4 4l4 4M8 4l-4 4" stroke="#4d0000" stroke-width="1.2" stroke-linecap="round"/></svg>
          </button>
          <button class="mac-btn minimize" @click="handleMinimize" :aria-label="t('common.minimize', 'Minimize')">
            <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><circle cx="6" cy="6" r="5" fill="#febc2e"/><path d="M3.5 6h5" stroke="#5a3900" stroke-width="1.2" stroke-linecap="round"/></svg>
          </button>
          <button class="mac-btn maximize" @click="handleMaximize" :aria-label="t('common.maximize', 'Maximize')">
            <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><circle cx="6" cy="6" r="5" fill="#28c840"/><path d="M4 4v4h4V4H4z" fill="#004400"/></svg>
          </button>
        </div>
        <!-- Windows/Linux下显示应用图标和标题 -->
        <img src="@/assets/images/app-icon-32.png" class="app-icon" :alt="t('common.appName', 'NRMM')" v-if="!isMac" width="32" height="32" />
        <span class="app-title" v-if="!isMac">{{ t('common.appName', 'NRMM') }}</span>
      </div>
      <div class="title-bar-center"></div>
      <!-- Windows风格窗口控制按钮（最小化/最大化/关闭） -->
      <div class="title-bar-right no-drag" v-if="!isMac">
        <WindowControls />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 标题栏组件
 * 根据操作系统显示不同风格的窗口控制按钮
 * macOS：左侧红黄绿圆点
 * Windows/Linux：左侧图标+标题，右侧最小化/最大化/关闭按钮
 */
import { ref, computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { minimizeWindow, toggleMaximize, closeWindow } from '@/utils/tauri'
import { usePlatform } from '@/stores/platform'
import { logger } from '@/utils/logger'
import WindowControls from './WindowControls.vue'

const { t } = useI18n()
const { platformInfo } = usePlatform()
/** 窗口是否最大化状态 */
const isMaximized = ref(false)

/** 是否为macOS平台 */
const isMac = computed(() => platformInfo.value?.os === 'macos')

onMounted(async () => {
  try {
    isMac.value
  } catch (e) {
    logger.warn('TitleBar', 'Platform detection failed, defaulting to Windows')
  }
})

/** 最小化窗口 */
async function handleMinimize() {
  try {
    await minimizeWindow('main')
  } catch (e) {
    logger.error('TitleBar', 'Failed to minimize window', e)
  }
}

/** 切换最大化/还原窗口 */
async function handleMaximize() {
  try {
    await toggleMaximize('main')
    isMaximized.value = !isMaximized.value
  } catch (e) {
    logger.error('TitleBar', 'Failed to toggle maximize', e)
  }
}

/** 关闭窗口 */
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
  backdrop-filter: blur(20px) saturate(180%);
  -webkit-backdrop-filter: blur(20px) saturate(180%);
  display: flex;
  align-items: center;
  position: relative;
  user-select: none;
  flex-shrink: 0;
  border-top-left-radius: var(--app-radius);
  border-top-right-radius: var(--app-radius);
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
  background: transparent;
}

.title-bar-left {
  display: flex;
  align-items: center;
  gap: 8px;
  background: transparent;
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
