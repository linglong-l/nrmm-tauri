/**
 * gameStore 单元测试
 *
 * 覆盖范围：
 * - validateCache：缓存数据校验机制
 * - findGroupByPath：分组查找（递归→迭代重构后的回归测试）
 * - findAncestorPaths：祖先路径查找
 * - setModGroups 中的 initSelectedMap：选中模组路径映射初始化
 * - sortedGroups：分组排序（含置顶功能）
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { useGameStore } from '../game';
import { useSettingsStore } from '../settings';
import { setModsCache } from '../../utils/cache';
import type { ModGroupData, TargetGame } from '../../types';

/**
 * 构造最小化的分组数据用于测试。
 * @param overrides 覆盖默认字段的对象
 * @returns 测试用分组数据
 */
function makeGroup(overrides: Partial<ModGroupData> = {}): ModGroupData {
  return {
    groupPath: '/test/group',
    iconPath: null,
    groupName: 'Test Group',
    favoriteDateTime: null,
    modsInGroup: [],
    realIndex: 1,
    previousSelectedModOnGroup: -1,
    children: [],
    isTreeNode: false,
    isVirtual: false,
    ...overrides,
  };
}

describe('validateCache', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    // 初始化 settingsStore 并 mock getModsPath 避免依赖真实配置
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('returns skip when game is none', () => {
    const result = gameStore.validateCache('none' as TargetGame);
    expect(result.action).toBe('skip');
  });

  it('returns load when cache empty and game is valid', () => {
    // 确保缓存为空：targetGame 为 none，modGroups 为空
    const result = gameStore.validateCache('genshin' as TargetGame);
    expect(result.action).toBe('load');
  });

  it('returns clear_and_load when cached game differs from requested', () => {
    // 模拟已加载 wuwa 数据（内存缓存命中）
    gameStore.setTargetGame('wuwa' as TargetGame);
    gameStore.setModsLoaded(true);
    gameStore.setModGroups([makeGroup({ groupPath: '/wuwa/group1' })]);

    // 请求 genshin（与缓存的 wuwa 不同）
    const result = gameStore.validateCache('genshin' as TargetGame);
    expect(result.action).toBe('clear_and_load');
  });

  it('returns clear_and_load when targetGame updated but cachedGame differs', () => {
    // 模拟已加载 wuwa 数据
    gameStore.setTargetGame('wuwa' as TargetGame);
    gameStore.setModsLoaded(true);
    gameStore.setModGroups([makeGroup({ groupPath: '/wuwa/group1' })]);

    // 模拟用户切换到 genshin：setTargetGame 立即更新 targetGame，
    // 但 cachedGame 仍是 wuwa（因为 modGroups 还是 wuwa 的数据）
    gameStore.setTargetGame('genshin' as TargetGame);

    // 此时 validateCache 应检测到 cachedGame 与请求游戏不一致
    const result = gameStore.validateCache('genshin' as TargetGame);
    expect(result.action).toBe('clear_and_load');
  });

  it('returns use_cache when cache valid and game matches', () => {
    // 模拟已加载 wuwa 数据
    gameStore.setTargetGame('wuwa' as TargetGame);
    gameStore.setModsLoaded(true);
    gameStore.setModGroups([makeGroup({ groupPath: '/wuwa/group1' })]);

    // 请求相同的 wuwa
    const result = gameStore.validateCache('wuwa' as TargetGame);
    expect(result.action).toBe('use_cache');
  });
});

describe('loadModsForGame cache clearing on game switch', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    vi.spyOn(settingsStore, 'saveSettings').mockResolvedValue(true);
    gameStore = useGameStore();
  });

  it('clears stale cache when switching games', async () => {
    // 模拟已加载 wuwa 数据：设置内存缓存 + localStorage 缓存
    gameStore.setTargetGame('wuwa' as TargetGame);
    gameStore.setModsLoaded(true);
    const wuwaGroups = [makeGroup({ groupPath: '/wuwa/group1', realIndex: 1 })];
    gameStore.setModGroups(wuwaGroups);
    setModsCache('wuwa', wuwaGroups);

    // 验证 wuwa 缓存确实存在
    expect(localStorage.getItem('mods_wuwa')).not.toBeNull();

    // Mock invoke 返回 genshin 的空分组列表
    vi.mocked(invoke).mockResolvedValueOnce([]);

    // 切换到 genshin
    await gameStore.loadModsForGame('genshin' as TargetGame);

    // wuwa 的缓存应被清除（因游戏名称不同）
    expect(localStorage.getItem('mods_wuwa')).toBeNull();
  });
});

describe('findGroupByPath (iterative refactor)', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('finds top-level group', () => {
    gameStore.setModGroups([
      makeGroup({ groupPath: '/a' }),
      makeGroup({ groupPath: '/b' }),
    ]);
    expect(gameStore.findGroupByPath('/a')?.groupPath).toBe('/a');
    expect(gameStore.findGroupByPath('/b')?.groupPath).toBe('/b');
  });

  it('finds deeply nested group', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/a',
        children: [
          makeGroup({
            groupPath: '/a/b',
            children: [
              makeGroup({ groupPath: '/a/b/c' }),
            ],
          }),
        ],
      }),
    ]);
    expect(gameStore.findGroupByPath('/a/b/c')?.groupPath).toBe('/a/b/c');
  });

  it('returns null for non-existent path', () => {
    gameStore.setModGroups([makeGroup({ groupPath: '/a' })]);
    expect(gameStore.findGroupByPath('/x')).toBeNull();
  });
});

