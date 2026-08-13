export {}

/**
 * 全局 Window 接口扩展
 * 声明由 HTML/main 注入的计时起点全局，消除各处的 `window as any` 强转。
 */
declare global {
  interface Window {
    /** HTML 解析开始时间（由 index.html 注入），供 main.ts 计时起点对齐 */
    __HTML_BOOT_START__?: number
    /** main.ts 记录并向外暴露的启动时间，供 App.vue 对齐计时起点 */
    __NRMM_FE_BOOT_START__?: number
  }
}
