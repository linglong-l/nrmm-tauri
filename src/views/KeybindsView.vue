<template>
  <div class="keybinds-view">
    <div class="keybinds-scroll hide-scrollbar" ref="scrollRef">
      <!-- 90% 宽面板：无边框、无背景、高度自适应内容 -->
      <div class="keybinds-panel">
        <!-- 默认视图：缓存中无按键切换目标模组 -->
        <template v-if="!keybindTarget">
          <div class="default-hint">{{ t('Right-click a mod and select Keybind.') }}</div>
        </template>

        <!-- 已选视图：模组名 + 开关 + 按键列表 + 页脚 -->
        <template v-else>
          <h2 class="mod-name">{{ keybindTarget.modName }}</h2>

          <!-- 启用按钮：仅控制点击卡片是否触发模拟（对齐 NRMM keybindSimulateKeypressProvider） -->
          <div class="simulate-row">
            <el-switch v-model="simulateEnabled" active-color="#4a9eff" inactive-color="#555" />
            <span class="simulate-label">{{ t('Click keybind to simulate keypress') }}</span>
          </div>

          <!-- 按键显示列表：每个 [KeyXXX] 段一张卡，整卡可点击模拟（hover 放大 + 蓝框） -->
          <div v-if="cardList.length > 0" class="keybind-list" role="list">
            <div
              v-for="(card, idx) in cardList"
              :key="idx"
              class="keybind-card"
              :class="{ disabled: card.section.disabled, editing: isEditing }"
              role="listitem"
              :title="card.section.disabled || isEditing ? undefined : t('Click keybind to simulate keypress')"
              @click="onCardClick(card)"
            >
              <div class="keybind-section">
                <input
                  v-if="isEditing"
                  v-model="card.section.section"
                  class="keybind-input section-input"
                  type="text"
                  spellcheck="false"
                  @click.stop
                />
                <span v-else class="keybind-section-text">{{ card.section.section }}</span>
              </div>
              <div
                v-for="(entry, ei) in card.section.keys"
                :key="ei"
                class="keybind-value"
                :class="{ 'is-back': !entry.isMainKey }"
                @click.stop="onKeyClick(entry, card.section)"
              >
                <input
                  v-if="isEditing"
                  v-model="entry.value"
                  class="keybind-input"
                  type="text"
                  spellcheck="false"
                  @click.stop
                />
                <span v-else class="keybind-value-text">{{ entry.value }}</span>
              </div>
            </div>
          </div>

          <!-- 无按键数据空态 -->
          <div v-else class="empty-card">
            <p>{{ t('keybinds.noKeybinds') }}</p>
          </div>
        </template>
      </div>
    </div>
    <!-- 页脚：位于滚动区之外，固定于 GUI 最底部（对齐 NRMM 底部按钮组） -->
    <div v-if="keybindTarget" class="keybinds-footer">
      <button type="button" class="footer-link" @click="onEditOrSave">
        {{ isEditing ? t('Save Keybinds') : t('Edit Keybinds') }}
      </button>
      <button v-if="!isEditing" type="button" class="footer-link" @click="onToggleAll">
        {{ allDisabled ? t('Enable All Keybinds') : t('Disable All Keybinds') }}
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 按键切换页面
 *
 * 显示右键菜单「按键切换」选中的模组按键配置（`[KeyXXX]` 段 + `key =` / `back =` 原始值）。
 * 缓存语义：仅当 `modsStore.keybindTargetMod` 非空（右键设置）时显示已选视图，
 * 否则显示默认视图（不依赖 selectedMod 回退，避免未右键时误显示分组选中模组）。
 *
 * 支持：
 * - 点击按键卡片模拟组合键（对齐 NRMM simulateKeysFromKeySections）
 * - 「编辑快速切换」行内编辑段名 / 组合键并写回 INI（对齐 NRMM saveKeys）
 * - 「禁用/启用所有按键切换」批量写入 `;+;` 前缀
 * - 页面底部 debug 面板（mod 路径 / 文件/段/键统计 / 最近错误），便于排查异常
 */
