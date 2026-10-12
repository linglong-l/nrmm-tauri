/**
 * 分组面板右键菜单 Composable
 *
 * 从 GroupPanel.vue 抽取：右键菜单显隐与定位、分组增删重命名、批量启停模组、收起父级分组。
 * 与 useModContextMenu.ts 同口径（store 与 t 由调用方注入，便于单测）。
 *
 * 使用方法：
 * ```vue
 * <script setup>
 * const { contextMenuVisible, contextMenuX, contextMenuY, contextGroup, collapsePath,
 *         handleContextMenu, handleContextMenuAction, handleSelectGroup } =
 *   useGroupContextMenu(groupedTree, modsStore, t)
 * </script>
 * ```
 */
import { nextTick, ref, type ComputedRef } from 'vue'
import type { Composer } from 'vue-i18n'
import { ElMessage, ElMessageBox } from 'element-plus'
import type { ModData, ModGroupData } from '@/types'
import { addGroup, addMutexGroup, renameGroup, removeGroupEx, openGroupFolder, validateSubfolderName, createSubfolder, isPathNotFoundError, disableAllModsInGroup, enableAllModsInGroup } from '@/utils/tauri'
import { recoverFromPathNotFound } from './usePathNotFoundRecovery'
import { predictToggledPath } from '@/utils/modPath'
import { placeContextMenu } from '@/utils/contextMenuPosition'
import { useModsStore } from '@/stores/mods'
import { useSettingsStore } from '@/stores/settings'

