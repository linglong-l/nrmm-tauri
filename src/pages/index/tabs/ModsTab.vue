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
import { ref, computed, onMounted, onUnmounted, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { ElMessage, ElMessageBox } from 'element-plus';
import {
  Search,
  Star,
  Plus,
  Grid,
  Cpu,
  MoreFilled,
  Refresh,
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
  invokeLoadMods,
  invokeRefreshMods,
  invokeToggleModDisabled,
  invokeAddGroup,
  invokeRemoveGroup,
  invokeRenameGroup,
  invokeOpenPath,
  invokeStartFileWatcher,
  invokeStopFileWatcher
} from '../../../utils/invoke';
import { EventNames, eventManager } from '../../../utils/events';
import type { ModData, ModGroupData, LayoutMode } from '../../../types';
import { LayoutMode as LayoutModeEnum } from '../../../types';

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
const contextMenuType = ref<'group' | 'mod' | null>(null); // 右键菜单目标类型
const contextMenuData = ref<ModGroupData | ModData | null>(null); // 右键菜单目标数据
const contextMenuGroupIndex = ref(-1);                 // 右键菜单目标分组索引
const contextMenuModIndex = ref(-1);                   // 右键菜单目标模组索引
// 事件监听取消句柄；组件卸载时需调用以避免内存泄漏
let fileWatcherUnlisten: (() => void) | null = null;
let modsUpdatedUnlisten: (() => void) | null = null;
// 文件监听防抖定时器句柄；用于合并短时间内的多次刷新请求
let refreshDebounceTimer: number | null = null;

// 实际显示的分组列表：基于排序后的分组，按收藏过滤开关筛选
const displayGroups = computed(() => {
  let groups = game.sortedGroups.value;
  if (showFavoritesOnly.value) {
    groups = groups.filter(g => g.favoriteDateTime !== null);
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

// 当前生效的布局模式（来自设置）
const effectiveLayoutMode = computed((): LayoutMode => {
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

// 将枚举暴露给模板使用（避免在模板中直接 import 枚举）
const LayoutModeValues = LayoutModeEnum;

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
 * 初次加载模组列表。
 * 业务逻辑：调用后端 invokeLoadMods 获取所有分组，写入 gameStore 并标记已加载。
 * 异常处理：路径不存在时通过 ElMessage 提示用户。
 */
async function loadMods() {
  isLoading.value = true;
  try {
    const groups = await invokeLoadMods();
    game.setModGroups(groups);
    game.setModsLoaded(true);
  } catch (error) {
    console.error('Failed to load mods:', error);
    ElMessage.error(t('Mods path does not exist.'));
  } finally {
    isLoading.value = false;
  }
}

/**
 * 刷新模组列表（重新读取文件系统）。
 * 与 loadMods 的区别：refreshMods 用于文件变化后的重新扫描，不会重置 modsLoaded 标记。
 */
async function refreshMods() {
  isLoading.value = true;
  try {
    const groups = await invokeRefreshMods();
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
 * 选中指定索引的分组。
 * 业务逻辑：由于 displayGroups 可能经过过滤，需先通过 groupPath 反查原始 modGroups 中的真实索引。
 * @param index displayGroups 中的索引
 */
function selectGroup(index: number) {
  const group = displayGroups.value[index];
  if (group) {
    const realIndex = game.modGroups.value.findIndex(g => g.groupPath === group.groupPath);
    if (realIndex !== -1) {
      game.setCurrentGroupIndex(realIndex);
      selectedModIndex.value = 0;
    }
  }
}

/**
 * 选中指定索引的模组（仅更新选中状态，不切换启用状态）。
 * @param index displayMods 中的索引
 */
function selectMod(index: number) {
  selectedModIndex.value = index;
}

/**
 * 切换模组的启用/禁用状态。
 * @param mod 待切换的模组数据
 * @param modIndex 该模组在当前分组模组列表中的索引
 * 限制：realIndex === 0 的空槽位不可切换。
 */
async function toggleMod(mod: ModData, modIndex: number) {
  if (mod.realIndex === 0) return;
  try {
    const success = await invokeToggleModDisabled(mod.modPath);
    if (success) {
      // 本地乐观更新：直接反转 isDisabled，避免等待下次刷新
      const updatedMod = { ...mod, isDisabled: !mod.isDisabled };
      game.updateModInGroup(game.currentGroupIndex.value, modIndex, updatedMod);
    }
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

/**
 * 搜索输入回调：将关键字传递给 game composable 进行模组/分组过滤。
 * @param value 用户输入的搜索关键字
 */
function onSearchInput(value: string) {
  game.searchMods(value);
}

/** 清空搜索关键字并重置过滤状态 */
function clearSearch() {
  game.clearSearch();
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
    ElMessage.warning('Group name cannot be empty');
    return;
  }
  try {
    await invokeAddGroup(newGroupName.value.trim());
    await refreshMods();
    dialogAddGroupVisible.value = false;
    ElMessage.success('Group added successfully');
  } catch (error) {
    console.error('Failed to add group:', error);
    ElMessage.error('Failed to add group');
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
    ElMessage.warning('Group name cannot be empty');
    return;
  }
  const group = game.currentGroup.value;
  if (!group) return;
  try {
    await invokeRenameGroup(group.groupPath, renameGroupName.value.trim());
    await refreshMods();
    dialogRenameGroupVisible.value = false;
    ElMessage.success('Group renamed successfully');
  } catch (error) {
    console.error('Failed to rename group:', error);
    ElMessage.error('Failed to rename group');
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
    ElMessage.success('Group removed successfully');
  } catch (error) {
    if (error !== 'cancel') {
      console.error('Failed to remove group:', error);
      ElMessage.error('Failed to remove group');
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
 * @param index 目标分组在 displayGroups 中的索引
 */
function showGroupContextMenu(event: MouseEvent, group: ModGroupData, index: number) {
  event.preventDefault();
  contextMenuPosition.value = { x: event.clientX, y: event.clientY };
  contextMenuType.value = 'group';
  contextMenuData.value = group;
  contextMenuGroupIndex.value = index;
  contextMenuVisible.value = true;
}

/**
 * 显示模组右键菜单。
 * @param event 鼠标事件，用于定位菜单坐标
 * @param mod 目标模组数据
 * @param index 目标模组在 displayMods 中的索引
 * 限制：realIndex === 0 的空槽位不显示菜单。
 */
function showModContextMenu(event: MouseEvent, mod: ModData, index: number) {
  event.preventDefault();
  if (mod.realIndex === 0) return;
  contextMenuPosition.value = { x: event.clientX, y: event.clientY };
  contextMenuType.value = 'mod';
  contextMenuData.value = mod;
  contextMenuModIndex.value = index;
  contextMenuVisible.value = true;
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
function handleContextMenuSelect(command: string) {
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
    const index = contextMenuModIndex.value;
    switch (command) {
      case 'toggle':
        toggleMod(mod, index);
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
      case 'delete':
        break;
    }
  }
  hideContextMenu();
}

/**
 * 模组双击回调。
 * 业务逻辑：
 *  - 空槽位双击仅选中，不切换状态。
 *  - 普通模组双击切换启用/禁用状态。
 * @param mod 目标模组
 * @param index 目标模组在 displayMods 中的索引
 */
function onModDoubleClick(mod: ModData, index: number) {
  if (mod.realIndex === 0) {
    selectMod(index);
    return;
  }
  toggleMod(mod, index);
}

/**
 * 启动后端文件监听器，监听 Mods 目录变化。
 * 业务逻辑：仅当存在 modsPath 时启动；失败时记录错误但不阻塞。
 */
async function setupFileWatcher() {
  try {
    if (game.modsPath.value) {
      await invokeStartFileWatcher(game.modsPath.value);
    }
  } catch (error) {
    console.error('Failed to start file watcher:', error);
  }
}

/**
 * 注册前端事件监听：
 *  - FILE_WATCHER_EVENT：文件变化时触发防抖刷新。
 *  - MODS_UPDATED：后端通知模组更新时同步到 gameStore。
 * 返回值：保存取消函数以便组件卸载时清理。
 */
async function setupEventListeners() {
  fileWatcherUnlisten = await eventManager.on(EventNames.FILE_WATCHER_EVENT, () => {
    debouncedRefresh();
  });

  modsUpdatedUnlisten = await eventManager.on(EventNames.MODS_UPDATED, (groups) => {
    game.setModGroups(groups);
  });
}

// 组件挂载：依次加载模组、注册事件监听、启动文件监听；并绑定全局点击事件用于关闭右键菜单
onMounted(async () => {
  await loadMods();
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
  invokeStopFileWatcher().catch(console.error);
  document.removeEventListener('click', hideContextMenu);
  if (refreshDebounceTimer) {
    clearTimeout(refreshDebounceTimer);
  }
});

// 监听布局模式变化（占位 watcher，预留用于未来扩展，如布局切换动画等）
watch(
  () => settings.layoutMode.value,
  () => {
  }
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
      顶部工具栏区域
      数据来源：game.searchKeyword (搜索关键字), showFavoritesOnly (收藏过滤), isGridLayout/isCarouselLayout (布局模式)
      交互行为：搜索、收藏过滤、添加分组、切换布局、刷新模组列表
    -->
    <div class="mods-toolbar">
      <!-- 工具栏左侧：搜索和收藏过滤 -->
      <div class="toolbar-left">
        <!-- 
          搜索输入框
          数据来源：v-model 绑定 game.searchKeyword
          交互行为：@input 实时搜索, @clear 清空搜索, clearable 显示清空按钮
          业务逻辑：输入时调用 onSearchInput 触发搜索，清空时调用 clearSearch 重置
        -->
        <el-input
          v-model="game.searchKeyword"
          :placeholder="t('Search mod/group by name or real folder name')"
          class="search-input"
          clearable
          @input="onSearchInput"
          @clear="clearSearch"
        >
          <!-- 搜索图标前缀 -->
          <template #prefix>
            <el-icon><Search /></el-icon>
          </template>
        </el-input>
        <!-- 
          收藏过滤按钮
          数据来源：showFavoritesOnly 控制是否仅显示收藏项
          交互行为：@click 切换收藏过滤状态
          动态绑定：:type 根据 showFavoritesOnly 切换按钮类型 (warning/default)
        -->
        <el-button
          :type="showFavoritesOnly ? 'warning' : 'default'"
          :icon="Star"
          @click="showFavoritesOnly = !showFavoritesOnly"
        >
          {{ t('Favorites') }}
        </el-button>
      </div>
      <!-- 工具栏右侧：分组和模组操作 -->
      <div class="toolbar-right">
        <!-- 
          添加分组按钮
          交互行为：@click 打开新建分组对话框
        -->
        <el-button :icon="FolderAdd" @click="showAddGroupDialog">
          {{ t('Add group') }}
        </el-button>
        <!-- 
          添加模组按钮（预留功能，当前未实现）
          交互行为：@click 未绑定事件
        -->
        <el-button :icon="Plus">
          {{ t('Add mods') }}
        </el-button>
        <!-- 
          布局模式切换按钮组
          数据来源：isGridLayout/isCarouselLayout 控制按钮高亮状态
          交互行为：@click 切换布局模式
          动态绑定：:type 根据当前布局模式切换按钮类型 (primary/default)
        -->
        <el-button-group>
          <!-- 网格布局按钮 -->
          <el-button
            :type="isGridLayout ? 'primary' : 'default'"
            :icon="Grid"
            @click="settings.setLayoutMode(LayoutModeValues.Grid)"
          />
          <!-- 轮播布局按钮 -->
          <el-button
            :type="isCarouselLayout ? 'primary' : 'default'"
            :icon="Cpu"
            @click="settings.setLayoutMode(LayoutModeValues.Carousel)"
          />
        </el-button-group>
        <!-- 
          刷新按钮
          交互行为：@click 重新扫描文件系统并刷新模组列表
        -->
        <el-button :icon="Refresh" @click="refreshMods">
          {{ t('Refresh') }}
        </el-button>
        <!-- 
          更多操作下拉菜单
          交互行为：点击展开菜单，菜单项触发对应操作
        -->
        <el-dropdown>
          <!-- 下拉菜单触发按钮 -->
          <el-button :icon="MoreFilled" />
          <!-- 下拉菜单内容 -->
          <template #dropdown>
            <el-dropdown-menu>
              <!-- 刷新菜单项 -->
              <el-dropdown-item @click="refreshMods">
                <el-icon><Refresh /></el-icon>
                {{ t('Refresh') }}
              </el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
      </div>
    </div>

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
      <div class="groups-sidebar">
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
              <el-icon><Plus /></el-icon>
            </el-button>
          </div>
        </div>
        
        <!-- 
          分组列表容器
          数据来源：displayGroups 提供分组数据
          加载状态：v-loading 在 isLoading 为 true 时显示加载动画
          交互行为：每个分组项支持点击选中、右键菜单
        -->
        <div v-loading="isLoading" class="groups-list">
          <!-- 
            分组列表项
            数据来源：v-for 遍历 displayGroups，key 使用 groupPath 保证唯一性
            动态绑定：:class 根据 isGroupActive(group) 添加 active 类（高亮当前选中分组）
            交互行为：
              - @click 调用 selectGroup(index) 切换选中分组
              - @contextmenu 调用 showGroupContextMenu 打开右键菜单
          -->
          <div
            v-for="(group, index) in displayGroups"
            :key="group.groupPath"
            class="group-item"
            :class="{ active: isGroupActive(group) }"
            @click="selectGroup(index)"
            @contextmenu="showGroupContextMenu($event, group, index)"
          >
            <!-- 
              分组图标区域
              数据来源：group.iconPath 存在时显示自定义图标，否则显示默认文件夹图标
              条件渲染：v-if/v-else 根据 iconPath 是否存在切换显示内容
            -->
            <div class="group-icon">
              <!-- 自定义分组图标 -->
              <img v-if="group.iconPath" :src="group.iconPath" alt="group icon" />
              <!-- 默认文件夹图标 -->
              <el-icon v-else><FolderOpened /></el-icon>
            </div>
            <!-- 
              分组信息区域
              数据来源：group.groupName (分组名称), group.modsInGroup.length (模组数量)
            -->
            <div class="group-info">
              <!-- 分组名称 -->
              <span class="group-name">{{ group.groupName }}</span>
              <!-- 模组数量统计 -->
              <span class="group-count">{{ group.modsInGroup.length }} {{ t('Mods') }}</span>
            </div>
            <!-- 
              分组收藏图标
              数据来源：group.favoriteDateTime 存在时显示（表示已收藏）
              条件渲染：v-if 根据 favoriteDateTime 是否存在决定是否显示
              样式：金色图标 (#f59e0b)
            -->
            <el-icon
              v-if="group.favoriteDateTime"
              class="group-favorite"
              color="#f59e0b"
            >
              <Star />
            </el-icon>
          </div>
        </div>
        <!-- 
          空状态提示
          条件渲染：v-if 在 !isLoading && displayGroups.length === 0 时显示
          数据来源：isLoading (加载状态), displayGroups.length (分组数量)
          作用：当没有分组时提示用户添加分组
        -->
        <el-empty
          v-if="!isLoading && displayGroups.length === 0"
          :description="t('Right-click and add group, then you can add mods.')"
          :image-size="80"
        />
      </div>

      <!-- 
        模组展示区域
        数据来源：displayMods (经过排序和收藏过滤后的模组列表), isLoading (加载状态)
        布局：根据 effectiveLayoutMode 切换网格布局或轮播布局
        交互行为：点击选中模组，双击切换启用/禁用，右键打开模组菜单
      -->
      <div class="mods-display">
        <!-- 
          模组容器
          加载状态：v-loading 在 isLoading 为 true 时显示加载动画
        -->
        <div v-loading="isLoading" class="mods-container">
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
              <div
                v-for="(mod, index) in displayMods"
                :key="mod.modPath"
                class="mod-card"
                :class="{
                  selected: selectedModIndex === index,
                  disabled: mod.isDisabled,
                  'none-slot': mod.realIndex === 0
                }"
                @click="selectMod(index)"
                @dblclick="onModDoubleClick(mod, index)"
                @contextmenu="showModContextMenu($event, mod, index)"
              >
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
                  <img v-if="mod.iconPath" :src="mod.iconPath" alt="mod icon" />
                  <!-- 空槽位图标 -->
                  <el-icon v-else-if="mod.realIndex === 0"><Close /></el-icon>
                  <!-- 默认查看图标 -->
                  <el-icon v-else><View /></el-icon>
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
                  <el-tooltip v-if="mod.isOldAutoFixed" :content="t('Mod syntax errors were auto-fixed by earlier NRMM versions.')">
                    <el-icon class="status-icon warning"><MagicStick /></el-icon>
                  </el-tooltip>
                  <!-- 
                    语法错误已移除图标
                    条件渲染：v-if="mod.isSyntaxErrorRemoved"
                    提示内容：显示 tooltip 说明"语法错误已被自动移除"
                    样式：info 类，蓝色图标
                  -->
                  <el-tooltip v-if="mod.isSyntaxErrorRemoved" :content="t('Mod syntax errors are automatically removed.')">
                    <el-icon class="status-icon info"><Operation /></el-icon>
                  </el-tooltip>
                  <!-- 
                    未优化模组图标
                    条件渲染：v-if="mod.isUnoptimized"
                    提示内容：显示 tooltip 说明"模组未优化，可能影响性能或破坏其他模组"
                    样式：warning 类，橙色图标
                  -->
                  <el-tooltip v-if="mod.isUnoptimized" :content="t('Mod is unoptimized and might slow down performance or even break other mods.')">
                    <el-icon class="status-icon warning"><Timer /></el-icon>
                  </el-tooltip>
                  <!-- 
                    命名空间模组图标
                    条件渲染：v-if="mod.isNamespaced"
                    提示内容：显示 tooltip 说明"模组使用命名空间"
                    样式：info 类，蓝色图标
                  -->
                  <el-tooltip v-if="mod.isNamespaced" :content="t('Mod uses namespaces')">
                    <el-icon class="status-icon info"><Cpu /></el-icon>
                  </el-tooltip>
                </div>
                <!-- 
                  模组收藏图标
                  数据来源：mod.favoriteDateTime 存在且 mod.realIndex !== 0 时显示
                  条件渲染：v-if 根据收藏状态和是否为空槽位决定是否显示
                  交互行为：@click.stop 阻止事件冒泡，调用 toggleModFavorite(mod) 切换收藏状态
                  样式：金色图标 (#f59e0b)，绝对定位在右上角
                -->
                <el-icon
                  v-if="mod.favoriteDateTime && mod.realIndex !== 0"
                  class="mod-favorite"
                  color="#f59e0b"
                  @click.stop="toggleModFavorite(mod)"
                >
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
            轮播布局模式
            条件渲染：v-else-if="isCarouselLayout" 当布局模式为 Carousel 时显示
            布局方式：Element Plus Carousel 组件，大卡片轮播展示
          -->
          <template v-else-if="isCarouselLayout">
            <!-- 
              轮播容器
              数据来源：displayMods.length > 0 时显示，否则不渲染
              双向绑定：:model-value 绑定 selectedModIndex（当前选中索引）
              交互行为：@change 调用 selectMod 切换选中模组
              样式：固定高度 400px
            -->
            <el-carousel
              v-if="displayMods.length > 0"
              :model-value="selectedModIndex"
              height="400px"
              @change="selectMod"
            >
              <!-- 
                轮播项
                数据来源：v-for 遍历 displayMods，key 使用 modPath
              -->
              <el-carousel-item v-for="(mod, index) in displayMods" :key="mod.modPath">
                <!-- 
                  轮播模组卡片
                  动态绑定：
                    - :class 根据 mod.isDisabled 添加 disabled 类
                    - :class 根据 mod.realIndex === 0 添加 none-slot 类
                  交互行为：
                    - @dblclick 调用 onModDoubleClick 切换启用/禁用
                    - @contextmenu 调用 showModContextMenu 打开右键菜单
                -->
                <div
                  class="carousel-mod-card"
                  :class="{
                    disabled: mod.isDisabled,
                    'none-slot': mod.realIndex === 0
                  }"
                  @dblclick="onModDoubleClick(mod, index)"
                  @contextmenu="showModContextMenu($event, mod, index)"
                >
                  <!-- 
                    轮播模组图标区域
                    数据来源：同网格布局的图标逻辑
                  -->
                  <div class="carousel-mod-icon">
                    <img v-if="mod.iconPath" :src="mod.iconPath" alt="mod icon" />
                    <el-icon v-else-if="mod.realIndex === 0"><Close /></el-icon>
                    <el-icon v-else><View /></el-icon>
                  </div>
                  <!-- 
                    轮播模组名称
                    数据来源：mod.modName
                    样式：大字号，居中显示
                  -->
                  <div class="carousel-mod-name">{{ mod.modName }}</div>
                  <!-- 
                    轮播模组状态标签区域
                    数据来源：mod.isDisabled, mod.favoriteDateTime
                    作用：显示启用/禁用状态和收藏状态
                  -->
                  <div class="carousel-mod-status">
                    <!-- 
                      禁用/启用状态标签
                      条件渲染：v-if/v-else 根据 mod.isDisabled 切换显示
                      样式：禁用显示 danger 类型（红色），启用显示 success 类型（绿色）
                    -->
                    <el-tag v-if="mod.isDisabled" type="danger">{{ t('Disabled') }}</el-tag>
                    <el-tag v-else type="success">{{ t('Enabled') }}</el-tag>
                    <!-- 
                      收藏状态标签
                      条件渲染：v-if 根据 mod.favoriteDateTime 存在且非空槽位时显示
                      样式：warning 类型（金色），带星标图标
                    -->
                    <el-tag v-if="mod.favoriteDateTime && mod.realIndex !== 0" type="warning">
                      <el-icon><Star /></el-icon>
                    </el-tag>
                  </div>
                </div>
              </el-carousel-item>
            </el-carousel>
          </template>

          <!-- 
            空状态提示
            条件渲染：v-if 在 !isLoading && displayMods.length === 0 时显示
            数据来源：isLoading (加载状态), displayMods.length (模组数量)
            作用：当没有模组时提示用户
          -->
          <el-empty
            v-if="!isLoading && displayMods.length === 0"
            :description="t('No mods found')"
            :image-size="100"
          />
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
    <div
      v-if="contextMenuVisible"
      class="context-menu"
      :style="{ left: contextMenuPosition.x + 'px', top: contextMenuPosition.y + 'px' }"
      @click.stop
    >
      <!-- 
        分组右键菜单
        条件渲染：v-if="contextMenuType === 'group'" 当右键点击分组时显示
        数据来源：contextMenuData 为 ModGroupData 类型
        交互行为：每个菜单项点击后调用 handleContextMenuSelect 并传入命令标识
      -->
      <template v-if="contextMenuType === 'group'">
        <!-- 
          重命名菜单项
          交互行为：@click 调用 handleContextMenuSelect('rename')
          图标：Edit 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('rename')">
          <el-icon><Edit /></el-icon>
          {{ t('Rename') }}
        </div>
        <!-- 
          收藏/取消收藏菜单项
          交互行为：@click 调用 handleContextMenuSelect('favorite')
          动态文本：根据 favoriteDateTime 是否存在显示"Favorite"或"Unfavorite"
          图标：Star 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('favorite')">
          <el-icon><Star /></el-icon>
          {{ (contextMenuData as ModGroupData)?.favoriteDateTime ? 'Unfavorite' : 'Favorite' }}
        </div>
        <!-- 
          在文件管理器中打开菜单项
          交互行为：@click 调用 handleContextMenuSelect('open')
          图标：FolderOpened 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('open')">
          <el-icon><FolderOpened /></el-icon>
          {{ t('Open in File Explorer') }}
        </div>
        <!-- 
          删除分组菜单项
          交互行为：@click 调用 handleContextMenuSelect('delete')
          样式：danger 类，红色文字，表示危险操作
          图标：Delete 图标
        -->
        <div class="context-menu-item danger" @click="handleContextMenuSelect('delete')">
          <el-icon><Delete /></el-icon>
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
          启用/禁用模组菜单项
          交互行为：@click 调用 handleContextMenuSelect('toggle')
          动态文本：根据 mod.isDisabled 显示"Enable mod"或"Disable mod completely"
          图标：View 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('toggle')">
          <el-icon><View /></el-icon>
          {{ (contextMenuData as ModData)?.isDisabled ? t('Enable mod') : t('Disable mod completely') }}
        </div>
        <!-- 
          收藏/取消收藏菜单项
          交互行为：@click 调用 handleContextMenuSelect('favorite')
          动态文本：根据 favoriteDateTime 是否存在显示"Favorite"或"Unfavorite"
          图标：Star 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('favorite')">
          <el-icon><Star /></el-icon>
          {{ (contextMenuData as ModData)?.favoriteDateTime ? 'Unfavorite' : 'Favorite' }}
        </div>
        <!-- 
          重命名菜单项
          交互行为：@click 调用 handleContextMenuSelect('rename')
          图标：Edit 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('rename')">
          <el-icon><Edit /></el-icon>
          {{ t('Rename') }}
        </div>
        <!-- 
          在文件管理器中打开菜单项
          交互行为：@click 调用 handleContextMenuSelect('open')
          图标：FolderOpened 图标
        -->
        <div class="context-menu-item" @click="handleContextMenuSelect('open')">
          <el-icon><FolderOpened /></el-icon>
          {{ t('Open in File Explorer') }}
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
    <el-dialog
      v-model="dialogAddGroupVisible"
      :title="t('Add group')"
      width="400px"
    >
      <!-- 分组名称输入框 -->
      <el-input
        v-model="newGroupName"
        :placeholder="t('Group Name')"
        @keyup.enter="handleAddGroup"
      />
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
    <el-dialog
      v-model="dialogRenameGroupVisible"
      :title="t('Rename')"
      width="400px"
    >
      <!-- 分组名称输入框 -->
      <el-input
        v-model="renameGroupName"
        :placeholder="t('Group Name')"
        @keyup.enter="handleRenameGroup"
      />
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
    <el-dialog
      v-model="dialogRenameModVisible"
      :title="t('Rename')"
      width="400px"
    >
      <!-- 模组名称输入框 -->
      <el-input
        v-model="renameModName"
        :placeholder="t('Mod Name')"
      />
      <!-- 对话框底部按钮区域 -->
      <template #footer>
        <!-- 取消按钮 -->
        <el-button @click="dialogRenameModVisible = false">{{ t('Cancel') }}</el-button>
        <!-- 确认按钮（预留功能） -->
        <el-button type="primary">{{ t('Confirm') }}</el-button>
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
  width: 200px;
  display: flex;
  flex-direction: column;
  overflow-y: auto;
  flex-shrink: 0;
  padding: 8px;
  gap: 8px;
  border-right: 1px solid rgba(255, 255, 255, 0.04);
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

.mods-container {
  flex: 1;
  overflow-y: auto;
  padding: 16px;
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
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
}

.mod-card.selected .mod-icon {
  box-shadow: 0 0 0 2px var(--el-color-primary), 0 8px 24px rgba(64, 158, 255, 0.2);
}

.mod-card.disabled {
  opacity: 0.5;
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
  position: absolute;
  top: 8px;
  left: 8px;
  padding: 2px 8px;
  font-size: 11px;
  background-color: var(--el-color-danger);
  color: white;
  border-radius: 4px;
  font-weight: 500;
}

.carousel-mod-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 16px;
  height: 100%;
  padding: 24px;
}

.carousel-mod-card.disabled {
  opacity: 0.6;
}

.carousel-mod-icon {
  width: 200px;
  height: 200px;
  border-radius: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  background-color: rgba(255, 255, 255, 0.06);
}

.carousel-mod-icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.carousel-mod-icon :deep(.el-icon) {
  font-size: 80px;
  color: rgba(255, 255, 255, 0.2);
}

.carousel-mod-name {
  font-size: 20px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.85);
  text-align: center;
}

.carousel-mod-status {
  display: flex;
  gap: 8px;
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

.context-menu-item :deep(.el-icon) {
  font-size: 16px;
}
</style>
