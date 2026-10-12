/**
 * 编译期环境常量（基于 Vite import.meta.env 静态替换）
 *
 * 设计目的：
 * - 提供统一的 dev/prod 判断入口，避免在业务代码中散落 import.meta.env 调用
 * - 在 npm run dev 时 Vite 注入 DEV=true，本模块的 DEV_MODE 即为 true
 * - 在 npm run build 时 Vite 注入 DEV=false（rollup tree-shaking 自动移除 `if (DEV_MODE)` 分支）
 * - 等同于 C/C++ 的 #ifdef 条件编译，零运行时开销，prod 包不含 dev 代码
 *
 * 使用方式：
 *   import { DEV_MODE } from '@/utils/env'
 *   if (DEV_MODE) { loadDevTools() }
 *
 *   <template>
 *     <DevTool v-if="DEV_MODE" />
 *   </template>
 *
 * 注意：
 * - 不要将 DEV_MODE 改为运行时计算的变量（如基于 URL query），否则会破坏 tree-shaking
 * - 动态导入 dev 组件时使用：() => import('@/components/dev/XXX.vue')，让 rollup 自动 code-splitting
 */

/**
 * 是否为开发模式（npm run dev）
 * - true：开发环境，可加载 dev 工具、输出 DEBUG 日志
 * - false：生产构建（npm run build），dev 代码被 tree-shaking 移除
 */
export const DEV_MODE: boolean = import.meta.env.DEV
