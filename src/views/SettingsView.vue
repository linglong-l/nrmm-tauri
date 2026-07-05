<script setup lang="ts">
/**
 * SettingsView.vue - 设置页面组件
 *
 * 作用：
 *  - 提供应用全部设置的可视化管理界面，采用左侧 Tab + 右侧表单的布局。
 *  - 包含 6 个设置分类：Game（游戏路径与目标进程）、Hotkeys（热键）、
 *    Display（显示：缩放/透明度/布局/主题/语言）、Features（功能开关）、
 *    Management（管理：更新模组数据、导入/导出/重置设置、打开目录）、About（关于）。
 *  - 每个设置项变更后会立即持久化到后端（saveSettings）。
 *  - Mods 路径变更后实时校验有效性，并显示成功/错误状态。
 *
 * 业务逻辑：
 *  - "Update Mod Data" 用于在用户通过文件管理器直接增删改模组后，重新同步后端的模组索引。
 *  - 语言切换需重启应用才能完全生效。
 */
import { ref, computed, onMounted, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { ElMessage, ElMessageBox, ElNotification } from 'element-plus';
import { Folder, Delete } from '@element-plus/icons-vue';
import { useSettingsStore } from '../stores/settings';
import { useGameStore } from '../stores/game';
import {
  invokeUpdateModData, invokeValidateModsPath, invokeOpenModFolder,
  invokeSelectDirectory, invokeLoadMods,
  invokeFindIniFiles, invokeProcessIniFiles
} from '../utils/invoke';
import { TargetGame, HotkeyKeyboard, HotkeyGamepad, LayoutMode, SortGroupMethod, ModsPathStatus } from '../types';
import {
  HOTKEY_KEYBOARD_NAMES, HOTKEY_GAMEPAD_NAMES,
  LAYOUT_MODE_NAMES, SORT_GROUP_METHOD_NAMES, SUPPORTED_LANGUAGES,
  MODS_PATH_STATUS_DESCRIPTIONS, getGameNameKey
} from '../utils/constants';
import { validateHotkeys } from '../utils/hotkeyValidator';
import { EventNames, eventManager } from '../utils/events';
import { getVersion } from '@tauri-apps/api/app';

const { t } = useI18n();
const settingsStore = useSettingsStore();
const gameStore = useGameStore();

// 当前激活的设置 Tab（默认游戏设置）
const activeTab = ref('game');
// 是否正在执行"更新模组数据"操作（用于按钮 loading 状态）
const isUpdatingModData = ref(false);
// 是否正在打开文件夹选择对话框（防止重复触发）
const isBrowsingFolder = ref(false);
// "更新模组数据"操作的输出日志
const updateModDataLog = ref('');
// 应用版本号（从 Tauri API 获取）
const appVersion = ref('');
// 各游戏的 Mods 路径校验状态映射表；null 表示尚未校验
const pathValidationStatus = ref<Record<TargetGame, ModsPathStatus | null>>({
  [TargetGame.none]: null,
  [TargetGame.Wuthering_Waves]: null,
  [TargetGame.Genshin_Impact]: null,
  [TargetGame.Honkai_Star_Rail]: null,
  [TargetGame.Zenless_Zone_Zero]: null,
  [TargetGame.Arknights_Endfield]: null
});

// 支持的游戏列表（不含 none）
const games = [
  TargetGame.Wuthering_Waves,
  TargetGame.Genshin_Impact,
  TargetGame.Honkai_Star_Rail,
  TargetGame.Zenless_Zone_Zero,
  TargetGame.Arknights_Endfield
];

// 键盘热键可选项：模板中直接遍历 HOTKEY_KEYBOARD_NAMES 生成，无需在此预构选项数组。

// 手柄热键可选项
const hotkeyGamepadOptions = Object.values(HotkeyGamepad).map(key => ({
  value: key,
  label: HOTKEY_GAMEPAD_NAMES[key as HotkeyGamepad]
}));

// 布局模式可选项（过滤掉字符串枚举值，仅保留数字枚举）
const layoutModeOptions = Object.values(LayoutMode).filter(v => typeof v === 'number').map(key => ({
  value: key as LayoutMode,
  label: LAYOUT_MODE_NAMES[key as LayoutMode]
}));

// 分组排序方式可选项
const sortGroupMethodOptions = Object.values(SortGroupMethod).filter(v => typeof v === 'number').map(key => ({
  value: key as SortGroupMethod,
  label: SORT_GROUP_METHOD_NAMES[key as SortGroupMethod]
}));

// 主题可选项
const themeOptions = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'system', label: 'System' }
];

// 以下 computed 都是设置项的双向绑定代理：get 读 store，set 调用对应的 handle* 函数（更新 store + 持久化）
const scaleValue = computed({
  get: () => settingsStore.overallScale,
  set: (val: number) => handleOverallScaleChange(val)
});

const transparencyValue = computed({
  get: () => settingsStore.bgTransparency,
  set: (val: number) => handleBgTransparencyChange(val)
});

const layoutModeValue = computed({
  get: () => settingsStore.layoutMode,
  set: (val: LayoutMode) => handleLayoutModeChange(val)
});

const themeValue = computed({
  get: () => settingsStore.theme,
  set: (val: string) => handleThemeChange(val)
});

const languageValue = computed({
  get: () => settingsStore.language,
  set: (val: string) => handleLanguageChange(val)
});

const autoGenerateFolderIconValue = computed({
  get: () => settingsStore.isAutoGenerateFolderIcon,
  set: (val: boolean) => handleAutoGenerateFolderIconChange(val)
});

const autoPinWindowValue = computed({
  get: () => settingsStore.isAutoPinWindow,
  set: (val: boolean) => handleAutoPinWindowChange(val)
});

const showMenuOutsideGameValue = computed({
  get: () => settingsStore.showMenuWhenTogglingOutsideGame,
  set: (val: boolean) => handleShowMenuWhenTogglingOutsideGameChange(val)
});

const keybindSimulateKeypressValue = computed({
  get: () => settingsStore.keybindSimulateKeypress,
  set: (val: boolean) => handleKeybindSimulateKeypressChange(val)
});

const sortGroupMethodValue = computed({
  get: () => settingsStore.sortGroupMethod,
  set: (val: SortGroupMethod) => handleSortGroupMethodChange(val)
});

/**
 * 游戏选择器的双向绑定代理。
 * getter 从 gameStore 读取，setter 调用完整的状态更新链路，
 * 确保 modsPath 更新和 GAME_SWITCHED 事件发射。
 */
const selectedGame = computed({
  get: () => gameStore.targetGame,
  set: (val: TargetGame) => {
    handleGameChange(val);
  }
});

