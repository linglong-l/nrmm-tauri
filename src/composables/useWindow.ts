// 窗口操作封装组合式函数模块。
// 该模块对 ui store 与底层 Tauri 窗口命令进行二次封装，
// 统一暴露窗口显示/隐藏/置顶/位置尺寸等操作、右键菜单与对话框管理、通知系统以及相关事件订阅能力。
import { storeToRefs } from 'pinia';
import { useUiStore } from '../stores/ui';
import { invokeShowWindow, invokeHideWindow, invokePinWindow, invokeIsWindowPinned, invokeGetWindowPosition, invokeSetWindowPosition, invokeSetWindowSize, invokeOpenPath } from '../utils/invoke';
import type { WindowPosition, TabType, DialogStates } from '../types';
import { useEvent, EventNames } from '../utils/events';

/**
 * 窗口与 UI 状态组合式函数。
 *
 * 作用：
 * - 通过 `storeToRefs` 把 ui store 中的状态转为可响应式引用，便于在组件中解构使用；
 * - 集中提供窗口显隐、置顶、位置尺寸、标签页切换、右键菜单、对话框、加载态、通知等 UI 能力；
 * - 桥接全局事件总线，提供窗口显示/隐藏事件的订阅入口；
 * - 所有窗口操作均会同步更新 store 状态并分发对应事件，保证 UI 与后端窗口状态一致。
 *
 * 限制条件：
 * - 必须在 Pinia 已初始化的上下文中调用（通常在 setup 函数内）；
 * - 窗口操作依赖 Tauri 后端命令，非 Tauri 环境下相关调用会被忽略；
 * - 所有错误均被静默吞掉（catch 块为空），仅返回安全默认值，调用方无法直接感知失败。
 *
 * @returns 包含窗口与 UI 响应式状态及一组操作方法的对象
 */
