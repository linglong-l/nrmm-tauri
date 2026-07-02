// 游戏组合式函数模块。
// 该模块对 game store 进行二次封装，统一暴露游戏选择、Mod 列表/分组、搜索、收藏、键位绑定等相关的响应式状态与方法。
// 通过该 composable，组件层无需直接依赖 Pinia store 即可完成 Mod 数据的读取与操作。
import { storeToRefs } from 'pinia';
import { useGameStore } from '../stores/game';
import type { TargetGame, ModKeybindInfo, ModGroupData, ModData } from '../types';

/**
 * 游戏与 Mod 管理组合式函数。
 *
 * 作用：
 * - 通过 `storeToRefs` 把 game store 中的状态转为可响应式引用，便于在组件中解构使用；
 * - 集中提供目标游戏切换、Mod 列表/分组的设置与查询、分组切换、搜索、收藏、键位绑定信息维护等能力；
 * - 涉及后端交互的操作（如收藏、搜索）由 store 转发到 Tauri 后端完成。
 *
 * 限制条件：
 * - 必须在 Pinia 已初始化的上下文中调用（通常在 setup 函数内）；
 * - 收藏与搜索相关操作为异步，调用方需自行处理加载/失败状态；
 * - `currentGroup`/`currentMods`/`favoriteGroups`/`sortedGroups` 为 getter，依赖其他 state 计算。
 *
 * @returns 包含游戏与 Mod 相关响应式状态及一组操作方法的对象
 */
