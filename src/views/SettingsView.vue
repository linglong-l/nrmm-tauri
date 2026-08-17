<template>
  <div class="settings-view">
    <div class="settings-scroll hide-scrollbar" ref="scrollRef">
      <div class="settings-container">
        <!-- 进程 + 路径行 -->
        <div class="process-path-row">
          <div class="process-col">
            <div class="field-label">{{ t('settings.targetProcess') }}</div>
            <el-input
              :model-value="targetProcessValue"
              class="process-input"
              :placeholder="t('settings.targetProcessPlaceholder')"
              @update:model-value="updateTargetProcess"
              @blur="onTargetProcessBlur"
            />
          </div>
          <div class="path-col">
            <div class="field-label">{{ t('settings.modsPath') }}</div>
            <div class="path-input-row">
              <el-input
                :model-value="settingsStore.currentModsPath"
                class="path-input"
                :class="{ 'is-error': !isModsPathValid }"
                readonly
              />
              <el-button @click="handleBrowse" class="browse-folder-btn" :title="t('settings.browse')">
                <el-icon><FolderOpened /></el-icon>
              </el-button>
            </div>
            <div v-if="!isModsPathValid && settingsStore.currentModsPath" class="path-error-hint">
              请确保路径为Mods目录
            </div>
          </div>
        </div>
        <br>
        <!-- 大按钮卡片区：重量级操作按钮 -->
        <div class="big-btn-cards">
          <!-- 更新模组数据按钮：执行完整INI扫描和修复（重量级操作） -->
            <el-button class="big-main-btn" @click="handleUpdateModData" :loading="updatingModData">
              {{ t('settings.updateModData') }}
            </el-button>
            <p class="big-btn-desc">{{ t('Press this after you add/remove/edit/fix mods (usually when add/edit/remove mods directly via File Explorer)') }}</p>
          <br>


            <el-button class="big-main-btn" @click="handleSaveCustomizations">
              {{ t('settings.saveModCustomizations') }}
            </el-button>
            <p class="big-btn-desc">{{ t('settings.saveCustomizationsDesc') }}</p>
          <br>

            <el-button class="big-main-btn" @click="handleToggleModsEnabled">
              {{ t('settings.enableDisableMods') }}
            </el-button>
            <p class="big-btn-desc">{{ t('settings.enableDisableModsDesc') }}</p>
          <br>

            <el-button class="big-main-btn" @click="handleDetectHashConflicts">
              {{ t('settings.detectHashConflicts') }}
            </el-button>
            <p class="big-btn-desc">{{ t('settings.detectHashConflictsDesc') }}</p>
          <br>
        </div>

        <!-- 还原区：从还原区恢复已删除模组 -->
        <div ref="restoreZoneRef" class="section-block">
          <h4 class="section-heading">{{ t('settings.restoreZone') }}</h4>
          <button
            type="button"
            class="restore-zone-btn"
            :class="{ 'drag-over': isRestoreDragging }"
            :disabled="restoring"
            @click="handleRestoreZone"
            @dragover.prevent="handleRestoreDragOver"
            @dragleave.prevent="handleRestoreDragLeave"
            @drop.prevent="handleRestoreDrop"
          >
            <el-icon :size="40" class="restore-icon" aria-hidden="true"><UploadFilled /></el-icon>
            <span class="restore-main">{{ t('settings.restoreZonePlaceholder') }}</span>
            <span class="restore-desc">{{ t('settings.restoreZoneDesc') }}</span>
          </button>
        </div>

        <br>
        <!-- 分组文件夹图标设置 -->
          <h4 class="section-heading">{{ t('settings.groupFolderIcon') }}</h4>
          <div class="block-row center">
            <el-button class="normal-btn" @click="handleGenerateFolderIcon">
              {{ t('settings.generateFolderIcon') }}
            </el-button>
          </div>
          <div class="block-row">
            <span class="row-label">{{ t('settings.autoGenerateIcon') }}</span>
            <el-switch v-model="settingsStore.settings.autoFolderIcon" active-color="#4a9eff" inactive-color="#555" @change="onSettingChange" />
          </div>


        <!-- 界面缩放滑块 -->
        <div class="section-block">
          <div class="block-row">
            <span class="row-label">{{ t('settings.overallScale') }}</span>
            <span class="slider-value">{{ interfaceScaleText }}</span>
          </div>
          <el-slider
            v-model="settingsStore.settings.interfaceScale"
            :min="SCALE_MIN"
            :max="SCALE_MAX"
            :step="SCALE_STEP"
            show-stops
            @input="onSettingChange"
          />
        </div>

        <!-- 背景透明度滑块 -->
        <div class="section-block">
          <div class="block-row">
            <span class="row-label">{{ t('settings.backgroundTransparency') }}</span>
            <span class="slider-value">{{ bgTransparencyText }}</span>
          </div>
          <el-slider
            v-model="settingsStore.settings.bgTransparency"
            :min="ALPHA_MIN"
            :max="ALPHA_MAX"
            :step="ALPHA_STEP"
            show-stops
            @input="onSettingChange"
          />
        </div>

        <!-- 窗口切换快捷键配置：键盘+手柄合并为同一行 -->
        <div class="section-block">
          <h4 class="section-heading">{{ t('settings.windowToggleHotkey') }}</h4>
          <div class="block-row hotkey-row">
            <div class="hotkey-cell">
              <span class="row-label">{{ t('settings.keyboardToggle') }}</span>
              <el-select
                v-model="settingsStore.settings.windowHotkey"
                class="field-select"
                popper-class="settings-select-dropdown"
                placeholder="Alt+D"
                @change="onSettingChange"
              >
                <el-option label="Alt+D" value="Alt+D" />
                <el-option label="Alt+W" value="Alt+W" />
                <el-option label="Alt+Q" value="Alt+Q" />
                <el-option label="Alt+E" value="Alt+E" />
                <el-option label="Alt+R" value="Alt+R" />
                <el-option label="Alt+T" value="Alt+T" />
                <el-option :label="t('settings.none')" value="none" />
              </el-select>
            </div>
            <div class="hotkey-cell">
              <span class="row-label">{{ t('settings.gamepadToggle') }}</span>
              <el-select
                v-model="settingsStore.settings.gamepadHotkeyToggle"
                class="field-select"
                popper-class="settings-select-dropdown"
                placeholder="LB+RB"
                @change="onSettingChange"
              >
                <el-option label="LB+RB" value="LB+RB" />
                <el-option label="LB+Start" value="LB+Start" />
                <el-option label="RB+Back" value="RB+Back" />
                <el-option :label="t('settings.none')" value="" />
              </el-select>
            </div>
          </div>
        </div>

        <!-- 开关设置区 -->
        <div class="section-block">
          <div class="block-row">
            <span class="row-label">{{ t('settings.autoPinWindow') }}</span>
            <el-switch v-model="settingsStore.settings.autoTopWindow" active-color="#4a9eff" inactive-color="#555" @change="onSettingChange" />
          </div>
          <div class="block-row">
            <span class="row-label">{{ t('settings.showTrayMenu') }}</span>
            <el-switch v-model="settingsStore.settings.alwaysShowMenuOnHotkey" active-color="#4a9eff" inactive-color="#555" @change="onSettingChange" />
          </div>
          <div class="block-row">
            <span class="row-label">{{ t('settings.simulateKeyOnSelection') }}</span>
            <el-switch v-model="settingsStore.simulateKeyOnSelection" active-color="#4a9eff" inactive-color="#555" @change="onSettingChange" />
          </div>
          <div class="setting-desc">{{ t('settings.simulateKeyOnSelectionDesc') }}</div>
        </div>

        <!-- 语言选择 -->
        <div class="section-block">
          <div class="block-row">
            <span class="row-label">{{ t('settings.language') }}</span>
            <el-select
              v-model="settingsStore.settings.language"
              class="field-select"
              popper-class="settings-select-dropdown"
              @change="handleLanguageChange"
            >
              <el-option :label="t('common.languageZhCN', '简体中文')" value="zh-CN" />
              <el-option :label="t('common.languageEn', 'English')" value="en" />
            </el-select>
          </div>
        </div>

        <!-- 底部三按钮：检查更新、重置位置、退出 -->
        <div class="section-block bottom-triplet">
          <el-button class="normal-btn" @click="handleCheckUpdates" :loading="checkingUpdates">
            {{ t('settings.checkUpdates') }}
          </el-button>
          <el-button class="normal-btn" @click="handleResetPosition">
            {{ t('settings.resetPosition') }}
          </el-button>
          <el-button class="normal-btn" @click="handleExit">
            {{ t('settings.exit') }}
          </el-button>
        </div>

        <!-- 底部链接：支持、联系、教程 -->
        <div class="bottom-links">
          <button type="button" class="link-item" @click="onSupportClick" :aria-label="t('settings.supportDeveloper')">
            {{ t('settings.supportDeveloper') }}
          </button>
          <span class="link-separator">·</span>
          <button type="button" class="link-item" @click="onContactClick" :aria-label="t('settings.contactHelp')">
            {{ t('settings.contactHelp') }}
          </button>
          <span class="link-separator">·</span>
          <button type="button" class="link-item" @click="onTutorialClick" :aria-label="t('settings.tutorial')">
            {{ t('settings.tutorial') }}
          </button>
          <span class="link-separator">·</span>
          <span class="link-separator">v{{ appVersion }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 设置页面
 * 提供游戏路径配置、模组数据更新、界面外观设置、快捷键配置等功能
 * 内置拖拽滚动、防抖保存（300ms延迟自动保存）
 * 重量级操作（更新模组数据）有明确提示和加载状态
 *
 * 注意：表单直接绑定settingsStore，不使用本地副本，
 * 确保切换标签页后数据不丢失
 */
import { ref, computed, onMounted, onBeforeUnmount, nextTick, inject } from 'vue'
import { useI18n } from 'vue-i18n'
import { UploadFilled, FolderOpened } from '@element-plus/icons-vue'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { useSettingsStore } from '@/stores/settings'
import { useModsStore } from '@/stores/mods'
import { usePlatform } from '@/stores/platform'
import { selectFolder, checkForUpdates, getAppVersion, updateModData, restoreManagedFolder } from '@/utils/tauri'
import type { RestoreManagedResult, OverlayController } from '@/types'
import { logger } from '@/utils/logger'
import { useDragScroll } from '@/composables/useDragScroll'

const { t, locale } = useI18n()
const settingsStore = useSettingsStore()
const modsStore = useModsStore()
const { platformInfo } = usePlatform()
const updateOverlay = inject<OverlayController>('updateOverlay')
const hashConflictOverlay = inject<{ show: () => void; hide: () => void }>('hashConflictOverlay')

/** 界面缩放滑块边界（须与后端 AppSettings.interfaceScale 取值范围一致） */
const SCALE_MIN = 0.6
const SCALE_MAX = 2.0
const SCALE_STEP = 0.1
const SCALE_DEFAULT = 1.0
/** 背景透明度滑块边界（须与后端 AppSettings.bgTransparency 取值范围一致） */
const ALPHA_MIN = 0
const ALPHA_MAX = 1
const ALPHA_STEP = 0.1
const ALPHA_DEFAULT = 0.3

/** 滑块当前值文本（带默认值兜底，提取为 computed 保持模板干净） */
const interfaceScaleText = computed(() => (settingsStore.settings.interfaceScale ?? SCALE_DEFAULT).toFixed(1))
const bgTransparencyText = computed(() => (settingsStore.settings.bgTransparency ?? ALPHA_DEFAULT).toFixed(1))

/**
 * 判断路径是否为合法的Mods目录（最后一级目录名必须为"Mods"，大小写不敏感）
 * 兼容 Windows 反斜杠和 Unix 正斜杠
 */
function isValidModsDir(path: string): boolean {
  if (!path) return false
  const normalized = path.replace(/\\/g, '/').replace(/\/+$/, '')
  if (!normalized) return false
  const lastSeg = normalized.substring(normalized.lastIndexOf('/') + 1)
  return lastSeg.toLowerCase() === 'mods'
}

/** 当前Mods路径是否合法（computed响应式） */
const isModsPathValid = computed<boolean>(() => {
  return isValidModsDir(settingsStore.currentModsPath)
})

/** 滚动容器DOM引用 */
const scrollRef = ref<HTMLElement | null>(null)
/** 拖拽滚动：复用 useDragScroll composable（默认排除规则已覆盖交互元素 + Element Plus 组件） */
useDragScroll(scrollRef)

/**
 * 当前游戏目标进程名（只读 computed）
 * 数据源：settingsStore.currentTargetProcess（含 fallback 默认进程名）
 * 修改统一通过 updateTargetProcess 方法触发，避免 computed setter 副作用
 */
const targetProcessValue = computed(() => settingsStore.currentTargetProcess)
/**
 * 更新目标进程名（显式方法，替代 computed setter 副作用）
 * 由 el-input 的 @update:model-value 触发，内部调用 store 持久化方法
 * @param val 新的进程名字符串
 */
function updateTargetProcess(val: string) {
  settingsStore.setCurrentTargetProcess(val)
}

/** 更新模组数据加载状态 */
const updatingModData = ref(false)
/** 检查更新加载状态 */
const checkingUpdates = ref(false)
/** 应用版本号 */
const appVersion = ref('')

/**
 * 目标进程输入框失焦处理
 * Windows下检查是否有.exe后缀，提示用户
 */
function onTargetProcessBlur() {
  const os = platformInfo.value?.os
  const val = targetProcessValue.value
  if (os === 'windows' && val && !/\.exe$/i.test(val)) {
    ElMessage.info(t('settings.targetProcessHint'))
  }
  // setCurrentTargetProcess已在input时调用，blur时确保保存
  settingsStore.save()
}

/**
 * 通用开关/值变更处理
 * 直接修改store中的值并触发防抖保存
 */
function onSettingChange() {
  settingsStore.save()
}

/**
 * 语言变更处理
 * 切换i18n locale并保存设置
 */
function handleLanguageChange(lang: string) {
  locale.value = lang
  settingsStore.settings.language = lang
  settingsStore.save()
}

/** 浏览选择模组文件夹 */
async function handleBrowse() {
  try {
    const selected = await selectFolder()
    if (!selected) return
    if (!isValidModsDir(selected)) {
      // 非法：即使非法也允许用户保存（保持原有行为兼容），只是显示提示
      // 但这里明确提示用户，然后还是让store保存以便用户看到提示
      logger.warn('SettingsView', 'Selected path is not a Mods directory:', selected)
    }
    settingsStore.setCurrentModsPath(selected)
  } catch (e: unknown) {
    ElMessage.error(t('Failed to open folder dialog'))
    logger.error('SettingsView', 'Browse failed', e)
  }
}

/**
 * 更新模组数据（重量级操作）
 * 触发后端完整INI解析、错误修复、互斥组处理
 * 与轻量扫描(loadMods/refresh)不同，此操作会：
 * - 解析所有INI文件的完整内容
 * - 检测并修复namespace冲突
 * - 处理missingEndif等错误
 * - 重新计算modDisabled状态
 */
async function handleUpdateModData() {
  logger.debug('SettingsView', 'handleUpdateModData started')
  try {
    logger.info('SettingsView', 'Updating mod data (heavyweight)...')
    if (!settingsStore.currentModsPath) {
      ElMessage.warning(t('Mods path does not exist.'))
      return
    }
    updateOverlay?.show('loading')
    const start = Date.now()
    try {
      const result = await modsStore.updateModData() ?? (await updateModData(settingsStore.currentGame, settingsStore.currentModsPath))
      updateOverlay?.show('completed', { result, durationMs: Date.now() - start })
    } catch (e: unknown) {
      const msg = typeof e === 'string' ? e : (e instanceof Error ? e.message : String(e))
      updateOverlay?.show('error', { error: msg })
    }
  } catch (e: unknown) {
    logger.error('SettingsView', 'Update mod data failed', e)
  }
}

/** 保存模组自定义（占位功能） */
function handleSaveCustomizations() {
  logger.info('SettingsView', 'Save mod customizations (placeholder)')
  ElMessage.info(t('Task completed!'))
}

/** 切换模组启用/禁用状态 */
function handleToggleModsEnabled() {
  logger.info('SettingsView', 'Toggle mods enabled (placeholder)')
  ElMessage.info(t('Task completed!'))
}

/** 触发 Hash 冲突检测遮罩 */
function handleDetectHashConflicts() {
  logger.info('SettingsView', 'Detect hash conflicts clicked')
  hashConflictOverlay?.show()
}

/** 生成文件夹图标（占位功能） */
function handleGenerateFolderIcon() {
  logger.info('SettingsView', 'Generate folder icon')
  ElMessage.info(t('Task completed!'))
}

/** 还原区是否正在拖拽文件（用于高亮反馈） */
const isRestoreDragging = ref(false)
/** 还原操作进行中状态 */
const restoring = ref(false)
/** 还原区整块DOM引用（标题+按钮，判断拖放落点是否在还原区内） */
const restoreZoneRef = ref<HTMLElement | null>(null)
/** Tauri拖放事件解绑函数 */
let unlistenDragDrop: (() => void) | null = null

/**
 * 判断拖放坐标是否落在还原区整块区域内（含标题与按钮上方区域）
 * @param x 拖放位置X坐标（Tauri 物理像素）
 * @param y 拖放位置Y坐标（Tauri 物理像素）
 * @returns 是否落在还原区矩形范围内
 */
function isInsideRestoreZone(x: number, y: number): boolean {
  const el = restoreZoneRef.value
  if (!el) return false
  const rect = el.getBoundingClientRect()
  // Tauri v2 的拖放 position 为物理像素（PhysicalPosition），
  // getBoundingClientRect 返回 CSS 逻辑像素；高 DPI 缩放（dpr>1）下二者不一致，
  // 需先将物理像素除以 devicePixelRatio 转为逻辑像素再比较，否则还原区下半部分会越界无法触发。
  const dpr = window.devicePixelRatio || 1
  const cx = x / dpr
  const cy = y / dpr
  return cx >= rect.left && cx <= rect.right && cy >= rect.top && cy <= rect.bottom
}

/**
 * 注册 Tauri 原生拖放事件监听
 *
 * Tauri v2 默认拦截系统文件拖放事件，HTML5 的 dragover/drop 不触发，
 * 必须通过 getCurrentWebview().onDragDropEvent 获取真实文件路径。
 * 仅在 Tauri 运行时可用；非 Tauri 环境（浏览器预览）下注册失败即跳过，
 * 由 HTML5 的 handleRestoreDragOver/handleRestoreDrop 兜底。
 */
async function setupTauriDragDrop() {
  try {
    unlistenDragDrop = await getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload
      // enter/over：指针进入或悬停还原区时高亮；leave：移出取消高亮
      if (payload.type === 'enter' || payload.type === 'over') {
        isRestoreDragging.value = isInsideRestoreZone(payload.position.x, payload.position.y)
        return
      }
      if (payload.type === 'leave') {
        isRestoreDragging.value = false
        return
      }
      // drop：释放文件，仅当落点在还原区内时触发还原流程
      if (payload.type === 'drop') {
        isRestoreDragging.value = false
        if (isInsideRestoreZone(payload.position.x, payload.position.y) && payload.paths.length > 0) {
          doRestore(payload.paths)
        }
      }
    })
  } catch (e) {
    // 非 Tauri 环境（如浏览器预览）不支持 onDragDropEvent，静默降级给 HTML5 事件
    logger.warn('SettingsView', 'Tauri onDragDropEvent unavailable, fallback to HTML5 drag events', e)
  }
}

