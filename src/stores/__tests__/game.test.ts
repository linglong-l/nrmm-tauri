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
import { useGameStore, sortModsForDisplay } from '../game';
import { useSettingsStore } from '../settings';
import { setModsCache } from '../../utils/cache';
import type { ModGroupData, ModData, TargetGame } from '../../types';

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
    isDisabled: false,
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

  it('favorited groups appear first regardless of character status (NRMM compatible)', () => {
    gameStore.setModGroups([
      makeGroup({ groupPath: '/other', groupName: 'Other', realIndex: 0, favoriteDateTime: null }),
      makeGroup({ groupPath: '/path/group_char1', groupName: 'Char1', realIndex: 1, favoriteDateTime: null }),
      makeGroup({ groupPath: '/fav_other', groupName: 'FavOther', realIndex: 2, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
      makeGroup({ groupPath: '/path/group_char2', groupName: 'FavChar', realIndex: 3, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
    ]);

    const sorted = gameStore.sortedGroups;
    // NRMM规则：收藏优先（不区分角色/非角色），收藏内时间相同则保持原顺序；未收藏按realIndex升序
    expect(sorted[0].groupName).toBe('FavOther');
    expect(sorted[1].groupName).toBe('FavChar');
    expect(sorted[2].groupName).toBe('Other');
    expect(sorted[3].groupName).toBe('Char1');
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

describe('NRMM compatibility', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('produces same sort order as NRMM reference implementation', () => {
    gameStore.setModGroups([
      makeGroup({ groupPath: '/g3', groupName: 'G3', realIndex: 3, favoriteDateTime: null }),
      makeGroup({ groupPath: '/g1', groupName: 'G1Fav', realIndex: 1, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
      makeGroup({ groupPath: '/g2', groupName: 'G2', realIndex: 1, favoriteDateTime: null }),
      makeGroup({ groupPath: '/g4', groupName: 'G4Fav', realIndex: 4, favoriteDateTime: '2026-06-01T00:00:00.000Z' }),
    ]);

    const sorted = gameStore.sortedGroups;
    expect(sorted.map(g => g.groupName)).toEqual(['G4Fav', 'G1Fav', 'G2', 'G3']);
  });

  it('ignores disabled mods in selection state', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/group_1',
        previousSelectedModOnGroup: 1,
        modsInGroup: [
          { modPath: 'None', iconPath: null, modName: 'None', realIndex: 0, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
          { modPath: '/enabled', iconPath: null, modName: 'Enabled', realIndex: 1, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
          { modPath: '/disabled', iconPath: null, modName: 'Disabled', realIndex: 2, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: true, favoriteDateTime: null },
        ],
      }),
    ]);

    expect(gameStore.getSelectedModPath('/group_1')).toBe('/enabled');
    
    gameStore.setSelectedModPath('/group_1', '/disabled');
    
    expect(gameStore.getSelectedModPath('/group_1')).toBe('/enabled');
  });
});

describe('edge cases', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('handles all disabled mods in group', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/group_1',
        previousSelectedModOnGroup: 0,
        modsInGroup: [
          { modPath: 'None', iconPath: null, modName: 'None', realIndex: 0, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
          { modPath: '/mod1', iconPath: null, modName: 'Mod1', realIndex: 1, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: true, favoriteDateTime: null },
          { modPath: '/mod2', iconPath: null, modName: 'Mod2', realIndex: 2, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: true, favoriteDateTime: null },
        ],
      }),
    ]);

    gameStore.setSelectedModPath('/group_1', '/mod1');
    expect(gameStore.getSelectedModPath('/group_1')).toBe('None');

    gameStore.setSelectedModPath('/group_1', '/mod2');
    expect(gameStore.getSelectedModPath('/group_1')).toBe('None');
  });

  it('handles non-existent mod path', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/group_1',
        previousSelectedModOnGroup: 0,
        modsInGroup: [
          { modPath: 'None', iconPath: null, modName: 'None', realIndex: 0, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
        ],
      }),
    ]);

    gameStore.setSelectedModPath('/group_1', '/nonexistent');
    expect(gameStore.getSelectedModPath('/group_1')).toBe('None');
  });

  it('handles non-existent group path', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/group_1',
        previousSelectedModOnGroup: 0,
        modsInGroup: [
          { modPath: 'None', iconPath: null, modName: 'None', realIndex: 0, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
        ],
      }),
    ]);

    gameStore.setSelectedModPath('/nonexistent', '/mod1');
    expect(gameStore.getSelectedModPath('/nonexistent')).toBe(null);
  });

  it('handles all favorited groups sorted by date', () => {
    gameStore.setModGroups([
      makeGroup({ groupPath: '/g1', groupName: 'G1', realIndex: 1, favoriteDateTime: '2026-03-01T00:00:00.000Z' }),
      makeGroup({ groupPath: '/g2', groupName: 'G2', realIndex: 2, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
      makeGroup({ groupPath: '/g3', groupName: 'G3', realIndex: 3, favoriteDateTime: '2026-06-01T00:00:00.000Z' }),
    ]);

    const sorted = gameStore.sortedGroups;
    expect(sorted.map(g => g.groupName)).toEqual(['G3', 'G1', 'G2']);
  });

  it('handles all unfavorited groups sorted by realIndex', () => {
    gameStore.setModGroups([
      makeGroup({ groupPath: '/g3', groupName: 'G3', realIndex: 3, favoriteDateTime: null }),
      makeGroup({ groupPath: '/g1', groupName: 'G1', realIndex: 1, favoriteDateTime: null }),
      makeGroup({ groupPath: '/g2', groupName: 'G2', realIndex: 2, favoriteDateTime: null }),
    ]);

    const sorted = gameStore.sortedGroups;
    expect(sorted.map(g => g.groupName)).toEqual(['G1', 'G2', 'G3']);
  });

  it('handles empty favoriteDateTime in favorited groups', () => {
    gameStore.setModGroups([
      makeGroup({ groupPath: '/g1', groupName: 'G1', realIndex: 1, favoriteDateTime: '' }),
      makeGroup({ groupPath: '/g2', groupName: 'G2', realIndex: 2, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
      makeGroup({ groupPath: '/g3', groupName: 'G3', realIndex: 3, favoriteDateTime: null }),
    ]);

    const sorted = gameStore.sortedGroups;
    expect(sorted[0].groupName).toBe('G2');
    expect(sorted[1].groupName).toBe('G1');
    expect(sorted[2].groupName).toBe('G3');
  });
});

describe('character selection filtering', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('does not select disabled mod when clicking', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/group_1',
        groupName: 'TestGroup',
        realIndex: 1,
        previousSelectedModOnGroup: 0,
        modsInGroup: [
          { modPath: 'None', iconPath: null, modName: 'None', realIndex: 0, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
          { modPath: '/mod1', iconPath: null, modName: 'EnabledMod', realIndex: 1, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
          { modPath: '/mod2', iconPath: null, modName: 'DisabledMod', realIndex: 2, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: true, favoriteDateTime: null },
        ],
      }),
    ]);

    const group = gameStore.findGroupByPath('/group_1');
    if (group) {
      const disabledMod = group.modsInGroup[2];
      gameStore.setSelectedModPath('/group_1', disabledMod.modPath);
    }

    expect(gameStore.getSelectedModPath('/group_1')).toBe('None');
  });

  it('selects enabled mod correctly', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/group_1',
        groupName: 'TestGroup',
        realIndex: 1,
        previousSelectedModOnGroup: 0,
        modsInGroup: [
          { modPath: 'None', iconPath: null, modName: 'None', realIndex: 0, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
          { modPath: '/mod1', iconPath: null, modName: 'EnabledMod', realIndex: 1, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
        ],
      }),
    ]);

    gameStore.setSelectedModPath('/group_1', '/mod1');
    expect(gameStore.getSelectedModPath('/group_1')).toBe('/mod1');
  });
});

