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
    <UpdateModDataOverlay ref="overlayRef" />
    <HashConflictOverlay ref="hashConflictOverlayRef" />
  </div>
</template>

<script setup lang="ts">
/**
 * 根组件
 * 负责应用初始化、全局样式应用、生命周期管理、平台事件监听
 * 管理背景透明度和界面缩放设置
 */
import { ref, computed, onMounted, onBeforeUnmount, watch, nextTick, defineAsyncComponent, provide } from 'vue'
import { useI18n } from 'vue-i18n'
import { ElMessage } from 'element-plus'
import { listen } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification'
import TitleBar from '@/components/common/TitleBar.vue'
import PillTabs from '@/components/nav/PillTabs.vue'
import UpdateModDataOverlay from '@/components/UpdateModDataOverlay.vue'
import HashConflictOverlay from '@/components/HashConflictOverlay.vue'
import { initPlatform } from '@/stores/platform'
import { useSettingsStore } from '@/stores/settings'
import { useModsStore } from '@/stores/mods'
import { logger } from '@/utils/logger'
import { DEV_MODE } from '@/utils/env'
import { switchTargetGame, checkModCacheValid, isFileWatcherRunning, currentWatchedPath, wasWindowRecreated } from '@/utils/tauri'
import { startPendingNoticeObserver } from '@/utils/pendingNotice'
import { isWatcherOnModsPath } from '@/utils/modPath'
import { clearThumbnailUrlCache } from '@/composables/useThumbnailUrl'
import type { TargetGame, OverlayController } from '@/types'
import { IpcEvent } from '@/events'

/**
 * 三个Tab视图使用异步组件懒加载
 * KeybindsView为默认显示页，首屏需要立即加载
 * ModsView和SettingsView仅在切换到对应Tab时才加载，减小初始包体积
 */
const KeybindsView = defineAsyncComponent(() => import('@/views/KeybindsView.vue'))
const ModsView = defineAsyncComponent(() => import('@/views/ModsView.vue'))
const SettingsView = defineAsyncComponent(() => import('@/views/SettingsView.vue'))

// ===== 前端启动计时（使用main.ts的全局起点） =====
// dev 模式输出启动耗时到控制台；prod 模式静默（编译期消除）
const FE_BOOT_START = window.__NRMM_FE_BOOT_START__ ?? performance.now()
if (DEV_MODE) {
  console.log(`[FE-BOOT] T+${(performance.now() - FE_BOOT_START).toFixed(0).padStart(6)}ms - App.vue <script setup> 开始执行`)
}

/**
 * 向后端报告前端启动阶段
 * @param stage 阶段名称
 */
function reportBootStage(stage: string) {
  const ms = performance.now() - FE_BOOT_START
  if (DEV_MODE) {
    console.log(`[FE-BOOT] T+${ms.toFixed(0).padStart(6)}ms - ${stage}`)
  }
  invoke('report_frontend_ready', { stage, frontendMs: ms }).catch(() => {})
}

const { t } = useI18n()
const settingsStore = useSettingsStore()
const modsStore = useModsStore()
const overlayRef = ref<InstanceType<typeof UpdateModDataOverlay> | null>(null)
provide<OverlayController>('updateOverlay', {
  show: (status, data) => overlayRef.value?.show(status, data),
  hide: () => overlayRef.value?.hide(),
})

const hashConflictOverlayRef = ref<InstanceType<typeof HashConflictOverlay> | null>(null)
provide('hashConflictOverlay', {
  show: () => hashConflictOverlayRef.value?.show(),
  hide: () => hashConflictOverlayRef.value?.hide(),
})
reportBootStage('script-setup-begin')

/** 导航标签页配置 */
const tabs = [
  { key: 'keybinds', label: t('Keybinds') },
  { key: 'mods', label: t('Mods') },
  { key: 'settings', label: t('Settings') }
]

/**
 * 当前激活的标签页，默认为模组页面
 * 值可以是 'keybinds' | 'mods' | 'settings'，对应三个Tab视图
 */
