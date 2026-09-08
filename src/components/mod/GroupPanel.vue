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
        @click="handleAddGroup"
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
import type { ModData, ModGroupData } from '@/types'
import { addGroup, renameGroup, removeGroupEx, openGroupFolder, validateSubfolderName, createSubfolder, handlePathNotFoundError, disableAllModsInGroup, enableAllModsInGroup } from '@/utils/tauri'
import { predictToggledPath } from '@/utils/modPath'
import { placeContextMenu } from '@/utils/contextMenuPosition'
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

/** 右键菜单是否可见 */
const contextMenuVisible = ref(false)
/** 右键菜单位置X */
const contextMenuX = ref(0)
/** 右键菜单位置Y */
const contextMenuY = ref(0)
/** 右键菜单关联的分组数据 */
const contextGroup = ref<ModGroupData | null>(null)
/** 右键菜单关联分组的树深度（0=顶层；折叠按钮对所有非顶层分组应用） */
const contextGroupDepth = ref(0)
/** 右键分组是否可折叠（入口判据）：自身有子树，或父级为真实分组且有子树 */
const contextGroupCollapsible = ref(false)
/** 外部「收起分组」信号：右键菜单收起时置当前分组路径，树节点命中后折叠（用后 nextTick 清回 null） */
const collapsePath = ref<string | null>(null)
/** 分组是否有子分组（右键「收起」入口判据，与树节点 hasChildren 同口径） */
function groupHasChildren(g: ModGroupData): boolean {
  return !!g.hasChild && !!g.children && g.children.length > 0
}

/**
 * 在分组树中查找目标分组的父级节点。
 * 折叠语义：右键深层分组时收起其「父级分组」整棵子树（释放空间查看其他分组）；
 * 顶层分组（无父）返回 null。虚拟 Groups 容器不计为真实父级（普通组不折叠收容器）。
 * @param tree 分组树（顶层列表）
 * @param target 目标分组路径
 * @returns 父级分组节点；顶层/虚拟根容器下无真实父级时返回 null
 */
function findParentPath(tree: ModGroupData[], target: string): ModGroupData | null {
  for (const g of tree) {
    if (g.children?.some((c) => c.groupPath === target)) return g
    if (g.children?.length) {
      const found = findParentPath(g.children, target)
      if (found) return found
    }
  }
  return null
}
/**
 * 右键菜单显示期间及刚关闭后忽略节点点击的时间戳
 * 防止右键弹出后左键点击菜单外部关闭菜单时，mousedown落在el-tree-node上导致误选中分组
 */
let ignoreNodeClickUntil = 0

/**
 * 选中分组
 * @param group 选中的分组对象
 */
function handleSelectGroup(group: ModGroupData) {
  // 如果处于误触忽略窗口，跳过选中
  if (performance.now() < ignoreNodeClickUntil) return
  modsStore.selectGroup(group)
}

/**
 * 分组项右键菜单
 * @param payload 事件和分组数据
 */
function handleContextMenu(payload: { event: MouseEvent; group: ModGroupData; depth: number }) {
  contextGroup.value = payload.group
  contextGroupDepth.value = payload.depth ?? 0
  // 可折叠性：自身有子树，或父级为真实分组（非虚拟 Groups 容器）且有子树时提供「收起父级」入口
  const parent = findParentPath(groupedTree.value, payload.group.groupPath)
  contextGroupCollapsible.value =
    groupHasChildren(payload.group) ||
    (!!parent && !parent.isVirtualRoot && groupHasChildren(parent))
  const e = payload.event
  // 标记300ms内忽略tree节点点击，防止右键后立即mousedown误选
  ignoreNodeClickUntil = performance.now() + 300
  // 溢出检测：菜单估算尺寸 180×220 保守值
  // 缩放校正：根元素 CSS zoom 会让 fixed 定位坐标空间与 clientX/clientY 失配，
  // zoom≠1 时菜单按倍率漂移 —— 统一走 placeContextMenu 处理
  const pos = placeContextMenu(e, 200, 380)
  contextMenuX.value = pos.x
  contextMenuY.value = pos.y
  contextMenuVisible.value = true
}

