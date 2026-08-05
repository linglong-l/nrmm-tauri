<template>
  <Teleport to="body">
    <Transition name="remove-mod-fade">
      <div v-if="visible" class="remove-mod-root" @click.self="handleOverlayClick">
        <div class="remove-mod-box">
          <!-- 标题：白色、较大字体（成功态加粗） -->
          <div class="remove-mod-title" :class="{ 'is-bold': state === 'success' }">
            {{ t('removeMod.title') }}
          </div>

          <!-- UI1：确认对话框 -->
          <template v-if="state === 'confirm'">
            <!-- 模组名称（白色） -->
            <div class="remove-mod-name">{{ modName }}</div>
            <!-- 警告文本（黄色） -->
            <div class="remove-mod-warning">{{ t('removeMod.warning') }}</div>
            <!-- 文件夹移动提示（淡灰色） -->
            <div class="remove-mod-folder-hint">{{ t('removeMod.folderMovedTo') }}</div>
            <!-- 底部按钮区：右下角取消（蓝色）+ 确认（红色） -->
            <div class="remove-mod-footer">
              <button class="rm-btn rm-btn-cancel" @click="handleCancel">
                {{ t('removeMod.cancel') }}
              </button>
              <button class="rm-btn rm-btn-confirm" @click="handleConfirm">
                {{ t('removeMod.confirm') }}
              </button>
            </div>
          </template>

          <!-- 加载中态 -->
          <template v-else-if="state === 'loading'">
            <div class="remove-mod-loading">
              <span class="loading-text">
                {{ t('removeMod.loading') }}
                <span v-for="n in dotsCount" :key="n">.</span>
              </span>
            </div>
          </template>

          <!-- UI2：成功提示框 -->
          <template v-else-if="state === 'success'">
            <!-- 文件夹已移至（绿色） -->
            <div class="remove-mod-success-line">{{ t('removeMod.success.folderMoved') }}</div>
            <!-- 模组还原成功（绿色） -->
            <div class="remove-mod-success-line">{{ t('removeMod.success.modRestored') }}</div>
            <!-- 底部确认按钮（蓝色） -->
            <div class="remove-mod-footer">
              <button class="rm-btn rm-btn-cancel" @click="handleSuccessClose">
                {{ t('removeMod.confirm') }}
              </button>
            </div>
          </template>

          <!-- 错误态 -->
          <template v-else-if="state === 'error'">
            <div class="remove-mod-error-line">
              <span class="error-icon">✕</span>
              <span class="error-text">{{ errorMessage }}</span>
            </div>
            <div class="remove-mod-footer">
              <button class="rm-btn rm-btn-cancel" @click="handleErrorClose">
                {{ t('removeMod.confirm') }}
              </button>
            </div>
          </template>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
/**
 * 移除模组对话框组件
 *
 * 两阶段交互流程（NRMM 对齐）：
 * - UI1（confirm）：用户确认是否移除模组
 *     - 取消：关闭对话框，不触发任何逻辑
 *     - 确认：调用后端 remove_mod（移动文件夹至 DISABLED_MANAGED_REMOVED + 还原 INI）
 * - UI2（success）：移除成功提示
 *     - 确认：关闭对话框并触发模组重读取（emit 'removed'）
 *
 * 对话框外观：
 * - 整体背景 #2B2930 圆角
 * - 标题白色较大字体（成功态加粗）
 * - 警告文本黄色，文件夹提示淡灰色，成功文本绿色
 * - 按钮为胶囊样式无背景色，悬停 #454B5D
 * - 取消按钮蓝色字体，确认按钮红色字体（UI1）；UI2 确认按钮蓝色字体
 */
import { ref, watch, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import { removeMod } from '@/utils/tauri'
import { logger } from '@/utils/logger'

type DialogState = 'confirm' | 'loading' | 'success' | 'error'

interface Props {
  /** 控制对话框显隐（v-model） */
  modelValue: boolean
  /** 待移除的模组名称（显示用） */
  modName: string
  /** 待移除的模组路径（传给后端） */
  modPath: string
}

const props = defineProps<Props>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: boolean): void
  /** UI2 确认按钮点击后触发，父组件应执行模组重读取 */
  (e: 'removed'): void
}>()

const { t } = useI18n()

/** 当前对话框状态 */
const state = ref<DialogState>('confirm')
/** 错误信息（error 态显示） */
const errorMessage = ref('')
/** 加载中省略号数量（动画） */
const dotsCount = ref(1)
let dotsTimer: ReturnType<typeof setInterval> | null = null

