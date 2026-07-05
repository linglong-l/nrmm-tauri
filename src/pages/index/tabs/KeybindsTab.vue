<script setup lang="ts">
/**
 * KeybindsTab.vue - 热键标签页组件
 *
 * 作用：
 *  - 展示当前在 Mods 标签页中选中的模组所包含的所有 [Key*] 段。
 *  - 自动扫描选中模组目录下的所有 .ini 文件，解析并聚合其中的快捷键绑定。
 *  - 无选中模组或未找到快捷键时给出友好空状态提示。
 */
import { ref, computed, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { ElEmpty, ElCard, ElDescriptions, ElDescriptionsItem, ElMessage } from 'element-plus';
import { useGameStore } from '../../../stores/game';
import {
  invokeFindModIniFiles,
  invokeLoadIniData,
  invokeSimulateKeyPress,
  invokeSimulateKeyCombination,
} from '../../../utils/invoke';
import type { IniFileData } from '../../../types';

const { t } = useI18n();
const gameStore = useGameStore();

/** 是否正在加载 INI 数据 */
const loading = ref(false);
/** 已加载的所有 INI 文件数据 */
const iniFiles = ref<IniFileData[]>([]);
/** 加载过程中发生的错误信息 */
const error = ref<string | null>(null);

/**
 * 当前选中模组的绝对路径。
 * 根据当前分组路径从 gameStore 中读取。
 */
const selectedModPath = computed(() => {
  if (!gameStore.currentGroupPath) return null;
  return gameStore.getSelectedModPath(gameStore.currentGroupPath);
});

/**
 * 从 INI 段中提取的快捷键条目。
 */
interface KeybindEntry {
  /** 来源 INI 文件路径 */
  filePath: string;
  /** 段全名（如 Key.Toggle） */
  sectionName: string;
  /** 所有 key = 绑定的值列表 */
  keys: string[];
  /** back = 值（可选） */
  back: string | null;
  /** condition = 值（可选） */
  condition: string | null;
  /** type = 值（可选） */
  type: string | null;
}

/**
 * 聚合所有 INI 文件中 [Key*] 段的快捷键列表。
 */
const keybinds = computed<KeybindEntry[]>(() => {
  const result: KeybindEntry[] = [];
  for (const file of iniFiles.value) {
    for (const section of file.sections) {
      if (!section.name.toLowerCase().startsWith('key')) continue;
      const keys: string[] = [];
      let back: string | null = null;
      let condition: string | null = null;
      let type: string | null = null;
      for (const line of section.lines) {
        const keyLower = line.key.toLowerCase();
        if (keyLower === 'key') {
          keys.push(line.value);
        } else if (keyLower === 'back') {
          back = line.value;
        } else if (keyLower === 'condition') {
          condition = line.value;
        } else if (keyLower === 'type') {
          type = line.value;
        }
      }
      result.push({
        filePath: file.path,
        sectionName: section.name,
        keys,
        back,
        condition,
        type
      });
    }
  }
  return result;
});

/**
 * 加载指定模组路径下的所有 INI 文件并解析快捷键信息。
 * @param path 模组目录绝对路径，为 null 时清空当前列表
 */
async function loadKeybinds(path: string | null) {
  iniFiles.value = [];
  error.value = null;
  if (!path) return;

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

/**
 * 点击快捷键卡片时触发按键模拟。
 * 单键调用 simulate_key_press，多键调用 simulate_key_combination。
 * @param kb 被点击的快捷键条目
 */
async function handleKeybindClick(kb: KeybindEntry) {
  if (kb.keys.length === 0) return;

  try {
    if (kb.keys.length === 1) {
      await invokeSimulateKeyPress(kb.keys[0]);
    } else {
      await invokeSimulateKeyCombination(kb.keys);
    }
  } catch (e) {
    ElMessage.error(String(e));
  }
}

/**
 * 监听选中模组变化，自动重新加载快捷键数据。
 */
watch(selectedModPath, (path) => loadKeybinds(path), { immediate: true });
</script>

<template>
  <div class="keybinds-tab">
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
        v-for="kb in keybinds"
        :key="`${kb.filePath}:${kb.sectionName}`"
        class="keybind-card keybind-card-clickable"
        @click="handleKeybindClick(kb)"
      >
        <template #header>{{ kb.sectionName }}</template>
        <ElDescriptions :column="1" border>
          <ElDescriptionsItem :label="t('keybinds.keys')">
            {{ kb.keys.join(', ') || '-' }}
          </ElDescriptionsItem>
          <ElDescriptionsItem :label="t('keybinds.back')">
            {{ kb.back || '-' }}
          </ElDescriptionsItem>
          <ElDescriptionsItem :label="t('keybinds.condition')">
            {{ kb.condition || '-' }}
          </ElDescriptionsItem>
          <ElDescriptionsItem :label="t('keybinds.type')">
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
}

.keybind-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.keybind-card {
  width: 100%;
}

.keybind-card-clickable {
  cursor: pointer;
}

.keybind-card-clickable:hover {
  box-shadow: 0 2px 12px 0 rgba(0, 0, 0, 0.1);
}
</style>