export function useGroupContextMenu(
  groupedTree: ComputedRef<ModGroupData[]>,
  modsStore: ReturnType<typeof useModsStore>,
  t: Composer['t']
) {
  const settingsStore = useSettingsStore()

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
      // 右键 mutex「添加分组」= 在父级目录新建同级 mutex（与右键分组共用父级目录）
      await handleAddMutexSibling()
      return
    }
    // 右键 group_xx / 空白 → 顶层 group_xx
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
   * 独立「添加分组」按钮：弹框选择 Group / Mutex 类型（均置顶于 _MANAGED_）。
   * - Group：调用 addGroup（自动 group_N 编号，名称可选）。
   * - Mutex：调用 createSubfolder(_MANAGED_, name) 新建顶层互斥分组（非 group_xx 目录）。
   */
  async function handleAddGroupWithDialog() {
    if (!guardDuringUpdate()) return
    if (!settingsStore.currentModsPath) {
      ElMessage.warning(t('Please select a group first'))
      return
    }
    // Step 1：选类型（confirm=Group, cancel=Mutex, close=放弃）
    let type: 'group' | 'mutex'
    try {
      await ElMessageBox.confirm(t('mods.addGroup'), t('mods.addGroup'), {
        confirmButtonText: 'Group',
        cancelButtonText: 'Mutex',
        distinguishCancelAndClose: true,
        type: 'info',
      })
      type = 'group'
    } catch (action: unknown) {
      if (action === 'cancel') {
        type = 'mutex'
      } else {
        return
      }
    }
    // Step 2：输入名称（Mutex 必填；Group 可空走自动编号）
    let name: string | undefined
    try {
      const { value } = await ElMessageBox.prompt(
        t('Group Name'),
        t('mods.addGroup') + (type === 'group' ? ' (Group)' : ' (Mutex)'),
        {
          confirmButtonText: t('common.confirm'),
          cancelButtonText: t('common.cancel'),
          inputValidator: (val) => {
            if (type === 'mutex') {
              return (val && val.trim().length > 0) || '分组名不能为空'
            }
            return true
          },
        }
      )
      name = value?.trim() || undefined
    } catch {
      return
    }
    // Step 3：创建
    try {
      if (type === 'group') {
        await addGroup(settingsStore.currentGame, settingsStore.currentModsPath, name)
      } else {
        await addMutexGroup(settingsStore.currentGame, settingsStore.currentModsPath, name!)
      }
      await modsStore.refresh()
      ElMessage.success(t('Group added successfully'))
    } catch (e: unknown) {
      ElMessage.error(t('Failed to add group') + ': ' + (e instanceof Error ? e.message : String(e)))
    }
  }

  /**
   * 右键 mutex「添加分组」：在右键分组的父级目录下新建同级 mutex。
   * 与「添加子分组」（handleAddSubGroup，子级）互补——同级共用父目录，子级以右键分组为父。
   */
  async function handleAddMutexSibling() {
    if (!guardDuringUpdate()) return
    if (!contextGroup.value) return
    const parentGroup = contextGroup.value
    // 右键分组同级 → 父级目录 = 右键分组路径去掉末段
    const parentPath = parentGroup.groupPath.replace(/[\\/][^\\/]*$/, '')
    const parentDisplayName = parentGroup.name || parentGroup.groupName
    closeContextMenu()

    try {
      const { value } = await ElMessageBox.prompt(
        t('Group Name'),
        t('mods.addGroup') + ' → ' + parentDisplayName,
        {
          confirmButtonText: t('common.confirm'),
          cancelButtonText: t('common.cancel'),
          inputValidator: (val) => (val && val.trim().length > 0) || '分组名不能为空',
        }
      )
      const name = value.trim()
      const [sanitizedName, isValid, errorMsg] = await validateSubfolderName(parentPath, name)
      if (!isValid) {
        ElMessage.error(errorMsg)
        return
      }
      await createSubfolder(parentPath, sanitizedName)
      await modsStore.refresh()
      ElMessage.success(t('Group added successfully'))
    } catch (e: unknown) {
      if (e !== 'cancel' && e !== 'close') {
        ElMessage.error(t('Failed to add group') + ': ' + (e instanceof Error ? e.message : String(e)))
      }
    }
  }

  /** 重命名分组 */
  async function handleRenameGroup() {
    if (!guardDuringUpdate()) return
    if (!contextGroup.value) return
    // closeContextMenu 会置空 contextGroup，先捕获所需字段（避免 null 解引用）
    const groupPath = contextGroup.value.groupPath
    const groupType = contextGroup.value.groupType
    const displayName = contextGroup.value.name || contextGroup.value.groupName
    closeContextMenu()
    try {
      const { value } = await ElMessageBox.prompt(
        t('Group Name'),
        t('common.rename'),
        {
          confirmButtonText: t('common.confirm'),
          cancelButtonText: t('common.cancel'),
          inputValue: displayName,
          inputValidator: (val) => {
            if (!val || !val.trim()) return t('Group name cannot be empty')
            return true
          }
        }
      )
      if (value && groupPath) {
        const isGroupXx = groupType !== 'mutexGroup'
        const delta = await renameGroup(groupPath, value.trim(), isGroupXx)
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
    // closeContextMenu 会置空 contextGroup，先捕获路径（避免 null 解引用）
    const groupPath = contextGroup.value.groupPath
    closeContextMenu()
    try {
      await openGroupFolder(groupPath)
    } catch (e: unknown) {
      // 路径不存在错误 → 清除缓存+重读模组（自动处理，不弹错误提示）
      if (isPathNotFoundError(e)) {
        await recoverFromPathNotFound()
        return
      }
      ElMessage.error(t('Failed to open folder') + ': ' + (e instanceof Error ? e.message : String(e)))
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
    // closeContextMenu 会置空 contextGroup，先捕获所需字段（避免 null 解引用）
    const groupPath = contextGroup.value.groupPath
    const isGx = isGroupXx(contextGroup.value)
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
      if (groupPath) {
        const delta = await removeGroupEx(groupPath, isGx)
        // 命令级增量：先瞬时剔除（树+扁平+选中回退），界面不留已删分组
        modsStore.applyGroupDelta(delta)
        ElMessage.success(t('Group removed successfully'))
        // 增量后仍须显式全量重载：remove_group_ex 会移动被删子树之外的目录（子分组上移 Mods 根、空壳进 DISABLED_MANAGED_REMOVED），delta 覆盖不到，而 watcher 回声已被 lastLocalPatchAt 抑制
        await modsStore.refresh()
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
    // closeContextMenu 会置空 contextGroup，先捕获所需字段（避免 null 解引用）
    const parentGroup = contextGroup.value
    const parentPath = parentGroup.groupPath
    const parentDisplayName = parentGroup.name || parentGroup.groupName
    closeContextMenu()

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

  return {
    contextMenuVisible,
    contextMenuX,
    contextMenuY,
    contextGroup,
    contextGroupDepth,
    contextGroupCollapsible,
    collapsePath,
    isMutexGroup,
    handleSelectGroup,
    handleContextMenu,
    onContextMenuEmpty,
    closeContextMenu,
    handleContextMenuAction,
    handleAddGroupWithDialog,
  }
}
