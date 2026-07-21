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
import type { InputInstance } from 'element-plus';
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
  Timer,
  Top,
  Search,
  Lock
} from '@element-plus/icons-vue';
import { useGame } from '../../../composables/useGame';
import { sortModsForDisplay } from '../../../stores/game';
import { useHashConflictStore } from '../../../stores/hashConflict';
import { fuzzyMatch, splitByIndices } from '../../../utils/fuzzyMatch';
import type { TextSegment } from '../../../utils/fuzzyMatch';
import { useSettings } from '../../../composables/useSettings';
import {
  invokeRefreshMods,
  invokeRefreshSingleGroup,
  invokeToggleModDisabled,
  invokeToggleTreeNodeModDisabled,
  invokeDisableTreeNodeMod,
  invokeToggleTreeNodeGroupDisabled,
  invokeAddGroup,
  invokeAddMods,
  invokeRemoveGroup,
  invokeRemoveMod,
  invokeRenameGroup,
  invokeRenameMod,
  invokeOpenPath,
  invokeStartFileWatcher,
  invokeStopFileWatcher,
  invokeSetSelectedMod,
  invokeValidateArchiveFile,
  invokeExtractArchive,
  invokeExportMod,
  invokeExportGroup,
  invokeUpdateModData,
  convertToAssetUrl
} from '../../../utils/invoke';
import { EventNames, eventManager } from '../../../utils/events';
import type { ModData, ModGroupData, LayoutMode, TargetGame } from '../../../types';
import { LayoutMode as LayoutModeEnum } from '../../../types';
import GroupTreeNode from './GroupTreeNode.vue';
import { createLogger } from '../../../utils/logger';

const { t } = useI18n();
const game = useGame();
const settings = useSettings();
const hashConflictStore = useHashConflictStore();
const log = createLogger('ModsTab');

// ===== 响应式状态 =====
const isLoading = ref(false);                          // 全局加载指示
const isApplyingSelection = ref(false);               // 正在应用模组选择
const showFavoritesOnly = ref(false);                  // 是否仅显示收藏项
const selectedModIndex = ref(0);                       // 当前选中的模组索引（用于高亮与轮播定位）
const dialogAddGroupVisible = ref(false);              // 新建分组对话框可见性
const dialogRenameGroupVisible = ref(false);           // 重命名分组对话框可见性
const dialogRenameModVisible = ref(false);             // 重命名模组对话框可见性
const newGroupName = ref('');                          // 新建分组名称输入
const targetGroupPath = ref<string | undefined>();      // 目标分组路径（右键点击时设置）
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
  hasDragged: boolean;
} | null = null;
// 用于在拖动结束后阻止一次点击/右键误触发
let suppressNextModsContainerClick = false;
// requestAnimationFrame ID，用于取消未完成的滚动更新
let modsScrollDragRaf: number | null = null;
// 拖动前容器的 scroll-behavior，拖动结束后恢复
let modsContainerOriginalScrollBehavior: string = '';

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
  if (e.button !== 0 || !modsContainerRef.value) return;
  // 排除交互元素：按钮、输入框、模组卡片上的收藏图标等
  const target = e.target as HTMLElement;
  if (
    target.closest('button') ||
    target.closest('.el-button') ||
    target.closest('.mod-favorite') ||
    target.closest('input') ||
    target.closest('textarea') ||
    target.closest('select') ||
    target.closest('[contenteditable]') ||
    target.closest('.el-input') ||
    target.closest('.el-textarea')
  ) return;

  log.debug('Mouse down', {
    targetTag: target.tagName,
    targetClass: target.className,
    clientX: e.clientX,
    clientY: e.clientY
  });

  // 清除可能存在的文本选中，避免拖动时产生文字拖拽残影
  window.getSelection()?.removeAllRanges();

  const container = modsContainerRef.value;
  modsContainerOriginalScrollBehavior = container.style.scrollBehavior || '';
  container.style.scrollBehavior = 'auto';
  container.classList.add('dragging');

  modsScrollDragState = {
    startY: e.clientY,
    startX: e.clientX,
    startScrollTop: container.scrollTop,
    startScrollLeft: container.scrollLeft,
    active: true,
    hasDragged: false,
  };
  document.addEventListener('mousemove', onModsContainerMouseMove);
  document.addEventListener('mouseup', onModsContainerMouseUp);
  window.addEventListener('mouseleave', onModsContainerMouseLeave);
}

function onModsContainerMouseMove(e: MouseEvent) {
  if (!modsScrollDragState?.active || !modsContainerRef.value) return;
  const dy = e.clientY - modsScrollDragState.startY;
  const dx = e.clientX - modsScrollDragState.startX;

  // 位移超过 5px 视为拖动
  if (Math.abs(dy) > 5 || Math.abs(dx) > 5) {
    if (!modsScrollDragState.hasDragged) {
      modsScrollDragState.hasDragged = true;
      e.preventDefault();
      window.getSelection()?.removeAllRanges();
      log.debug('Drag started', { dx, dy });
    }
  }

  if (modsScrollDragState.hasDragged) {
    if (modsScrollDragRaf) cancelAnimationFrame(modsScrollDragRaf);
    modsScrollDragRaf = requestAnimationFrame(() => {
      if (!modsScrollDragState || !modsContainerRef.value) return;
      modsContainerRef.value.scrollTop = modsScrollDragState.startScrollTop - dy;
      modsContainerRef.value.scrollLeft = modsScrollDragState.startScrollLeft - dx;
      modsScrollDragRaf = null;
    });
  }
}