/**
 * 还原区拖拽进入/移动：识别文件拖入并提示可投放
 * @param e 拖拽事件对象
 */
function handleRestoreDragOver(e: DragEvent) {
  if (e.dataTransfer?.types.includes('Files')) {
    e.dataTransfer.dropEffect = 'copy'
    isRestoreDragging.value = true
  }
}

/**
 * 还原区拖拽离开：指针移出区域时取消高亮
 * @param e 拖拽事件对象
 */
function handleRestoreDragLeave(e: DragEvent) {
  const el = e.currentTarget as HTMLElement
  const rect = el.getBoundingClientRect()
  if (
    e.clientX <= rect.left ||
    e.clientX >= rect.right ||
    e.clientY <= rect.top ||
    e.clientY >= rect.bottom
  ) {
    isRestoreDragging.value = false
  }
}

/**
 * 还原区拖放：从拖拽文件中提取目录路径并触发还原
 * @param e 拖拽事件对象
 */
async function handleRestoreDrop(e: DragEvent) {
  isRestoreDragging.value = false
  const files = e.dataTransfer?.files
  if (!files || files.length === 0) return

  const paths: string[] = []
  for (let i = 0; i < files.length; i++) {
    const f = files[i] as File & { path?: string; webkitRelativePath?: string }
    const p = f.path || f.webkitRelativePath
    if (p) paths.push(p)
  }

  if (paths.length === 0) {
    ElMessage.warning(t('settings.restoreNoPath'))
    return
  }
  await doRestore(paths)
}

