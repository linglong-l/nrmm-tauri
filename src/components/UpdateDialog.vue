<script setup lang="ts">
/**
 * UpdateDialog.vue
 *
 * 应用自更新主对话框组件（基于 tauri-plugin-updater）。
 *
 * 8 态状态机展示：
 *  - idle: 尚未检查
 *  - checking: 正在检查
 *  - up-to-date: 已是最新
 *  - available: 有可用更新
 *  - downloading: 正在下载
 *  - installing: 正在安装
 *  - done: 安装完成（等待重启）
 *  - error: 出错
 *
 * 用法（在父组件中通过 v-model 控制显隐）：
 *   <UpdateDialog v-model="showUpdateDialog" />
 */
import { computed, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useUpdaterStore } from '../stores/updater';
import { useVersionStore } from '../stores/version';
import UpdateProgress from './UpdateProgress.vue';

interface Props {
  modelValue: boolean;
}

const props = defineProps<Props>();

const emit = defineEmits<{
  (e: 'update:modelValue', v: boolean): void;
}>();

const { t } = useI18n();
const updaterStore = useUpdaterStore();
const versionStore = useVersionStore();

function close() {
  emit('update:modelValue', false);
}

watch(
  () => props.modelValue,
  (visible) => {
    if (visible) {
      if (updaterStore.status === 'done') {
        return;
      }
      if (!updaterStore.isBusy && updaterStore.status !== 'available') {
        updaterStore.reset();
      }
    }
  }
);

async function handleCheck() {
  await updaterStore.check();
}

async function handleUpdate() {
  await updaterStore.confirmInstall();
}

async function handleRestart() {
  await updaterStore.restartApp();
}

function handleDismiss() {
  updaterStore.dismiss();
}

function formatDate(iso: string): string {
  if (!iso) return '-';
  try {
    const d = new Date(iso);
    if (isNaN(d.getTime())) return iso;
    return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
  } catch (_e) {
    return iso;
  }
}

const dialogTitle = computed(() => {
  if (updaterStore.isChecking) return t('Updater.titleChecking');
  return t('Updater.title');
});

const showCheckBtn = computed(() => {
  return !updaterStore.isBusy && updaterStore.status !== 'done';
});

const showUpdateBtn = computed(() => {
  return updaterStore.status === 'available';
});

const showRestartBtn = computed(() => {
  return updaterStore.status === 'done';
});

const showCloseBtn = computed(() => {
  return !updaterStore.isBusy && updaterStore.status !== 'done';
});
</script>

