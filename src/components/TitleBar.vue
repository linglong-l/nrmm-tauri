<script setup lang="ts">
/**
 * TitleBar.vue - 自定义窗口标题栏组件（跨平台版本）
 *
 * 作用：
 *  - 替换操作系统默认标题栏，提供应用图标、应用名与三个窗口控制按钮：
 *    最小化、最大化/还原、关闭（隐藏）。
 *  - 通过 data-tauri-drag-region 属性使整个标题栏可拖拽移动窗口。
 *  - 关闭按钮实际执行 hide() 而非 destroy()，使应用驻留托盘以便热键唤起。
 *
 * 跨平台支持：
 *  - Windows/Linux：控制按钮在右侧
 *  - macOS：控制按钮在左侧（交通灯风格布局）
 *  - 自动检测当前平台并调整布局
 *
 * 业务逻辑：
 *  - 最大化状态由 isMaximized 本地维护，仅用于图标切换的视觉反馈（实际窗口状态以 Tauri 为准）。
 */
import { ref, onMounted, computed } from 'vue';
import { Minus, Close, Crop } from '@element-plus/icons-vue';
import appIcon from '@/assets/images/app-icon-32.png';

type Platform = 'windows' | 'macos' | 'linux' | 'unknown';

const platform = ref<Platform>('unknown');

const isMacOS = computed(() => platform.value === 'macos');

const isMaximized = ref(false);

onMounted(async () => {
  try {
    const os = detectPlatform();
    platform.value = os;
  } catch {
    platform.value = 'unknown';
  }
});

function detectPlatform(): Platform {
  const userAgent = navigator.userAgent.toLowerCase();
  const platform = navigator.platform?.toLowerCase() || '';

  if (platform.includes('mac') || userAgent.includes('mac')) {
    return 'macos';
  }
  if (platform.includes('win') || userAgent.includes('win')) {
    return 'windows';
  }
  if (platform.includes('linux') || userAgent.includes('linux')) {
    return 'linux';
  }
  return 'unknown';
}

async function minimize() {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    await getCurrentWindow().minimize();
  } catch {
    // ignore
  }
}

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
  <div class="title-bar" :class="{ 'title-bar-macos': isMacOS }" data-tauri-drag-region>
    <!-- macOS：控制按钮在左侧（交通灯风格） -->
    <div v-if="isMacOS" class="title-bar-left title-bar-controls" data-tauri-drag-region>
      <ElButton
        class="title-bar-btn close-btn"
        :icon="Close"
        circle
        size="small"
        text
        @click.stop="close"
      />
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
    </div>
    <!-- 左侧：应用图标与名称（macOS 下在控制按钮右侧） -->
    <div class="title-bar-left" data-tauri-drag-region>
      <img :src="appIcon" class="app-icon" alt="app icon" />
      <span class="app-title">XXMI-NRMM</span>
    </div>
    <!-- 右侧：窗口控制按钮（Windows/Linux 风格） -->
    <div v-if="!isMacOS" class="title-bar-right">
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

.title-bar-macos {
  padding: 0 16px;
}

.title-bar-left {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  height: 100%;
}

.title-bar-macos .title-bar-left.title-bar-controls {
  flex: 0;
  min-width: auto;
  margin-right: 12px;
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

.title-bar-macos .app-title {
  margin-left: 0;
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

.title-bar-macos .title-bar-controls .title-bar-btn {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  padding: 0;
}

.title-bar-macos .title-bar-controls .title-bar-btn :deep(.el-icon) {
  font-size: 8px;
}

.title-bar-macos .title-bar-controls .close-btn {
  color: rgba(255, 95, 86, 0.85);
}

.title-bar-macos .title-bar-controls .close-btn:hover {
  background-color: rgba(255, 95, 86, 0.9) !important;
  color: white !important;
}

.title-bar-macos .title-bar-controls .title-bar-btn:nth-child(2) {
  color: rgba(255, 189, 46, 0.85);
}

.title-bar-macos .title-bar-controls .title-bar-btn:nth-child(2):hover {
  background-color: rgba(255, 189, 46, 0.9) !important;
  color: white !important;
}

.title-bar-macos .title-bar-controls .title-bar-btn:nth-child(3) {
  color: rgba(39, 201, 63, 0.85);
}

.title-bar-macos .title-bar-controls .title-bar-btn:nth-child(3):hover {
  background-color: rgba(39, 201, 63, 0.9) !important;
  color: white !important;
}
</style>
