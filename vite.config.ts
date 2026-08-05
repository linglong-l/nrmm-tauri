import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { resolve } from "path";
const __dirname = resolve(import.meta.dirname);

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(async () => ({
  plugins: [vue()],

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
          port: 5174,
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
          if (id.includes('node_modules/element-plus')) {
            return 'element-plus';
          }
          if (id.includes('node_modules/vue') || id.includes('node_modules/pinia')) {
            return 'vue-core';
          }
        },
      },
    },
  },
}));
