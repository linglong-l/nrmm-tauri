import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import type { AppSettings, LayoutMode, SortGroupMethod, HotkeyKeyboard, HotkeyGamepad, TargetGame } from '../types';
import { invokeGetSettings, invokeSaveSettings, getDefaultSettings } from '../utils/invoke';
import { EventNames, eventManager } from '../utils/events';

/**
 * 应用设置 Store
 *
 * 集中管理 XXMI-NRMM 应用的全部用户配置项，包括：
 * - 热键配置（键盘 / 手柄）
 * - 各游戏的目标进程名与 Mods 路径
 * - 界面外观（缩放、透明度、布局、语言、主题）
 * - 窗口位置与尺寸的持久化
 * - 行为开关（自动生成文件夹图标、自动置顶、游戏外切换菜单等）
 *
 * 采用 Pinia 的组合式（Setup Store）写法定义。所有状态通过 ref 持有，
 * 派生属性通过 computed 暴露，修改逻辑统一通过 setter 函数完成，
 * 持久化则交由 loadSettings / saveSettings 与 Tauri 后端交互。
 */
export const useSettingsStore = defineStore('settings', () => {
  // 应用设置主数据对象。初始化为内置默认值，loadSettings 成功后会被后端数据合并覆盖。
  const settings = ref<AppSettings>(getDefaultSettings());

  // 是否正在从后端加载设置（loadSettings 进行中）。用于在 UI 上显示加载态。
  const isLoading = ref(false);
  // 是否正在向后端保存设置（saveSettings 进行中）。用于禁用保存按钮、显示保存中状态。
  const isSaving = ref(false);
  // 设置是否已完成首次加载。避免在尚未加载完成时使用默认值覆盖后端真实配置。
  const isLoaded = ref(false);

  // ====== 以下为对外暴露的只读派生属性，统一从 settings 中取值，便于组件按需订阅 ======

  /** 当前键盘热键配置（如 Alt+W） */
  const hotkeyKeyboard = computed(() => settings.value.hotkeyKeyboard);
  /** 当前手柄热键配置（如 none / lsB） */
  const hotkeyGamepad = computed(() => settings.value.hotkeyGamepad);
  /** 整体缩放比例（影响 UI 元素大小） */
  const overallScale = computed(() => settings.value.overallScale);
  /** 背景透明度（0~1，1 为完全不透明） */
  const bgTransparency = computed(() => settings.value.bgTransparency);
  /** 布局模式：Auto / Carousel / Grid */
  const layoutMode = computed(() => settings.value.layoutMode);
  /** 界面语言代码，如 'en' / 'zh-CN' */
  const language = computed(() => settings.value.language);
  /** 主题名称，如 'dark' */
  const theme = computed(() => settings.value.theme);
  /** 是否在新增文件夹时自动生成其图标 */
  const isAutoGenerateFolderIcon = computed(() => settings.value.isAutoGenerateFolderIcon);
  /** 是否在显示菜单窗口时自动置顶 */
  const isAutoPinWindow = computed(() => settings.value.isAutoPinWindow);
  /** 当游戏未运行时通过热键切换菜单，是否仍然显示菜单 */
  const showMenuWhenTogglingOutsideGame = computed(() => settings.value.showMenuWhenTogglingOutsideGame);
  /** 快捷键绑定是否以模拟按键方式触发（而非直接调用） */
  const keybindSimulateKeypress = computed(() => settings.value.keybindSimulateKeypress);
  /** 模组分组的排序方式：ByIndex / ByName */
  const sortGroupMethod = computed(() => settings.value.sortGroupMethod);

  /** 设置键盘热键。注意：此处仅修改内存中的值，需另外调用 saveSettings 持久化。 */
  function setHotkeyKeyboard(value: HotkeyKeyboard) {
    settings.value.hotkeyKeyboard = value;
  }

  /** 设置手柄热键。仅修改内存值，需另行保存。 */
  function setHotkeyGamepad(value: HotkeyGamepad) {
    settings.value.hotkeyGamepad = value;
  }

  /** 设置整体缩放比例。 */
  function setOverallScale(value: number) {
    settings.value.overallScale = value;
  }

  /** 设置背景透明度。 */
  function setBgTransparency(value: number) {
    settings.value.bgTransparency = value;
  }

  /** 设置布局模式。 */
  function setLayoutMode(value: LayoutMode) {
    settings.value.layoutMode = value;
  }

  /** 设置界面语言代码。 */
  function setLanguage(value: string) {
    settings.value.language = value;
  }

  /** 设置主题名称。 */
  function setTheme(value: string) {
    settings.value.theme = value;
  }

  /** 设置是否自动生成文件夹图标。 */
  function setAutoGenerateFolderIcon(value: boolean) {
    settings.value.isAutoGenerateFolderIcon = value;
  }

  /** 设置是否自动置顶窗口。 */
  function setAutoPinWindow(value: boolean) {
    settings.value.isAutoPinWindow = value;
  }

  /** 设置游戏外切换菜单时是否显示菜单。 */
  function setShowMenuWhenTogglingOutsideGame(value: boolean) {
    settings.value.showMenuWhenTogglingOutsideGame = value;
  }

  /** 设置快捷键是否以模拟按键方式触发。 */
  function setKeybindSimulateKeypress(value: boolean) {
    settings.value.keybindSimulateKeypress = value;
  }

  /** 设置分组排序方式。 */
  function setSortGroupMethod(value: SortGroupMethod) {
    settings.value.sortGroupMethod = value;
  }

  /**
   * 设置指定游戏对应的目标进程名。
   * 通过 switch 分支将不同的 TargetGame 映射到 settings 内对应字段，
   * 未匹配的游戏将被忽略。
   * @param game 目标游戏枚举值
   * @param processName 进程名，例如 'Wuthering Waves.exe'
   */
  function setTargetProcess(game: TargetGame, processName: string) {
    switch (game) {
      case 'Wuthering_Waves':
        settings.value.targetProcessWuwa = processName;
        break;
      case 'Genshin_Impact':
        settings.value.targetProcessGenshin = processName;
        break;
      case 'Honkai_Star_Rail':
        settings.value.targetProcessHsr = processName;
        break;
      case 'Zenless_Zone_Zero':
        settings.value.targetProcessZzz = processName;
        break;
      case 'Arknights_Endfield':
        settings.value.targetProcessEndfield = processName;
        break;
    }
  }

  /**
   * 获取指定游戏对应的目标进程名。
   * @param game 目标游戏枚举值
   * @returns 进程名；未匹配时返回空字符串
   */
  function getTargetProcess(game: TargetGame): string {
    switch (game) {
      case 'Wuthering_Waves':
        return settings.value.targetProcessWuwa;
      case 'Genshin_Impact':
        return settings.value.targetProcessGenshin;
      case 'Honkai_Star_Rail':
        return settings.value.targetProcessHsr;
      case 'Zenless_Zone_Zero':
        return settings.value.targetProcessZzz;
      case 'Arknights_Endfield':
        return settings.value.targetProcessEndfield;
      default:
        return '';
    }
  }

  /**
   * 设置指定游戏的 Mods 路径。
   * @param game 目标游戏枚举值
   * @param path Mods 文件夹绝对路径
   */
  function setModsPath(game: TargetGame, path: string) {
    switch (game) {
      case 'Wuthering_Waves':
        settings.value.modsPathWuwa = path;
        break;
      case 'Genshin_Impact':
        settings.value.modsPathGenshin = path;
        break;
      case 'Honkai_Star_Rail':
        settings.value.modsPathHsr = path;
        break;
      case 'Zenless_Zone_Zero':
        settings.value.modsPathZzz = path;
        break;
      case 'Arknights_Endfield':
        settings.value.modsPathEndfield = path;
        break;
    }
  }

  /**
   * 获取指定游戏的 Mods 路径。
   * @param game 目标游戏枚举值
   * @returns Mods 文件夹路径；未匹配时返回空字符串
   */
  function getModsPath(game: TargetGame): string {
    switch (game) {
      case 'Wuthering_Waves':
        return settings.value.modsPathWuwa;
      case 'Genshin_Impact':
        return settings.value.modsPathGenshin;
      case 'Honkai_Star_Rail':
        return settings.value.modsPathHsr;
      case 'Zenless_Zone_Zero':
        return settings.value.modsPathZzz;
      case 'Arknights_Endfield':
        return settings.value.modsPathEndfield;
      default:
        return '';
    }
  }

  /**
   * 同时设置窗口的宽高，用于窗口尺寸持久化。
   * @param width 宽度（像素）
   * @param height 高度（像素）
   */
  function setWindowSize(width: number, height: number) {
    settings.value.savedWindowWidth = width;
    settings.value.savedWindowHeight = height;
  }

  /**
   * 同时设置窗口左上角坐标，用于窗口位置持久化。
   * @param x 横坐标
   * @param y 纵坐标
   */
  function setWindowPosition(x: number, y: number) {
    settings.value.savedWindowX = x;
    settings.value.savedWindowY = y;
  }

  /**
   * 以浅合并方式批量更新设置项。
   * 适用于一次性更新多个字段的场景；不会触发自动保存。
   * @param newSettings 需要覆盖的字段集合
   */
  function updateSettings(newSettings: Partial<AppSettings>) {
    settings.value = { ...settings.value, ...newSettings };
  }

  /**
   * 从后端加载设置。
   * 业务逻辑：
   * 1. 置 isLoading 为 true；
   * 2. 调用 invokeGetSettings 拉取后端持久化的设置；
   * 3. 成功时以默认设置为基底合并后端数据，保证新增字段有默认值；
   * 4. 失败时回退为默认设置，保证应用可用；
   * 5. 无论成败均标记 isLoaded 为 true，并在 finally 中复位 isLoading。
   * @returns 是否加载成功
   */
  async function loadSettings(): Promise<boolean> {
    isLoading.value = true;
    try {
      const loadedSettings = await invokeGetSettings();
      if (loadedSettings) {
        // 以默认值为基底合并后端数据，确保后端缺失的新增字段仍取默认值
        settings.value = { ...getDefaultSettings(), ...loadedSettings };
      }
      isLoaded.value = true;
      return true;
    } catch {
      // 加载失败时回退到默认设置，避免界面因无数据而异常
      settings.value = getDefaultSettings();
      isLoaded.value = true;
      return false;
    } finally {
      isLoading.value = false;
    }
  }

  /**
   * 将当前设置保存到后端。
   * 业务逻辑：
   * 1. 置 isSaving 为 true；
   * 2. 调用 invokeSaveSettings 持久化整个 settings 对象；
   * 3. 成功后广播 SETTINGS_UPDATED 事件，通知其它模块响应设置变化；
   * 4. 失败时返回 false（不抛出），由调用方决定如何提示。
   * @returns 是否保存成功
   */
  async function saveSettings(): Promise<boolean> {
    isSaving.value = true;
    try {
      await invokeSaveSettings(settings.value);
      // 保存成功后通知全局，便于热键、UI 等模块同步刷新
      eventManager.emit(EventNames.SETTINGS_UPDATED, undefined);
      return true;
    } catch {
      return false;
    } finally {
      isSaving.value = false;
    }
  }

  /**
   * 更新单个设置字段并立即保存。
   * @param key AppSettings 的字段名
   * @param value 对应字段的新值
   * @returns 保存是否成功
   */
  async function updateSetting<K extends keyof AppSettings>(
    key: K,
    value: AppSettings[K]
  ): Promise<boolean> {
    settings.value[key] = value;
    return saveSettings();
  }

  /**
   * 将设置重置为内置默认值。
   * 注意：此操作仅修改内存，不会自动保存到后端，需另行调用 saveSettings。
   */
  function resetToDefaults() {
    settings.value = getDefaultSettings();
  }

  return {
    settings,
    isLoading,
    isSaving,
    isLoaded,
    hotkeyKeyboard,
    hotkeyGamepad,
    overallScale,
    bgTransparency,
    layoutMode,
    language,
    theme,
    isAutoGenerateFolderIcon,
    isAutoPinWindow,
    showMenuWhenTogglingOutsideGame,
    keybindSimulateKeypress,
    sortGroupMethod,
    setHotkeyKeyboard,
    setHotkeyGamepad,
    setOverallScale,
    setBgTransparency,
    setLayoutMode,
    setLanguage,
    setTheme,
    setAutoGenerateFolderIcon,
    setAutoPinWindow,
    setShowMenuWhenTogglingOutsideGame,
    setKeybindSimulateKeypress,
    setSortGroupMethod,
    setTargetProcess,
    getTargetProcess,
    setModsPath,
    getModsPath,
    setWindowSize,
    setWindowPosition,
    updateSettings,
    loadSettings,
    saveSettings,
    updateSetting,
    resetToDefaults
  };
});
