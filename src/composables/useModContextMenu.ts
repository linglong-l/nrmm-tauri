/**
 * 模组右键菜单 Composable
 *
 * 从 ModCard.vue 抽取右键菜单相关逻辑，提升可测试性。
 * 管理：右键菜单的显隐、位置、各菜单项操作（选择/启用禁用/收藏/按键切换/打开文件夹/取消选择分组）。
 *
 * 使用方法：
 * ```vue
 * <script setup>
 * const { contextMenuVisible, contextMenuPosition, openContextMenu, closeContextMenu,
 *         handleSelectMod, handleToggleEnabled, handleToggleFavorite,
 *         handleOpenKeybind, handleOpenFolder, handleDeselectGroup } =
 *   useModContextMenu(mod, modIndex, isNoneSlot, emit, modsStore, t)
 * </script>
 * ```
 */
import { ref, inject, type Ref } from 'vue'
import { ElMessage } from 'element-plus'
import type { ModData } from '@/types'
import { toggleModDisabled, toggleFavorite, openModFolder, deselectGroupMod, isPathNotFoundError } from '@/utils/tauri'
import { recoverFromPathNotFound } from './usePathNotFoundRecovery'
import { predictToggledPath } from '@/utils/modPath'
import { useModsStore } from '@/stores/mods'
import { useSettingsStore } from '@/stores/settings'
import { logger } from '@/utils/logger'
import { placeContextMenu } from '@/utils/contextMenuPosition'

