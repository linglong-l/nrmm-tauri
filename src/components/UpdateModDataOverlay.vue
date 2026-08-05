<template>
  <Teleport to="body">
    <Transition name="overlay-fade">
      <div v-if="visible" class="overlay-root" @click.self>
        <div class="overlay-box">
          <div class="overlay-title">{{ $t('updateModData.title') }}</div>

          <template v-if="state === 'loading'">
            <div class="overlay-loading">
              <span class="loading-text">
                {{ $t('updateModData.updating') }}
                <span v-for="n in dotsCount" :key="n">.</span>
              </span>
            </div>
          </template>

          <template v-else-if="state === 'completed'">
            <!-- ORFix/TexFx 检测结果（位于"模组管理"标题与"模组管理成功"之间） -->
            <div v-if="result?.orfixDetection?.hasDetection" class="overlay-orfix-hint">
              <!-- 库在模组内 -->
              <div
                v-for="lib in result.orfixDetection.libsInMods"
                :key="`inmod-${lib.modPath}`"
                class="orfix-block"
              >
                <div class="orfix-line orfix-warning">
                  {{ lib.libNames.join('/') }} {{ $t('updateModData.orfixDetectedIn', { mod: lib.modName }) }}
                </div>
                <div class="orfix-line orfix-error">
                  {{ $t('updateModData.orfixMoveToRoot') }}
                </div>
              </div>
              <!-- 重复声明 -->
              <div
                v-for="dup in result.orfixDetection.duplicateLibs"
                :key="`dup-${dup.libName}`"
                class="orfix-block"
              >
                <div class="orfix-line orfix-warning">
                  {{ dup.libName }} {{ $t('updateModData.orfixDuplicate', { mods: dup.modNames.join('、') }) }}
                </div>
                <div class="orfix-line orfix-error">
                  {{ $t('updateModData.orfixMoveToRoot') }}
                </div>
              </div>
              <!-- 引用未声明 -->
              <div
                v-for="ne in result.orfixDetection.nonexistentLibs"
                :key="`ne-${ne.libName}`"
                class="orfix-block"
              >
                <div class="orfix-line orfix-warning">
                  {{ ne.libName }} {{ $t('updateModData.orfixNonExistent', { mods: ne.modNames.join('、') }) }}
                </div>
                <div class="orfix-line orfix-error">
                  {{ $t('updateModData.orfixMoveToRoot') }}
                </div>
              </div>
            </div>
            <div class="overlay-success-line">
              <span class="success-text">{{ $t('updateModData.success') }}</span>
              <span v-if="durationSec >= 0" class="duration-text">
                {{ $t('updateModData.loadTime') }} {{ durationSec.toFixed(2) }}s
              </span>
            </div>
            <div class="overlay-tip-text">{{ $t('updateModData.optimizationTip') }}</div>
            <div v-if="result?.isStandardXxmi" class="overlay-xxmi-hint">
              <div class="xxmi-line">{{ $t('updateModData.xxmiDetected') }}</div>
              <div class="xxmi-line">{{ $t('updateModData.xxmiBackground') }}</div>
            </div>
            <div class="overlay-footer">
              <button class="btn-close-reload" @click="handleCloseAndReload">
                {{ $t('updateModData.closeAndReload') }}
              </button>
            </div>
          </template>

          <template v-else-if="state === 'error'">
            <div class="overlay-error-line">
              <span class="error-icon">✕</span>
              <span class="error-text">{{ errorMessage }}</span>
            </div>
            <div class="overlay-footer">
              <button class="btn-close-reload" @click="hideOverlay">
                {{ $t('updateModData.close') }}
              </button>
            </div>
          </template>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
import { ref, onBeforeUnmount } from 'vue'
import { useSettingsStore } from '@/stores/settings'
import { useModsStore } from '@/stores/mods'
import { simulateF10 } from '@/utils/tauri'
import { logger } from '@/utils/logger'

type OverlayState = 'loading' | 'completed' | 'error'

const visible = ref(false)
const state = ref<OverlayState>('loading')
const result = ref<any>(null)
const errorMessage = ref('')
const durationSec = ref(-1)

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

function show(s: OverlayState, data?: { result?: any; error?: string; durationMs?: number }) {
  state.value = s
  result.value = data?.result ?? null
  errorMessage.value = data?.error ?? ''
  durationSec.value = data?.durationMs != null ? data.durationMs / 1000 : -1
  visible.value = true
  if (s === 'loading') startDots()
  else stopDots()
}
function hide() {
  stopDots()
  visible.value = false
}
defineExpose({ show, hide })

async function handleCloseAndReload() {
  // 先立即关闭遮罩和提示框，恢复正常 UI 交互
  hide()
  // 后续 F10 模拟与模组刷新在后台执行，不阻塞 UI
  try {
    const settingsStore = useSettingsStore()
    await simulateF10(settingsStore.currentGame ?? undefined)
  } catch (e) {
    logger.error('UpdateModDataOverlay', 'simulateF10 failed (non-blocking)', e)
  }
  try {
    const settingsStore = useSettingsStore()
    const modsStore = useModsStore()
    if (settingsStore.currentGame && settingsStore.currentModsPath) {
      await modsStore.refresh()
    }
  } catch (e) {
    logger.error('UpdateModDataOverlay', 'refresh after close failed', e)
  }
}
function hideOverlay() { hide() }

onBeforeUnmount(() => stopDots())
</script>

<style scoped>
.overlay-root {
  position: fixed; inset: 0; z-index: 9999; pointer-events: all;
  background: rgba(0, 0, 0, 0.45);
  display: flex; align-items: center; justify-content: center;
}
.overlay-box {
  background: #1e1e1e; border-radius: 8px; padding: 28px 32px 24px;
  min-width: 440px; max-width: 560px; color: #fff;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
}
.overlay-title { font-size: 18px; font-weight: 700; margin-bottom: 16px; }
.overlay-loading { }
.loading-text { font-size: 14px; color: #fff; }
.success-text { color: #67c23a; font-size: 14px; font-weight: 600; margin-right: 12px; }
.duration-text { color: #909399; font-size: 14px; }
.overlay-success-line { margin-bottom: 12px; }
.overlay-tip-text { color: #fff; font-size: 14px; margin-bottom: 14px; line-height: 1.6; }
.overlay-xxmi-hint { margin-top: 4px; padding-top: 12px; }
.xxmi-line { color: #606266; font-size: 12px; line-height: 1.6; }
.overlay-orfix-hint { margin-bottom: 14px; padding: 12px 0; max-height: 200px; overflow-y: auto; }
.orfix-block { margin-bottom: 12px; padding: 8px 0; border-bottom: 1px solid rgba(255,255,255,0.06); }
.orfix-block:last-child { border-bottom: none; margin-bottom: 0; }
.orfix-line { font-size: 13px; line-height: 1.6; }
.orfix-warning { color: #e6a23c; }
.orfix-error { color: #f56c6c; }
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