/**
 * 空白区域右键菜单（仅显示添加分组选项）
 * @param e 鼠标事件
 */
function onContextMenuEmpty(e: MouseEvent) {
  contextGroup.value = null
  ignoreNodeClickUntil = performance.now() + 300
  const MENU_ESTIMATED_W = 200
  const MENU_ESTIMATED_H = 80
  // 缩放校正：同 handleContextMenu（根 CSS zoom 使 fixed 坐标空间与 clientX/Y 失配）
  const pos = placeContextMenu(e, MENU_ESTIMATED_W, MENU_ESTIMATED_H)
  contextMenuX.value = pos.x
  contextMenuY.value = pos.y
  contextMenuVisible.value = true
}

/** 关闭右键菜单 */
function closeContextMenu() {
  contextMenuVisible.value = false
  contextGroup.value = null
  // 关闭后短时间忽略tree节点点击，防左键关闭菜单时误触分组节点
  ignoreNodeClickUntil = performance.now() + 150
}

/**
 * 更新模组数据期间的写操作门控
 *
 * 后端这些分组写命令（`add_group` / `rename_group` / 创建子分组等）都要先获取
 * `UPDATE_LOCK`，而重量级 `update_mod_data`（dev 实测数十秒）会长期持有该锁 ——
 * 此时触发会被静默排队，界面毫无反馈（用户以为点了没用）。故统一拦截并提示。
 *
 * @returns `true` = 放行；`false` = 已拦截（并已给出提示）
 */
function guardDuringUpdate(): boolean {
  if (modsStore.isUpdatingModData) {
    ElMessage.warning(t('Updating mod data, please wait'))
    return false
  }
  return true
}

/**
 * 添加分组 / 添加子分组（右键菜单分发器）
 * - 右键选中非 group 互斥组 → 委托给自动命名的子分组创建流程 handleAddSubGroupAutoNamed
 * - 其余（group_xx / 空白区域）→ 委托给 handleAddGroupXx 创建新的 group_int 分组
 */
async function handleAddGroup() {
  closeContextMenu()
  if (!guardDuringUpdate()) return
  if (contextGroup.value && isMutexGroup(contextGroup.value)) {
    await handleAddSubGroupAutoNamed()
    return
  }
  await handleAddGroupXx()
}

/**
 * 创建新的 group_xx 普通分组（或空白区域添加分组）
 * 流程：输入分组名 → 校验非空 → 调用后端 addGroup 创建 → 刷新列表
 */
async function handleAddGroupXx() {
  if (!guardDuringUpdate()) return
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
  } catch (e: unknown) {
    if (e !== 'cancel' && e !== 'close') {
      ElMessage.error(t('Failed to add group') + ': ' + (e instanceof Error ? e.message : String(e)))
    }
  }
}

/**
 * 在互斥组下创建子分组（默认名自动生成，如 未命名_int1）
 * 流程：收集已有名称 → 生成不冲突的默认名 → 输入框实时校验 → 后端二次校验 → 二次确认 → 创建目录
 */
