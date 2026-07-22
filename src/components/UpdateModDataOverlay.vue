<script setup lang="ts">
/**
 * UpdateModDataOverlay.vue - 全屏遮罩组件
 *
 * 作用：
 *  - 在 update_mod_data 操作期间展示全屏遮罩，阻止用户任何交互。
 *  - 三种状态：loading（加载中）、completed（完成，展示结果）、error（异常）。
 *  - loading: 旋转加载动画 + "正在更新模组数据中..."
 *  - completed: 耗时、成功/失败统计、per-mod 错误列表（可折叠）、关闭按钮
 *  - error: 错误图标 + 错误消息 + 关闭按钮
 *
 * 使用方式：
 *  父组件通过 v-model:visible 控制显示/隐藏，通过 props 传入结果数据。
 */
import { ref, computed, watch } from 'vue';
import { Loading, CircleCheckFilled, WarningFilled, CircleCloseFilled } from '@element-plus/icons-vue';
import type { UpdateModDataResult, ModManageError } from '../types';
import { formatDuration } from '../utils/format';

const props = defineProps<{
  /** 是否显示遮罩 */
  visible: boolean;
  /** 当前状态 */
  state: 'loading' | 'completed' | 'error';
  /** 操作结果（completed 状态时传入） */
  result: UpdateModDataResult | null;
  /** 错误消息（error 状态时传入） */
  errorMessage: string | null;
}>();

const emit = defineEmits<{
  'update:visible': [value: boolean];
  close: [];
}>();

/** el-collapse 的激活项，用于控制错误列表折叠 */
const activeNames = ref<string[]>(['errors']);

/** 是否有 per-mod 错误 */
const hasErrors = computed(() => {
  return props.result && props.result.perModErrors && props.result.perModErrors.length > 0;
});

/** 成功数 */
const successCount = computed(() => {
  if (!props.result) return 0;
  return props.result.totalModsProcessed - props.result.totalErrors;
});

/** 格式化的耗时字符串 */
const formattedDuration = computed(() => {
  if (!props.result) return '0.00 秒';
  return formatDuration(props.result.durationMs);
});

/** 错误条目分组：按 stage 分组显示 */
const groupedErrors = computed(() => {
  if (!props.result) return {};
  const groups: Record<string, ModManageError[]> = {};
  for (const err of props.result.perModErrors) {
    const stage = err.stage;
    if (!groups[stage]) {
      groups[stage] = [];
    }
    groups[stage].push(err);
  }
  return groups;
});

/** 阶段标题映射 */
const stageLabels: Record<string, string> = {
  ini_backup: 'INI 备份异常',
  ini_modify: 'INI 修改异常',
  ini_write: 'INI 写入异常',
  validate: '路径校验异常'
};

/** 显示加载态时自动展开错误列表 */
watch(() => props.state, (newState) => {
  if (newState === 'completed') {
    activeNames.value = hasErrors.value ? ['errors'] : [];
  }
});

/** 关闭遮罩 */
function handleClose() {
  emit('update:visible', false);
  emit('close');
}

/** 阻止遮罩上的点击冒泡到下层 */
function stopPropagation(e: MouseEvent) {
  e.stopPropagation();
}
</script>

<template>
  <Teleport to="body">
    <Transition name="overlay-fade">
      <div
        v-if="visible"
        class="update-mod-overlay"
        @click.self="stopPropagation"
      >
        <!-- Loading 状态 -->
        <div v-if="state === 'loading'" class="overlay-content loading-content">
          <el-icon class="loading-icon" :size="48">
            <Loading />
          </el-icon>
          <p class="loading-text">正在更新模组数据中...</p>
        </div>

        <!-- Completed 状态 -->
        <div v-else-if="state === 'completed' && result" class="overlay-content completed-content">
          <!-- 图标与标题 -->
          <div class="result-header">
            <el-icon v-if="result.totalErrors === 0" class="result-icon success-icon" :size="48">
              <CircleCheckFilled />
            </el-icon>
            <el-icon v-else class="result-icon warn-icon" :size="48">
              <WarningFilled />
            </el-icon>
            <h2 class="result-title">模组数据更新完成</h2>
          </div>

          <!-- 统计信息 -->
          <div class="result-stats">
            <div class="stat-item">
              <span class="stat-label">共处理</span>
              <span class="stat-value">{{ result.totalModsProcessed }}</span>
              <span class="stat-label">个模组</span>
            </div>
            <div class="stat-item">
              <span class="stat-label">成功</span>
              <span class="stat-value success">{{ successCount }}</span>
              <span class="stat-label">个</span>
            </div>
            <div class="stat-item">
              <span class="stat-label">失败</span>
              <span class="stat-value error">{{ result.totalErrors }}</span>
              <span class="stat-label">个</span>
            </div>
            <div class="stat-item">
              <span class="stat-label">耗时</span>
              <span class="stat-value">{{ formattedDuration }}</span>
            </div>
          </div>

          <!-- Per-mod 错误列表 -->
          <div v-if="hasErrors" class="error-section">
            <el-collapse v-model="activeNames">
              <el-collapse-item title="模组处理异常详情" name="errors">
                <div
                  v-for="(errors, stage) in groupedErrors"
                  :key="stage"
                  class="error-group"
                >
                  <p class="error-stage-label">{{ stageLabels[stage] || stage }}</p>
                  <div
                    v-for="(err, index) in errors"
                    :key="index"
                    class="error-item"
                  >
                    <pre class="error-text">【模组：{{ err.modName }} 异常
{{ err.message }}】</pre>
                  </div>
                </div>
              </el-collapse-item>
            </el-collapse>
          </div>

          <!-- 关闭按钮 -->
          <el-button
            type="primary"
            class="close-btn"
            @click="handleClose"
          >
            关闭
          </el-button>
        </div>

        <!-- Error 状态 -->
        <div v-else-if="state === 'error'" class="overlay-content error-content">
          <el-icon class="result-icon error-icon" :size="48">
            <CircleCloseFilled />
          </el-icon>
          <h2 class="result-title">更新失败</h2>
          <p class="error-msg">{{ errorMessage || '未知错误' }}</p>
          <el-button
            type="primary"
            class="close-btn"
            @click="handleClose"
          >
            关闭
          </el-button>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.update-mod-overlay {
  position: fixed;
  top: 0;
  left: 0;
  width: 100vw;
  height: 100vh;
  z-index: 9999;
  display: flex;
  align-items: center;
  justify-content: center;
  background-color: rgba(0, 0, 0, 0.6);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  pointer-events: all;
}