/**
 * 构造最小化的 ModData 用于测试。
 * @param overrides 覆盖默认字段的对象
 * @returns 测试用 ModData
 */
function makeMod(overrides: Partial<ModData> = {}): ModData {
  return {
    modPath: '/mod',
    iconPath: null,
    modName: 'Mod',
    realIndex: 1,
    isOldAutoFixed: false,
    isSyntaxErrorRemoved: false,
    isUnoptimized: false,
    isNamespaced: false,
    isDisabled: false,
    favoriteDateTime: null,
    ...overrides,
  };
}

describe('sortModsForDisplay', () => {
  it('puts None slot (realIndex 0) first', () => {
    const mods = [
      makeMod({ modPath: '/mod1', modName: 'Mod1', realIndex: 1 }),
      makeMod({ modPath: 'None', modName: 'None', realIndex: 0 }),
      makeMod({ modPath: '/mod2', modName: 'Mod2', realIndex: 2 }),
    ];
    const result = sortModsForDisplay(mods, null);
    expect(result[0].modPath).toBe('None');
  });

  it('puts selected mod second (after None)', () => {
    const mods = [
      makeMod({ modPath: '/mod1', modName: 'Mod1', realIndex: 1 }),
      makeMod({ modPath: 'None', modName: 'None', realIndex: 0 }),
      makeMod({ modPath: '/mod2', modName: 'Mod2', realIndex: 2 }),
    ];
    const result = sortModsForDisplay(mods, '/mod2');
    expect(result[0].modPath).toBe('None');
    expect(result[1].modPath).toBe('/mod2');
  });

  it('maintains backend array order for non-None non-selected mods', () => {
    const mods = [
      makeMod({ modPath: '/mod1', modName: 'Mod1', realIndex: 1 }),
      makeMod({ modPath: 'None', modName: 'None', realIndex: 0 }),
      makeMod({ modPath: '/mod2', modName: 'Mod2', realIndex: 2 }),
      makeMod({ modPath: '/mod3', modName: 'Mod3', realIndex: 3 }),
    ];
    const result = sortModsForDisplay(mods, null);
    expect(result[0].modPath).toBe('None');
    expect(result[1].modPath).toBe('/mod1');
    expect(result[2].modPath).toBe('/mod2');
    expect(result[3].modPath).toBe('/mod3');
  });

  it('sorts by array position not realIndex (post-fix scenario: realIndex != array position)', () => {
    // 修复后场景：real_index 来自目录列表原始顺序，而非数组位置
    // 数组顺序（后端已排序）: [None, modA, modB] —— 这是我们想要保持的显示顺序
    // 但 modA 的 realIndex=2（原始目录位置2），modB 的 realIndex=1（原始目录位置1）
    // 排序应按数组位置保持 [None, modA, modB]，而非按 realIndex 变成 [None, modB, modA]
    const mods = [
      makeMod({ modPath: 'None', modName: 'None', realIndex: 0 }),
      makeMod({ modPath: '/modA', modName: 'ModA', realIndex: 2 }),
      makeMod({ modPath: '/modB', modName: 'ModB', realIndex: 1 }),
    ];
    const result = sortModsForDisplay(mods, null);
    expect(result[0].modPath).toBe('None');
    expect(result[1].modPath).toBe('/modA');
    expect(result[2].modPath).toBe('/modB');
  });

  it('handles empty mods array', () => {
    const result = sortModsForDisplay([], null);
    expect(result).toEqual([]);
  });

  it('handles single mod', () => {
    const mods = [makeMod({ modPath: '/only', modName: 'Only', realIndex: 1 })];
    const result = sortModsForDisplay(mods, null);
    expect(result).toHaveLength(1);
    expect(result[0].modPath).toBe('/only');
  });
});

