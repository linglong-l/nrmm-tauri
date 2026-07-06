<script setup lang="ts">
/**
 * StatusBar.vue - 应用底部状态栏组件
 *
 * 作用：
 *  - 在窗口底部显示一行状态信息：模组总数、当前游戏名、热键启用状态、应用版本号。
 *  - 数据全部派生自各个 store，本身无独立状态。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { useGameStore } from '../stores/game';
import { useHotkeyStore } from '../stores/hotkey';
import { useSettingsStore } from '../stores/settings';
import { getGameNameKey } from '../utils/constants';
import { HotkeyGamepad } from '../types';

const { t } = useI18n();
const gameStore = useGameStore();
const hotkeyStore = useHotkeyStore();
const settingsStore = useSettingsStore();

// 模组总数：累加所有分组内的模组数量
const totalMods = computed(() => {
  return gameStore.modGroups.reduce((sum, group) => sum + group.modsInGroup.length, 0);
});

// 当前游戏显示名（未选择时显示 None），使用 i18n 国际化
const currentGameName = computed(() => {
  return t(getGameNameKey(gameStore.targetGame));
});

// 热键状态文本：键盘热键始终启用，手柄热键可设为 none
// 业务逻辑：当前实现下两者只要有一个非 none 即视为"已启用"
const hotkeyStatus = computed(() => {
  // 键盘热键始终启用（无法禁用），手柄热键可设为 "none"
  // 如果手柄热键不是 none，则两者都启用
  if (settingsStore.hotkeyGamepad !== HotkeyGamepad.none) {
    return t('Enabled');
  }
  // 键盘热键始终启用
  return t('Enabled');
});

// 应用版本号（硬编码，仅状态栏展示用）
const version = '0.1.1';
</script>

<template>
  <!--
    状态栏根容器
    作用：在窗口底部显示一行状态信息
    布局：flex 水平布局，左右两侧分别显示不同状态项
  -->
  <div class="status-bar">
    <!--
      左侧状态区域
      作用：显示模组总数和当前游戏名称
      布局：flex 水平布局，元素间距 8px
    -->
    <div class="status-left">
      <!--
        模组总数显示项
        数据来源：
          - totalMods (computed) 累加所有分组内的模组数量
        结构：标签 "Mods:" + 数值
      -->
      <span class="status-item">
        <!-- 标签文本：固定显示 "Mods:" -->
        <span class="status-label">{{ t('Mods') }}:</span>
        <!-- 数值：显示 totalMods computed 计算的模组总数 -->
        <span class="status-value">{{ totalMods }}</span>
      </span>
      <!-- 分隔符：竖线 "|"，用于视觉分隔不同状态项 -->
      <span class="status-divider">|</span>
      <!--
        当前游戏名称显示项
        数据来源：
          - currentGameName (computed) 从 GAME_NAMES 映射表获取当前游戏的显示名
          - gameStore.targetGame 未选择时显示 "None"
      -->
      <span class="status-item">
        <span class="status-label">{{ currentGameName }}</span>
      </span>
    </div>
    <!--
      右侧状态区域
      作用：显示热键启用状态和应用版本号
      布局：flex 水平布局，元素间距 8px
    -->
    <div class="status-right">
      <!--
        热键状态显示项
        数据来源：
          - hotkeyStatus (computed) 热键启用状态文本（始终显示 "Enabled"）
          - hotkeyStore.isHotkeyEnabled 控制是否添加高亮样式
        动态绑定：
          - :class 当 isHotkeyEnabled 为 true 时添加 'status-enabled' 类（绿色高亮）
      -->
      <span class="status-item">
        <span class="status-label">{{ t('Hotkey:') }}</span>
        <span class="status-value" :class="{ 'status-enabled': hotkeyStore.isHotkeyEnabled }">
          {{ hotkeyStatus }}
        </span>
      </span>
      <!-- 分隔符：竖线 "|"，用于视觉分隔不同状态项 -->
      <span class="status-divider">|</span>
      <!--
        应用版本号显示项
        数据来源：
          - version (const) 硬编码版本号 '0.1.1'
        显示格式：前缀 "v" + 版本号
      -->
      <span class="status-item">
        <span class="status-value">v{{ version }}</span>
      </span>
    </div>
  </div>
</template>

<style scoped>
.status-bar {
  height: 26px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 12px;
  font-size: 11px;
  color: rgba(255, 255, 255, 0.35);
  border-top: 1px solid rgba(255, 255, 255, 0.04);
  flex-shrink: 0;
}

.status-left,
.status-right {
  display: flex;
  align-items: center;
  gap: 8px;
}

.status-item {
  display: flex;
  align-items: center;
  gap: 4px;
}

.status-label {
  color: rgba(255, 255, 255, 0.35);
}

.status-value {
  color: rgba(255, 255, 255, 0.55);
  font-weight: 500;
}

.status-enabled {
  color: #67c23a;
}

.status-divider {
  color: rgba(255, 255, 255, 0.1);
}
</style>