/**
 * 还原区选择文件夹：通过系统文件夹选择器获取目录路径并触发还原
 */
async function handleRestoreZone() {
  logger.info('SettingsView', 'Restore zone clicked')
  try {
    const selected = await selectFolder()
    if (!selected) return
    await doRestore([selected])
  } catch (e: unknown) {
    logger.error('SettingsView', 'Restore zone select failed', e)
  }
}

/**
 * 还原模组核心流程
 * 1. 二次确认是否还原
 * 2. 逐个调用后端还原接口
 * 3. 汇总还原结果并弹出结果对话框（右下角关闭按钮）
 * @param paths 待还原的目录路径数组
 */
async function doRestore(paths: string[]) {
  // 二次确认
  try {
    await ElMessageBox.confirm(
      t('settings.restoreConfirm', { count: paths.length }),
      t('settings.restoreZone'),
      {
        confirmButtonText: t('common.confirm', '确定'),
        cancelButtonText: t('common.cancel', '取消'),
        type: 'warning',
        customClass: 'restore-confirm-dialog',
      }
    )
  } catch {
    // 用户取消，不执行还原
    logger.info('SettingsView', 'Restore cancelled by user')
    return
  }

  restoring.value = true
  try {
    const results: RestoreManagedResult[] = []
    for (const p of paths) {
      try {
        results.push(await restoreManagedFolder(p))
      } catch (err: unknown) {
        // 单个目录还原失败：记录失败结果，不中断其余目录
        logger.error('SettingsView', 'Restore failed for path', p, err)
        results.push({ path: p, iniCount: 0, failedCount: 1, success: false })
      }
    }
    await showRestoreResult(results)
  } finally {
    restoring.value = false
  }
}

