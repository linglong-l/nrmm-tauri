<template>
  <div v-if="visible" class="search-bar no-drag" ref="searchBarEl" @click.stop>
    <!-- 搜索图标 -->
    <svg class="search-icon" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <circle cx="11" cy="11" r="8"/>
      <line x1="21" y1="21" x2="16.65" y2="16.65"/>
    </svg>
    <!-- 搜索输入框：支持实时搜索、键盘导航 -->
    <input
      ref="inputEl"
      v-model="localQuery"
      class="search-input"
      :placeholder="t('common.searchPlaceholder', '搜索模组/分组')"
      @input="onInput"
      @keydown="onKeydown"
      @blur="onBlur"
    />
    <!-- 匹配计数显示：当前/总数 -->
    <span v-if="(totalMatches ?? 0) > 0" class="match-count">{{ (currentIndex ?? 0) + 1 }}/{{ totalMatches }}</span>
    <!-- 上一个/下一个导航按钮 -->
    <div v-if="(totalMatches ?? 0) > 0" class="nav-btns">
      <button class="nav-btn" @click="$emit('prev')" :title="t('common.searchPrev', '上一个 (Shift+Enter)')">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="18 15 12 9 6 15"/></svg>
      </button>
      <button class="nav-btn" @click="$emit('next')" :title="t('common.searchNext', '下一个 (Enter)')">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
      </button>
    </div>
    <!-- 关闭按钮 -->
    <button class="close-btn" @click="close" :title="t('common.searchClose', '关闭 (Esc)')">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
      </svg>
    </button>
  </div>
</template>

<script setup lang="ts">
/**
 * 搜索栏组件
 * 提供模糊搜索输入框，支持键盘导航（Enter/Shift+Enter切换匹配项，Esc关闭）
 * 失焦时自动关闭（有内容时保持打开）
 */
import { ref, watch, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'

const { t } = useI18n()

const props = defineProps<{
  /** 搜索栏是否可见 */
  visible: boolean
  /** 搜索关键词（v-model） */
  modelValue: string
  /** 总匹配数 */
  totalMatches?: number
  /** 当前匹配项索引 */
  currentIndex?: number
}>()

const emit = defineEmits<{
  'update:modelValue': [value: string]
  'update:visible': [value: boolean]
  /** 跳转到下一个匹配项 */
  next: []
  /** 跳转到上一个匹配项 */
  prev: []
  /** 关闭搜索栏 */
  close: []
}>()

/** 本地搜索关键词（避免直接修改props） */
const localQuery = ref(props.modelValue)
/** 输入框DOM引用 */
const inputEl = ref<HTMLInputElement | null>(null)
/** 搜索栏容器DOM引用（用于失焦检测） */
const searchBarEl = ref<HTMLElement | null>(null)

/** 搜索栏显示时自动聚焦并选中文本 */
watch(() => props.visible, (v) => {
  if (v) {
    nextTick(() => {
      inputEl.value?.focus()
      inputEl.value?.select()
    })
  }
})

/** 同步外部modelValue变化到本地 */
watch(() => props.modelValue, (v) => {
  localQuery.value = v
})

/** 输入事件：同步到父组件 */
function onInput() {
  emit('update:modelValue', localQuery.value)
}

/**
 * 键盘事件处理
 * - Escape: 关闭搜索栏
 * - Enter: 下一个匹配项
 * - Shift+Enter: 上一个匹配项
 */
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

/**
 * 失焦事件处理
 * 搜索词为空时延迟关闭，避免点击导航按钮时意外关闭
 */
function onBlur() {
  if (!localQuery.value) {
    setTimeout(() => {
      if (!localQuery.value && !searchBarEl.value?.contains(document.activeElement)) {
        close()
      }
    }, 150)
  }
}

/** 关闭搜索栏并清空搜索词 */
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