const activeTab = ref<'keybinds' | 'mods' | 'settings'>("mods")
/**
 * 模组页焦点令牌：每次窗口显示或切回模组页时自增。
 * 子组件（GroupPanel/ModGrid）监听此令牌，在焦点回归时把选中的分组居中、
 * 选中的模组卡片滚动到可视范围。
 */
const modsFocusTick = ref(0)
provide('modsFocusTick', modsFocusTick)
/**
 * 暴露给子组件的标签页切换函数（provide/inject 机制）
 * 用于 ModCard 等组件从右键菜单「按键切换」跳转到 Keybinds 页
 */
provide('switchTab', (key: 'keybinds' | 'mods' | 'settings') => {
  activeTab.value = key
})
/**
 * 游戏切换事件（target-game-switched）的取消监听函数
 * 用于在组件卸载时移除 Tauri 事件监听，防止内存泄漏
 */
let unlistenTargetGame: (() => void) | null = null
/**
 * 窗口隐藏事件（window-hidden）的取消监听函数
 * 用于在组件卸载时移除 Tauri 事件监听
 */
let unlistenWindowHidden: (() => void) | null = null
/**
 * 窗口隐藏 120s 后清空模组数据事件（window-hidden-clear-data）的取消监听函数
 * 用于在组件卸载时移除 Tauri 事件监听
 */
let unlistenWindowHiddenClear: (() => void) | null = null
/**
 * 窗口显示事件（window-shown）的取消监听函数
 * 用于在组件卸载时移除 Tauri 事件监听
 */
let unlistenWindowShown: (() => void) | null = null
/**
 * 窗口显示并检测到前台游戏事件（window-show-with-game）的取消监听函数
 * 用于在组件卸载时移除 Tauri 事件监听
 */
let unlistenWindowShowWithGame: (() => void) | null = null
/**
 * 标记窗口显示时是否已通过游戏检测事件处理了模组加载，避免重复加载
 *
 * 流程：window-show-with-game 事件触发 → applyDetectedGameSwitch(game, true) 设置此标记为 true
 *       → 后续 window-shown 事件检测到此标记为 true 则跳过重复加载
 * 重置时机：window-shown 事件处理完后重置为 false
 */
let gameSwitchPending = false
/**
 * 自发切换目标游戏的时间戳（C13 幂等去重依据）
 * applyDetectedGameSwitch 主动切换时记录，后端 target-game-switched 事件回弹
 * 在 ~2s 内且 game 与当前一致时视为回声，跳过二次加载，避免双重 IPC/序列化/内存峰值
 */
let lastSwitchedBySelf = 0
/**
 * 前端冷启动是否源于窗口运行时重建（后端 was_window_recreated 读后清零）
 * 重建冷启动时跳过启动系统通知，避免用户隐藏窗口后再次弹通知构成骚扰。
 */
let windowRecreated = false

/**
 * 通用：切换目标游戏并刷新模组（window-show-with-game / 菜单选择 / 托盘都走这里）
 *
 * 完整流程：
 * 1. 可选标记 gameSwitchPending，防止后续 window-shown 事件重复加载
 * 2. 同步更新前端 settingsStore 的 currentGame/currentModsPath（立即生效，不等后端返回）
 * 3. 异步持久化到后端（switchTargetGame Tauri 命令），不阻塞后续流程
 * 4. 停止旧文件监听器，清空旧模组数据
 * 5. 如果当前在模组页：直接重启监听并加载新游戏模组（nextTick 确保 DOM 更新后执行）
 * 6. 如果不在模组页：仅切换 activeTab 到模组页，ModsView.onMounted 会自动按缓存判断加载
 *
 * @param game 目标游戏（如 'GI' / 'HSR' / 'ZZZ' / 'WW'）
 * @param markPending 是否标记 gameSwitchPending（true 时 window-shown 会跳过重复加载）
 */