/**
 * 分组搜索快捷键双向绑定代理。
 * getter 从 settingsStore 读取，setter 调用 setGroupSearchHotkey 写回内存值。
 */
const groupSearchHotkeyProxy = computed({
  get: () => settingsStore.groupSearchHotkey,
  set: (val: string) => settingsStore.setGroupSearchHotkey(val),
});

/**
 * 模组搜索快捷键双向绑定代理。
 * getter 从 settingsStore 读取，setter 调用 setModSearchHotkey 写回内存值。
 */
const modSearchHotkeyProxy = computed({
  get: () => settingsStore.modSearchHotkey,
  set: (val: string) => settingsStore.setModSearchHotkey(val),
});

/**
 * 快捷键冲突检测结果。
 * 基于窗口切换、分组搜索、模组搜索三类快捷键计算冲突列表，
 * 用于在界面中以 el-alert 形式逐条提示。
 */
const hotkeyConflicts = computed(() => {
  return validateHotkeys(
    settingsStore.settings?.hotkeyKeyboard ?? 'altW',
    settingsStore.groupSearchHotkey,
    settingsStore.modSearchHotkey
  ).conflicts;
});

/**
 * 处理游戏切换：更新 gameStore 和 settingsStore。
 * 注意：gameStore.setTargetGame 内部已同步更新 settingsStore 并保存设置，
 * 此处无需重复调用 saveSettings。
 * @param game 目标游戏
 */
function handleGameChange(game: TargetGame) {
  gameStore.setTargetGame(game);
}

/**
 * 获取指定游戏的 Mods 路径。
 * @param game 目标游戏
 * @returns 配置的路径字符串（可能为空）
 */
function getGameModsPath(game: TargetGame): string {
  return settingsStore.getModsPath(game);
}

/**
 * 各游戏目标进程名的响应式计算映射。
 * 使用 computed 确保设置变化时输入框能响应式更新。
 */
const targetProcessMap = computed(() => {
  const map: Record<TargetGame, string> = {} as Record<TargetGame, string>;
  for (const game of games) {
    map[game] = settingsStore.getTargetProcess(game);
  }
  return map;
});

/**
 * 获取指定游戏的默认目标进程名。
 * 用作输入框占位符提示。
 * @param game 目标游戏
 */
function getDefaultTargetProcess(game: TargetGame): string {
  switch (game) {
    case TargetGame.Wuthering_Waves:
      return 'Wuthering Waves.exe';
    case TargetGame.Genshin_Impact:
      return 'GenshinImpact.exe';
    case TargetGame.Honkai_Star_Rail:
      return 'StarRail.exe';
    case TargetGame.Zenless_Zone_Zero:
      return 'ZenlessZoneZero.exe';
    case TargetGame.Arknights_Endfield:
      return 'Endfield-Win64-Shipping.exe';
    default:
      return '';
  }
}

/**
 * 创建防抖函数
 * @param fn 需要防抖的异步函数
 * @param delay 延迟毫秒
 */
function createDebounce<T extends (...args: any[]) => Promise<void>>(
  fn: T,
  delay: number
): (...args: Parameters<T>) => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  return (...args: Parameters<T>) => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      fn(...args);
    }, delay);
  };
}

/**
 * 处理 Mods 路径变更：更新 store、保存设置、立即校验路径有效性。
 * 若修改的是当前游戏的路径，同步更新 gameStore.modsPath。
 * @param game 目标游戏
 * @param path 新的路径
 */
async function handleModsPathChange(game: TargetGame, path: string) {
  settingsStore.setModsPath(game, path);
  await settingsStore.saveSettings();
  if (gameStore.targetGame === game) {
    gameStore.setModsPath(path);
  }
  await validateModsPath(game, path);
}

/**
 * 校验指定游戏的 Mods 路径有效性。
 * @param game 目标游戏
 * @param path 待校验的路径
 * 业务逻辑：路径为空时清空校验状态；调用后端校验并更新 pathValidationStatus。
 */
async function validateModsPath(game: TargetGame, path: string) {
  if (!path) {
    pathValidationStatus.value[game] = null;
    return;
  }
  try {
    const status = await invokeValidateModsPath(path, game);
    pathValidationStatus.value[game] = status;
  } catch {
    // 后端调用异常时，默认标记为"路径不存在"
    pathValidationStatus.value[game] = ModsPathStatus.invalidNotExist;
  }
}

/**
 * 获取路径校验状态对应的 Element Plus input status 类型。
 * @param game 目标游戏
 * @returns 'success' | 'error' | 'warning' | ''（未校验）
 */
function getPathStatusType(game: TargetGame): 'success' | 'error' | 'warning' | '' {
  const status = pathValidationStatus.value[game];
  if (status === null) return '';
  if (status === ModsPathStatus.valid) return 'success';
  return 'error';
}

/**
 * 获取路径校验状态对应的本地化描述文本。
 * @param game 目标游戏
 */
function getPathStatusText(game: TargetGame): string {
  const status = pathValidationStatus.value[game];
  if (status === null) return '';
  return MODS_PATH_STATUS_DESCRIPTIONS[status];
}

/**
 * 打开系统文件夹选择对话框，并将所选路径应用到指定游戏。
 * 限制：通过 isBrowsingFolder 防止重复触发。
 * @param game 目标游戏
 */
async function browseFolder(game: TargetGame) {
  if (isBrowsingFolder.value) return;
  isBrowsingFolder.value = true;
  try {
    const selected = await invokeSelectDirectory();
    if (selected && typeof selected === 'string') {
      await handleModsPathChange(game, selected);
    }
  } catch (error) {
    console.error('Failed to open folder dialog:', error);
    ElMessage.error(t('Failed to open folder dialog'));
  } finally {
    isBrowsingFolder.value = false;
  }
}

/**
 * 处理目标进程名变更：更新 store 并持久化。
 * @param game 目标游戏
 * @param processName 进程名（如 Game.exe）
 */
async function handleTargetProcessChange(game: TargetGame, processName: string) {
  settingsStore.setTargetProcess(game, processName);
  await settingsStore.saveSettings();
}

// 防抖版本：避免输入框每次按键都触发保存导致表单禁用、输入失焦
const debouncedHandleModsPathChange = createDebounce(handleModsPathChange, 500);
const debouncedHandleTargetProcessChange = createDebounce(handleTargetProcessChange, 500);

/** 键盘热键变更处理 */
async function handleHotkeyKeyboardChange(value: HotkeyKeyboard) {
  settingsStore.setHotkeyKeyboard(value);
  await settingsStore.saveSettings();
  // 保存设置后，后端 save_settings 命令会检测热键配置变化并自动调用
  // HotkeyManager::register_from_settings() 完成系统级热键重注册。
  // 前端无需再调用 index.vue 的 registerHotkeys()，避免与后端管理流程竞争导致旧键残留。
  ElMessage.success(t('Hotkey updated, old hotkey has been unregistered'));
}