/** 对外暴露的 visible 计算属性，与 v-model 同步 */
const visible = ref(false)

watch(
  () => props.modelValue,
  (v) => {
    visible.value = v
    if (v) {
      // 每次打开都重置为确认态
      state.value = 'confirm'
      errorMessage.value = ''
    }
  },
  { immediate: true }
)

watch(visible, (v) => {
  if (v !== props.modelValue) {
    emit('update:modelValue', v)
  }
})

function startDots() {
  stopDots()
  dotsTimer = setInterval(() => {
    dotsCount.value = (dotsCount.value % 6) + 1
  }, 300)
}

function stopDots() {
  if (dotsTimer) {
    clearInterval(dotsTimer)
    dotsTimer = null
  }
}

/** UI1 取消按钮：仅关闭对话框，不触发任何逻辑 */
function handleCancel() {
  close()
}

/** 点击遮罩层：仅在 confirm 态允许关闭（避免 loading/success 误关） */
function handleOverlayClick() {
  if (state.value === 'confirm') {
    close()
  }
}

/** UI1 确认按钮：调用后端移除模组，成功后切换到 UI2 */
async function handleConfirm() {
  if (!props.modPath) return
  state.value = 'loading'
  startDots()
  try {
    await removeMod(props.modPath)
    state.value = 'success'
  } catch (e: any) {
    const msg = typeof e === 'string' ? e : (e?.message ?? String(e))
    logger.error('RemoveModDialog', 'remove_mod failed', e)
    errorMessage.value = msg || 'Unknown error'
    state.value = 'error'
  } finally {
    stopDots()
  }
}

/** UI2 确认按钮：关闭对话框并通知父组件重读模组 */
function handleSuccessClose() {
  close()
  emit('removed')
}

/** 错误态关闭：仅关闭对话框 */
function handleErrorClose() {
  close()
}

function close() {
  visible.value = false
}

onBeforeUnmount(() => {
  stopDots()
})
</script>

<style scoped>
.remove-mod-root {
  position: fixed;
  inset: 0;
  z-index: 9999;
  pointer-events: all;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
}

.remove-mod-box {
  background: #2b2930;
  border-radius: 12px;
  padding: 24px 28px 20px;
  min-width: 380px;
  max-width: 480px;
  color: #fff;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
}

.remove-mod-title {
  color: #ffffff;
  font-size: 18px;
  font-weight: 500;
  margin-bottom: 16px;
  line-height: 1.4;
}

.remove-mod-title.is-bold {
  font-weight: 700;
}

.remove-mod-name {
  color: #ffffff;
  font-size: 14px;
  line-height: 1.6;
  margin-bottom: 14px;
  word-break: break-all;
}

.remove-mod-warning {
  color: #ffc107;
  font-size: 13px;
  line-height: 1.6;
  margin-bottom: 12px;
}

.remove-mod-folder-hint {
  color: #909399;
  font-size: 12px;
  line-height: 1.6;
  margin-bottom: 18px;
}

.remove-mod-loading {
  padding: 8px 0 4px;
}

.loading-text {
  color: #ffffff;
  font-size: 14px;
}

.remove-mod-success-line {
  color: #67c23a;
  font-size: 14px;
  line-height: 1.8;
  font-weight: 500;
}

.remove-mod-error-line {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  margin-bottom: 18px;
}

.error-icon {
  color: #f56c6c;
  font-weight: 700;
}

.error-text {
  color: #f56c6c;
  word-break: break-all;
}

.remove-mod-footer {
  display: flex;
  justify-content: flex-end;
  align-items: center;
  gap: 8px;
  margin-top: 18px;
}

.rm-btn {
  padding: 8px 20px;
  font-size: 13px;
  font-weight: 600;
  background: transparent;
  border: none;
  border-radius: 999px;
  cursor: pointer;
  transition: background-color 0.15s ease;
}

.rm-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.rm-btn:hover:not(:disabled) {
  background-color: #454b5d;
}

/* 取消按钮（蓝色字体） */
.rm-btn-cancel {
  color: #409eff;
}

/* 确认按钮（红色字体，仅 UI1） */
.rm-btn-confirm {
  color: #f56c6c;
}

.remove-mod-fade-enter-active,
.remove-mod-fade-leave-active {
  transition: opacity 0.18s ease;
}

.remove-mod-fade-enter-from,
.remove-mod-fade-leave-to {
  opacity: 0;
}
</style>