function onModsContainerMouseUp() {
  endModsContainerDrag();
}

function onModsContainerMouseLeave() {
  endModsContainerDrag();
}

function endModsContainerDrag() {
  const hadDragged = modsScrollDragState?.hasDragged ?? false;
  if (modsScrollDragState?.active && modsScrollDragState.hasDragged) {
    suppressNextModsContainerClick = true;
  }

  log.debug('Drag ended', { hasDragged: hadDragged, suppressClick: suppressNextModsContainerClick });

  if (modsScrollDragRaf) {
    cancelAnimationFrame(modsScrollDragRaf);
    modsScrollDragRaf = null;
  }

  const container = modsContainerRef.value;
  if (container) {
    container.style.scrollBehavior = modsContainerOriginalScrollBehavior;
    container.classList.remove('dragging');
  }

  modsScrollDragState = null;
  document.removeEventListener('mousemove', onModsContainerMouseMove);
  document.removeEventListener('mouseup', onModsContainerMouseUp);
  window.removeEventListener('mouseleave', onModsContainerMouseLeave);
}

// 捕获阶段阻止拖动结束后产生的点击/右键事件（仅针对模组卡片）
function onModsContainerClick(e: MouseEvent) {
  if (!suppressNextModsContainerClick) return;
  const target = e.target as HTMLElement;
  if (target.closest('.mod-card') || target.closest('.list-mod-card')) {
    e.stopImmediatePropagation();
    e.preventDefault();
    log.debug('Suppressed card click after drag');
  }
  suppressNextModsContainerClick = false;
}

function onModsContainerContextMenu(e: MouseEvent) {
  if (!suppressNextModsContainerClick) return;
  const target = e.target as HTMLElement;
  if (target.closest('.mod-card') || target.closest('.list-mod-card')) {
    e.stopImmediatePropagation();
    e.preventDefault();
    log.debug('Suppressed card contextmenu after drag');
  }
  suppressNextModsContainerClick = false;
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

// 实际显示的模组列表：基于当前分组模组，按收藏过滤开关筛选，并按选中状态排序
const displayMods = computed(() => {
  let mods = game.currentMods.value;
  if (showFavoritesOnly.value) {
    mods = mods.filter(m => m.favoriteDateTime !== null);
  }
  // 排序：None 占位模组 → 选中模组 → 未选中模组（按后端返回的数组顺序）
  // 注意：修复 realIndex 一致性后，realIndex 来自目录列表原始顺序而非数组位置，
  // 因此按数组位置排序以保持后端返回的 disabled-last / favorites-first / name 显示顺序
  const selectedPath = game.getSelectedModPath(game.currentGroupPath.value);
  return sortModsForDisplay(mods, selectedPath);
});

// 统一搜索关键字（同时用于分组高亮与模组过滤）
const searchKeyword = ref('');
// 分组列表容器引用（用于搜索时滚动到第一个匹配项）
const groupListRef = ref<HTMLElement | null>(null);
// 搜索输入框引用（用于快捷键聚焦）
const searchInputRef = ref<InputInstance | null>(null);
// 搜索栏显示/隐藏状态（通过快捷键切换）
const searchVisible = ref(false);

// 搜索关键字变化时滚动到第一个匹配项
watch(searchKeyword, (newVal) => {
  if (!newVal || !groupListRef.value) return;
  nextTick(() => {
    const firstMatch = groupListRef.value?.querySelector('.group-highlight');
    firstMatch?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  });
});

// 经过搜索关键字过滤后的模组列表：在 displayMods（已应用收藏过滤与排序）基础上叠加模糊匹配
const filteredMods = computed(() => {
  const currentMods = displayMods.value;
  if (!searchKeyword.value) return currentMods;
  return currentMods.filter(mod => fuzzyMatch(searchKeyword.value, mod.modName).matched);
});

/**
 * 根据搜索关键字将模组名称拆分为高亮片段。
 * @param modName 模组名称
 * @returns 文本片段数组，每个片段标记是否高亮
 */
function getModNameSegments(modName: string): TextSegment[] {
  if (!searchKeyword.value) return [{ text: modName, highlight: false }];
  const result = fuzzyMatch(searchKeyword.value, modName);
  return splitByIndices(modName, result.indices);
}

/**
 * 判断指定模组是否为当前分组的选中模组（用于紫色描边显示）。
 * 仅对已启用且被用户明确选择的模组显示紫色描边效果。
 * @param mod 待判断的模组
 * @returns true 表示该模组为当前分组的已启用选中模组
 */
function isModSelected(mod: ModData): boolean {
  if (mod.realIndex === 0) return false;
  if (mod.isDisabled) return false;
  const currentGroup = game.currentGroup.value;
  if (currentGroup?.isTreeNode && !currentGroup.isVirtual) {
    return !mod.isDisabled;
  }
  const selectedPath = game.getSelectedModPath(game.currentGroupPath.value);
  return mod.modPath === selectedPath;
}

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
      // 任务取消是 TaskQueue 的正常行为（新请求取消旧请求），不作为错误
      const errMsg = String(error);
      if (errMsg.includes('was cancelled')) {
        log.debug('Task cancelled by newer request, skipping');
      } else {
        log.error('Failed to refresh mods', error);
      }
    } finally {
      isLoading.value = false;
    }
    log.debug('Mods refreshed', { game: game.targetGame.value });
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
 * 同时从 store 的 selectedModPaths 中恢复该分组的选中模组索引，确保高亮与持久化状态一致。
 * @param group 选中的分组数据
 */
