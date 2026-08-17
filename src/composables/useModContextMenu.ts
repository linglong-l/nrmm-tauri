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
import type { ModData } from '@/types'
import { toggleModDisabled, toggleFavorite, openModFolder, handlePathNotFoundError, deselectGroupMod } from '@/utils/tauri'
import { useModsStore } from '@/stores/mods'
import { useSettingsStore } from '@/stores/settings'
import { logger } from '@/utils/logger'

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
    // 标记后续短暂时间内的左键为右键后残留点击，避免误选中
    // Teleport后误触概率已大幅降低，缩短时间窗到300ms
    ignoreNextClickUntil.value = performance.now() + 300
    // 菜单估算尺寸（min-width:160px，按6项约184x240px 保守计算）
    const MENU_ESTIMATED_W = 200
    const MENU_ESTIMATED_H = 260
    const vw = window.innerWidth
    const vh = window.innerHeight
    let x = e.clientX
    let y = e.clientY
    // 水平溢出：右边缘超出就左对齐到鼠标位置左侧
    if (x + MENU_ESTIMATED_W > vw - 4) {
      x = Math.max(4, x - MENU_ESTIMATED_W)
    }
    // 垂直溢出：下边缘超出就向上弹出
    if (y + MENU_ESTIMATED_H > vh - 4) {
      y = Math.max(4, y - MENU_ESTIMATED_H)
    }
    contextMenuPosition.value = { x, y }
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
   * 切换模组启用/禁用状态
   * 调用后端toggle_mod_disabled命令，传入isMutex参数处理互斥组逻辑
   */
  async function handleToggleEnabled() {
    if (!mod.value) return
    closeContextMenu()
    // 操作前捕获原禁用状态，用于生成操作后的提示文案（refresh 后会更新为操作后状态，不能直接拿来判断）
    const wasDisabled = mod.value.modDisabled
    try {
      await toggleModDisabled(mod.value.modPath, mod.value.modDisabled, mod.value.isMutex)
      // 标记需要更新模组数据（仅NormalGroup操作）
      if (!mod.value.isMutex) {
        modsStore.markNeedUpdate(mod.value.groupIndex)
      }
      await modsStore.refresh()
      ElMessage.success(wasDisabled ? t('Enabled') : t('Disabled'))
    } catch (e: unknown) {
      ElMessage.error(t('Failed to enable mod') + ': ' + (e instanceof Error ? e.message : String(e)))
    }
  }

  /** 切换收藏状态 */
  async function handleToggleFavorite() {
    if (!mod.value) return
    closeContextMenu()
    try {
      await toggleFavorite(mod.value.modPath)
      await modsStore.refresh()
    } catch (e: unknown) {
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
      const handled = await handlePathNotFoundError(e)
      if (!handled) {
        ElMessage.error(t('Failed to open mod folder.') + ': ' + (e instanceof Error ? e.message : String(e)))
      }
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