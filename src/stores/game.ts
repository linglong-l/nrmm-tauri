import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import type { ModData, ModGroupData, ModsPathStatus, TargetGame, ModKeybindInfo } from '../types';
import { invoke } from '@tauri-apps/api/core';
import { invokeToggleModFavorite, invokeToggleGroupFavorite, invokeSearchMods, invokeSetSelectedMod } from '../utils/invoke';
import { EventNames, eventManager } from '../utils/events';
import { useSettingsStore } from './settings';
import { getModsCache, setModsCache, removeModsCache } from '../utils/cache';
import { ElMessage } from 'element-plus';
import { i18n } from '../locales';

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
   * 性能优化与交互改进相关状态
   */
  // 是否正在加载模组（用于 UI loading 状态显示）
  const isLoading = ref(false);
  // 加载状态：idle(空闲) | loading(加载中) | cancelled(已取消) | completed(已完成) | error(错误)
  const loadStatus = ref<'idle' | 'loading' | 'cancelled' | 'completed' | 'error'>('idle');
  // 最后一次请求加载的游戏（用于数据一致性校验）
  const lastRequestedGame = ref<TargetGame | null>(null);
  /**
   * 当前缓存数据所属的游戏。
   * 与 targetGame 分离：targetGame 在 setTargetGame 中立即更新（UI 响应），
   * 而 cachedGame 仅在 setModGroups 中更新（数据实际加载完成时）。
   * validateCache 通过 cachedGame 检测"缓存数据所属游戏与请求游戏不一致"的场景，
   * 避免 setTargetGame 提前更新 targetGame 导致缓存校验误判为 use_cache。
   */
  const cachedGame = ref<TargetGame>('none' as TargetGame);

  /**
   * 各分组当前选中的模组路径映射表。
   * key: 分组路径，value: 选中模组的 modPath。
   * 从后端返回的 previousSelectedModOnGroup 索引初始化，用于前端紫色描边和排序。
   */
  const selectedModPaths = ref<Map<string, string>>(new Map());

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
   * 查找分组（支持树形结构）。
   *
   * 使用栈进行 DFS 迭代遍历，避免递归导致的深层次栈溢出风险。
   * 遍历顺序：将子分组压入栈中，逐个弹出检查路径匹配。
   *
   * @param groupPath 分组路径
   * @param groups 分组列表（默认使用 modGroups）
   * @returns 找到的分组对象，未找到返回 null
   */
  function findGroupByPath(groupPath: string, groups: ModGroupData[] = modGroups.value): ModGroupData | null {
    const stack: ModGroupData[] = [...groups];
    while (stack.length > 0) {
      const group = stack.pop()!;
      if (group.groupPath === groupPath) {
        return group;
      }
      // 将子分组压入栈中继续迭代查找
      if (group.children && group.children.length > 0) {
        stack.push(...group.children);
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
    const next = new Set(expandedPaths.value);
    if (next.has(groupPath)) {
      next.delete(groupPath);
    } else {
      next.add(groupPath);
    }
    expandedPaths.value = next;
  }

  /**
   * 查找目标分组的所有祖先路径（用于展开父节点确保可见）。
   *
   * 使用栈进行 DFS 迭代遍历，避免递归导致的深层次栈溢出风险。
   * 每个栈元素携带当前分组及其祖先路径列表，匹配到目标时直接返回其祖先路径。
   *
   * @param targetPath 目标分组路径
   * @returns 祖先路径数组（从顶层到直接父节点），未找到返回空数组
   */
  function findAncestorPaths(targetPath: string): string[] {
    // 使用栈进行 DFS 迭代，每个栈元素包含当前分组和其祖先路径
    const stack: Array<{ group: ModGroupData; ancestors: string[] }> = [];
    for (const g of modGroups.value) {
      stack.push({ group: g, ancestors: [] });
    }
    while (stack.length > 0) {
      const { group, ancestors } = stack.pop()!;
      if (group.groupPath === targetPath) {
        return ancestors;
      }
      // 将子分组及其更新后的祖先路径压入栈中继续查找
      if (group.children && group.children.length > 0) {
        const childAncestors = [...ancestors, group.groupPath];
        for (const child of group.children) {
          stack.push({ group: child, ancestors: childAncestors });
        }
      }
    }
    return [];
  }

  /**
   * 展开指定分组的所有父节点（确保该分组可见）。
   * @param groupPath 分组路径
   */
  function expandParentPaths(groupPath: string) {
    const ancestors = findAncestorPaths(groupPath);
    if (ancestors.length === 0) return;
    const next = new Set(expandedPaths.value);
    for (const ancestorPath of ancestors) {
      next.add(ancestorPath);
    }
    expandedPaths.value = next;
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
   * 排序后的分组列表（含置顶功能）。
   *
   * 排序优先级（从高到低）：
   * 1. 收藏的角色分组（groupPath 包含 'group_'）— 按 favoriteDateTime 降序
   * 2. 收藏的非角色分组 — 按 favoriteDateTime 降序
   * 3. 未收藏的角色分组 — 按 realIndex 升序
   * 4. 未收藏的非角色分组 — 按 realIndex 升序
   *
   * 置顶机制复用 favoriteDateTime 字段：收藏即置顶。
   * 角色分组通过 groupPath 是否包含 'group_' 识别（对应 Mods/_MANAGED_/group_xx 目录）。
   *
   * 注意：返回的是拷贝，不会影响 modGroups 原数组的顺序。
   */
  const sortedGroups = computed(() => {
    return [...modGroups.value].sort((a, b) => {
      const aFav = a.favoriteDateTime !== null;
      const bFav = b.favoriteDateTime !== null;
      
      // 收藏分组始终排在未收藏分组之前（NRMM兼容）
      if (aFav !== bFav) {
        return aFav ? -1 : 1;
      }
      
      // 收藏分组按 favoriteDateTime 降序（最近收藏的在前）
      if (aFav && bFav) {
        return (b.favoriteDateTime ?? '').localeCompare(a.favoriteDateTime ?? '');
      }
      
      // 未收藏分组按 realIndex 升序（NRMM兼容）
      return a.realIndex - b.realIndex;
    });
  });

  /**
   * 切换目标游戏，并广播 GAME_SWITCHED 事件。
   * 同时从 settingsStore 同步对应游戏的 Mods 路径到 gameStore.modsPath。
   *
   * 注意：本函数仅负责状态更新和事件发射，不触发模组加载。
   * 模组加载由调用方（如 useGame.ts 的防抖回调或 ModsTab.vue 的事件监听器）显式控制，
   * 避免状态更新与加载操作耦合导致的事件循环和重复加载问题。
   *
   * 同时同步更新 settingsStore 中的 targetGame 并持久化到配置文件，
   * 确保 gameStore 与 settingsStore 的目标游戏状态始终一致。
   *
   * @param game 新的目标游戏
   */
  function setTargetGame(game: TargetGame) {
    targetGame.value = game;
    const settingsStore = useSettingsStore();
    modsPath.value = settingsStore.getModsPath(game);
    // 同步更新 settings 中的 targetGame 并持久化
    settingsStore.setTargetGame(game);
    settingsStore.saveSettings().catch(() => {
      console.warn('[gameStore] Failed to save settings after game change');
    });
    eventManager.emitLocal(EventNames.GAME_SWITCHED, { game });
  }

  /**
   * 从设置中初始化游戏状态。
   * 在设置加载完成后调用，确保 gameStore 与 settingsStore 保持同步。
   * 仅当 targetGame 仍为 'none' 时才同步，避免覆盖用户已选择的游戏。
   */
  function initFromSettings() {
    if (targetGame.value !== 'none') {
      return;
    }
    const settingsStore = useSettingsStore();
    const settingsGame = settingsStore.targetGame;
    if (settingsGame && settingsGame !== 'none') {
      targetGame.value = settingsGame;
      modsPath.value = settingsStore.getModsPath(settingsGame);
      eventManager.emitLocal(EventNames.GAME_SWITCHED, { game: settingsGame });
    }
  }

  /**
   * 加载指定游戏的模组数据。
   * 
   * 实现缓存优先策略与数据一致性保障：
   * 1. 检查内存缓存：若当前游戏的数据已加载且未过期，直接返回
   * 2. 检查 localStorage 缓存：若存在有效缓存，直接使用
   * 3. 从后端加载：缓存不存在或过期时，调用后端 API 并更新缓存
   * 4. 数据一致性校验：仅当返回的数据与最后一次请求的游戏匹配时才更新状态
   * 5. 根据结果更新加载状态（completed / cancelled / error）
   * 
   * @param game 目标游戏
   */
  async function loadModsForGame(game: TargetGame) {
    // 检查 game 是否为 none（未选择游戏），若是则直接返回，不发起任何请求
    if (game === 'none') {
      return;
    }

    // 缓存数据校验：检查当前缓存数据与请求游戏的一致性
    const cacheCheck = validateCache(game);

    // skip：游戏未选择，直接返回
    if (cacheCheck.action === 'skip') {
      return;
    }

    // use_cache：缓存有效且游戏匹配，直接返回
    if (cacheCheck.action === 'use_cache') {
      loadStatus.value = 'completed';
      return;
    }

    // clear_and_load：缓存数据所属游戏与请求游戏不一致，先清除旧缓存并重置加载状态
    if (cacheCheck.action === 'clear_and_load') {
      clearModsCache(cachedGame.value);
      // 重置加载状态，确保走完整加载流程（避免后续 isModsLoaded 检查误判）
      isModsLoaded.value = false;
      modGroups.value = [];
    }

    // 更新加载状态
    loadStatus.value = 'loading';
    isLoading.value = true;
    // 记录最后一次请求的游戏（用于数据一致性校验）
    lastRequestedGame.value = game;

    try {
      // 检查 localStorage 缓存
      const cachedGroups = getModsCache<ModGroupData[]>(game);
      if (cachedGroups && cachedGroups.length > 0) {
        if (lastRequestedGame.value === game) {
          setModGroups(cachedGroups);
          setModsLoaded(true);
          loadStatus.value = 'completed';
        } else {
          loadStatus.value = 'cancelled';
        }
        return;
      }

      // 缓存不存在或过期，从后端加载
      const groups = await invoke<ModGroupData[]>('load_mods', { game });

      // 数据一致性检查：仅当返回的数据与最后一次请求的游戏匹配时才更新状态
      if (lastRequestedGame.value === game) {
        setModGroups(groups);
        setModsLoaded(true);
        loadStatus.value = 'completed';
        // 更新 localStorage 缓存
        setModsCache(game, groups);
      } else {
        // 数据已过时（用户已切换到其他游戏），标记为已取消
        loadStatus.value = 'cancelled';
      }
    } catch (error) {
      // 检查是否为后端任务取消错误
      if (error === 'Task cancelled' || String(error).includes('cancelled')) {
        loadStatus.value = 'cancelled';
      } else {
        // 加载失败：清空旧数据，避免展示上一个游戏的残留内容
        modGroups.value = [];
        isModsLoaded.value = false;
        loadStatus.value = 'error';
        console.error('[gameStore] loadModsForGame failed:', error);
      }
    } finally {
      isLoading.value = false;
    }
  }

  /**
   * 缓存校验结果类型。
   * - `skip`：游戏未选择（none），无需任何操作
   * - `load`：缓存为空且游戏合法，需从后端加载最新数据
   * - `clear_and_load`：缓存数据所属游戏与请求游戏不一致，需先清除旧缓存再加载
   * - `use_cache`：缓存有效且游戏匹配，可直接使用现有数据
   */
  type CacheValidationResult = {
    action: 'skip' | 'load' | 'clear_and_load' | 'use_cache';
  };

  /**
   * 校验缓存数据与当前请求游戏的一致性。
   *
   * 业务逻辑：
   * 1. 游戏为 'none' 时直接返回 skip，不发起任何请求
   * 2. 内存缓存为空（modGroups 为空或未加载）且游戏合法时返回 load
   * 3. 内存缓存所属游戏（cachedGame.value）与请求游戏不同且游戏合法时返回 clear_and_load
   * 4. 内存缓存有效且游戏匹配时返回 use_cache
   *
   * 注意：使用 cachedGame 而非 targetGame 进行比较，因为 targetGame 在 setTargetGame
   * 中已被立即更新为新游戏，而 cachedGame 仅在 setModGroups 中更新，能准确反映当前
   * modGroups 数据实际所属的游戏。
   *
   * 该函数为纯函数，无副作用，调用方根据返回的 action 决定后续操作。
   *
   * @param game 待校验的目标游戏
   * @returns 校验结果对象，包含 action 字段指示后续操作
   */
  function validateCache(game: TargetGame): CacheValidationResult {
    // 游戏未选择，直接跳过
    if (game === 'none') {
      return { action: 'skip' };
    }

    // 内存缓存为空：需从后端加载
    if (!isModsLoaded.value || modGroups.value.length === 0) {
      return { action: 'load' };
    }

    // 缓存数据所属游戏与请求游戏不一致：需先清除旧缓存再加载
    // 使用 cachedGame 而非 targetGame，避免 setTargetGame 提前更新导致误判
    if (cachedGame.value !== game) {
      return { action: 'clear_and_load' };
    }

    // 缓存有效且游戏匹配
    return { action: 'use_cache' };
  }

  /**
   * 清除指定游戏的模组缓存。
   * 
   * 适用于文件变化、手动刷新等场景，确保下次加载时从后端获取最新数据。
   * 
   * @param game 目标游戏，不传则清除当前游戏的缓存
   */
  function clearModsCache(game?: TargetGame) {
    const target = game || targetGame.value;
    removeModsCache(target);
  }

  /** 直接替换整个 Mods 列表。 */
  function setMods(newMods: ModData[]) {
    mods.value = newMods;
  }

  /**
   * 替换整个分组列表，并恢复或设置默认选中的分组。
   *
   * 优先恢复重载前用户选中的分组（通过 currentGroupPath 查找），
   * 若分组在新数据中不存在（如已被删除）则回退到默认选中逻辑：
   * 第一个非虚拟、realIndex 最小的真实分组（跳过虚拟分类节点）。
   * currentGroupPath 是唯一真相源，currentGroupIndex 仅作辅助。
   */
  function setModGroups(newGroups: ModGroupData[]) {
    // 保存重载前的当前选中分组路径，用于后续恢复
    const previousGroupPath = currentGroupPath.value;

    modGroups.value = newGroups;
    // 同步更新 cachedGame：当前 modGroups 数据实际所属的游戏
    cachedGame.value = targetGame.value;

    // 初始化各分组的选中模组路径映射（使用栈进行 DFS 迭代，避免递归）
    const newSelectedMap = new Map<string, string>();
    // 收集加载时发现的"选中项为禁用模组"冲突，加载完成后统一告知用户
    const disabledSelectionConflicts: Array<{ groupName: string; modName: string; groupPath: string }> = [];
    const initStack: ModGroupData[] = [...newGroups];
    while (initStack.length > 0) {
      const group = initStack.pop()!;
      const idx = group.previousSelectedModOnGroup;
      if (idx >= 0 && idx < group.modsInGroup.length) {
        const selectedMod = group.modsInGroup[idx];
        if (selectedMod) {
          // 与点击路径（selectModInGroup）保持一致：禁用模组不可被选中
          if (selectedMod.isDisabled) {
            console.warn(`[ModSelection] Group '${group.groupName}': selected mod '${selectedMod.modName}' is disabled, deselecting.`);
            disabledSelectionConflicts.push({ groupName: group.groupName, modName: selectedMod.modName, groupPath: group.groupPath });
          } else {
            newSelectedMap.set(group.groupPath, selectedMod.modPath);
          }
        }
      }
      // 将子分组压入栈中继续处理
      if (group.children && group.children.length > 0) {
        initStack.push(...group.children);
      }
    }
    selectedModPaths.value = newSelectedMap;

    // 对加载到禁用模组的分组，异步重置 selectedindex 文件为 0
    // 确保下次重启时不再加载到禁用模组
    for (const conflict of disabledSelectionConflicts) {
      // # 目录分组不使用 selectedindex 机制，跳过重置
      const conflictGroup = findGroupByPath(conflict.groupPath);
      if (conflictGroup && conflictGroup.isTreeNode && !conflictGroup.isVirtual) continue;
      invokeSetSelectedMod(conflict.groupPath, 0).catch((e) => {
        console.error(`[ModSelection] Failed to reset selectedindex for group '${conflict.groupName}':`, e);
      });
    }

    // 统一告知用户（系统只负责告知，具体处理由用户决定）
    if (disabledSelectionConflicts.length > 0) {
      const t = i18n.global.t;
      const message = disabledSelectionConflicts.length === 1
        ? t('Selected mod "{modName}" in group "{groupName}" is disabled, selection has been reset',
            { modName: disabledSelectionConflicts[0].modName, groupName: disabledSelectionConflicts[0].groupName })
        : t('{count} selected mods are disabled and have been reset',
            { count: disabledSelectionConflicts.length });
      ElMessage.warning(message);
    }

    // 恢复选中分组：优先查找之前的分组，找不到时回退到默认
    if (previousGroupPath) {
      const previousGroup = findGroupByPath(previousGroupPath, newGroups);
      if (previousGroup) {
        // 之前的选中分组在新数据中仍然存在，恢复选择
        currentGroupPath.value = previousGroupPath;
        const idx = newGroups.findIndex(g => g.groupPath === previousGroupPath);
        currentGroupIndex.value = idx !== -1 ? idx : 0;
        // 展开父节点确保选中分组可见
        expandParentPaths(previousGroupPath);
        return;
      }
    }

    // 回退：选择第一个非虚拟、realIndex 最小的顶层分组作为默认选中
    if (newGroups.length === 0) {
      currentGroupIndex.value = 0;
      currentGroupPath.value = '';
      return;
    }
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
      const nextIndex = (currentGroupIndex.value + 1) % modGroups.value.length;
      currentGroupIndex.value = nextIndex;
      currentGroupPath.value = modGroups.value[nextIndex].groupPath;
    }
  }

  /** 切换到上一个分组，循环回到末尾。 */
  function prevGroup() {
    if (modGroups.value.length > 0) {
      const prevIndex = (currentGroupIndex.value - 1 + modGroups.value.length) % modGroups.value.length;
      currentGroupIndex.value = prevIndex;
      currentGroupPath.value = modGroups.value[prevIndex].groupPath;
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
   * 获取指定分组当前选中的模组路径。
   * @param groupPath 分组路径
   * @returns 选中模组的 modPath，未选中时返回 null
   */
  function getSelectedModPath(groupPath: string): string | null {
    return selectedModPaths.value.get(groupPath) ?? null;
  }

  /**
   * 设置指定分组当前选中的模组路径。
   * 仅允许选择已启用的模组，禁用模组的选择请求将被忽略。
   * @param groupPath 分组路径
   * @param modPath 选中模组的 modPath
   */
  function setSelectedModPath(groupPath: string, modPath: string) {
    const group = findGroupByPath(groupPath);
    if (group) {
      const mod = group.modsInGroup.find(m => m.modPath === modPath);
      if (mod && !mod.isDisabled) {
        selectedModPaths.value.set(groupPath, modPath);
        selectedModPaths.value = new Map(selectedModPaths.value);
      }
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

/**
 * 从指定分组中移除指定索引的模组。
 * 用于将模组移动到还原区的场景。
 * @param groupPath 分组路径
 * @param modIndex 该分组内的模组索引
 */
function removeModFromGroup(groupPath: string, modIndex: number) {
  const group = findGroupByPath(groupPath);
  if (group && modIndex >= 0 && modIndex < group.modsInGroup.length) {
    group.modsInGroup.splice(modIndex, 1);
    // realIndex 由后端管理（来自目录列表原始顺序），前端不维护递减逻辑；
    // refreshMods() 会从后端重新加载完整数据，无需在此处手动修正 realIndex。
  }
}

  return {
    targetGame,
    cachedGame: computed(() => cachedGame.value),
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
    // 性能优化与交互改进相关状态
    isLoading,
    loadStatus,
    lastRequestedGame,
    currentGroup,
    currentMods,
    favoriteGroups,
    sortedGroups,
    setTargetGame,
    initFromSettings,
    loadModsForGame,
    validateCache,
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
    getSelectedModPath,
    setSelectedModPath,
    updateGroup,
    addModGroup,
    removeModGroup,
    removeModFromGroup,
    findGroupByPath,
    findGroupIndexByPath,
    findAncestorPaths,
    toggleExpandPath,
    expandParentPaths,
    clearModsCache
  };
});

/**
 * 模组显示排序工具函数。
 *
 * 作用：
 * - 用于前端展示前的模组排序，规则与 NRMM 一致：
 *   1. None 占位模组（realIndex === 0）始终排在最前；
 *   2. 当前选中的模组排在 None 之后；
 *   3. 其余模组按后端返回的数组原始顺序排列（保持 disabled-last / favorites-first / name 排序结果）。
 *
 * 设计原因：
 * - 修复 realIndex 一致性后，realIndex 不再等于数组位置（realIndex 来自目录列表原始顺序），
 *   因此不能按 realIndex 排序，否则显示顺序会变为目录列表顺序而非 disabled/favorites/name 顺序。
 * - 改为按原始数组位置排序，可保持后端返回的排序顺序不变。
 *
 * @param mods 后端返回的模组数组（已按 disabled-last / favorites-first / name 排序）
 * @param selectedPath 当前选中模组的 modPath，无选中时传 null
 * @returns 排序后的新数组（不修改原数组）
 */
export function sortModsForDisplay(mods: ModData[], selectedPath: string | null): ModData[] {
  const indexed = mods.map((mod, idx) => ({ mod, idx }));
  indexed.sort((a, b) => {
    if (a.mod.realIndex === 0) return -1;
    if (b.mod.realIndex === 0) return 1;
    // 仅对已启用且被选中的模组执行置顶操作
    const aSelected = !a.mod.isDisabled && a.mod.modPath === selectedPath;
    const bSelected = !b.mod.isDisabled && b.mod.modPath === selectedPath;
    if (aSelected && !bSelected) return -1;
    if (!aSelected && bSelected) return 1;
    return a.idx - b.idx;
  });
  return indexed.map(item => item.mod);
}