describe('removeModFromGroup (realIndex not decremented)', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('does not decrement realIndex of remaining mods after removal', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/group_1',
        modsInGroup: [
          { modPath: 'None', iconPath: null, modName: 'None', realIndex: 0, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
          { modPath: '/mod1', iconPath: null, modName: 'Mod1', realIndex: 1, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
          { modPath: '/mod2', iconPath: null, modName: 'Mod2', realIndex: 2, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
          { modPath: '/mod3', iconPath: null, modName: 'Mod3', realIndex: 3, isOldAutoFixed: false, isSyntaxErrorRemoved: false, isUnoptimized: false, isNamespaced: false, isDisabled: false, favoriteDateTime: null },
        ],
      }),
    ]);

    // 移除索引 1 的模组（mod1）
    gameStore.removeModFromGroup('/group_1', 1);

    const group = gameStore.findGroupByPath('/group_1');
    expect(group).not.toBeNull();
    expect(group!.modsInGroup).toHaveLength(3);
    // mod1 已移除，剩余模组应保持原始 realIndex 值，不被递减
    expect(group!.modsInGroup[0].modPath).toBe('None');
    expect(group!.modsInGroup[0].realIndex).toBe(0);
    expect(group!.modsInGroup[1].modPath).toBe('/mod2');
    expect(group!.modsInGroup[1].realIndex).toBe(2); // 不被递减为 1
    expect(group!.modsInGroup[2].modPath).toBe('/mod3');
    expect(group!.modsInGroup[2].realIndex).toBe(3); // 不被递减为 2
  });
});