<template>
  <el-dialog
    :model-value="modelValue"
    :title="dialogTitle"
    width="560px"
    class="update-dialog"
    :close-on-click-modal="false"
    @update:model-value="(v: boolean) => emit('update:modelValue', v)"
  >
    <!-- idle: 尚未检查 -->
    <div v-if="updaterStore.status === 'idle'" class="empty-state">
      <el-icon :size="40" color="#909399"><Tools /></el-icon>
      <div class="empty-title">{{ t('Updater.title') }}</div>
      <div class="empty-sub">{{ t('Updater.currentVersion') }}: v{{ versionStore.version }}</div>
    </div>

    <!-- checking: 正在检查 -->
    <div v-else-if="updaterStore.status === 'checking'" class="empty-state">
      <el-icon class="is-loading" :size="40"><Loading /></el-icon>
      <div class="empty-title" style="margin-top: 12px">{{ t('Updater.checking') }}</div>
    </div>

    <!-- up-to-date: 已是最新 -->
    <div v-else-if="updaterStore.status === 'up-to-date'" class="empty-state">
      <el-icon :size="48" color="#67c23a"><CircleCheckFilled /></el-icon>
      <div class="empty-title">{{ t('Updater.upToDate') }}</div>
      <div class="empty-sub">{{ t('Updater.currentVersion') }}: v{{ versionStore.version }}</div>
    </div>

    <!-- available: 有可用更新 -->
    <div v-else-if="updaterStore.status === 'available' && updaterStore.updateInfo" class="version-card">
      <div class="version-row">
        <div class="version-main">
          <span class="version-label">{{ t('Updater.latestVersion') }}</span>
          <span class="version-num">v{{ updaterStore.updateInfo.version }}</span>
        </div>
        <div class="version-date">{{ formatDate(updaterStore.updateInfo.date) }}</div>
      </div>

      <div class="version-current">
        {{ t('Updater.currentVersion') }}: v{{ versionStore.version }}
      </div>

      <div class="notes-wrap" v-if="updaterStore.updateInfo.body">
        <div class="notes-label">{{ t('Updater.releaseNotes') }}</div>
        <div class="notes-body">
          <pre>{{ updaterStore.updateInfo.body }}</pre>
        </div>
      </div>
    </div>

    <!-- downloading / installing: 进度展示 -->
    <div v-else-if="updaterStore.status === 'downloading' || updaterStore.status === 'installing'" class="progress-container">
      <div v-if="updaterStore.status === 'installing'" class="installing-info">
        <el-icon class="is-loading" :size="32"><Loading /></el-icon>
        <div class="installing-text">{{ t('Updater.installingUpdate') }}</div>
      </div>
      <UpdateProgress
        v-else
        :status="updaterStore.status"
        :percent="updaterStore.progressPercent"
        :error-message="updaterStore.errorMessage"
        :downloaded-bytes="updaterStore.downloadedBytes"
        :total-bytes="updaterStore.totalBytes"
      />
    </div>

    <!-- done: 安装完成 -->
    <div v-else-if="updaterStore.status === 'done'" class="empty-state">
      <el-icon :size="48" color="#67c23a"><CircleCheckFilled /></el-icon>
      <div class="empty-title">{{ t('Updater.updateReady') }}</div>
      <div class="empty-sub">{{ t('Updater.installComplete') }}</div>
    </div>

    <!-- error: 出错 -->
    <div v-else-if="updaterStore.status === 'error'" class="empty-state">
      <el-icon :size="48" color="#f56c6c"><CircleCloseFilled /></el-icon>
      <div class="empty-title">{{ t('Updater.checkFailed') }}</div>
      <div class="empty-sub error-text">{{ updaterStore.errorMessage }}</div>
    </div>

    <!-- 底部按钮区 -->
    <template #footer>
      <div class="dialog-footer">
        <el-button v-if="showCheckBtn" :loading="updaterStore.isChecking" @click="handleCheck">
          {{ t('Updater.checkNow') }}
        </el-button>

        <el-button
          v-if="showUpdateBtn"
          type="primary"
          :loading="updaterStore.isDownloading || updaterStore.isInstalling"
          @click="handleUpdate"
        >
          {{ t('Updater.updateNow') }}
        </el-button>

        <el-button
          v-if="showRestartBtn"
          type="success"
          @click="handleRestart"
        >
          {{ t('Updater.restartNow') }}
        </el-button>

        <el-button v-if="updaterStore.status === 'up-to-date' || updaterStore.status === 'error'" @click="handleDismiss">
          {{ t('Common.ok') }}
        </el-button>

        <el-button v-if="showCloseBtn" @click="close">{{ t('Common.close') }}</el-button>
      </div>
    </template>
  </el-dialog>
</template>

<script lang="ts">
import {
  CircleCheckFilled,
  CircleCloseFilled,
  Loading,
  Tools
} from '@element-plus/icons-vue';

export default {
  components: {
    CircleCheckFilled,
    CircleCloseFilled,
    Loading,
    Tools
  }
};
</script>

<style scoped>
.update-dialog :deep(.el-dialog__body) {
  padding: 12px 20px 4px;
}

.version-card {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin-bottom: 8px;
}

.version-row {
  display: flex;
  justify-content: space-between;
  align-items: flex-end;
}

.version-main {
  display: flex;
  align-items: baseline;
}

.version-label {
  font-size: 12px;
  color: var(--text-secondary, #909399);
  margin-right: 8px;
}

.version-num {
  font-size: 22px;
  font-weight: 700;
  color: var(--text-primary, #303133);
}

.version-date {
  font-size: 12px;
  color: var(--text-secondary, #909399);
}

.version-current {
  font-size: 13px;
  color: var(--text-regular, #606266);
}

.notes-wrap {
  border: 1px solid var(--border-color-lighter, #ebeef5);
  border-radius: 6px;
  overflow: hidden;
}

.notes-label {
  padding: 6px 10px;
  background: var(--fill-color-light, #f5f7fa);
  border-bottom: 1px solid var(--border-color-lighter, #ebeef5);
  font-size: 12px;
  color: var(--text-regular, #606266);
  font-weight: 600;
}

.notes-body {
  max-height: 220px;
  overflow: auto;
  padding: 10px 12px;
  font-size: 13px;
  line-height: 1.6;
  color: var(--text-primary, #303133);
}

.notes-body pre {
  margin: 0;
  font-family: inherit;
  white-space: pre-wrap;
  word-break: break-word;
}

.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 36px 12px 24px;
  gap: 6px;
  color: var(--text-regular, #606266);
}

.empty-title {
  font-size: 16px;
  font-weight: 600;
  color: var(--text-primary, #303133);
}

.empty-sub {
  font-size: 13px;
  color: var(--text-secondary, #909399);
}

.error-text {
  color: var(--danger-color, #f56c6c);
  max-width: 100%;
  word-break: break-word;
  text-align: center;
}

.progress-container {
  padding: 8px 0;
}

.installing-info {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 24px 0;
}

.installing-text {
  font-size: 14px;
  color: var(--text-regular, #606266);
}

.dialog-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  flex-wrap: wrap;
}
</style>
