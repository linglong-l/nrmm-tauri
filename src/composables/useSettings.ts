// 设置组合式函数模块。
// 该模块对 settings store 进行二次封装，统一暴露应用配置相关的响应式状态与读写方法。
// 通过该 composable，组件层无需直接依赖 Pinia store 即可完成配置的加载、保存、单项更新与重置。
import { storeToRefs } from 'pinia';
import { useSettingsStore } from '../stores/settings';
import type { AppSettings, HotkeyKeyboard, HotkeyGamepad, LayoutMode, SortGroupMethod, TargetGame } from '../types';

/**
 * 应用设置组合式函数。
 *
 * 作用：
 * - 通过 `storeToRefs` 把 settings store 中的状态转为可响应式引用，便于在组件中解构使用；
 * - 集中提供应用全局配置（热键、缩放、透明度、布局、语言、主题、Mods 路径、窗口尺寸位置等）的读写能力；
 * - 所有写操作最终都会落到后端持久化文件，保证配置在重启后恢复。
 *
 * 限制条件：
 * - 必须在 Pinia 已初始化的上下文中调用（通常在 setup 函数内）；
 * - 部分配置（如窗口尺寸/位置）的生效依赖后端 Tauri 命令；
 * - 单字段更新会触发后端写入，频繁调用可能产生 IO 开销，必要时使用 `updateSettings` 批量更新。
 *
 * @returns 包含设置响应式状态与一组读写方法的对象
 */
