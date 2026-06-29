import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { resolve } from "path";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [vue()],

  resolve: {
    alias: {
      "@": resolve(__dirname, "src"),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },

  // Build optimizations
  build: {
    // Increase chunk size warning limit to 600KB
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      output: {
        // Code splitting for large dependencies
        manualChunks: {
          // Split Element Plus into its own chunk
          "element-plus": ["element-plus"],
          // Split Vue core libraries
          "vue-core": ["vue", "vue-router", "pinia"],
          // Split i18n
          "vue-i18n": ["vue-i18n"],
          // Split VueUse utilities
          "vueuse": ["@vueuse/core"],
        },
      },
    },
  },
}));
