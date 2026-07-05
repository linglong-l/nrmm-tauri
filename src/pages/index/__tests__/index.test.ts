/**
 * index.vue 集成测试
 *
 * 覆盖范围：
 * - 切换到 mods 标签页时触发缓存校验
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { nextTick } from 'vue';
import IndexPage from '../index.vue';
import { useUiStore } from '../../../stores/ui';
import { useGameStore } from '../../../stores/game';
import { useSettingsStore } from '../../../stores/settings';

/**
 * Mock 事件管理器，避免测试中实际注册 Tauri 跨进程监听器。
 * eventManager.on 返回一个空函数作为 unlisten 句柄。
 */
vi.mock('../../../utils/events', () => ({
  EventNames: {
    MOD_GROUPS_UPDATED: 'mod-groups-updated',
    GAME_SWITCHED: 'game-switched',
    FILE_WATCHER_EVENT: 'file-watcher-event',
    MODS_UPDATED: 'mods-updated',
  },
  eventManager: {
    on: vi.fn().mockResolvedValue(() => {}),
    emit: vi.fn().mockResolvedValue(undefined),
    emitLocal: vi.fn(),
  },
}));

/**
 * Mock vue-i18n，保留真实的 createI18n 导出，仅覆盖 useI18n。
 * t 函数直接返回原始 key 作为翻译结果。
 */
vi.mock('vue-i18n', async (importOriginal) => {
  const actual = await importOriginal<typeof import('vue-i18n')>();
  return {
    ...actual,
    useI18n: () => ({
      t: (key: string) => key,
      locale: { value: 'en' },
    }),
  };
});

/**
 * Mock 子组件，避免其副作用（如 Tauri invoke、文件监听等）。
 */
vi.mock('../tabs/ModsTab.vue', () => ({
  default: { template: '<div class="mock-mods-tab"></div>' },
}));
vi.mock('../../../views/SettingsView.vue', () => ({
  default: { template: '<div class="mock-settings-view"></div>' },
}));
vi.mock('../../components', () => ({
  SideNav: { template: '<div class="mock-side-nav"></div>' },
}));

describe('IndexPage cache validation on tab switch', () => {
  let gameStore: ReturnType<typeof useGameStore>;
  let uiStore: ReturnType<typeof useUiStore>;
  let settingsStore: ReturnType<typeof useSettingsStore>;

  beforeEach(() => {
    uiStore = useUiStore();
    gameStore = useGameStore();
    settingsStore = useSettingsStore();

    // Mock settingsStore 方法避免依赖真实配置
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    vi.spyOn(settingsStore, 'saveSettings').mockResolvedValue(true);
  });

  it('triggers cache validation when switching to mods tab', async () => {
    // 设置初始标签页为 settings
    uiStore.setActiveTab('settings');

    const validateSpy = vi.spyOn(gameStore, 'validateCache');
    validateSpy.mockReturnValue({ action: 'use_cache' });

    mount(IndexPage);

    await nextTick();

    // 切回 mods 标签页
    uiStore.setActiveTab('mods');
    await nextTick();

    // 验证 validateCache 被调用
    expect(validateSpy).toHaveBeenCalled();
  });

  it('does not trigger cache validation when switching to non-mods tab', async () => {
    // 设置初始标签页为 mods
    uiStore.setActiveTab('mods');

    const validateSpy = vi.spyOn(gameStore, 'validateCache');
    validateSpy.mockReturnValue({ action: 'use_cache' });

    mount(IndexPage);

    await nextTick();

    // 切换到 settings 标签页
    uiStore.setActiveTab('settings');
    await nextTick();

    // 验证 validateCache 未被调用（切换到非 mods 标签页不触发校验）
    expect(validateSpy).not.toHaveBeenCalled();
  });
});