/** 手柄热键变更处理 */
async function handleHotkeyGamepadChange(value: HotkeyGamepad) {
  settingsStore.setHotkeyGamepad(value);
  await settingsStore.saveSettings();
}

/**
 * 搜索快捷键变更处理。
 * 仅保存设置到后端，无需向后端注册热键（窗口内快捷键由前端监听）。
 */
async function handleSearchHotkeyChange() {
  await settingsStore.saveSettings();
}

/** 整体缩放变更处理 */
async function handleOverallScaleChange(value: number) {
  settingsStore.setOverallScale(value);
  await settingsStore.saveSettings();
}

/** 背景透明度变更处理 */
async function handleBgTransparencyChange(value: number) {
  settingsStore.setBgTransparency(value);
  await settingsStore.saveSettings();
}

/** 布局模式变更处理 */
async function handleLayoutModeChange(value: LayoutMode) {
  settingsStore.setLayoutMode(value);
  await settingsStore.saveSettings();
}

/** 主题变更处理 */
async function handleThemeChange(value: string) {
  settingsStore.setTheme(value);
  await settingsStore.saveSettings();
}

/**
 * 语言变更处理：保存设置并提示用户重启应用。
 * 限制：语言切换需重启才能完全生效（i18n 资源与 Element Plus 语言包需重新初始化）。
 */
async function handleLanguageChange(value: string) {
  settingsStore.setLanguage(value);
  await settingsStore.saveSettings();
  ElMessage.info(t('Language changed, please Restart.'));
}

/** 自动生成文件夹图标开关变更处理 */
async function handleAutoGenerateFolderIconChange(value: boolean) {
  settingsStore.setAutoGenerateFolderIcon(value);
  await settingsStore.saveSettings();
}

/** 自动置顶窗口开关变更处理 */
async function handleAutoPinWindowChange(value: boolean) {
  settingsStore.setAutoPinWindow(value);
  await settingsStore.saveSettings();
}

/** 游戏外切换时显示托盘菜单开关变更处理 */
async function handleShowMenuWhenTogglingOutsideGameChange(value: boolean) {
  settingsStore.setShowMenuWhenTogglingOutsideGame(value);
  await settingsStore.saveSettings();
}

/** 点击 keybind 时模拟按键开关变更处理 */
async function handleKeybindSimulateKeypressChange(value: boolean) {
  settingsStore.setKeybindSimulateKeypress(value);
  await settingsStore.saveSettings();
}

/** 分组排序方式变更处理 */
async function handleSortGroupMethodChange(value: SortGroupMethod) {
  settingsStore.setSortGroupMethod(value);
  await settingsStore.saveSettings();
}

/**
 * 触发"更新模组数据"操作。
 * 业务逻辑：
 *  - 必须先选择游戏，否则提示警告。
 *  - 弹窗二次确认后调用后端 invokeUpdateModData。
 *  - 成功/失败均通过 ElNotification 与日志文本框反馈。
 * 限制：操作期间禁用按钮（loading 状态）。
 */
async function handleUpdateModData() {
  const game = gameStore.targetGame;
  if (game === TargetGame.none) {
    ElMessage.warning(t('Please select a game first.'));
    return;
  }

  try {
    await ElMessageBox.confirm(
      t('Press this after you add/remove/edit/fix mods (usually when add/edit/remove mods directly via File Explorer)'),
      t('Update Mod Data'),
      {
        confirmButtonText: t('Confirm'),
        cancelButtonText: t('Cancel'),
        type: 'info'
      }
    );
  } catch {
    // 用户取消，直接返回
    return;
  }

  isUpdatingModData.value = true;
  updateModDataLog.value = '';

  try {
    const result = await invokeUpdateModData(game);
    if (result.success) {
      updateModDataLog.value = t('Update Mod Data completed successfully!');
      ElNotification({
        message: t('Mods successfully managed!'),
        type: 'success'
      });
      // 更新模组数据成功后，重新加载模组列表并通知前端更新
      // 注意：仅通过事件通知，由 index.vue 的 MOD_GROUPS_UPDATED 监听器和
      // ModsTab.vue 的 MODS_UPDATED 监听器统一调用 setModGroups，避免冗余更新
      try {
        const groups = await invokeLoadMods(game);
        gameStore.setModsLoaded(true);
        eventManager.emitLocal(EventNames.MOD_GROUPS_UPDATED, groups);
        eventManager.emitLocal(EventNames.MODS_UPDATED, groups);
      } catch {
        // 刷新失败不影响主流程，静默忽略
      }
    } else {
      updateModDataLog.value = result.errorMessage || t('Unknown error occurred.');
      ElNotification({
        message: result.errorMessage || t('Unexpected error!'),
        type: 'error'
      });
    }
  } catch (error) {
    updateModDataLog.value = `Error: ${error}`;
    ElNotification({
      message: t('Unexpected error!'),
      type: 'error'
    });
  } finally {
    isUpdatingModData.value = false;
  }
}

// 还原区相关状态
const isRestoreZoneDragging = ref(false);
const restoreZoneFiles = ref<Array<{ name: string; path: string }>>([]);
const isProcessingRestore = ref(false);

/**
 * 处理还原区文件拖拽悬停事件。
 * 设置拖拽状态，用于触发样式变化。
 */
function onRestoreZoneDragOver(event: DragEvent) {
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = 'copy';
  }
  isRestoreZoneDragging.value = true;
}

/**
 * 处理还原区文件拖拽离开事件。
 * 重置拖拽状态。
 */
function onRestoreZoneDragLeave() {
  isRestoreZoneDragging.value = false;
}

/**
 * 处理还原区文件放置事件。
 * 从拖拽数据中提取文件路径，校验有效性后添加到还原区列表。
 */
