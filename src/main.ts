/**
 * main.ts - 前端应用入口文件
 *
 * 作用：
 *  - 创建 Vue 应用实例，挂载到 DOM 中的 #app 节点。
 *  - 注册两个核心插件：
 *      1) Pinia：状态管理（stores/*）。
 *      2) i18n：多语言国际化（locales/*）。
 *  - Element Plus 组件和样式通过 unplugin-auto-import + unplugin-vue-components
 *    按需自动导入，无需在此处全量注册。
 *  - 动态注入 favicon：构建时 appIcon 会被 Vite 内联为 base64 data URL，
 *    确保构建后不依赖外部图标文件。
 *  - 应用挂载后预加载版本信息。
 */
import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from './App.vue';
import { i18n } from './locales';
import appIcon from '@/assets/images/app-icon-32.png';
import { setupVueErrorHandler } from './utils/errorBoundary';
import { setupGlobalErrorHandlers } from './utils/errorBoundary';
import { useVersionStore } from './stores/version';

// 动态注入 favicon（构建时 appIcon 会被 Vite 内联为 base64 data URL）
const faviconLink = document.createElement('link');
faviconLink.rel = 'icon';
faviconLink.type = 'image/png';
faviconLink.href = appIcon;
document.head.appendChild(faviconLink);

// 创建 Vue 应用实例与 Pinia 实例
const app = createApp(App);
const pinia = createPinia();

// 依次注册 Pinia（状态管理）、i18n（国际化）
// Element Plus 由 vite 插件按需自动导入，无需 app.use(ElementPlus)
app.use(pinia);
app.use(i18n);

// 预加载版本信息
const versionStore = useVersionStore();
versionStore.load();

// 注册 Vue 应用级错误处理器（捕获组件渲染/侦听器/生命周期中的未捕获异常）
setupVueErrorHandler(app);
// 注册全局 unhandledrejection/onerror 处理器
setupGlobalErrorHandlers();

// 将应用挂载到 index.html 中的 #app 元素
app.mount('#app');
