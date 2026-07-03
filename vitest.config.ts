/**
 * Vitest 配置文件
 *
 * 集成 Vite 配置，复用 vite.config.ts 中的别名解析与 Vue 插件。
 * 使用 jsdom 环境模拟浏览器 DOM API，支持组件挂载测试。
 */
import { defineConfig } from 'vitest/config';
import vue from '@vitejs/plugin-vue';
import { resolve } from 'path';

export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src'),
    },
  },
  test: {
    // 使用 jsdom 环境模拟浏览器 API（localStorage、window 等）
    environment: 'jsdom',
    // 测试全局设置文件路径（在每个测试文件执行前运行）
    setupFiles: ['src/test/setup.ts'],
    // 包含的测试文件匹配模式
    include: ['src/**/__tests__/**/*.test.ts'],
    // 启用全局 API（describe、it、expect 等），无需显式导入
    globals: true,
  },
});