async function onRestoreZoneDrop(event: DragEvent) {
  isRestoreZoneDragging.value = false;
  const items = event.dataTransfer?.items;
  const files = event.dataTransfer?.files;
  if (!items && !files) return;

  const validFiles: Array<{ name: string; path: string }> = [];

  if (items) {
    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (item.kind === 'file') {
        const entry = item.webkitGetAsEntry();
        if (entry) {
          if (entry.isFile) {
            const file = item.getAsFile();
            if (file) {
              const isIniFile = file.name.toLowerCase().endsWith('.ini');
              if (isIniFile) {
                validFiles.push({ name: file.name, path: file.path! });
              } else {
                const iniFiles = await findIniFilesInPath(file.path!);
                validFiles.push(...iniFiles);
              }
            }
          } else if (entry.isDirectory) {
            const file = item.getAsFile();
            if (file) {
              const iniFiles = await findIniFilesInPath(file.path!);
              validFiles.push(...iniFiles);
            }
          }
        }
      }
    }
  } else if (files) {
    for (let i = 0; i < files.length; i++) {
      const file = files[i];
      const isIniFile = file.name.toLowerCase().endsWith('.ini');
      if (isIniFile) {
        validFiles.push({ name: file.name, path: file.path! });
      } else {
        const iniFiles = await findIniFilesInPath(file.path!);
        validFiles.push(...iniFiles);
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

/**
 * 使用后端 BFS 算法查找指定路径下的所有 .ini 文件。
 * @param path 起始路径（文件或目录）
 * @returns .ini 文件列表（包含文件名和完整路径）
 */
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

/**
 * 从还原区列表中移除指定索引的文件。
 * @param index 文件在列表中的索引
 */
function removeRestoreZoneFile(index: number) {
  restoreZoneFiles.value.splice(index, 1);
}

/**
 * 清空还原区文件列表。
 */
function clearRestoreZone() {
  restoreZoneFiles.value = [];
}

/**
 * 处理还原区文件，移除其中的 xxmi 专属 INI 语句。
 * 调用后端命令批量处理所有已添加的 .ini 文件。
 */
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

/**
 * 打开当前目标游戏的 Mods 文件夹（工作目录）。
 * 限制：必须先选择游戏。
 */
async function handleOpenModFolder() {
  const game = gameStore.targetGame;
  if (game === TargetGame.none) {
    ElMessage.warning(t('Please select a game first.'));
    return;
  }
  try {
    await invokeOpenModFolder(game);
  } catch {
    ElMessage.error(t('Failed to open mod folder.'));
  }
}

/** 在新标签页打开 GitHub 仓库链接 */
function openGitHubLink() {
  window.open('https://github.com/Aglglg/No-Reload-Mod-Manager', '_blank');
}

/**
 * 缩放滑块 tooltip 格式化：将数值转为 "x.xx x" 形式。
 * @param val 滑块当前值（0.5~2.0）
 */
function formatScaleTooltip(val: number): string {
  return `${val.toFixed(2)}x`;
}

/**
 * 透明度滑块 tooltip 格式化：将数值转为百分比字符串。
 * @param val 滑块当前值（0.1~1.0）
 */
function formatTransparencyTooltip(val: number): string {
  return `${Math.round(val * 100)}%`;
}

/**
 * 获取应用版本号并填充到 appVersion。
 * 异常处理：获取失败时回退到 v0.1.0。
 */
function getAppVersion() {
  getVersion().then(version => {
    appVersion.value = `v${version}`;
  }).catch(() => {
    appVersion.value = 'v0.1.0';
  });
}

// 组件挂载：若设置未加载，则对每个游戏已配置的路径进行校验；并获取应用版本号
onMounted(async () => {
  if (!settingsStore.isLoaded) {
    for (const game of games) {
      const path = settingsStore.getModsPath(game);
      if (path) {
        validateModsPath(game, path);
      }
    }
  }
  getAppVersion();
});

// 监听设置加载完成，重新校验所有游戏路径
// 防止 onMounted 时设置尚未加载导致路径校验被跳过
watch(
  () => settingsStore.isLoaded,
  (loaded) => {
    if (loaded) {
      for (const game of games) {
        const path = settingsStore.getModsPath(game);
        if (path) {
          validateModsPath(game, path);
        }
      }
    }
  }
);
</script>

<template>
  <!-- 
    设置页面根容器
    作用：承载整个设置界面的布局和导航
    布局：左侧 Tab 导航 + 右侧内容区域
  -->
  <div class="settings-container">
    <!-- 
      设置标签页容器
      数据来源：activeTab 控制当前激活的标签页
      布局：tab-position="left" 左侧导航布局
    -->
    <el-tabs v-model="activeTab" class="settings-tabs" tab-position="left">
      <!-- 
        游戏设置标签页
        作用：配置各游戏的 Mods 路径和目标进程
        数据来源：gameStore.targetGame (当前选中游戏), games (支持的游戏列表)
        交互行为：选择游戏、输入路径、浏览文件夹
        禁用条件：settingsStore.isSaving (正在保存) 或 isBrowsingFolder (正在浏览文件夹)
      -->
      <el-tab-pane :label="t('Game')" name="game">
        <div class="tab-content-inner">
          <!-- 
            游戏设置表单
            禁用条件：:disabled 在保存或浏览时禁用所有输入
          -->
          <el-form label-position="top" :disabled="settingsStore.isSaving || isBrowsingFolder">
            <!-- 
              选择游戏表单项
              数据来源：v-model 绑定 selectedGame (computed 代理)
              交互行为：选择游戏后触发完整状态更新链路
              配置：teleported=true 确保下拉框不被父容器裁剪
            -->
            <el-form-item :label="t('Select Game')">
              <!-- 游戏选择下拉框 -->
              <el-select
                v-model="selectedGame"
                style="width: 100%"
                :teleported="true"
                popper-class="game-select-popper"
              >
                <!-- 游戏选项列表（使用 i18n 国际化） -->
                <el-option
                  v-for="game in games"
                  :key="game"
                  :label="t(getGameNameKey(game))"
                  :value="game"
                />
              </el-select>
            </el-form-item>

            <!-- 分隔线 -->
            <div class="settings-divider" />

            <!-- 
              各游戏配置区域
              数据来源：v-for 遍历 games 数组，为每个游戏生成配置区域
            -->
            <template v-for="game in games" :key="game">
              <!-- 
                单个游戏配置区块
                数据来源：game 变量（当前遍历的游戏）
              -->
              <div class="game-section">
                <!-- 游戏名称标题（使用 i18n 国际化） -->
                <div class="section-title">{{ t(getGameNameKey(game)) }}</div>
                <!-- 
                  Mods 路径表单项
                  数据来源：getGameModsPath(game) 获取当前游戏的 Mods 路径
                  交互行为：
                    - @update:model-value 输入路径时实时更新并校验
                    - @click 浏览按钮打开文件夹选择对话框
                  动态绑定：:status 根据路径校验结果显示状态（success/error/warning）
                -->
                <el-form-item :label="t('Mods Path')">
                  <!-- Mods 路径输入框 -->
                  <el-input
                    :model-value="getGameModsPath(game)"
                    placeholder='example: D:\XXMI Launcher\Mods'
                    :status="getPathStatusType(game)"
                    @update:model-value="(val: string) => debouncedHandleModsPathChange(game, val)"
                  >
                    <!-- 浏览文件夹按钮 -->
                    <template #append>
                      <el-button @click="browseFolder(game)">
                        <el-icon><Folder /></el-icon>
                      </el-button> 
                    </template>
                  </el-input>
                  <!-- 
                    路径校验状态提示
                    条件渲染：v-if 根据 getPathStatusText(game) 是否有内容决定是否显示
                    动态绑定：:class 根据状态类型添加对应颜色类
                  -->
                  <div v-if="getPathStatusText(game)" class="path-status" :class="getPathStatusType(game)">
                    {{ getPathStatusText(game) }}
                  </div>
                </el-form-item>

                <!-- 
                  目标进程表单项
                  数据来源：targetProcessMap[game] 响应式计算属性
                  交互行为：@update:model-value 输入进程名时实时更新
                  占位符：显示默认进程名作为提示
                -->
                <el-form-item :label="t('Target Process')">
                  <!-- 目标进程输入框 -->
                  <el-input
                    :model-value="targetProcessMap[game]"
                    :placeholder="getDefaultTargetProcess(game)"
                    @update:model-value="(val: string) => debouncedHandleTargetProcessChange(game, val)"
                  />
                </el-form-item>
              </div>
            </template>
          </el-form>
        </div>
      </el-tab-pane>

      <!-- 
        热键设置标签页
        作用：配置窗口切换热键和导航热键
        数据来源：settingsStore.settings.hotkeyKeyboard (键盘热键), hotkeyGamepad (手柄热键)
        交互行为：选择热键组合
        禁用条件：settingsStore.isSaving (正在保存)
      -->
      <el-tab-pane :label="t('Hotkeys')" name="hotkey">
        <div class="tab-content-inner">
          <!-- 热键设置表单 -->
          <el-form label-position="top" :disabled="settingsStore.isSaving">
            <!--
              手柄热键表单项
              数据来源：v-model 绑定 settingsStore.settings.hotkeyGamepad
              交互行为：@change 选择后调用 handleHotkeyGamepadChange 保存
            -->
            <el-form-item :label="t('Gamepad(XInput) Toggle')">
              <!-- 手柄热键选择下拉框 -->
              <el-select
                v-model="settingsStore.settings.hotkeyGamepad"
                style="width: 100%"
                @change="handleHotkeyGamepadChange"
              >
                <!-- 手柄热键选项列表 -->
                <el-option
                  v-for="opt in hotkeyGamepadOptions"
                  :key="opt.value"
                  :label="opt.label"
                  :value="opt.value"
                />
              </el-select>
            </el-form-item>

            <!-- 分隔线 -->
            <div class="settings-divider" />

            <!--
              快捷键网格区域
              作用：以响应式 CSS Grid 布局展示 4 个键盘快捷键配置项
              布局：默认每行 3 个，窗口缩窄时自适应
              包含：窗口切换、分组搜索、模组搜索、切换搜索栏
            -->
            <div class="hotkey-grid">
              <!-- 窗口切换键盘热键 -->
              <div class="hotkey-grid-item">
                <label class="hotkey-grid-label">{{ t('Keyboard Toggle') }}</label>
                <el-select
                  v-model="settingsStore.settings.hotkeyKeyboard"
                  style="width: 100%"
                  @change="handleHotkeyKeyboardChange"
                >
                  <el-option
                    v-for="(label, key) in HOTKEY_KEYBOARD_NAMES"
                    :key="key"
                    :label="label"
                    :value="key"
                  />
                </el-select>
              </div>
              <!-- 分组搜索快捷键 -->
              <div class="hotkey-grid-item">
                <label class="hotkey-grid-label">{{ t('settings.groupSearchHotkey') }}</label>
                <el-select
                  v-model="groupSearchHotkeyProxy"
                  style="width: 100%"
                  @change="handleSearchHotkeyChange"
                >
                  <el-option
                    v-for="(label, key) in HOTKEY_KEYBOARD_NAMES"
                    :key="key"
                    :label="label"
                    :value="key"
                  />
                </el-select>
              </div>
              <!-- 模组搜索快捷键 -->
              <div class="hotkey-grid-item">
                <label class="hotkey-grid-label">{{ t('settings.modSearchHotkey') }}</label>
                <el-select
                  v-model="modSearchHotkeyProxy"
                  style="width: 100%"
                  @change="handleSearchHotkeyChange"
                >
                  <el-option
                    v-for="(label, key) in HOTKEY_KEYBOARD_NAMES"
                    :key="key"
                    :label="label"
                    :value="key"
                  />
                </el-select>
              </div>
            </div>

            <!--
              快捷键冲突提示区域
              作用：当三类快捷键之间存在重复时，逐条展示冲突信息
            -->
            <el-alert
              v-for="(conflict, index) in hotkeyConflicts"
              :key="index"
              :title="conflict.message"
              type="error"
              :closable="false"
              show-icon
              style="margin-bottom: 8px;"
            />

            <!--
              快捷键说明提示区域
              作用：分别说明窗口切换、分组搜索、模组搜索三类快捷键的作用
            -->
            <div class="hotkey-hint">
              <p>{{ t('settings.hotkeyHintWindow') }}</p>
              <p>{{ t('settings.hotkeyHintGroupSearch') }}</p>
              <p>{{ t('settings.hotkeyHintModSearch') }}</p>
            </div>
          </el-form>
        </div>
      </el-tab-pane>

      <!-- 
        显示设置标签页
        作用：配置界面显示相关参数（缩放、透明度、布局、主题、语言）
        数据来源：
          - scaleValue (整体缩放), transparencyValue (背景透明度)
          - layoutModeValue (布局模式), themeValue (主题), languageValue (语言)
        交互行为：拖动滑块、选择下拉选项
        禁用条件：settingsStore.isSaving (正在保存)
      -->
      <el-tab-pane :label="t('Display')" name="display">
        <div class="tab-content-inner">
          <!-- 显示设置表单 -->
          <el-form label-position="top" :disabled="settingsStore.isSaving">
            <!-- 
              整体缩放表单项
              数据来源：v-model 绑定 scaleValue (computed 代理)
              交互行为：拖动滑块实时调整缩放比例
              范围：0.5x ~ 2.0x，步长 0.05
              提示：formatScaleTooltip 格式化显示 "x.xx x"
            -->
            <el-form-item :label="t('Overall Scale')">
              <!-- 缩放滑块 -->
              <el-slider
                v-model="scaleValue"
                :min="0.5"
                :max="2.0"
                :step="0.05"
                :show-tooltip="true"
                :format-tooltip="formatScaleTooltip"
              />
            </el-form-item>

            <!-- 
              背景透明度表单项
              数据来源：v-model 绑定 transparencyValue (computed 代理)
              交互行为：拖动滑块实时调整透明度
              范围：0.1 ~ 1.0，步长 0.05
              提示：formatTransparencyTooltip 格式化显示百分比
            -->
            <el-form-item :label="t('Background Transparency')">
              <!-- 透明度滑块 -->
              <el-slider
                v-model="transparencyValue"
                :min="0.1"
                :max="1.0"
                :step="0.05"
                :show-tooltip="true"
                :format-tooltip="formatTransparencyTooltip"
              />
            </el-form-item>

            <!-- 分隔线 -->
            <div class="settings-divider" />

            <!-- 
              布局模式表单项
              数据来源：v-model 绑定 layoutModeValue (computed 代理)
              交互行为：@change 选择后调用 handleLayoutModeChange 保存
            -->
            <el-form-item :label="t('Layout')">
              <!-- 布局模式选择下拉框 -->
              <el-select v-model="layoutModeValue" style="width: 100%">
                <!-- 布局模式选项列表 -->
                <el-option
                  v-for="opt in layoutModeOptions"
                  :key="opt.value"
                  :label="opt.label"
                  :value="opt.value"
                />
              </el-select>
            </el-form-item>

            <!-- 
              主题表单项
              数据来源：v-model 绑定 themeValue (computed 代理)
              交互行为：@change 选择后调用 handleThemeChange 保存
            -->
            <el-form-item :label="t('Theme')">
              <!-- 主题选择下拉框 -->
              <el-select v-model="themeValue" style="width: 100%">
                <!-- 主题选项列表 -->
                <el-option
                  v-for="opt in themeOptions"
                  :key="opt.value"
                  :label="opt.label"
                  :value="opt.value"
                />
              </el-select>
            </el-form-item>

            <!-- 
              语言表单项
              数据来源：v-model 绑定 languageValue (computed 代理)
              交互行为：@change 选择后调用 handleLanguageChange 保存并提示重启
            -->
            <el-form-item :label="t('Languages')">
              <!-- 语言选择下拉框 -->
              <el-select v-model="languageValue" style="width: 100%">
                <!-- 语言选项列表 -->
                <el-option
                  v-for="lang in SUPPORTED_LANGUAGES"
                  :key="lang.code"
                  :label="lang.nativeName"
                  :value="lang.code"
                />
              </el-select>
            </el-form-item>
          </el-form>
        </div>
      </el-tab-pane>

      <!-- 功能设置 -->
      <el-tab-pane :label="t('Features')" name="features">
        <div class="tab-content-inner">
          <el-form label-position="top" :disabled="settingsStore.isSaving">
            <el-form-item :label="t('Group Folder Icon')">
              <div class="switch-row">
                <el-switch v-model="autoGenerateFolderIconValue" />
                <span class="switch-label">{{ t('Auto generate folder icon when changing group icon') }}</span>
              </div>
            </el-form-item>

            <el-form-item :label="t('Auto pin window')">
              <div class="switch-row">
                <el-switch v-model="autoPinWindowValue" />
                <span class="switch-label">{{ t('Auto pin window') }}</span>
              </div>
            </el-form-item>

            <el-form-item :label="t('Show Tray Menu when toggling outside the game')">
              <div class="switch-row">
                <el-switch v-model="showMenuOutsideGameValue" />
                <span class="switch-label">{{ t('Show Tray Menu when toggling outside the game') }}</span>
              </div>
            </el-form-item>

            <el-form-item :label="t('Click keybind to simulate keypress')">
              <div class="switch-row">
                <el-switch v-model="keybindSimulateKeypressValue" />
                <span class="switch-label">{{ t('Click keybind to simulate keypress') }}</span>
              </div>
            </el-form-item>

            <div class="settings-divider" />

            <el-form-item :label="t('Sort group by')">
              <el-select v-model="sortGroupMethodValue" style="width: 100%">
                <el-option
                  v-for="opt in sortGroupMethodOptions"
                  :key="opt.value"
                  :label="opt.label"
                  :value="opt.value"
                />
              </el-select>
            </el-form-item>
          </el-form>
        </div>
      </el-tab-pane>

      <!-- 管理设置 -->
      <el-tab-pane :label="t('Management')" name="management">
        <div class="tab-content-inner">
          <el-form label-position="top" :disabled="settingsStore.isSaving">
            <el-form-item :label="t('Update Mod Data')">
              <el-button
                type="primary"
                :loading="isUpdatingModData"
                @click="handleUpdateModData"
                style="width: 100%"
              >
                {{ t('Update Mod Data') }}
              </el-button>
              <div class="description">
                {{ t('Press this after you add/remove/edit/fix mods (usually when add/edit/remove mods directly via File Explorer)') }}
              </div>
            </el-form-item>

            <el-form-item v-if="updateModDataLog" :label="t('Result')">
              <el-input
                v-model="updateModDataLog"
                type="textarea"
                :rows="4"
                readonly
              />
            </el-form-item>

            <div class="settings-divider" />

            <div class="settings-section-title">{{ t('Open Folders') }}</div>
            <el-form-item>
              <el-button @click="handleOpenModFolder" style="width: 100%">
                {{ t('Open {game} working directory', { game: gameStore.targetGame !== TargetGame.none ? t(getGameNameKey(gameStore.targetGame)) : t('Game') }) }}
              </el-button>
            </el-form-item>

            <div class="settings-divider" />

            <div class="settings-section-title">{{ t('Restore Zone') }}</div>
            <el-form-item>
              <div class="restore-zone" :class="{ 'drag-over': isRestoreZoneDragging }"
                @dragover.prevent="onRestoreZoneDragOver"
                @dragleave="onRestoreZoneDragLeave"
                @drop.prevent="onRestoreZoneDrop">
                <div class="restore-zone-header">
                  <span class="restore-zone-hint">{{ t('Drop .ini files here') }}</span>
                </div>
                <div class="restore-zone-content">
                  <div v-if="!isRestoreZoneDragging && restoreZoneFiles.length === 0" class="restore-zone-empty">
                    <p>{{ t('Drag files or directories here') }}</p>
                    <p class="restore-zone-sub">{{ t('Only .ini files will be processed') }}</p>
                  </div>
                  <div v-else-if="restoreZoneFiles.length > 0" class="restore-zone-files">
                    <div v-for="(file, index) in restoreZoneFiles" :key="index" class="restore-zone-file-item">
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
            </el-form-item>
          </el-form>
        </div>
      </el-tab-pane>

      <!-- 关于 -->
      <el-tab-pane :label="t('About')" name="about">
        <div class="tab-content-inner">
          <div class="about-section">
            <div class="app-logo">
              <img src="/tauri.svg" alt="Logo" class="logo-img" />
            </div>
            <h2 class="app-title">XXMI-NRMM</h2>
            <p class="app-version">{{ appVersion }}</p>
            <p class="app-description">
              {{ t('Mod Manager for Gacha Games') }}
            </p>
            <div class="settings-divider" />
            <div class="about-links">
              <el-button link type="primary" @click="openGitHubLink">
                GitHub
              </el-button>
            </div>
            <p class="copyright">
              © 2024 XXMI-NRMM. All rights reserved.
            </p>
          </div>
        </div>
      </el-tab-pane>
    </el-tabs>
  </div>
</template>

<style scoped>
.settings-container {
  padding: 0;
  height: 100%;
  overflow: hidden;
  background-color: transparent;
}

.settings-tabs {
  height: 100%;
}

/* Element Plus 深色主题覆盖 */
:deep(.el-tabs) {
  --el-bg-color: transparent;
  --el-bg-color-overlay: rgba(30, 30, 34, 0.95);
  --el-border-color: rgba(255, 255, 255, 0.08);
  --el-border-color-light: rgba(255, 255, 255, 0.06);
  --el-fill-color-light: rgba(255, 255, 255, 0.06);
  --el-text-color-primary: rgba(255, 255, 255, 0.85);
  --el-text-color-regular: rgba(255, 255, 255, 0.65);
  --el-text-color-secondary: rgba(255, 255, 255, 0.45);
}

:deep(.el-tabs__header) {
  background-color: rgba(0, 0, 0, 0.2);
  border-right: 1px solid rgba(255, 255, 255, 0.04);
  margin-right: 0;
  padding-top: 8px;
}

:deep(.el-tabs__content) {
  overflow-y: auto;
  height: 100%;
}

:deep(.el-tab-pane) {
  height: 100%;
  padding: 0;
}

:deep(.el-tabs__item) {
  color: rgba(255, 255, 255, 0.55);
  height: 44px;
  line-height: 44px;
  padding: 0 24px;
  font-size: 13px;
}

:deep(.el-tabs__item:hover) {
  color: rgba(255, 255, 255, 0.85);
}

:deep(.el-tabs__item.is-active) {
  color: rgba(255, 255, 255, 0.95);
  background-color: rgba(255, 255, 255, 0.06);
}

:deep(.el-tabs__nav-wrap::after) {
  display: none;
}

:deep(.el-tabs__active-bar) {
  background-color: var(--el-color-primary);
}

.tab-content-inner {
  padding: 20px 24px;
  height: 100%;
  overflow-y: auto;
}

/* 表单样式 */
:deep(.el-form-item) {
  margin-bottom: 20px;
}

:deep(.el-form-item__label) {
  color: rgba(255, 255, 255, 0.65);
  font-weight: 500;
  font-size: 13px;
  margin-bottom: 6px;
}

:deep(.el-form-item__content) {
  line-height: 1;
}

.nested-label :deep(.el-form-item__label) {
  font-size: 12px;
  color: rgba(255, 255, 255, 0.45);
}

:deep(.el-input__wrapper),
:deep(.el-textarea__inner) {
  background-color: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.08);
  box-shadow: none;
  border-radius: 8px;
}

:deep(.el-input__wrapper:hover),
:deep(.el-textarea__inner:hover) {
  border-color: rgba(255, 255, 255, 0.15);
}

:deep(.el-input__wrapper.is-focus),
:deep(.el-textarea__inner:focus) {
  border-color: var(--el-color-primary);
  box-shadow: 0 0 0 2px rgba(64, 158, 255, 0.15);
}

:deep(.el-input__inner) {
  color: rgba(255, 255, 255, 0.85);
}

:deep(.el-input__inner::placeholder) {
  color: rgba(255, 255, 255, 0.3);
}

:deep(.el-input-group__append) {
  background-color: rgba(255, 255, 255, 0.04);
  border-color: rgba(255, 255, 255, 0.08);
}

:deep(.el-input-group__append .el-button) {
  border: none;
  background: transparent;
  color: rgba(255, 255, 255, 0.6);
}

:deep(.el-select__wrapper) {
  background-color: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.08);
  box-shadow: none;
  border-radius: 8px;
}

:deep(.el-select__wrapper:hover) {
  border-color: rgba(255, 255, 255, 0.15);
}

:deep(.el-select__wrapper.is-focused) {
  border-color: var(--el-color-primary);
  box-shadow: 0 0 0 2px rgba(64, 158, 255, 0.15);
}

:deep(.el-select__placeholder) {
  color: rgba(255, 255, 255, 0.5);
}

:deep(.el-select-dropdown) {
  background-color: rgba(30, 30, 34, 0.98);
  border: 1px solid rgba(255, 255, 255, 0.08);
}

:deep(.el-select-dropdown__item) {
  color: rgba(255, 255, 255, 0.75);
}

:deep(.el-select-dropdown__item.is-hovering) {
  background-color: rgba(255, 255, 255, 0.06);
}

:deep(.el-select-dropdown__item.is-selected) {
  color: var(--el-color-primary);
  font-weight: 500;
}

/* 按钮样式 */
:deep(.el-button) {
  background-color: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.1);
  color: rgba(255, 255, 255, 0.75);
  border-radius: 8px;
}

