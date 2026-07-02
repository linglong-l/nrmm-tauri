<script setup lang="ts">
/**
 * index.vue - 主页面组件
 *
 * 作用：
 *  - 应用的主内容区骨架，左侧为 SideNav 顶部导航，右侧根据 activeTab 渲染对应内容。
 *  - 管理三个标签页：mods（模组管理）、keybinds（热键提示）、settings（设置页）。
 *  - 负责模组数据的初次加载与刷新：监听"游戏切换"和"模组分组更新"事件。
 *  - 在设置加载完成后，注册全局热键到后端。
 */
import { computed, onMounted, onUnmounted, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { ElEmpty } from 'element-plus';
import { SideNav } from '../../components';
import ModsTab from './tabs/ModsTab.vue';
import SettingsView from '../../views/SettingsView.vue';
import { useUiStore } from '../../stores/ui';
import { useGameStore } from '../../stores/game';
import { useSettingsStore } from '../../stores/settings';
import { useHotkeyStore } from '../../stores/hotkey';
import { EventNames, eventManager } from '../../utils/events';
import { invokeLoadMods } from '../../utils/invoke';
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

// 事件监听取消函数句柄；组件卸载时需调用以避免内存泄漏
let unlistenModsUpdated: (() => void) | null = null;
let unlistenGameSwitched: (() => void) | null = null;

/**
 * 加载当前目标游戏的模组分组数据。
 * 业务逻辑：
 *  - 若未选择游戏（targetGame === 'none'），直接返回不加载。
 *  - 加载期间设置全局 loading 状态，便于 UI 显示加载指示。
 *  - 成功后将分组写入 gameStore 并标记 modsLoaded = true；失败则置为 false。
 */
async function loadModData() {
  if (gameStore.targetGame === 'none') return;
  try {
    uiStore.setLoading(true, t('Loading mods'));
    const groups = await invokeLoadMods(gameStore.targetGame);
    gameStore.setModGroups(groups);
    gameStore.setModsLoaded(true);
  } catch {
    gameStore.setModsLoaded(false);
  } finally {
    uiStore.setLoading(false);
  }
}

/**
 * 将设置中配置的键盘热键注册到 Tauri 后端。
 * 限制：仅在设置加载完成时调用；失败时静默忽略，避免阻塞主流程。
 */
async function registerHotkeys() {
  try {
    const hotkey = settingsStore.hotkeyKeyboard;
    await hotkeyStore.registerHotkeyBackend(hotkey);
  } catch {
    // ignore
  }
}

/**
 * 注册前端事件监听：
 *  - GAME_SWITCHED：游戏切换时重新加载模组数据。
 *  - MOD_GROUPS_UPDATED：后端通知模组分组更新时，同步到 gameStore。
 * 业务逻辑：监听返回 Promise，注册成功后保存取消函数以便后续清理。
 */
function setupEventListeners() {
  eventManager.on(EventNames.GAME_SWITCHED, () => {
    loadModData();
  }).then((unlisten) => {
    unlistenGameSwitched = unlisten;
  }).catch(() => {});

  eventManager.on(EventNames.MOD_GROUPS_UPDATED, (groups) => {
    gameStore.setModGroups(groups);
  }).then((unlisten) => {
    unlistenModsUpdated = unlisten;
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
  if (unlistenGameSwitched) {
    unlistenGameSwitched();
    unlistenGameSwitched = null;
  }
}

// 监听设置加载完成事件：加载完成后立即拉取模组数据并注册热键
watch(
  () => settingsStore.isLoaded,
  (loaded) => {
    if (loaded) {
      loadModData();
      registerHotkeys();
    }
  }
);

// 组件挂载：注册事件监听；若设置已加载则立即加载数据与注册热键
onMounted(() => {
  setupEventListeners();
  if (settingsStore.isLoaded) {
    loadModData();
    registerHotkeys();
  }
});

// 组件卸载：清理事件监听
onUnmounted(() => {
  cleanupEventListeners();
});
</script>

<template>
  <div class="index-page">
    <SideNav />
    <div class="content-area">
      <div class="content-body">
        <!-- 模组管理标签页 -->
        <div v-if="activeTab === 'mods'" class="tab-content">
          <ModsTab />
        </div>
        <!-- 热键标签页：仅显示空状态提示，引导用户从模组右键菜单进入 -->
        <div v-else-if="activeTab === 'keybinds'" class="tab-content">
          <ElEmpty :description="t('Right-click a mod and select Keybind.')" />
        </div>
        <!-- 设置标签页 -->
        <div v-else-if="activeTab === 'settings'" class="tab-content">
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
