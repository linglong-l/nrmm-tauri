<template>
  <div v-if="visible" class="search-bar no-drag" ref="searchBarEl">
    <svg class="search-icon" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <circle cx="11" cy="11" r="8"/>
      <line x1="21" y1="21" x2="16.65" y2="16.65"/>
    </svg>
    <input
      ref="inputEl"
      v-model="localQuery"
      class="search-input"
      placeholder="搜索模组/分组"
      @input="onInput"
      @keydown="onKeydown"
      @blur="onBlur"
    />
    <span v-if="(totalMatches ?? 0) > 0" class="match-count">{{ (currentIndex ?? 0) + 1 }}/{{ totalMatches }}</span>
    <div v-if="(totalMatches ?? 0) > 0" class="nav-btns">
      <button class="nav-btn" @click="$emit('prev')" title="上一个 (Shift+Enter)">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="18 15 12 9 6 15"/></svg>
      </button>
      <button class="nav-btn" @click="$emit('next')" title="下一个 (Enter)">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
      </button>
    </div>
    <button class="close-btn" @click="close" title="关闭 (Esc)">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
      </svg>
    </button>
  </div>
</template>

<script setup lang="ts">
import { ref, watch, nextTick } from 'vue'

const props = defineProps<{
  visible: boolean
  modelValue: string
  totalMatches?: number
  currentIndex?: number
}>()

const emit = defineEmits<{
  'update:modelValue': [value: string]
  'update:visible': [value: boolean]
  next: []
  prev: []
  close: []
}>()

const localQuery = ref(props.modelValue)
const inputEl = ref<HTMLInputElement | null>(null)
const searchBarEl = ref<HTMLElement | null>(null)

watch(() => props.visible, (v) => {
  if (v) {
    nextTick(() => {
      inputEl.value?.focus()
      inputEl.value?.select()
    })
  }
})

watch(() => props.modelValue, (v) => {
  localQuery.value = v
})

function onInput() {
  emit('update:modelValue', localQuery.value)
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    e.preventDefault()
    close()
  } else if (e.key === 'Enter') {
    e.preventDefault()
    if (e.shiftKey) {
      emit('prev')
    } else {
      emit('next')
    }
  }
}

function onBlur() {
  if (!localQuery.value) {
    setTimeout(() => {
      if (!localQuery.value && !searchBarEl.value?.contains(document.activeElement)) {
        close()
      }
    }, 150)
  }
}

function close() {
  emit('update:visible', false)
  emit('close')
  localQuery.value = ''
  emit('update:modelValue', '')
}
</script>

<style scoped>
.search-bar {
  display: flex;
  align-items: center;
  background: transparent;
  border-radius: 24px;
  padding: 0 12px;
  height: 40px;
  margin: 8px 16px 0;
  gap: 8px;
  -webkit-app-region: no-drag;
  backdrop-filter: blur(10px);
  flex-shrink: 0;
}

.search-icon {
  color: rgba(255, 255, 255, 0.5);
  flex-shrink: 0;
}

.search-input {
  flex: 1;
  background: transparent;
  border: none;
  outline: none;
  color: #fff;
  font-size: 14px;
  font-family: inherit;
  height: 100%;
}

.search-input::placeholder {
  color: rgba(255, 255, 255, 0.4);
}

.match-count {
  color: rgba(255, 255, 255, 0.5);
  font-size: 12px;
  flex-shrink: 0;
  min-width: 32px;
  text-align: center;
}

.nav-btns {
  display: flex;
  gap: 2px;
  flex-shrink: 0;
}

.nav-btn {
  width: 24px;
  height: 24px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: none;
  color: rgba(255, 255, 255, 0.6);
  cursor: pointer;
  border-radius: 4px;
  padding: 0;
}

.nav-btn:hover {
  background: transparent;
  color: #fff;
}

.close-btn {
  width: 24px;
  height: 24px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: none;
  color: rgba(255, 255, 255, 0.4);
  cursor: pointer;
  border-radius: 4px;
  padding: 0;
  flex-shrink: 0;
}

.close-btn:hover {
  background: transparent;
  color: #fff;
}
</style>
