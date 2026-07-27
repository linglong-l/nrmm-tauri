<template>
  <div class="pill-tabs no-drag">
    <button
      v-for="tab in tabs"
      :key="tab.key"
      class="pill-tab"
      :class="{ active: modelValue === tab.key }"
      @click="$emit('update:modelValue', tab.key)"
    >
      {{ tab.label }}
    </button>
  </div>
</template>

<script setup lang="ts">
defineProps<{
  modelValue: string
  tabs: Array<{ key: string; label: string }>
}>()

defineEmits<{
  'update:modelValue': [key: string]
}>()
</script>

<style scoped>
.pill-tabs {
  display: inline-flex;
  background: transparent;
  border-radius: 24px;
  padding: 3px;
  -webkit-app-region: no-drag;
  user-select: none;
}

.pill-tab {
  background: transparent;
  border: none;
  color: rgba(255, 255, 255, 0.6);
  font-size: 13px;
  padding: 6px 24px;
  border-radius: 20px;
  cursor: pointer;
  transition: all 0.2s ease;
  font-family: inherit;
  white-space: nowrap;
}

.pill-tab:hover {
  color: rgba(255, 255, 255, 0.85);
}

.pill-tab.active {
  background: transparent;
  color: var(--text-primary);
  font-weight: 600;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
}
</style>
