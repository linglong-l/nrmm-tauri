<script setup lang="ts">
/**
 * App.vue - 应用根组件
 *
 * 作用：
 *  - 作为整个应用的根容器，负责组合 TitleBar、主内容区与 StatusBar 三大区块。
 *  - 负责应用初始化：加载持久化的设置、设置当前语言。
 *  - 监听 Tauri 后端派发的全局热键事件，触发窗口显示/隐藏切换。
 *  - 根据设置中的主题与背景透明度，动态切换 dark 类与 CSS 变量。
 *  - 根据当前语言切换 Element Plus 的内置语言包。
 */
import { onMounted, watch, computed, onUnmounted, provide, reactive, readonly, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import zhCn from 'element-plus/es/locale/lang/zh-cn';
import zhTw from 'element-plus/es/locale/lang/zh-tw';
import en from 'element-plus/es/locale/lang/en';
import ru from 'element-plus/es/locale/lang/ru';
import id from 'element-plus/es/locale/lang/id';
import { TitleBar, StatusBar, HashConflictFab } from './components';
import UpdateModDataOverlay from './components/UpdateModDataOverlay.vue';
import IndexPage from './pages/index/index.vue';
import { useSettingsStore } from './stores/settings';
import { useGameStore } from './stores/game';
import { EventNames, eventManager } from './utils/events';
import { useHashConflict, initPlatformInfo } from './composables';
import { createLogger } from './utils/logger';
import { setupGlobalErrorHandlers } from './utils/errorBoundary';
import type { UpdateModDataResult } from './types';

const { locale } = useI18n();
const settingsStore = useSettingsStore();
const gameStore = useGameStore();
const log = createLogger('App');

/** 更新模组数据遮罩状态 */
export interface UpdateModOverlayState {
  visible: boolean;
  state: 'loading' | 'completed' | 'error';
  result: UpdateModDataResult | null;
  errorMessage: string | null;
}

const overlayState = reactive<UpdateModOverlayState>({
  visible: false,
  state: 'loading',
  result: null,
  errorMessage: null,
});

/** 提供 overlayState 给子组件 */
provide('updateModOverlay', readonly(overlayState));

/** 提供控制函数给子组件 */
provide('updateModOverlayControls', {
  show: showUpdateModOverlay,
  finish: finishUpdateModOverlay,
  error: errorUpdateModOverlay,
  hide: hideUpdateModOverlay,
});

/** 显示遮罩（loading 态） */
function showUpdateModOverlay() {
  overlayState.visible = true;
  overlayState.state = 'loading';
  overlayState.result = null;
  overlayState.errorMessage = null;
}

/** 遮罩切换为完成态 */
function finishUpdateModOverlay(result: UpdateModDataResult) {
  overlayState.state = 'completed';
  overlayState.result = result;
}

/** 遮罩切换为错误态 */
function errorUpdateModOverlay(message: string) {
  overlayState.state = 'error';
  overlayState.errorMessage = message;
}

/** 隐藏遮罩 */
function hideUpdateModOverlay() {
  overlayState.visible = false;
}

// 注册全局 Hash 冲突事件监听（与 HashConflictFab 联动）
useHashConflict();

let unlistenHotkey: (() => void) | null = null;

/**
 * 根据设置中的语言代码返回对应的 Element Plus 语言包。
 * 限制：仅支持 zh-CN / zh-TW / ru / id 四种显式映射，其他语言回退到英文。
 */
const elementPlusLocale = computed(() => {
  const lang = settingsStore.language;
  switch (lang) {
    case 'zh-CN':
      return zhCn;
    case 'zh-TW':
      return zhTw;
    case 'ru':
      return ru;
    case 'id':
      return id;
    default:
      return en;
  }
});

// 当前是否为深色主题，用于给根容器附加 dark-theme 类
const isDark = computed(() => settingsStore.theme === 'dark');

/**
 * 平台透明支持状态。
 * - true（默认）：使用用户设置的背景透明度 + 毛玻璃 + 圆角；
 * - false（Wayland/WSLg 等不兼容环境）：强制不透明背景、禁用毛玻璃、移除圆角，
 *   避免出现黑块/黑角/闪烁/无 blur 等渲染问题。
 */
const supportsTransparency = ref(true);

/** 是否应用不透明降级样式（no-transparency 类） */
const isTransparencyDisabled = computed(() => !supportsTransparency.value);

// 背景透明度数值（0~1），通过 CSS 变量 --bg-opacity 透传给样式。
// 不支持透明的平台上强制为 1（完全不透明）。
const bgOpacity = computed(() =>
  supportsTransparency.value ? settingsStore.bgTransparency : 1
);

// 整体缩放比例，通过 transform: scale 应用到根容器
const appScale = computed(() => settingsStore.overallScale);

// 动态计算的根容器内联样式：注入背景透明度变量与缩放变量
const appStyle = computed(() => ({
  '--bg-opacity': bgOpacity.value,
  '--app-scale': appScale.value
}));

/**
 * 应用初始化：加载平台环境信息、持久化设置，同步 i18n 语言。
 * 异常处理：各步骤失败时静默忽略，避免阻塞渲染。
 * 顺序说明：
 *   1. 先 initPlatformInfo()（极快，本地 IPC），拿到透明支持状态后
 *      后续样式计算才能正确应用降级，避免首帧闪烁。
 *   2. 再加载 settings（包含主题/语言/透明度等用户偏好）。
 */
async function initApp() {
  try {
    const info = await initPlatformInfo();
    supportsTransparency.value = info.transparencySupported;
  } catch {
    // IPC 失败时保守禁用透明（Linux 回退），Windows/macOS 前端检测会在 fallback 中处理
    supportsTransparency.value = !navigator.platform?.toLowerCase().includes('linux');
  }

  try {
    await settingsStore.loadSettings();
    locale.value = settingsStore.language;
    // 设置加载完成后，同步目标游戏到 gameStore
    gameStore.initFromSettings();
  } catch {
    // ignore
  }
}

/**
 * 全局热键按下事件的回调。
 * 窗口切换已由后端 HotkeyManager 统一处理，此处接收事件通知用于 UI 反馈。
 * @param payload 事件载荷，包含热键标识和来源
 */
function handleHotkeyPressed(payload: { key: string; source: 'in-game' | 'outside-game'; timestamp?: number }) {
  const { key, source } = payload;
  logHotkeyPressed(key, source);
}

function logHotkeyPressed(key: string, source: 'in-game' | 'outside-game') {
  log.debug('Hotkey pressed', { key, source });
}

// 监听语言变化，实时同步到 i18n，使界面文案立即切换
watch(
  () => settingsStore.language,
  (newLang) => {
    locale.value = newLang;
  }
);

// 监听主题变化，通过在 <html> 上增删 dark 类来控制全局深色模式
// immediate: true 保证组件挂载时立即按当前主题应用一次
watch(
  () => settingsStore.theme,
  (newTheme) => {
    if (newTheme === 'dark') {
      document.documentElement.classList.add('dark');
    } else {
      document.documentElement.classList.remove('dark');
    }
  },
  { immediate: true }
);

/**
 * 全局右键菜单禁用回调函数
 * @param e - 右键菜单事件
 */
function handleContextMenu(e: MouseEvent) {
  e.preventDefault();
}

// 组件挂载：初始化应用，并注册全局热键事件监听
onMounted(() => {
  // 注册全局运行时错误处理器（unhandledrejection + window.onerror）
  setupGlobalErrorHandlers();

  // 全局禁用默认右键菜单，防止与自定义右键功能冲突
  document.addEventListener('contextmenu', handleContextMenu);

  initApp();

  eventManager.on(EventNames.HOTKEY_PRESSED, handleHotkeyPressed).then((unlisten) => {
    unlistenHotkey = unlisten;
  }).catch(() => {});
});

// 组件卸载：注销事件监听和全局右键菜单禁用，避免内存泄漏与重复触发
onUnmounted(() => {
  // 移除全局右键菜单禁用事件监听
  document.removeEventListener('contextmenu', handleContextMenu);

  if (unlistenHotkey) {
    unlistenHotkey();
    unlistenHotkey = null;
  }
});
</script>

<template>
  <!-- 应用根容器：通过 ElConfigProvider 注入语言，外层 div 控制整体布局/主题/背景透明度 -->
  <ElConfigProvider :locale="elementPlusLocale">
    <div
      class="app-container"
      :class="{ 'dark-theme': isDark, 'no-transparency': isTransparencyDisabled }"
      :style="appStyle"
    >
      <TitleBar />
      <div class="main-content">
        <IndexPage />
      </div>
      <StatusBar />
      <!--
        全局浮动 Hash 冲突入口：
        - position: fixed 定位在右下角
        - v-if="hashConflict.hasConflicts" 控制可见性
        - 不受 Tab 切换影响
      -->
      <HashConflictFab />
      <!-- 全屏遮罩：更新模组数据 -->
      <UpdateModDataOverlay
        :visible="overlayState.visible"
        :state="overlayState.state"
        :result="overlayState.result"
        :error-message="overlayState.errorMessage"
        @update:visible="overlayState.visible = $event"
      />
    </div>
  </ElConfigProvider>
</template>

<style>
* {
  margin: 0;
  padding: 0;
  box-sizing: border-box;
}

html,
body,
#app {
  height: 100%;
  margin: 0;
  padding: 0;
  overflow: hidden;
}

