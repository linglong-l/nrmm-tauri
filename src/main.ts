/**
 * 应用入口文件
 * 初始化Vue应用、Pinia状态管理、ElementPlus组件库、i18n国际化
 * 配置全局事件监听和默认样式
 *
 * 优化策略：
 * - Tab视图组件使用defineAsyncComponent懒加载，减少首屏JS体积
 * - Element Plus图标由各组件按需import，不在此全局注册
 */

// 使用HTML中更早的时间戳作为全局统一计时起点（HTML解析开始时间）
const HTML_BOOT_START: number = (window as any).__HTML_BOOT_START__ ?? performance.now()
const mainBootTs = new Date().toISOString().substring(11, 23)
// 记录main.ts模块被解析执行的时间点（import语句执行前）
console.log(`[FE-BOOT] T+${(performance.now() - HTML_BOOT_START).toFixed(0).padStart(6)}ms - main.ts 开始执行 at ${mainBootTs}`)

import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import i18n from './utils/i18n'
import { logger } from './utils/logger'

console.log(`[FE-BOOT] T+${(performance.now() - HTML_BOOT_START).toFixed(0).padStart(6)}ms - 所有import完成`)

/** 创建Vue应用实例 */
const app = createApp(App)
console.log(`[FE-BOOT] T+${(performance.now() - HTML_BOOT_START).toFixed(0).padStart(6)}ms - createApp完成`)
/** 创建Pinia状态管理实例 */
const pinia = createPinia()

/** 挂载插件 */
app.use(pinia)
console.log(`[FE-BOOT] T+${(performance.now() - HTML_BOOT_START).toFixed(0).padStart(6)}ms - pinia挂载完成`)
app.use(i18n)
console.log(`[FE-BOOT] T+${(performance.now() - HTML_BOOT_START).toFixed(0).padStart(6)}ms - i18n挂载完成`)

/** 设置文档颜色方案为深色模式 */
document.documentElement.style.setProperty('color-scheme', 'dark')
document.documentElement.classList.add('color-scheme-dark')

/**
 * 全局右键菜单拦截
 * 在非输入框/文本域/标记为.allow-context-menu的元素上禁用右键菜单
 */
document.addEventListener(
  'contextmenu',
  (e) => {
    const target = e.target as HTMLElement
    const tag = target?.tagName?.toLowerCase()
    const allow =
      tag === 'input' ||
      tag === 'textarea' ||
      target?.closest?.('.allow-context-menu')
    if (!allow) {
      e.preventDefault()
    }
  },
  true
)

/**
 * 全局文本选择拦截
 * 在非输入框/文本域/标记为.allow-text-select的元素上禁用文本选择
 */
document.addEventListener('selectstart', (e) => {
  const target = e.target as HTMLElement
  const tag = target?.tagName?.toLowerCase()
  const allow =
    tag === 'input' ||
    tag === 'textarea' ||
    target?.closest?.('.allow-text-select')
  if (!allow) {
    e.preventDefault()
  }
})

/**
 * 三层错误兜底机制
 * 1. Vue errorHandler：捕获组件内未处理的异常
 * 2. unhandledrejection：捕获未处理的 Promise 拒绝
 * 3. window.onerror：捕获全局脚本错误
 */
app.config.errorHandler = (err, _instance, info) => {
  logger.error('VueApp', `Unhandled error in component: ${info}`, err)
}

window.addEventListener('unhandledrejection', (event) => {
  logger.error('Promise', 'Unhandled promise rejection', event.reason)
})

window.onerror = (message, source, lineno, colno, error) => {
  logger.error('Window', `Script error: ${message} at ${source}:${lineno}:${colno}`, error)
  return false
}

/** 挂载应用到DOM */
console.log(`[FE-BOOT] T+${(performance.now() - HTML_BOOT_START).toFixed(0).padStart(6)}ms - 即将调用 app.mount('#app')`)
app.mount('#app')
console.log(`[FE-BOOT] T+${(performance.now() - HTML_BOOT_START).toFixed(0).padStart(6)}ms - app.mount 调用完成（等待Vue组件渲染）`)

// 将HTML启动时间暴露到全局，供App.vue对齐计时起点
;(window as any).__NRMM_FE_BOOT_START__ = HTML_BOOT_START
