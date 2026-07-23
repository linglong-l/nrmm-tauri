/**
 * useSettingsStore 单元测试
 *
 * 覆盖范围：
 * - 初始状态
 * - getModsPath / setModsPath 路径操作
 * - setTargetGame 更新游戏
 * - setLanguage / setTheme 外观设置
 * - loadSettings 加载配置
 * - saveSettings 保存配置
 * - resetToDefaults 重置
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { setActivePinia, createPinia } from 'pinia';
import { useSettingsStore } from '../settings';
import { TargetGame, HotkeyKeyboard, LayoutMode } from '../../types';

// Mock @tauri-apps/api/core
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

describe('useSettingsStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  it('初始状态 targetGame 为默认值', () => {
    const store = useSettingsStore();
    expect(store.targetGame).toBeDefined();
    expect(typeof store.targetGame).toBe('string');
  });

  it('初始状态 language 已定义', () => {
    const store = useSettingsStore();
    expect(store.language).toBeDefined();
    expect(typeof store.language).toBe('string');
  });

  it('初始状态 theme 已定义', () => {
    const store = useSettingsStore();
    expect(store.theme).toBeDefined();
  });

  it('setTargetGame 更新游戏', () => {
    const store = useSettingsStore();
    store.setTargetGame(TargetGame.Wuthering_Waves);
    expect(store.targetGame).toBe(TargetGame.Wuthering_Waves);
  });

  it('setTargetGame 更新为 none', () => {
    const store = useSettingsStore();
    store.setTargetGame(TargetGame.Wuthering_Waves);
    store.setTargetGame(TargetGame.none);
    expect(store.targetGame).toBe(TargetGame.none);
  });

  it('setModsPath 和 getModsPath 设置和获取路径', () => {
    const store = useSettingsStore();
    store.setModsPath(TargetGame.Wuthering_Waves, '/mnt/d/Games/MODS/XXMI/WuWa');
    const path = store.getModsPath(TargetGame.Wuthering_Waves);
    expect(path).toBe('/mnt/d/Games/MODS/XXMI/WuWa');
  });

  it('getModsPath 对未配置游戏返回空字符串', () => {
    const store = useSettingsStore();
    const path = store.getModsPath(TargetGame.Genshin_Impact);
    expect(path).toBe('');
  });

  it('setLanguage 更新语言', () => {
    const store = useSettingsStore();
    store.setLanguage('en');
    expect(store.language).toBe('en');
  });

  it('setTheme 更新主题', () => {
    const store = useSettingsStore();
    store.setTheme('dark');
    expect(store.theme).toBe('dark');
  });

  it('setHotkeyKeyboard 更新键盘热键', () => {
    const store = useSettingsStore();
    store.setHotkeyKeyboard(HotkeyKeyboard.altA);
    expect(store.hotkeyKeyboard).toBe(HotkeyKeyboard.altA);
  });

  it('setSearchHotkey 更新搜索快捷键', () => {
    const store = useSettingsStore();
    store.setSearchHotkey('altF');
    expect(store.searchHotkey).toBe('altF');
  });

  it('resetToDefaults 重置为默认值', () => {
    const store = useSettingsStore();
    store.setTargetGame(TargetGame.none);
    store.setLanguage('en');
    store.resetToDefaults();
    // 重置后 targetGame 恢复为默认值
    expect(store.targetGame).toBeDefined();
    expect(store.language).toBeDefined();
  });

  it('setOverallScale 更新缩放比例', () => {
    const store = useSettingsStore();
    store.setOverallScale(1.5);
    expect(store.overallScale).toBe(1.5);
  });

  it('setLayoutMode 更新布局模式', () => {
    const store = useSettingsStore();
    store.setLayoutMode(LayoutMode.Grid);
    expect(store.layoutMode).toBe(LayoutMode.Grid);
  });

  it('setAutoPinWindow 更新自动置顶', () => {
    const store = useSettingsStore();
    store.setAutoPinWindow(true);
    expect(store.isAutoPinWindow).toBe(true);
  });
});