function selectGroup(group: ModGroupData) {
  game.setCurrentGroupByPath(group.groupPath);
  nextTick(() => {
    const selectedModPath = game.getSelectedModPath(group.groupPath);
    let targetModPath: string | null = selectedModPath;
    if (!targetModPath) {
      const selectedIdx = group.previousSelectedModOnGroup;
      if (selectedIdx >= 0 && selectedIdx < group.modsInGroup.length) {
        targetModPath = group.modsInGroup[selectedIdx].modPath;
      }
    }
    if (targetModPath) {
      const idx = filteredMods.value.findIndex(m => m.modPath === targetModPath);
      selectedModIndex.value = idx >= 0 ? idx : 0;
    } else {
      selectedModIndex.value = 0;
    }
  });
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
  if (isApplyingSelection.value) return;
  selectedModIndex.value = index;
}

async function applyModSelection(mod: ModData) {
  if (isApplyingSelection.value) return;
  const currentGroup = game.currentGroup.value;
  if (!currentGroup) return;

  const isTreeNode = currentGroup.isTreeNode && !currentGroup.isVirtual;
  if (!isTreeNode && mod.isDisabled) return;

  const arrayIndex = currentGroup.modsInGroup.findIndex(m => m.modPath === mod.modPath);
  if (arrayIndex < 0) return;
  isApplyingSelection.value = true;
  try {
    const filteredIndex = filteredMods.value.findIndex(m => m.modPath === mod.modPath);
    selectedModIndex.value = filteredIndex;

    if (isTreeNode) {
      const realIndex = currentGroup.modsInGroup.findIndex(m => m.modPath === mod.modPath);
      if (realIndex !== -1) {
        const childGroupPaths = new Set((currentGroup.children || []).map(c => c.groupPath));
        for (const other of currentGroup.modsInGroup) {
          if (other.realIndex === 0) continue;
          if (other.modPath === mod.modPath) continue;
          if (other.isDisabled) continue;
          if (childGroupPaths.has(other.modPath)) continue;
          try {
            const disabledPath = await invokeDisableTreeNodeMod(other.modPath);
            const otherIndex = currentGroup.modsInGroup.findIndex(m => m.modPath === other.modPath);
            if (otherIndex !== -1) {
              game.updateModInGroup(game.currentGroupPath.value, otherIndex, { ...other, modPath: disabledPath, isDisabled: true });
            }
          } catch (e) {
            log.error('Failed to disable tree node mod', e);
          }
        }

        const [enabledPath, enabledDisabled] = await invokeToggleTreeNodeModDisabled(mod.modPath);
        const updatedMod = { ...mod, modPath: enabledPath, isDisabled: enabledDisabled };
        game.updateModInGroup(game.currentGroupPath.value, realIndex, updatedMod);

        const updatedGroup = await invokeRefreshSingleGroup(game.currentGroupPath.value);
        game.updateGroup(game.currentGroupPath.value, updatedGroup);
        game.setSelectedModPath(game.currentGroupPath.value, enabledPath);
      }
    } else {
      game.setSelectedModPath(game.currentGroupPath.value, mod.modPath);
      await invokeSetSelectedMod(game.currentGroupPath.value, arrayIndex);
      await invokeUpdateModData(game.targetGame.value);
      await refreshMods();
    }
  } catch (error) {
    log.error('Failed to select mod', error);
    ElMessage.error(t('Failed to enable mod'));
  } finally {
    isApplyingSelection.value = false;
  }
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
    log.error('Failed to toggle mod', error);
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

/**
 * 切换 # 目录分组的启用/禁用状态。
 * 仅对 isTreeNode 且非 isVirtual 的分组生效（即 # 目录分组）。
 * 操作通过目录名 DISABLED 前缀控制，不涉及 INI 文件修改。
 * @param group 待切换的分组
 */
async function toggleTreeNodeGroupDisabled(group: ModGroupData) {
  if (!group.isTreeNode || group.isVirtual) return;
  try {
    await invokeToggleTreeNodeGroupDisabled(group.groupPath);
    const updatedGroup = await invokeRefreshSingleGroup(group.groupPath);
    game.updateGroup(group.groupPath, updatedGroup);
    ElMessage.success(group.isDisabled ? t('Group enabled') : t('Group disabled'));
  } catch (error) {
    log.error('Failed to toggle group disabled', error);
    ElMessage.error(t('Failed to toggle group disabled'));
  }
}

// ===== 拖拽添加 Mod =====

const isImporting = ref(false);

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
 * 验证文件是否为有效的压缩文件（后缀名+文件头魔数）。
 * @param filePath 文件路径
 * @returns (是否有效, 文件类型)
 */
async function validateArchive(filePath: string): Promise<[boolean, string]> {
  try {
    return await invokeValidateArchiveFile(filePath);
  } catch {
    return [false, 'unknown'];
  }
}

/**
 * 拖拽释放时获取文件路径并调用后端添加 Mod。
 * 支持：目录深度遍历、压缩文件自动解压（zip/7z）、RAR仅验证不支持解压。
 * 默认添加到当前选中的分组。
 * @param event 拖拽事件
 */
async function onDrop(event: DragEvent) {
  event.preventDefault();
  isDraggingOver.value = false;

  if (isImporting.value) {
    ElMessage.warning(t('Import is already in progress'));
    return;
  }

  const files = event.dataTransfer?.files;
  if (!files || files.length === 0) return;

  const currentGroup = game.currentGroup.value;
  if (!currentGroup) {
    ElMessage.warning(t('Please select a group first'));
    return;
  }

  isImporting.value = true;
  const pathsToAdd: string[] = [];

  try {
    for (let i = 0; i < files.length; i++) {
      const file = files[i] as any;
      if (!file.path) continue;

      if (file.isDirectory) {
        pathsToAdd.push(file.path);
      } else {
        const ext = file.name.toLowerCase().split('.').pop();
        if (ext === 'zip' || ext === '7z' || ext === 'rar') {
          const [valid, fileType] = await validateArchive(file.path);
          if (!valid) {
            ElMessage.warning(t('Invalid archive file: {name}', { name: file.name }));
            continue;
          }

          if (fileType === 'rar') {
            ElMessage.warning(t('RAR format is not supported for extraction'));
            continue;
          }

          const extractDir = file.path.replace(/\.(zip|7z)$/i, '_extracted');
          try {
            await invokeExtractArchive(file.path, extractDir);
            pathsToAdd.push(extractDir);
          } catch {
            ElMessage.error(t('Failed to extract archive: {name}', { name: file.name }));
          }
        } else {
          pathsToAdd.push(file.path);
        }
      }
    }

    if (pathsToAdd.length > 0) {
      await invokeAddMods(pathsToAdd, currentGroup.groupPath);
      await refreshMods();
      ElMessage.success(t('Mods added successfully'));
    } else {
      ElMessage.warning(t('No valid files to import'));
    }
  } catch (error) {
    log.error('Failed to add mods', error);
    ElMessage.error(t('Failed to add mods'));
  } finally {
    isImporting.value = false;
  }
}

/** 
 * 打开新建分组对话框，并清空输入框。
 * @param targetPath 目标分组路径（可选）。右键点击分组时传入，新分组将与该分组处于同一目录层级。
 */
function showAddGroupDialog(targetPath?: string) {
  newGroupName.value = '';
  targetGroupPath.value = targetPath;
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
    await invokeAddGroup(newGroupName.value.trim(), targetGroupPath.value);
    await refreshMods();
    dialogAddGroupVisible.value = false;
    ElMessage.success(t('Group added successfully'));
  } catch (error) {
    log.error('Failed to add group', error);
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
    log.error('Failed to rename group', error);
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
    log.error('Failed to rename mod', error);
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
      log.error('Failed to remove group', error);
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
  console.log('openModFolder', mod);
  if (mod.realIndex === 0) {
    return
  };
  if (!mod.modPath || !mod.modPath.trim()) {
    ElMessage.warning(t('Mod path is empty, cannot open'));
    return;
  }
  invokeOpenPath(mod.modPath).catch((err) => {
    log.error('Failed to open mod folder', err);
    ElMessage.error(t('Failed to open folder'));
  });
}

/**
 * 在系统文件管理器中打开分组所在路径。
 * @param group 目标分组
 */
function openGroupFolder(group: ModGroupData) {
  if (!group.groupPath || !group.groupPath.trim()) {
    ElMessage.warning(t('Group path is empty, cannot open'));
    return;
  }
  invokeOpenPath(group.groupPath).catch((err) => {
    log.error('Failed to open group folder', err);
    ElMessage.error(t('Failed to open folder'));
  });
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
 * @param command 菜单命令标识：rename / favorite / pin / open / delete / toggle / export
 * 业务逻辑：根据 contextMenuType 分发到 group 或 mod 的对应处理函数，最后统一隐藏菜单。
 */
async function handleContextMenuSelect(command: string) {
  // console.log("handleContextMenuSelect", command, contextMenuType.value, contextMenuData.value);
  if (contextMenuType.value === 'group') {
    const group = contextMenuData.value as ModGroupData;
    switch (command) {
      case 'rename':
        showRenameGroupDialog(group);
        break;
      case 'favorite':
        toggleGroupFavorite(group);
        break;
      case 'pin':
        // 置顶复用 favoriteDateTime 字段，调用同一函数实现排序置顶
        toggleGroupFavorite(group);
        break;
      case 'open':
        openGroupFolder(group);
        break;
      case 'delete':
        handleRemoveGroup();
        break;
      case 'toggle-disabled':
        await toggleTreeNodeGroupDisabled(group);
        break;
      case 'export':
        await exportGroup(group);
        break;
    }
  } else if (contextMenuType.value === 'mod') {
    const mod = contextMenuData.value as ModData;
    // console.log("handleContextMenuSelect mod", command, mod);
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
      case 'export':
        await exportMod(mod);
        break;
      case 'remove':
        await removeModFromGroup(mod);
        break;
    }
  }
  hideContextMenu();
}

async function selectModInGroup(mod: ModData) {
  await applyModSelection(mod);
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
    // 调用后端命令：先还原（启用）再移动到 DISABLED_MANAGED_REMOVED
    await invokeRemoveMod(mod.modPath);
    await refreshMods();
    ElMessage.success(t('Mod removed successfully'));
  } catch (error) {
    if (error !== 'cancel') {
      log.error('Failed to remove mod', error);
      ElMessage.error(t('Failed to remove mod'));
    }
  }
}