.app-container {
  display: flex;
  flex-direction: column;
  height: calc(100vh / var(--app-scale, 1));
  width: calc(100vw / var(--app-scale, 1));
  overflow: hidden;
  background-color: rgba(20, 20, 24, var(--bg-opacity, 0.88));
  backdrop-filter: blur(24px) saturate(1.2);
  -webkit-backdrop-filter: blur(24px) saturate(1.2);
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
  border-radius: 12px;
  transform: scale(var(--app-scale, 1));
  transform-origin: top left;
}

.app-container.dark-theme {
  background-color: rgba(20, 20, 24, var(--bg-opacity, 0.88));
}

/**
 * 透明窗口降级样式（Wayland/WSLg 等不兼容环境）。
 *
 * 问题：Wayland 合成器（GNOME Mutter、KWin XWayland、WSLg Weston）对
 *      ARGB 透明窗口的支持不稳定：
 *      - 圆角区域出现黑色方块/黑边；
 *      - backdrop-filter: blur() 完全不生效或渲染为杂色；
 *      - 透明区域闪烁/不刷新。
 *
 * 解决方案：
 *   1. 强制 --bg-opacity: 1（完全不透明背景，已在 JS computed 中处理）；
 *   2. 移除 backdrop-filter 毛玻璃效果（避免渲染异常）；
 *   3. 移除 border-radius 圆角（避免黑角）；
 *   4. 使用纯色 rgb 背景（不依赖 alpha 通道）。
 */
.app-container.no-transparency {
  background-color: rgb(28, 28, 32) !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  border-radius: 0 !important;
}

.app-container.no-transparency.dark-theme {
  background-color: rgb(20, 20, 24) !important;
}

.main-content {
  flex: 1;
  overflow: hidden;
  display: flex;
}

html.dark {
  color-scheme: dark;
}
</style>
