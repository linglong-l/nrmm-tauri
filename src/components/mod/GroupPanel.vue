<template>
  <div class="group-panel" @click="handlePanelClick">
    <!-- 分组列表：可拖拽滚动 -->
    <div ref="groupListRef" class="group-list" @contextmenu.prevent="onContextMenuEmpty">
      <!-- 递归树节点：遍历顶层分组（已将NormalGroup收拢到虚拟"Groups"节点下） -->
      <GroupTreeNode
        v-for="(group, index) in groupedTree"
        :key="group.groupPath || index"
        :group="group"
        :depth="0"
        :selected-group-path="selectedGroupPath"
        :default-expanded="isGroupExpanded(group.groupPath)"
        :search-query="modsStore.searchQuery"
        :is-group-hit="groupHitMap.get(group.groupPath)"
        :collapse-path="collapsePath"
        @select="handleSelectGroup"
        @context-menu="handleContextMenu"
      />

      <!-- 底部添加分组按钮（虚线圆形） -->
      <button
        type="button"
        class="add-group-btn"
        :title="t('mods.addGroup')"
        @click="handleAddGroupWithDialog"
      >
        <el-icon :size="18"><Plus /></el-icon>
      </button>
    </div>

    <!-- 右键菜单：添加/重命名/打开文件夹/删除分组（Teleport到body避免fixed漂移） -->
    <Teleport to="body">
      <div
        v-if="contextMenuVisible"
        class="context-menu"
        :style="{ top: contextMenuY + 'px', left: contextMenuX + 'px' }"
        @click.stop
      >
        <div class="menu-item" @click="handleContextMenuAction('addGroup')">
          <el-icon><Plus /></el-icon>
          {{ t('mods.addGroup') }}
        </div>
        <div v-if="contextGroup && isMutexGroup(contextGroup)" class="menu-item" @click="handleContextMenuAction('addSubGroup')">
          <el-icon><FolderAdd /></el-icon>
          {{ t('mods.addSubfolder') }}
        </div>
        <div class="menu-divider" v-if="contextGroup"></div>
        <div v-if="contextGroup" class="menu-item" @click="handleContextMenuAction('renameGroup')">
          <el-icon><Edit /></el-icon>
          {{ t('common.rename') }}
        </div>
        <div v-if="contextGroup" class="menu-item" @click="handleContextMenuAction('openGroupFolder')">
          <el-icon><FolderOpened /></el-icon>
          {{ t('Open in File Explorer') }}
        </div>
        <!-- 收起分组：右键深层分组收起其父级分组整棵子树（分组过多时可释放空间查看其他分组） -->
        <div v-if="contextGroup && contextGroupCollapsible" class="menu-item" @click="handleContextMenuAction('collapseGroup')">
          <el-icon><Fold /></el-icon>
          {{ t('common.collapse') }}
        </div>
        <div class="menu-divider" v-if="contextGroup"></div>
        <div v-if="contextGroup" class="menu-item" @click="handleContextMenuAction('disableAllMods')">
          <el-icon><VideoPause /></el-icon>
          {{ t('Disable all mods') }}
        </div>
        <div v-if="contextGroup" class="menu-item" @click="handleContextMenuAction('enableAllMods')">
          <el-icon><VideoPlay /></el-icon>
          {{ t('Enable all mods') }}
        </div>
        <div class="menu-divider" v-if="contextGroup"></div>
        <div v-if="contextGroup" class="menu-item danger" @click="handleContextMenuAction('removeGroup')">
          <el-icon><Delete /></el-icon>
          {{ t('Remove group') }}
        </div>
      </div>
    </Teleport>
  </div>
</template>

<script setup lang="ts">
/**
 * 分组面板组件
 * 左侧导航栏，以圆形头像树状列表展示所有模组分组
 * 支持：递归树结构、点击选中分组、展开/折叠子分组、右键菜单、拖拽滚动
 */
import { ref, computed, onMounted, onUnmounted, watch, inject, nextTick } from 'vue'
import type { Ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Plus, Edit, Delete, FolderOpened, FolderAdd, VideoPause, VideoPlay, Fold } from '@element-plus/icons-vue'
import type { ModGroupData } from '@/types'
import { useModsStore } from '@/stores/mods'
import { useGroupContextMenu } from '@/composables/useGroupContextMenu'
import { useDragScroll } from '@/composables/useDragScroll'
import GroupTreeNode from './GroupTreeNode.vue'