/**
 * 导出模组为7z压缩文件。
 * @param mod 目标模组
 */
async function exportMod(mod: ModData) {
  if (mod.realIndex === 0) return;

  try {
    const result = await window.showDirectoryPicker?.();
    if (!result) return;

    const destDir = result.path;
    const exportPath = await invokeExportMod(mod.modPath, destDir);
    ElMessage.success(t('Mod exported to: {path}', { path: exportPath }));
  } catch (error) {
    log.error('Failed to export mod', error);
    ElMessage.error(t('Failed to export mod'));
  }
}

/**
 * 导出分组为7z压缩文件（保持目录结构）。
 * @param group 目标分组
 */
async function exportGroup(group: ModGroupData) {
  try {
    const result = await window.showDirectoryPicker?.();
    if (!result) return;

    const destDir = result.path;
    const exportPath = await invokeExportGroup(group.groupPath, destDir);
    ElMessage.success(t('Group exported to: {path}', { path: exportPath }));
  } catch (error) {
    log.error('Failed to export group', error);
    ElMessage.error(t('Failed to export group'));
  }
}

/**
 * 模组双击回调。
 * 业务逻辑：选择模组作为当前槽位启用的模组。
 * @param mod 目标模组
 * @param index 目标模组在 displayMods 中的索引（保留参数以兼容调用）
 */