export function useGame() {
  // 获取 game store 实例，所有 Mod 数据与后端交互均由该 store 承载
  const gameStore = useGameStore();

  // 将 store 中的 state/getter 转为 ref，保证解构后仍具响应性
  const {
    // 当前目标游戏（决定加载哪个游戏的 Mods 目录）
    targetGame,
    // 当前游戏下全部 Mod 的扁平列表
    mods,
    // 当前游戏下的全部分组数据
    modGroups,
    // Mods 目录路径的合法性校验状态
    modsPathStatus,
    // 当前游戏 Mods 目录的绝对路径
    modsPath,
    // Mods 是否已加载完成
    isModsLoaded,
    // 当前选中的分组索引
    currentGroupIndex,
    // 当前选中的分组路径（用于树形结构定位）
    currentGroupPath,
    // 展开的树节点路径集合
    expandedPaths,
    // 当前搜索关键词
    searchKeyword,
    // 是否正在执行搜索
    isSearching,
    // 搜索结果列表
    searchResults,
    // 当前 keybinds 标签页需要展示的 Mod 键位绑定信息
    modKeybindInfo,
    // 是否为休闲风格（影响展示样式）
    isCasualStyle,
    // 是否为 INI 文件类型 Mod
    isIniFile,
    // 当前选中的分组对象（getter，由 currentGroupIndex 推导）
    currentGroup,
    // 当前选中分组下的 Mod 列表（getter）
    currentMods,
    // 收藏的分组列表（getter）
    favoriteGroups,
    // 排序后的分组列表（getter，按 sortGroupMethod 排序）
    sortedGroups
  } = storeToRefs(gameStore);

  /**
   * 切换目标游戏。
   * 切换后会触发对应游戏 Mods 目录的加载与界面状态重置。
   * @param game 目标游戏枚举值
   */
  function setTargetGame(game: TargetGame) {
    gameStore.setTargetGame(game);
  }

  /**
   * 设置当前游戏的 Mod 列表（覆盖式赋值）。
   * 通常由后端刷新数据后调用，将最新 Mod 数据写入前端状态。
   * @param newMods 新的 Mod 数据数组
   */
  function setMods(newMods: ModData[]) {
    gameStore.setMods(newMods);
  }

  /**
   * 设置当前游戏的分组列表（覆盖式赋值）。
   * @param newGroups 新的分组数据数组
   */
  function setModGroups(newGroups: ModGroupData[]) {
    gameStore.setModGroups(newGroups);
  }

  /**
   * 设置 Mods 目录路径的合法性校验状态。
   * 用于在 UI 上显示具体的路径错误原因。
   * @param status 校验状态值（与 modsPathStatus 同类型）
   */
  function setModsPathStatus(status: typeof modsPathStatus.value) {
    gameStore.setModsPathStatus(status);
  }

  /**
   * 设置当前游戏的 Mods 目录路径。
   * @param path Mods 目录绝对路径
   */
  function setModsPath(path: string) {
    gameStore.setModsPath(path);
  }

  /**
   * 设置 Mods 是否已加载完成的标记。
   * @param loaded 是否已加载
   */
  function setModsLoaded(loaded: boolean) {
    gameStore.setModsLoaded(loaded);
  }

  /**
   * 设置当前选中的分组索引。
   * 会同步更新 currentGroup/currentMods 等 getter。
   * @param index 分组索引
   */
  function setCurrentGroupIndex(index: number) {
    gameStore.setCurrentGroupIndex(index);
  }

  /**
   * 通过路径设置当前选中的分组。
   * @param groupPath 分组路径
   * @returns 是否成功设置
   */
  function setCurrentGroupByPath(groupPath: string): boolean {
    return gameStore.setCurrentGroupByPath(groupPath);
  }

  /**
   * 切换树节点的展开/折叠状态。
   * @param groupPath 分组路径
   */
  function toggleExpandPath(groupPath: string) {
    gameStore.toggleExpandPath(groupPath);
  }

  /**
   * 查找分组（支持树形结构）。
   * @param groupPath 分组路径
   * @returns 找到的分组对象，未找到返回 null
   */
  function findGroupByPath(groupPath: string): ModGroupData | null {
    return gameStore.findGroupByPath(groupPath);
  }

  /**
   * 切换到下一个分组（循环到末尾后会回到开头）。
   */
  function nextGroup() {
    gameStore.nextGroup();
  }

  /**
   * 切换到上一个分组（循环到开头后会回到末尾）。
   */
  function prevGroup() {
    gameStore.prevGroup();
  }

  /**
   * 设置搜索关键词（仅更新内存状态，不触发实际搜索）。
   * @param keyword 搜索关键词
   */
  function setSearchKeyword(keyword: string) {
    gameStore.setSearchKeyword(keyword);
  }

  /**
   * 执行 Mod 搜索（异步，会调用后端搜索逻辑并填充 searchResults）。
   * @param keyword 搜索关键词
   * @returns 搜索结果（具体返回类型由 store 决定，通常为搜索结果数组或操作是否成功）
   */
  async function searchMods(keyword: string) {
    return gameStore.searchMods(keyword);
  }

  /**
   * 清空搜索状态（重置关键词与搜索结果）。
   */
  function clearSearch() {
    gameStore.clearSearch();
  }

  /**
   * 设置当前 keybinds 标签页需要展示的 Mod 键位绑定信息。
   * 传 null 表示清除当前展示。
   * @param info 键位绑定信息对象，或 null
   */
  function setModKeybindInfo(info: ModKeybindInfo | null) {
    gameStore.setModKeybindInfo(info);
  }

  /**
   * 切换某个 Mod 的收藏状态（异步，会同步至后端持久化）。
   * @param modPath Mod 的完整路径
   * @returns 是否操作成功
   */
  async function toggleModFavorite(modPath: string): Promise<boolean> {
    return gameStore.toggleModFavorite(modPath);
  }

  /**
   * 切换某个分组的收藏状态（异步，会同步至后端持久化）。
   * @param groupPath 分组的完整路径
   * @returns 是否操作成功
   */
  async function toggleGroupFavorite(groupPath: string): Promise<boolean> {
    return gameStore.toggleGroupFavorite(groupPath);
  }

  /**
   * 局部更新指定分组内指定 Mod 的部分字段。
   * 用于在编辑 Mod 后无需全量刷新即可更新 UI。
   * @param groupPath 分组路径
   * @param modIndex 该分组内的 Mod 索引
   * @param modData 待覆盖的字段集合
   */
  function updateModInGroup(groupPath: string, modIndex: number, modData: Partial<ModData>) {
    gameStore.updateModInGroup(groupPath, modIndex, modData);
  }

  /**
   * 更新单个分组的模组列表（仅更新 mods，保留原有 children）。
   * @param groupPath 分组路径
   * @param newGroup 包含最新 mods 的分组数据
   */
  function updateGroup(groupPath: string, newGroup: ModGroupData) {
    gameStore.updateGroup(groupPath, newGroup);
  }

  /**
   * 添加一个新的分组到分组列表。
   * @param group 新的分组数据
   */
  function addModGroup(group: ModGroupData) {
    gameStore.addModGroup(group);
  }

  /**
   * 按路径移除一个分组。
   * @param groupPath 分组的完整路径
   */
  function removeModGroup(groupPath: string) {
    gameStore.removeModGroup(groupPath);
  }

  // 统一返回响应式状态与方法，供调用方按需解构使用
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
    toggleExpandPath,
    findGroupByPath,
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
    removeModGroup
  };
}
