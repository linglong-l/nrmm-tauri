<script setup lang="ts">
/**
 * ModsTab.vue - 模组管理标签页
 *
 * 作用：
 *  - 应用核心页面，负责模组与模组分组的浏览、搜索、启用/禁用、收藏、重命名、删除等操作。
 *  - 支持两种布局：网格布局（Grid）与轮播布局（Carousel），可实时切换。
 *  - 集成文件系统监听：当 Mods 目录下的文件发生变化时，自动防抖刷新模组列表。
 *  - 提供右键上下文菜单：对分组/模组进行重命名、收藏、打开所在文件夹、删除等操作。
 *  - 提供新建分组、重命名分组、重命名模组三个对话框。
 *
 * 限制条件：
 *  - realIndex === 0 的项是"空槽位"占位符，不可切换启用状态、不可收藏、不可打开文件夹。
 *  - 删除分组会使其内部模组脱离模组管理器管理（重新可直接被游戏读取）。
 */
import { ref, computed, onMounted, onUnmounted, watch, nextTick } from 'vue';
import { useI18n } from 'vue-i18n';
import { ElMessage, ElMessageBox } from 'element-plus';
import {
  Star,
  Plus,
  Cpu,
  FolderAdd,
  FolderOpened,
  Edit,
  Delete,
  View,
  Close,
  MagicStick,
  Operation,
  Timer
} from '@element-plus/icons-vue';
import { useGame } from '../../../composables/useGame';
import { useSettings } from '../../../composables/useSettings';
import {
  invokeRefreshMods,
  invokeRefreshSingleGroup,
  invokeToggleModDisabled,
  invokeToggleTreeNodeModDisabled,
  invokeAddGroup,
  invokeAddMods,
  invokeRemoveGroup,
  invokeRenameGroup,
  invokeRenameMod,
  invokeOpenPath,
  invokeStartFileWatcher,
  invokeStopFileWatcher,
  invokeSetSelectedMod,
  invokeFindIniFiles,
  invokeProcessIniFiles,
  convertToAssetUrl
} from '../../../utils/invoke';
import { EventNames, eventManager } from '../../../utils/events';
import type { ModData, ModGroupData, LayoutMode, TargetGame } from '../../../types';
import { LayoutMode as LayoutModeEnum } from '../../../types';
import GroupTreeNode from './GroupTreeNode.vue';

const { t } = useI18n();
const game = useGame();
const settings = useSettings();

// ===== 响应式状态 =====
const isLoading = ref(false);                          // 全局加载指示
const showFavoritesOnly = ref(false);                  // 是否仅显示收藏项
const selectedModIndex = ref(0);                       // 当前选中的模组索引（用于高亮与轮播定位）
const dialogAddGroupVisible = ref(false);              // 新建分组对话框可见性
const dialogRenameGroupVisible = ref(false);           // 重命名分组对话框可见性
const dialogRenameModVisible = ref(false);             // 重命名模组对话框可见性
const newGroupName = ref('');                          // 新建分组名称输入
const renameGroupName = ref('');                       // 重命名分组名称输入
const renameModName = ref('');                         // 重命名模组名称输入
const contextMenuVisible = ref(false);                 // 右键菜单可见性
const contextMenuPosition = ref({ x: 0, y: 0 });       // 右键菜单显示坐标
const contextMenuRef = ref<HTMLElement | null>(null);   // 右键菜单 DOM 引用
const contextMenuType = ref<'group' | 'mod' | null>(null); // 右键菜单目标类型
const contextMenuData = ref<ModGroupData | ModData | null>(null); // 右键菜单目标数据
const contextMenuGroupIndex = ref(-1);                 // 右键菜单目标分组索引
const contextMenuModIndex = ref(-1);                   // 右键菜单目标模组索引
// 事件监听取消句柄；组件卸载时需调用以避免内存泄漏
let fileWatcherUnlisten: (() => void) | null = null;
let modsUpdatedUnlisten: (() => void) | null = null;
let gameSwitchedUnlisten: (() => void) | null = null; // 游戏切换事件监听取消句柄
// 文件监听防抖定时器句柄；用于合并短时间内的多次刷新请求
let refreshDebounceTimer: number | null = null;
// 游戏切换防抖定时器句柄；防止快速连续选择游戏导致的频繁请求
let gameSwitchDebounceTimer: ReturnType<typeof setTimeout> | null = null;
const GAME_SWITCH_DEBOUNCE_DELAY = 1000; // 游戏切换防抖延迟（毫秒）

// 拖拽状态
const isDraggingOver = ref(false); // 拖拽悬停状态

// 还原区相关状态
const isRestoreZoneDragging = ref(false); // 还原区拖拽悬停状态
const restoreZoneFiles = ref<Array<{ name: string; path: string }>>([]); // 还原区文件列表
const isProcessingRestore = ref(false); // 还原区处理中状态

// 左侧栏宽度和拖拽调节
const sidebarRef = ref<HTMLElement | null>(null);
const sidebarWidth = ref(240); // 默认宽度 240px（适量增加）
const SIDEBAR_MIN_WIDTH = 160;
const SIDEBAR_MAX_WIDTH = 480;

// 手机风格拖动滚动状态
let scrollDragState: {
  startY: number;
  startScrollTop: number;
  active: boolean;
} | null = null;

// 右侧模组容器拖动滚动状态
const modsContainerRef = ref<HTMLElement | null>(null);
let modsScrollDragState: {
  startY: number;
  startX: number;
  startScrollTop: number;
  startScrollLeft: number;
  active: boolean;
  moved: boolean;
} | null = null;

// 鼠标按下：启动手机风格拖动滚动
function onSidebarMouseDown(e: MouseEvent) {
  if (!sidebarRef.value) return;
  // 排除点击按钮等交互元素
  const target = e.target as HTMLElement;
  if (target.closest('button') || target.closest('.el-button')) return;

  scrollDragState = {
    startY: e.clientY,
    startScrollTop: sidebarRef.value.scrollTop,
    active: true,
  };
  document.addEventListener('mousemove', onSidebarMouseMove);
  document.addEventListener('mouseup', onSidebarMouseUp);
}

function onSidebarMouseMove(e: MouseEvent) {
  if (!scrollDragState?.active || !sidebarRef.value) return;
  const dy = e.clientY - scrollDragState.startY;
  sidebarRef.value.scrollTop = scrollDragState.startScrollTop - dy;
}

function onSidebarMouseUp() {
  scrollDragState = null;
  document.removeEventListener('mousemove', onSidebarMouseMove);
  document.removeEventListener('mouseup', onSidebarMouseUp);
}

// 右侧模组容器：鼠标按下启动拖动滚动
function onModsContainerMouseDown(e: MouseEvent) {
  if (!modsContainerRef.value) return;
  // 排除交互元素：按钮、输入框、模组卡片上的收藏图标等
  const target = e.target as HTMLElement;
  if (target.closest('button') ||
      target.closest('.el-button') ||
      target.closest('.mod-favorite') ||
      target.closest('input') ||
      target.closest('textarea')) return;

  modsScrollDragState = {
    startY: e.clientY,
    startX: e.clientX,
    startScrollTop: modsContainerRef.value.scrollTop,
    startScrollLeft: modsContainerRef.value.scrollLeft,
    active: true,
    moved: false,
  };
  document.addEventListener('mousemove', onModsContainerMouseMove);
  document.addEventListener('mouseup', onModsContainerMouseUp);
}

function onModsContainerMouseMove(e: MouseEvent) {
  if (!modsScrollDragState?.active || !modsContainerRef.value) return;
  const dy = e.clientY - modsScrollDragState.startY;
  const dx = e.clientX - modsScrollDragState.startX;

  // 位移超过 5px 视为拖动
  if (Math.abs(dy) > 5 || Math.abs(dx) > 5) {
    modsScrollDragState.moved = true;
  }

  if (modsScrollDragState.moved) {
    modsContainerRef.value.scrollTop = modsScrollDragState.startScrollTop - dy;
    modsContainerRef.value.scrollLeft = modsScrollDragState.startScrollLeft - dx;
  }
}

function onModsContainerMouseUp() {
  modsScrollDragState = null;
  document.removeEventListener('mousemove', onModsContainerMouseMove);
  document.removeEventListener('mouseup', onModsContainerMouseUp);
}

// 检查模组容器是否处于拖动状态（用于阻止点击事件）
function isModsContainerDragging(): boolean {
  return modsScrollDragState?.moved === true;
}