function onModDoubleClick(mod: ModData, _index: number) {
  applyModSelection(mod);
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
      log.debug('File watcher setup complete', { path: game.modsPath.value, game: game.targetGame.value });
    }
  } catch (error) {
    log.error('Failed to setup file watcher', error);
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
    if (gameSwitchDebounceTimer) {
      clearTimeout(gameSwitchDebounceTimer);
    }
    gameSwitchDebounceTimer = setTimeout(async () => {
      // 直接更新 gameStore 的目标游戏状态（不触发事件，避免循环触发）
      // 前端 setTargetGame 调用 emitLocal 时，targetGame 已更新，此处赋值相同值无副作用
      // 后端托盘 emit 时，targetGame 未更新，此处修复游戏选择器显示为空的问题
      game.targetGame.value = newGame as TargetGame;
      game.modsPath.value = settings.getModsPath(newGame as TargetGame);
      // 先加载模组，完成后再启动文件监听，避免监听器在加载期间触发不必要的 refreshMods
      await game.loadModsForGame(newGame as TargetGame);
      await setupFileWatcher();
      // 模组加载完成后主动触发 hash 冲突检测（检测失败不影响主流程）
      if (newGame !== 'none') {
        try {
          await hashConflictStore.checkHashConflicts();
        } catch (e) {
          log.warn('Hash conflict check after game switch failed', { reason: String(e) });
        }
      }
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
  // 首次加载完成后主动触发 hash 冲突检测（检测失败不影响主流程）
  if (game.targetGame.value !== 'none') {
    try {
      await hashConflictStore.checkHashConflicts();
    } catch (e) {
      log.warn('Initial hash conflict check failed', { reason: String(e) });
    }
  }
  document.addEventListener('click', hideContextMenu);
  document.addEventListener('keydown', handleKeyDown);
  // 捕获阶段监听容器点击/右键，用于阻止拖动结束后的误触发
  modsContainerRef.value?.addEventListener('click', onModsContainerClick, true);
  modsContainerRef.value?.addEventListener('contextmenu', onModsContainerContextMenu, true);
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
  invokeStopFileWatcher().catch((err) => log.error('Failed to stop file watcher on unmount', err));
  document.removeEventListener('click', hideContextMenu);
  document.removeEventListener('keydown', handleKeyDown);
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
  window.removeEventListener('mouseleave', onModsContainerMouseLeave);
  modsContainerRef.value?.removeEventListener('click', onModsContainerClick, true);
  modsContainerRef.value?.removeEventListener('contextmenu', onModsContainerContextMenu, true);
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
      game.loadModsForGame(game.targetGame.value);
    }
  }
);

