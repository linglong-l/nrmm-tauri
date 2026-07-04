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
import { onMounted, watch, computed, onUnmounted } from 'vue';
import { useI18n } from 'vue-i18n';
import { ElConfigProvider } from 'element-plus';
import zhCn from 'element-plus/es/locale/lang/zh-cn';
import zhTw from 'element-plus/es/locale/lang/zh-tw';
import en from 'element-plus/es/locale/lang/en';
import ru from 'element-plus/es/locale/lang/ru';
import id from 'element-plus/es/locale/lang/id';
import { TitleBar, StatusBar } from './components';
import IndexPage from './pages/index/index.vue';
import { useSettingsStore } from './stores/settings';
import { useGameStore } from './stores/game';
import { EventNames, eventManager } from './utils/events';

const { locale } = useI18n();
const settingsStore = useSettingsStore();
const gameStore = useGameStore();

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

// 背景透明度数值（0~1），通过 CSS 变量 --bg-opacity 透传给样式
const bgOpacity = computed(() => settingsStore.bgTransparency);

// 整体缩放比例，通过 transform: scale 应用到根容器
const appScale = computed(() => settingsStore.overallScale);

// 动态计算的根容器内联样式：注入背景透明度变量与缩放变量
const appStyle = computed(() => ({
  '--bg-opacity': bgOpacity.value,
  '--app-scale': appScale.value
}));

/**
 * 应用初始化：加载持久化设置并将 i18n 语言同步为设置中的语言。
 * 异常处理：加载失败时静默忽略，避免阻塞渲染。
 */
async function initApp() {
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
  console.debug('[Hotkey] Pressed:', key, 'from:', source);
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
      :class="{ 'dark-theme': isDark }"
      :style="appStyle"
    >
      <TitleBar />
      <div class="main-content">
        <IndexPage />
      </div>
      <StatusBar />
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

.main-content {
  flex: 1;
  overflow: hidden;
  display: flex;
}

html.dark {
  color-scheme: dark;
}
</style>