export function useWindow() {
  // 获取 ui store 实例，所有 UI 状态由该 store 维护
  const uiStore = useUiStore();
  // 取出事件订阅/分发方法，用于桥接窗口相关事件
  const { on, emit } = useEvent();

  // 将 store 中的 state/getter 转为 ref，保证解构后仍具响应性
  const {
    // 主窗口是否可见
    isWindowVisible,
    // 主窗口是否已置顶
    isWindowPinned,
    // 当前激活的标签页（keybinds/mods/settings）
    activeTab,
    // 右键菜单状态（位置、类型、携带数据）
    contextMenu,
    // 各类对话框的开关状态集合
    dialogs,
    // 是否处于全局加载态
    isLoading,
    // 全局加载态的提示文案
    loadingMessage,
    // 通知列表
    notifications,
    // 是否存在任意通知（getter）
    hasNotifications,
    // 焦点是否在文本输入框上（影响热键拦截）
    isFocusedOnTextField,
    // Shift 键是否按下
    isShiftPressed,
    // Ctrl 键是否按下
    isCtrlPressed,
    // 最近一次输入是否使用键盘（用于切换键盘/手柄交互模式）
    wasUsingKeyboard
  } = storeToRefs(uiStore);

  /**
   * 显示主窗口。
   * 调用后端显示窗口命令，更新前端可见状态，并分发 WINDOW_SHOWN 事件。
   * 失败时静默忽略，不影响其他逻辑。
   */
  async function showWindow(): Promise<void> {
    try {
      await invokeShowWindow();
      uiStore.setWindowVisible(true);
      emit(EventNames.WINDOW_SHOWN, { visible: true, source: 'frontend' });
    } catch {
      // ignore
    }
  }

  /**
   * 隐藏主窗口。
   * 调用后端隐藏窗口命令，更新前端可见状态，并分发 WINDOW_HIDDEN 事件。
   * 失败时静默忽略。
   */
  async function hideWindow(): Promise<void> {
    try {
      await invokeHideWindow();
      uiStore.setWindowVisible(false);
      emit(EventNames.WINDOW_HIDDEN, { visible: false, source: 'frontend' });
    } catch {
      // ignore
    }
  }

  /**
   * 根据当前可见状态切换窗口显示/隐藏。
   */
  async function toggleWindow(): Promise<void> {
    if (isWindowVisible.value) {
      await hideWindow();
    } else {
      await showWindow();
    }
  }

  /**
   * 设置窗口置顶状态。
   * @param pinned 是否置顶
   */
  async function pinWindow(pinned: boolean): Promise<void> {
    try {
      await invokePinWindow(pinned);
      uiStore.setWindowPinned(pinned);
    } catch {
      // ignore
    }
  }

  /**
   * 切换窗口置顶状态（取反当前值）。
   */
  async function togglePinWindow(): Promise<void> {
    await pinWindow(!isWindowPinned.value);
  }

  /**
   * 向后端查询窗口当前是否处于置顶状态，并同步到前端状态。
   * @returns 是否置顶；查询失败时返回 false
   */
  async function checkWindowPinned(): Promise<boolean> {
    try {
      const pinned = await invokeIsWindowPinned();
      uiStore.setWindowPinned(pinned);
      return pinned;
    } catch {
      return false;
    }
  }

  /**
   * 获取窗口当前位置与尺寸。
   * 成功后会分发 WINDOW_POSITION_CHANGED 事件。
   * @returns 窗口位置信息；失败时返回 null
   */
  async function getWindowPosition(): Promise<WindowPosition | null> {
    try {
      const pos = await invokeGetWindowPosition();
      emit(EventNames.WINDOW_POSITION_CHANGED, pos);
      return pos;
    } catch {
      return null;
    }
  }

  /**
   * 设置窗口位置。
   * 注意：分发事件时 width/height 占位为 0，仅作为位置变化通知。
   * @param x 窗口左上角 X 坐标
   * @param y 窗口左上角 Y 坐标
   */
  async function setWindowPosition(x: number, y: number): Promise<void> {
    try {
      await invokeSetWindowPosition(x, y);
      emit(EventNames.WINDOW_POSITION_CHANGED, { x, y, width: 0, height: 0 });
    } catch {
      // ignore
    }
  }

  /**
   * 设置窗口尺寸。
   * 成功后分发 WINDOW_SIZE_CHANGED 事件。
   * @param width 窗口宽度
   * @param height 窗口高度
   */
  async function setWindowSize(width: number, height: number): Promise<void> {
    try {
      await invokeSetWindowSize(width, height);
      emit(EventNames.WINDOW_SIZE_CHANGED, { width, height });
    } catch {
      // ignore
    }
  }

  /**
   * 切换当前激活的标签页。
   * @param tab 标签页类型（keybinds/mods/settings）
   */
  function setActiveTab(tab: TabType) {
    uiStore.setActiveTab(tab);
  }

  /**
   * 在指定屏幕坐标显示右键菜单。
   * @param x 菜单左上角 X 坐标（屏幕坐标）
   * @param y 菜单左上角 Y 坐标（屏幕坐标）
   * @param menuType 菜单类型标识（用于区分不同右键场景）
   * @param data 菜单附带数据（类型随 menuType 变化，可选）
   */
  function showContextMenu(x: number, y: number, menuType: string, data?: unknown) {
    uiStore.showContextMenu(x, y, menuType, data);
  }

  /**
   * 隐藏右键菜单。
   */
  function hideContextMenu() {
    uiStore.hideContextMenu();
  }

  /**
   * 显示指定对话框。
   * @param dialogName 对话框名称（DialogStates 的键）
   */
  function showDialog(dialogName: keyof DialogStates) {
    uiStore.showDialog(dialogName);
  }

  /**
   * 隐藏指定对话框。
   * @param dialogName 对话框名称
   */
  function hideDialog(dialogName: keyof DialogStates) {
    uiStore.hideDialog(dialogName);
  }

  /**
   * 切换指定对话框的显隐状态。
   * @param dialogName 对话框名称
   */
  function toggleDialog(dialogName: keyof DialogStates) {
    uiStore.toggleDialog(dialogName);
  }

  /**
   * 关闭所有对话框。
   * 通常在切换标签页或全局点击遮罩时调用。
   */
  function closeAllDialogs() {
    uiStore.closeAllDialogs();
  }

  /**
   * 设置全局加载态及提示文案。
   * @param loading 是否处于加载态
   * @param message 加载提示文案（默认空字符串）
   */
  function setLoading(loading: boolean, message = '') {
    uiStore.setLoading(loading, message);
  }

  /**
   * 添加一条通知消息。
   * @param type 通知类型（success/error/warning/info）
   * @param title 通知标题
   * @param message 通知正文
   * @param duration 自动关闭时长（毫秒），未传时使用 store 默认值
   * @returns 通知唯一 ID
   */
  function addNotification(
    type: 'success' | 'error' | 'warning' | 'info',
    title: string,
    message: string,
    duration?: number
  ): string {
    const id = uiStore.addNotification(type, title, message, duration);
    return id;
  }

  /**
   * 按 ID 移除一条通知。
   * @param id 通知唯一 ID
   */
  function removeNotification(id: string) {
    uiStore.removeNotification(id);
  }

  /**
   * 清空全部通知。
   */
  function clearNotifications() {
    uiStore.clearNotifications();
  }

  /**
   * 调用系统默认程序打开指定路径（文件或目录）。
   * 失败时静默忽略。
   * @param path 待打开的路径
   */
  async function openPath(path: string): Promise<void> {
    try {
      await invokeOpenPath(path);
    } catch {
      // ignore
    }
  }

  /**
   * 设置焦点是否在文本输入框上。
   * 用于在文本输入时屏蔽全局热键，避免误触发。
   * @param focused 是否聚焦
   */
  function setFocusedOnTextField(focused: boolean) {
    uiStore.setFocusedOnTextField(focused);
  }

  /**
   * 设置 Shift 键按下状态。
   * @param pressed 是否按下
   */
  function setShiftPressed(pressed: boolean) {
    uiStore.setShiftPressed(pressed);
  }

  /**
   * 设置 Ctrl 键按下状态。
   * @param pressed 是否按下
   */
  function setCtrlPressed(pressed: boolean) {
    uiStore.setCtrlPressed(pressed);
  }

  /**
   * 设置"最近一次是否使用键盘输入"标记。
   * 用于在键盘/手柄交互模式之间切换。
   * @param using 是否使用键盘
   */
  function setWasUsingKeyboard(using: boolean) {
    uiStore.setWasUsingKeyboard(using);
  }

  /**
   * 订阅窗口显示事件。
   * @param callback 窗口显示时的回调
   * @returns 取消订阅函数
   */
  async function onWindowShown(callback: () => void): Promise<() => void> {
    return on(EventNames.WINDOW_SHOWN, () => {
      callback();
    });
  }

  /**
   * 订阅窗口隐藏事件。
   * @param callback 窗口隐藏时的回调
   * @returns 取消订阅函数
   */
  async function onWindowHidden(callback: () => void): Promise<() => void> {
    return on(EventNames.WINDOW_HIDDEN, () => {
      callback();
    });
  }

  // 统一返回响应式状态与方法，供调用方按需解构使用
  return {
    isWindowVisible,
    isWindowPinned,
    activeTab,
    contextMenu,
    dialogs,
    isLoading,
    loadingMessage,
    notifications,
    hasNotifications,
    isFocusedOnTextField,
    isShiftPressed,
    isCtrlPressed,
    wasUsingKeyboard,
    showWindow,
    hideWindow,
    toggleWindow,
    pinWindow,
    togglePinWindow,
    checkWindowPinned,
    getWindowPosition,
    setWindowPosition,
    setWindowSize,
    setActiveTab,
    showContextMenu,
    hideContextMenu,
    showDialog,
    hideDialog,
    toggleDialog,
    closeAllDialogs,
    setLoading,
    addNotification,
    removeNotification,
    clearNotifications,
    openPath,
    setFocusedOnTextField,
    setShiftPressed,
    setCtrlPressed,
    setWasUsingKeyboard,
    onWindowShown,
    onWindowHidden
  };
}