// 监听当前分组变化，同步选中模组索引并打印调试信息
watch(
  () => game.currentGroup.value,
  (newGroup) => {
    // 从后端记录的选中索引恢复高亮状态（处理分组刷新后 selectedModIndex 被重置的情况）
    if (newGroup) {
      const selectedIdx = newGroup.previousSelectedModOnGroup;
      if (selectedIdx >= 0 && selectedIdx < newGroup.modsInGroup.length) {
        selectedModIndex.value = selectedIdx;
      } else {
        selectedModIndex.value = 0;
      }
    }
  },
  { immediate: true }
);

/**
 * 切换统一搜索框显示/隐藏（供父组件通过 ref 调用）。
 * 已显示时隐藏并清空关键字；隐藏时显示并聚焦搜索输入框。
 */
function toggleSearch(): void {
  if (searchVisible.value) {
    searchVisible.value = false;
    searchKeyword.value = '';
  } else {
    searchVisible.value = true;
    setTimeout(() => {
      searchInputRef.value?.focus();
    }, 300);
  }
}

/**
 * 隐藏搜索栏并清空关键字（点击外部区域或按 Esc 时调用）。
 */
function hideSearchBars(): void {
  if (!searchVisible.value) return;
  searchVisible.value = false;
  searchKeyword.value = '';
}

/**
 * 全局键盘事件处理：按 Esc 退出搜索；W/S/↑/↓切换模组；A/D/←/→切换分组；Enter/Space确认选择。
 */
function handleKeyDown(event: KeyboardEvent): void {
  const modsTab = document.querySelector('.mods-tab') as HTMLElement | null;
  if (!modsTab) return;
  const tabContent = modsTab.closest('.tab-content') as HTMLElement | null;
  if (tabContent && getComputedStyle(tabContent).display === 'none') return;
  if (isApplyingSelection.value) return;
  if (dialogAddGroupVisible.value || dialogRenameGroupVisible.value || dialogRenameModVisible.value) return;
  if (document.querySelector('.el-message-box__wrapper') || document.querySelector('.el-dialog__wrapper')) return;
  if (contextMenuVisible.value) {
    if (event.key === 'Escape') {
      event.preventDefault();
      hideContextMenu();
    }
    return;
  }
  const active = document.activeElement;
  if (active) {
    const tag = active.tagName.toLowerCase();
    if (tag === 'input' || tag === 'textarea' || tag === 'select' || (active as HTMLElement).isContentEditable) {
      if (event.key === 'Escape' && (searchVisible.value)) {
        event.preventDefault();
        hideSearchBars();
        (active as HTMLElement).blur();
      }
      return;
    }
  }
  if (event.key === 'Escape' && (searchVisible.value)) {
    event.preventDefault();
    hideSearchBars();
    (document.activeElement as HTMLElement)?.blur();
    return;
  }
  const key = event.key.toLowerCase();
  switch (key) {
    case 'w':
    case 'arrowup':
      event.preventDefault();
      navigateMod(-1);
      break;
    case 's':
    case 'arrowdown':
      event.preventDefault();
      navigateMod(1);
      break;
    case 'a':
    case 'arrowleft':
      event.preventDefault();
      navigateGroup(-1);
      break;
    case 'd':
    case 'arrowright':
      event.preventDefault();
      navigateGroup(1);
      break;
    case 'enter':
    case ' ':
      event.preventDefault();
      confirmCurrentMod();
      break;
  }
}

function navigateMod(delta: number) {
  const mods = filteredMods.value;
  if (mods.length === 0) return;
  let newIndex = selectedModIndex.value + delta;
  if (newIndex < 0) newIndex = 0;
  if (newIndex >= mods.length) newIndex = mods.length - 1;
  selectedModIndex.value = newIndex;
  nextTick(() => {
    const container = modsContainerRef.value;
    if (!container) return;
    const selected = container.querySelector('.mod-card.selected, .list-mod-card.selected') as HTMLElement | null;
    if (selected) {
      selected.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    }
  });
}

function getNavigableGroups(): ModGroupData[] {
  const result: ModGroupData[] = [];
  function walk(groups: ModGroupData[]) {
    for (const g of groups) {
      if (!g.isVirtual && !g.isDisabled) {
        result.push(g);
      }
      if (g.children && g.children.length > 0 && game.expandedPaths.value.has(g.groupPath)) {
        walk(g.children);
      }
    }
  }
  walk(displayGroups.value);
  return result;
}

function navigateGroup(delta: number) {
  const groups = getNavigableGroups();
  if (groups.length === 0) return;
  const currentPath = game.currentGroupPath.value;
  let currentIdx = groups.findIndex(g => g.groupPath === currentPath);
  if (currentIdx < 0) currentIdx = 0;
  let newIdx = currentIdx + delta;
  if (newIdx < 0) newIdx = 0;
  if (newIdx >= groups.length) newIdx = groups.length - 1;
  const targetGroup = groups[newIdx];
  if (targetGroup.groupPath !== currentPath) {
    selectGroup(targetGroup);
    nextTick(() => {
      const selected = groupListRef.value?.querySelector('.group-item.active') as HTMLElement | null;
      if (selected) {
        selected.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
      }
    });
  }
}

