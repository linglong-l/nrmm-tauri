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
    <!-- 热键未匹配到游戏时的选择菜单 -->
    <GamePickerMenu
      :show="showGamePicker"
      :screen-x="pickerScreenX"
      :screen-y="pickerScreenY"
      :foreground-process-name="pickerForegroundProcessName"
      @select="onPickerSelectGame"
      @close="showGamePicker = false"
    />
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
import GamePickerMenu from '@/components/common/GamePickerMenu.vue'
import { initPlatform } from '@/stores/platform'
import { useSettingsStore } from '@/stores/settings'
import { useModsStore } from '@/stores/mods'
import { logger } from '@/utils/logger'
import { switchTargetGame } from '@/utils/tauri'
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
/** 窗口隐藏事件取消监听函数 */
let unlistenWindowHidden: (() => void) | null = null
/** 窗口显示事件取消监听函数 */
let unlistenWindowShown: (() => void) | null = null
/** 窗口显示并检测到游戏事件取消监听函数 */
let unlistenWindowShowWithGame: (() => void) | null = null
/** 需用户选择游戏事件取消监听函数 */
let unlistenNeedPickGame: (() => void) | null = null
/** 标记窗口显示时是否已通过游戏检测事件处理了模组加载，避免重复加载 */
let gameSwitchPending = false

/* ===== GamePicker 相关状态 ===== */
/** 热键未匹配游戏时显示的选择菜单是否打开 */
const showGamePicker = ref(false)
/** 光标屏幕 X（来自后端事件 payload） */
const pickerScreenX = ref(0)
/** 光标屏幕 Y */
const pickerScreenY = ref(0)
/** 检测到的前台进程名（传给菜单展示） */
const pickerForegroundProcessName = ref<string>('')

/**
 * 通用：切换目标游戏并刷新模组（window-show-with-game / 菜单选择 / 托盘都走这里）
 * @param game  目标游戏
 * @param markPending  是否标记 gameSwitchPending（true 时 window-shown 会跳过重复加载）
 */
async function applyDetectedGameSwitch(game: TargetGame, markPending = true): Promise<void> {
  if (markPending) gameSwitchPending = true

  // 先同步更新前端游戏状态（确保 currentModsPath 立即切换到新游戏）
  settingsStore.switchGame(game)
  // 持久化到后端
  switchTargetGame(game).catch(e => logger.warn('App', 'Failed to switch target game', e))

  // 停止旧的文件监听并清理
  await modsStore.stopWatching()
  modsStore.clearData()

  if (activeTab.value === 'mods') {
    // 已在模组页：直接重启监听并加载新游戏模组
    await nextTick()
    await modsStore.startWatching()
    await modsStore.loadMods()
  } else {
    // 切换到模组页（ModsView 挂载时会自动 startWatching + loadMods）
    activeTab.value = 'mods'
  }
}

/** GamePicker 菜单选择后的逻辑：关闭菜单 + 切换游戏 */
async function onPickerSelectGame(game: TargetGame) {
  showGamePicker.value = false
  logger.info('App', `Game picker selected: ${game}`)
  await applyDetectedGameSwitch(game, true)
}

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
      // 切换游戏后如在模组页面需要重新加载模组和文件监听
      if (activeTab.value === 'mods') {
        await modsStore.startWatching()
        await modsStore.loadMods()
      }
    })
  } catch (e) {
    logger.warn('App', 'Failed to register target-game-switched listener', e)
  }

  // 监听窗口隐藏事件：清除前端模组数据和文件监听
  // 触发时机：点击关闭按钮、按热键隐藏窗口
  try {
    unlistenWindowHidden = await listen('window-hidden', async () => {
      logger.debug('App', 'Window hidden, clearing mods data')
      await modsStore.stopWatching()
      modsStore.clearData()
    })
  } catch (e) {
    logger.warn('App', 'Failed to register window-hidden listener', e)
  }

  // 监听窗口显示事件：如果在模组页则重新加载模组
  // 触发时机：按热键显示窗口（未检测到游戏时）
  try {
    unlistenWindowShown = await listen('window-shown', async () => {
      logger.debug('App', 'Window shown')
      if (gameSwitchPending) {
        // window-show-with-game 事件已处理了游戏切换和模组加载，此处跳过
        gameSwitchPending = false
        return
      }
      if (activeTab.value === 'mods' && settingsStore.currentModsPath) {
        await modsStore.startWatching()
        await modsStore.loadMods()
      }
    })
  } catch (e) {
    logger.warn('App', 'Failed to register window-shown listener', e)
  }

  // 监听窗口显示并检测到前台游戏事件：自动切换到对应游戏并导航到模组页
  // 触发时机：按热键显示窗口且前台是受支持的游戏进程
  try {
    unlistenWindowShowWithGame = await listen<TargetGame>('window-show-with-game', async (event) => {
      const game = event.payload
      logger.info('App', `Window shown with detected game: ${game}`)
      await applyDetectedGameSwitch(game, true)
    })
  } catch (e) {
    logger.warn('App', 'Failed to register window-show-with-game listener', e)
  }

  // 监听「未匹配到游戏，需用户选择」事件：在光标处弹出 GamePicker 菜单
  // 触发时机：按热键显示窗口，但前台进程不在受支持游戏进程名列表内
  try {
    type NeedPickGamePayload = {
      cursorX: number
      cursorY: number
      foregroundProcessName?: string | null
    }
    unlistenNeedPickGame = await listen<NeedPickGamePayload>('need-pick-game', (event) => {
      const p = event.payload ?? { cursorX: 0, cursorY: 0, foregroundProcessName: null }
      logger.debug(
        'App',
        `need-pick-game: fg=${p.foregroundProcessName ?? 'null'} pos=(${p.cursorX},${p.cursorY})`
      )
      pickerScreenX.value = Number(p.cursorX) || 0
      pickerScreenY.value = Number(p.cursorY) || 0
      pickerForegroundProcessName.value = String(p.foregroundProcessName ?? '')
      showGamePicker.value = true
    })
  } catch (e) {
    logger.warn('App', 'Failed to register need-pick-game listener', e)
  }
})

onBeforeUnmount(() => {
  // 清理事件监听器防止内存泄漏
  if (unlistenTargetGame) {
    unlistenTargetGame()
    unlistenTargetGame = null
  }
  if (unlistenWindowHidden) {
    unlistenWindowHidden()
    unlistenWindowHidden = null
  }
  if (unlistenWindowShown) {
    unlistenWindowShown()
    unlistenWindowShown = null
  }
  if (unlistenWindowShowWithGame) {
    unlistenWindowShowWithGame()
    unlistenWindowShowWithGame = null
  }
  if (unlistenNeedPickGame) {
    unlistenNeedPickGame()
    unlistenNeedPickGame = null
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