:deep(.el-button:hover) {
  background-color: rgba(255, 255, 255, 0.1);
  border-color: rgba(255, 255, 255, 0.2);
  color: rgba(255, 255, 255, 0.9);
}

:deep(.el-button--primary) {
  background-color: var(--el-color-primary);
  border-color: var(--el-color-primary);
  color: white;
}

:deep(.el-button--primary:hover) {
  background-color: #53a8ff;
  border-color: #53a8ff;
}

:deep(.el-button--danger) {
  background-color: rgba(245, 108, 108, 0.15);
  border-color: rgba(245, 108, 108, 0.3);
  color: #f56c6c;
}

:deep(.el-button--danger:hover) {
  background-color: rgba(245, 108, 108, 0.25);
  border-color: rgba(245, 108, 108, 0.5);
}

:deep(.el-button--text) {
  background: transparent;
  border: none;
  color: var(--el-color-primary);
}

:deep(.el-button--text:hover) {
  color: #53a8ff;
}

/* 开关样式 */
:deep(.el-switch) {
  --el-switch-off-color: rgba(255, 255, 255, 0.15);
  --el-switch-on-color: var(--el-color-primary);
}

/* 滑块样式 */
:deep(.el-slider) {
  --el-slider-main-bg-color: var(--el-color-primary);
}