// 分隔条拖拽调节宽度
function onResizerMouseDown(e: MouseEvent) {
  e.preventDefault();
  const startX = e.clientX;
  const startWidth = sidebarWidth.value;

  const onMove = (ev: MouseEvent) => {
    const dx = ev.clientX - startX;
    sidebarWidth.value = Math.min(
      SIDEBAR_MAX_WIDTH,
      Math.max(SIDEBAR_MIN_WIDTH, startWidth + dx)
    );
  };
  const onUp = () => {
    document.removeEventListener('mousemove', onMove);
    document.removeEventListener('mouseup', onUp);
  };
  document.addEventListener('mousemove', onMove);
  document.addEventListener('mouseup', onUp);
}

// 右键菜单边界检测：确保菜单完全可见
function adjustContextMenuPosition() {
  if (!contextMenuRef.value) return;
  
  const menu = contextMenuRef.value;
  const rect = menu.getBoundingClientRect();
  const windowWidth = window.innerWidth;
  const windowHeight = window.innerHeight;
  
  let { x, y } = contextMenuPosition.value;
  
  // 如果菜单底部超出窗口，向上偏移
  if (rect.bottom > windowHeight) {
    y = windowHeight - rect.height - 8;
  }
  
  // 如果菜单右侧超出窗口，向左偏移
  if (rect.right > windowWidth) {
    x = windowWidth - rect.width - 8;
  }
  
  // 确保不超出左上角
  x = Math.max(8, x);
  y = Math.max(8, y);
  
  contextMenuPosition.value = { x, y };
}

// 实际显示的分组列表：基于排序后的分组，按收藏过滤开关筛选
const displayGroups = computed(() => {
  let groups = game.sortedGroups.value;
  if (showFavoritesOnly.value) {
    // 虚拟分类节点始终保留，避免其下子分组被隐藏
    groups = groups.filter(g => g.isVirtual || g.favoriteDateTime !== null);
  }
  return groups;
});

// 实际显示的模组列表：基于当前分组模组，按收藏过滤开关筛选
const displayMods = computed(() => {
  let mods = game.currentMods.value;
  if (showFavoritesOnly.value) {
    mods = mods.filter(m => m.favoriteDateTime !== null);
  }
  return mods;
});

// 窗口宽度响应式（用于 Auto 布局模式自动切换 Grid/Carousel）
const windowWidth = ref(window.innerWidth);
let resizeHandler: (() => void) | null = null;

// 当前生效的布局模式：Auto 模式下根据窗口宽度自动选择
const effectiveLayoutMode = computed((): LayoutMode => {
  if (settings.layoutMode.value === LayoutModeEnum.Auto) {
    return windowWidth.value < 900 ? LayoutModeEnum.Carousel : LayoutModeEnum.Grid;
  }
  return settings.layoutMode.value;
});

// 是否为网格布局
const isGridLayout = computed(() => {
  return effectiveLayoutMode.value === LayoutModeEnum.Grid;
});

// 是否为轮播布局
const isCarouselLayout = computed(() => {
  return effectiveLayoutMode.value === LayoutModeEnum.Carousel;
});

/**
 * 判断指定分组是否为当前激活的分组（用于高亮显示）。
 * @param group 待判断的分组
 * @returns true 表示该分组为当前选中分组
 */
function isGroupActive(group: ModGroupData): boolean {
  const currentGroup = game.currentGroup.value;
  if (!currentGroup) return false;
  return group.groupPath === currentGroup.groupPath;
}

/**
 * 刷新模组列表（重新读取文件系统）。
 * 与 loadMods 的区别：refreshMods 用于文件变化后的重新扫描，不会重置 modsLoaded 标记。
 */
async function refreshMods() {
  isLoading.value = true;
  try {
    const groups = await invokeRefreshMods(game.targetGame.value);
    game.setModGroups(groups);
  } catch (error) {
    console.error('Failed to refresh mods:', error);
  } finally {
    isLoading.value = false;
  }
}

/**
 * 防抖刷新：合并 500ms 内的多次文件变化事件为一次刷新调用。
 * 业务逻辑：文件监听可能在短时间内触发多次，防抖可避免 UI 卡顿与重复请求。
 */
function debouncedRefresh() {
  if (refreshDebounceTimer) {
    clearTimeout(refreshDebounceTimer);
  }
  refreshDebounceTimer = window.setTimeout(() => {
    refreshMods();
    refreshDebounceTimer = null;
  }, 500);
}

/**
 * 选中指定分组。
 * 业务逻辑：通过 groupPath 定位分组，并更新 currentGroupPath 和 currentGroupIndex。
 * @param group 选中的分组数据
 */
function selectGroup(group: ModGroupData) {
  game.setCurrentGroupByPath(group.groupPath);
  selectedModIndex.value = 0;
}

/**
 * 切换树节点的展开/折叠状态。
 * @param groupPath 分组路径
 */
function toggleExpand(groupPath: string) {
  game.toggleExpandPath(groupPath);
}

/**
 * 选中指定索引的模组（仅更新选中状态，不切换启用状态）。
 * @param index displayMods 中的索引
 */
function selectMod(index: number) {
  if (isModsContainerDragging()) return;
  selectedModIndex.value = index;
}

/**
 * 切换模组的启用/禁用状态。
 * - 普通分组：使用 invokeToggleModDisabled（独立切换）。
 * - 树节点（# 目录）分组：使用 invokeToggleTreeNodeModDisabled（互斥切换，启用时禁用同组其他模组）。
 * @param mod 待切换的模组数据
 * 限制：realIndex === 0 的空槽位不可切换。
 * 注意：通过 modPath 在当前分组中查找真实索引，避免收藏过滤导致索引错位。
 */
async function toggleMod(mod: ModData) {
  if (mod.realIndex === 0) return;
  const currentGroup = game.currentGroup.value;
  const isTreeNode = currentGroup?.isTreeNode ?? false;

  // 通过 modPath 在当前分组中查找真实索引，避免收藏过滤导致索引错位
  const realIndex = currentGroup?.modsInGroup.findIndex(m => m.modPath === mod.modPath) ?? -1;
  if (realIndex === -1) return;

  try {
    if (isTreeNode) {
      // 树节点模式：互斥切换
      const [newPath, newDisabled] = await invokeToggleTreeNodeModDisabled(mod.modPath);
      const updatedMod = { ...mod, modPath: newPath, isDisabled: newDisabled };
      game.updateModInGroup(game.currentGroupPath.value, realIndex, updatedMod);
    } else {
      // 普通模式：独立切换
      const success = await invokeToggleModDisabled(mod.modPath);
      if (success) {
        const updatedMod = { ...mod, isDisabled: !mod.isDisabled };
        game.updateModInGroup(game.currentGroupPath.value, realIndex, updatedMod);
      }
    }
    // 刷新分组信息，确保互斥模式下其他模组状态正确更新
    const updatedGroup = await invokeRefreshSingleGroup(game.currentGroupPath.value);
    game.updateGroup(game.currentGroupPath.value, updatedGroup);
  } catch (error) {
    console.error('Failed to toggle mod:', error);
    ElMessage.error(mod.isDisabled ? t('Failed to enable mod') : t('Failed to disable mod'));
  }
}

/**
 * 切换模组的收藏状态。
 * @param mod 待切换收藏的模组
 * 限制：realIndex === 0 的空槽位不可收藏。
 */
async function toggleModFavorite(mod: ModData) {
  if (mod.realIndex === 0) return;
  await game.toggleModFavorite(mod.modPath);
}

/**
 * 切换分组的收藏状态。
 * @param group 待切换收藏的分组
 */
async function toggleGroupFavorite(group: ModGroupData) {
  await game.toggleGroupFavorite(group.groupPath);
}

// ===== 拖拽添加 Mod =====

/**
 * 拖拽悬停时标记为可放置状态。
 * @param event 拖拽事件
 */
function onDragOver(event: DragEvent) {
  event.preventDefault();
  isDraggingOver.value = true;
}

/**
 * 拖拽离开时清除悬停状态。
 */
function onDragLeave() {
  isDraggingOver.value = false;
}

/**
 * 拖拽释放时获取文件路径并调用后端添加 Mod。
 * 默认添加到当前选中的分组。
 * @param event 拖拽事件
 */
async function onDrop(event: DragEvent) {
  event.preventDefault();
  isDraggingOver.value = false;

  const files = event.dataTransfer?.files;
  if (!files || files.length === 0) return;

  // 获取当前分组路径，如果未选择分组则提示用户
  const currentGroup = game.currentGroup.value;
  if (!currentGroup) {
    ElMessage.warning(t('Please select a group first'));
    return;
  }

  // 提取文件路径列表
  const paths: string[] = [];
  for (let i = 0; i < files.length; i++) {
    const file = files[i] as any;
    if (file.path) {
      paths.push(file.path);
    }
  }

  if (paths.length === 0) {
    ElMessage.warning(t('No valid paths found'));
    return;
  }

  try {
    await invokeAddMods(paths, currentGroup.groupPath);
    await refreshMods();
    ElMessage.success(t('Mods added successfully'));
  } catch (error) {
    console.error('Failed to add mods:', error);
    ElMessage.error(t('Failed to add mods'));
  }
}

