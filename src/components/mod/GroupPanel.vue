<template>
  <div class="group-panel">
    <!-- 分组标题：竖排"Groups"文字 -->
    <div class="groups-header">
      <span class="groups-title">{{ t('common.groups', 'Groups') }}</span>
    </div>
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
        @select="handleSelectGroup"
        @context-menu="handleContextMenu"
      />

      <!-- 底部添加分组按钮（虚线圆形） -->
      <button
        type="button"
        class="add-group-btn"
        :title="t('mods.addGroup')"
        @click="handleAddGroup"
      >
        <el-icon :size="18"><Plus /></el-icon>
      </button>
    </div>

    <!-- 右键菜单：添加/重命名/打开文件夹/删除分组 -->
    <div
      v-if="contextMenuVisible"
      class="context-menu"
      :style="{ top: contextMenuY + 'px', left: contextMenuX + 'px' }"
      @click.stop
    >
      <div class="menu-item" @click="handleAddGroup">
        <el-icon><Plus /></el-icon>
        {{ t('mods.addGroup') }}
      </div>
      <div v-if="contextGroup && isMutexGroup(contextGroup)" class="menu-item" @click="handleAddSubfolder">
        <el-icon><FolderAdd /></el-icon>
        {{ t('mods.addSubfolder') }}
      </div>
      <div class="menu-divider" v-if="contextGroup"></div>
      <div v-if="contextGroup" class="menu-item" @click="handleRename">
        <el-icon><Edit /></el-icon>
        {{ t('common.rename') }}
      </div>
      <div v-if="contextGroup" class="menu-item" @click="handleOpenFolder">
        <el-icon><FolderOpened /></el-icon>
        {{ t('Open in File Explorer') }}
      </div>
      <div class="menu-divider" v-if="contextGroup"></div>
      <div v-if="contextGroup" class="menu-item danger" @click="handleDelete">
        <el-icon><Delete /></el-icon>
        {{ t('Remove group') }}
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 分组面板组件
 * 左侧导航栏，以圆形头像树状列表展示所有模组分组
 * 支持：递归树结构、点击选中分组、展开/折叠子分组、右键菜单、拖拽滚动
 */
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { Plus, Edit, Delete, FolderOpened, FolderAdd } from '@element-plus/icons-vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import type { ModGroupData } from '@/types'
import { addGroup, renameGroup, removeGroup, openGroupFolder, validateSubfolderName, createSubfolder } from '@/utils/tauri'
import { useModsStore } from '@/stores/mods'
import { useSettingsStore } from '@/stores/settings'
import { useDragScroll } from '@/composables/useDragScroll'
import GroupTreeNode from './GroupTreeNode.vue'

const { t } = useI18n()
const modsStore = useModsStore()
const settingsStore = useSettingsStore()

/** 分组列表DOM引用，用于拖拽滚动 */
const groupListRef = ref<HTMLElement | null>(null)
useDragScroll(groupListRef)

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
      childGroups: roleGroups,
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

/**
 * 默认虚拟"Groups"根节点展开状态集合（path -> isExpanded）。
 * 虚拟根默认展开，避免角色分组默认被折叠不可见。
 */
const expandedMap = ref<Map<string, boolean>>(new Map([['__VIRTUAL_GROUPS__', true]]))
/** 获取分组展开状态，虚拟根默认展开 */
function isGroupExpanded(groupPath: string): boolean {
  if (!expandedMap.value.has(groupPath)) {
    expandedMap.value.set(groupPath, groupPath === '__VIRTUAL_GROUPS__')
  }
  return expandedMap.value.get(groupPath) === true
}

/** 右键菜单是否可见 */
const contextMenuVisible = ref(false)
/** 右键菜单位置X */
const contextMenuX = ref(0)
/** 右键菜单位置Y */
const contextMenuY = ref(0)
/** 右键菜单关联的分组数据 */
const contextGroup = ref<ModGroupData | null>(null)

/**
 * 选中分组
 * @param group 选中的分组对象
 */
function handleSelectGroup(group: ModGroupData) {
  modsStore.selectGroup(group)
}

/**
 * 分组项右键菜单
 * @param payload 事件和分组数据
 */
function handleContextMenu(payload: { event: MouseEvent; group: ModGroupData }) {
  contextGroup.value = payload.group
  const e = payload.event
  // 溢出检测：菜单估算尺寸 160×192 保守值
  const MENU_ESTIMATED_W = 180
  const MENU_ESTIMATED_H = 220
  const vw = window.innerWidth
  const vh = window.innerHeight
  let x = e.clientX
  let y = e.clientY
  if (x + MENU_ESTIMATED_W > vw - 4) {
    x = Math.max(4, x - MENU_ESTIMATED_W)
  }
  if (y + MENU_ESTIMATED_H > vh - 4) {
    y = Math.max(4, y - MENU_ESTIMATED_H)
  }
  contextMenuX.value = x
  contextMenuY.value = y
  contextMenuVisible.value = true
}

/**
 * 空白区域右键菜单（仅显示添加分组选项）
 * @param e 鼠标事件
 */
function onContextMenuEmpty(e: MouseEvent) {
  contextGroup.value = null
  const MENU_ESTIMATED_W = 180
  const MENU_ESTIMATED_H = 64
  const vw = window.innerWidth
  const vh = window.innerHeight
  let x = e.clientX
  let y = e.clientY
  if (x + MENU_ESTIMATED_W > vw - 4) {
    x = Math.max(4, x - MENU_ESTIMATED_W)
  }
  if (y + MENU_ESTIMATED_H > vh - 4) {
    y = Math.max(4, y - MENU_ESTIMATED_H)
  }
  contextMenuX.value = x
  contextMenuY.value = y
  contextMenuVisible.value = true
}

