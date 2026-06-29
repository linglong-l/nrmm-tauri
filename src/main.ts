/**
 * main.ts - 前端应用入口文件
 *
 * 作用：
 *  - 创建 Vue 应用实例，挂载到 DOM 中的 #app 节点。
 *  - 注册三个核心插件：
 *      1) Pinia：状态管理（stores/*）。
 *      2) Element Plus：UI 组件库及其样式。
 *      3) i18n：多语言国际化（locales/*）。
 */
import { createApp } from 'vue';
import { createPinia } from 'pinia';
import ElementPlus from 'element-plus';
import 'element-plus/dist/index.css';
import App from './App.vue';
import { i18n } from './locales';

// 创建 Vue 应用实例与 Pinia 实例
const app = createApp(App);
const pinia = createPinia();

// 依次注册 Pinia（状态管理）、Element Plus（UI 组件）、i18n（国际化）
app.use(pinia);
app.use(ElementPlus);
app.use(i18n);

// 将应用挂载到 index.html 中的 #app 元素
app.mount('#app');
