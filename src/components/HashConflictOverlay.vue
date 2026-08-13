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
                <div class="hash-item-main">
                  <span class="hash-label">hash:</span>
                  <span
                    class="hash-value hash-clickable"
                    @click="toggleHashExpand(conflict.hash)"
                  >{{ conflict.hash }}</span>
                  <span class="hash-arrow">-&gt;</span>
                  <span class="hash-mods">
                    <template v-for="(entry, idx) in conflict.entries" :key="entry.modName">
                      <span
                        class="hash-mod-name"
                        @mouseenter="onModEnter($event, conflict.hash, entry.modName)"
                        @mouseleave="onModLeave"
                      >{{ entry.modName }}<span v-if="idx < conflict.entries.length - 1" class="mod-separator">、</span></span>
                    </template>
                  </span>
                </div>
                <!-- 行内展开详情：按模组分组显示 INI 路径（数据已按模组聚合） -->
                <div v-if="expandedHash === conflict.hash" class="hash-detail">
                  <div v-for="entry in conflict.entries" :key="entry.modName" class="hash-detail-group">
                    <div class="hash-detail-mod-name">{{ entry.modName }}</div>
                    <div
                      v-for="ini in entry.iniVec"
                      :key="ini"
                      class="hash-detail-ini-path"
                      :title="ini"
                    >{{ ini }}</div>
                  </div>
                </div>
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

    <!-- 自定义 Tooltip：悬浮模组名称 >1s 显示模组目录路径 -->
    <Transition name="tooltip-fade">
      <div
        v-if="tooltipVisible"
        class="hash-tooltip"
        :style="{ left: tooltipX + 'px', top: tooltipY + 'px' }"
      >{{ tooltipText }}</div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
import { ref, onBeforeUnmount } from 'vue'
import { detectHashConflicts } from '@/utils/tauri'
import { useSettingsStore } from '@/stores/settings'
import { logger } from '@/utils/logger'
import type { HashConflictResult } from '@/types'
import { useLoadingDots } from '@/composables/useLoadingDots'

type OverlayState = 'loading' | 'completed' | 'error'

const visible = ref(false)
const state = ref<OverlayState>('loading')
const result = ref<HashConflictResult | null>(null)
const errorMessage = ref('')
const scrollContainer = ref<HTMLElement | null>(null)

// 加载动画省略号（复用统一 composable，避免四处重复实现）
const { dotsCount, startDots, stopDots } = useLoadingDots()

// 鼠标拖动滚动（隐藏原生滚动条，符合项目"支持鼠标拖拽且无滚动条"约束）
let isDragging = false
let startY = 0
let startScrollTop = 0
// 拖拽抑制点击标志：拖拽位移 > 3px 时标记，抑制后续 click 事件
let hasDragged = false

function startDrag(e: MouseEvent) {
  if (!scrollContainer.value) return
  isDragging = true
  hasDragged = false
  startY = e.clientY
  startScrollTop = scrollContainer.value.scrollTop
  document.addEventListener('mousemove', onDrag)
  document.addEventListener('mouseup', stopDrag)
  e.preventDefault()
}
function onDrag(e: MouseEvent) {
  if (!isDragging || !scrollContainer.value) return
  const delta = e.clientY - startY
  // 位移超过 3px 标记为拖拽，抑制后续点击
  if (Math.abs(delta) > 3) {
    hasDragged = true
  }
  scrollContainer.value.scrollTop = startScrollTop - delta
}
function stopDrag() {
  isDragging = false
  document.removeEventListener('mousemove', onDrag)
  document.removeEventListener('mouseup', stopDrag)
}

// Tooltip 状态：悬浮模组名称 >1s 显示模组目录路径
const tooltipVisible = ref(false)
const tooltipText = ref('')
const tooltipX = ref(0)
const tooltipY = ref(0)
let hoverTimer: ReturnType<typeof setTimeout> | null = null

/**
 * 模组名称鼠标进入：1s 后显示该模组的目录路径
 * 收起状态仅显示模组目录（modPath），INI 文件路径在点击 hash 展开后可见
 */
