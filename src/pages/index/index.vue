<script setup lang="ts">
/**
 * index.vue - 主页面组件
 *
 * 作用：
 *  - 应用的主内容区骨架，左侧为 SideNav 顶部导航，右侧根据 activeTab 渲染对应内容。
 *  - 管理三个标签页：mods（模组管理）、keybinds（热键提示）、settings（设置页）。
 *  - 监听"模组分组更新"事件，同步到 gameStore。
 *  - 监听全局热键与窗口事件并输出调试日志。
 *
 * 注意：模组加载逻辑已统一交由 ModsTab.vue 负责，此处不再处理。
 * 注意：窗口切换全局热键的注册/注销由后端在设置保存时自动管理，此处不再手动注册。
 */
import { ref, computed, onMounted, onUnmounted, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { ElMessage } from 'element-plus';
import { SideNav } from '../../components';
import ModsTab from './tabs/ModsTab.vue';
import KeybindsTab from './tabs/KeybindsTab.vue';
import SettingsView from '../../views/SettingsView.vue';
import { useUiStore } from '../../stores/ui';
import { useGameStore } from '../../stores/game';
import { useSettingsStore } from '../../stores/settings';
import { useHotkeyStore } from '../../stores/hotkey';
import { EventNames, eventManager } from '../../utils/events';
import { createLogger } from '../../utils/logger';
import type { TabType } from '../../types';
import { TargetGame } from '../../types';

/**
 * 后端 TargetGame::as_str() 返回 PascalCase 格式（如 "WutheringWaves"），
 * 前端 TargetGame 枚举值为 snake_case 格式（如 "Wuthering_Waves"）。
 * 此映射表用于 HOTKEY_PRESSED 事件中将后端游戏名转换为前端枚举值。
 */
const BACKEND_GAME_TO_TARGET_GAME: Record<string, TargetGame> = {
  'WutheringWaves': TargetGame.Wuthering_Waves,
  'GenshinImpact': TargetGame.Genshin_Impact,
  'HonkaiStarRail': TargetGame.Honkai_Star_Rail,
  'ZenlessZoneZero': TargetGame.Zenless_Zone_Zero,
  'ArknightsEndfield': TargetGame.Arknights_Endfield,
};

const { t } = useI18n();
const uiStore = useUiStore();
const gameStore = useGameStore();
const settingsStore = useSettingsStore();
const hotkeyStore = useHotkeyStore();
const log = createLogger('Index');

// 当前激活的标签页（读写均委托给 uiStore，便于全局共享状态）
const activeTab = computed({
  get: () => uiStore.activeTab,
  set: (val: TabType) => uiStore.setActiveTab(val)
});

// ModsTab 组件实例引用（用于调用搜索框聚焦方法）
const modsTabRef = ref<InstanceType<typeof ModsTab> | null>(null);

// 事件监听取消函数句柄；组件卸载时需调用以避免内存泄漏
let unlistenModsUpdated: (() => void) | null = null;
let unlistenHotkeyRegistered: (() => void) | null = null;
let unlistenHotkeyUnregistered: (() => void) | null = null;
let unlistenHotkeyPressed: (() => void) | null = null;
let unlistenWindowFocusChanged: (() => void) | null = null;
let unlistenWindowShown: (() => void) | null = null;
let unlistenWindowHidden: (() => void) | null = null;

/**
 * 注册窗口内搜索快捷键监听器。
 * 使用捕获阶段（capture phase），确保在焦点元素阻止事件冒泡前也能收到事件，
 * 解决分组展开后树形组件拦截 Alt+G/Alt+F 的问题。
 */
function registerSearchHotkeys(): void {
  log.debug('Registering window-scope search hotkeys (capture phase)');
  window.addEventListener('keydown', handleSearchHotkey, true);
}

/**
 * 注销窗口内搜索快捷键监听器。
 */
function unregisterSearchHotkeys(): void {
  log.debug('Unregistering window-scope search hotkeys');
  window.removeEventListener('keydown', handleSearchHotkey, true);
}

/**
 * 注册前端事件监听：
 *  - MOD_GROUPS_UPDATED：后端通知模组分组更新时，同步到 gameStore。
 *  - HOTKEY_REGISTERED / HOTKEY_UNREGISTERED / HOTKEY_PRESSED：调试用，打印热键事件流。
 *  - WINDOW_SHOWN / WINDOW_HIDDEN：调试用，打印窗口显隐事件。
 * 业务逻辑：监听返回 Promise，注册成功后保存取消函数以便后续清理。
 * 注意：GAME_SWITCHED 的模组加载由 ModsTab.vue 负责，此处不再注册 no-op 监听器。
 * 注意：窗口切换全局热键的注册/注销由后端在设置保存时自动管理，此处不再监听 SETTINGS_UPDATED 进行重注册。
 */

function setupEventListeners() {
  eventManager.on(EventNames.MOD_GROUPS_UPDATED, (groups) => {
    gameStore.setModGroups(groups);
  }).then((unlisten) => {
    unlistenModsUpdated = unlisten;
  }).catch(() => {});

  eventManager.on(EventNames.HOTKEY_REGISTERED, (payload) => {
    log.debug('Registered event', { payload });
  }).then((unlisten) => {
    unlistenHotkeyRegistered = unlisten;
  }).catch(() => {});

  eventManager.on(EventNames.HOTKEY_UNREGISTERED, (payload) => {
    log.debug('Unregistered event', { payload });
  }).then((unlisten) => {
    unlistenHotkeyUnregistered = unlisten;
  }).catch(() => {});

  eventManager.on(EventNames.HOTKEY_PRESSED, (payload) => {
    log.debug('Pressed event', { payload });
    // 全局热键唤起窗口后，WebView 可能未立即触发 focus 事件，手动启用搜索快捷键
    setSearchHotkeysEnabled(true, 'hotkey-pressed');
    // 将后端游戏名（PascalCase，如 "WutheringWaves"）映射为前端 TargetGame 枚举值（snake_case，如 "Wuthering_Waves"）
    const matchedGame = payload.matchedGame
      ? (BACKEND_GAME_TO_TARGET_GAME[payload.matchedGame] ?? null)
      : null;
    if (matchedGame && matchedGame !== gameStore.targetGame) {
      log.debug('Auto-switching game', { matchedGame });
      gameStore.setTargetGame(matchedGame);
    }
  }).then((unlisten) => {
    unlistenHotkeyPressed = unlisten;
  }).catch(() => {});

  eventManager.on(EventNames.WINDOW_SHOWN, (payload) => {
    log.debug('Window shown event', { payload });
    // 校验已选择游戏是否合法，若合法且模组未加载则立即触发加载
    const currentGame = gameStore.targetGame;
    if (!currentGame || currentGame === 'none') {
      log.error('Window shown but no valid game selected', undefined, { trigger: 'WINDOW_SHOWN', env: { currentGame } });
      return;
    }
    if (!gameStore.isModsLoaded) {
      log.debug('Window shown, triggering mod load', { game: currentGame });
      gameStore.loadModsForGame(currentGame as TargetGame).catch((e) => {
        log.error('Failed to load mods on window shown', e, { trigger: 'WINDOW_SHOWN' });
      });
    }
  }).then((unlisten) => {
    unlistenWindowShown = unlisten;
  }).catch(() => {});

  eventManager.on(EventNames.WINDOW_HIDDEN, (payload) => {
    log.debug('Window hidden event', { payload });
  }).then((unlisten) => {
    unlistenWindowHidden = unlisten;
  }).catch(() => {});
}

/**
 * 清理事件监听：调用保存的取消函数并重置句柄。
 * 必须在 onUnmounted 中调用，防止组件销毁后仍触发回调导致报错。
 */
function cleanupEventListeners() {
  if (unlistenModsUpdated) {
    unlistenModsUpdated();
    unlistenModsUpdated = null;
  }
  if (unlistenHotkeyRegistered) {
    unlistenHotkeyRegistered();
    unlistenHotkeyRegistered = null;
  }
  if (unlistenHotkeyUnregistered) {
    unlistenHotkeyUnregistered();
    unlistenHotkeyUnregistered = null;
  }
  if (unlistenHotkeyPressed) {
    unlistenHotkeyPressed();
    unlistenHotkeyPressed = null;
  }
  if (unlistenWindowFocusChanged) {
    unlistenWindowFocusChanged();
    unlistenWindowFocusChanged = null;
  }
  if (unlistenWindowShown) {
    unlistenWindowShown();
    unlistenWindowShown = null;
  }
  if (unlistenWindowHidden) {
    unlistenWindowHidden();
    unlistenWindowHidden = null;
  }
}

/**
 * 窗口内搜索快捷键处理函数。
 *
 * 作用：
 *  - 监听窗口内的 Alt+字母 组合键，匹配设置中的搜索快捷键配置
 *  - 匹配成功时聚焦对应搜索框（分组搜索框或模组搜索框）
 *  - 当焦点在 input/textarea/contenteditable 元素时不触发，避免干扰文字输入
 *  - 仅在 mods 标签页激活且窗口处于聚焦状态时生效
 *
 * 设计目的：为用户提供窗口内快捷键快速聚焦搜索框，与后端注册的全局热键
 * （呼出菜单）解耦，仅作用于本窗口且不干扰正常文字输入。
 *
 * 监听方式：在 index.vue 中以捕获阶段（capture phase）注册，确保分组树展开后
 * 内部组件阻止事件冒泡时仍能稳定触发。
 *
 * @param event 键盘事件
 */
function handleSearchHotkey(event: KeyboardEvent): void {
  // 诊断日志：确认事件已到达顶层捕获阶段处理器
  log.debug('Event reached window handler');

  // 窗口失焦时不响应窗口内热键
  if (!hotkeyStore.isSearchHotkeysEnabled) {
    log.debug('Ignored: search hotkeys disabled');
    return;
  }

  // 仅在 mods 标签页激活时响应
  if (uiStore.activeTab !== 'mods') {
    log.debug('Ignored: active tab is not mods');
    return;
  }

  // 必须按住 Alt 键
  if (!event.altKey) {
    log.debug('Ignored: Alt key not pressed');
    return;
  }

  // 焦点在 input/textarea/contenteditable 元素时不触发，避免干扰文字输入
  // 例外：若搜索框已显示且焦点正好在对应的搜索输入框内，允许快捷键关闭搜索框
  const target = event.target as HTMLElement;
  if (target) {
    const tagName = target.tagName.toLowerCase();
    if (tagName === 'input' || tagName === 'textarea') {
      // 仅在焦点位于统一搜索输入框且搜索框可见时放行，以便快捷键关闭搜索框
      const searchInputEl = modsTabRef.value?.getSearchInputEl() ?? null;
      const isSearchInput = searchInputEl !== null && target === searchInputEl;
      const searchVisible = modsTabRef.value?.isSearchVisible() ?? false;
      if (!(isSearchInput && searchVisible)) {
        log.debug('Ignored: focus is in input/textarea');
        return;
      }
    }
    if (target.isContentEditable) {
      log.debug('Ignored: focus is contenteditable');
      return;
    }
  }

  // 转换按键为 "alt+字母" 格式（小写）
  const key = event.key.toLowerCase();
  // 仅匹配单个字母键（a-z），忽略其他功能键
  if (key.length !== 1 || !/[a-z]/.test(key)) {
    log.debug('Ignored: key is not a single letter', { key: event.key });
    return;
  }

  const pressedHotkey = `alt${key}`;

  // 诊断日志：记录事件路径，便于排查分组展开后未触发的问题
  const pathTarget = event.composedPath()[0] as HTMLElement | undefined;
  log.debug('Keydown target path', {
    targetTag: (event.target as HTMLElement | null)?.tagName,
    targetClass: (event.target as HTMLElement | null)?.className,
    composedPathTag: pathTarget?.tagName,
    composedPathClass: pathTarget?.className,
    key: event.key,
    altKey: event.altKey
  });

  // 诊断日志：记录按键输入和当前配置
  log.debug('Pressed hotkey', {
    pressedHotkey,
    groupSearch: settingsStore.groupSearchHotkey,
    modSearch: settingsStore.modSearchHotkey,
    windowHotkey: settingsStore.hotkeyKeyboard
  });

  // 检查是否与全局热键冲突 — 告知用户
  if (pressedHotkey === settingsStore.hotkeyKeyboard.toLowerCase()) {
    log.warn('Conflict with window hotkey', { reason: `Search hotkey ${pressedHotkey} conflicts with window toggle hotkey`, impact: 'Search hotkey blocked to prevent accidental window toggle' });
    ElMessage.warning(t('Hotkey conflict: {key} is used by window toggle', { key: pressedHotkey }));
    return;
  }

  // 匹配任意搜索快捷键（统一小写比较）— 触发统一搜索框 toggle
  const searchHotkeys = [settingsStore.groupSearchHotkey, settingsStore.modSearchHotkey].map(k => k.toLowerCase());
  if (searchHotkeys.includes(pressedHotkey)) {
    event.preventDefault();
    log.debug('Triggering unified search');
    modsTabRef.value?.toggleSearch();
    return;
  }

  log.debug('Ignored: no matching search hotkey', { pressedHotkey });
}



// 监听标签页切换：切回 mods 标签页时触发缓存校验
// 业务逻辑：由于使用 v-show 切换标签页，ModsTab 只 mount 一次，
// 切回时不会重新触发 onMounted，因此需要在此显式校验缓存数据一致性。
watch(activeTab, (newTab) => {
  if (newTab === 'mods') {
    const result = gameStore.validateCache(gameStore.targetGame);
    if (result.action === 'load') {
      gameStore.loadModsForGame(gameStore.targetGame);
    } else if (result.action === 'clear_and_load') {
      gameStore.clearModsCache();
      gameStore.loadModsForGame(gameStore.targetGame);
    }
    // action 为 'skip' 或 'use_cache' 时无需操作
  }
});

/**
 * 统一设置窗口内搜索快捷键可用状态，并记录变更来源。
 * 用于聚合 WebView focus/blur、Tauri onFocusChanged、全局热键唤起等来源，
 * 避免多处直接修改 `isSearchHotkeysEnabled` 导致日志分散或状态竞态。
 *
 * @param enabled 是否启用搜索快捷键
 * @param source 触发来源，用于调试（webview-focus / webview-blur / tauri-focus-changed / hotkey-pressed）
 */
function setSearchHotkeysEnabled(enabled: boolean, source: string) {
  const previous = hotkeyStore.isSearchHotkeysEnabled;
  hotkeyStore.setSearchHotkeysEnabled(enabled);
  if (previous !== enabled) {
    log.debug(`Search hotkeys ${enabled ? 'enabled' : 'disabled'}`, { source });
  }
}

// 窗口聚焦/失焦处理：控制窗口内搜索快捷键是否响应
function handleWindowFocus() {
  setSearchHotkeysEnabled(true, 'webview-focus');
}

function handleWindowBlur() {
  setSearchHotkeysEnabled(false, 'webview-blur');
}

// 组件挂载：注册事件监听；注册窗口内搜索快捷键监听器；监听窗口聚焦状态
onMounted(async () => {
  setupEventListeners();
  registerSearchHotkeys();

  // 通过 Tauri 窗口 API 监听真实窗口焦点变化，比 WebView 的 window focus/blur 更可靠
  let tauriFocusAttached = false;
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    const win = getCurrentWindow();
    const focused = await win.isFocused();
    setSearchHotkeysEnabled(focused, 'tauri-focus-initial');
    unlistenWindowFocusChanged = await win.onFocusChanged(({ payload: focused }) => {
      setSearchHotkeysEnabled(focused, 'tauri-focus-changed');
    });
    tauriFocusAttached = true;
    log.debug('Tauri window focus listener attached');
  } catch (error) {
    log.debug('Failed to attach Tauri window focus listener', { error });
  }

  // Tauri API 不可用时降级使用 WebView 的 focus/blur 事件
  if (!tauriFocusAttached) {
    window.addEventListener('focus', handleWindowFocus);
    window.addEventListener('blur', handleWindowBlur);
    log.debug('Fallback to WebView focus/blur listeners');
  }
});

