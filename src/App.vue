<template>
  <div id="app">
    <!-- 标题栏：包含窗口控制按钮和应用图标 -->
    <TitleBar />
    <!-- 导航容器：胶囊标签页切换 -->
    <div class="nav-container">
      <PillTabs v-model="activeTab" :tabs="tabs" />
    </div>
    <!-- 主内容区：根据选中标签显示对应视图 -->
    <div class="main-content">
      <KeybindsView v-if="activeTab === 'keybinds'" />
      <ModsView v-else-if="activeTab === 'mods'" />
      <SettingsView v-else-if="activeTab === 'settings'" />
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 根组件
 * 负责应用初始化、全局样式应用、生命周期管理、平台事件监听
 * 管理背景透明度和界面缩放设置
 */
import { ref, computed, onMounted, onBeforeUnmount, watch, nextTick, defineAsyncComponent } from 'vue'
import { useI18n } from 'vue-i18n'
import { listen } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification'
import TitleBar from '@/components/common/TitleBar.vue'
import PillTabs from '@/components/nav/PillTabs.vue'
import { initPlatform } from '@/stores/platform'
import { useSettingsStore } from '@/stores/settings'
import { useModsStore } from '@/stores/mods'
import { logger } from '@/utils/logger'
import type { TargetGame } from '@/types'

/**
 * 三个Tab视图使用异步组件懒加载
 * KeybindsView为默认显示页，首屏需要立即加载
 * ModsView和SettingsView仅在切换到对应Tab时才加载，减小初始包体积
 */
const KeybindsView = defineAsyncComponent(() => import('@/views/KeybindsView.vue'))
const ModsView = defineAsyncComponent(() => import('@/views/ModsView.vue'))
const SettingsView = defineAsyncComponent(() => import('@/views/SettingsView.vue'))

// ===== 前端启动计时（使用main.ts的全局起点） =====
const FE_BOOT_START: number = (window as any).__NRMM_FE_BOOT_START__ ?? performance.now()
console.log(`[FE-BOOT] T+${(performance.now() - FE_BOOT_START).toFixed(0).padStart(6)}ms - App.vue <script setup> 开始执行`)

/**
 * 向后端报告前端启动阶段
 * @param stage 阶段名称
 */
function reportBootStage(stage: string) {
  const ms = performance.now() - FE_BOOT_START
  console.log(`[FE-BOOT] T+${ms.toFixed(0).padStart(6)}ms - ${stage}`)
  invoke('report_frontend_ready', { stage, frontendMs: ms }).catch(() => {})
}

const { t } = useI18n()
const settingsStore = useSettingsStore()
const modsStore = useModsStore()
reportBootStage('script-setup-begin')

/** 导航标签页配置 */
const tabs = [
  { key: 'keybinds', label: t('Keybinds') },
  { key: 'mods', label: t('Mods') },
  { key: 'settings', label: t('Settings') }
]

/** 当前激活的标签页，默认为模组页面 */
const activeTab = ref("mods")
/** 游戏切换事件取消监听函数 */
let unlistenTargetGame: (() => void) | null = null

/** 背景透明度计算属性，带默认值和类型校验 */
const bgAlpha = computed(() => {
  const v = settingsStore.settings.bgTransparency
  return typeof v === 'number' && !Number.isNaN(v) ? v : 0.3
})

/** 界面缩放比例计算属性，带默认值和类型校验 */
const scaleVal = computed(() => {
  const v = settingsStore.settings.interfaceScale
  return typeof v === 'number' && !Number.isNaN(v) ? v : 1.0
})

/**
 * 应用背景透明度到DOM
 * 将透明度值限制在0-1范围内并设置CSS背景色
 */
function applyBackground() {
  const el = document.getElementById('app')
  if (!el) return
  const alpha = Math.max(0, Math.min(1, bgAlpha.value))
  const a = Number(alpha.toFixed(2))
  el.style.setProperty('background-color', `rgba(18, 18, 18, ${a})`)
}

/**
 * 应用界面缩放到DOM
 * 将缩放值限制在0.6-2.0范围内并设置CSS zoom属性
 */
function applyScale() {
  const value = Math.max(0.6, Math.min(2.0, scaleVal.value))
  document.documentElement.style.zoom = String(value)
}

/**
 * 发送启动成功通知
 * 请求通知权限后发送系统通知
 */