export function useSettings() {
  // 获取 settings store 实例，所有状态与后端持久化由该 store 承载
  const settingsStore = useSettingsStore();

  // 将 store 中的 state/getter 转为 ref，保证解构后仍具响应性
  const {
    // 完整的应用设置对象（与后端 settings 文件一一对应）
    settings,
    // 是否正在加载配置
    isLoading,
    // 是否正在保存配置
    isSaving,
    // 配置是否已加载完成
    isLoaded,
    // 键盘热键配置
    hotkeyKeyboard,
    // 手柄热键配置
    hotkeyGamepad,
    // 整体界面缩放比例
    overallScale,
    // 背景透明度（0-1）
    bgTransparency,
    // 界面布局模式（Auto/Carousel/Grid）
    layoutMode,
    // 界面语言代码
    language,
    // 主题名称
    theme,
    // 是否自动生成分组文件夹图标
    isAutoGenerateFolderIcon,
    // 是否自动置顶窗口
    isAutoPinWindow,
    // 游戏外切换时是否显示菜单
    showMenuWhenTogglingOutsideGame,
    // 是否通过模拟按键方式触发 keybind
    keybindSimulateKeypress,
    // 分组排序方式（按索引/按名称）
    sortGroupMethod
  } = storeToRefs(settingsStore);

  /**
   * 从后端加载应用设置。
   * 通常在应用启动时调用一次，加载完成后 `isLoaded` 会被置为 true。
   * @returns 是否加载成功
   */
  async function loadSettings(): Promise<boolean> {
    return settingsStore.loadSettings();
  }

  /**
   * 将当前设置保存到后端持久化文件。
   * @returns 是否保存成功
   */
  async function saveSettings(): Promise<boolean> {
    return settingsStore.saveSettings();
  }

  /**
   * 更新单个设置项并立即持久化。
   * @param key 设置项字段名（AppSettings 的键）
   * @param value 该字段的新值
   * @returns 是否更新成功
   */
  async function updateSetting<K extends keyof AppSettings>(
    key: K,
    value: AppSettings[K]
  ): Promise<boolean> {
    return settingsStore.updateSetting(key, value);
  }

  /**
   * 批量更新多个设置项（仅修改内存中的状态，需另行调用 save 持久化）。
   * 适用于一次性调整多个相关配置，减少多次单字段更新带来的 IO 开销。
   * @param newSettings 待覆盖的设置项集合（部分字段）
   */
  function updateSettings(newSettings: Partial<AppSettings>) {
    settingsStore.updateSettings(newSettings);
  }

  /**
   * 设置键盘热键配置。
   * @param value 键盘热键枚举值
   */
  function setHotkeyKeyboard(value: HotkeyKeyboard) {
    settingsStore.setHotkeyKeyboard(value);
  }

  /**
   * 设置手柄热键配置。
   * @param value 手柄热键枚举值
   */
  function setHotkeyGamepad(value: HotkeyGamepad) {
    settingsStore.setHotkeyGamepad(value);
  }

  /**
   * 设置整体界面缩放比例。
   * @param value 缩放比例数值
   */
  function setOverallScale(value: number) {
    settingsStore.setOverallScale(value);
  }

  /**
   * 设置背景透明度。
   * @param value 透明度数值（0-1）
   */
  function setBgTransparency(value: number) {
    settingsStore.setBgTransparency(value);
  }

  /**
   * 设置界面布局模式。
   * @param value 布局模式枚举值
   */
  function setLayoutMode(value: LayoutMode) {
    settingsStore.setLayoutMode(value);
  }

  /**
   * 设置界面语言代码。
   * @param value 语言代码（如 zh-CN）
   */
  function setLanguage(value: string) {
    settingsStore.setLanguage(value);
  }

  /**
   * 设置主题名称。
   * @param value 主题名称字符串
   */
  function setTheme(value: string) {
    settingsStore.setTheme(value);
  }

  /**
   * 设置是否自动生成分组文件夹图标。
   * @param value 是否启用
   */
  function setAutoGenerateFolderIcon(value: boolean) {
    settingsStore.setAutoGenerateFolderIcon(value);
  }

  /**
   * 设置是否自动置顶窗口。
   * @param value 是否启用
   */
  function setAutoPinWindow(value: boolean) {
    settingsStore.setAutoPinWindow(value);
  }

  /**
   * 设置游戏外切换时是否显示菜单。
   * @param value 是否启用
   */
  function setShowMenuWhenTogglingOutsideGame(value: boolean) {
    settingsStore.setShowMenuWhenTogglingOutsideGame(value);
  }

  /**
   * 设置是否通过模拟按键方式触发 keybind。
   * @param value 是否启用
   */
  function setKeybindSimulateKeypress(value: boolean) {
    settingsStore.setKeybindSimulateKeypress(value);
  }

  /**
   * 设置分组排序方式。
   * @param value 排序方式枚举值
   */
  function setSortGroupMethod(value: SortGroupMethod) {
    settingsStore.setSortGroupMethod(value);
  }

  /**
   * 设置指定游戏的目标进程名。
   * @param game 目标游戏
   * @param processName 进程名
   */
  function setTargetProcess(game: TargetGame, processName: string) {
    settingsStore.setTargetProcess(game, processName);
  }

  /**
   * 获取指定游戏的目标进程名。
   * @param game 目标游戏
   * @returns 进程名字符串
   */
  function getTargetProcess(game: TargetGame): string {
    return settingsStore.getTargetProcess(game);
  }

  /**
   * 设置指定游戏的 Mods 目录路径。
   * @param game 目标游戏
   * @param path Mods 目录绝对路径
   */
  function setModsPath(game: TargetGame, path: string) {
    settingsStore.setModsPath(game, path);
  }

  /**
   * 获取指定游戏的 Mods 目录路径。
   * @param game 目标游戏
   * @returns Mods 目录绝对路径
   */
  function getModsPath(game: TargetGame): string {
    return settingsStore.getModsPath(game);
  }

  /**
   * 设置窗口尺寸（同时更新内存状态，用于下次启动时恢复）。
   * @param width 窗口宽度
   * @param height 窗口高度
   */
  function setWindowSize(width: number, height: number) {
    settingsStore.setWindowSize(width, height);
  }

  /**
   * 设置窗口位置（同时更新内存状态，用于下次启动时恢复）。
   * @param x 窗口左上角 X 坐标
   * @param y 窗口左上角 Y 坐标
   */
  function setWindowPosition(x: number, y: number) {
    settingsStore.setWindowPosition(x, y);
  }

  /**
   * 将所有设置重置为默认值。
   * 注意：重置后通常仍需调用 saveSettings 以持久化到后端文件。
   */
  function resetToDefaults() {
    settingsStore.resetToDefaults();
  }

  // 统一返回响应式状态与方法，供调用方按需解构使用
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
    loadSettings,
    saveSettings,
    updateSetting,
    updateSettings,
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
    resetToDefaults
  };
}
