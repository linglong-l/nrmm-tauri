<template>
  <Teleport to="body">
    <Transition name="overlay-fade">
      <div v-if="visible" class="overlay-root" @click.self="hide">
        <div class="overlay-box">
          <div class="overlay-title">{{ $t('hashConflict.title') }}</div>

          <!-- 加载中 -->
          <template v-if="state === 'loading'">
            <div class="overlay-loading">
              <span class="loading-text">
                {{ $t('hashConflict.scanning') }}
                <span v-for="n in dotsCount" :key="n">.</span>
              </span>
            </div>
          </template>

          <!-- 完成 -->
          <template v-else-if="state === 'completed'">
            <div v-if="result" class="hash-summary">
              <span class="summary-text">
                {{ $t('hashConflict.summary', {
                  conflicts: result.conflicts.length,
                  mods: result.scannedMods,
                  hashes: result.scannedHashes
                }) }}
              </span>
            </div>
            <!-- 冲突列表（可滚动区域，支持鼠标拖动） -->
            <div
              v-if="result && result.conflicts.length > 0"
              class="hash-list"
              ref="scrollContainer"
              @mousedown="startDrag"
            >
              <div v-for="conflict in result.conflicts" :key="conflict.hash" class="hash-item">
                <span class="hash-label">hash:</span>
                <span class="hash-value">{{ conflict.hash }}</span>
                <span class="hash-arrow">-&gt;</span>
                <span class="hash-mods">{{ conflict.modNames.join('、') }}</span>
              </div>
            </div>
            <div v-else class="no-conflict">
              {{ $t('hashConflict.noConflict') }}
            </div>
            <div class="overlay-footer">
              <button class="btn-close-reload" @click="hide">{{ $t('hashConflict.close') }}</button>
            </div>
          </template>

          <!-- 错误 -->
          <template v-else-if="state === 'error'">
            <div class="overlay-error-line">
              <span class="error-icon">✕</span>
              <span class="error-text">{{ errorMessage }}</span>
            </div>
            <div class="overlay-footer">
              <button class="btn-close-reload" @click="hide">{{ $t('hashConflict.close') }}</button>
            </div>
          </template>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
import { ref, onBeforeUnmount } from 'vue'
import { detectHashConflicts } from '@/utils/tauri'
import { useSettingsStore } from '@/stores/settings'
import { logger } from '@/utils/logger'
import type { HashConflictResult } from '@/types'

type OverlayState = 'loading' | 'completed' | 'error'

const visible = ref(false)
const state = ref<OverlayState>('loading')
const result = ref<HashConflictResult | null>(null)
const errorMessage = ref('')
const scrollContainer = ref<HTMLElement | null>(null)

// 加载动画省略号
const dotsCount = ref(1)
let dotsTimer: ReturnType<typeof setInterval> | null = null

function startDots() {
  stopDots()
  dotsTimer = setInterval(() => {
    dotsCount.value = (dotsCount.value % 6) + 1
  }, 300)
}
function stopDots() {
  if (dotsTimer) { clearInterval(dotsTimer); dotsTimer = null }
}

// 鼠标拖动滚动（隐藏原生滚动条，符合项目"支持鼠标拖拽且无滚动条"约束）
let isDragging = false
let startY = 0
let startScrollTop = 0

function startDrag(e: MouseEvent) {
  if (!scrollContainer.value) return
  isDragging = true
  startY = e.clientY
  startScrollTop = scrollContainer.value.scrollTop
  document.addEventListener('mousemove', onDrag)
  document.addEventListener('mouseup', stopDrag)
  e.preventDefault()
}
function onDrag(e: MouseEvent) {
  if (!isDragging || !scrollContainer.value) return
  const delta = e.clientY - startY
  scrollContainer.value.scrollTop = startScrollTop - delta
}
function stopDrag() {
  isDragging = false
  document.removeEventListener('mousemove', onDrag)
  document.removeEventListener('mouseup', stopDrag)
}

async function show() {
  visible.value = true
  state.value = 'loading'
  result.value = null
  errorMessage.value = ''
  startDots()
  try {
    const settingsStore = useSettingsStore()
    if (!settingsStore.currentModsPath) {
      throw new Error('No mods path configured')
    }
    const res = await detectHashConflicts(settingsStore.currentModsPath)
    result.value = res
    state.value = 'completed'
  } catch (e: any) {
    logger.error('HashConflictOverlay', 'detectHashConflicts failed', e)
    errorMessage.value = e?.message || String(e)
    state.value = 'error'
  } finally {
    stopDots()
  }
}
function hide() {
  stopDots()
  stopDrag()
  visible.value = false
}
defineExpose({ show, hide })

onBeforeUnmount(() => {
  stopDots()
  stopDrag()
})
</script>

<style scoped>
.overlay-root {
  position: fixed; inset: 0; z-index: 9999; pointer-events: all;
  background: rgba(0, 0, 0, 0.45);
  display: flex; align-items: center; justify-content: center;
}
.overlay-box {
  background: #1e1e1e; border-radius: 8px; padding: 28px 32px 24px;
  min-width: 480px; max-width: 640px; max-height: 80vh; color: #fff;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
  display: flex; flex-direction: column;
}
.overlay-title { font-size: 18px; font-weight: 700; margin-bottom: 16px; }
.overlay-loading { }
.loading-text { font-size: 14px; color: #fff; }
.hash-summary { margin-bottom: 12px; }
.summary-text { color: #909399; font-size: 13px; }
.hash-list {
  flex: 1; overflow-y: auto; max-height: 400px;
  cursor: grab; user-select: none;
  scrollbar-width: none; -ms-overflow-style: none;
}
.hash-list::-webkit-scrollbar { display: none; }
.hash-list:active { cursor: grabbing; }
.hash-item {
  padding: 8px 0; border-bottom: 1px solid rgba(255,255,255,0.06);
  font-size: 13px; line-height: 1.6;
}
.hash-item:last-child { border-bottom: none; }
.hash-label { color: #606266; margin-right: 4px; }
.hash-value { color: #4a9eff; margin-right: 8px; font-family: monospace; }
.hash-arrow { color: #606266; margin-right: 8px; }
.hash-mods { color: #fff; }
.no-conflict { color: #67c23a; font-size: 14px; padding: 16px 0; }
.overlay-footer { display: flex; justify-content: flex-end; margin-top: 18px; }
.btn-close-reload {
  background: transparent; border: none; color: #409eff; font-size: 14px;
  cursor: pointer; padding: 6px 4px;
}
.btn-close-reload:hover { text-decoration: underline; }
.overlay-error-line { display: flex; align-items: center; gap: 8px; font-size: 14px; }
.error-icon { color: #f56c6c; font-weight: 700; }
.error-text { color: #f56c6c; }

.overlay-fade-enter-active, .overlay-fade-leave-active { transition: opacity 0.18s ease; }
.overlay-fade-enter-from, .overlay-fade-leave-to { opacity: 0; }
</style>