:deep(.el-slider__runway) {
  background-color: rgba(255, 255, 255, 0.1);
}

:deep(.el-slider__bar) {
  background-color: var(--el-color-primary);
}

:deep(.el-slider__button) {
  border-color: var(--el-color-primary);
}

/* 上传组件样式 */
:deep(.el-upload) {
  width: 100%;
}

/* 分隔线和自定义分隔样式 */
.settings-divider {
  height: 1px;
  background-color: rgba(255, 255, 255, 0.06);
  margin: 24px 0;
}

:deep(.el-divider) {
  border-color: rgba(255, 255, 255, 0.06);
  margin: 24px 0;
}

/* 游戏设置区域 */
.game-section {
  margin-bottom: 24px;
  padding-bottom: 16px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.04);
}

.game-section:last-child {
  border-bottom: none;
  margin-bottom: 0;
}

.section-title {
  font-size: 14px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.75);
  margin-bottom: 12px;
}

/* 路径状态样式 */
.path-status {
  font-size: 12px;
  margin-top: 6px;
}

.path-status.success {
  color: #67c23a;
}

.path-status.error {
  color: #f56c6c;
}

.path-status.warning {
  color: #e6a23c;
}

/* 开关行 */
.switch-row {
  display: flex;
  align-items: center;
  gap: 12px;
}