/**
 * 展示还原结果对话框
 * 逐行列出每个路径的还原成功/失败信息，
 * 关闭按钮位于右下角（蓝色字体、胶囊框、无背景色，悬停淡蓝色背景）
 * @param results 还原结果数组
 */
async function showRestoreResult(results: RestoreManagedResult[]) {
  const allSuccess = results.every(r => r.success)
  const lines = results.map(r => {
    const pathText = r.path
    if (r.success) {
      return t('settings.restoreResultSuccessLine', { path: pathText, iniCount: r.iniCount })
    }
    return t('settings.restoreResultFailedLine', { path: pathText, failedCount: r.failedCount })
  })
  const message = lines.join('\n')

  await ElMessageBox({
    title: allSuccess ? t('settings.restoreResultSuccessTitle') : t('settings.restoreResultFailedTitle'),
    message,
    showCancelButton: false,
    confirmButtonText: t('common.close', '关闭'),
    customClass: 'restore-result-dialog',
    confirmButtonClass: 'restore-result-close-btn',
    dangerouslyUseHTMLString: false,
  })
}

/** 检查应用更新 */
async function handleCheckUpdates() {
  checkingUpdates.value = true
  try {
    const result = await checkForUpdates()
    if (result) {
      ElMessage.info(t('Updater.newVersionAvailable', { version: result }))
    } else {
      ElMessage.info(t('Updater.upToDate'))
    }
  } catch (e: unknown) {
    ElMessage.error(t('Updater.checkFailed') + ': ' + (e instanceof Error ? e.message : String(e)))
  } finally {
    checkingUpdates.value = false
  }
}