function confirmCurrentMod() {
  const mods = filteredMods.value;
  const idx = selectedModIndex.value;
  if (idx < 0 || idx >= mods.length) return;
  const mod = mods[idx];
  applyModSelection(mod);
}

/**
 * 获取统一搜索输入框的原生 DOM 元素。
 * 用于父组件判断焦点是否在搜索输入框内，以便快捷键关闭搜索框。
 * @returns 原生 input 元素，未挂载时返回 null
 */
function getSearchInputEl(): HTMLInputElement | null {
  return (searchInputRef.value as any)?.ref ?? null;
}

/** 返回统一搜索框是否可见。 */
function isSearchVisible(): boolean {
  return searchVisible.value;
}

// 暴露搜索框方法供父组件通过组件 ref 调用，用于快捷键聚焦
defineExpose({
  toggleSearch,
  getSearchInputEl,
  isSearchVisible
});
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
        @click="hideSearchBars"
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
        <div v-loading="isLoading" class="groups-list" ref="groupListRef">
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
            :search-keyword="searchKeyword"
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
        @drop="onDrop" @click="hideSearchBars">
        <!-- 
          模组容器
          加载状态：v-loading 在 isLoading 为 true 时显示加载动画
        -->
        <div v-loading="isLoading || isApplyingSelection" class="mods-container" ref="modsContainerRef" @mousedown="onModsContainerMouseDown" :element-loading-text="isApplyingSelection ? t('Double-click or press F to select a mod') : ''">
          <!--
            统一搜索框
            作用：同时用于分组高亮与当前分组内的模组模糊匹配
            交互行为：v-model 绑定 searchKeyword，clearable 支持一键清空
          -->
          <div class="search-bars-wrapper" :class="{ 'search-visible': searchVisible }" @click.stop>
            <div class="search-bar">
              <el-input
                ref="searchInputRef"
                v-model="searchKeyword"
                :placeholder="t('mods.searchPlaceholderUnified')"
                clearable
                :prefix-icon="Search"
                @keydown.esc.prevent="hideSearchBars"
              />
            </div>
          </div>
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
              <div v-for="(mod, index) in filteredMods" :key="mod.modPath" class="mod-card" :class="{
                selected: selectedModIndex === index,
                disabled: mod.isDisabled,
                'none-slot': mod.realIndex === 0,
                'mod-selected': isModSelected(mod)
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
                  模组名称（按搜索关键字分段高亮）
                  数据来源：mod.modName 经 getModNameSegments 拆分
                  样式：最多显示 2 行，超出显示省略号
                -->
                <div class="mod-name">
                  <template v-for="(seg, i) in getModNameSegments(mod.modName)" :key="i">
                    <span :class="{ 'highlight-text': seg.highlight }">{{ seg.text }}</span>
                  </template>
                </div>
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
            <div class="mods-list" v-if="filteredMods.length > 0">
              <div v-for="(mod, index) in filteredMods" :key="mod.modPath" class="list-mod-card" :class="{
                selected: selectedModIndex === index,
                disabled: mod.isDisabled,
                'none-slot': mod.realIndex === 0,
                'mod-selected': isModSelected(mod)
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
                <!-- 模组名称（按搜索关键字分段高亮） -->
                <div class="list-mod-name">
                  <template v-for="(seg, i) in getModNameSegments(mod.modName)" :key="i">
                    <span :class="{ 'highlight-text': seg.highlight }">{{ seg.text }}</span>
                  </template>
                </div>
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
            条件渲染：v-if 在 !isLoading && filteredMods.length === 0 时显示
            数据来源：isLoading (加载状态), filteredMods.length (过滤后模组数量)
            作用：当没有模组或搜索无结果时提示用户
          -->
          <el-empty v-if="!isLoading && filteredMods.length === 0" :description="t('No mods found')" :image-size="100" />
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
        <div class="context-menu-item" @click="showAddGroupDialog((contextMenuData as ModGroupData).groupPath)">
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
          启用/禁用分组菜单项（仅 # 目录分组显示）
          交互行为：@click 调用 handleContextMenuSelect('toggle-disabled')
          动态文本：根据 isDisabled 显示"Enable group"或"Disable group"
          图标：Lock 图标
          条件渲染：仅 isTreeNode && !isVirtual 时显示
        -->
        <div
          v-if="(contextMenuData as ModGroupData)?.isTreeNode && !(contextMenuData as ModGroupData)?.isVirtual"
          class="context-menu-item"
          @click="handleContextMenuSelect('toggle-disabled')"
        >
          <el-icon>
            <Lock />
          </el-icon>
          {{ (contextMenuData as ModGroupData)?.isDisabled ? t('Enable group') : t('Disable group') }}
        </div>
        <!--
          置顶/取消置顶菜单项
          交互行为：@click 调用 handleContextMenuSelect('pin')
          动态文本：根据 favoriteDateTime 是否存在显示"Pin to top"或"Unpin from top"
          图标：Top 图标
          说明：置顶复用 favoriteDateTime 字段，与收藏操作相同，但语义侧重排序置顶
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('pin')">
          <el-icon>
            <Top />
          </el-icon>
          {{ (contextMenuData as ModGroupData)?.favoriteDateTime ? t('Unpin from top') : t('Pin to top') }}
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
        <div class="context-menu-separator" />
        <div class="context-menu-item" @click="handleContextMenuSelect('export')">
          <el-icon>
            <Operation />
          </el-icon>
          {{ t('Export group') }}
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
        <div class="context-menu-separator" />
        <div class="context-menu-item" @click="handleContextMenuSelect('export')">
          <el-icon>
            <Operation />
          </el-icon>
          {{ t('Export mod') }}
        </div>
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

