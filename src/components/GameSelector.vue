<script setup lang="ts">
/**
 * GameSelector.vue - 游戏选择器组件
 *
 * 作用：
 *  - 提供一个下拉选择框，用于在 5 个支持的抽卡游戏之间切换当前目标游戏。
 *  - 选中值双向绑定到 gameStore.targetGame，切换后会触发游戏切换事件链（重新加载模组等）。
 *  - 选项文案通过 i18n 进行本地化。
 *
 * 支持的游戏：鸣潮、原神、崩坏：星穹铁道、绝区零、明日方舟：终末地。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { ElSelect, ElOption } from 'element-plus';
import { useGameStore } from '../stores/game';
import { TargetGame } from '../types';

const { t } = useI18n();
const gameStore = useGameStore();

// 游戏选项列表：value 为枚举值，labelKey 为 i18n 翻译键
const gameOptions = [
  { value: TargetGame.Wuthering_Waves, labelKey: 'Wuthering Waves' },
  { value: TargetGame.Genshin_Impact, labelKey: 'Genshin Impact' },
  { value: TargetGame.Honkai_Star_Rail, labelKey: 'Honkai Star Rail' },
  { value: TargetGame.Zenless_Zone_Zero, labelKey: 'Zenless Zone Zero' },
  { value: TargetGame.Arknights_Endfield, labelKey: 'Arknights Endfield' }
] as const;

// 当前选中的游戏（双向绑定到 gameStore）
const currentGame = computed({
  get: () => gameStore.targetGame,
  set: (val: TargetGame) => gameStore.setTargetGame(val)
});
</script>

<template>
  <!-- 游戏选择下拉框：change 事件再次调用 setTargetGame 以确保状态同步 -->
  <div class="game-selector">
    <ElSelect
      v-model="currentGame"
      size="small"
      class="game-select"
      @change="(val: TargetGame) => gameStore.setTargetGame(val)"
    >
      <ElOption
        v-for="game in gameOptions"
        :key="game.value"
        :label="t(game.labelKey)"
        :value="game.value"
      />
    </ElSelect>
  </div>
</template>

<style scoped>
.game-selector {
  display: flex;
  align-items: center;
}

.game-select {
  width: 200px;
}
</style>
