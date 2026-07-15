<script setup lang="ts">
/**
 * HashConflictFab.vue - 右下角浮动 Hash 冲突状态入口组件
 *
 * 作用：
 *  - 在主窗口右下角始终显示一个圆形按钮（FAB），实时反映 hash 冲突检测状态。
 *  - 根据状态显示不同 UI：
 *    - checking：旋转 Loading 图标 + "正在检测..."
 *    - conflicts：Warning 图标 + 红色徽标 + 可点击弹出 Popover 详情
 *    - done（无冲突）：Success 图标 + "无hash冲突"
 *    - idle/error：Info 图标 + 状态提示
 *
 * 定位策略：
 *  - position: fixed; right: 24px; bottom: 24px;
 *  - z-index: 9999;
 */
import { ref, computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { WarningFilled, Loading, SuccessFilled, InfoFilled } from '@element-plus/icons-vue';
import { useHashConflictStore } from '../stores/hashConflict';
import { useGameStore } from '../stores/game';
import type { HashConflictEntry } from '../types';

const { t } = useI18n();
const hashConflictStore = useHashConflictStore();
const gameStore = useGameStore();

const popoverVisible = ref(false);

const isChecking = computed(() => hashConflictStore.status === 'checking');
const hasConflicts = computed(() => hashConflictStore.hasConflicts);
const isDone = computed(() => hashConflictStore.status === 'done' && !hashConflictStore.hasConflicts);
const isError = computed(() => hashConflictStore.status === 'error');

/** FAB 当前状态：checking | conflicts | done | idle */
const fabState = computed<'checking' | 'conflicts' | 'done' | 'idle'>(() => {
  if (isChecking.value) return 'checking';
  if (hasConflicts.value) return 'conflicts';
  if (isDone.value) return 'done';
  return 'idle';
});

/** 状态标签文字 */
const statusLabel = computed(() => {
  switch (fabState.value) {
    case 'checking': return t('Checking for hash conflicts...');
    case 'conflicts': return '';
    case 'done': return t('No hash conflicts');
    case 'idle': return isError.value ? t('Hash conflict check failed') : '';
  }
});

/** 按钮图标 */
const fabIcon = computed(() => {
  switch (fabState.value) {
    case 'checking': return Loading;
    case 'conflicts': return WarningFilled;
    case 'done': return SuccessFilled;
    case 'idle': return InfoFilled;
  }
});

/** 按钮颜色类型 */
const fabType = computed<'warning' | 'success' | 'info'>(() => {
  switch (fabState.value) {
    case 'checking': return 'warning';
    case 'conflicts': return 'warning';
    case 'done': return 'success';
    case 'idle': return 'info';
  }
});

const visibleCount = computed(() => hashConflictStore.conflictCount);

const popoverTitle = computed(() => {
  return t('Hash conflict detected: {count} group(s)', { count: visibleCount.value });
});

function formatConflictMessage(entry: HashConflictEntry): string {
  const shortHash = entry.hash.slice(0, 8);
  if (entry.modNames.length === 2) {
    return t('{mod1} and {mod2} conflict, hash: {hash}', {
      mod1: entry.modNames[0],
      mod2: entry.modNames[1],
      hash: shortHash
    });
  }
  if (entry.modNames.length > 2) {
    const lastMod = entry.modNames[entry.modNames.length - 1];
    const firstMods = entry.modNames.slice(0, -1).join('、');
    return t('{mods} and {lastMod} conflict, hash: {hash}', {
      mods: firstMods,
      lastMod,
      hash: shortHash
    });
  }
  return entry.modNames[0] || '';
}

function handleViewLocation(entry: HashConflictEntry) {
  if (entry.modPaths.length === 0) return;
  const modPath = entry.modPaths[0];
  const pathSep = modPath.includes('\\') ? '\\' : '/';
  const parts = modPath.split(pathSep);
  if (parts.length >= 2) {
    const groupPath = parts.slice(0, -1).join(pathSep);
    if (typeof gameStore.setCurrentGroupByPath === 'function') {
      gameStore.setCurrentGroupByPath(groupPath);
    }
  }
  popoverVisible.value = false;
}

function handleIgnore(entry: HashConflictEntry) {
  hashConflictStore.ignoreHash(entry.hash);
}

function handleClose() {
  popoverVisible.value = false;
}

/** 仅在存在冲突时打开 Popover，其他状态不响应点击 */
function handleFabClick() {
  if (fabState.value === 'conflicts') {
    popoverVisible.value = !popoverVisible.value;
  }
}
</script>

<template>
  <div class="hash-conflict-fab">
    <el-popover
      v-model:visible="popoverVisible"
      placement="top"
      :width="360"
      :show-arrow="true"
      trigger="click"
      popper-class="hash-conflict-popover"
      :disabled="fabState !== 'conflicts'"
    >
      <template #reference>
        <div class="hash-conflict-fab__trigger" @click.stop="handleFabClick">
          <el-badge
            :value="visibleCount"
            :hidden="isChecking || fabState !== 'conflicts'"
            :max="99"
            type="danger"
          >
            <el-button
              circle
              :type="fabType"
              size="large"
              class="hash-conflict-fab__button"
              :class="{ 'is-loading-spin': isChecking }"
              :aria-label="statusLabel"
            >
              <el-icon :class="{ 'is-loading': isChecking }">
                <component :is="fabIcon" />
              </el-icon>
            </el-button>
          </el-badge>
          <span v-if="statusLabel" class="hash-conflict-fab__label">
            {{ statusLabel }}
          </span>
        </div>
      </template>

      <!-- 弹层内容（仅 conflicts 状态可访问） -->
      <div class="hash-conflict-popover__content">
        <div class="hash-conflict-popover__title">
          {{ popoverTitle }}
        </div>

        <div class="hash-conflict-popover__list">
          <div
            v-for="(entry, index) in hashConflictStore.visibleConflicts"
            :key="entry.hash"
            class="hash-conflict-item"
            :class="{ 'is-last': index === hashConflictStore.visibleConflicts.length - 1 }"
          >
            <div class="hash-conflict-item__message">
              {{ formatConflictMessage(entry) }}
            </div>
            <div class="hash-conflict-item__actions">
              <el-button size="small" link type="primary" @click="handleViewLocation(entry)">
                {{ t('View location') }}
              </el-button>
              <el-button size="small" link type="info" @click="handleIgnore(entry)">
                {{ t('Ignore') }}
              </el-button>
            </div>
          </div>
        </div>

        <div class="hash-conflict-popover__suggestion">
          <div class="hash-conflict-popover__suggestion-title">
            {{ t('Conflict resolution suggestion') }}
          </div>
          <div class="hash-conflict-popover__suggestion-text">
            {{ t('Conflict resolution suggestion text') }}
          </div>
        </div>

        <div class="hash-conflict-popover__footer">
          <el-button size="small" @click="handleClose">
            {{ t('Close') }}
          </el-button>
        </div>
      </div>
    </el-popover>
  </div>
</template>

<style scoped>
.hash-conflict-fab {
  position: fixed;
  right: 24px;
  bottom: 24px;
  z-index: 9999;
}

.hash-conflict-fab__trigger {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  cursor: pointer;
}

.hash-conflict-fab__button {
  width: 48px;
  height: 48px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
  transition: transform 0.2s ease, box-shadow 0.2s ease;
}

.hash-conflict-fab__button:hover {
  transform: translateY(-2px);
  box-shadow: 0 6px 16px rgba(0, 0, 0, 0.2);
}

.hash-conflict-fab__label {
  font-size: 11px;
  color: rgba(255, 255, 255, 0.45);
  text-align: center;
  max-width: 72px;
  line-height: 1.3;
  word-break: break-word;
}

.hash-conflict-popover__content {
  max-height: 480px;
  display: flex;
  flex-direction: column;
}

.hash-conflict-popover__title {
  font-size: 14px;
  font-weight: 600;
  margin-bottom: 12px;
  color: var(--el-color-danger);
}

.hash-conflict-popover__list {
  flex: 1;
  overflow-y: auto;
  max-height: 280px;
}

.hash-conflict-item {
  padding: 10px 0;
  border-bottom: 1px solid var(--el-border-color-lighter);
}

.hash-conflict-item.is-last {
  border-bottom: none;
}

.hash-conflict-item__message {
  font-size: 13px;
  line-height: 1.5;
  color: var(--el-text-color-primary);
  margin-bottom: 6px;
  word-break: break-word;
}

.hash-conflict-item__actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}

.hash-conflict-popover__suggestion {
  margin-top: 12px;
  padding: 10px;
  background-color: var(--el-color-warning-light-9);
  border-radius: 4px;
}

.hash-conflict-popover__suggestion-title {
  font-size: 12px;
  font-weight: 600;
  margin-bottom: 4px;
  color: var(--el-color-warning-dark-2);
}

.hash-conflict-popover__suggestion-text {
  font-size: 12px;
  line-height: 1.5;
  color: var(--el-text-color-regular);
}

.hash-conflict-popover__footer {
  margin-top: 12px;
  display: flex;
  justify-content: flex-end;
  border-top: 1px solid var(--el-border-color-lighter);
  padding-top: 10px;
}

.is-loading {
  animation: rotating 2s linear infinite;
}

.is-loading-spin .el-icon {
  animation: rotating 2s linear infinite;
}

@keyframes rotating {
  0% { transform: rotate(0deg); }
  100% { transform: rotate(360deg); }
}
</style>