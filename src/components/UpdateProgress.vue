<script setup lang="ts">
/**
 * UpdateProgress.vue
 *
 * 自更新下载/安装进度展示组件。
 *
 * 用法：
 *   <UpdateProgress
 *     :status="updaterStore.status"
 *     :percent="updaterStore.progressPercent"
 *     :error-message="updaterStore.errorMessage"
 *     :downloaded-bytes="updaterStore.downloadedBytes"
 *     :total-bytes="updaterStore.totalBytes"
 *   />
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import type { UpdaterStatus } from '../types';

interface Props {
  status: UpdaterStatus;
  percent: number;
  errorMessage?: string;
  downloadedBytes?: number;
  totalBytes?: number;
}

const props = withDefaults(defineProps<Props>(), {
  errorMessage: '',
  downloadedBytes: 0,
  totalBytes: 0
});

const { t } = useI18n();

/** 格式化字节数为人类可读的文本（B / KB / MB / GB） */
function formatBytes(bytes: number): string {
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  const val = bytes / Math.pow(1024, i);
  return `${val.toFixed(i === 0 ? 0 : 2)} ${units[i]}`;
}

const progressStatus = computed<'success' | 'warning' | 'exception' | undefined>(() => {
  switch (props.status) {
    case 'done':
      return 'success';
    case 'error':
      return 'exception';
    default:
      return undefined;
  }
});

const statusText = computed(() => {
  switch (props.status) {
    case 'downloading':
      return t('Updater.downloading');
    case 'installing':
      return t('Updater.installingUpdate');
    case 'error':
      return props.errorMessage || t('Updater.downloadError');
    default:
      return '';
  }
});

const showProgress = computed(() =>
  props.status === 'downloading' || props.status === 'installing'
);

const showBytes = computed(() =>
  props.status === 'downloading' && props.totalBytes > 0
);
</script>

<template>
  <div class="update-progress">
    <div v-if="showProgress" class="phase-row">
      <span class="phase-label">{{ statusText }}</span>
      <span v-if="showBytes" class="phase-sub">
        {{ formatBytes(props.downloadedBytes) }} / {{ formatBytes(props.totalBytes) }}
      </span>
      <span v-else-if="props.status === 'installing'" class="phase-sub">
        {{ percent }}%
      </span>
    </div>

    <el-progress
      v-if="showProgress"
      :percentage="percent"
      :status="progressStatus"
      :stroke-width="10"
      :show-text="false"
    />

    <div v-if="props.status === 'error' && errorMessage" class="error-msg">
      {{ errorMessage }}
    </div>
  </div>
</template>

<style scoped>
.update-progress {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 8px 4px;
}

.phase-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  font-size: 13px;
  color: var(--text-regular, #606266);
}

.phase-label {
  font-weight: 600;
  color: var(--text-primary, #303133);
}

.phase-sub {
  font-variant-numeric: tabular-nums;
}

.error-msg {
  margin-top: 4px;
  padding: 8px 10px;
  border-radius: 4px;
  background: var(--danger-color-light-9, #fef0f0);
  color: var(--danger-color, #f56c6c);
  font-size: 13px;
  white-space: pre-wrap;
  word-break: break-word;
}
</style>
