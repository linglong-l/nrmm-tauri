/**
 * 应用设置状态管理Store
 *
 * 职责：
 * - 从后端加载/保存应用配置
 * - 管理当前选中游戏、模组路径、目标进程名
 * - 提供设置的响应式访问
 * - 设置修改后debounced自动保存（300ms延迟）
 *
 * 注意：设置保存通过后端tauri命令持久化到本地配置文件
 */
import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { getSettings, saveSettings, reregisterHotkeys } from '../utils/tauri'
import { logger } from '../utils/logger'
import type { AppSettings, PlatformInfo, TargetGame } from '../types'

/** 各游戏默认进程名（fallback，优先使用后端fill_defaults返回的值） */
const FALLBACK_PROCESS_DEFAULTS: Record<TargetGame, string> = {
  GenshinImpact: 'GenshinImpact.exe',
  HonkaiStarRail: 'StarRail.exe',
  Wuwa: 'Client-Win64-Shipping.exe',
  ZZZ: 'ZenlessZoneZero.exe',
  HonkaiImpact3rd: 'BH3.exe',
  ArknightsEndfield: 'Endfield.exe',
}

export const useSettingsStore = defineStore('settings', () => {
  /** 应用完整设置对象（从后端加载） */
  const settings = ref<Partial<AppSettings>>({})
  /** 平台信息（OS类型、按键模拟支持等） */
  const platformInfo = ref<PlatformInfo | null>(null)
  /** 当前选中的目标游戏 */
  const currentGame = ref<TargetGame>('GenshinImpact')
  /** 设置是否已加载完成 */
  const loaded = ref(false)
  /** 选择模组时是否模拟按键 */
  const simulateKeyOnSelection = ref(true)
  /** 防抖保存定时器 */
  let saveTimer: ReturnType<typeof setTimeout> | null = null
  /**
   * 已同步到后端（即已注册生效）的窗口切换热键。
   *
   * 背景：`saveNow()` 原先在**每次**保存设置后无条件调用 `reregisterHotkeys()`，
   * 而后端 `register_all()` 会先 unregister_all 再重新注册 22 个全局热键。
   * 启动时的游戏自动检测（`App.vue::applyDetectedGameSwitch` → `switchGame()`）
   * 会触发一次设置保存，于是每次启动/页面重启都会重复看到全部热键卸载+重注册，
   * 并伴随 22 行 "Registered hotkey" 日志。
   * 其实只有 `windowHotkey` 变化才需要重注册，故用此变量做变更检测。
   */
  let registeredWindowHotkey: string | null = null

  /**
   * 当前游戏的模组文件夹路径（computed）
   * 从settings.gameModsPath中读取当前游戏的路径
   */
  const currentModsPath = computed<string>(() => {
    const gameMods = settings.value.gameModsPath
    if (!gameMods) return ''
    return gameMods[currentGame.value] || ''
  })

  /**
   * 当前游戏的目标进程名（computed）
   * 从settings.targetProcessPerGame中读取，没有则使用fallback默认值
   */
  const currentTargetProcess = computed<string>(() => {
    const procMap = settings.value.targetProcessPerGame
    if (procMap && procMap[currentGame.value]) {
      return procMap[currentGame.value]
    }
    return FALLBACK_PROCESS_DEFAULTS[currentGame.value] || ''
  })

  /**
   * 从后端加载设置
   * 调用get_settings命令读取配置文件，解析后更新store状态
   * 同时初始化currentGame
   */
  async function load() {
    logger.debug('SettingsStore', 'loadSettings started')
    try {
      const loadedSettings = await getSettings() as AppSettings
      settings.value = loadedSettings
      // 启动时后端已完成一次全局热键注册，记录当前生效值以免首次保存触发无谓重注册
      registeredWindowHotkey = loadedSettings.windowHotkey ?? ''
      if (loadedSettings.targetGame) {
        currentGame.value = loadedSettings.targetGame
      }
      simulateKeyOnSelection.value = loadedSettings.simulateKeyOnSelection ?? true
      loaded.value = true
      logger.debug('SettingsStore', 'loadSettings completed')
      logger.info('SettingsStore', 'Settings loaded successfully')
    } catch (e) {
      logger.error('SettingsStore', 'Failed to load settings', e)
    }
  }

  /**
   * 立即保存设置到后端（不防抖）
   * 调用save_settings命令将当前settings写入配置文件
   * 保存成功后重新注册全局热键，确保热键配置即时生效
   */
  async function saveNow() {
    logger.debug('SettingsStore', 'saveSettings started')
    try {
      settings.value.simulateKeyOnSelection = simulateKeyOnSelection.value
      // 保存前捕获窗口热键：仅当它真正变化时才重注册全局热键
      // （其余设置如模组路径/目标进程与全局热键无关，无需卸载+重注册 22 个热键）
      const prevHotkey = registeredWindowHotkey
      const nextHotkey = settings.value.windowHotkey ?? ''
      await saveSettings(settings.value)
      if (nextHotkey !== prevHotkey) {
        registeredWindowHotkey = nextHotkey
        // 热键变更才重新注册（如windowHotkey变更），不阻塞UI
        reregisterHotkeys(nextHotkey).catch(e => logger.warn('SettingsStore', 'Failed to reregister hotkeys after save', e))
      }
      logger.debug('SettingsStore', 'saveSettings completed')
    } catch (e) {
      logger.error('SettingsStore', 'Failed to save settings', e)
    }
  }

  /**
   * 防抖保存设置
   * 300ms延迟后自动保存，避免滑块拖动等连续操作时频繁写入磁盘
   */
  function save() {
    if (saveTimer) clearTimeout(saveTimer)
    saveTimer = setTimeout(() => {
      saveNow()
    }, 300)
  }

  /**
   * 切换当前游戏
   * 更新currentGame，并同步settings.targetGame
   * @param game 目标游戏类型
   */
  function switchGame(game: TargetGame) {
    currentGame.value = game
    if (settings.value) {
      settings.value.targetGame = game
    }
    save()
  }

  /**
   * 更新当前游戏的模组路径
   * @param path 模组文件夹绝对路径
   */
  function setCurrentModsPath(path: string) {
    if (!settings.value.gameModsPath) {
      settings.value.gameModsPath = {} as Record<TargetGame, string>
    }
    settings.value.gameModsPath[currentGame.value] = path
    save()
  }

  /**
   * 更新当前游戏的目标进程名
   * @param processName 目标进程名（如 StarRail.exe）
   */
  function setCurrentTargetProcess(processName: string) {
    if (!settings.value.targetProcessPerGame) {
      settings.value.targetProcessPerGame = {} as Record<TargetGame, string>
    }
    settings.value.targetProcessPerGame[currentGame.value] = processName
    save()
  }

  return {
    settings,
    platformInfo,
    currentGame,
    currentModsPath,
    currentTargetProcess,
    loaded,
    simulateKeyOnSelection,
    load,
    save,
    saveNow,
    switchGame,
    setCurrentModsPath,
    setCurrentTargetProcess,
  }
})
