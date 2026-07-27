<template>
  <el-dialog
    :model-value="modelValue"
    :title="title"
    width="420px"
    :close-on-click-modal="false"
    @update:model-value="handleClose"
    class="dialog-box"
    append-to-body
  >
    <div class="dialog-content">
      <slot>{{ content }}</slot>
    </div>
    <template #footer>
      <div class="dialog-footer">
        <el-button v-if="showCancel" @click="handleCancel">
          {{ cancelText || t('common.cancel') }}
        </el-button>
        <el-button type="primary" @click="handleConfirm">
          {{ confirmText || t('common.confirm') }}
        </el-button>
      </div>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { useI18n } from 'vue-i18n'

const { t } = useI18n()

interface Props {
  modelValue: boolean
  title: string
  content?: string
  showCancel?: boolean
  confirmText?: string
  cancelText?: string
}

withDefaults(defineProps<Props>(), {
  content: '',
  showCancel: true,
  confirmText: '',
  cancelText: '',
})

const emit = defineEmits<{
  (e: 'update:modelValue', value: boolean): void
  (e: 'confirm'): void
  (e: 'cancel'): void
}>()

function handleClose(value: boolean) {
  emit('update:modelValue', value)
}

function handleConfirm() {
  emit('confirm')
  emit('update:modelValue', false)
}

function handleCancel() {
  emit('cancel')
  emit('update:modelValue', false)
}
</script>

<style scoped>
.dialog-content {
  color: var(--text-primary);
  font-size: 14px;
  line-height: 1.6;
  padding: 8px 0;
}

.dialog-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
</style>

<style>
.dialog-box {
  border-radius: var(--border-radius);
  border: 1px solid var(--border-color);
}

.dialog-box .el-dialog__header {
  margin-right: 0;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border-color);
}

.dialog-box .el-dialog__title {
  color: var(--text-primary);
  font-weight: 600;
  font-size: 15px;
}

.dialog-box .el-dialog__body {
  padding: 16px 20px;
}

.dialog-box .el-dialog__footer {
  padding: 12px 20px;
  border-top: 1px solid var(--border-color);
}

.dialog-box .el-dialog__headerbtn .el-dialog__close {
  color: var(--text-muted);
}

.dialog-box .el-dialog__headerbtn:hover .el-dialog__close {
  color: var(--text-primary);
}
</style>