.switch-label {
  font-size: 13px;
  color: rgba(255, 255, 255, 0.55);
  line-height: 1.4;
}

/* 快捷键网格布局 */
.hotkey-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
  gap: 12px;
  margin-bottom: 12px;
}
.hotkey-grid-item {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.hotkey-grid-label {
  font-size: 13px;
  color: var(--el-text-color-secondary);
}

/* 热键提示 */
.hotkey-hint {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  font-size: 12px;
  color: rgba(255, 255, 255, 0.4);
}

.hotkey-hint span {
  background-color: rgba(255, 255, 255, 0.04);
  padding: 4px 8px;
  border-radius: 4px;
}

/* 设置区域标题 */
.settings-section-title {
  font-size: 13px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.65);
  margin-bottom: 16px;
}

/* 描述文字 */
.description {
  font-size: 12px;
  color: rgba(255, 255, 255, 0.35);
  margin-top: 8px;
  line-height: 1.5;
}

/* 关于页面 */
.about-section {
  text-align: center;
  padding: 40px 20px;
}

.app-logo {
  margin-bottom: 20px;
}

.logo-img {
  width: 80px;
  height: 80px;
  border-radius: 16px;
}

.app-title {
  font-size: 28px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.9);
  margin: 16px 0 8px 0;
}

