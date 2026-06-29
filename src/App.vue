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
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import zhCn from 'element-plus/es/locale/lang/zh-cn';
import zhTw from 'element-plus/es/locale/lang/zh-tw';
import en from 'element-plus/es/locale/lang/en';
import ru from 'element-plus/es/locale/lang/ru';
import id from 'element-plus/es/locale/lang/id';
import { TitleBar, StatusBar } from './components';
import IndexPage from './pages/index/index.vue';
import { useSettingsStore } from './stores/settings';
import { invokeToggleWindow } from './utils/invoke';

const { locale } = useI18n();
const settingsStore = useSettingsStore();

// 用于在组件卸载时取消 Tauri 事件监听的句柄；null 表示尚未注册或已注销
let unlistenHotkey: UnlistenFn | null = null;

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

// 动态计算的根容器内联样式：仅注入背景透明度变量
const appStyle = computed(() => ({
  '--bg-opacity': bgOpacity.value
}));

/**
 * 应用初始化：加载持久化设置并将 i18n 语言同步为设置中的语言。
 * 异常处理：加载失败时静默忽略，避免阻塞渲染。
 */
async function initApp() {
  try {
    await settingsStore.loadSettings();
    locale.value = settingsStore.language;
  } catch {
    // ignore
  }
}

/**
 * 全局热键按下事件的回调：调用后端切换窗口的显示/隐藏状态。
 * @param _event Tauri 事件对象，payload 为热键标识字符串（此处未使用）
 */
async function handleHotkeyPressed(_event: { payload: string }) {
  try {
    await invokeToggleWindow();
  } catch {
    // ignore
  }
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

// 组件挂载：初始化应用，并注册 Tauri 全局热键事件监听
onMounted(() => {
  initApp();

  listen('hotkey-pressed', handleHotkeyPressed).then((unlisten) => {
    unlistenHotkey = unlisten;
  }).catch(() => {});
});

// 组件卸载：注销 Tauri 事件监听，避免内存泄漏与重复触发
onUnmounted(() => {
  if (unlistenHotkey) {
    unlistenHotkey();
    unlistenHotkey = null;
  }
});
</script>

<template>
  <!-- 应用根容器：通过 ElConfigProvider 注入语言与主题，外层 div 控制整体布局/主题/背景透明度 -->
  <ElConfigProvider :locale="elementPlusLocale" theme="dark">
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
  height: 100vh;
  width: 100vw;
  overflow: hidden;
  background-color: rgba(20, 20, 24, var(--bg-opacity, 0.88));
  backdrop-filter: blur(24px) saturate(1.2);
  -webkit-backdrop-filter: blur(24px) saturate(1.2);
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
  border-radius: 12px;
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
