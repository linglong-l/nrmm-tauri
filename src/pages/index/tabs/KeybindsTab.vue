<script setup lang="ts">
/**
 * KeybindsTab.vue - 热键标签页组件
 *
 * 作用：
 *  - 展示当前在 Mods 标签页中选中的模组所包含的所有 [Key*] 段。
 *  - 自动扫描选中模组目录下的所有 .ini 文件，解析并聚合其中的快捷键绑定。
 *  - 支持点击按键绑定进入编辑模式，捕获键盘输入修改按键值。
 *  - 支持启用/禁用切换开关，禁用的按键以分号注释。
 *  - 无选中模组或未找到快捷键时给出友好空状态提示。
 */
import { ref, computed, watch, onUnmounted } from 'vue';
import { useI18n } from 'vue-i18n';
import { ElMessage } from 'element-plus';
import { useGameStore } from '../../../stores/game';
import {
  invokeFindModIniFiles,
  invokeLoadIniData,
  invokeSimulateKeyPress,
  invokeSimulateKeyCombination,
  invokeSaveKeybind,
  invokeToggleKeybindEnabled,
} from '../../../utils/invoke';
import type { IniFileData, IniSectionData } from '../../../types';

const { t } = useI18n();
const gameStore = useGameStore();

const loading = ref(false);
const iniFiles = ref<IniFileData[]>([]);
const error = ref<string | null>(null);

interface KeyEntry {
  /** key= 行在该段所有 key= 行中的索引（0-based，含被注释的行） */
  keyIndex: number;
  /** 按键值（如 "VK_F1"、"VK_CONTROL + VK_F2"） */
  value: string;
  /** 是否被禁用（行以 ; 开头注释） */
  disabled: boolean;
  /** 原始行内容 */
  rawLine: string;
}

interface KeybindEntry {
  filePath: string;
  sectionName: string;
  keys: KeyEntry[];
  back: string | null;
  condition: string | null;
  type: string | null;
}

interface EditingState {
  bindIndex: number;
  keyIndex: number;
  currentValue: string;
  pressedKeys: Set<string>;
  combinationKeys: string[];
  finalized: boolean;
}

const editing = ref<EditingState | null>(null);

function isCommentedLine(line: string): boolean {
  const trimmed = line.trimStart();
  return trimmed.startsWith(';') && !trimmed.startsWith(';+;') && !trimmed.startsWith(';-;');
}

function isKeyValueLine(line: string, keyName: string): boolean {
  const trimmed = line.trimStart();
  const check = isCommentedLine(line) ? trimmed.substring(1).trimStart() : trimmed;
  const lower = check.toLowerCase();
  const keyLower = keyName.toLowerCase();
  if (!lower.startsWith(keyLower)) return false;
  const after = lower.substring(keyLower.length).trimStart();
  return after.startsWith('=');
}

function parseKeyValueFromLine(line: string): string | null {
  const trimmed = line.trim();
  const check = isCommentedLine(line) ? trimmed.substring(1).trimStart() : trimmed;
  const eqPos = check.indexOf('=');
  if (eqPos < 0) return null;
  return check.substring(eqPos + 1).trim();
}

function extractKeybindsFromSection(filePath: string, section: IniSectionData): KeybindEntry {
  const keyEntries: KeyEntry[] = [];
  let back: string | null = null;
  let condition: string | null = null;
  let type: string | null = null;
  let keyIndex = 0;

  for (const rawLine of section.rawLines) {
    if (isKeyValueLine(rawLine, 'key')) {
      const value = parseKeyValueFromLine(rawLine) ?? '';
      keyEntries.push({
        keyIndex,
        value,
        disabled: isCommentedLine(rawLine),
        rawLine,
      });
      keyIndex++;
    } else if (!isCommentedLine(rawLine)) {
      if (isKeyValueLine(rawLine, 'back')) {
        back = parseKeyValueFromLine(rawLine);
      } else if (isKeyValueLine(rawLine, 'condition')) {
        condition = parseKeyValueFromLine(rawLine);
      } else if (isKeyValueLine(rawLine, 'type')) {
        type = parseKeyValueFromLine(rawLine);
      }
    }
  }

  return {
    filePath,
    sectionName: section.name,
    keys: keyEntries,
    back,
    condition,
    type,
  };
}

const keybinds = computed<KeybindEntry[]>(() => {
  const result: KeybindEntry[] = [];
  for (const file of iniFiles.value) {
    for (const section of file.sections) {
      if (!section.name.toLowerCase().startsWith('key')) continue;
      const entry = extractKeybindsFromSection(file.path, section);
      if (entry.keys.length > 0) {
        result.push(entry);
      }
    }
  }
  return result;
});

