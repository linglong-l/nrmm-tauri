<template>
  <Transition name="reminder-fade">
    <div v-if="modsStore.needUpdate" class="update-mod-reminder">
      <!-- 提示文本 -->
      <span class="reminder-text">{{ t('settings.dontForgetUpdate') }}</span>
      <!-- 操作按钮区：仅显示"更新模组数据"按钮 -->
      <div class="reminder-actions">
        <el-button type="primary" class="reminder-btn" @click="handleClick">
          {{ t('settings.updateModData') }}
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
import { inject } from 'vue'
import { useI18n } from 'vue-i18n'
import { ElMessage } from 'element-plus'
import { useSettingsStore } from '@/stores/settings'
import { useModsStore } from '@/stores/mods'
import { updateModData as tauriUpdateModData } from '@/utils/tauri'
import { logger } from '@/utils/logger'

const { t } = useI18n()
const settingsStore = useSettingsStore()
const modsStore = useModsStore()
/** 复用 App.vue 提供的更新遮罩控制（与设置页 handleUpdateModData 相同的遮罩） */
const updateOverlay: any = inject('updateOverlay')

/**
 * 点击"更新模组数据"按钮
 * 与设置页 handleUpdateModData 完全相同的逻辑与遮罩：
 * - 显示 loading 遮罩
 * - 调用 modsStore.updateModData() 执行重量级更新
 * - 完成后显示 completed 遮罩（含 XXMI 检测提示、耗时统计）
 * - 失败显示 error 遮罩
 */
async function handleClick() {
  try {
    if (!settingsStore.currentModsPath) {
      ElMessage.warning(t('Mods path does not exist.'))
      return
    }
    updateOverlay?.show('loading')
    const start = Date.now()
    try {
      const result = await modsStore.updateModData() ?? (await tauriUpdateModData(settingsStore.currentGame, settingsStore.currentModsPath))
      updateOverlay?.show('completed', { result, durationMs: Date.now() - start })
    } catch (e: any) {
      const msg = typeof e === 'string' ? e : (e?.message ?? String(e))
      updateOverlay?.show('error', { error: msg })
    }
  } catch (e: any) {
    logger.error('UpdateModDataReminder', 'Update mod data failed', e)
  }
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
  border-radius: 999px;
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