// 组件卸载：清理事件监听；移除窗口内搜索快捷键监听器；移除聚焦监听
onUnmounted(() => {
  cleanupEventListeners();
  unregisterSearchHotkeys();
  // 若 Tauri 焦点监听未成功 attach，则 WebView 降级监听器仍存在，需要清理
  if (!unlistenWindowFocusChanged) {
    window.removeEventListener('focus', handleWindowFocus);
    window.removeEventListener('blur', handleWindowBlur);
  }
});
</script>

<template>
  <div class="index-page">
    <SideNav />
    <div class="content-area">
      <div class="content-body">
        <!-- 模组管理标签页 -->
        <div v-show="activeTab === 'mods'" class="tab-content">
          <ModsTab ref="modsTabRef" />
        </div>
        <!-- 热键标签页：展示当前选中模组的 [Key*] 快捷键列表 -->
        <div v-show="activeTab === 'keybinds'" class="tab-content">
          <KeybindsTab />
        </div>
        <!-- 设置标签页 -->
        <div v-show="activeTab === 'settings'" class="tab-content">
          <SettingsView />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.index-page {
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  overflow: hidden;
}

.content-area {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.content-body {
  flex: 1;
  overflow: hidden;
}

.tab-content {
  height: 100%;
  display: flex;
  flex-direction: column;
}
</style>
