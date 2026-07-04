<script setup lang="ts">
/**
 * index.vue - 主页面组件
 *
 * 作用：
 *  - 应用的主内容区骨架，左侧为 SideNav 顶部导航，右侧根据 activeTab 渲染对应内容。
 *  - 管理三个标签页：mods（模组管理）、keybinds（热键提示）、settings（设置页）。
 *  - 监听"模组分组更新"事件，同步到 gameStore。
 *  - 在设置加载完成后，注册全局热键到后端。
 * 
 * 注意：模组加载逻辑已统一交由 ModsTab.vue 负责，此处不再处理。
 */
import { ref, computed, onMounted, onUnmounted, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { ElEmpty, ElMessage } from 'element-plus';
import { SideNav } from '../../components';
import ModsTab from './tabs/ModsTab.vue';
import SettingsView from '../../views/SettingsView.vue';
import { useUiStore } from '../../stores/ui';
import { useGameStore } from '../../stores/game';
import { useSettingsStore } from '../../stores/settings';
import { useHotkeyStore } from '../../stores/hotkey';
import { EventNames, eventManager } from '../../utils/events';
import type { TabType } from '../../types';

const { t } = useI18n();
const uiStore = useUiStore();
const gameStore = useGameStore();
const settingsStore = useSettingsStore();
const hotkeyStore = useHotkeyStore();

// 当前激活的标签页（读写均委托给 uiStore，便于全局共享状态）
const activeTab = computed({
  get: () => uiStore.activeTab,
  set: (val: TabType) => uiStore.setActiveTab(val)
});

// ModsTab 组件实例引用（用于调用搜索框聚焦方法）
const modsTabRef = ref<InstanceType<typeof ModsTab> | null>(null);

// 事件监听取消函数句柄；组件卸载时需调用以避免内存泄漏
let unlistenModsUpdated: (() => void) | null = null;

/**
 * 将设置中配置的键盘热键注册到 Tauri 后端。
 * 先注销所有已注册热键，再注册新键，避免旧键未释放导致新键注册失败。
 * 限制：仅在设置变更时调用；失败时静默忽略，避免阻塞主流程。
 */
async function registerHotkeys() {
  try {
    const hotkey = settingsStore.hotkeyKeyboard;
    // 先注销所有已注册热键，避免旧键未释放导致新键注册失败
    await hotkeyStore.unregisterAllHotkeys();
    if (hotkey) {
      await hotkeyStore.registerHotkeyBackend(hotkey);
    }
  } catch {
    // ignore
  }
}

/**
 * 注册前端事件监听：
 *  - MOD_GROUPS_UPDATED：后端通知模组分组更新时，同步到 gameStore。
 * 业务逻辑：监听返回 Promise，注册成功后保存取消函数以便后续清理。
 * 注意：GAME_SWITCHED 的模组加载由 ModsTab.vue 负责，此处不再注册 no-op 监听器。
 */
let unlistenSettingsUpdated: (() => void) | null = null;

function setupEventListeners() {
  eventManager.on(EventNames.MOD_GROUPS_UPDATED, (groups) => {
    gameStore.setModGroups(groups);
  }).then((unlisten) => {
    unlistenModsUpdated = unlisten;
  }).catch(() => {});

  eventManager.on(EventNames.SETTINGS_UPDATED, () => {
    registerHotkeys();
  }).then((unlisten) => {
    unlistenSettingsUpdated = unlisten;
  }).catch(() => {});
}

/**
 * 清理事件监听：调用保存的取消函数并重置句柄。
 * 必须在 onUnmounted 中调用，防止组件销毁后仍触发回调导致报错。
 */
function cleanupEventListeners() {
  if (unlistenModsUpdated) {
    unlistenModsUpdated();
    unlistenModsUpdated = null;
  }
  if (unlistenSettingsUpdated) {
    unlistenSettingsUpdated();
    unlistenSettingsUpdated = null;
  }
}

/**
 * 全局键盘按下事件处理函数。
 *
 * 作用：
 *  - 监听窗口内的 Alt+字母 组合键，匹配设置中的搜索快捷键配置
 *  - 匹配成功时聚焦对应搜索框（分组搜索框或模组搜索框）
 *  - 当焦点在 input/textarea/contenteditable 元素时不触发，避免干扰文字输入
 *  - 仅在 mods 标签页激活时生效（搜索框仅在 mods 标签页存在）
 *
 * 设计目的：为用户提供窗口内快捷键快速聚焦搜索框，与后端注册的全局热键
 * （呼出菜单）解耦，仅作用于本窗口且不干扰正常文字输入。
 *
 * @param event 键盘事件
 */
function handleSearchHotkey(event: KeyboardEvent): void {
  // 仅在 mods 标签页激活时响应
  if (uiStore.activeTab !== 'mods') return;

  // 必须按住 Alt 键
  if (!event.altKey) return;

  // 焦点在 input/textarea/contenteditable 元素时不触发，避免干扰文字输入
  const target = event.target as HTMLElement;
  if (target) {
    const tagName = target.tagName.toLowerCase();
    if (tagName === 'input' || tagName === 'textarea') return;
    if (target.isContentEditable) return;
  }

  // 转换按键为 "alt+字母" 格式（小写）
  const key = event.key.toLowerCase();
  // 仅匹配单个字母键（a-z），忽略其他功能键
  if (key.length !== 1 || !/[a-z]/.test(key)) return;

  const pressedHotkey = `alt${key}`;

  // 诊断日志：记录按键输入和当前配置
  console.debug('[SearchHotkey] Pressed:', pressedHotkey,
    'groupSearch:', settingsStore.groupSearchHotkey,
    'modSearch:', settingsStore.modSearchHotkey,
    'windowHotkey:', settingsStore.hotkeyKeyboard);

  // 检查是否与全局热键冲突 — 告知用户
  if (pressedHotkey === settingsStore.hotkeyKeyboard.toLowerCase()) {
    console.warn('[SearchHotkey] Conflict with window hotkey:', pressedHotkey);
    ElMessage.warning(t('Hotkey conflict: {key} is used by window toggle', { key: pressedHotkey }));
    return;
  }

  // 匹配分组搜索快捷键（统一小写比较）— toggle 显示/隐藏
  if (pressedHotkey === settingsStore.groupSearchHotkey.toLowerCase()) {
    event.preventDefault();
    console.debug('[SearchHotkey] Triggering group search');
    modsTabRef.value?.toggleGroupSearch();
    return;
  }

  // 匹配模组搜索快捷键（统一小写比较）— toggle 显示/隐藏
  if (pressedHotkey === settingsStore.modSearchHotkey.toLowerCase()) {
    event.preventDefault();
    console.debug('[SearchHotkey] Triggering mod search');
    modsTabRef.value?.toggleModSearch();
    return;
  }
}



// 监听标签页切换：切回 mods 标签页时触发缓存校验
// 业务逻辑：由于使用 v-show 切换标签页，ModsTab 只 mount 一次，
// 切回时不会重新触发 onMounted，因此需要在此显式校验缓存数据一致性。
watch(activeTab, (newTab) => {
  if (newTab === 'mods') {
    const result = gameStore.validateCache(gameStore.targetGame);
    if (result.action === 'load') {
      gameStore.loadModsForGame(gameStore.targetGame);
    } else if (result.action === 'clear_and_load') {
      gameStore.clearModsCache();
      gameStore.loadModsForGame(gameStore.targetGame);
    }
    // action 为 'skip' 或 'use_cache' 时无需操作
  }
});

// 组件挂载：注册事件监听；注册窗口内搜索快捷键监听器
onMounted(() => {
  setupEventListeners();
  // 注册窗口内搜索快捷键监听器
  window.addEventListener('keydown', handleSearchHotkey);
});

// 组件卸载：清理事件监听；移除窗口内搜索快捷键监听器
onUnmounted(() => {
  cleanupEventListeners();
  // 移除窗口内搜索快捷键监听器
  window.removeEventListener('keydown', handleSearchHotkey);
});
</script>

<template>
  <div class="index-page">
    <SideNav />
    <div class="content-area">
      <div class="content-body">
        <!-- 模组管理标签页 -->
        <div v-show="activeTab === 'mods'" class="tab-content">
          <ModsTab ref="modsTabRef" />
        </div>
        <!-- 热键标签页：仅显示空状态提示，引导用户从模组右键菜单进入 -->
        <div v-show="activeTab === 'keybinds'" class="tab-content">
          <ElEmpty :description="t('Right-click a mod and select Keybind.')" />
        </div>
        <!-- 设置标签页 -->
        <div v-show="activeTab === 'settings'" class="tab-content">
          <SettingsView />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.index-page {
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  overflow: hidden;
}

.content-area {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.content-body {
  flex: 1;
  overflow: hidden;
}

.tab-content {
  height: 100%;
  display: flex;
  flex-direction: column;
}
</style>