/** 重置窗口位置 */
async function handleResetPosition() {
  try {
    const { resetWindowPosition } = await import('@/utils/tauri')
    await resetWindowPosition('main')
    ElMessage.success(t('Reset Position'))
  } catch (e: unknown) {
    logger.error('SettingsView', 'Reset position failed', e)
  }
}

/** 退出应用 */
async function handleExit() {
  try {
    const { hardQuitApp } = await import('@/utils/tauri')
    await hardQuitApp()
  } catch (e: unknown) {
    logger.error('SettingsView', 'Exit failed', e)
  }
}

/** 支持开发者链接（占位） */
function onSupportClick() {
  logger.info('SettingsView', 'Support developer clicked (placeholder)')
  ElMessage.info(t('Task completed!'))
}

/** 联系帮助链接（占位） */
function onContactClick() {
  logger.info('SettingsView', 'Contact help clicked (placeholder)')
  ElMessage.info(t('Task completed!'))
}

/** 教程链接（占位） */
function onTutorialClick() {
  logger.info('SettingsView', 'Tutorial clicked (placeholder)')
  ElMessage.info(t('Task completed!'))
}

onMounted(async () => {
  // 等待settings加载完成（App.vue已调用load，这里确保loaded）
  if (!settingsStore.loaded) {
    await settingsStore.load()
  }
  // 同步语言设置
  if (settingsStore.settings.language) {
    locale.value = settingsStore.settings.language
  }
  // 获取应用版本
  try {
    appVersion.value = await getAppVersion()
  } catch (e) {
    logger.warn('SettingsView', 'Failed to get app version', e)
  }
  await nextTick()
  await setupTauriDragDrop()
})

onBeforeUnmount(() => {
  // 清理 Tauri 拖放事件监听，防止组件卸载后回调残留
  if (unlistenDragDrop) {
    unlistenDragDrop()
    unlistenDragDrop = null
  }
})
</script>

<style scoped>
.settings-view {
  flex: 1;
  display: flex;
  flex-direction: column;
  position: relative;
  overflow: hidden;
  background: transparent;
  /* 所有字符（包括input/select/label文字）全局居中显示 */
  text-align: center;
}
.settings-view * {
  text-align: center;
}

/* 精修各控件的文字水平+垂直居中对齐 */
.settings-view :deep(.el-input__inner),
.settings-view :deep(.el-textarea__inner),
.settings-view :deep(.el-select__selected-item),
.settings-view :deep(.el-select__placeholder) {
  text-align: center !important;
  justify-content: center;
  align-items: center;
}
.settings-view :deep(.el-select__wrapper) {
  text-align: center;
}
.settings-view :deep(.el-button) {
  text-align: center !important;
  justify-content: center;
  align-items: center;
  line-height: 1;
}
.settings-view :deep(.el-switch) {
  display: inline-flex;
  align-items: center;
  vertical-align: middle;
}

/*
 * 设置页面所有按钮兜底：
 * - 胶囊形状
 * - 背景色 #848484
 * - opacity 0.4（hover 0.6 / active 0.75）
 * 四个自定义按钮类（normal-btn/big-main-btn/browse-folder-btn）
 * 已在上文各自覆盖，这里补齐 .el-button 默认样式的兜底，避免遗漏按钮。
 */
