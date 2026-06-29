<script setup lang="ts">
/**
 * SideNav.vue - 顶部侧边导航组件
 *
 * 作用：
 *  - 提供主页面三个标签页的切换入口：Keybinds（热键）、Mods（模组）、Settings（设置）。
 *  - 当前激活的标签页读写均委托给 uiStore，便于全局共享状态。
 *  - 标签页文案通过 i18n 进行本地化。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { useUiStore } from '../stores/ui';
import type { TabType } from '../types';

const { t } = useI18n();
const uiStore = useUiStore();

// 当前激活的标签页（双向绑定到 uiStore）
const activeTab = computed({
  get: () => uiStore.activeTab,
  set: (val: TabType) => uiStore.setActiveTab(val)
});

// 标签页配置：key 用于状态判断，label 用于 i18n 翻译键
const tabs = [
  { key: 'keybinds' as TabType, label: 'Keybinds' },
  { key: 'mods' as TabType, label: 'Mods' },
  { key: 'settings' as TabType, label: 'Settings' }
] as const;
</script>

<template>
  <!-- 顶部导航容器：居中放置一组标签按钮 -->
  <div class="top-nav">
    <div class="nav-tabs">
      <button
        v-for="tab in tabs"
        :key="tab.key"
        class="nav-tab"
        :class="{ active: activeTab === tab.key }"
        @click="activeTab = tab.key"
      >
        {{ t(tab.label) }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.top-nav {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 8px 16px;
  flex-shrink: 0;
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
}

.nav-tabs {
  display: flex;
  align-items: center;
  gap: 4px;
  background-color: rgba(255, 255, 255, 0.06);
  border-radius: 10px;
  padding: 4px;
}

.nav-tab {
  padding: 6px 20px;
  border-radius: 8px;
  border: none;
  background: transparent;
  color: rgba(255, 255, 255, 0.55);
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.2s ease;
  white-space: nowrap;
}

.nav-tab:hover {
  color: rgba(255, 255, 255, 0.85);
}

.nav-tab.active {
  background-color: rgba(255, 255, 255, 0.95);
  color: #1a1a1a;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.15);
}
</style>