function isPathUnderHashDir(path: string): boolean {
  const normalized = path.replace(/\\/g, '/');
  return normalized.split('/').some(segment => {
    const stripped = segment.startsWith('DISABLED')
      ? segment.replace(/^DISABLED_?/, '')
      : segment;
    return stripped.startsWith('#');
  });
}

const selectedModPath = computed(() => {
  if (!gameStore.currentGroupPath) return null;
  return gameStore.getSelectedModPath(gameStore.currentGroupPath);
});

async function loadKeybinds(path: string | null) {
  iniFiles.value = [];
  error.value = null;
  cancelEdit();
  if (!path) return;

  if (isPathUnderHashDir(path)) {
    error.value = '# 目录下的模组不支持快捷键查看';
    return;
  }

  loading.value = true;
  try {
    const iniPaths = await invokeFindModIniFiles(path);
    iniFiles.value = await Promise.all(iniPaths.map((p) => invokeLoadIniData(p)));
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

async function refreshCurrentKeybinds() {
  const path = selectedModPath.value;
  if (!path) return;
  if (isPathUnderHashDir(path)) return;
  try {
    const iniPaths = await invokeFindModIniFiles(path);
    iniFiles.value = await Promise.all(iniPaths.map((p) => invokeLoadIniData(p)));
  } catch (e) {
    error.value = String(e);
  }
}

function isEditing(bindIndex: number, keyIndex: number): boolean {
  return editing.value?.bindIndex === bindIndex && editing.value.keyIndex === keyIndex;
}

function startEdit(bindIndex: number, keyIdx: number) {
  const kb = keybinds.value[bindIndex];
  if (!kb) return;
  const keyEntry = kb.keys[keyIdx];
  if (!keyEntry) return;
  editing.value = {
    bindIndex,
    keyIndex: keyIdx,
    currentValue: t('keybinds.pressKey'),
    pressedKeys: new Set(),
    combinationKeys: [],
    finalized: false,
  };
  window.addEventListener('keydown', onWindowKeydown, true);
  window.addEventListener('keyup', onWindowKeyup, true);
}

function cancelEdit() {
  if (editing.value) {
    window.removeEventListener('keydown', onWindowKeydown, true);
    window.removeEventListener('keyup', onWindowKeyup, true);
  }
  editing.value = null;
}

async function handleKeybindClick(_bindIndex: number, kb: KeybindEntry, keyIdx: number) {
  if (editing.value) return;
  const keyEntry = kb.keys[keyIdx];
  if (!keyEntry) return;
  if (keyEntry.disabled) return;
  try {
    if (kb.keys.filter(k => !k.disabled).length === 1) {
      await invokeSimulateKeyPress(keyEntry.value);
    } else {
      await invokeSimulateKeyCombination(kb.keys.filter(k => !k.disabled).map(k => k.value));
    }
  } catch (e) {
    ElMessage.error(String(e));
  }
}

const MODIFIER_KEYS = new Set(['Control', 'Alt', 'Shift']);
const MODIFIER_VK: Record<string, string> = {
  Control: 'VK_CONTROL',
  Alt: 'VK_MENU',
  Shift: 'VK_SHIFT',
};

function eventToVkCode(e: KeyboardEvent): string | null {
  const key = e.key;
  if (MODIFIER_KEYS.has(key)) return null;
  if (key === 'Escape') return null;

  if (key === 'F1') return 'VK_F1';
  if (key === 'F2') return 'VK_F2';
  if (key === 'F3') return 'VK_F3';
  if (key === 'F4') return 'VK_F4';
  if (key === 'F5') return 'VK_F5';
  if (key === 'F6') return 'VK_F6';
  if (key === 'F7') return 'VK_F7';
  if (key === 'F8') return 'VK_F8';
  if (key === 'F9') return 'VK_F9';
  if (key === 'F10') return 'VK_F10';
  if (key === 'F11') return 'VK_F11';
  if (key === 'F12') return 'VK_F12';
  if (key === 'Enter' || key === 'Return') return 'VK_RETURN';
  if (key === ' ') return 'VK_SPACE';
  if (key === 'Tab') return 'VK_TAB';
  if (key === 'Escape') return 'VK_ESCAPE';
  if (key === 'Backspace') return 'VK_BACK';
  if (key === 'Delete') return 'VK_DELETE';
  if (key === 'Insert') return 'VK_INSERT';
  if (key === 'Home') return 'VK_HOME';
  if (key === 'End') return 'VK_END';
  if (key === 'PageUp') return 'VK_PRIOR';
  if (key === 'PageDown') return 'VK_NEXT';
  if (key === 'ArrowLeft') return 'VK_LEFT';
  if (key === 'ArrowRight') return 'VK_RIGHT';
  if (key === 'ArrowUp') return 'VK_UP';
  if (key === 'ArrowDown') return 'VK_DOWN';
  if (key === 'CapsLock') return 'VK_CAPITAL';
  if (key === 'NumLock') return 'VK_NUMLOCK';
  if (key === 'ScrollLock') return 'VK_SCROLL';
  if (key === 'PrintScreen') return 'VK_SNAPSHOT';
  if (key === 'Pause') return 'VK_PAUSE';
  if (key === 'ContextMenu') return 'VK_APPS';

  if (key.length === 1) {
    const code = key.toUpperCase().charCodeAt(0);
    if (code >= 48 && code <= 57) {
      return `VK_${key}`;
    }
    if (code >= 65 && code <= 90) {
      return `VK_${key.toUpperCase()}`;
    }
  }

  if (e.code.startsWith('Numpad')) {
    const num = e.code.replace('Numpad', '');
    if (num >= '0' && num <= '9') return `VK_NUMPAD${num}`;
    if (num === 'Add') return 'VK_ADD';
    if (num === 'Subtract') return 'VK_SUBTRACT';
    if (num === 'Multiply') return 'VK_MULTIPLY';
    if (num === 'Divide') return 'VK_DIVIDE';
    if (num === 'Decimal') return 'VK_DECIMAL';
    if (num === 'Enter') return 'VK_RETURN';
  }

  return null;
}

function onWindowKeydown(e: KeyboardEvent) {
  if (!editing.value) return;
  e.preventDefault();
  e.stopPropagation();

  if (e.key === 'Escape') {
    cancelEdit();
    return;
  }

  if (MODIFIER_KEYS.has(e.key)) {
    editing.value.pressedKeys.add(e.key);
    updateEditingDisplay();
    return;
  }

  const vk = eventToVkCode(e);
  if (!vk) return;

  const modifiers: string[] = [];
  if (editing.value.pressedKeys.has('Control')) modifiers.push(MODIFIER_VK.Control);
  if (editing.value.pressedKeys.has('Alt')) modifiers.push(MODIFIER_VK.Alt);
  if (editing.value.pressedKeys.has('Shift')) modifiers.push(MODIFIER_VK.Shift);

  const fullCombination = [...modifiers, vk];
  editing.value.currentValue = fullCombination.join(' + ');
  editing.value.combinationKeys = fullCombination;
  editing.value.finalized = true;
}

function onWindowKeyup(e: KeyboardEvent) {
  if (!editing.value) return;
  if (MODIFIER_KEYS.has(e.key)) {
    editing.value.pressedKeys.delete(e.key);
    if (!editing.value.finalized) {
      updateEditingDisplay();
    }
  }
}

function updateEditingDisplay() {
  if (!editing.value) return;
  const modifiers: string[] = [];
  if (editing.value.pressedKeys.has('Control')) modifiers.push(MODIFIER_VK.Control);
  if (editing.value.pressedKeys.has('Alt')) modifiers.push(MODIFIER_VK.Alt);
  if (editing.value.pressedKeys.has('Shift')) modifiers.push(MODIFIER_VK.Shift);
  if (modifiers.length > 0) {
    editing.value.currentValue = modifiers.join(' + ') + ' + ...';
  } else {
    editing.value.currentValue = t('keybinds.pressKey');
  }
}

async function saveEdit() {
  if (!editing.value) return;
  const { bindIndex, keyIndex, currentValue } = editing.value;
  const kb = keybinds.value[bindIndex];
  if (!kb) {
    cancelEdit();
    return;
  }
  const entry = kb.keys[keyIndex];
  if (!entry) {
    cancelEdit();
    return;
  }

  try {
    await invokeSaveKeybind(kb.filePath, kb.sectionName, entry.keyIndex, currentValue);
    ElMessage.success(t('keybinds.saveSuccess'));
    cancelEdit();
    await refreshCurrentKeybinds();
  } catch (e) {
    ElMessage.error(t('keybinds.saveFailed', { error: String(e) }));
  }
}

async function toggleEnabled(bindIndex: number, keyIdx: number, enabled: boolean) {
  const kb = keybinds.value[bindIndex];
  if (!kb) return;
  const entry = kb.keys[keyIdx];
  if (!entry) return;
  if (entry.disabled === !enabled) {
    try {
      await invokeToggleKeybindEnabled(kb.filePath, kb.sectionName, entry.keyIndex, enabled);
      await refreshCurrentKeybinds();
    } catch (e) {
      ElMessage.error(t('keybinds.toggleFailed', { error: String(e) }));
    }
  }
}

watch(selectedModPath, (path) => loadKeybinds(path), { immediate: true });

onUnmounted(() => {
  cancelEdit();
});
</script>

<template>
  <div class="keybinds-tab" tabindex="-1">
    <ElEmpty
      v-if="!selectedModPath"
      :description="t('keybinds.emptySelectMod')"
    />
    <ElEmpty
      v-else-if="error"
      :description="error"
    />
    <ElEmpty
      v-else-if="!loading && keybinds.length === 0"
      :description="t('keybinds.noKeybinds')"
    />
    <div v-else class="keybind-list">
      <ElCard
        v-for="(kb, bindIndex) in keybinds"
        :key="`${kb.filePath}:${kb.sectionName}`"
        class="keybind-card"
      >
        <template #header>
          <span class="section-header">{{ kb.sectionName }}</span>
        </template>
        <ElDescriptions :column="1" border>
          <ElDescriptionsItem :label="t('keybinds.keys')">
            <div class="key-entries">
              <div
                v-for="(keyEntry, keyIdx) in kb.keys"
                :key="keyIdx"
                class="key-entry-row"
                :class="{ 'is-disabled': keyEntry.disabled, 'is-editing': isEditing(bindIndex, keyIdx) }"
              >
                <template v-if="isEditing(bindIndex, keyIdx)">
                  <div class="key-edit-area">
                    <ElInput
                      :model-value="editing?.currentValue"
                      :placeholder="t('keybinds.pressKey')"
                      readonly
                      class="key-edit-input"
                    />
                    <div class="key-edit-actions">
                      <ElButton type="primary" size="small" @click="saveEdit">
                        {{ t('keybinds.save') }}
                      </ElButton>
                      <ElButton size="small" @click="cancelEdit">
                        {{ t('keybinds.cancel') }}
                      </ElButton>
                    </div>
                  </div>
                </template>
                <template v-else>
                  <span
                    class="key-value"
                    :class="{ 'key-value-disabled': keyEntry.disabled }"
                    :title="t('keybinds.clickToEdit')"
                    @click="startEdit(bindIndex, keyIdx)"
                  >
                    {{ keyEntry.value || '-' }}
                  </span>
                  <div class="key-entry-actions">
                    <ElSwitch
                      :model-value="!keyEntry.disabled"
                      :active-text="t('keybinds.enabled')"
                      :inactive-text="t('keybinds.disabled')"
                      size="small"
                      @change="(val: boolean) => toggleEnabled(bindIndex, keyIdx, val)"
                    />
                    <ElButton
                      text
                      size="small"
                      type="primary"
                      class="simulate-btn"
                      @click.stop="handleKeybindClick(bindIndex, kb, keyIdx)"
                    >
                      ▶
                    </ElButton>
                  </div>
                </template>
              </div>
            </div>
          </ElDescriptionsItem>
          <ElDescriptionsItem v-if="kb.back !== null" :label="t('keybinds.back')">
            {{ kb.back || '-' }}
          </ElDescriptionsItem>
          <ElDescriptionsItem v-if="kb.condition !== null" :label="t('keybinds.condition')">
            <code>{{ kb.condition || '-' }}</code>
          </ElDescriptionsItem>
          <ElDescriptionsItem v-if="kb.type !== null" :label="t('keybinds.type')">
            {{ kb.type || '-' }}
          </ElDescriptionsItem>
        </ElDescriptions>
      </ElCard>
    </div>
  </div>
</template>

<style scoped>
.keybinds-tab {
  height: 100%;
  overflow-y: auto;
  padding: 16px;
  outline: none;
}

.keybind-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.keybind-card {
  width: 100%;
}

.section-header {
  font-family: 'Consolas', 'Monaco', monospace;
  font-weight: 600;
}

.key-entries {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.key-entry-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 4px 0;
}

.key-entry-row.is-editing {
  padding: 8px;
  background: var(--el-color-primary-light-9);
  border-radius: 4px;
}

.key-value {
  font-family: 'Consolas', 'Monaco', monospace;
  padding: 4px 10px;
  border-radius: 4px;
  cursor: pointer;
  background: var(--el-fill-color-light);
  transition: background 0.15s ease;
  user-select: none;
}

.key-value:hover {
  background: var(--el-color-primary-light-8);
}

.key-value-disabled {
  color: var(--el-text-color-placeholder);
  text-decoration: line-through;
  opacity: 0.7;
}

.key-entry-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.simulate-btn {
  padding: 4px 8px;
  font-size: 14px;
}

.key-edit-area {
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.key-edit-input {
  width: 100%;
}

.key-edit-input :deep(.el-input__inner) {
  text-align: center;
  font-family: 'Consolas', 'Monaco', monospace;
  font-weight: 500;
}

.key-edit-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}
</style>