.settings-view :deep(.el-button) {
  background: #848484 !important;
  border-color: #848484 !important;
  color: var(--text-primary, #fff) !important;
  opacity: 0.84;
  /* 胶囊 */
  border-radius: 999px !important;
  box-shadow: none !important;
  transition: opacity 0.2s ease, background-color 0.2s ease, border-color 0.2s ease;
}
.settings-view :deep(.el-button:hover) {
  opacity: 0.6;
  background: #848484 !important;
  border-color: #9a9a9a !important;
}
.settings-view :deep(.el-button:active) {
  opacity: 0.75;
}
.settings-view :deep(.el-button.is-disabled) {
  opacity: 0.2;
}

.settings-scroll {
  flex: 1;
  overflow-y: auto;
  padding: 20px 24px 108px;
}

.settings-container {
  max-width: 780px;
  margin: 0 auto;
}

.process-path-row {
  display: grid;
  grid-template-columns: 1fr 1.4fr;
  gap: 20px;
  margin-bottom: 22px;
}

.process-col,
.path-col {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.field-label {
  font-size: 12px;
  color: var(--text-secondary);
  font-weight: 500;
}

.process-input :deep(.el-input__wrapper) {
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(255, 255, 255, 0.18);
  box-shadow: none !important;
  border-radius: 6px;
  /* 文字垂直+水平居中 */
  display: flex;
  align-items: center;
  justify-content: center;
}

.process-input :deep(.el-input__wrapper:hover) {
  border-color: rgba(255, 255, 255, 0.32);
}

.process-input :deep(.el-input__wrapper.is-focus) {
  border-color: var(--accent-primary, #4a9eff);
}

.process-input :deep(.el-input__inner) {
  color: var(--text-primary);
  text-align: center !important;
  line-height: normal;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0 !important;
}

.path-input-row {
  display: flex;
  gap: 8px;
}

.path-input {
  flex: 1;
}

.path-input :deep(.el-input__wrapper) {
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(255, 255, 255, 0.15);
  box-shadow: none !important;
  border-radius: 6px;
  cursor: default;
  display: flex;
  align-items: center;
  justify-content: center;
}

.path-input :deep(.el-input__inner) {
  color: var(--text-secondary);
  cursor: default;
  text-align: center !important;
  padding: 0 !important;
  display: flex;
  align-items: center;
  justify-content: center;
}

/* Mods路径非法时的错误态样式 */
.path-input.is-error :deep(.el-input__wrapper) {
  border-color: #f56c6c !important;
  background: rgba(245, 108, 108, 0.06);
}

.path-input.is-error :deep(.el-input__inner) {
  color: #f56c6c;
}

/* 路径错误提示 */
.path-error-hint {
  margin-top: 6px;
  font-size: 12px;
  line-height: 1.4;
  color: #f56c6c;
  text-align: center;
}

/* 大按钮卡片区 */
.big-btn-cards {
  display: flex;
  flex-direction: column;
  align-items: center;
  margin-bottom: 16px;
}

.big-main-btn {
  width: 100%;
  min-height: 44px;
  height: 44px;
  font-size: 14px;
  font-weight: 600;
  padding: 0 22px;
  background: #848484 !important;
  border: 2px solid #848484 !important;
  color: var(--text-primary) !important;
  opacity: 0.4;
  /* 胶囊：44px / 2 */
  border-radius: 22px !important;
  box-shadow: none !important;
  transition: opacity 0.2s ease, background-color 0.2s ease, border-color 0.2s ease, transform 0.08s ease;
  /* 文字精确居中 */
  display: inline-flex !important;
  align-items: center !important;
  justify-content: center !important;
  text-align: center !important;
  line-height: 1 !important;
  white-space: nowrap;
}

.big-main-btn:hover {
  opacity: 0.4;
  background: #848484 !important;
  border-color: #ffffff !important;
}

.big-main-btn:active {
  transform: scale(0.995);
  opacity: 0.75;
}

.big-btn-desc {
  margin: 12px 0 0;
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.5;
  white-space: normal;
  text-align: center;
}

/* 通用 section 块 */
.section-block {
  padding: 14px 4px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
  background: transparent !important;
}

.section-block:last-of-type {
  border-bottom: none;
  background: transparent !important;
}

.section-heading {
  text-align: center;
  font-size: 15px;
  font-weight: 600;
  color: var(--text-primary);
  margin: 4px 0 14px;
}

.block-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 2px;
  gap: 16px;
}

.block-row.center {
  justify-content: center;
  padding: 8px 2px 14px;
}

/*
 * 快捷键合并行：键盘切换 + 手柄切换 同一行并排展示
 * 两个 hotkey-cell 等宽，且整体不换行；
 * 子元素 (label + select) 垂直列对齐。
 */
.block-row.hotkey-row {
  display: flex;
  flex-direction: row;
  flex-wrap: nowrap;
  align-items: stretch;
  justify-content: space-between;
  gap: 20px;
}
.hotkey-cell {
  flex: 1 1 0;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: stretch;
  justify-content: flex-start;
  gap: 8px;
}
.hotkey-cell .row-label {
  text-align: center;
  padding: 0 4px;
}

.row-label {
  font-size: 13px;
  color: var(--text-primary);
  flex-shrink: 0;
  background: transparent !important;
  /* 所有行标签文字居中 */
  text-align: center;
  line-height: 1.4;
}

.setting-desc {
  margin-top: 6px;
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.5;
  text-align: center;
}

.slider-value {
  font-size: 13px;
  color: var(--text-secondary);
  min-width: 36px;
  text-align: center;
  font-variant-numeric: tabular-nums;
}

.field-select {
  min-width: 180px;
  background: transparent !important;
  /* 变量层统一覆盖ElementPlus默认圆角，确保多层嵌套节点都为胶囊 */
  --el-bg-color: transparent;
  --el-fill-color-blank: transparent;
  --el-select-bg-color: transparent;
  --el-input-bg-color: transparent;
  --el-border-radius: 20px;
  --el-border-radius-base: 20px;
  --el-input-border-radius: 20px;
  --el-select-border-radius: 20px;
  /* el-select根节点本身：胶囊 */
  border-radius: 20px !important;
  overflow: visible;
}

.field-select :deep(.el-select) {
  background: transparent !important;
  --el-bg-color: transparent;
  --el-fill-color-blank: transparent;
  --el-border-radius: 20px;
  --el-border-radius-base: 20px;
  --el-input-border-radius: 20px;
  border-radius: 20px !important;
  overflow: visible;
}

/*
 * 新版 ElementPlus v2 el-select 的trigger主体：
 * .el-select__wrapper / .el-input / .el-input__wrapper 三层都必须是圆角+溢出可见
 * 否则4px边框和胶囊会被外层overflow:hidden裁成直角。
 * 为了让UI边界对用户可见，在 el-select__wrapper 和 el-input__wrapper 双层
 * 都画上 #848484 淡灰边框（兼容不同 ElementPlus 版本对 wrapper 的差异）。
 */
.field-select :deep(.el-select__wrapper) {
  border-radius: 20px !important;
  background: transparent !important;
  box-shadow: none !important;
  overflow: visible;
  /* 可见的淡灰边框，和 input__wrapper 边框保持一致的颜色与厚度 */
  border: 4px solid #848484 !important;
  display: flex !important;
  align-items: center !important;
  justify-content: center !important;
  transition: border-color 0.2s ease, background-color 0.2s ease;
}
.field-select :deep(.el-select__wrapper:hover) {
  border-color: #9a9a9a !important;
  background: rgba(255, 255, 255, 0.06) !important;
}
.field-select :deep(.el-select__wrapper.is-focused),
.field-select :deep(.el-select__wrapper.is-focus) {
  border-color: var(--accent-primary, #4a9eff) !important;
  background: rgba(255, 255, 255, 0.04) !important;
  box-shadow: 0 0 0 1px rgba(74, 158, 255, 0.2) inset;
}

.field-select :deep(.el-input) {
  border-radius: 20px !important;
  --el-input-border-radius: 20px;
  overflow: visible;
}

/*
 * 胶囊下拉框：
 * - 4px 灰色(#848484)边框
 * - 高度40px，border-radius=20px（pill胶囊形）
 * - 保持透明背景，仅悬停/获取焦点时给极弱底色
 */
.field-select :deep(.el-input__wrapper) {
  background-color: transparent !important;
  background: transparent !important;
  border: 4px solid #848484 !important;
  box-shadow: none !important;
  border-radius: 20px !important;
  padding: 2px 14px;
  height: 40px;
  min-height: 40px;
  box-sizing: border-box;
  overflow: visible;
  transition: border-color 0.2s ease, background-color 0.2s ease;
}

.field-select :deep(.el-input__wrapper:hover) {
  border-color: #9a9a9a !important;
  background: rgba(255, 255, 255, 0.06) !important;
  border-radius: 20px !important;
}

.field-select :deep(.el-input__wrapper.is-focus) {
  border-color: var(--accent-primary, #4a9eff) !important;
  background: rgba(255, 255, 255, 0.04) !important;
  border-radius: 20px !important;
  box-shadow: 0 0 0 1px rgba(74, 158, 255, 0.2) inset;
}

.field-select :deep(.el-input__inner) {
  color: var(--text-primary);
  text-align: center !important;
  background: transparent !important;
  line-height: 32px;
  height: 32px;
  /* 垂直居中：消除上下 padding 与默认 baseline 对齐偏差 */
  display: flex !important;
  align-items: center !important;
  justify-content: center !important;
  padding: 0 !important;
  vertical-align: middle;
}

/* el-select v2 选中项/占位符文字精确居中 */
.field-select :deep(.el-select__selected-item),
.field-select :deep(.el-select__placeholder) {
  justify-content: center !important;
  text-align: center !important;
  margin: 0 auto !important;
}

.field-select :deep(.el-select__caret) {
  color: var(--text-secondary);
}

/* Mods目录浏览按钮：胶囊 + 灰色#848484背景 + 0.4透明度（文件夹图标） */
.browse-folder-btn {
  background: #848484 !important;
  background-color: #848484 !important;
  border: 2px solid #848484 !important;
  color: var(--text-primary) !important;
  opacity: 0.4;
  padding: 0 14px !important;
  height: 36px;
  /* 胶囊 */
  border-radius: 18px !important;
  box-shadow: none !important;
  transition: all 0.15s ease;
}
.browse-folder-btn:hover {
  opacity: 0.6;
  background: #848484 !important;
  background-color: #848484 !important;
  border-color: #9a9a9a !important;
  color: #fff !important;
}
.browse-folder-btn .el-icon {
  font-size: 18px;
}

/* 还原区按钮：胶囊 + 灰色#848484背景 + 0.4透明度（大面积虚线边框风格保持，但背景改为纯色灰） */
.restore-zone-btn {
  width: 100%;
  padding: 24px 20px;
  background-color: #848484 !important;
  border: 2px dashed;
  /* 胶囊 */
  border-radius: 999px;
  color: var(--text-primary);
  cursor: pointer;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  font-family: inherit;
  opacity: 0.8;
  transition: opacity 0.2s ease, background-color 0.2s ease, border-color 0.2s ease, transform 0.08s ease;
}

.restore-zone-btn:hover {
  opacity: 0.85;
  background: #848484 !important;
  border-color: #9a9a9a;
}

.restore-zone-btn:active {
  transform: scale(0.995);
  opacity: 0.75;
}

/* 还原区拖拽高亮反馈 */
.restore-zone-btn.drag-over {
  border-color: var(--accent-primary, #4a9eff) !important;
  background: rgba(74, 158, 255, 0.12) !important;
  opacity: 1;
  box-shadow: 0 0 0 2px rgba(74, 158, 255, 0.25) inset;
}

/* 还原操作进行中禁用态 */
.restore-zone-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.restore-icon {
  color: var(--text-muted);
  margin-bottom: 6px;
}

.restore-main {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
  text-align: center;
  line-height: 1.4;
}

.restore-desc {
  margin: 4px 0 0;
  font-size: 12px;
  color: var(--text-primary);
  text-align: center;
  line-height: 1.5;
  max-width: 520px;
  white-space: pre-line;
}

.normal-btn {
  background: #848484 !important;
  background-color: #848484 !important;
  border: 2px solid #848484 !important;
  /* 胶囊 */
  border-radius: 999px !important;
  color: var(--text-primary) !important;
  opacity: 0.4;
  font-size: 13px;
  padding: 0 20px !important;
  height: 36px;
  box-shadow: none !important;
  transition: opacity 0.2s ease, background-color 0.2s ease, border-color 0.2s ease, color 0.2s ease;
  width: 98%;
  /* 文字精确居中 */
  display: inline-flex !important;
  align-items: center !important;
  justify-content: center !important;
  text-align: center !important;
  line-height: 1 !important;
  white-space: nowrap;
}

.normal-btn:hover {
  opacity: 0.6;
  background: #848484 !important;
  background-color: #848484 !important;
  border-color: #9a9a9a !important;
}

/* 底部三按钮 */
.bottom-triplet {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  justify-content: center;
  border-bottom: none;
  padding-top: 16px;
}

/* 底部链接 */
.bottom-links {
  display: flex;
  justify-content: center;
  align-items: center;
  gap: 8px;
  margin-top: 16px;
  padding-top: 14px;
  padding-bottom: 8px;
}

.link-item {
  background: none;
  border: none;
  padding: 4px 6px;
  color: var(--text-muted);
  font-size: 12px;
  text-decoration: none;
  cursor: pointer;
  font-family: inherit;
  transition: color 0.2s ease, background-color 0.2s ease;
  border-radius: 4px;
}

.link-item:hover {
  color: var(--text-secondary);
  background: rgba(255, 255, 255, 0.05);
}

.link-item:focus-visible {
  outline: 2px solid var(--accent-primary, #4a9eff);
  outline-offset: 1px;
}

.link-separator {
  color: var(--text-muted);
  font-size: 12px;
}

/* 版本号右下角 */
.version-tag {
  position: absolute;
  right: 16px;
  bottom: 64px;
  font-size: 11px;
  color: var(--text-muted);
  z-index: 10;
  user-select: none;
  letter-spacing: 0.3px;
}

/* 滑块通用样式微调 */
:deep(.el-slider__runway) {
  background: rgba(255, 255, 255, 0.12);
}

:deep(.el-slider__bar) {
  background: var(--accent-primary, #4a9eff);
}

:deep(.el-slider__button) {
  border-color: var(--accent-primary, #4a9eff);
  background: #121212;
  box-shadow: 0 0 0 2px var(--accent-primary, #4a9eff), 0 2px 6px rgba(0, 0, 0, 0.3);
}

:deep(.el-slider__stop) {
  background: rgba(255, 255, 255, 0.28);
}

/* 响应式：窄屏 进程/路径上下排列 */
@media (max-width: 680px) {
  .process-path-row {
    grid-template-columns: 1fr;
    gap: 16px;
  }
  .big-btn-card {
    width: 94%;
  }
}
</style>

<!--
  设置页下拉弹层（被ElementPlus teleport到body）的 #848484 淡灰边框
  通过每个 el-select 上配置的 popper-class="settings-select-dropdown" 命中。
  为避免“双层边框”，只在 .el-popper 外层画一次边框，内层 .el-select-dropdown 置为无边框。
-->
<style>
.settings-select-dropdown.el-popper,
.settings-select-dropdown.el-popper.is-light {
  border: 1px solid #848484 !important;
  background: transparent;
  --el-popper-border-color: #848484;
  --el-popper-border-radius: 8px;
  border-radius: 8px !important;
}
.settings-select-dropdown .el-select-dropdown {
  border: none !important;
  border-radius: 8px;
}
.settings-select-dropdown .el-select-dropdown__wrap {
  border-radius: 8px;
}
/* 选项item也要有轻微底纹，边界更可见 */
.settings-select-dropdown .el-select-dropdown__item {
  color: #e5e7eb;
}
.settings-select-dropdown .el-select-dropdown__item.hover,
.settings-select-dropdown .el-select-dropdown__item:hover {
  background: rgba(132, 132, 132, 0.16);
  color: #ffffff;
}
.settings-select-dropdown .el-select-dropdown__item.selected {
  background: rgba(74, 158, 255, 0.18);
  color: #ffffff;
  font-weight: 600;
}

/* 还原结果对话框：右下角关闭按钮（蓝色字体、胶囊框、无背景色，悬停淡蓝色背景） */
.restore-result-dialog.el-message-box {
  background: #1a1a1a;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 12px;
}
.restore-result-dialog.el-message-box .el-message-box__title {
  color: #ffffff;
  font-weight: 600;
}
.restore-result-dialog.el-message-box .el-message-box__content {
  color: #e5e7eb;
  white-space: pre-line;
  line-height: 1.6;
  text-align: left;
}
.restore-result-dialog .el-button.restore-result-close-btn {
  background: transparent !important;
  border: none !important;
  color: #4a9eff !important;
  border-radius: 999px !important;
  padding: 0 20px !important;
  height: 32px;
  box-shadow: none !important;
  transition: background-color 0.2s ease, color 0.2s ease;
}
.restore-result-dialog .el-button.restore-result-close-btn:hover {
  background: rgba(74, 158, 255, 0.15) !important;
  color: #7ab8ff !important;
}
</style>
