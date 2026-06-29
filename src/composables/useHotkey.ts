// 热键组合式函数模块。
// 该模块对 hotkey store 进行二次封装，统一暴露热键相关的响应式状态与方法，
// 供组件层在不直接依赖 Pinia store 的情况下完成热键的注册、注销、启用/禁用以及事件监听。
import { storeToRefs } from 'pinia';
import { useHotkeyStore } from '../stores/hotkey';
import type { HotkeyKeyboard, HotkeyGamepad } from '../types';
import { useEvent, EventNames } from '../utils/events';

/**
 * 热键组合式函数。
 *
 * 作用：
 * - 通过 `storeToRefs` 将 hotkey store 中的状态转为可响应式引用，保证在组件中解构后仍保持响应性；
 * - 提供热键的注册/注销、启用/禁用、状态查询及事件订阅/分发等能力；
 * - 同时桥接全局事件总线，使组件可监听热键按下事件。
 *
 * 限制条件：
 * - 必须在 Pinia 已初始化的上下文中调用（通常在 setup 函数内）；
 * - 后端热键注册为全局热键，需操作系统层面授权；
 * - 同一按键重复注册会由 store 层进行去重/拒绝处理。
 *
 * @returns 包含热键响应式状态与一组操作方法的对象
 */
export function useHotkey() {
  // 获取 hotkey store 实例，所有状态与后端交互均由该 store 承载
  const hotkeyStore = useHotkeyStore();
  // 取出事件订阅/分发方法，用于桥接 HOTKEY_PRESSED 事件
  const { on, emit } = useEvent();

  // 将 store 中的 state/getter 转为 ref，保证解构后仍具响应性
  const {
    // 已注册的热键集合（键为热键标识字符串）
    registeredHotkeys,
    // 热键功能整体是否启用（关闭后所有已注册热键暂不触发）
    isHotkeyEnabled,
    // 最近一次按下的热键标识，未触发时为 null
    lastHotkeyPressed,
    // 最近一次按下的时间戳（毫秒），未触发时为 null
    lastHotkeyPressedTime,
    // 当前生效的键盘热键配置
    currentKeyboardHotkey,
    // 当前生效的手柄热键配置
    currentGamepadHotkey,
    // 左摇杆是否已触发（用于手柄组合键判定，避免重复触发）
    leftThumbWasTriggered,
    // 热键运行时聚合状态（注册/启用/最近触发等）
    hotkeyState,
    // 是否存在任意已注册的热键（getter）
    hasRegisteredHotkeys
  } = storeToRefs(hotkeyStore);

  /**
   * 注册一个全局热键。
   * 实际由 store 调用后端完成注册，前端仅做转发与状态维护。
   * @param key 热键标识字符串（与 HotkeyKeyboard/HotkeyGamepad 的枚举值对应）
   * @returns 是否注册成功
   */
  async function registerHotkey(key: string): Promise<boolean> {
    return hotkeyStore.registerHotkeyBackend(key);
  }

  /**
   * 注销一个已注册的全局热键。
   * @param key 热键标识字符串
   * @returns 是否注销成功
   */
  async function unregisterHotkey(key: string): Promise<boolean> {
    return hotkeyStore.unregisterHotkeyBackend(key);
  }

  /**
   * 启用或禁用热键功能。
   * 禁用后已注册的热键不会触发回调，但注册状态保留。
   * @param enabled 是否启用
   */
  function setHotkeyEnabled(enabled: boolean) {
    hotkeyStore.setHotkeyEnabled(enabled);
  }

  /**
   * 手动设置"最近一次按下的热键"。
   * 通常用于调试或由外部事件源同步状态。
   * @param key 热键标识字符串
   */
  function setLastHotkeyPressed(key: string) {
    hotkeyStore.setLastHotkeyPressed(key);
  }

  /**
   * 清除"最近一次按下的热键"记录。
   * 用于在切换游戏或重置场景下避免残留状态影响 UI。
   */
  function clearLastHotkeyPressed() {
    hotkeyStore.clearLastHotkeyPressed();
  }

  /**
   * 设置当前键盘热键配置。
   * @param hotkey 键盘热键枚举值
   */
  function setKeyboardHotkey(hotkey: HotkeyKeyboard) {
    hotkeyStore.setKeyboardHotkey(hotkey);
  }

  /**
   * 设置当前手柄热键配置。
   * @param hotkey 手柄热键枚举值
   */
  function setGamepadHotkey(hotkey: HotkeyGamepad) {
    hotkeyStore.setGamepadHotkey(hotkey);
  }

  /**
   * 设置左摇杆触发状态。
   * 手柄组合键需依赖该标记来区分"摇杆推动"与"摇杆回位"事件，避免误触发。
   * @param triggered 是否已触发
   */
  function setLeftThumbTriggered(triggered: boolean) {
    hotkeyStore.setLeftThumbTriggered(triggered);
  }

  /**
   * 查询指定热键是否已注册。
   * @param key 热键标识字符串
   * @returns 是否已注册
   */
  function isHotkeyRegistered(key: string): boolean {
    return hotkeyStore.isHotkeyRegistered(key);
  }

  /**
   * 清空前端维护的已注册热键集合（仅清状态，不通知后端注销）。
   * 一般用于后端已整体重置、前端需同步清空时调用。
   */
  function clearAllHotkeys() {
    hotkeyStore.clearAllHotkeys();
  }

  /**
   * 注销全部已注册热键（会同步通知后端逐个注销）。
   * 通常在应用退出或切换用户配置时调用。
   */
  async function unregisterAllHotkeys(): Promise<void> {
    return hotkeyStore.unregisterAllHotkeys();
  }

  /**
   * 订阅全局热键按下事件。
   * 事件来源可能是 Tauri 后端推送，也可能是前端自定义事件分发。
   * @param callback 回调函数，接收按键标识与触发时间戳
   * @returns 取消订阅函数，调用后移除该监听
   */
  async function onHotkeyPressed(callback: (key: string, timestamp: number) => void): Promise<() => void> {
    return on(EventNames.HOTKEY_PRESSED, (payload) => {
      callback(payload.key, payload.timestamp);
    });
  }

  /**
   * 在前端进程内分发一次热键按下事件。
   * 注意：该方法仅触发前端自定义监听，不会通知后端；主要用于模拟或前端联动场景。
   * @param key 热键标识字符串
   */
  function emitHotkeyPressed(key: string) {
    emit(EventNames.HOTKEY_PRESSED, { key, timestamp: Date.now() });
  }

  // 统一返回响应式状态与方法，供调用方按需解构使用
  return {
    registeredHotkeys,
    isHotkeyEnabled,
    lastHotkeyPressed,
    lastHotkeyPressedTime,
    currentKeyboardHotkey,
    currentGamepadHotkey,
    leftThumbWasTriggered,
    hotkeyState,
    hasRegisteredHotkeys,
    registerHotkey,
    unregisterHotkey,
    setHotkeyEnabled,
    setLastHotkeyPressed,
    clearLastHotkeyPressed,
    setKeyboardHotkey,
    setGamepadHotkey,
    setLeftThumbTriggered,
    isHotkeyRegistered,
    clearAllHotkeys,
    unregisterAllHotkeys,
    onHotkeyPressed,
    emitHotkeyPressed
  };
}