function onModEnter(e: MouseEvent, hash: string, modName: string) {
  const conflict = result.value?.conflicts.find(c => c.hash === hash)
  if (!conflict) return
  const entry = conflict.entries.find(en => en.modName === modName)
  if (!entry) return
  const target = e.currentTarget as HTMLElement
  const rect = target.getBoundingClientRect()
  tooltipX.value = rect.left
  tooltipY.value = rect.top - 8
  hoverTimer = setTimeout(() => {
    tooltipText.value = entry.modPath
    tooltipVisible.value = true
  }, 1000)
}

/** 模组名称鼠标离开：清除定时器并隐藏 tooltip */
function onModLeave() {
  if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = null }
  tooltipVisible.value = false
}

// 行内展开状态：当前展开的 hash 值（null 表示全部收起）
const expandedHash = ref<string | null>(null)

/**
 * 切换 hash 详情展开状态
 * 拖拽滚动后（hasDragged=true）不触发点击，避免误展开
 */
function toggleHashExpand(hash: string) {
  if (hasDragged) return
  expandedHash.value = expandedHash.value === hash ? null : hash
}

async function show() {
  visible.value = true
  state.value = 'loading'
  result.value = null
  errorMessage.value = ''
  expandedHash.value = null
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
  onModLeave()
  expandedHash.value = null
  visible.value = false
}
defineExpose({ show, hide })

onBeforeUnmount(() => {
  stopDots()
  stopDrag()
  if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = null }
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
.hash-item-main { display: flex; align-items: center; flex-wrap: wrap; }
.hash-label { color: #606266; margin-right: 4px; }
.hash-value { color: #4a9eff; margin-right: 8px; font-family: monospace; }
.hash-arrow { color: #606266; margin-right: 8px; }
.hash-mods { color: #fff; }
.hash-mod-name {
  cursor: help;
  border-bottom: 1px dotted transparent;
  transition: border-color 0.15s ease;
}
.hash-mod-name:hover {
  border-bottom-color: rgba(74, 158, 255, 0.5);
}
.mod-separator { margin: 0 0; }
.hash-clickable {
  cursor: pointer;
  transition: color 0.15s ease;
}
.hash-clickable:hover {
  color: #66b1ff;
}
.no-conflict { color: #67c23a; font-size: 14px; padding: 16px 0; }
.overlay-footer { display: flex; justify-content: flex-end; margin-top: 18px; }
.btn-close-reload {
  background: transparent; border: none; color: #409eff; font-size: 14px;
  cursor: pointer; padding: 6px 16px; border-radius: 999px;
  transition: background-color 0.2s ease;
}
.btn-close-reload:hover {
  background: rgba(74, 158, 255, 0.12);
}
.overlay-error-line { display: flex; align-items: center; gap: 8px; font-size: 14px; }
.error-icon { color: #f56c6c; font-weight: 700; }
.error-text { color: #f56c6c; }

/* 行内展开详情：按模组分组 */
.hash-detail {
  margin-top: 6px;
  padding: 8px 12px;
  background: rgba(255, 255, 255, 0.03);
  border-left: 2px solid rgba(74, 158, 255, 0.4);
  border-radius: 4px;
}
.hash-detail-group { margin-bottom: 8px; }
.hash-detail-group:last-child { margin-bottom: 0; }
.hash-detail-mod-name {
  font-size: 13px;
  font-weight: 500;
  color: #4a9eff;
  margin-bottom: 2px;
}
.hash-detail-ini-path {
  font-size: 12px;
  line-height: 1.8;
  color: #909399;
  font-family: monospace;
  padding-left: 16px;
  word-break: break-all;
}

.overlay-fade-enter-active, .overlay-fade-leave-active { transition: opacity 0.18s ease; }
.overlay-fade-enter-from, .overlay-fade-leave-to { opacity: 0; }
.tooltip-fade-enter-active, .tooltip-fade-leave-active { transition: opacity 0.15s ease; }
.tooltip-fade-enter-from, .tooltip-fade-leave-to { opacity: 0; }
</style>

<!-- Tooltip 样式（非 scoped，因 Teleport 到 body） -->
<style>
.hash-tooltip {
  position: fixed; z-index: 10000;
  background: #2a2a2a; color: #fff; font-size: 12px;
  padding: 6px 10px; border-radius: 4px;
  max-width: 400px; word-break: break-all;
  font-family: monospace;
  transform: translateY(-100%);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.4);
  pointer-events: none;
}
</style>
