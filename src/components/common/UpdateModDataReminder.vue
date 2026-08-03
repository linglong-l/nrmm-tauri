<template>
  <Transition name="reminder-fade">
    <div v-if="modsStore.needUpdate" class="update-mod-reminder">
      <!-- 提示文本 -->
      <span class="reminder-text">{{ t('settings.dontForgetUpdate') }}</span>
      <!-- 操作按钮区 -->
      <div class="reminder-actions">
        <!-- 更新按钮：触发重量级模组数据更新 -->
        <el-button type="primary" class="reminder-btn" @click="handleClick" :loading="loading">
          {{ t('settings.updateModData') }}
        </el-button>
        <!-- 关闭并重载按钮：仅在需要手动重载时显示 -->
        <el-button v-if="modsStore.needReloadManual" class="reminder-btn" @click="handleCloseAndReload">
          {{ t('settings.closeAndReload') }}
        </el-button>
        <!-- 关闭按钮 -->
        <el-button class="reminder-btn" @click="handleClose">
          {{ t('settings.close') }}
        </el-button>
      </div>
    </div>
  </Transition>
</template>

<script setup lang="ts">
/**
 * 更新模组数据提示组件
 * 固定在模组页面底部，提醒用户执行重量级更新操作
 *
 * 显示逻辑：
 * - 仅在Mods页面显示
 * - 非互斥组（普通group_int分组）模组启用/禁用操作后显示
 * - 按游戏独立维护显示状态
 * - 点击"更新模组数据"成功后自动隐藏
 */
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { ElMessage } from 'element-plus'
import { useSettingsStore } from '@/stores/settings'
import { useModsStore } from '@/stores/mods'
import { simulateF10 } from '@/utils/tauri'
import { logger } from '@/utils/logger'

const { t } = useI18n()
const settingsStore = useSettingsStore()
const modsStore = useModsStore()

/** 按钮加载状态 */
const loading = ref(false)

/**
 * 点击更新按钮处理
 * 调用modsStore.updateModData()执行重量级更新：
 * - 解析所有INI文件完整内容
 * - 检测并修复namespace冲突
 * - 处理missingEndif等错误
 * - 重新计算modDisabled状态
 * 更新成功后modsStore.updateModData内部会自动清除needUpdate状态
 */
async function handleClick() {
  loading.value = true
  try {
    if (!settingsStore.currentModsPath) {
      ElMessage.warning(t('Mods path does not exist.'))
      return
    }
    await modsStore.updateModData()
    ElMessage.success(t('Update Mod Data completed successfully!'))
  } catch (e: any) {
    logger.error('UpdateModDataReminder', 'Failed to update mod data', e)
    ElMessage.error(t('Unknown error occurred.'))
  } finally {
    loading.value = false
  }
}

/**
 * 关闭并重载按钮处理
 * 调用 simulateF10() 模拟 F10 按键（3Dmigoto 重载），然后清除提醒状态
 */
async function handleCloseAndReload() {
  try {
    await simulateF10(settingsStore.currentGame ?? undefined)
  } catch (e: any) {
    logger.error('UpdateModDataReminder', 'Failed to simulate F10', e)
  }
  modsStore.clearNeedUpdate()
}

/**
 * 关闭按钮处理
 * 清除提醒状态，隐藏提示条
 */
function handleClose() {
  modsStore.clearNeedUpdate()
}
</script>

<style scoped>
.update-mod-reminder {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  z-index: 50;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 20px;
  background: rgba(60, 60, 70, 0.72);
  border-top: 1px solid rgba(255, 255, 255, 0.08);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
}

.reminder-text {
  color: var(--text-primary);
  font-size: 13px;
  font-weight: 500;
  white-space: nowrap;
}

.reminder-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.reminder-btn {
  padding: 8px 18px;
  font-size: 13px;
  font-weight: 600;
}

/* 淡入淡出过渡动画 */
.reminder-fade-enter-active,
.reminder-fade-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}

.reminder-fade-enter-from,
.reminder-fade-leave-to {
  opacity: 0;
  transform: translateY(100%);
}
</style>
