import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import type { HotkeyKeyboard, HotkeyGamepad, HotkeyState } from '../types';
import { invokeRegisterHotkey, invokeUnregisterHotkey } from '../utils/invoke';
import { EventNames, eventManager } from '../utils/events';

/**
 * 热键 Store
 *
 * 管理全局热键的注册 / 注销、当前启用状态、最近一次按下的热键，
 * 以及键盘热键与手柄热键的当前选择。同时处理手柄左摇杆触发的去抖标志。
 *
 * 注册/注销分为两层：
 * - registerHotkey / unregisterHotkey：仅维护前端内存中的已注册列表；
 * - registerHotkeyBackend / unregisterHotkeyBackend：同步调用 Tauri 后端完成系统级注册/注销。
 */
export const useHotkeyStore = defineStore('hotkey', () => {
  // 已注册的热键标识列表（前端内存态）。仅反映前端认知，未必与系统级注册完全同步。
  const registeredHotkeys = ref<string[]>([]);
  // 热键总开关。为 false 时即便已注册也不会响应（业务层判断使用）。
  const isHotkeyEnabled = ref(true);
  // 最近一次被按下的热键标识；尚未按下时为 null。
  const lastHotkeyPressed = ref<string | null>(null);
  // 最近一次按下热键的时间戳（Date.now()）；用于防抖或展示。
  const lastHotkeyPressedTime = ref<number | null>(null);
  // 当前选中的键盘热键配置，默认 Alt+W。
  const currentKeyboardHotkey = ref<HotkeyKeyboard>('altW' as HotkeyKeyboard);
  // 当前选中的手柄热键配置，默认 'none'（不使用手柄热键）。
  const currentGamepadHotkey = ref<HotkeyGamepad>('none' as HotkeyGamepad);
  // 手柄左摇杆是否已触发过的标志，用于避免长按时重复触发同一动作。
  const leftThumbWasTriggered = ref(false);

  /**
   * 热键整体状态的聚合视图。
   * 便于订阅方一次性拿到 isRegistered / isEnabled / lastPressed / lastPressedTime。
   */
  const hotkeyState = computed<HotkeyState>(() => ({
    isRegistered: registeredHotkeys.value.length > 0,
    isEnabled: isHotkeyEnabled.value,
    lastPressed: lastHotkeyPressed.value,
    lastPressedTime: lastHotkeyPressedTime.value
  }));

  /** 是否已注册了任意热键。 */
  const hasRegisteredHotkeys = computed(() => registeredHotkeys.value.length > 0);

  /**
   * 仅在前端注册热键（不调用后端）。
   * 若该 key 已存在则跳过，保证列表唯一。
   * @param key 热键标识
   */
  function registerHotkey(key: string) {
    if (!registeredHotkeys.value.includes(key)) {
      registeredHotkeys.value.push(key);
    }
  }

  /**
   * 仅在前端注销热键（不调用后端）。
   * @param key 热键标识
   */
  function unregisterHotkey(key: string) {
    const index = registeredHotkeys.value.indexOf(key);
    if (index > -1) {
      registeredHotkeys.value.splice(index, 1);
    }
  }

  /**
   * 通过 Tauri 后端注册系统级热键。
   * 业务逻辑：
   * 1. 调用 invokeRegisterHotkey 完成系统注册；
   * 2. 成功后同步前端列表（registerHotkey）并广播 HOTKEY_REGISTERED 事件（success:true）；
   * 3. 失败时广播 success:false 事件并返回 false，不抛出。
   * @param key 热键标识
   * @returns 是否注册成功
   */
  async function registerHotkeyBackend(key: string): Promise<boolean> {
    try {
      await invokeRegisterHotkey(key);
      registerHotkey(key);
      eventManager.emit(EventNames.HOTKEY_REGISTERED, { key, success: true });
      return true;
    } catch {
      eventManager.emit(EventNames.HOTKEY_REGISTERED, { key, success: false });
      return false;
    }
  }

  /**
   * 通过 Tauri 后端注销系统级热键。
   * 业务逻辑：
   * 1. 调用 invokeUnregisterHotkey 完成系统注销；
   * 2. 成功后同步前端列表（unregisterHotkey）并广播 HOTKEY_UNREGISTERED 事件（success:true）；
   * 3. 失败时广播 success:false 事件并返回 false，不抛出。
   * @param key 热键标识
   * @returns 是否注销成功
   */
  async function unregisterHotkeyBackend(key: string): Promise<boolean> {
    try {
      await invokeUnregisterHotkey(key);
      unregisterHotkey(key);
      eventManager.emit(EventNames.HOTKEY_UNREGISTERED, { key, success: true });
      return true;
    } catch {
      eventManager.emit(EventNames.HOTKEY_UNREGISTERED, { key, success: false });
      return false;
    }
  }

  /** 设置热键总开关。 */
  function setHotkeyEnabled(enabled: boolean) {
    isHotkeyEnabled.value = enabled;
  }

  /**
   * 记录最近一次按下的热键。
   * 同时更新按下时间戳，并广播 HOTKEY_PRESSED 事件供其它模块响应。
   * @param key 被按下的热键标识
   */
  function setLastHotkeyPressed(key: string) {
    lastHotkeyPressed.value = key;
    lastHotkeyPressedTime.value = Date.now();
    eventManager.emit(EventNames.HOTKEY_PRESSED, { key, source: 'in-game' as const, timestamp: Date.now() });
  }

  /** 清除最近一次按下热键的记录（标识与时间戳均置空）。 */
  function clearLastHotkeyPressed() {
    lastHotkeyPressed.value = null;
    lastHotkeyPressedTime.value = null;
  }

  /** 设置当前键盘热键配置。 */
  function setKeyboardHotkey(hotkey: HotkeyKeyboard) {
    currentKeyboardHotkey.value = hotkey;
  }

  /** 设置当前手柄热键配置。 */
  function setGamepadHotkey(hotkey: HotkeyGamepad) {
    currentGamepadHotkey.value = hotkey;
  }

  /** 设置手柄左摇杆触发标志，用于去抖。 */
  function setLeftThumbTriggered(triggered: boolean) {
    leftThumbWasTriggered.value = triggered;
  }

  /**
   * 判断指定热键是否已在前端列表中注册。
   * @param key 热键标识
   * @returns 是否已注册
   */
  function isHotkeyRegistered(key: string): boolean {
    return registeredHotkeys.value.includes(key);
  }

  /** 清空前端已注册热键列表（仅内存，不调用后端注销）。 */
  function clearAllHotkeys() {
    registeredHotkeys.value = [];
  }

  /**
   * 注销所有已注册热键（含后端）。
   * 复制当前已注册列表后逐个调用 unregisterHotkeyBackend，
   * 避免在遍历过程中修改原数组。
   */
  async function unregisterAllHotkeys(): Promise<void> {
    const keys = [...registeredHotkeys.value];
    for (const key of keys) {
      await unregisterHotkeyBackend(key);
    }
  }

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
    registerHotkeyBackend,
    unregisterHotkeyBackend,
    setHotkeyEnabled,
    setLastHotkeyPressed,
    clearLastHotkeyPressed,
    setKeyboardHotkey,
    setGamepadHotkey,
    setLeftThumbTriggered,
    isHotkeyRegistered,
    clearAllHotkeys,
    unregisterAllHotkeys
  };
});
