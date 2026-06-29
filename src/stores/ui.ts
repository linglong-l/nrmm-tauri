import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import type { TabType, ContextMenuState, DialogStates, Notification } from '../types';
import { NOTIFICATION_DURATION } from '../utils/constants';

/**
 * UI Store
 *
 * 集中管理界面层的运行时状态，包括：
 * - 全局加载态与加载提示文案
 * - 窗口可见性、置顶、托盘是否已就绪
 * - 当前激活的功能标签页（mods / keybinds / settings）
 * - 右键上下文菜单的位置与类型
 * - 各类对话框（设置、关于、新增/编辑 Mod/Group、确认删除、Mod 信息）的开关
 * - 通知消息列表及其增删
 * - 输入设备相关状态（是否聚焦于文本框、Shift / Ctrl 按下、是否正在使用键盘）
 *
 * 本 Store 只负责 UI 状态本身，不直接涉及业务数据持久化。
 */
export const useUiStore = defineStore('ui', () => {
  // 是否处于全局加载态（覆盖式遮罩），通常配合 loadingMessage 一同使用。
  const isLoading = ref(false);
  // 加载态下展示的提示文案。
  const loadingMessage = ref('');
  // 窗口当前是否可见（未被最小化 / 隐藏）。
  const isWindowVisible = ref(true);
  // 窗口是否处于置顶状态。
  const isWindowPinned = ref(false);
  // 系统托盘是否已完成初始化。
  const isTraySetup = ref(false);
  // 当前激活的标签页，默认 'mods'。
  const activeTab = ref<TabType>('mods');
  // 右键上下文菜单状态：可见性、坐标、菜单类型、附加数据。
  const contextMenu = ref<ContextMenuState>({
    visible: false,
    x: 0,
    y: 0,
    menuType: null,
    data: null
  });
  // 各对话框的开关状态集合。
  const dialogs = ref<DialogStates>({
    settings: false,
    about: false,
    addMod: false,
    addGroup: false,
    editMod: false,
    editGroup: false,
    confirmDelete: false,
    modInfo: false
  });
  // 当前活跃的通知列表（按加入顺序排列）。
  const notifications = ref<Notification[]>([]);
  // 当前焦点是否位于文本输入框内，用于决定是否拦截某些快捷键。
  const isFocusedOnTextField = ref(false);
  // Shift 键是否处于按下状态。
  const isShiftPressed = ref(false);
  // Ctrl 键是否处于按下状态。
  const isCtrlPressed = ref(false);
  // 当前是否正在使用键盘（而非手柄/鼠标），用于切换交互提示样式。
  const wasUsingKeyboard = ref(false);

  /** 是否存在任何待展示的通知。 */
  const hasNotifications = computed(() => notifications.value.length > 0);

  /**
   * 设置全局加载态及其提示文案。
   * @param loading 是否加载中
   * @param message 加载提示文案，默认为空
   */
  function setLoading(loading: boolean, message = '') {
    isLoading.value = loading;
    loadingMessage.value = message;
  }

  /** 设置窗口可见性。 */
  function setWindowVisible(visible: boolean) {
    isWindowVisible.value = visible;
  }

  /** 设置窗口置顶状态。 */
  function setWindowPinned(pinned: boolean) {
    isWindowPinned.value = pinned;
  }

  /** 切换窗口置顶状态（取反当前值）。 */
  function toggleWindowPinned() {
    isWindowPinned.value = !isWindowPinned.value;
  }

  /** 标记系统托盘是否已就绪。 */
  function setTraySetup(setup: boolean) {
    isTraySetup.value = setup;
  }

  /** 设置当前激活的标签页。 */
  function setActiveTab(tab: TabType) {
    activeTab.value = tab;
  }

  /**
   * 在指定坐标显示右键上下文菜单。
   * @param x 菜单左上角横坐标
   * @param y 菜单左上角纵坐标
   * @param menuType 菜单类型标识，用于决定渲染哪些菜单项
   * @param data 附带给菜单的上下文数据，可选
   */
  function showContextMenu(x: number, y: number, menuType: string, data?: unknown) {
    contextMenu.value = {
      visible: true,
      x,
      y,
      menuType,
      data: data ?? null
    };
  }

  /**
   * 隐藏右键菜单并清除其类型与数据。
   * 保留 x/y 坐标无意义，故仅复位语义字段。
   */
  function hideContextMenu() {
    contextMenu.value.visible = false;
    contextMenu.value.menuType = null;
    contextMenu.value.data = null;
  }

  /** 打开指定对话框。 */
  function showDialog(dialogName: keyof DialogStates) {
    dialogs.value[dialogName] = true;
  }

  /** 关闭指定对话框。 */
  function hideDialog(dialogName: keyof DialogStates) {
    dialogs.value[dialogName] = false;
  }

  /** 切换指定对话框的开关状态。 */
  function toggleDialog(dialogName: keyof DialogStates) {
    dialogs.value[dialogName] = !dialogs.value[dialogName];
  }

  /** 关闭所有对话框。 */
  function closeAllDialogs() {
    Object.keys(dialogs.value).forEach(key => {
      dialogs.value[key as keyof DialogStates] = false;
    });
  }

  /**
   * 添加一条通知并返回其唯一 id。
   * 业务逻辑：
   * 1. 生成基于时间戳 + 随机数的唯一 id；
   * 2. 构造 Notification 对象并追加到列表；
   * 3. 若 duration > 0，则在指定毫秒后自动移除该通知；
   *    duration 为 0 表示常驻，需调用方手动移除。
   * @param type 通知类型：success / error / warning / info
   * @param title 标题
   * @param message 正文
   * @param duration 自动消失时长（毫秒），默认取中等时长
   * @returns 通知 id
   */
  function addNotification(
    type: Notification['type'],
    title: string,
    message: string,
    duration: number = NOTIFICATION_DURATION.medium
  ): string {
    const id = `notif-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
    const notification: Notification = {
      id,
      type,
      title,
      message,
      duration,
      timestamp: Date.now()
    };
    notifications.value.push(notification);

    if (duration > 0) {
      // 自动消失：到达时长后从列表移除
      setTimeout(() => {
        removeNotification(id);
      }, duration);
    }

    return id;
  }

  /**
   * 按 id 移除指定通知。
   * @param id 通知 id
   */
  function removeNotification(id: string) {
    const index = notifications.value.findIndex(n => n.id === id);
    if (index > -1) {
      notifications.value.splice(index, 1);
    }
  }

  /** 清空所有通知。 */
  function clearNotifications() {
    notifications.value = [];
  }

  /** 设置焦点是否位于文本输入框内。 */
  function setFocusedOnTextField(focused: boolean) {
    isFocusedOnTextField.value = focused;
  }

  /** 设置 Shift 键按下状态。 */
  function setShiftPressed(pressed: boolean) {
    isShiftPressed.value = pressed;
  }

  /** 设置 Ctrl 键按下状态。 */
  function setCtrlPressed(pressed: boolean) {
    isCtrlPressed.value = pressed;
  }

  /** 设置是否正在使用键盘。 */
  function setWasUsingKeyboard(using: boolean) {
    wasUsingKeyboard.value = using;
  }

  return {
    isLoading,
    loadingMessage,
    isWindowVisible,
    isWindowPinned,
    isTraySetup,
    activeTab,
    contextMenu,
    dialogs,
    notifications,
    isFocusedOnTextField,
    isShiftPressed,
    isCtrlPressed,
    wasUsingKeyboard,
    hasNotifications,
    setLoading,
    setWindowVisible,
    setWindowPinned,
    toggleWindowPinned,
    setTraySetup,
    setActiveTab,
    showContextMenu,
    hideContextMenu,
    showDialog,
    hideDialog,
    toggleDialog,
    closeAllDialogs,
    addNotification,
    removeNotification,
    clearNotifications,
    setFocusedOnTextField,
    setShiftPressed,
    setCtrlPressed,
    setWasUsingKeyboard
  };
});