function onRestoreZoneDragOver(event: DragEvent) {
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = 'copy';
  }
  isRestoreZoneDragging.value = true;
}

function onRestoreZoneDragLeave() {
  isRestoreZoneDragging.value = false;
}

async function onRestoreZoneDrop(event: DragEvent) {
  isRestoreZoneDragging.value = false;
  const files = event.dataTransfer?.files;
  if (!files || files.length === 0) return;

  const validFiles: Array<{ name: string; path: string }> = [];

  for (let i = 0; i < files.length; i++) {
    const file = files[i] as any;
    if (file.path) {
      const isValid = validatePath(file.path);
      if (isValid) {
        const isIniFile = file.name.toLowerCase().endsWith('.ini');
        if (isIniFile) {
          validFiles.push({ name: file.name, path: file.path });
        } else {
          const iniFiles = await findIniFilesInPath(file.path);
          validFiles.push(...iniFiles);
        }
      }
    }
  }

  if (validFiles.length === 0) {
    ElMessage.warning(t('No valid .ini files found'));
    return;
  }

  restoreZoneFiles.value = [...restoreZoneFiles.value, ...validFiles];
  ElMessage.success(t(`${validFiles.length} file(s) added to restore zone`));
}

function validatePath(path: string): boolean {
  if (!path || path.trim() === '') {
    return false;
  }
  const normalizedPath = path.replace(/\\/g, '/');
  if (normalizedPath.length > 32000) {
    return false;
  }
  if (normalizedPath.includes('..')) {
    return false;
  }
  return true;
}

async function findIniFilesInPath(path: string): Promise<Array<{ name: string; path: string }>> {
  try {
    const result = await invokeFindIniFiles(path);
    return result.map((p: string) => ({
      name: p.split(/[\\/]/).pop() || '',
      path: p
    }));
  } catch (error) {
    console.error('Failed to find ini files:', error);
    return [];
  }
}

function removeRestoreZoneFile(index: number) {
  restoreZoneFiles.value.splice(index, 1);
}

function clearRestoreZone() {
  restoreZoneFiles.value = [];
}

async function processRestoreZoneFiles() {
  if (restoreZoneFiles.value.length === 0) return;

  isProcessingRestore.value = true;
  try {
    const paths = restoreZoneFiles.value.map(f => f.path);
    const result = await invokeProcessIniFiles(paths);
    if (result) {
      ElMessage.success(t('Files processed successfully'));
      clearRestoreZone();
    } else {
      ElMessage.error(t('Failed to process files'));
    }
  } catch (error) {
    console.error('Failed to process ini files:', error);
    ElMessage.error(t('Failed to process files'));
  } finally {
    isProcessingRestore.value = false;
  }
}

/** 打开新建分组对话框，并清空输入框 */
function showAddGroupDialog() {
  newGroupName.value = '';
  dialogAddGroupVisible.value = true;
}

/**
 * 处理新建分组确认。
 * 业务逻辑：
 *  - 校验分组名非空。
 *  - 调用后端创建分组，成功后刷新列表并关闭对话框。
 */
async function handleAddGroup() {
  if (!newGroupName.value.trim()) {
    ElMessage.warning(t('Group name cannot be empty'));
    return;
  }
  try {
    await invokeAddGroup(newGroupName.value.trim());
    await refreshMods();
    dialogAddGroupVisible.value = false;
    ElMessage.success(t('Group added successfully'));
  } catch (error) {
    console.error('Failed to add group:', error);
    ElMessage.error(t('Failed to add group'));
  }
}

/**
 * 打开重命名分组对话框，预填当前分组名。
 * @param group 待重命名的分组
 */
function showRenameGroupDialog(group: ModGroupData) {
  renameGroupName.value = group.groupName;
  dialogRenameGroupVisible.value = true;
}

/**
 * 处理重命名分组确认。
 * 业务逻辑：对当前选中的分组调用后端重命名接口，成功后刷新列表。
 */
async function handleRenameGroup() {
  if (!renameGroupName.value.trim()) {
    ElMessage.warning(t('Group name cannot be empty'));
    return;
  }
  const group = game.currentGroup.value;
  if (!group) return;
  try {
    await invokeRenameGroup(group.groupPath, renameGroupName.value.trim());
    await refreshMods();
    dialogRenameGroupVisible.value = false;
    ElMessage.success(t('Group renamed successfully'));
  } catch (error) {
    console.error('Failed to rename group:', error);
    ElMessage.error(t('Failed to rename group'));
  }
}

/**
 * 处理模组重命名确认。
 * 业务逻辑：
 *  - 验证输入名称不为空
 *  - 获取当前右键菜单选中的模组数据
 *  - 调用后端命令重命名模组（禁用状态会自动保留 DISABLED 前缀）
 *  - 成功后刷新模组列表并关闭对话框
 */
async function handleRenameMod() {
  if (!renameModName.value.trim()) {
    ElMessage.warning(t('Mod name cannot be empty'));
    return;
  }
  const mod = contextMenuData.value as ModData;
  if (!mod) return;
  try {
    await invokeRenameMod(mod.modPath, renameModName.value.trim());
    await refreshMods();
    dialogRenameModVisible.value = false;
    ElMessage.success(t('Mod renamed successfully'));
  } catch (error) {
    console.error('Failed to rename mod:', error);
    ElMessage.error(t('Failed to rename mod'));
  }
}

/**
 * 删除当前选中的分组。
 * 业务逻辑：
 *  - 弹窗二次确认，提示用户删除后内部模组将脱离管理器。
 *  - 确认后调用后端删除并刷新列表。
 *  - 用户点击取消时静默忽略（error === 'cancel'）。
 */
async function handleRemoveGroup() {
  const group = game.currentGroup.value;
  if (!group) return;
  try {
    await ElMessageBox.confirm(
      t('Removing group will make the mods inside the group can be used again without mod manager.'),
      t('Warning'),
      {
        confirmButtonText: t('Confirm'),
        cancelButtonText: t('Cancel'),
        type: 'warning'
      }
    );
    await invokeRemoveGroup(group.groupPath);
    await refreshMods();
    ElMessage.success(t('Group removed successfully'));
  } catch (error) {
    if (error !== 'cancel') {
      console.error('Failed to remove group:', error);
      ElMessage.error(t('Failed to remove group'));
    }
  }
}

/**
 * 在系统文件管理器中打开模组所在路径。
 * @param mod 目标模组
 * 限制：realIndex === 0 的空槽位不可打开。
 */
function openModFolder(mod: ModData) {
  if (mod.realIndex === 0) return;
  invokeOpenPath(mod.modPath).catch(console.error);
}

/**
 * 在系统文件管理器中打开分组所在路径。
 * @param group 目标分组
 */
function openGroupFolder(group: ModGroupData) {
  invokeOpenPath(group.groupPath).catch(console.error);
}

/**
 * 显示分组右键菜单。
 * @param event 鼠标事件，用于定位菜单坐标
 * @param group 目标分组数据
 */
function showGroupContextMenu(event: MouseEvent, group: ModGroupData) {
  // 虚拟分类节点不支持右键菜单
  if (group.isVirtual) return;
  event.preventDefault();
  contextMenuPosition.value = { x: event.clientX, y: event.clientY };
  contextMenuType.value = 'group';
  contextMenuData.value = group;
  // 通过路径查找索引
  contextMenuGroupIndex.value = game.modGroups.value.findIndex(g => g.groupPath === group.groupPath);
  contextMenuVisible.value = true;
  // 边界检测：确保菜单完全可见
  nextTick(adjustContextMenuPosition);
}

/**
 * 显示模组右键菜单。
 * @param event 鼠标事件，用于定位菜单坐标
 * @param mod 目标模组数据
 * 限制：realIndex === 0 的空槽位不显示菜单。
 */
function showModContextMenu(event: MouseEvent, mod: ModData) {
  event.preventDefault();
  if (mod.realIndex === 0) return;
  contextMenuPosition.value = { x: event.clientX, y: event.clientY };
  contextMenuType.value = 'mod';
  contextMenuData.value = mod;
  contextMenuVisible.value = true;
  // 边界检测：确保菜单完全可见
  nextTick(adjustContextMenuPosition);
}