async function handleAddSubGroupAutoNamed() {
  if (!guardDuringUpdate()) return
  if (!contextGroup.value || !isMutexGroup(contextGroup.value)) return
  // 从当前非group分组的一级子目录中收集已有名称，生成默认 未命名_int{自增}
  const parentPath = contextGroup.value.groupPath
  const parentName = contextGroup.value.name || contextGroup.value.groupName
  // 收集该分组直接子分组名（children）以便去重
  const existingNames = new Set<string>()
  for (const child of (contextGroup.value.children || [])) {
    existingNames.add(child.name || child.groupName || '')
  }
  // 也遍历 mods 防止模组目录同名冲突
  for (const mod of (contextGroup.value.mods || [])) {
    existingNames.add(mod.name || '')
  }
  let defaultName = ''
  let i = 1
  // eslint-disable-next-line no-constant-condition
  while (true) {
    const candidate = `未命名_int${i}`
    if (!existingNames.has(candidate)) {
      defaultName = candidate
      break
    }
    i++
  }

  const illegalCharsRegex = /[\\/:*?"<>|\x00-\x1F]/g
  try {
    const { value } = await ElMessageBox.prompt(
      t('mods.subfolderName'),
      t('mods.addSubfolder'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        inputPlaceholder: t('mods.subfolderName'),
        inputValue: defaultName,
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
    const trimmed = value?.trim()
    if (!trimmed) return
    const [sanitizedName, isValid, errorMsg] = await validateSubfolderName(parentPath, trimmed)
    if (!isValid) {
      ElMessage.error(errorMsg)
      return
    }
    await ElMessageBox.confirm(
      `确定在「${parentName}」下创建子分组「${sanitizedName}」吗？`,
      t('mods.addSubfolder'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        type: 'info',
        customClass: 'app-confirm-box'
      }
    )
    await createSubfolder(parentPath, sanitizedName)
    await modsStore.refresh()
    ElMessage.success(`已在「${parentName}」下创建子分组「${sanitizedName}」`)
  } catch (e: unknown) {
    if (e !== 'cancel' && e !== 'close') {
      ElMessage.error('创建子分组失败: ' + (e instanceof Error ? e.message : String(e)))
    }
  }
}

/** 重命名分组 */
async function handleRenameGroup() {
  if (!guardDuringUpdate()) return
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
      const isGroupXx = contextGroup.value.groupType !== 'mutexGroup'
      const delta = await renameGroup(contextGroup.value.groupPath, value.trim(), isGroupXx)
      // 命令级增量：重命名后的分组新状态原地 patch（零全量 refresh）
      modsStore.applyGroupDelta(delta)
      ElMessage.success(t('Group renamed successfully'))
    }
  } catch (e) {
  }
}

/** 在文件管理器中打开分组文件夹 */
async function handleOpenGroupFolder() {
  if (!contextGroup.value) return
  closeContextMenu()
  try {
    await openGroupFolder(contextGroup.value.groupPath)
  } catch (e: unknown) {
    // 路径不存在错误 → 清除缓存+重读模组（自动处理，不弹错误提示）
    const handled = await handlePathNotFoundError(e)
    if (!handled) {
      ElMessage.error(t('Failed to open folder') + ': ' + (e instanceof Error ? e.message : String(e)))
    }
  }
}

/**
 * 批量启停的乐观更新基建：await 前用组内模组构造预测 pairs 并本地先行 patch。
 *
 * @param groupMods 分组直接模组列表（含 None 槽位，内部排除）
 * @param targetDisabled 目标状态（true=禁用 / false=启用）
 * @returns 预测 pairs（oldPath→newPath+目标禁用态）；无待变更模组时为空数组
 */
function buildOptimisticBatchPairs(groupMods: ModData[], targetDisabled: boolean): { oldPath: string; newPath: string; disabled: boolean }[] {
  const pairs: { oldPath: string; newPath: string; disabled: boolean }[] = []
  for (const m of groupMods) {
    if (m.name === 'None' || m.modPath === 'None') continue
    // 与后端口径一致：已处于目标状态的模组跳过（幂等）
    if (m.modDisabled === targetDisabled) continue
    pairs.push({ oldPath: m.modPath, newPath: predictToggledPath(m.modPath, targetDisabled), disabled: targetDisabled })
  }
  return pairs
}

/** 禁用分组下所有一级模组（已禁用的跳过）；UI 先行乐观更新；零扫描：目录重命名 + 逐条本地 patch；异常降级增量扫描并告知 */
async function handleDisableAllMods() {
  if (!contextGroup.value || !contextGroup.value.groupPath) return
  if (modsStore.isUpdatingModData) return
  const gp = contextGroup.value.groupPath
  const displayName = contextGroup.value.name || contextGroup.value.groupName
  // closeContextMenu 会置空 contextGroup，先捕获组内模组快照
  const groupMods = [...(contextGroup.value.mods || [])]
  closeContextMenu()
  // UI 先行：预测禁用路径并立即批量 patch（灰化即时呈现）
  const optimisticPairs = buildOptimisticBatchPairs(groupMods, true)
  if (optimisticPairs.length > 0) {
    modsStore.batchToggleModLocal(optimisticPairs)
  }
  try {
    const batch = await disableAllModsInGroup(gp)
    if (batch.degraded) {
      // 异常降级：增量扫描 + 组增量 patch（磁盘真相覆盖乐观态），明确告知用户发生异常
      modsStore.applyGroupDelta(batch.delta)
      ElMessage.warning('禁用操作异常，模组数据已增量刷新')
    } else {
      // 零扫描校正：后端权威路径与预测不一致的条目二次 patch（同口径幂等下通常一致）
      const predictedByOld = new Map(optimisticPairs.map((p) => [p.oldPath, p.newPath]))
      for (const t of batch.toggled) {
        const predicted = predictedByOld.get(t.oldPath)
        if (predicted !== undefined && predicted !== t.newPath) {
          modsStore.toggleModLocal(predicted, t.newPath, true)
        } else if (predicted === undefined) {
          // 后端实际变更但未被预测（竞态新增等）：直接 patch
          modsStore.toggleModLocal(t.oldPath, t.newPath, true)
        }
      }
    }
    if (batch.changed > 0) {
      ElMessage.success(`已禁用 ${displayName} 下的 ${batch.changed} 个模组`)
    } else {
      ElMessage.info(`${displayName} 下没有需要禁用的模组`)
    }
  } catch (e: unknown) {
    // 失败：反向回滚乐观 patch（恢复原路径与原禁用态）并告知用户
    if (optimisticPairs.length > 0) {
      modsStore.batchToggleModLocal(
        optimisticPairs.map((p) => ({ oldPath: p.newPath, newPath: p.oldPath, disabled: !p.disabled })),
      )
    }
    ElMessage.error('禁用所有模组失败: ' + (e instanceof Error ? e.message : String(e)))
  }
}

/** 启用分组下所有一级禁用模组（已启用的跳过）；UI 先行乐观更新；零扫描：本地逐条 patch，异常降级增量扫描并告知 */
async function handleEnableAllMods() {
  if (!contextGroup.value || !contextGroup.value.groupPath) return
  if (modsStore.isUpdatingModData) return
  const gp = contextGroup.value.groupPath
  const displayName = contextGroup.value.name || contextGroup.value.groupName
  const groupMods = [...(contextGroup.value.mods || [])]
  closeContextMenu()
  // UI 先行：预测启用路径并立即批量 patch（恢复即时呈现）
  const optimisticPairs = buildOptimisticBatchPairs(groupMods, false)
  if (optimisticPairs.length > 0) {
    modsStore.batchToggleModLocal(optimisticPairs)
  }
  try {
    const batch = await enableAllModsInGroup(gp)
    if (batch.degraded) {
      modsStore.applyGroupDelta(batch.delta)
      ElMessage.warning('启用操作异常，模组数据已增量刷新')
    } else {
      const predictedByOld = new Map(optimisticPairs.map((p) => [p.oldPath, p.newPath]))
      for (const t of batch.toggled) {
        const predicted = predictedByOld.get(t.oldPath)
        if (predicted !== undefined && predicted !== t.newPath) {
          modsStore.toggleModLocal(predicted, t.newPath, false)
        } else if (predicted === undefined) {
          modsStore.toggleModLocal(t.oldPath, t.newPath, false)
        }
      }
    }
    if (batch.changed > 0) {
      ElMessage.success(`已启用 ${displayName} 下的 ${batch.changed} 个模组`)
    } else {
      ElMessage.info(`${displayName} 下没有需要启用的模组`)
    }
  } catch (e: unknown) {
    if (optimisticPairs.length > 0) {
      modsStore.batchToggleModLocal(
        optimisticPairs.map((p) => ({ oldPath: p.newPath, newPath: p.oldPath, disabled: !p.disabled })),
      )
    }
    ElMessage.error('启用所有模组失败: ' + (e instanceof Error ? e.message : String(e)))
  }
}

/** 判断分组是否为 group_xx 普通分组（非 mutexGroup） */
function isGroupXx(group: ModGroupData): boolean {
  return group.groupType !== 'mutexGroup'
}

/** 删除分组（NRMM对齐：移至 _MANAGED_REMOVED_ 目录） */
async function handleRemoveGroup() {
  if (!contextGroup.value || !contextGroup.value.groupPath) return
  if (modsStore.isUpdatingModData) return
  closeContextMenu()
  try {
    await ElMessageBox.confirm(
      t('Removing group will make the mods inside the group can be used again without mod manager.'),
      t('Warning'),
      {
        confirmButtonText: t('common.confirm'),
        cancelButtonText: t('common.cancel'),
        type: 'warning',
        customClass: 'app-confirm-box'
      }
    )
    if (contextGroup.value.groupPath) {
      const isGx = isGroupXx(contextGroup.value)
      const delta = await removeGroupEx(contextGroup.value.groupPath, isGx)
      // 命令级增量：分组移除后原地 patch（树剔除 + 扁平剔除 + 选中回退，零全量 refresh）
      modsStore.applyGroupDelta(delta)
      ElMessage.success(t('Group removed successfully'))
    }
  } catch (e: unknown) {
    if (e !== 'cancel' && e !== 'close') {
      ElMessage.error('移除分组失败: ' + (e instanceof Error ? e.message : String(e)))
    }
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
async function handleAddSubGroup() {
  if (!guardDuringUpdate()) return
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
        customClass: 'app-confirm-box',
      }
    )

    // 步骤4：调用后端创建子文件夹
    await createSubfolder(parentPath, sanitizedName)

    // 步骤5：创建成功，刷新列表
    ElMessage.success(t('mods.subfolderCreated'))
    await modsStore.refresh()

  } catch (e: unknown) {
    if (e !== 'cancel' && e !== 'close') {
      ElMessage.error(t('mods.failedToCreateSubfolder') + ': ' + (e instanceof Error ? e.message : String(e)))
    }
  }
}

/**
 * 右键菜单 action 分发器
 * 将各个菜单项的操作统一分发到对应的独立处理函数
 */
function handleContextMenuAction(action: string) {
  switch (action) {
    case 'addGroup':
      handleAddGroup()
      break
    case 'addSubGroup':
      handleAddSubGroup()
      break
    case 'renameGroup':
      handleRenameGroup()
      break
    case 'openGroupFolder':
      handleOpenGroupFolder()
      break
    case 'disableAllMods':
      handleDisableAllMods()
      break
    case 'enableAllMods':
      handleEnableAllMods()
      break
    case 'collapseGroup':
      // 折叠目标 = 右键分组的「父级分组」（深层节点收起父级整棵子树，释放空间查看其他分组）；
      // 顶层分组（无真实父级）折叠自身子树
      const targetPath = contextGroup.value?.groupPath ?? null
      if (targetPath) {
        const parent = findParentPath(groupedTree.value, targetPath)
        const collapseTarget = parent && !parent.isVirtualRoot ? parent.groupPath : targetPath
        collapsePath.value = collapseTarget
        nextTick(() => {
          collapsePath.value = null
        })
      }
      closeContextMenu()
      break
    case 'removeGroup':
      handleRemoveGroup()
      break
  }
}

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