const { t } = useI18n()
const modsStore = useModsStore()

/** 分组列表DOM引用，用于拖拽滚动 */
const groupListRef = ref<HTMLElement | null>(null)
useDragScroll(groupListRef)

/**
 * 模组页焦点令牌（由 App.vue provide）
 * 窗口显示或切回模组页时自增，触发选中分组居中滚动
 */
const modsFocusTick = inject<Ref<number>>('modsFocusTick', ref(0))

/** 分组列表计算属性：从modsStore获取 */
const groups = computed(() => modsStore.groups)
/** 当前选中分组路径 */
const selectedGroupPath = computed(() => modsStore.selectedGroupPath)

/**
 * 角色分组（group_int 即 NormalGroup + ExclusiveSlot + CustomParallel）
 * 收拢到虚拟"Groups"节点下，与 MutexGroup 独立并列，便于用户区分角色模组和其他模组。
 * 虚拟Groups节点默认展开且不响应选择/右键菜单（因为并非真实存在的目录）。
 */
const groupedTree = computed<ModGroupData[]>(() => {
  const rootGroups = groups.value || []
  const roleGroups: ModGroupData[] = []
  const otherGroups: ModGroupData[] = []
  for (const g of rootGroups) {
    if (g.groupType === 'normalGroup' || g.groupType === 'exclusiveSlot' || g.groupType === 'customParallel') {
      roleGroups.push(g)
    } else {
      otherGroups.push(g)
    }
  }
  const result: ModGroupData[] = []
  if (roleGroups.length > 0) {
    const virtualGroupsRoot: ModGroupData = {
      groupPath: '__VIRTUAL_GROUPS__',
      groupName: 'Groups',
      name: 'Groups',
      groupId: -1001,
      groupIndex: -1,
      mods: [],
      modCount: roleGroups.reduce((sum, g) => sum + (g.modCount || 0), 0),
      isActive: false,
      isFavorite: false,
      groupDisabled: false,
      groupType: 'normalGroup',
      hasChild: true,
      children: roleGroups,
      activeModIndex: -1,
      isVirtualRoot: true,
    }
    result.push(virtualGroupsRoot)
  }
  for (const g of otherGroups) {
    result.push(g)
  }
  return result
})

/** 各顶层分组的搜索命中状态（computed 仅在 groupedTree / 搜索词变化时重算，避免模板每帧调用 isGroupMatch） */
const groupHitMap = computed(() => {
  const map = new Map<string, boolean>()
  for (const g of groupedTree.value) {
    map.set(g.groupPath, modsStore.isGroupMatch(g.groupPath))
  }
  return map
})

/**
 * 默认虚拟"Groups"根节点展开状态集合（path -> isExpanded）。
 * 虚拟根默认展开，避免角色分组默认被折叠不可见。
 */
const expandedMap = ref<Map<string, boolean>>(new Map([['__VIRTUAL_GROUPS__', true]]))
/** 获取分组展开状态，虚拟根默认展开；搜索命中的祖先路径强制展开 */
function isGroupExpanded(groupPath: string): boolean {
  const hasQuery = !!modsStore.searchQuery?.trim()
  // 搜索时虚拟Groups根始终展开（因为其下的角色分组可能命中需要可见）
  if (hasQuery && groupPath === '__VIRTUAL_GROUPS__') return true
  const autoExpanded = hasQuery && !!modsStore.autoExpandGroupPaths?.has(groupPath)
  if (autoExpanded) return true
  if (!expandedMap.value.has(groupPath)) {
    expandedMap.value.set(groupPath, groupPath === '__VIRTUAL_GROUPS__')
  }
  return expandedMap.value.get(groupPath) === true
}

const {
  contextMenuVisible,
  contextMenuX,
  contextMenuY,
  contextGroup,
  contextGroupCollapsible,
  collapsePath,
  isMutexGroup,
  handleSelectGroup,
  handleContextMenu,
  onContextMenuEmpty,
  closeContextMenu,
  handleContextMenuAction,
  handleAddGroupWithDialog,
} = useGroupContextMenu(groupedTree, modsStore, t)

