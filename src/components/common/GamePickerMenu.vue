<template>
  <Teleport to="body">
    <Transition name="fade">
      <div
        v-if="show"
        class="game-picker-backdrop no-drag allow-context-menu"
        @mousedown.self="onClickOutside"
      >
        <div
          ref="menuEl"
          class="game-picker-menu"
          :style="menuStyle"
          @mousedown.stop
        >
          <div class="game-picker-header">
            <div class="game-picker-title">选择要加载的游戏</div>
            <div v-if="foregroundProcessName" class="game-picker-subtitle">
              当前前台进程：<span class="game-picker-proc-name">{{ foregroundProcessName }}</span>
            </div>
            <div v-else class="game-picker-subtitle">未识别到受支持的游戏进程，请手动选择</div>
          </div>
          <div class="game-picker-divider" />
          <div class="game-picker-list" role="menu">
            <button
              v-for="item in GAME_OPTIONS"
              :key="item.value"
              class="game-picker-item"
              role="menuitem"
              :class="{ 'is-active': item.value === currentGame }"
              @click="onSelect(item.value)"
            >
              <span class="game-picker-item__icon">{{ item.icon }}</span>
              <span class="game-picker-item__label">{{ item.label }}</span>
              <span v-if="item.value === currentGame" class="game-picker-item__check">✓</span>
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
/**
 * 热键未匹配到前台游戏时弹出的游戏选择菜单
 * - 轻量右键菜单样式，通过 x/y 定位在光标附近
 * - 点击外部 / Escape 关闭
 * - 选中游戏后 emit('select', game)
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { TargetGame } from '@/types'
import { useSettingsStore } from '@/stores/settings'

const settingsStore = useSettingsStore()
const currentGame = computed(() => settingsStore.currentGame)

interface Props {
  show: boolean
  /** 屏幕 X 坐标（后端传入的光标屏幕位置），会在组件内转换为客户端坐标 */
  screenX: number
  /** 屏幕 Y 坐标 */
  screenY: number
  /** 当前前台进程名，展示给用户提示（可空） */
  foregroundProcessName?: string
}

const props = withDefaults(defineProps<Props>(), {
  show: false,
  screenX: 0,
  screenY: 0,
  foregroundProcessName: '',
})

const emit = defineEmits<{
  (e: 'select', game: TargetGame): void
  (e: 'close'): void
}>()

type GameOption = { value: TargetGame; label: string; icon: string }
const GAME_OPTIONS: ReadonlyArray<GameOption> = [
  { value: 'GenshinImpact', label: 'Genshin Impact (原神)', icon: '⚔' },
  { value: 'HonkaiStarRail', label: 'Honkai: Star Rail (星穹铁道)', icon: '🚄' },
  { value: 'ZZZ', label: 'Zenless Zone Zero (绝区零)', icon: '🎮' },
  { value: 'HonkaiImpact3rd', label: 'Honkai Impact 3rd (崩坏3)', icon: '⚡' },
  { value: 'Wuwa', label: 'Wuthering Waves (鸣潮)', icon: '🌊' },
  { value: 'ArknightsEndfield', label: 'Arknights: Endfield (明日方舟：终末地)', icon: '🏗' },
]

const menuEl = ref<HTMLElement | null>(null)

/** 最终应用到菜单的 style（屏幕坐标→客户端坐标，并裁剪到不超出窗口大小） */
const menuStyle = computed(() => {
  const baseLeft = props.screenX - (window.screenX ?? 0)
  const baseTop = props.screenY - (window.screenY ?? 0)

  const vw = window.innerWidth
  const vh = window.innerHeight
  const menuW = 320
  const menuH = 340

  let left = baseLeft
  let top = baseTop
  // 溢出右/下边时，翻转到左上
  if (left + menuW > vw - 8) left = Math.max(8, vw - menuW - 8)
  if (top + menuH > vh - 8) top = Math.max(8, vh - menuH - 8)
  if (left < 8) left = 8
  if (top < 8) top = 8

  return {
    left: `${left}px`,
    top: `${top}px`,
    width: `${menuW}px`,
  }
})

function onClickOutside() {
  emit('close')
}

function onSelect(game: TargetGame) {
  emit('select', game)
}

function onKey(e: KeyboardEvent) {
  if (!props.show) return
  if (e.key === 'Escape') {
    e.preventDefault()
    emit('close')
  }
}

onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))

// 显示时，阻止首次 mousedown 从外部穿透（避免与 hotkey 触发时的按键释放冲突）
watch(
  () => props.show,
  (val) => {
    if (val && menuEl.value) {
      // 空操作：保留结构以便后续扩展（焦点放置等）
    }
  }
)
</script>

<style scoped>
.game-picker-backdrop {
  position: fixed;
  inset: 0;
  z-index: 9999;
}

.game-picker-menu {
  position: absolute;
  background: rgba(30, 30, 34, 0.94);
  color: var(--text-primary, #fff);
  border: 1px solid rgba(255, 255, 255, 0.1);
  border-radius: 10px;
  box-shadow:
    0 8px 28px rgba(0, 0, 0, 0.45),
    0 0 0 1px rgba(255, 255, 255, 0.04) inset;
  padding: 10px 0;
  font-size: 13px;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.game-picker-header {
  padding: 2px 14px 8px;
}
.game-picker-title {
  font-size: 13px;
  font-weight: 600;
  letter-spacing: 0.2px;
  color: #fff;
}
.game-picker-subtitle {
  margin-top: 4px;
  font-size: 11.5px;
  color: rgba(255, 255, 255, 0.55);
}
.game-picker-proc-name {
  color: rgba(74, 158, 255, 0.9);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
}
.game-picker-divider {
  height: 1px;
  margin: 4px 10px 6px;
  background: rgba(255, 255, 255, 0.08);
}

.game-picker-list {
  display: flex;
  flex-direction: column;
  padding: 2px 6px;
  gap: 1px;
}

.game-picker-item {
  all: unset;
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 8px;
  border-radius: 7px;
  color: var(--text-primary, #fff);
  transition: background-color 0.12s ease, color 0.12s ease;
}
.game-picker-item:hover {
  background: rgba(74, 158, 255, 0.18);
  color: #fff;
}
.game-picker-item.is-active {
  background: rgba(74, 158, 255, 0.26);
  color: #fff;
}
.game-picker-item__icon {
  width: 20px;
  text-align: center;
  font-size: 14px;
  opacity: 0.85;
}
.game-picker-item__label {
  flex: 1;
  font-size: 12.5px;
}
.game-picker-item__check {
  color: var(--accent-primary, #4a9eff);
  font-size: 12px;
  font-weight: 700;
}

.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.12s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