/** 隐藏右键菜单并清空相关状态 */
function hideContextMenu() {
  contextMenuVisible.value = false;
  contextMenuType.value = null;
  contextMenuData.value = null;
  contextMenuGroupIndex.value = -1;
  contextMenuModIndex.value = -1;
}

/**
 * 处理右键菜单项点击。
 * @param command 菜单命令标识：rename / favorite / open / delete / toggle
 * 业务逻辑：根据 contextMenuType 分发到 group 或 mod 的对应处理函数，最后统一隐藏菜单。
 */
async function handleContextMenuSelect(command: string) {
  if (contextMenuType.value === 'group') {
    const group = contextMenuData.value as ModGroupData;
    switch (command) {
      case 'rename':
        showRenameGroupDialog(group);
        break;
      case 'favorite':
        toggleGroupFavorite(group);
        break;
      case 'open':
        openGroupFolder(group);
        break;
      case 'delete':
        handleRemoveGroup();
        break;
    }
  } else if (contextMenuType.value === 'mod') {
    const mod = contextMenuData.value as ModData;
    switch (command) {
      case 'select':
        selectModInGroup(mod);
        break;
      case 'toggle':
        toggleMod(mod);
        break;
      case 'rename':
        renameModName.value = mod.modName;
        dialogRenameModVisible.value = true;
        break;
      case 'favorite':
        toggleModFavorite(mod);
        break;
      case 'open':
        openModFolder(mod);
        break;
      case 'remove':
        await removeModFromGroup(mod);
        break;
    }
  }
  hideContextMenu();
}

async function selectModInGroup(mod: ModData) {
  const realIndex = game.currentGroup.value?.modsInGroup.findIndex(m => m.modPath === mod.modPath) ?? -1;
  if (realIndex >= 0) {
    selectedModIndex.value = realIndex;
    try {
      await invokeSetSelectedMod(game.currentGroupPath.value, realIndex);
    } catch (error) {
      console.error('Failed to select mod:', error);
    }
  }
}

async function removeModFromGroup(mod: ModData) {
  if (mod.realIndex === 0) return;
  try {
    await ElMessageBox.confirm(
      t('Removing mod will move it to restore zone.'),
      t('Warning'),
      {
        confirmButtonText: t('Confirm'),
        cancelButtonText: t('Cancel'),
        type: 'warning'
      }
    );
    const realIndex = game.currentGroup.value?.modsInGroup.findIndex(m => m.modPath === mod.modPath) ?? -1;
    if (realIndex >= 0) {
      game.removeModFromGroup(game.currentGroupPath.value, realIndex);
      await refreshMods();
      ElMessage.success(t('Mod removed successfully'));
    }
  } catch (error) {
    if (error !== 'cancel') {
      console.error('Failed to remove mod:', error);
      ElMessage.error(t('Failed to remove mod'));
    }
  }
}

/**
 * 模组双击回调。
 * 业务逻辑：
 *  - 空槽位双击仅选中，不切换状态。
 *  - 普通模组双击切换启用/禁用状态。
 * @param mod 目标模组
 * @param index 目标模组在 displayMods 中的索引（仅用于空槽位选中高亮）
 */
function onModDoubleClick(mod: ModData, index: number) {
  if (mod.realIndex === 0) {
    selectMod(index);
    return;
  }
  toggleMod(mod);
}

/**
 * 启动后端文件监听器，监听 Mods 目录变化。
 * 业务逻辑：先停止旧监听避免泄漏；仅当存在 modsPath 时启动新监听；失败时记录错误但不阻塞。
 */
async function setupFileWatcher() {
  try {
    // 先停止旧的文件监听器，避免泄漏
    try {
      await invokeStopFileWatcher();
    } catch {
      // 忽略停止失败（可能没有正在运行的监听器）
    }
    if (game.modsPath.value) {
      await invokeStartFileWatcher(game.modsPath.value);
    }
  } catch (error) {
    console.error('[ModsTab] Failed to setup file watcher:', error);
  }
}

/**
 * 注册前端事件监听：
 *  - FILE_WATCHER_EVENT：文件变化时触发防抖刷新。
 *  - MODS_UPDATED：后端通知模组更新时同步到 gameStore。
 *  - GAME_SWITCHED：游戏切换时重新加载模组列表并重启文件监听。
 * 返回值：保存取消函数以便组件卸载时清理。
 */
async function setupEventListeners() {
  fileWatcherUnlisten = await eventManager.on(EventNames.FILE_WATCHER_EVENT, () => {
    debouncedRefresh();
  });

  modsUpdatedUnlisten = await eventManager.on(EventNames.MODS_UPDATED, (groups) => {
    game.setModGroups(groups);
  });

  // 游戏切换时：重新加载模组列表、重启文件监听（因 modsPath 可能变化）
  // 使用事件载荷中的 game 参数而非 game.targetGame.value，确保获取最新值
  // 避免 Vue 响应式更新延迟导致读取到旧值（如 'none'）
  // 添加防抖机制，防止用户快速连续选择游戏导致的频繁请求
  gameSwitchedUnlisten = await eventManager.on(EventNames.GAME_SWITCHED, async ({ game: newGame }) => {
    console.log('[ModsTab] GAME_SWITCHED event received, newGame:', newGame);
    if (gameSwitchDebounceTimer) {
      clearTimeout(gameSwitchDebounceTimer);
    }
    gameSwitchDebounceTimer = setTimeout(async () => {
      console.log('[ModsTab] GAME_SWITCHED debounce complete, loading mods for game:', newGame);
      await game.loadModsForGame(newGame as TargetGame);
      await setupFileWatcher();
    }, GAME_SWITCH_DEBOUNCE_DELAY);
  });
}

// 组件挂载：依次加载模组、注册事件监听、启动文件监听；并绑定全局点击事件用于关闭右键菜单
onMounted(async () => {
  resizeHandler = () => { windowWidth.value = window.innerWidth; };
  window.addEventListener('resize', resizeHandler);
  // 若已有当前游戏的模组数据，则跳过加载（避免切页重复读取）
  // 使用 game.loadModsForGame() 统一加载入口，利用 store 中的缓存逻辑
  if (game.isModsLoaded.value && game.targetGame.value !== 'none') {
    // 已有数据，仅启动事件监听和文件监听
  } else {
    await game.loadModsForGame(game.targetGame.value);
  }
  await setupEventListeners();
  await setupFileWatcher();
  document.addEventListener('click', hideContextMenu);
});

// 组件卸载：取消事件监听、停止文件监听、移除全局点击监听、清理防抖定时器
onUnmounted(() => {
  if (fileWatcherUnlisten) {
    fileWatcherUnlisten();
  }
  if (modsUpdatedUnlisten) {
    modsUpdatedUnlisten();
  }
  if (gameSwitchedUnlisten) {
    gameSwitchedUnlisten();
  }
  invokeStopFileWatcher().catch(console.error);
  document.removeEventListener('click', hideContextMenu);
  if (resizeHandler) {
    window.removeEventListener('resize', resizeHandler);
    resizeHandler = null;
  }
  if (refreshDebounceTimer) {
    clearTimeout(refreshDebounceTimer);
  }
  if (gameSwitchDebounceTimer) {
    clearTimeout(gameSwitchDebounceTimer);
  }
  // 清理拖动滚动事件
  document.removeEventListener('mousemove', onSidebarMouseMove);
  document.removeEventListener('mouseup', onSidebarMouseUp);
  document.removeEventListener('mousemove', onModsContainerMouseMove);
  document.removeEventListener('mouseup', onModsContainerMouseUp);
});

// 监听布局模式变化（占位 watcher，预留用于未来扩展，如布局切换动画等）
watch(
  () => settings.layoutMode.value,
  () => {
  }
);

// 监听设置加载完成：如果设置加载后 targetGame 变为有效值且模组未加载，触发加载
// 使用 isInitialModsLoad 标志防止与 onMounted 中的加载逻辑双重触发
let isInitialModsLoad = false;
watch(
  () => settings.isLoaded.value,
  (loaded) => {
    if (loaded && game.targetGame.value !== 'none' && !game.isModsLoaded.value && !isInitialModsLoad) {
      isInitialModsLoad = true;
      console.log('[ModsTab] Settings loaded, triggering mods load for game:', game.targetGame.value);
      game.loadModsForGame(game.targetGame.value);
    }
  }
);

// 监听当前分组变化，打印调试信息
watch(
  () => game.currentGroup.value,
  (newGroup) => {
    console.log('[ModsTab] currentGroup changed:', {
      groupName: newGroup?.groupName || null,
      groupPath: newGroup?.groupPath || null,
      modsCount: newGroup?.modsInGroup.length || 0,
      mods: newGroup?.modsInGroup.map(m => ({ name: m.modName, path: m.modPath })) || []
    });
  },
  { immediate: true }
);
</script>

