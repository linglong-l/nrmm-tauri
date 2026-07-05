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
 */
import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from './App.vue';
import { i18n } from './locales';

// 创建 Vue 应用实例与 Pinia 实例
const app = createApp(App);
const pinia = createPinia();

// 依次注册 Pinia（状态管理）、i18n（国际化）
// Element Plus 由 vite 插件按需自动导入，无需 app.use(ElementPlus)
app.use(pinia);
app.use(i18n);

// 将应用挂载到 index.html 中的 #app 元素
app.mount('#app');