import { ref, computed, watch, onMounted, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { readModKeybinds, saveModKeybinds, simulateKeys, simulateF10 } from '@/utils/tauri'
import { useModsStore } from '@/stores/mods'
import { logger } from '@/utils/logger'
import { useDragScroll } from '@/composables/useDragScroll'
import type { KeybindEntry, KeybindFile, KeybindSection } from '@/types'

const { t } = useI18n()
const modsStore = useModsStore()

/** 滚动容器DOM引用 */
const scrollRef = ref<HTMLElement | null>(null)
/** 拖拽滚动：复用 useDragScroll composable（默认排除规则已覆盖按钮/开关等交互元素） */
useDragScroll(scrollRef)

/** 按键切换目标模组（仅右键菜单设置；null 时显示默认视图） */
const keybindTarget = computed(() => modsStore.keybindTargetMod)
/** 缓存的按键数据（按文件分组） */
const keybindFiles = ref<KeybindFile[]>([])
/** 已拉取数据的模组路径（缓存命中判断） */
const cachedModPath = ref('')
/** 是否处于编辑模式 */
const isEditing = ref(false)
/** 是否启用点击模拟按键（仅控制 click→simulate，不持久化，对齐 NRMM keybindSimulateKeypressProvider） */
const simulateEnabled = ref(true)

/** 拉取竞态防护 token（快速切换模组时丢弃过期响应） */
let fetchToken = 0

/** 卡片列表：扁平化 file → section（渲染单位：每个段一张卡） */
const cardList = computed<{ file: KeybindFile; section: KeybindSection }[]>(() =>
  keybindFiles.value.flatMap((f) => f.sections.map((s) => ({ file: f, section: s })))
)

/** 是否所有按键段均已禁用（决定页脚按钮文案） */
const allDisabled = computed(() => {
  const all = keybindFiles.value.flatMap((f) => f.sections)
  return all.length > 0 && all.every((s) => s.disabled)
})

/**
 * 监听目标模组路径：首次进入或路径变化时拉取按键数据
 * - 无目标模组 → 清空（默认视图）
 * - 路径未变（缓存命中）→ 不重拉
 */
  watch(
  () => keybindTarget.value?.modPath,
  async (p) => {
    const token = ++fetchToken
    if (!p) {
      keybindFiles.value = []
      cachedModPath.value = ''
      isEditing.value = false
      return
    }
    if (p === cachedModPath.value) {
      // 缓存命中：无需重拉（保留编辑态）
      return
    }
    cachedModPath.value = p
    isEditing.value = false
    try {
      const data = await readModKeybinds(p)
      if (token !== fetchToken) return // 已切换到其他模组，丢弃过期响应
      keybindFiles.value = data
    } catch (e) {
      if (token !== fetchToken) return
      keybindFiles.value = []
      logger.warn('KeybindsView', 'readModKeybinds failed', p, e)
    }
  },
  { immediate: true }
)

/**
 * 点击整张按键卡片 → 模拟该段第一个 key（主 key）
 * 条件：非编辑模式 + 开关开启 + 段未禁用（对齐 NRMM _KeyCard 的 onPressed 判定）
 */
async function onCardClick(card: { file: KeybindFile; section: KeybindSection }) {
  const entry = card.section.keys[0]
  if (!entry) return
  await onKeyClick(entry, card.section)
}

/**
 * 点击按键值行 → 模拟该行组合键（多 key 段内每个 key/back 独立可点）
 * 条件：非编辑模式 + 开关开启 + 段未禁用（对齐 NRMM _KeyCard 的 onPressed 判定）
 */
async function onKeyClick(entry: KeybindEntry, section: KeybindSection) {
  if (isEditing.value || !simulateEnabled.value || section.disabled) return
  try {
    await simulateKeys(entry.value)
  } catch (e) {
    logger.warn('KeybindsView', 'simulateKeys failed', entry.value, e)
    ElMessage.error(t('Invalid keys'))
  }
}

/**
 * 页脚主按钮：编辑快速切换 ↔ 保存快速切换
 * 保存：传回编辑后的按键数据 → 后端原位改写写回 → 重拉最新内容
 */
async function onEditOrSave() {
  if (!isEditing.value) {
    isEditing.value = true
    return
  }
  await saveAndReload()
}

/**
 * 页脚副按钮：禁用/启用所有按键切换（批量写 `;+;` 前缀）
 */
async function onToggleAll() {
  const target = !allDisabled.value
  for (const f of keybindFiles.value) {
    for (const s of f.sections) s.disabled = target
  }
  await saveAndReload()
}

/**
 * 公共保存路径：保存 → 重拉 → 重复段名警告 + F10 重载提示
 */
async function saveAndReload() {
  const mod = keybindTarget.value
  if (!mod) return
  try {
    // 深拷贝为纯 JSON（Tauri invoke 序列化，剥离 Vue reactive proxy）
    const payload: KeybindFile[] = JSON.parse(JSON.stringify(keybindFiles.value))
    const result = await saveModKeybinds(mod.modPath, payload)
    // 重拉最新内容（行号可能因写回变化）
    keybindFiles.value = await readModKeybinds(mod.modPath)
    isEditing.value = false

    if (result.duplicateSections) {
      ElMessage.warning(t('Each section name must be unique (case-insensitive), to ensure all keys are working.'))
    }
    // F10 重载提示（对齐 NRMM saveKeys 的 Reload action）
    ElMessageBox.confirm(
      t("Don't forget to reload the mod with F10."),
      t('Keybinds'),
      {
        confirmButtonText: t('Reload'),
        cancelButtonText: t('keybinds.cancel', 'Cancel'),
        type: 'info',
        distinguishCancelAndClose: true,
      }
    )
      .then(() => {
        void simulateF10()
      })
      .catch(() => {})
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e)
    logger.error('KeybindsView', 'saveModKeybinds failed', e)
    ElMessage.error(t('keybinds.saveFailed', 'Save failed') + ': ' + msg)
  }
}

