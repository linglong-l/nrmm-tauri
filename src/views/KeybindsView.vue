<template>
  <div class="keybinds-view">
    <div class="keybinds-scroll hide-scrollbar" ref="scrollRef">
      <div class="keybinds-container">
        <!-- 标题行：右侧对齐的页面标题 -->
        <div class="header-row">
          <div class="spacer" aria-hidden="true"></div>
          <h2 class="keybinds-title">{{ t('Keybinds') }}</h2>
        </div>

        <!-- 按键模拟支持状态卡片 -->
        <div class="status-card" :class="keypressStatusClass">
          <el-icon :size="24" class="status-icon" aria-hidden="true">
            <WarningFilled v-if="!keypressSupported" />
            <CircleCheckFilled v-else />
          </el-icon>
          <div class="status-content">
            <h3 class="status-title">
              {{ keypressSupported ? t('keybinds.keypressAvailable', 'Keypress simulation available') : t('keybinds.keypressUnavailable', 'Keypress simulation not available') }}
            </h3>
            <p class="status-desc">
              {{ keypressSupported ? t('keybinds.keypressWorkDesc', 'Hotkeys will work in supported games.') : keypressHint }}
            </p>
          </div>
        </div>

        <!-- 当前选中模组名称显示 -->
        <div class="mod-section">
          <h3 class="mod-name">
            {{ selectedMod?.modName || t('keybinds.emptySelectMod') }}
          </h3>
        </div>

        <!-- 按键模拟开关 -->
        <div class="simulate-row">
          <el-switch v-model="simulateEnabled" active-color="#4a9eff" inactive-color="#555" />
          <span class="simulate-label">{{ t('Click keybind to simulate keypress') }}</span>
        </div>

        <!-- 按键绑定卡片网格：遍历当前选中模组的所有按键配置 -->
        <div v-if="selectedMod && keybindList.length > 0" class="keybind-grid" role="list">
          <button
            v-for="(kb, idx) in keybindList"
            :key="idx"
            type="button"
            class="keybind-card"
            role="listitem"
            :disabled="kb.disabled"
            :class="{ disabled: kb.disabled }"
            @click="onKeybindClick(kb)"
          >
            <div class="keybind-section">{{ kb.section || t('keybinds.keys') }}</div>
            <div class="keybind-value">{{ kb.value || '—' }}</div>
          </button>
        </div>

        <!-- 无按键绑定时的空状态 -->
        <div v-else-if="selectedMod && keybindList.length === 0" class="empty-card">
          <p>{{ t('keybinds.noKeybinds') }}</p>
        </div>
      </div>
    </div>

    <!-- 版本号显示 -->
    <div class="version-tag-bottom" v-if="appVersion">v{{ appVersion }}</div>
  </div>
</template>

<script setup lang="ts">
/**
 * 按键绑定页面
 * 显示当前选中模组的按键配置列表，支持点击模拟按键
 * 拖拽滚动复用 useDragScroll composable
 * 检测平台按键模拟支持状态并显示提示
 */
