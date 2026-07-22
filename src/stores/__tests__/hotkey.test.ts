/**
 * useHotkeyStore 单元测试
 *
 * 覆盖范围：
 * - 初始默认热键值
 * - setKeyboardHotkey / setGamepadHotkey 更新热键
 * - registerHotkey / unregisterHotkey 前端注册
 * - setHotkeyEnabled 开关
 * - setSearchHotkeysEnabled 搜索热键开关
 * - 热键状态检查
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { setActivePinia, createPinia } from 'pinia';
import { useHotkeyStore } from '../hotkey';
import { HotkeyKeyboard, HotkeyGamepad } from '../../types';

// Mock @tauri-apps/api/core
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

describe('useHotkeyStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  it('初始状态有默认键盘热键', () => {
    const store = useHotkeyStore();
    expect(store.currentKeyboardHotkey).toBe('altW');
  });

  it('初始状态手柄热键为 none', () => {
    const store = useHotkeyStore();
    expect(store.currentGamepadHotkey).toBe('none');
  });

  it('初始状态热键已启用', () => {
    const store = useHotkeyStore();
    expect(store.isHotkeyEnabled).toBe(true);
  });

  it('初始状态无已注册热键', () => {
    const store = useHotkeyStore();
    expect(store.registeredHotkeys).toEqual([]);
    expect(store.hasRegisteredHotkeys).toBe(false);
  });

  it('setKeyboardHotkey 更新键盘热键', () => {
    const store = useHotkeyStore();
    store.setKeyboardHotkey(HotkeyKeyboard.altA);
    expect(store.currentKeyboardHotkey).toBe(HotkeyKeyboard.altA);
  });

  it('setGamepadHotkey 更新手柄热键', () => {
    const store = useHotkeyStore();
    store.setGamepadHotkey(HotkeyGamepad.lsB);
    expect(store.currentGamepadHotkey).toBe(HotkeyGamepad.lsB);
  });

  it('setHotkeyEnabled 禁用热键', () => {
    const store = useHotkeyStore();
    store.setHotkeyEnabled(false);
    expect(store.isHotkeyEnabled).toBe(false);
  });

  it('setSearchHotkeysEnabled 启用搜索热键', () => {
    const store = useHotkeyStore();
    store.setSearchHotkeysEnabled(true);
    expect(store.isSearchHotkeysEnabled).toBe(true);
  });

  it('registerHotkey 注册前端热键', () => {
    const store = useHotkeyStore();
    store.registerHotkey('altD');
    expect(store.isHotkeyRegistered('altD')).toBe(true);
    expect(store.hasRegisteredHotkeys).toBe(true);
  });

  it('registerHotkey 重复注册不重复添加', () => {
    const store = useHotkeyStore();
    store.registerHotkey('altD');
    store.registerHotkey('altD');
    expect(store.registeredHotkeys.length).toBe(1);
  });

  it('unregisterHotkey 注销前端热键', () => {
    const store = useHotkeyStore();
    store.registerHotkey('altD');
    store.unregisterHotkey('altD');
    expect(store.isHotkeyRegistered('altD')).toBe(false);
  });

  it('clearAllHotkeys 清空所有热键', () => {
    const store = useHotkeyStore();
    store.registerHotkey('altD');
    store.registerHotkey('altF');
    store.clearAllHotkeys();
    expect(store.registeredHotkeys).toEqual([]);
  });

  it('setLastHotkeyPressed 记录最近按下热键', () => {
    const store = useHotkeyStore();
    store.setLastHotkeyPressed('altD');
    expect(store.lastHotkeyPressed).toBe('altD');
    expect(store.lastHotkeyPressedTime).toBeDefined();
    expect(store.lastHotkeyPressedTime).toBeGreaterThan(0);
  });

  it('clearLastHotkeyPressed 清除记录', () => {
    const store = useHotkeyStore();
    store.setLastHotkeyPressed('altD');
    store.clearLastHotkeyPressed();
    expect(store.lastHotkeyPressed).toBeNull();
    expect(store.lastHotkeyPressedTime).toBeNull();
  });

  it('hotkeyState computed 聚合状态正确', () => {
    const store = useHotkeyStore();
    store.registerHotkey('altD');
    const state = store.hotkeyState;
    expect(state.isRegistered).toBe(true);
    expect(state.isEnabled).toBe(true);
    expect(state.isSearchHotkeysEnabled).toBe(false);
  });
});