async function sendStartupNotification() {
  try {
    let permissionGranted = await isPermissionGranted()
    if (!permissionGranted) {
      const permission = await requestPermission()
      permissionGranted = permission === 'granted'
    }
    if (permissionGranted) {
      // 发送系统通知前输出时间戳（用户可感知的关键节点）
      console.log(`[FE-BOOT] T+${(performance.now() - FE_BOOT_START).toFixed(0).padStart(6)}ms - >>> 正在发送系统通知（用户可见）<<<`)
      sendNotification({
        title: t('notification.startupSuccess.title'),
        body: t('notification.startupSuccess.body'),
      })
    }
  } catch (e) {
    logger.warn('App', 'Failed to send startup notification', e)
  }
}

/** 监听设置变化，实时应用背景透明度 */
watch(bgAlpha, applyBackground, { flush: 'post' })
/** 监听设置变化，实时应用界面缩放 */
watch(scaleVal, applyScale, { flush: 'post' })

onMounted(async () => {
  reportBootStage('dom-mounted')

  // 加载用户设置
  const settingsLoadStart = performance.now()
  try {
    await settingsStore.load()
    console.log(`[FE-BOOT] T+${(performance.now() - FE_BOOT_START).toFixed(0).padStart(6)}ms - 设置加载完成 (耗时${(performance.now() - settingsLoadStart).toFixed(0)}ms)`)
  } catch (e) {
    logger.error('App', 'settingsStore.load failed', e)
  }
  reportBootStage('settings-loaded')

  // 应用已保存的视觉设置
  applyBackground()
  applyScale()
  reportBootStage('visual-settings-applied')

  // 等待DOM首次渲染完成
  await nextTick()
  reportBootStage('first-nexttick')

  // 发送启动通知（用户可感知的"启动完成"信号）
  const notifyStart = performance.now()
  sendStartupNotification().then(() => {
    console.log(`[FE-BOOT] T+${(performance.now() - FE_BOOT_START).toFixed(0).padStart(6)}ms - 系统通知已发送 (耗时${(performance.now() - notifyStart).toFixed(0)}ms)`)
    reportBootStage('notification-sent')
  })

  // 报告UI完全就绪
  logger.info('App', `UI ready in ${(performance.now() - FE_BOOT_START).toFixed(0)}ms`)
  reportBootStage('fully-ready')

  // 异步初始化平台信息（不阻塞UI渲染）
  initPlatform().catch(e => logger.error('App', 'initPlatform failed', e))

  // 监听后端游戏切换事件
  // 事件名：target-game-switched
  // 处理逻辑：更新当前游戏设置，保存配置，如在模组页则重新加载模组列表
  try {
    unlistenTargetGame = await listen<TargetGame>('target-game-switched', async (event) => {
      const game = event.payload
      logger.info('App', `Target game switched to: ${game}`)
      settingsStore.switchGame(game)
      // 切换游戏后如在模组页面需要重新加载模组
      if (activeTab.value === 'mods') {
        await modsStore.loadMods()
        console.debug(modsStore)
      }
    })
  } catch (e) {
    logger.warn('App', 'Failed to register target-game-switched listener', e)
  }
})

onBeforeUnmount(() => {
  // 清理事件监听器防止内存泄漏
  if (unlistenTargetGame) {
    unlistenTargetGame()
    unlistenTargetGame = null
  }
})
</script>

<style>
:root {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
  font-size: 14px;
  line-height: 1.5;
  font-weight: 400;
  color: rgba(255, 255, 255, 0.8);
  background-color: transparent;
  color-scheme: dark;
  --app-radius: 12px;
  --text-primary: rgb(255 255 255);
  --text-secondary: rgba(255, 255, 255, 0.70);
  --text-muted: rgba(255, 255, 255, 0.45);
  --accent-primary: #4a9eff;
  --accent-success: #28c840;
  --accent-danger: #e74c3c;
  --accent-warning: #f39c12;
  --accent-info: #4a9eff;
  /* --border-color: rgba(255, 255, 255, 0.12); */
  --border-radius: 12px;
  font-synthesis: none;
  text-rendering: optimizeLegibility;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  -webkit-text-size-adjust: 100%;

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
  border-radius: var(--app-radius);
}

body {
  /* background-color: rgba(18, 18, 18, 0.85); */
  outline: 1px solid rgba(255, 255, 255, 0.06);
  outline-offset: -1px;
  /* color-scheme: dark; */
}

#app {
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background-color: rgba(18, 18, 18, 0.3);
  color-scheme: dark;
}

.nav-container {
  display: flex;
  justify-content: center;
  padding: 8px 0;
  flex-shrink: 0;
  background: transparent !important;
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
  user-select: none;
  -webkit-user-select: none;
}

:not(input):not(textarea):not(.allow-context-menu) {
  -webkit-touch-callout: none;
}
</style>