/** 关闭右键菜单 */
function closeContextMenu() {
  contextMenuVisible.value = false
  contextGroup.value = null
}

/**
 * 添加新分组
 * 弹出输入框让用户输入分组名，调用后端addGroup命令
 */
async function handleAddGroup() {
  closeContextMenu()
  if (!settingsStore.currentModsPath) {
    ElMessage.warning(t('Please select a group first'))
    return
  }

  try {
    const { value } = await ElMessageBox.prompt(
      t('Group Name'),
      t('mods.addGroup'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        inputValidator: (val) => {
          if (val && val.trim().length > 0) return true
          return true
        }
      }
    )
    const groupName = value || undefined
    await addGroup(settingsStore.currentGame, settingsStore.currentModsPath, groupName)
    await modsStore.refresh()
    ElMessage.success(t('Group added successfully'))
  } catch (e: any) {
    if (e !== 'cancel' && e !== 'close') {
      ElMessage.error(t('Failed to add group') + ': ' + (e?.message || e))
    }
  }
}

/** 重命名分组 */
async function handleRename() {
  if (!contextGroup.value) return
  closeContextMenu()
  try {
    const { value } = await ElMessageBox.prompt(
      t('Group Name'),
      t('common.rename'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        inputValue: contextGroup.value.name || contextGroup.value.groupName,
        inputValidator: (val) => {
          if (!val || !val.trim()) return t('Group name cannot be empty')
          return true
        }
      }
    )
    if (value && contextGroup.value.groupPath) {
      await renameGroup(contextGroup.value.groupPath, value.trim())
      await modsStore.refresh()
      ElMessage.success(t('Group renamed successfully'))
    }
  } catch (e) {
  }
}

/** 在文件管理器中打开分组文件夹 */
async function handleOpenFolder() {
  if (!contextGroup.value) return
  closeContextMenu()
  try {
    await openGroupFolder(contextGroup.value.groupPath)
  } catch (e: any) {
    ElMessage.error(t('Failed to open folder') + ': ' + (e?.message || e))
  }
}

/**
 * 删除分组
 * 弹出确认对话框，确认后调用后端removeGroup命令
 */
async function handleDelete() {
  if (!contextGroup.value) return
  closeContextMenu()
  try {
    await ElMessageBox.confirm(
      t('Removing group will make the mods inside the group can be used again without mod manager.'),
      t('Warning'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        type: 'warning'
      }
    )
    if (contextGroup.value.groupPath) {
      await removeGroup(contextGroup.value.groupPath)
      await modsStore.refresh()
      ElMessage.success(t('Group removed successfully'))
    }
  } catch (e) {
  }
}

/**
 * 判断分组是否为互斥组（非group_int的自定义分组，支持子分组）
 * @param group 分组数据
 * @returns 是否为互斥组
 */
function isMutexGroup(group: ModGroupData): boolean {
  return group.groupType === 'mutexGroup'
}

/**
 * 添加子分组（仅对mutexGroup类型分组可用）
 * 流程：输入名称→前端实时校验→后端二次校验→二次确认→创建目录→刷新
 */
async function handleAddSubfolder() {
  if (!contextGroup.value) return
  closeContextMenu()

  const parentGroup = contextGroup.value
  const parentPath = parentGroup.groupPath
  const parentDisplayName = parentGroup.name || parentGroup.groupName

  // 非法字符集合（Windows）
  const illegalCharsRegex = /[\\/:*?"<>|\x00-\x1F]/g

  try {
    // 步骤1：弹出输入框，带实时校验
    const { value } = await ElMessageBox.prompt(
      t('mods.subfolderName'),
      t('mods.addSubfolder'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        inputPlaceholder: t('mods.subfolderName'),
        inputValue: '',
        showClose: true,
        inputValidator: (val) => {
          if (!val || !val.trim()) {
            return t('Group name cannot be empty')
          }
          const trimmed = val.trim()
          if (trimmed === '.' || trimmed === '..') {
            return '文件夹名称不能为 "." 或 ".."'
          }
          const matches = trimmed.match(illegalCharsRegex)
          if (matches) {
            const uniqueChars = [...new Set(matches)].join(' ')
            return `目录名包含非法字符: ${uniqueChars}`
          }
          if (trimmed.endsWith('.') || trimmed.endsWith(' ')) {
            return '文件夹名称末尾不能是点或空格'
          }
          return true
        }
      }
    )

    const inputName = (value || '').trim()
    if (!inputName) return

    // 步骤2：调用后端二次校验
    const [sanitizedName, isValid, errorMsg] = await validateSubfolderName(parentPath, inputName)
    if (!isValid) {
      ElMessage.error(errorMsg)
      return
    }

    // 步骤3：二次确认对话框
    await ElMessageBox.confirm(
      t('mods.confirmCreateSubfolder', { groupName: parentDisplayName, folderName: sanitizedName }),
      t('mods.addSubfolder'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        type: 'info',
        showClose: true,
      }
    )

    // 步骤4：调用后端创建子文件夹
    await createSubfolder(parentPath, sanitizedName)

    // 步骤5：创建成功，刷新列表
    ElMessage.success(t('mods.subfolderCreated'))
    await modsStore.refresh()

  } catch (e: any) {
    if (e !== 'cancel' && e !== 'close') {
      ElMessage.error(t('mods.failedToCreateSubfolder') + ': ' + (e?.message || e))
    }
  }
}

/** 点击外部区域关闭右键菜单 */
function handleClickOutside() {
  closeContextMenu()
}

onMounted(() => {
  document.addEventListener('click', handleClickOutside)
})

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
  z-index: 9999;
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