describe('findAncestorPaths (iterative refactor)', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('returns ancestor paths for nested group', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/a',
        children: [
          makeGroup({
            groupPath: '/a/b',
            children: [
              makeGroup({ groupPath: '/a/b/c' }),
            ],
          }),
        ],
      }),
    ]);
    const ancestors = gameStore.findAncestorPaths('/a/b/c');
    expect(ancestors).toEqual(['/a', '/a/b']);
  });

  it('returns empty for top-level group', () => {
    gameStore.setModGroups([makeGroup({ groupPath: '/a' })]);
    expect(gameStore.findAncestorPaths('/a')).toEqual([]);
  });

  it('returns empty for non-existent path', () => {
    gameStore.setModGroups([makeGroup({ groupPath: '/a' })]);
    expect(gameStore.findAncestorPaths('/x')).toEqual([]);
  });
});

describe('initSelectedMap (iterative refactor)', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('initializes selected mod paths for nested groups', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/a',
        previousSelectedModOnGroup: 0,
        modsInGroup: [
          { modPath: '/a/mod0', iconPath: null, modName: 'None', realIndex: 0, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
          { modPath: '/a/mod1', iconPath: null, modName: 'Mod1', realIndex: 1, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
        ],
        children: [
          makeGroup({
            groupPath: '/a/b',
            previousSelectedModOnGroup: 1,
            modsInGroup: [
              { modPath: '/a/b/mod0', iconPath: null, modName: 'None', realIndex: 0, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
              { modPath: '/a/b/mod1', iconPath: null, modName: 'Mod1', realIndex: 1, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
            ],
          }),
        ],
      }),
    ]);
    expect(gameStore.getSelectedModPath('/a')).toBe('/a/mod0');
    expect(gameStore.getSelectedModPath('/a/b')).toBe('/a/b/mod1');
  });

  it('returns null for groups without valid selection index', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/x',
        previousSelectedModOnGroup: -1,
        modsInGroup: [],
      }),
    ]);
    expect(gameStore.getSelectedModPath('/x')).toBeNull();
  });
});

describe('sortedGroups (pinning)', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('favorited character groups appear first', () => {
    // 构造 4 种优先级的分组：收藏角色、收藏非角色、未收藏角色、未收藏非角色
    gameStore.setModGroups([
      // 未收藏非角色分组（realIndex 最小，但应排在最后）
      makeGroup({ groupPath: '/other', groupName: 'Other', realIndex: 0, favoriteDateTime: null }),
      // 未收藏角色分组
      makeGroup({ groupPath: '/path/group_char1', groupName: 'Char1', realIndex: 1, favoriteDateTime: null }),
      // 收藏非角色分组
      makeGroup({ groupPath: '/fav_other', groupName: 'FavOther', realIndex: 2, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
      // 收藏角色分组（应排在最前）
      makeGroup({ groupPath: '/path/group_char2', groupName: 'FavChar', realIndex: 3, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
    ]);

    const sorted = gameStore.sortedGroups;
    // 优先级顺序：收藏角色 > 收藏非角色 > 未收藏角色 > 未收藏非角色
    expect(sorted[0].groupName).toBe('FavChar');
    expect(sorted[1].groupName).toBe('FavOther');
    expect(sorted[2].groupName).toBe('Char1');
    expect(sorted[3].groupName).toBe('Other');
  });

  it('favorited non-character groups rank before unfavorited character groups', () => {
    gameStore.setModGroups([
      // 未收藏角色分组
      makeGroup({ groupPath: '/path/group_a', groupName: 'UnfavChar', realIndex: 0, favoriteDateTime: null }),
      // 收藏非角色分组（应排在未收藏角色之前）
      makeGroup({ groupPath: '/fav_misc', groupName: 'FavMisc', realIndex: 1, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
    ]);

    const sorted = gameStore.sortedGroups;
    expect(sorted[0].groupName).toBe('FavMisc');
    expect(sorted[1].groupName).toBe('UnfavChar');
  });

  it('multiple favorited groups sorted by favoriteDateTime descending', () => {
    gameStore.setModGroups([
      // 较早收藏的角色分组
      makeGroup({ groupPath: '/path/group_old', groupName: 'OldFav', realIndex: 0, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
      // 较新收藏的角色分组（应排在前面）
      makeGroup({ groupPath: '/path/group_new', groupName: 'NewFav', realIndex: 1, favoriteDateTime: '2026-06-01T00:00:00.000Z' }),
    ]);

    const sorted = gameStore.sortedGroups;
    expect(sorted[0].groupName).toBe('NewFav');
    expect(sorted[1].groupName).toBe('OldFav');
  });

  it('unfavorited groups sorted by realIndex ascending', () => {
    gameStore.setModGroups([
      // 未收藏非角色分组，realIndex 较大
      makeGroup({ groupPath: '/misc_b', groupName: 'MiscB', realIndex: 5, favoriteDateTime: null }),
      // 未收藏非角色分组，realIndex 较小（应排在前面）
      makeGroup({ groupPath: '/misc_a', groupName: 'MiscA', realIndex: 2, favoriteDateTime: null }),
    ]);

    const sorted = gameStore.sortedGroups;
    expect(sorted[0].groupName).toBe('MiscA');
    expect(sorted[1].groupName).toBe('MiscB');
  });

  it('returns empty array for empty group list', () => {
    gameStore.setModGroups([]);
    expect(gameStore.sortedGroups).toEqual([]);
  });
});