/* Transition */
.overlay-fade-enter-active,
.overlay-fade-leave-active {
  transition: opacity 0.25s ease;
}
.overlay-fade-enter-from,
.overlay-fade-leave-to {
  opacity: 0;
}

.overlay-content {
  background-color: rgba(30, 30, 34, 0.98);
  border: 1px solid rgba(255, 255, 255, 0.1);
  border-radius: 16px;
  padding: 40px;
  min-width: 400px;
  max-width: 560px;
  max-height: 80vh;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  align-items: center;
  box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
}

/* Loading */
.loading-content {
  gap: 24px;
}

.loading-icon {
  color: var(--el-color-primary);
  animation: spin 1s linear infinite;
}

@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

.loading-text {
  font-size: 16px;
  color: rgba(255, 255, 255, 0.75);
  margin: 0;
}

/* Result header */
.result-header {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 16px;
  margin-bottom: 24px;
}

.result-icon.success-icon { color: #67c23a; }
.result-icon.warn-icon { color: #e6a23c; }
.result-icon.error-icon { color: #f56c6c; }

.result-title {
  font-size: 20px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.9);
  margin: 0;
}

/* Stats */
.result-stats {
  display: flex;
  flex-wrap: wrap;
  gap: 16px;
  justify-content: center;
  margin-bottom: 24px;
}

.stat-item {
  display: flex;
  align-items: baseline;
  gap: 4px;
}

.stat-label {
  font-size: 13px;
  color: rgba(255, 255, 255, 0.5);
}

.stat-value {
  font-size: 18px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.85);
}

.stat-value.success { color: #67c23a; }
.stat-value.error { color: #f56c6c; }

/* Error section */
.error-section {
  width: 100%;
  margin-bottom: 24px;
}

.error-section :deep(.el-collapse) {
  --el-collapse-header-bg-color: rgba(255, 255, 255, 0.04);
  --el-collapse-content-bg-color: rgba(255, 255, 255, 0.02);
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 8px;
  overflow: hidden;
}

.error-section :deep(.el-collapse-item__header) {
  color: rgba(255, 255, 255, 0.75);
  font-size: 14px;
  padding: 12px 16px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
}

.error-section :deep(.el-collapse-item__wrap) {
  border-bottom: none;
}

.error-section :deep(.el-collapse-item__content) {
  color: rgba(255, 255, 255, 0.65);
  padding: 12px 16px;
}

.error-group {
  margin-bottom: 12px;
}

.error-group:last-child {
  margin-bottom: 0;
}

.error-stage-label {
  font-size: 12px;
  color: rgba(255, 255, 255, 0.45);
  margin-bottom: 6px;
}

.error-item {
  margin-bottom: 8px;
}

.error-item:last-child {
  margin-bottom: 0;
}

.error-text {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
  font-size: 13px;
  color: rgba(255, 255, 255, 0.7);
  background-color: rgba(255, 255, 255, 0.04);
  border-radius: 6px;
  padding: 10px 12px;
  white-space: pre-wrap;
  word-break: break-all;
  line-height: 1.6;
  margin: 0;
}

/* Error state message */
.error-msg {
  font-size: 14px;
  color: rgba(255, 255, 255, 0.6);
  margin: 12px 0 24px;
  text-align: center;
}

/* Close button */
.close-btn {
  margin-top: 8px;
  min-width: 120px;
}

/* Scrollbar */
.overlay-content::-webkit-scrollbar {
  width: 6px;
}
.overlay-content::-webkit-scrollbar-track {
  background: rgba(255, 255, 255, 0.03);
  border-radius: 3px;
}
.overlay-content::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.15);
  border-radius: 3px;
}
</style>