<template>
  <!-- 
    模组管理标签页根容器
    作用：承载整个模组管理界面的布局和交互
    交互行为：@click.self 点击空白区域时隐藏右键菜单
  -->
  <div class="mods-tab" @click.self="hideContextMenu">

    <!-- 
      主体内容区域：左侧分组列表 + 右侧模组展示
      布局：flex 水平布局，左侧固定宽度 200px，右侧自适应
    -->
    <div class="mods-content">
      <!-- 
        分组侧边栏
        数据来源：displayGroups (经过排序和收藏过滤后的分组列表), isLoading (加载状态)
        交互行为：点击分组切换选中，右键打开分组菜单，点击收藏图标切换收藏状态
      -->
      <div
        class="groups-sidebar"
        ref="sidebarRef"
        :style="{ width: sidebarWidth + 'px' }"
        @mousedown="onSidebarMouseDown"
      >
        <!-- 
          分组区域头部
          作用：显示"Groups"标题和添加分组快捷按钮
        -->
        <div class="groups-reserved">
          <div class="reserved-header">
            <!-- 分组标题 -->
            <span class="reserved-title">{{ t('Groups') }}</span>
            <!-- 添加分组快捷按钮 -->
            <el-button size="small" circle @click="showAddGroupDialog">
              <el-icon>
                <Plus />
              </el-icon>
            </el-button>
          </div>
        </div>

        <!--
          分组列表容器（树形结构）
          数据来源：displayGroups 提供分组数据
          加载状态：v-loading 在 isLoading 为 true 时显示加载动画
          交互行为：每个分组项支持点击选中、右键菜单、展开/折叠
        -->
        <div v-loading="isLoading" class="groups-list">
          <!--
            树形分组节点
            数据来源：v-for 遍历 displayGroups（顶层分组），递归渲染子节点
            动态绑定：
              - :group 传递分组数据
              - :depth 设置深度为 0（顶层节点）
              - :is-active 根据 groupPath 判断是否为当前选中分组
              - :is-expanded 根据 expandedPaths 判断是否展开
              - :expanded-paths 传递展开状态集合
            交互行为：
              - @select 调用 selectGroup 切换选中分组
              - @contextmenu 调用 showGroupContextMenu 打开右键菜单
              - @toggle-expand 调用 toggleExpand 切换展开/折叠
          -->
          <GroupTreeNode
            v-for="group in displayGroups"
            :key="group.groupPath"
            :group="group"
            :depth="0"
            :is-active="isGroupActive(group)"
            :is-expanded="game.expandedPaths.value.has(group.groupPath)"
            :expanded-paths="game.expandedPaths.value"
            :current-group-path="game.currentGroupPath.value"
            @select="selectGroup"
            @contextmenu="showGroupContextMenu"
            @toggle-expand="toggleExpand"
          />
        </div>
        <!-- 
          空状态提示
          条件渲染：v-if 在 !isLoading && displayGroups.length === 0 时显示
          数据来源：isLoading (加载状态), displayGroups.length (分组数量)
          作用：当没有分组时提示用户添加分组
        -->
        <el-empty v-if="!isLoading && displayGroups.length === 0"
          :description="t('Right-click and add group, then you can add mods.')" :image-size="80" />
      </div>
      <!-- 拖拽分隔条：调节左侧栏宽度 -->
      <div
        class="sidebar-resizer"
        @mousedown="onResizerMouseDown"
      ></div>

      <!-- 
        模组展示区域
        数据来源：displayMods (经过排序和收藏过滤后的模组列表), isLoading (加载状态)
        布局：根据 effectiveLayoutMode 切换网格布局或轮播布局
        交互行为：点击选中模组，双击切换启用/禁用，右键打开模组菜单
        拖拽行为：@dragover/@dragleave/@drop 处理拖拽文件/目录添加 Mod
      -->
      <div class="mods-display" :class="{ 'drag-over': isDraggingOver }" @dragover="onDragOver" @dragleave="onDragLeave"
        @drop="onDrop">
        <!-- 
          模组容器
          加载状态：v-loading 在 isLoading 为 true 时显示加载动画
        -->
        <div v-loading="isLoading" class="mods-container" ref="modsContainerRef" @mousedown="onModsContainerMouseDown">
          <!-- 
            网格布局模式
            条件渲染：v-if="isGridLayout" 当布局模式为 Grid 时显示
            布局方式：CSS Grid 自适应列数，每列最小 140px
          -->
          <template v-if="isGridLayout">
            <!-- 
              模组网格容器
              布局：grid 布局，自动填充列
            -->
            <div class="mods-grid">
              <!-- 
                模组卡片
                数据来源：v-for 遍历 displayMods，key 使用 modPath 保证唯一性
                动态绑定：
                  - :class 根据 selectedModIndex 添加 selected 类（高亮选中模组）
                  - :class 根据 mod.isDisabled 添加 disabled 类（禁用状态半透明）
                  - :class 根据 mod.realIndex === 0 添加 none-slot 类（空槽位特殊样式）
                交互行为：
                  - @click 调用 selectMod(index) 选中模组
                  - @dblclick 调用 onModDoubleClick(mod, index) 切换启用/禁用
                  - @contextmenu 调用 showModContextMenu 打开右键菜单
              -->
              <div v-for="(mod, index) in displayMods" :key="mod.modPath" class="mod-card" :class="{
                selected: selectedModIndex === index,
                disabled: mod.isDisabled,
                'none-slot': mod.realIndex === 0
              }" @click="selectMod(index)" @dblclick="onModDoubleClick(mod, index)"
                @contextmenu="showModContextMenu($event, mod)">
                <!-- 
                  模组图标区域
                  数据来源：
                    - mod.iconPath 存在时显示自定义图标
                    - mod.realIndex === 0 时显示关闭图标（空槽位）
                    - 其他情况显示查看图标
                  条件渲染：v-if/v-else-if/v-else 三级条件判断
                -->
                <div class="mod-icon">
                  <!-- 自定义模组图标 -->
                  <img 
                    v-if="mod.iconPath" 
                    :src="convertToAssetUrl(mod.iconPath)" 
                    alt="mod icon" 
                    loading="lazy"
                    @error="(e: Event) => (e.target as HTMLImageElement).style.display = 'none'"
                  />
                  <!-- 空槽位图标 -->
                  <el-icon v-else-if="mod.realIndex === 0">
                    <Close />
                  </el-icon>
                  <!-- 默认查看图标 -->
                  <el-icon v-else>
                    <View />
                  </el-icon>
                </div>
                <!-- 
                  模组名称
                  数据来源：mod.modName
                  样式：最多显示 2 行，超出显示省略号
                -->
                <div class="mod-name">{{ mod.modName }}</div>
                <!-- 
                  模组状态图标区域
                  数据来源：mod 的各种状态标志位
                  作用：显示模组的特殊状态（自动修复、未优化、命名空间等）
                -->
                <div class="mod-status-icons">
                  <!-- 
                    旧版本自动修复图标
                    条件渲染：v-if="mod.isOldAutoFixed"
                    提示内容：显示 tooltip 说明"旧版本 NRMM 自动修复了语法错误"
                    样式：warning 类，橙色图标
                  -->
                  <el-tooltip v-if="mod.isOldAutoFixed"
                    :content="t('Mod syntax errors were auto-fixed by earlier NRMM versions.')">
                    <el-icon class="status-icon warning">
                      <MagicStick />
                    </el-icon>
                  </el-tooltip>
                  <!-- 
                    语法错误已移除图标
                    条件渲染：v-if="mod.isSyntaxErrorRemoved"
                    提示内容：显示 tooltip 说明"语法错误已被自动移除"
                    样式：info 类，蓝色图标
                  -->
                  <el-tooltip v-if="mod.isSyntaxErrorRemoved"
                    :content="t('Mod syntax errors are automatically removed.')">
                    <el-icon class="status-icon info">
                      <Operation />
                    </el-icon>
                  </el-tooltip>
                  <!-- 
                    未优化模组图标
                    条件渲染：v-if="mod.isUnoptimized"
                    提示内容：显示 tooltip 说明"模组未优化，可能影响性能或破坏其他模组"
                    样式：warning 类，橙色图标
                  -->
                  <el-tooltip v-if="mod.isUnoptimized"
                    :content="t('Mod is unoptimized and might slow down performance or even break other mods.')">
                    <el-icon class="status-icon warning">
                      <Timer />
                    </el-icon>
                  </el-tooltip>
                  <!-- 
                    命名空间模组图标
                    条件渲染：v-if="mod.isNamespaced"
                    提示内容：显示 tooltip 说明"模组使用命名空间"
                    样式：info 类，蓝色图标
                  -->
                  <el-tooltip v-if="mod.isNamespaced" :content="t('Mod uses namespaces')">
                    <el-icon class="status-icon info">
                      <Cpu />
                    </el-icon>
                  </el-tooltip>
                </div>
                <!-- 
                  模组收藏图标
                  数据来源：mod.favoriteDateTime 存在且 mod.realIndex !== 0 时显示
                  条件渲染：v-if 根据收藏状态和是否为空槽位决定是否显示
                  交互行为：@click.stop 阻止事件冒泡，调用 toggleModFavorite(mod) 切换收藏状态
                  样式：金色图标 (#f59e0b)，绝对定位在右上角
                -->
                <el-icon v-if="mod.favoriteDateTime && mod.realIndex !== 0" class="mod-favorite" color="#f59e0b"
                  @click.stop="toggleModFavorite(mod)">
                  <Star />
                </el-icon>
                <!-- 
                  禁用状态徽章
                  数据来源：mod.isDisabled
                  条件渲染：v-if 根据禁用状态决定是否显示
                  样式：绝对定位在左上角，红色背景
                -->
                <div v-if="mod.isDisabled" class="mod-disabled-badge">
                  {{ t('Disabled') }}
                </div>
              </div>
            </div>
          </template>

          <!--
            行列布局模式（原轮播布局）
            条件渲染：v-else-if="isCarouselLayout" 当布局模式为 List 时显示
            布局方式：flex-wrap 横向排列多行，整体纵向滚动（Android 风格）
          -->
          <template v-else-if="isCarouselLayout">
            <!--
              行列布局容器
              数据来源：displayMods 遍历渲染卡片
              布局：flex-wrap，卡片固定宽度，纵向滚动
            -->
            <div class="mods-list" v-if="displayMods.length > 0">
              <div v-for="(mod, index) in displayMods" :key="mod.modPath" class="list-mod-card" :class="{
                selected: selectedModIndex === index,
                disabled: mod.isDisabled,
                'none-slot': mod.realIndex === 0
              }" @click="selectMod(index)" @dblclick="onModDoubleClick(mod, index)"
                @contextmenu="showModContextMenu($event, mod)">
                <!-- 模组图标 -->
                <div class="list-mod-icon">
                  <img 
                    v-if="mod.iconPath" 
                    :src="convertToAssetUrl(mod.iconPath)" 
                    alt="mod icon" 
                    loading="lazy"
                    @error="(e: Event) => (e.target as HTMLImageElement).style.display = 'none'"
                  />
                  <el-icon v-else-if="mod.realIndex === 0">
                    <Close />
                  </el-icon>
                  <el-icon v-else>
                    <View />
                  </el-icon>
                </div>
                <!-- 模组名称 -->
                <div class="list-mod-name">{{ mod.modName }}</div>
                <!-- 状态图标 -->
                <div class="list-mod-status-icons">
                  <el-tooltip v-if="mod.isOldAutoFixed"
                    :content="t('Mod syntax errors were auto-fixed by earlier NRMM versions.')">
                    <el-icon class="status-icon warning">
                      <MagicStick />
                    </el-icon>
                  </el-tooltip>
                  <el-tooltip v-if="mod.isSyntaxErrorRemoved"
                    :content="t('Mod syntax errors are automatically removed.')">
                    <el-icon class="status-icon info">
                      <Operation />
                    </el-icon>
                  </el-tooltip>
                  <el-tooltip v-if="mod.isUnoptimized"
                    :content="t('Mod is unoptimized and might slow down performance or even break other mods.')">
                    <el-icon class="status-icon warning">
                      <Timer />
                    </el-icon>
                  </el-tooltip>
                  <el-tooltip v-if="mod.isNamespaced" :content="t('Mod uses namespaces')">
                    <el-icon class="status-icon info">
                      <Cpu />
                    </el-icon>
                  </el-tooltip>
                </div>
                <!-- 收藏图标 -->
                <el-icon v-if="mod.favoriteDateTime && mod.realIndex !== 0" class="mod-favorite" color="#f59e0b"
                  @click.stop="toggleModFavorite(mod)">
                  <Star />
                </el-icon>
              </div>
            </div>
          </template>

          <!-- 
            空状态提示
            条件渲染：v-if 在 !isLoading && displayMods.length === 0 时显示
            数据来源：isLoading (加载状态), displayMods.length (模组数量)
            作用：当没有模组时提示用户
          -->
          <el-empty v-if="!isLoading && displayMods.length === 0" :description="t('No mods found')" :image-size="100" />
        </div>

        <!-- 
          还原区功能模块
          作用：允许用户将文件或目录拖拽至此区域进行处理
          功能：仅处理.ini文件类型，移除xxmi专属ini语句
          交互行为：@dragover/@dragleave/@drop 处理拖拽事件
        -->
        <div class="restore-zone" :class="{ 'drag-over': isRestoreZoneDragging }"
          @dragover.prevent="onRestoreZoneDragOver"
          @dragleave="onRestoreZoneDragLeave"
          @drop.prevent="onRestoreZoneDrop">
          <div class="restore-zone-header">
            <el-icon class="restore-zone-icon">
              <FolderOpened />
            </el-icon>
            <span class="restore-zone-title">{{ t('Restore Zone') }}</span>
            <span class="restore-zone-hint">{{ t('Drop .ini files here') }}</span>
          </div>
          <div class="restore-zone-content">
            <div v-if="!isRestoreZoneDragging && restoreZoneFiles.length === 0" class="restore-zone-empty">
              <el-icon size="48" color="rgba(255,255,255,0.3)">
                <FolderAdd />
              </el-icon>
              <p>{{ t('Drag files or directories here') }}</p>
              <p class="restore-zone-sub">{{ t('Only .ini files will be processed') }}</p>
            </div>
            <div v-else-if="restoreZoneFiles.length > 0" class="restore-zone-files">
              <div v-for="(file, index) in restoreZoneFiles" :key="index" class="restore-zone-file-item">
                <el-icon size="16" color="#f59e0b">
                  <Cpu />
                </el-icon>
                <span class="restore-zone-file-name">{{ file.name }}</span>
                <span class="restore-zone-file-path">{{ file.path }}</span>
                <el-button size="small" type="danger" @click="removeRestoreZoneFile(index)">
                  <el-icon>
                    <Delete />
                  </el-icon>
                </el-button>
              </div>
              <div class="restore-zone-actions">
                <el-button type="primary" @click="processRestoreZoneFiles" :loading="isProcessingRestore">
                  {{ t('Process') }}
                </el-button>
                <el-button @click="clearRestoreZone">{{ t('Clear') }}</el-button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 
      右键上下文菜单
      数据来源：
        - contextMenuVisible 控制菜单显示/隐藏
        - contextMenuPosition 控制菜单位置（绝对定位）
        - contextMenuType 区分是分组菜单还是模组菜单
        - contextMenuData 存储右键点击的目标数据
      条件渲染：v-if="contextMenuVisible" 仅在可见时渲染
      交互行为：@click.stop 阻止事件冒泡，避免触发外层点击隐藏
      样式：固定定位，z-index 9999 确保在最上层
    -->
    <div v-if="contextMenuVisible" class="context-menu"
      ref="contextMenuRef"
      :style="{ left: contextMenuPosition.x + 'px', top: contextMenuPosition.y + 'px' }" @click.stop>
      <!-- 
        分组右键菜单
        条件渲染：v-if="contextMenuType === 'group'" 当右键点击分组时显示
        数据来源：contextMenuData 为 ModGroupData 类型
        交互行为：每个菜单项点击后调用 handleContextMenuSelect 并传入命令标识
      -->
      <template v-if="contextMenuType === 'group'">
        <!-- 
          添加分组菜单项
          交互行为：@click 调用 showAddGroupDialog 打开新建分组对话框
          图标：FolderAdd 图标
        -->
        <div class="context-menu-item" @click="showAddGroupDialog">
          <el-icon>
            <FolderAdd />
          </el-icon>
          {{ t('Add group') }}
        </div>
        <!-- 分隔线 -->
        <div class="context-menu-separator" />
        <!-- 
          重命名菜单项
          交互行为：@click 调用 handleContextMenuSelect('rename')
          图标：Edit 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('rename')">
          <el-icon>
            <Edit />
          </el-icon>
          {{ t('Rename') }}
        </div>
        <!-- 
          收藏/取消收藏菜单项
          交互行为：@click 调用 handleContextMenuSelect('favorite')
          动态文本：根据 favoriteDateTime 是否存在显示"Favorite"或"Unfavorite"
          图标：Star 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('favorite')">
          <el-icon>
            <Star />
          </el-icon>
          {{ (contextMenuData as ModGroupData)?.favoriteDateTime ? t('Unfavorite') : t('Favorite') }}
        </div>
        <!-- 
          在文件管理器中打开菜单项
          交互行为：@click 调用 handleContextMenuSelect('open')
          图标：FolderOpened 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('open')">
          <el-icon>
            <FolderOpened />
          </el-icon>
          {{ t('Open in File Explorer') }}
        </div>
        <!-- 
          删除分组菜单项
          交互行为：@click 调用 handleContextMenuSelect('delete')
          样式：danger 类，红色文字，表示危险操作
          图标：Delete 图标
        -->
        <div class="context-menu-item danger" @click="handleContextMenuSelect('delete')">
          <el-icon>
            <Delete />
          </el-icon>
          {{ t('Remove group') }}
        </div>
      </template>
      <!-- 
        模组右键菜单
        条件渲染：v-else-if="contextMenuType === 'mod'" 当右键点击模组时显示
        数据来源：contextMenuData 为 ModData 类型
        交互行为：每个菜单项点击后调用 handleContextMenuSelect 并传入命令标识
      -->
      <template v-else-if="contextMenuType === 'mod'">
        <!-- 
          选择菜单项（顶层）
          交互行为：@click 调用 handleContextMenuSelect('select')
          图标：View 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('select')">
          <el-icon>
            <View />
          </el-icon>
          {{ t('Select') }}
        </div>
        <!-- 分隔线 -->
        <div class="context-menu-separator" />
        <!-- 
          启用/禁用模组菜单项
          交互行为：@click 调用 handleContextMenuSelect('toggle')
          动态文本：根据 mod.isDisabled 显示"Enable mod"或"Disable mod completely"
          图标：View 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('toggle')">
          <el-icon>
            <View />
          </el-icon>
          {{ (contextMenuData as ModData)?.isDisabled ? t('Enable mod') : t('Disable mod completely') }}
        </div>
        <!-- 
          收藏/取消收藏菜单项
          交互行为：@click 调用 handleContextMenuSelect('favorite')
          动态文本：根据 favoriteDateTime 是否存在显示"Favorite"或"Unfavorite"
          图标：Star 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('favorite')">
          <el-icon>
            <Star />
          </el-icon>
          {{ (contextMenuData as ModData)?.favoriteDateTime ? t('Unfavorite') : t('Favorite') }}
        </div>
        <!-- 
          重命名菜单项
          交互行为：@click 调用 handleContextMenuSelect('rename')
          图标：Edit 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('rename')">
          <el-icon>
            <Edit />
          </el-icon>
          {{ t('Rename') }}
        </div>
        <!-- 
          在文件管理器中打开菜单项
          交互行为：@click 调用 handleContextMenuSelect('open')
          图标：FolderOpened 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('open')">
          <el-icon>
            <FolderOpened />
          </el-icon>
          {{ t('Open in File Explorer') }}
        </div>
        <!-- 分隔线 -->
        <div class="context-menu-separator" />
        <!-- 
          移除菜单项（底层）
          交互行为：@click 调用 handleContextMenuSelect('remove')
          样式：danger 类，红色文字，表示危险操作
          图标：Delete 图标
        -->
        <div class="context-menu-item danger" @click="handleContextMenuSelect('remove')">
          <el-icon>
            <Delete />
          </el-icon>
          {{ t('Remove') }}
        </div>
      </template>
    </div>

    <!-- 
      新建分组对话框
      数据来源：
        - dialogAddGroupVisible 控制对话框显示/隐藏
        - newGroupName 双向绑定到输入框
      交互行为：
        - @keyup.enter 按回车键触发 handleAddGroup 确认
        - 取消按钮点击关闭对话框
        - 确认按钮点击调用 handleAddGroup 创建分组
      样式：固定宽度 400px
    -->
    <el-dialog v-model="dialogAddGroupVisible" :title="t('Add group')" width="400px">
      <!-- 分组名称输入框 -->
      <el-input v-model="newGroupName" :placeholder="t('Group Name')" @keyup.enter="handleAddGroup" />
      <!-- 对话框底部按钮区域 -->
      <template #footer>
        <!-- 取消按钮 -->
        <el-button @click="dialogAddGroupVisible = false">{{ t('Cancel') }}</el-button>
        <!-- 确认按钮 -->
        <el-button type="primary" @click="handleAddGroup">{{ t('Confirm') }}</el-button>
      </template>
    </el-dialog>

    <!-- 
      重命名分组对话框
      数据来源：
        - dialogRenameGroupVisible 控制对话框显示/隐藏
        - renameGroupName 双向绑定到输入框（预填当前分组名）
      交互行为：
        - @keyup.enter 按回车键触发 handleRenameGroup 确认
        - 取消按钮点击关闭对话框
        - 确认按钮点击调用 handleRenameGroup 重命名分组
      样式：固定宽度 400px
    -->
    <el-dialog v-model="dialogRenameGroupVisible" :title="t('Rename')" width="400px">
      <!-- 分组名称输入框 -->
      <el-input v-model="renameGroupName" :placeholder="t('Group Name')" @keyup.enter="handleRenameGroup" />
      <!-- 对话框底部按钮区域 -->
      <template #footer>
        <!-- 取消按钮 -->
        <el-button @click="dialogRenameGroupVisible = false">{{ t('Cancel') }}</el-button>
        <!-- 确认按钮 -->
        <el-button type="primary" @click="handleRenameGroup">{{ t('Confirm') }}</el-button>
      </template>
    </el-dialog>

    <!-- 
      重命名模组对话框
      数据来源：
        - dialogRenameModVisible 控制对话框显示/隐藏
        - renameModName 双向绑定到输入框
      交互行为：
        - 取消按钮点击关闭对话框
        - 确认按钮点击（当前未绑定事件，预留功能）
      样式：固定宽度 400px
    -->
    <el-dialog v-model="dialogRenameModVisible" :title="t('Rename')" width="400px">
      <!-- 模组名称输入框 -->
      <el-input v-model="renameModName" :placeholder="t('Mod Name')" />
      <!-- 对话框底部按钮区域 -->
      <template #footer>
        <!-- 取消按钮 -->
        <el-button @click="dialogRenameModVisible = false">{{ t('Cancel') }}</el-button>
        <!-- 确认按钮 -->
        <el-button type="primary" @click="handleRenameMod">{{ t('Confirm') }}</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<style scoped>
