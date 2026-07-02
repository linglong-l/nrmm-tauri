import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import type { ModData, ModGroupData, ModsPathStatus, TargetGame, ModKeybindInfo } from '../types';
import { invokeToggleModFavorite, invokeToggleGroupFavorite, invokeSearchMods } from '../utils/invoke';
import { EventNames, eventManager } from '../utils/events';
import { useSettingsStore } from './settings';

/**
 * 游戏 / Mods 数据 Store
 *
 * 管理当前选中的目标游戏、扫描得到的 Mods 与分组、Mods 路径校验状态，
 * 以及搜索、收藏、快捷键绑定等与模组展示相关的运行时状态。
 *
 * 该 Store 主要负责"内存态"数据的组织与派生，磁盘 I/O 与重载逻辑
 * 由调用方通过 invoke 与后端交互后再调用本 Store 的 setter 注入数据。
 */
export const useGameStore = defineStore('game', () => {
  // 当前选中的目标游戏。'none' 表示未选择任何游戏。
  const targetGame = ref<TargetGame>('none' as TargetGame);
  // 当前游戏下扫描到的全部 Mod 列表（扁平结构，未按分组聚合）。
  const mods = ref<ModData[]>([]);
  // 当前游戏下的分组列表，每个分组包含其下属的 modsInGroup。
  const modGroups = ref<ModGroupData[]>([]);
  // Mods 路径的校验状态，用于在 UI 上提示路径缺失 / 缺文件 / 已过时 / 有效等。
  const modsPathStatus = ref<ModsPathStatus>('invalidNotExist' as ModsPathStatus);
  // 当前游戏的 Mods 文件夹绝对路径。
  const modsPath = ref('');
  // Mods 是否已加载完成。控制界面是显示加载中还是真实内容。
  const isModsLoaded = ref(false);
  // 当前选中的分组索引（对应 modGroups 数组下标）。
  const currentGroupIndex = ref(0);
  // 当前选中的分组路径（用于树形结构定位）。
  const currentGroupPath = ref<string>('');
  // 展开的树节点路径集合（用于控制折叠/展开状态）。
  const expandedPaths = ref<Set<string>>(new Set());
  // 当前搜索关键字。
  const searchKeyword = ref('');
  // 是否处于搜索结果展示状态。
  const isSearching = ref(false);
  // 搜索返回的 Mod 列表。
  const searchResults = ref<ModData[]>([]);
  // 当前处于快捷键绑定流程中的 Mod 信息；null 表示未在绑定。
  const modKeybindInfo = ref<ModKeybindInfo | null>(null);
  // 是否使用休闲风格（影响快捷键绑定时的展示与行为）。
  const isCasualStyle = ref(false);
  // 绑定目标是否为 ini 文件（区别于运行时 Mod）。
  const isIniFile = ref(false);

  /**
   * 当前选中的分组对象。
   * 通过 currentGroupPath 递归查找（支持嵌套子分组），找不到时返回 null。
   * currentGroupPath 是唯一真相源，currentGroupIndex 仅用于顶层循环切换。
   */
  const currentGroup = computed(() => {
    if (!currentGroupPath.value) return null;
    return findGroupByPath(currentGroupPath.value);
  });

  /**
   * 递归查找分组（支持树形结构）。
   * @param groupPath 分组路径
   * @param groups 分组列表（默认使用 modGroups）
   * @returns 找到的分组对象，未找到返回 null
   */
  function findGroupByPath(groupPath: string, groups: ModGroupData[] = modGroups.value): ModGroupData | null {
    for (const group of groups) {
      if (group.groupPath === groupPath) {
        return group;
      }
      // 递归查找子分组
      if (group.children && group.children.length > 0) {
        const found = findGroupByPath(groupPath, group.children);
        if (found) return found;
      }
    }
    return null;
  }

  /**
   * 递归查找分组在原始数组中的索引。
   * @param groupPath 分组路径
   * @returns 分组在 modGroups 中的索引，未找到返回 -1
   */
  function findGroupIndexByPath(groupPath: string): number {
    return modGroups.value.findIndex(g => g.groupPath === groupPath);
  }

  /**
   * 切换树节点的展开/折叠状态。
   * @param groupPath 分组路径
   */
  function toggleExpandPath(groupPath: string) {
    if (expandedPaths.value.has(groupPath)) {
      expandedPaths.value.delete(groupPath);
    } else {
      expandedPaths.value.add(groupPath);
    }
  }

  /**
   * 递归查找目标分组的所有祖先路径（用于展开父节点确保可见）。
   * @param targetPath 目标分组路径
   * @returns 祖先路径数组（从顶层到直接父节点），未找到返回空数组
   */
  function findAncestorPaths(targetPath: string): string[] {
    function search(groups: ModGroupData[], ancestors: string[]): string[] | null {
      for (const group of groups) {
        if (group.groupPath === targetPath) {
          return ancestors;
        }
        if (group.children && group.children.length > 0) {
          const result = search(group.children, [...ancestors, group.groupPath]);
          if (result !== null) return result;
        }
      }
      return null;
    }
    return search(modGroups.value, []) || [];
  }

  /**
   * 展开指定分组的所有父节点（确保该分组可见）。
   * @param groupPath 分组路径
   */
  function expandParentPaths(groupPath: string) {
    const ancestors = findAncestorPaths(groupPath);
    for (const ancestorPath of ancestors) {
      expandedPaths.value.add(ancestorPath);
    }
  }

  /**
   * 当前应展示给用户的 Mod 列表。
   * 处于搜索中时返回 searchResults；否则返回当前分组内的 modsInGroup。
   */
  const currentMods = computed(() => {
    if (isSearching.value && searchKeyword.value) {
      return searchResults.value;
    }
    return currentGroup.value?.modsInGroup || [];
  });

  /**
   * 已收藏的分组列表（favoriteDateTime 不为 null 视为已收藏）。
   */
  const favoriteGroups = computed(() => {
    return modGroups.value.filter(g => g.favoriteDateTime !== null);
  });

  /**
   * 按 realIndex 升序排列的分组副本。
   * 注意：返回的是拷贝，不会影响 modGroups 原数组的顺序。
   */
  const sortedGroups = computed(() => {
    return [...modGroups.value].sort((a, b) => a.realIndex - b.realIndex);
  });

  /**
   * 切换目标游戏，并广播 GAME_SWITCHED 事件。
   * 同时从 settingsStore 同步对应游戏的 Mods 路径到 gameStore.modsPath。
   * 注意：本函数仅更新状态并通知，不负责重新扫描 Mods，
   * 调用方应在监听到事件后自行触发加载流程。
   * @param game 新的目标游戏
   */
  function setTargetGame(game: TargetGame) {
    targetGame.value = game;
    const settingsStore = useSettingsStore();
    modsPath.value = settingsStore.getModsPath(game);
    eventManager.emit(EventNames.GAME_SWITCHED, { game });
  }

  /** 直接替换整个 Mods 列表。 */
  function setMods(newMods: ModData[]) {
    mods.value = newMods;
  }

  /**
   * 替换整个分组列表，并设置默认选中的分组。
   * 默认选中第一个非虚拟、realIndex 最小的真实分组（跳过虚拟分类节点），
   * 以保证左侧高亮分组与右侧展示内容同步。
   * currentGroupPath 是唯一真相源，currentGroupIndex 仅作辅助。
   */
  function setModGroups(newGroups: ModGroupData[]) {
    modGroups.value = newGroups;
    if (newGroups.length === 0) {
      currentGroupIndex.value = 0;
      currentGroupPath.value = '';
      return;
    }
    // 找到第一个非虚拟、realIndex 最小的顶层分组作为默认选中
    let selectedPath = '';
    let minRealIndex = Infinity;
    let minIndex = 0;
    for (let i = 0; i < newGroups.length; i++) {
      if (newGroups[i].isVirtual) continue;
      if (newGroups[i].realIndex < minRealIndex) {
        minRealIndex = newGroups[i].realIndex;
        minIndex = i;
        selectedPath = newGroups[i].groupPath;
      }
    }
    // 若全是虚拟节点（防御性处理），回退到第一个
    if (!selectedPath) {
      minIndex = 0;
      selectedPath = newGroups[0].groupPath;
    }
    currentGroupIndex.value = minIndex;
    currentGroupPath.value = selectedPath;
  }

  /** 设置 Mods 路径校验状态。 */
  function setModsPathStatus(status: ModsPathStatus) {
    modsPathStatus.value = status;
  }

  /** 设置 Mods 文件夹路径。 */
  function setModsPath(path: string) {
    modsPath.value = path;
  }

  /** 标记 Mods 是否已加载完成。 */
  function setModsLoaded(loaded: boolean) {
    isModsLoaded.value = loaded;
  }

  /**
   * 设置当前选中的分组索引。
   * 仅在索引合法（0 ~ length-1）时才更新，避免越界。
   * 同时更新 currentGroupPath 以保持同步。
   */
  function setCurrentGroupIndex(index: number) {
    if (index >= 0 && index < modGroups.value.length) {
      currentGroupIndex.value = index;
      currentGroupPath.value = modGroups.value[index].groupPath;
    }
  }

  /**
   * 通过路径设置当前选中的分组。
   * 使用递归查找（findGroupByPath），支持嵌套子分组。
   * @param groupPath 分组路径
   * @returns 是否成功设置（找不到分组时返回 false）
   */
  function setCurrentGroupByPath(groupPath: string): boolean {
    const group = findGroupByPath(groupPath);
    if (group) {
      currentGroupPath.value = groupPath;
      // 同步 currentGroupIndex（仅顶层有效，嵌套时保持原值）
      const idx = modGroups.value.findIndex(g => g.groupPath === groupPath);
      if (idx !== -1) {
        currentGroupIndex.value = idx;
      }
      // 展开所有父节点，确保选中的分组可见
      expandParentPaths(groupPath);
      return true;
    }
    return false;
  }

  /** 切换到下一个分组，循环回到开头。 */
  function nextGroup() {
    if (modGroups.value.length > 0) {
      currentGroupIndex.value = (currentGroupIndex.value + 1) % modGroups.value.length;
    }
  }

  /** 切换到上一个分组，循环回到末尾。 */
  function prevGroup() {
    if (modGroups.value.length > 0) {
      currentGroupIndex.value = (currentGroupIndex.value - 1 + modGroups.value.length) % modGroups.value.length;
    }
  }

  /**
   * 设置搜索关键字并同步更新 isSearching 标志。
   * 关键字非空即视为进入搜索态。
   */
  function setSearchKeyword(keyword: string) {
    searchKeyword.value = keyword;
    isSearching.value = keyword.length > 0;
  }

  /**
   * 调用后端执行 Mod 搜索。
   * 业务逻辑：
   * 1. 关键字为空时直接清空搜索结果并退出搜索态；
   * 2. 否则进入搜索态，调用 invokeSearchMods 取结果；
   * 3. 出错时将结果置空，保证界面不显示陈旧数据。
   * @param keyword 搜索关键字
   */
  async function searchMods(keyword: string) {
    searchKeyword.value = keyword;
    if (!keyword.trim()) {
      isSearching.value = false;
      searchResults.value = [];
      return;
    }
    isSearching.value = true;
    try {
      searchResults.value = await invokeSearchMods(keyword, targetGame.value);
    } catch {
      // 搜索失败时清空结果，避免展示上一次的过期数据
      searchResults.value = [];
    }
  }

  /** 清空搜索状态：关键字、搜索标志、结果列表全部复位。 */
  function clearSearch() {
    searchKeyword.value = '';
    isSearching.value = false;
    searchResults.value = [];
  }

  /** 设置当前快捷键绑定流程对应的 Mod 信息，传 null 表示结束绑定。 */
  function setModKeybindInfo(info: ModKeybindInfo | null) {
    modKeybindInfo.value = info;
  }

  /**
   * 切换单个 Mod 的收藏状态。
   * 业务逻辑：
   * 1. 调用后端 invokeToggleModFavorite 完成持久化；
   * 2. 同步更新 mods 平铺列表中对应 Mod 的 favoriteDateTime；
   * 3. 同步遍历所有分组，更新分组内同一 Mod 的收藏时间，保持数据一致；
   * 4. 出错返回 false，不抛出。
   * @param modPath Mod 路径（唯一标识）
   * @returns 收藏结果：true 表示已收藏，false 表示已取消或失败
   */
  async function toggleModFavorite(modPath: string): Promise<boolean> {
    try {
      const result = await invokeToggleModFavorite(modPath);
      // 同步更新平铺列表中的收藏时间
      const mod = mods.value.find(m => m.modPath === modPath);
      if (mod) {
        mod.favoriteDateTime = result ? new Date().toISOString() : null;
      }
      // 同步更新各分组内同一 Mod 的收藏时间，保证多处展示一致
      for (const group of modGroups.value) {
        const groupMod = group.modsInGroup.find(m => m.modPath === modPath);
        if (groupMod) {
          groupMod.favoriteDateTime = result ? new Date().toISOString() : null;
        }
      }
      return result;
    } catch {
      return false;
    }
  }

  /**
   * 切换分组的收藏状态。
   * 持久化后同步更新对应分组的 favoriteDateTime（递归查找，支持嵌套子分组）。
   * @param groupPath 分组路径（唯一标识）
   * @returns 收藏结果：true 表示已收藏，false 表示已取消或失败
   */
  async function toggleGroupFavorite(groupPath: string): Promise<boolean> {
    try {
      const result = await invokeToggleGroupFavorite(groupPath);
      const group = findGroupByPath(groupPath);
      if (group) {
        group.favoriteDateTime = result ? new Date().toISOString() : null;
      }
      return result;
    } catch {
      return false;
    }
  }

  /**
   * 局部更新某分组内指定 Mod 的字段。
   * 采用浅合并方式，仅更新 modData 中给出的字段。
   * 通过 groupPath 递归查找分组（支持嵌套子分组）。
   * @param groupPath 分组路径
   * @param modIndex 分组内 Mod 下标
   * @param modData 需要覆盖的字段集合
   */
  function updateModInGroup(groupPath: string, modIndex: number, modData: Partial<ModData>) {
    const group = findGroupByPath(groupPath);
    if (group && modIndex >= 0 && modIndex < group.modsInGroup.length) {
      group.modsInGroup[modIndex] = { ...group.modsInGroup[modIndex], ...modData };
    }
  }

  /**
   * 更新单个分组的模组列表（仅更新 mods，保留原有 children）。
   * 通过 groupPath 递归查找分组（支持嵌套子分组）。
   * @param groupPath 分组路径
   * @param newGroup 包含最新 mods 的分组数据
   */
  function updateGroup(groupPath: string, newGroup: ModGroupData) {
    const group = findGroupByPath(groupPath);
    if (group) {
      group.modsInGroup = newGroup.modsInGroup;
      group.previousSelectedModOnGroup = newGroup.previousSelectedModOnGroup;
    }
  }

  /** 追加一个新分组到分组列表末尾。 */
  function addModGroup(group: ModGroupData) {
    modGroups.value.push(group);
  }

  /**
   * 按分组路径移除分组。
   * 移除后若 currentGroupIndex 越界，则回退到末尾合法位置。
   */
  function removeModGroup(groupPath: string) {
    const index = modGroups.value.findIndex(g => g.groupPath === groupPath);
    if (index > -1) {
      modGroups.value.splice(index, 1);
      if (currentGroupIndex.value >= modGroups.value.length) {
        currentGroupIndex.value = Math.max(0, modGroups.value.length - 1);
      }
    }
  }

  return {
    targetGame,
    mods,
    modGroups,
    modsPathStatus,
    modsPath,
    isModsLoaded,
    currentGroupIndex,
    currentGroupPath,
    expandedPaths,
    searchKeyword,
    isSearching,
    searchResults,
    modKeybindInfo,
    isCasualStyle,
    isIniFile,
    currentGroup,
    currentMods,
    favoriteGroups,
    sortedGroups,
    setTargetGame,
    setMods,
    setModGroups,
    setModsPathStatus,
    setModsPath,
    setModsLoaded,
    setCurrentGroupIndex,
    setCurrentGroupByPath,
    nextGroup,
    prevGroup,
    setSearchKeyword,
    searchMods,
    clearSearch,
    setModKeybindInfo,
    toggleModFavorite,
    toggleGroupFavorite,
    updateModInGroup,
    updateGroup,
    addModGroup,
    removeModGroup,
    findGroupByPath,
    findGroupIndexByPath,
    findAncestorPaths,
    toggleExpandPath,
    expandParentPaths
  };
});