.app-version {
  font-size: 14px;
  color: rgba(255, 255, 255, 0.45);
  margin: 0 0 16px 0;
}

.app-description {
  font-size: 14px;
  color: rgba(255, 255, 255, 0.55);
  margin: 0 0 20px 0;
}

.about-links {
  margin: 20px 0;
}

.copyright {
  font-size: 12px;
  color: rgba(255, 255, 255, 0.3);
  margin-top: 24px;
}

/* 栅格间距 */
:deep(.el-row) {
  margin-bottom: 0;
}

:deep(.el-col) {
  padding-bottom: 0;
}

/*
 * 自定义滚动条样式
 * 隐藏默认滚动条，实现与界面视觉风格统一的自定义滚动条
 */

/* 隐藏默认滚动条（WebKit/Blink 浏览器） */
.tab-content-inner::-webkit-scrollbar {
  display: none;
}

/* 隐藏默认滚动条（IE/Edge） */
.tab-content-inner {
  -ms-overflow-style: none;
}

/* 隐藏默认滚动条（Firefox） */
.tab-content-inner {
  scrollbar-width: none;
}

/* 自定义滚动条容器 */
.tab-content-inner {
  position: relative;
  overflow: auto;
}

/* 自定义滚动条轨道 */
.tab-content-inner::after {
  content: '';
  position: absolute;
  right: 0;
  top: 0;
  bottom: 0;
  width: 6px;
  background: rgba(255, 255, 255, 0.03);
  border-radius: 3px;
  pointer-events: none;
}

/* 自定义滚动条滑块 - 使用 CSS scrollbar-gutter 和 scrollbar-color（现代浏览器） */
.tab-content-inner {
  scrollbar-gutter: stable;
}

/* 滚动容器 hover 时显示更明显的滚动条提示 */
.tab-content-inner:hover {
  scrollbar-width: thin;
  scrollbar-color: rgba(255, 255, 255, 0.2) rgba(255, 255, 255, 0.03);
}

/* 文本域自定义滚动条 */
:deep(.el-textarea__inner)::-webkit-scrollbar {
  width: 6px;
}

:deep(.el-textarea__inner)::-webkit-scrollbar-track {
  background: rgba(255, 255, 255, 0.03);
  border-radius: 3px;
}

:deep(.el-textarea__inner)::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.2);
  border-radius: 3px;
}

:deep(.el-textarea__inner)::-webkit-scrollbar-thumb:hover {
  background: rgba(255, 255, 255, 0.35);
}

:deep(.el-textarea__inner) {
  scrollbar-width: thin;
  scrollbar-color: rgba(255, 255, 255, 0.2) rgba(255, 255, 255, 0.03);
}

/* 还原区样式 */
.restore-zone {
  border: 2px dashed rgba(255, 255, 255, 0.2);
  border-radius: 8px;
  padding: 16px;
  transition: all 0.3s ease;
  background-color: rgba(255, 255, 255, 0.02);
}

.restore-zone:hover {
  border-color: rgba(255, 255, 255, 0.3);
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
}

.restore-zone-hint {
  font-size: 13px;
  color: rgba(255, 255, 255, 0.6);
  font-weight: 500;
}

.restore-zone-content {
  min-height: 80px;
}

.restore-zone-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 20px 0;
}

.restore-zone-empty p {
  margin: 4px 0;
  font-size: 13px;
  color: rgba(255, 255, 255, 0.4);
}

.restore-zone-sub {
  font-size: 12px !important;
  color: rgba(255, 255, 255, 0.3) !important;
}

.restore-zone-files {
  max-height: 200px;
  overflow-y: auto;
}

.restore-zone-file-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 12px;
  background-color: rgba(255, 255, 255, 0.04);
  border-radius: 6px;
  margin-bottom: 8px;
}

.restore-zone-file-item:last-child {
  margin-bottom: 0;
}

.restore-zone-file-name {
  flex: 1;
  font-size: 13px;
  color: rgba(255, 255, 255, 0.8);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.restore-zone-file-path {
  flex: 2;
  font-size: 12px;
  color: rgba(255, 255, 255, 0.4);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.restore-zone-actions {
  display: flex;
  gap: 10px;
  margin-top: 12px;
}

.restore-zone-actions :deep(.el-button) {
  flex: 1;
}
</style>