.mods-tab {
  display: flex;
  flex-direction: column;
  height: 100%;
  width: 100%;
  overflow: hidden;
}

.mods-toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 16px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.04);
  flex-shrink: 0;
  gap: 12px;
}

.toolbar-left,
.toolbar-right {
  display: flex;
  align-items: center;
  gap: 8px;
}

.search-input {
  width: 300px;
}

.mods-content {
  display: flex;
  flex: 1;
  overflow: hidden;
}

.groups-sidebar {
  width: 240px;
  display: flex;
  flex-direction: column;
  overflow-y: auto;
  flex-shrink: 0;
  padding: 8px;
  gap: 8px;
  border-right: 1px solid rgba(255, 255, 255, 0.04);
  /* 隐藏滚动条 */
  scrollbar-width: none; /* Firefox */
  -ms-overflow-style: none; /* IE/Edge */
  cursor: grab;
  user-select: none;
  -webkit-user-select: none;
}

.groups-sidebar::-webkit-scrollbar {
  display: none; /* Chrome/Safari/WebView */
}

.groups-sidebar:active {
  cursor: grabbing;
}

/* 拖拽分隔条 */
.sidebar-resizer {
  width: 4px;
  flex-shrink: 0;
  cursor: col-resize;
  background-color: transparent;
  transition: background-color 0.2s ease;
}