onMounted(async () => {
  await nextTick()
})
</script>

<style scoped>
.keybinds-view {
  flex: 1;
  display: flex;
  flex-direction: column;
  position: relative;
  overflow: hidden;
  background: transparent;
}

.keybinds-scroll {
  flex: 1;
  overflow-y: auto;
  padding: 0;
  display: flex;
  justify-content: center;
  align-items: flex-start;
}

/* 90% 宽面板：无边框、无背景、高度自适应内容 */
.keybinds-panel {
  width: 90%;
  min-width: 480px;
  padding: 8px;
  background: transparent;
  box-sizing: border-box;
}

/* 默认视图提示（含换行，保留 \n） */
.default-hint {
  text-align: right;
  white-space: pre-line;
  font-size: 14px;
  font-weight: 500;
  color: var(--text-primary, #fff);
  line-height: 1.6;
  user-select: none;
}

/* 模组名称：大号加粗居中 */
.mod-name {
  text-align: center;
  font-size: 22px;
  font-weight: 700;
  color: var(--text-primary, #fff);
  margin: 0 0 8px;
  line-height: 1.4;
  word-break: break-word;
  user-select: text;
}

/* 启用按钮行 */
.simulate-row {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  margin-bottom: 8px;
}

.simulate-label {
  color: var(--text-secondary, rgba(255, 255, 255, 0.78));
  font-size: 14px;
}

/* 按键卡片列表：flex 换行居中，卡片宽度自适应内容 */
.keybind-list {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 14px;
}

/* 按键容器：无背景色、宽度自适应内容、高度固定、整卡可点击，hover 轻微放大 + 蓝色边框 */
.keybind-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  width: fit-content; /* 宽度自适应内容 */
  height: 88px; /* 高度固定 */
  min-width: 110px; /* 防过窄（如单个字符 section） */
  padding: 2px 4px;
  background: transparent;
  border: 2px solid transparent; /* 常态不可见，hover 变蓝（避免布局跳动） */
  border-radius: 12px;
  cursor: pointer;
  border-color: #4a9eff; /* 蓝色边框 */
}

.keybind-card:hover:not(.disabled):not(.editing) {
  transform: scale(1.04); /* 轻微放大 */
}

.keybind-card:active:not(.disabled):not(.editing) {
  transform: scale(0.98);
}

.keybind-card.disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.keybind-card.editing {
  cursor: default;
}

.keybind-section {
  margin-bottom: 6px;
  width: 100%;
  text-align: center;
}

.keybind-section-text {
  font-size: 16px;
  font-weight: 700;
  color: var(--text-primary, #fff);
  word-break: break-word;
  white-space: pre-wrap;
}

/* 按键值：每行一个（key 与 back 分行） */
.keybind-value {
  width: 100%;
  display: flex;
  justify-content: center;
}

.keybind-value + .keybind-value {
  margin-top: 4px;
}

/* 按键值文本：白色字体、无背景色、自动换行（整卡可点，行内不再单独按钮） */
.keybind-value-text {
  padding: 2px 4px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 12px;
  color: var(--text-muted, rgba(255, 255, 255, 0.72));
  white-space: pre-wrap;
  word-break: break-word;
  text-align: center;
  line-height: 1.4;
  transition: color 0.15s ease;
}

.keybind-card:hover:not(.disabled):not(.editing) .keybind-value-text {
  color: #fff;
}

/* 编辑模式输入框 */
.keybind-input {
  width: 100%;
  background: transparent;
  border: none;
  border-bottom: 1px solid rgba(255, 255, 255, 0.35);
  border-radius: 0;
  color: #fff;
  font-family: inherit;
  font-size: 13px;
  text-align: center;
  outline: none;
  padding: 2px 4px;
}

.keybind-input:focus {
  border-bottom-color: var(--accent-primary, #4a9eff);
}

.section-input {
  font-size: 16px;
  font-weight: 700;
}

/* 空态 */
.empty-card {
  background: transparent;
  border: none;
  padding: 12px 0 4px 0;
  text-align: center;
  color: var(--text-secondary, rgba(255, 255, 255, 0.78));
  font-size: 14px;
}

/* 页脚 */
.keybinds-footer {
  /* 固定于 GUI 最底部（scroll 区外）：不参与内容滚动，flex-shrink 0 保持贴底 */
  flex-shrink: 0;
  display: flex;
  justify-content: center;
  gap: 24px;
  padding: 12px 16px 16px;
  flex-wrap: wrap;
  background: transparent;
}

.footer-link {
  background: transparent;
  border: none;
  padding: 4px 8px;
  font-size: 13px;
  font-weight: 600;
  color: var(--accent-primary, #4a9eff);
  cursor: pointer;
  transition: opacity 0.15s ease;
}

.footer-link:hover {
  opacity: 0.8;
  text-decoration: underline;
}
</style>