async function applyDetectedGameSwitch(game: TargetGame, markPending = true): Promise<void> {
  lastSwitchedBySelf = Date.now()   // C13：标记自发切换，抑制后端事件回弹的二次加载
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
    // 此处仅切换 activeTab 到 mods，不再重复 loadMods（ModsView onMounted 自动按缓存判断）
    activeTab.value = 'mods'
  }
}

/** 背景透明度计算属性，带默认值和类型校验 */
const bgAlpha = computed(() => {
  const v = settingsStore.settings.bgTransparency
  return typeof v === 'number' && !Number.isNaN(v) ? v : 0.3
})

/** 窗口是否透明化（带默认值兜底，提取为 computed 保持模板干净） */
const windowTransparent = computed(() => {
  const v = settingsStore.settings.windowTransparent
  return typeof v === 'boolean' ? v : true
})

/** 界面缩放比例计算属性，带默认值和类型校验 */
const scaleVal = computed(() => {
  const v = settingsStore.settings.interfaceScale
  return typeof v === 'number' && !Number.isNaN(v) ? v : 1.0
})

/**
 * 应用背景透明度到DOM
 * 将透明度值限制在0-1范围内并设置CSS背景色
 *
 * 窗口未透明化时（windowTransparent=false，如 Linux 默认），
 * 强制纯黑背景（rgb(0,0,0)），不随透明度滑块变化，避免 Wayland 合成器异常。
 */