.sidebar-resizer:hover {
  background-color: rgba(255, 255, 255, 0.15);
}

.groups-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.group-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px;
  border-radius: 12px;
  cursor: pointer;
  transition: all 0.2s ease;
}

.group-item:hover {
  background-color: rgba(255, 255, 255, 0.06);
}

.group-item.active {
  background-color: rgba(64, 158, 255, 0.2);
}

.group-icon {
  width: 48px;
  height: 48px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  border: 2px solid transparent;
  transition: border-color 0.2s;
  flex-shrink: 0;
}

.group-item.active .group-icon {
  border-color: var(--el-color-primary);
}

.group-icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.group-icon :deep(.el-icon) {
  font-size: 24px;
  color: rgba(255, 255, 255, 0.3);
}

.group-info {
  flex: 1;
  min-width: 0;
}

.group-name {
  font-size: 13px;
  font-weight: 500;
  color: rgba(255, 255, 255, 0.85);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.group-count {
  font-size: 11px;
  color: rgba(255, 255, 255, 0.4);
}

.group-favorite {
  flex-shrink: 0;
  font-size: 16px;
}

.groups-reserved {
  padding: 8px 12px;
  margin-bottom: 4px;
}

.reserved-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.reserved-title {
  font-size: 11px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.35);
  text-transform: uppercase;
  letter-spacing: 1px;
}

