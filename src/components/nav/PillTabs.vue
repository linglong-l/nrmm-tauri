<template>
  <div class="pill-tabs no-drag" role="tablist" aria-label="Main navigation">
    <!-- 胶囊标签按钮：遍历tabs配置，支持键盘导航 -->
    <button
      v-for="tab in tabs"
      :key="tab.key"
      class="pill-tab"
      :class="{ active: modelValue === tab.key, 'is-empty': !tab.label }"
      role="tab"
      :aria-selected="modelValue === tab.key"
      :aria-label="tab.label"
      :tabindex="modelValue === tab.key ? 0 : -1"
      @click="tab.label && $emit('update:modelValue', tab.key)"
      @keydown.enter.prevent="tab.label && $emit('update:modelValue', tab.key)"
      @keydown.space.prevent="tab.label && $emit('update:modelValue', tab.key)"
    >
      {{ tab.label }}
    </button>
  </div>
</template>

<script setup lang="ts">
/**
 * 胶囊导航标签组件
 * 提供页面切换的导航UI，支持键盘Enter/Space激活
 * 空标签项作为占位符不响应点击
 */
defineProps<{
  /** 当前激活的标签key（v-model） */
  modelValue: string
  /** 标签配置数组 */
  tabs: Array<{ key: string; label: string }>
}>()

defineEmits<{
  'update:modelValue': [key: string]
}>()
</script>

<style scoped>
.pill-tabs {
  display: inline-flex;
  border-radius: 24px;
  padding: 3px;
  -webkit-app-region: no-drag;
  user-select: none;
  backdrop-filter: blur(20px) saturate(180%);
  -webkit-backdrop-filter: blur(20px) saturate(180%);
}

.pill-tab {
  background: transparent;
  border: none;
  color: rgba(255, 255, 255, 0.6);
  font-size: 13px;
  padding: 6px 24px;
  border-radius: 20px;
  cursor: pointer;
  transition: background-color 0.2s ease, color 0.2s ease, transform 0.1s ease;
  font-family: inherit;
  white-space: nowrap;
}

.pill-tab:hover {
  color: rgba(255, 255, 255, 0.85);
  background: rgba(255, 255, 255, 0.06);
}

.pill-tab:focus-visible {
  outline: 2px solid var(--accent-primary);
  outline-offset: 2px;
}

.pill-tab.active {
  background: rgba(255, 255, 255, 0.92);
  color: #121212;
  font-weight: 600;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.12);
}

.pill-tab.is-empty {
  cursor: default;
  min-width: 20px;
  padding: 6px 12px;
}

.pill-tab.is-empty:hover {
  color: rgba(255, 255, 255, 0.6);
}
</style>