function applyBackground() {
  const el = document.getElementById('app')
  if (!el) return
  if (!windowTransparent.value) {
    el.style.setProperty('background-color', 'rgb(0, 0, 0)')
    return
  }
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
  // dev 模式跳过启动通知：tauri dev 文件监视会在源码变化时重编译并重启应用
  // （实测一轮格式化/检查可触发 1 分钟内十余次重启），每次重启都弹通知构成骚扰；
  // 启动通知仅在正式构建（用户双击/安装启动）时发送
  // windowRecreated 为 true 时表示本次冷启动源于窗口运行时重建（用户隐藏窗口回收后
  // 重新拉起），同样跳过通知，避免用户隐藏窗口后再次弹启动通知构成骚扰
  if (DEV_MODE || windowRecreated) return
  try {
    let permissionGranted = await isPermissionGranted()
    if (!permissionGranted) {
      const permission = await requestPermission()
      permissionGranted = permission === 'granted'
    }
    if (permissionGranted) {
      // 发送系统通知前输出时间戳（用户可感知的关键节点），dev 模式可见
      if (DEV_MODE) {
        console.log(`[FE-BOOT] T+${(performance.now() - FE_BOOT_START).toFixed(0).padStart(6)}ms - >>> 正在发送系统通知（用户可见）<<<`)
      }
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
/** 监听设置变化，实时应用窗口透明化（非透明时切换为纯黑背景） */
watch(windowTransparent, applyBackground, { flush: 'post' })
/** 监听设置变化，实时应用界面缩放 */
watch(scaleVal, applyScale, { flush: 'post' })

/**
 * 切回模组页时自增焦点令牌
 * 触发子组件（GroupPanel/ModGrid）滚动选中分组居中、选中模组卡片到可视范围
 */
watch(activeTab, (tab) => {
  if (tab === 'mods') {
    nextTick(() => {
      modsFocusTick.value++
    })
  }
})

/**
 * 非法 UTF-8 路径跳过提示（后端扫描上报 invalidUtf8Entries）
 *
 * store 在 loadMods/refresh/updateModData 成功后置位报文字符串，
 * 此处消费并弹窗提示；null 表示无跳过（不弹）。弹窗为一次性 toast，
 * 下次状态变化可再次触发（如用户修复文件名后重扫清零再置零不弹）。
 */
watch(
  () => modsStore.invalidUtf8Notice,
  (notice) => {
    if (notice) {
      ElMessage.warning({
        message: notice,
        duration: 8000,
        showClose: true,
        customClass: 'invalid-utf8-toast',
        dangerouslyUseHTMLString: false,
      })
    }
  },
)

onMounted(async () => {
  reportBootStage('dom-mounted')

  // 加载用户设置
  const settingsLoadStart = performance.now()
  try {
    await settingsStore.load()
    if (DEV_MODE) {
      console.log(`[FE-BOOT] T+${(performance.now() - FE_BOOT_START).toFixed(0).padStart(6)}ms - 设置加载完成 (耗时${(performance.now() - settingsLoadStart).toFixed(0)}ms)`)
    }
  } catch (e) {
    logger.error('App', 'settingsStore.load failed', e)
  }
  reportBootStage('settings-loaded')

  // 读取「前端冷启动是否源于窗口运行时重建」标记（读后后端清零）
  // 重建冷启动时不重复弹启动系统通知；非 Tauri 环境（浏览器调试）忽略
  try {
    windowRecreated = await wasWindowRecreated()
  } catch {
    // 非 Tauri 环境忽略
  }

  // 应用已保存的视觉设置
  applyBackground()
  applyScale()
  reportBootStage('visual-settings-applied')

  // 等待DOM首次渲染完成
  await nextTick()
  reportBootStage('first-nexttick')

  // DOM 挂载后启动模态框待处理状态监视（上报后端窗口空闲回收决策）
  startPendingNoticeObserver()

  // 发送启动通知（用户可感知的"启动完成"信号）
  const notifyStart = performance.now()
  sendStartupNotification().then(() => {
    if (DEV_MODE) {
      console.log(`[FE-BOOT] T+${(performance.now() - FE_BOOT_START).toFixed(0).padStart(6)}ms - 系统通知已发送 (耗时${(performance.now() - notifyStart).toFixed(0)}ms)`)
    }
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
    unlistenTargetGame = await listen<TargetGame>(IpcEvent.TargetGameSwitched, async (event) => {
      const game = event.payload
      logger.info('App', `Target game switched to: ${game}`)
      // 幂等去重（C13）：短时间内自发切换的事件回声且 game 与当前一致 → 本地已加载，跳过
      const isEchoOfSelfSwitch = Date.now() - lastSwitchedBySelf < 2000 && game === settingsStore.currentGame
      if (isEchoOfSelfSwitch) {
        logger.debug('App', 'target-game-switched 为自发切换回声，跳过二次加载', { game })
        return
      }
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

  // 监听窗口隐藏事件：保留缓存和监控，不清数据不停止监控
  // Phase 2 策略：窗口隐藏时保留所有缓存数据和文件监控，避免重新加载开销
  // 触发时机：点击关闭按钮、按热键隐藏窗口
  try {
    unlistenWindowHidden = await listen(IpcEvent.WindowHidden, async () => {
      logger.info('App', 'window hidden -> keeping cache and watcher alive')
    })
  } catch (e) {
    logger.warn('App', 'Failed to register window-hidden listener', e)
  }

  // 监听窗口隐藏 120s 后清空模组数据事件：释放前端内存（保留后端缓存与文件监控）
  // 触发时机：窗口隐藏超过 120s 且无待处理模态框，后端回收模组数据后 emit
  try {
    unlistenWindowHiddenClear = await listen(IpcEvent.WindowHiddenClearData, async () => {
      logger.info('App', 'window hidden 120s -> clearing mod data (cache kept)')
      modsStore.clearData()
      clearThumbnailUrlCache() // 释放模块级缩略图 URL 缓存
    })
  } catch (e) {
    logger.warn('App', 'Failed to register window-hidden-clear-data listener', e)
  }

  // 监听窗口显示事件：检查缓存有效性 + 监控状态，决定是否重扫
  // 缓存校验流程：
  //   1. 如果 gameSwitchPending 为 true，说明已通过 window-show-with-game 事件处理，直接跳过
  //   2. 调用后端 checkModCacheValid 命令检查缓存是否仍有效（未被文件监控标记失效）
  //   3. 检查文件监控是否正在运行，以及监控路径是否正确
  //   4. 缓存有效 + 监控正常 → 保留现有数据，无需重新加载
  //   5. 缓存失效或监控异常 → 重启监控 + 完整 loadMods
  // 触发时机：按热键显示窗口（未检测到游戏时）
  try {
    unlistenWindowShown = await listen(IpcEvent.WindowShown, async () => {
      reportBootStage('window-shown')
      logger.info('App', 'window shown -> checking cache validity')

      if (gameSwitchPending) {
        gameSwitchPending = false
        // 已通过 window-show-with-game 完成加载，焦点回归也需滚动
        nextTick(() => {
          modsFocusTick.value++
        })
        return
      }

      const s = settingsStore
      if (!s.currentModsPath || !s.currentGame) {
        logger.warn('App', 'window-shown but no game/path configured, skipping load')
        return
      }

      // C14 竞态兜底：update_mod_data 期间发生 clearData/切游戏（epoch 变化）时，
      // store 侧不回灌加载，改置 pendingReloadAfterRecreate 交给本监听恢复。
      // 同游戏 → 强制 loadMods（后端缓存已被 invalidate_game 失效，必为全量新扫）
      // 成功后 clearNeedUpdate（对齐正常路径「更新→loadMods→清标记」语义）；
      // 已切到其他游戏 → 仅丢弃标志，避免误清新游戏 needUpdate。
      const pendingReloadGame = modsStore.pendingReloadAfterRecreate
      if (pendingReloadGame) {
        modsStore.pendingReloadAfterRecreate = null
        if (pendingReloadGame === s.currentGame) {
          logger.info('App', 'pending reload after recreate -> force loadMods')
          await modsStore.loadMods()
          modsStore.clearNeedUpdate()
          nextTick(() => {
            modsFocusTick.value++
          })
          return
        }
        logger.info('App', `pending reload dropped (game switched: ${pendingReloadGame} -> ${s.currentGame})`)
      }

      try {
        const cacheValid = await checkModCacheValid(s.currentGame, s.currentModsPath)
        const watcherRunning = await isFileWatcherRunning()
        const watchedPath = await currentWatchedPath()

        logger.info('App', `cacheValid=${cacheValid}, watcherRunning=${watcherRunning}, watched=${watchedPath ?? 'null'}`)

        if (cacheValid) {
            if (!watcherRunning) {
              await modsStore.startWatching()
            } else if (!isWatcherOnModsPath(watchedPath, s.currentModsPath)) {
              await modsStore.startWatching()
            }
            // 数据已被 120s 清空（window-hidden-clear-data）但后端缓存仍有效：
            // 直接重载模组数据恢复 UI，避免留下空界面
            if (modsStore.groups.length === 0) {
              await modsStore.loadMods()
              nextTick(() => {
                modsFocusTick.value++
              })
              return
            }
            // 兜底校验：缓存命中时 selectedGroupPath 应已保留，但若数据异常则回退到第一个分组
            if (modsStore.groups.length > 0) {
              const sp = modsStore.selectedGroupPath
              if (!sp || !modsStore.findGroupByPathInList(modsStore.groups, sp)) {
                modsStore.selectGroupByPath(modsStore.groups[0].groupPath)
              }
            }
            nextTick(() => {
              modsFocusTick.value++
            })
            return
          }

          if (!watcherRunning) {
            await modsStore.startWatching()
          }
          await modsStore.loadMods()
          nextTick(() => {
            modsFocusTick.value++
          })
        } catch (e) {
          logger.warn('App', 'window-shown cache check failed, fallback to full load', e)
          await modsStore.startWatching()
          await modsStore.loadMods()
          nextTick(() => {
            modsFocusTick.value++
          })
        }
    })
  } catch (e) {
    logger.warn('App', 'Failed to register window-shown listener', e)
  }

  // 监听窗口显示并检测到前台游戏事件：自动切换到对应游戏并导航到模组页
  // 触发时机：按热键显示窗口且前台是受支持的游戏进程
  try {
    unlistenWindowShowWithGame = await listen<TargetGame>(IpcEvent.WindowShowWithGame, async (event) => {
      const game = event.payload
      logger.info('App', `Window shown with detected game: ${game}`)
      await applyDetectedGameSwitch(game, true)
      // 焦点回归模组页：触发子组件滚动选中分组居中、选中模组可见
      nextTick(() => {
        modsFocusTick.value++
      })
    })
  } catch (e) {
    logger.warn('App', 'Failed to register window-show-with-game listener', e)
  }
})

onBeforeUnmount(() => {
  // 清理所有 Tauri 事件监听器，防止组件卸载后回调仍被执行导致内存泄漏或状态异常
  // 依次清理：target-game-switched / window-hidden / window-hidden-clear-data / window-shown / window-show-with-game
  if (unlistenTargetGame) {
    unlistenTargetGame()
    unlistenTargetGame = null
  }
  if (unlistenWindowHidden) {
    unlistenWindowHidden()
    unlistenWindowHidden = null
  }
  if (unlistenWindowHiddenClear) {
    unlistenWindowHiddenClear()
    unlistenWindowHiddenClear = null
  }
  if (unlistenWindowShown) {
    unlistenWindowShown()
    unlistenWindowShown = null
  }
  if (unlistenWindowShowWithGame) {
    unlistenWindowShowWithGame()
    unlistenWindowShowWithGame = null
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
  --text-primary: rgb(255 255 255 / 1);
  --text-secondary: rgba(255, 255, 255, 0.9);
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

/* ===== ElMessageBox.confirm 项目风格统一 =====
   仅作用于 confirm 对话框（通过 customClass: 'app-confirm-box' 限定作用域），
   复用 App.vue :root 设计令牌（--app-radius / --accent-* / --text-*），
   不引入第三方库，纯 CSS 覆盖、零运行时开销。 */
.app-confirm-box.el-message-box {
  /* 将项目调色板注入 Element Plus 内部变量，驱动按钮/图标/文字 */
  --el-color-primary: var(--accent-primary);
  --el-color-warning: var(--accent-warning);
  --el-color-success: var(--accent-success);
  --el-color-danger: var(--accent-danger);
  --el-color-info: var(--accent-info);
  --el-text-color-primary: var(--text-primary);
  --el-text-color-regular: var(--text-secondary);
  --el-border-color: rgba(255, 255, 255, 0.12);
  --el-border-radius-base: 8px;
  --el-bg-color: #1a1a1a;
  --el-fill-color-blank: #1a1a1a;

  background: #1a1a1a;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: var(--app-radius);
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45);
  padding: 20px 24px 16px;
}
.app-confirm-box.el-message-box .el-message-box__header {
  padding: 0 0 6px;
}
.app-confirm-box.el-message-box .el-message-box__title {
  color: var(--text-primary);
  font-weight: 600;
  font-size: 16px;
}
.app-confirm-box.el-message-box .el-message-box__content {
  color: var(--text-secondary);
  font-size: 14px;
  line-height: 1.6;
  padding: 4px 0;
}
.app-confirm-box.el-message-box .el-message-box__message {
  color: var(--text-secondary);
}
.app-confirm-box.el-message-box .el-message-box__btns {
  padding-top: 14px;
}
/* 取消按钮：复用项目幽灵风格（透明底 + 细边框 + 悬停淡白） */
.app-confirm-box.el-message-box .el-button--default {
  background: transparent;
  border-color: rgba(255, 255, 255, 0.12);
  color: var(--text-secondary);
}
.app-confirm-box.el-message-box .el-button--default:hover,
.app-confirm-box.el-message-box .el-button--default:focus {
  background: rgba(255, 255, 255, 0.06);
  border-color: rgba(255, 255, 255, 0.28);
  color: var(--text-primary);
}

/* 非法 UTF-8 路径跳过提示：允许多行展示（父路径\名称逐行） */
.invalid-utf8-toast .el-message__content {
  white-space: pre-line;
  word-break: break-all;
  line-height: 1.6;
}
</style>
