<script setup lang="ts">
/**
 * TitleBar.vue - 自定义窗口标题栏组件
 *
 * 作用：
 *  - 替换操作系统默认标题栏，提供应用图标、应用名与三个窗口控制按钮：
 *    最小化、最大化/还原、关闭（隐藏）。
 *  - 通过 data-tauri-drag-region 属性使整个标题栏可拖拽移动窗口。
 *  - 关闭按钮实际执行 hide() 而非 destroy()，使应用驻留托盘以便热键唤起。
 *
 * 业务逻辑：
 *  - 最大化状态由 isMaximized 本地维护，仅用于图标切换的视觉反馈（实际窗口状态以 Tauri 为准）。
 */
import { ref } from 'vue';
import { ElButton } from 'element-plus';
import { Minus, Close, Crop } from '@element-plus/icons-vue';

// 当前窗口是否处于最大化状态（用于切换按钮图标/tooltip）
const isMaximized = ref(false);

/** 最小化窗口；失败时静默忽略 */
async function minimize() {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    await getCurrentWindow().minimize();
  } catch {
    // ignore
  }
}

/**
 * 切换窗口最大化/还原状态。
 * 业务逻辑：先查询当前是否最大化，再决定调用 maximize 还是 unmaximize。
 */
async function toggleMaximize() {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    const window = getCurrentWindow();
    const maximized = await window.isMaximized();
    if (maximized) {
      await window.unmaximize();
    } else {
      await window.maximize();
    }
    isMaximized.value = !maximized;
  } catch {
    // ignore
  }
}

/**
 * 关闭按钮：实际为隐藏窗口，使应用驻留系统托盘，便于通过热键重新唤起。
 * 失败时静默忽略。
 */
async function close() {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    await getCurrentWindow().hide();
  } catch {
    // ignore
  }
}
</script>

<template>
  <!-- 标题栏：data-tauri-drag-region 使该区域可拖拽移动窗口 -->
  <div class="title-bar" data-tauri-drag-region>
    <!-- 左侧：应用图标与名称 -->
    <div class="title-bar-left" data-tauri-drag-region>
      <img src="/tauri.svg" class="app-icon" alt="app icon" />
      <span class="app-title">XXMI-NRMM</span>
    </div>
    <!-- 右侧：窗口控制按钮（最小化 / 最大化 / 关闭） -->
    <div class="title-bar-right">
      <ElButton
        class="title-bar-btn"
        :icon="Minus"
        circle
        size="small"
        text
        @click.stop="minimize"
      />
      <ElButton
        class="title-bar-btn"
        :icon="Crop"
        circle
        size="small"
        text
        @click.stop="toggleMaximize"
      />
      <ElButton
        class="title-bar-btn close-btn"
        :icon="Close"
        circle
        size="small"
        text
        @click.stop="close"
      />
    </div>
  </div>
</template>

<style scoped>
.title-bar {
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 12px;
  user-select: none;
  flex-shrink: 0;
  background-color: transparent;
  border-bottom: 1px solid rgba(255, 255, 255, 0.04);
}

.title-bar-left {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  height: 100%;
}

.app-icon {
  width: 16px;
  height: 16px;
}

.app-title {
  font-size: 12px;
  font-weight: 500;
  color: rgba(255, 255, 255, 0.5);
}

.title-bar-right {
  display: flex;
  align-items: center;
  gap: 4px;
}

.title-bar-btn {
  width: 26px;
  height: 26px;
  border-radius: 6px;
  color: rgba(255, 255, 255, 0.5);
}

.title-bar-btn:hover {
  background-color: rgba(255, 255, 255, 0.1);
  color: rgba(255, 255, 255, 0.85);
}

.title-bar-btn :deep(.el-icon) {
  font-size: 14px;
}

.close-btn:hover {
  background-color: rgba(239, 68, 68, 0.9) !important;
  color: white !important;
}
</style>