/** 点击外部区域关闭右键菜单 */
function handleClickOutside() {
  closeContextMenu()
}

/**
 * 点击左侧导航栏容器时关闭搜索框
 * 分组节点点击通过 stopPropagation 阻止冒泡，不触发关闭
 */
function handlePanelClick() {
  if (modsStore.searchVisible) {
    modsStore.setSearchVisible(false)
    modsStore.clearSearch()
  }
}

/**
 * 将当前选中的分组滚动到可视范围中央（尽量居中）
 * 选中分组节点为 .group-item.active（可能位于任意递归深度）
 * scrollIntoView({ block: 'center' }) 在无法真正居中时（靠近顶部/底部）会自动滚动到最接近位置
 */
function scrollToSelectedGroup() {
  const container = groupListRef.value
  if (!container) return
  const active = container.querySelector<HTMLElement>('.group-item.active')
  if (active) {
    active.scrollIntoView({ behavior: 'smooth', block: 'center' })
  }
}

onMounted(() => {
  document.addEventListener('click', handleClickOutside)
  // 挂载后（分组数据可能尚未加载完成，selectedGroupPath变化时也会再次触发）
  nextTick(scrollToSelectedGroup)
})

/**
 * 焦点回归 / 选中分组变化 / 分组数量变化时，确保选中分组居中
 * - selectedGroupPath 变化：用户切换分组
 * - groups.length 变化：分组数据首次加载完成（此时 selectedGroupPath 可能未变）
 * - modsFocusTick 变化：窗口显示或切回模组页（焦点回归）
 */
watch(
  () => [selectedGroupPath.value, groups.value.length, modsFocusTick.value],
  () => {
    nextTick(scrollToSelectedGroup)
  }
)

onUnmounted(() => {
  document.removeEventListener('click', handleClickOutside)
})
</script>

<style scoped>
.group-panel {
  width: 88px;
  min-width: 88px;
  height: 100%;
  background: transparent;
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 8px 0;
  gap: 4px;
}

.groups-header {
  width: 100%;
  padding: 8px 0;
  display: flex;
  justify-content: center;
  flex-shrink: 0;
}

.groups-title {
  font-size: 10px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.4);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  writing-mode: vertical-rl;
  text-orientation: mixed;
}

.group-list {
  flex: 1;
  width: 100%;
  overflow-y: auto;
  overflow-x: hidden;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  padding: 4px 0;
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.group-list::-webkit-scrollbar {
  display: none;
}

.add-group-btn {
  width: 44px;
  height: 44px;
  border-radius: 50%;
  border: 2px dashed rgba(255, 255, 255, 0.15);
  background: transparent;
  color: rgba(255, 255, 255, 0.3);
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  margin-top: 8px;
  flex-shrink: 0;
  transition: all 0.2s ease;
  padding: 0;
}

.add-group-btn:hover {
  border-color: rgba(74, 158, 255, 0.5);
  color: #4a9eff;
  background: rgba(74, 158, 255, 0.1);
}

.add-group-btn:focus-visible {
  outline: 2px solid var(--accent-primary);
  outline-offset: 2px;
}

.context-menu {
  position: fixed;
  background: rgba(30, 30, 30, 0.96);
  backdrop-filter: blur(20px) saturate(180%);
  -webkit-backdrop-filter: blur(20px) saturate(180%);
  border: 1px solid rgba(255, 255, 255, 0.10);
  border-radius: 8px;
  padding: 4px 0;
  min-width: 160px;
  z-index: 10000;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
}

.menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  font-size: 13px;
  color: #eee;
  cursor: pointer;
  transition: background-color 0.1s ease;
}

.menu-item:hover {
  background: rgba(74, 158, 255, 0.16);
}

.menu-item:focus-visible {
  outline: 2px solid var(--accent-primary);
  outline-offset: -2px;
}

.menu-item.danger {
  color: #e74c3c;
}

.menu-item.danger:hover {
  background: rgba(231, 76, 60, 0.12);
}

.menu-divider {
  height: 1px;
  background: rgba(255, 255, 255, 0.08);
  margin: 4px 0;
}
</style>