import { ref, computed, onMounted, onBeforeUnmount, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { WarningFilled, CircleCheckFilled } from '@element-plus/icons-vue'
import { checkKeypressSupport, getAppVersion } from '@/utils/tauri'
import { usePlatform } from '@/stores/platform'
import { useModsStore } from '@/stores/mods'
import { logger } from '@/utils/logger'
import { useDragScroll } from '@/composables/useDragScroll'
import type { KeybindData } from '@/types'

const { t } = useI18n()
const { platformInfo } = usePlatform()
const modsStore = useModsStore()

/** 滚动容器DOM引用 */
const scrollRef = ref<HTMLElement | null>(null)
/** 拖拽滚动：复用 useDragScroll composable（默认排除规则已覆盖按钮/开关等交互元素） */
useDragScroll(scrollRef)

onBeforeUnmount(() => {
  // 离开按键绑定页时清理目标模组（防止切回 Mods 后 selectedMod 仍显示旧 keybind 目标）
  modsStore.clearKeybindTargetMod()
})

/** 当前选中的模组 */
const selectedMod = computed(() => modsStore.selectedMod)
/** 当前选中模组的按键绑定列表 */
const keybindList = computed<KeybindData[]>(() => selectedMod.value?.modIni?.keybinds || [])

/** 按键模拟功能是否受支持 */
const keypressSupported = ref(true)
/** 是否启用点击模拟按键 */
const simulateEnabled = ref(true)
/** 应用版本号 */
const appVersion = ref('')

/** 状态卡片样式类（成功/错误） */
const keypressStatusClass = computed(() => ({
  'status-error': !keypressSupported.value,
  'status-success': keypressSupported.value
}))

/**
 * 按键不支持时的平台特定提示
 * Linux: 提示安装xdotool/ydotool
 * macOS: 提示授予辅助功能权限
 */
const keypressHint = computed(() => {
  if (!platformInfo.value) return ''
  const os = platformInfo.value.os
  if (os === 'linux') {
    return t('keybinds.keypressLinuxHint', 'Please install xdotool or ydotool for keypress simulation support.')
  }
  if (os === 'macos') {
    return t('keybinds.keypressMacosHint', 'Please grant accessibility permissions for keypress simulation.')
  }
  return t('keybinds.keypressGenericHint', 'Keypress simulation requires additional setup.')
})

/**
 * 按键卡片点击处理
 * 根据开关决定是否模拟按键（当前仅记录日志，实际模拟由后端热键系统处理）
 */
function onKeybindClick(kb: KeybindData) {
  if (!simulateEnabled.value) return
  logger.info('KeybindsView', `Simulate keybind: section=${kb.section}, value=${kb.value}`)
}

onMounted(async () => {
  // 检测平台按键模拟支持状态
  try {
    await checkKeypressSupport()
    keypressSupported.value = true
  } catch (e) {
    keypressSupported.value = false
  }
  // 获取应用版本号
  try {
    appVersion.value = await getAppVersion()
  } catch (e) {
    logger.warn('KeybindsView', 'Failed to get app version', e)
  }
  await nextTick()
})
</script>

<style scoped>
.keybinds-view {
  flex: 1;
  display: flex;
  flex-direction: column;
  position: relative;
  overflow: hidden;
  background: transparent;
}

.keybinds-scroll {
  flex: 1;
  overflow-y: auto;
  padding: 24px 24px 96px;
}

.keybinds-container {
  max-width: 760px;
  margin: 0 auto;
}

.header-row {
  display: flex;
  align-items: center;
  margin-bottom: 16px;
}

.spacer {
  flex: 1;
}

.keybinds-title {
  font-size: 24px;
  font-weight: 600;
  color: var(--text-primary);
  margin: 0;
  text-align: right;
}

.status-card {
  display: flex;
  align-items: flex-start;
  gap: 16px;
  padding: 20px;
  border-radius: var(--border-radius, 12px);
  border: 1px solid;
  margin-bottom: 24px;
}

.status-card.status-success {
  background: rgba(40, 200, 64, 0.08);
  border-color: var(--accent-success, #28c840);
}

.status-card.status-error {
  background: rgba(231, 76, 60, 0.08);
  border-color: var(--accent-danger, #e74c3c);
}

.status-icon {
  flex-shrink: 0;
  margin-top: 2px;
}

.status-success .status-icon {
  color: var(--accent-success, #28c840);
}

.status-error .status-icon {
  color: var(--accent-danger, #e74c3c);
}

.status-content {
  flex: 1;
}

.status-title {
  margin: 0 0 8px;
  font-size: 15px;
  font-weight: 600;
  color: var(--text-primary);
}

.status-desc {
  margin: 0;
  font-size: 13px;
  color: var(--text-secondary);
  line-height: 1.5;
}

.mod-section {
  margin-bottom: 20px;
}

.mod-name {
  text-align: center;
  font-size: 18px;
  font-weight: 700;
  color: var(--text-primary);
  margin: 8px 0;
  line-height: 1.4;
}

.simulate-row {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  margin-bottom: 24px;
}

.simulate-label {
  color: var(--text-secondary);
  font-size: 14px;
}

.keybind-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
  gap: 14px;
}

.keybind-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 18px 12px;
  min-height: 96px;
  background: rgba(10, 10, 12, 0.45);
  border: 1px solid rgba(255, 255, 255, 0.16);
  border-radius: 12px;
  color: var(--text-primary);
  cursor: pointer;
  transition: transform 0.08s ease, border-color 0.2s ease, background-color 0.2s ease;
  font-family: inherit;
}

.keybind-card:hover {
  border-color: rgba(255, 255, 255, 0.32);
  background: rgba(18, 18, 22, 0.55);
}

.keybind-card:active {
  transform: scale(0.98);
  border-color: var(--accent-primary, #4a9eff);
}

.keybind-card.disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.keybind-section {
  font-size: 16px;
  font-weight: 700;
  margin-bottom: 8px;
  color: var(--text-primary);
}

.keybind-value {
  font-size: 12px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  color: var(--text-muted);
}

.empty-card {
  background: rgba(255, 255, 255, 0.03);
  border: 1px solid var(--border-color, rgba(255,255,255,0.12));
  border-radius: var(--border-radius, 12px);
  padding: 36px 24px;
  text-align: center;
  color: var(--text-secondary);
  font-size: 14px;
}

.version-tag-bottom {
  position: absolute;
  right: 16px;
  bottom: 64px;
  font-size: 11px;
  color: var(--text-muted);
  z-index: 10;
  user-select: none;
}
</style>
