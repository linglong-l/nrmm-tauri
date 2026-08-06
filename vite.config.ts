import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import AutoImport from "unplugin-auto-import/vite";
import Components from "unplugin-vue-components/vite";
import { ElementPlusResolver } from "unplugin-vue-components/resolvers";
import { resolve } from "path";
const __dirname = resolve(import.meta.dirname);

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(async () => ({
  plugins: [
    vue(),
    // ElementPlus 按需引入：自动导入 compose API（vue/vue-i18n/pinia）及 ElMessage 等 API
    AutoImport({
      imports: ["vue", "vue-i18n", "pinia"],
      resolvers: [ElementPlusResolver()],
      dts: "src/auto-imports.d.ts",
    }),
    // ElementPlus 组件按需引入：模板中的 el-* 组件自动注册
    Components({
      resolvers: [ElementPlusResolver()],
      dts: "src/components.d.ts",
    }),
  ],

  resolve: {
    alias: {
      "@": resolve(__dirname, "src"),
    },
  },

  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host
      ? {
        protocol: "ws",
        host,
        port: 5175,
      }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
    // dev 服务器启动时自动打开浏览器（Tauri 环境下不生效，但可加快预构建）
    open: false,
  },

  // 依赖预构建优化：提前构建大型依赖，减少首次页面加载时的即时编译开销
  optimizeDeps: {
    include: [
      "vue",
      "vue-i18n",
      "pinia",
      "element-plus",
      "@element-plus/icons-vue",
      "@tauri-apps/api/core",
      "@tauri-apps/api/event",
      "@tauri-apps/plugin-dialog",
      "@tauri-apps/plugin-notification",
      "@tauri-apps/plugin-opener",
      "@tauri-apps/plugin-shell",
      "@tauri-apps/plugin-os",
    ],
  },

  build: {
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      output: {
        manualChunks(id) {
          // ElementPlus 已改为按需引入，组件随各视图 chunk 打包，不再强制聚块
          if (id.includes('node_modules/vue') || id.includes('node_modules/pinia')) {
            return 'vue-core';
          }
        },
      },
    },
  },
}));