export function useModContextMenu(
  mod: Ref<ModData | null | undefined>,
  modIndex: Ref<number>,
  isNoneSlot: Ref<boolean | undefined>,
  emit: (event: 'activate', modIndex: number) => void,
  modsStore: ReturnType<typeof useModsStore>,
  t: (key: string, fallback?: string) => string
) {
  const settingsStore = useSettingsStore()
  /** 从 App.vue provide 注入的标签页切换函数，用于「按键切换」菜单跳转到 Keybinds 页 */
  const switchTab = inject<(key: 'keybinds' | 'mods' | 'settings') => void>('switchTab', () => {})

  /** 右键菜单是否可见 */
  const contextMenuVisible = ref(false)
  /** 右键菜单位置 */
  const contextMenuPosition = ref<{ x: number; y: number }>({ x: 0, y: 0 })

  /**
   * 右键菜单显示期间及刚关闭后，左键点击不触发选中，防止"飘逸"。
   * - 飘逸根因：用户右键→菜单弹出→左键点空白关闭菜单→这次click事件恰好落在卡片上→
   *   浏览器把它当普通左键点击卡片→select触发→选中状态跳动（看起来飘逸/误选中其他mod）
   */
  const ignoreNextClickUntil = ref(0)

  /** 打开右键菜单 */
  function openContextMenu(e: MouseEvent) {
    if (isNoneSlot.value) return
    // 更新模组数据期间禁止弹出右键菜单：后台正在批量改写 INI/标记文件，
    // 此时触发的启停/移除/选择等操作会与 update 写 `_MANAGED_` 交叠产生中间态竞态
    if (modsStore.isUpdatingModData) {
      logger.debug('ModContextMenu', 'openContextMenu blocked: updating mod data')
      return
    }
    // 标记后续短暂时间内的左键为右键后残留点击，避免误选中
    // Teleport后误触概率已大幅降低，缩短时间窗到300ms
    ignoreNextClickUntil.value = performance.now() + 300
    // 菜单估算尺寸（min-width:160px，按6项约184x240px 保守计算）
    // 缩放校正：根元素 CSS zoom 会让 fixed 定位坐标空间与 clientX/clientY 失配
    // （zoom≠1 时菜单按倍率漂移），placeContextMenu 内部统一处理
    contextMenuPosition.value = placeContextMenu(e, 200, 260)
    contextMenuVisible.value = true
  }

  /** 关闭右键菜单 */
  function closeContextMenu() {
    contextMenuVisible.value = false
    // 关闭菜单后短暂忽略左键（teleport后已极短100ms即可）
    ignoreNextClickUntil.value = performance.now() + 100
  }

  /**
   * 选择模组（右键菜单第一项）
   * 与双击激活走相同的 emit('activate') 路径，
   * 触发后端 select_mod 命令完成 selectedindex 持久化 + 按键模拟
   */
  function handleSelectMod() {
    if (!mod.value || isNoneSlot.value) return
    closeContextMenu()
    emit('activate', modIndex.value)
  }

  /**
   * 切换模组启用/禁用状态（UI 先行乐观更新）
   *
   * 顺序：本地立即 patch 为目标态（路径预测镜像后端 derive_toggled_path）→
   * await 后端 toggle_mod_disabled（目录重命名 + 缓存 patch）→
   * 成功：后端权威路径校正 + 通知；失败/降级：按返回值对齐磁盘真相并告知用户。
   */
  async function handleToggleEnabled() {
    if (!mod.value) return
    closeContextMenu()
    // 操作前捕获原状态（乐观更新与回滚基准；await 后 mod.value 即为 patch 后实例）
    const wasDisabled = mod.value.modDisabled
    const oldPath = mod.value.modPath
    const isMutex = mod.value.isMutex
    const groupIndex = mod.value.groupIndex
    const targetDisabled = !wasDisabled
    // UI 先行：预测目标路径并立即本地 patch（灰化/恢复即时呈现，不等待后端）
    const predictedPath = predictToggledPath(oldPath, targetDisabled)
    modsStore.toggleModLocal(oldPath, predictedPath, targetDisabled)
    try {
      const res = await toggleModDisabled(oldPath, wasDisabled, isMutex)
      if (res.degraded) {
        // 异常降级：增量扫描 + 组增量原地 patch（磁盘真相覆盖乐观态），并明确告知用户发生异常
        modsStore.applyGroupDelta(res.groupDelta)
        ElMessage.warning(t('Operation abnormal, mod data refreshed incrementally'))
      } else if (res.newPath !== predictedPath) {
        // 零扫描校正：后端权威路径与预测不一致时二次 patch（同口径幂等下通常一致）
        modsStore.toggleModLocal(predictedPath, res.newPath, res.disabled)
      }
      // 标记需要更新模组数据（仅NormalGroup操作；失败路径不置位）
      if (!isMutex) {
        modsStore.markNeedUpdate(groupIndex)
      }
      // 通知在乐观 UI 更新之后发出（S3：不得先于状态变更）
      ElMessage.success(wasDisabled ? t('Enabled') : t('Disabled'))
    } catch (e: unknown) {
      // 失败：回滚乐观 patch（恢复原路径与原禁用态）并告知用户
      modsStore.toggleModLocal(predictedPath, oldPath, wasDisabled)
      ElMessage.error(t('Failed to enable mod') + ': ' + (e instanceof Error ? e.message : String(e)))
    }
  }

  /**
   * 切换收藏状态（UI 先行乐观更新；R1：命令返回 bool 后本地翻转，0 IPC 零全量）
   */
  async function handleToggleFavorite() {
    if (!mod.value) return
    closeContextMenu()
    const modPath = mod.value.modPath
    const wasFav = mod.value.isFavorite
    // UI 先行：立即翻转收藏态（收藏星标即时呈现）
    modsStore.toggleFavoriteLocal(modPath, !wasFav)
    try {
      const isFav = await toggleFavorite(modPath)
      // 后端权威值校正（与乐观值不一致时）
      if (isFav !== !wasFav) {
        modsStore.toggleFavoriteLocal(modPath, isFav)
      }
    } catch (e: unknown) {
      // 失败：回翻收藏态
      modsStore.toggleFavoriteLocal(modPath, wasFav)
      logger.error('ModCard', 'Failed to toggle favorite', e)
    }
  }

  /**
   * 跳转按键绑定页并设置目标模组（对齐 NRMM modKeybindProvider）
   * - 设置 keybindTargetMod 使 KeybindsView.selectedMod 优先显示当前模组
   * - 切换胶囊导航到 Keybinds 标签页
   */
  function handleOpenKeybind() {
    if (!mod.value) return
    closeContextMenu()
    modsStore.setKeybindTargetMod(mod.value)
    switchTab('keybinds')
  }

  /** 在文件管理器中打开模组文件夹 */
  async function handleOpenFolder() {
    if (!mod.value) return
    closeContextMenu()
    try {
      await openModFolder(mod.value.modPath)
    } catch (e: unknown) {
      // 路径不存在错误 → 清除缓存+重读模组（自动处理，不弹错误提示）
      if (isPathNotFoundError(e)) {
        await recoverFromPathNotFound()
        return
      }
      ElMessage.error(t('Failed to open mod folder.') + ': ' + (e instanceof Error ? e.message : String(e)))
    }
  }

  /**
   * 取消选择分组（取消该分组中所有模组的选中状态）
   * 调用 deselectGroupMod 后端命令，标记需要更新并刷新
   */
  async function handleDeselectGroup() {
    if (!mod.value) return
    closeContextMenu()
    try {
      await deselectGroupMod(settingsStore.currentGame, settingsStore.currentModsPath, mod.value.groupIndex)
      modsStore.markNeedUpdate(mod.value.groupIndex)
      await modsStore.refresh()
    } catch (e: unknown) {
      ElMessage.error(t('Failed to deselect group') + ': ' + (e instanceof Error ? e.message : String(e)))
    }
  }

  return {
    contextMenuVisible,
    contextMenuPosition,
    ignoreNextClickUntil,
    openContextMenu,
    closeContextMenu,
    handleSelectMod,
    handleToggleEnabled,
    handleToggleFavorite,
    handleOpenKeybind,
    handleOpenFolder,
    handleDeselectGroup,
  }
}