.mods-display {
  flex: 1;
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

/* 拖拽悬停时的样式 */
.mods-display.drag-over {
  outline: 2px dashed rgba(64, 158, 255, 0.8);
  outline-offset: -4px;
  background-color: rgba(64, 158, 255, 0.05);
}

.mods-container {
  flex: 1;
  overflow-y: auto;
  padding: 16px;
  scroll-behavior: smooth;
  -webkit-overflow-scrolling: touch;
  cursor: grab;
  user-select: none;
}

.mods-container:active {
  cursor: grabbing;
}

/* Android 风格滚动条：隐藏原生滚动条 */
.mods-container::-webkit-scrollbar {
  width: 4px;
}

.mods-container::-webkit-scrollbar-track {
  background: transparent;
}

.mods-container::-webkit-scrollbar-thumb {
  background-color: rgba(255, 255, 255, 0.15);
  border-radius: 2px;
}

.mods-container::-webkit-scrollbar-thumb:hover {
  background-color: rgba(255, 255, 255, 0.3);
}

.mods-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
  gap: 16px;
}

.mod-card {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 0;
  border-radius: 16px;
  cursor: pointer;
  transition: all 0.2s ease;
  background-color: transparent;
  overflow: hidden;
}

.mod-card:hover {
  transform: translateY(-2px);
}

.mod-card:hover .mod-icon {
  box-shadow: 0 0 0 2px var(--el-color-primary), 0 8px 24px rgba(64, 158, 255, 0.3);
}

.mod-card .mod-icon {
  box-shadow: 0 0 0 2px var(--el-color-primary);
}

.mod-card.selected .mod-icon {
  box-shadow: 0 0 0 3px var(--el-color-primary), 0 8px 24px rgba(64, 158, 255, 0.3);
}

.mod-card.disabled .mod-icon {
  box-shadow: 0 0 0 2px #ef4444;
}

.mod-card.disabled.selected .mod-icon {
  box-shadow: 0 0 0 3px #ef4444, 0 8px 24px rgba(239, 68, 68, 0.3);
}

.mod-card.disabled {
  opacity: 1;
}

.mod-card.none-slot .mod-icon {
  background-color: rgba(255, 255, 255, 0.04);
}

.mod-icon {
  width: 100%;
  aspect-ratio: 3 / 4;
  border-radius: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  background-color: rgba(255, 255, 255, 0.06);
  transition: box-shadow 0.2s ease;
}

.mod-icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.mod-icon :deep(.el-icon) {
  font-size: 32px;
  color: rgba(255, 255, 255, 0.2);
}

.mod-name {
  font-size: 11px;
  font-weight: 500;
  color: rgba(255, 255, 255, 0.7);
  text-align: center;
  overflow: hidden;
  text-overflow: ellipsis;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  line-height: 1.4;
  min-height: 30px;
  width: 100%;
  padding: 0 4px;
}

.mod-status-icons {
  display: flex;
  gap: 4px;
  justify-content: center;
  min-height: 16px;
}

.status-icon {
  font-size: 14px;
}

.status-icon.warning {
  color: var(--el-color-warning);
}

.status-icon.info {
  color: var(--el-color-info);
}

.mod-favorite {
  position: absolute;
  top: 8px;
  right: 8px;
  font-size: 18px;
  cursor: pointer;
  z-index: 1;
}

.mod-disabled-badge {
  display: none;
}

/* ===== 行列布局（List）样式 ===== */

.mods-list {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  padding: 8px;
  scroll-snap-type: y proximity;
}

.list-mod-card {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  width: 160px;
  padding: 0;
  border-radius: 14px;
  cursor: pointer;
  transition: all 0.2s ease;
  background-color: transparent;
  overflow: hidden;
  scroll-snap-align: start;
}

.list-mod-card:hover {
  transform: translateY(-2px);
}

.list-mod-card:hover .list-mod-icon {
  box-shadow: 0 0 0 2px var(--el-color-primary), 0 8px 24px rgba(64, 158, 255, 0.3);
}

.list-mod-card .list-mod-icon {
  box-shadow: 0 0 0 2px var(--el-color-primary);
}

.list-mod-card.selected .list-mod-icon {
  box-shadow: 0 0 0 3px var(--el-color-primary), 0 8px 24px rgba(64, 158, 255, 0.3);
}

.list-mod-card.disabled .list-mod-icon {
  box-shadow: 0 0 0 2px #ef4444;
}

.list-mod-card.disabled.selected .list-mod-icon {
  box-shadow: 0 0 0 3px #ef4444, 0 8px 24px rgba(239, 68, 68, 0.3);
}

.list-mod-card.disabled {
  opacity: 1;
}

.list-mod-card.none-slot .list-mod-icon {
  background-color: rgba(255, 255, 255, 0.04);
}

.list-mod-icon {
  width: 100%;
  aspect-ratio: 3 / 4;
  border-radius: 14px;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  background-color: rgba(255, 255, 255, 0.06);
  transition: box-shadow 0.2s ease;
}

.list-mod-icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.list-mod-icon :deep(.el-icon) {
  font-size: 36px;
  color: rgba(255, 255, 255, 0.2);
}

.list-mod-name {
  font-size: 12px;
  font-weight: 500;
  color: rgba(255, 255, 255, 0.7);
  text-align: center;
  overflow: hidden;
  text-overflow: ellipsis;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  line-height: 1.4;
  min-height: 32px;
  width: 100%;
  padding: 0 4px;
}

.list-mod-status-icons {
  display: flex;
  gap: 4px;
  justify-content: center;
  min-height: 16px;
}

.context-menu {
  position: fixed;
  z-index: 9999;
  background-color: rgba(30, 30, 34, 0.95);
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 8px;
  padding: 4px;
  min-width: 180px;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
}

.context-menu-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  font-size: 14px;
  color: rgba(255, 255, 255, 0.8);
  border-radius: 6px;
  cursor: pointer;
  transition: background-color 0.15s ease;
}

.context-menu-item:hover {
  background-color: rgba(255, 255, 255, 0.08);
}

.context-menu-item.danger {
  color: #f56c6c;
}

.context-menu-item.danger:hover {
  background-color: rgba(245, 108, 108, 0.15);
}

.context-menu-separator {
  height: 1px;
  background-color: rgba(255, 255, 255, 0.1);
  margin: 4px 0;
}

.context-menu-item :deep(.el-icon) {
  font-size: 16px;
}

.restore-zone {
  margin-top: 16px;
  border: 2px dashed rgba(255, 255, 255, 0.1);
  border-radius: 12px;
  padding: 16px;
  background-color: rgba(255, 255, 255, 0.02);
  transition: all 0.2s ease;
}

.restore-zone.drag-over {
  border-color: var(--el-color-primary);
  background-color: rgba(64, 158, 255, 0.1);
}

.restore-zone-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 12px;
  padding-bottom: 8px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
}

.restore-zone-icon {
  font-size: 18px;
  color: rgba(255, 255, 255, 0.6);
}

.restore-zone-title {
  font-size: 14px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.85);
}

.restore-zone-hint {
  font-size: 12px;
  color: rgba(255, 255, 255, 0.4);
  margin-left: auto;
}

.restore-zone-content {
  min-height: 80px;
}

.restore-zone-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 20px;
}

.restore-zone-empty p {
  font-size: 13px;
  color: rgba(255, 255, 255, 0.5);
  margin: 0;
}

.restore-zone-sub {
  font-size: 12px !important;
  color: rgba(255, 255, 255, 0.3) !important;
}

.restore-zone-files {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.restore-zone-file-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background-color: rgba(255, 255, 255, 0.04);
  border-radius: 8px;
}

.restore-zone-file-name {
  font-size: 13px;
  color: rgba(255, 255, 255, 0.8);
  flex-shrink: 0;
}

.restore-zone-file-path {
  font-size: 12px;
  color: rgba(255, 255, 255, 0.4);
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.restore-zone-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px solid rgba(255, 255, 255, 0.06);
}
</style>