.search-bars-wrapper {
  max-height: 0;
  opacity: 0;
  overflow: hidden;
  transition: max-height 0.25s ease, opacity 0.25s ease;
}

.search-bars-wrapper.search-visible {
  max-height: 80px;
  opacity: 1;
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

.mods-container.dragging,
.mods-container:active {
  cursor: grabbing;
}

.mods-container.dragging *,
.mods-container.dragging *::before,
.mods-container.dragging *::after {
  user-select: none !important;
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
  padding: 4px;
  border-radius: 16px;
  cursor: pointer;
  transition: all 0.2s ease;
  background-color: transparent;
  overflow: hidden;
}

.mod-card:hover {
  transform: translateY(-2px);
  background-color: rgba(255, 255, 255, 0.04);
}

.mod-card:hover .mod-icon {
  box-shadow: 0 0 0 2px var(--el-color-primary), 0 8px 24px rgba(64, 158, 255, 0.2);
}

.mod-card .mod-icon {
  box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.1);
  transition: all 0.2s ease;
}

.mod-card.selected {
  background-color: rgba(64, 158, 255, 0.08);
}

.mod-card.selected .mod-icon {
  box-shadow: 0 0 0 3px var(--el-color-primary), 0 8px 24px rgba(64, 158, 255, 0.3);
  transform: scale(1.02);
}

/* 选中且已应用状态：紫色描边+发光 */
.mod-card.mod-selected {
  background-color: rgba(168, 85, 247, 0.1);
}

.mod-card.mod-selected .mod-icon {
  box-shadow: 0 0 0 3px #a855f7, 0 0 16px rgba(168, 85, 247, 0.5);
}

.mod-card.disabled .mod-icon {
  box-shadow: 0 0 0 2px rgba(239, 68, 68, 0.5);
  opacity: 0.5;
}

.mod-card.disabled.selected .mod-icon {
  box-shadow: 0 0 0 3px #ef4444, 0 8px 24px rgba(239, 68, 68, 0.3);
  opacity: 0.6;
}

.mod-card.disabled {
  opacity: 1;
  cursor: not-allowed;
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
  padding: 4px;
  border-radius: 14px;
  cursor: pointer;
  transition: all 0.2s ease;
  background-color: transparent;
  overflow: hidden;
  scroll-snap-align: start;
}

.list-mod-card:hover {
  transform: translateY(-2px);
  background-color: rgba(255, 255, 255, 0.04);
}

.list-mod-card:hover .list-mod-icon {
  box-shadow: 0 0 0 2px var(--el-color-primary), 0 8px 24px rgba(64, 158, 255, 0.2);
}

.list-mod-card .list-mod-icon {
  box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.1);
  transition: all 0.2s ease;
}

.list-mod-card.selected {
  background-color: rgba(64, 158, 255, 0.08);
}

.list-mod-card.selected .list-mod-icon {
  box-shadow: 0 0 0 3px var(--el-color-primary), 0 8px 24px rgba(64, 158, 255, 0.3);
  transform: scale(1.02);
}

/* 选中且已应用状态：紫色描边+发光 */
.list-mod-card.mod-selected {
  background-color: rgba(168, 85, 247, 0.1);
}

.list-mod-card.mod-selected .list-mod-icon {
  box-shadow: 0 0 0 3px #a855f7, 0 0 16px rgba(168, 85, 247, 0.5);
}

.list-mod-card.disabled .list-mod-icon {
  box-shadow: 0 0 0 2px rgba(239, 68, 68, 0.5);
  opacity: 0.5;
}

.list-mod-card.disabled.selected .list-mod-icon {
  box-shadow: 0 0 0 3px #ef4444, 0 8px 24px rgba(239, 68, 68, 0.3);
  opacity: 0.6;
}

.list-mod-card.disabled {
  opacity: 1;
  cursor: not-allowed;
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

/* ===== 统一搜索框样式 ===== */
.search-bar {
  padding: 8px;
  margin-bottom: 8px;
}

/* 搜索关键字高亮字符 */
.highlight-text {
  color: var(--el-color-primary);
  font-weight: 600;
}
</style>
