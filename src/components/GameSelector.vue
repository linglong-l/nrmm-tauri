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
import { useGame } from '../composables/useGame';
import { TargetGame } from '../types';
import { getGameNameKey } from '../utils/constants';

const { t } = useI18n();
const game = useGame();

// 游戏选项列表：value 为枚举值
const gameOptions = [
  TargetGame.Wuthering_Waves,
  TargetGame.Genshin_Impact,
  TargetGame.Honkai_Star_Rail,
  TargetGame.Zenless_Zone_Zero,
  TargetGame.Arknights_Endfield
] as const;

// 当前选中的游戏（双向绑定到 gameStore，setter 使用 useGame 的防抖版本）
const currentGame = computed({
  get: () => game.targetGame.value,
  set: (val: TargetGame) => game.setTargetGame(val)
});
</script>

<template>
  <!-- 游戏选择下拉框：v-model setter 已绑定 useGame 的防抖版本，无需额外 @change -->
  <div class="game-selector">
    <ElSelect
      v-model="currentGame"
      size="small"
      class="game-select"
    >
      <ElOption
        v-for="game in gameOptions"
        :key="game"
        :label="t(getGameNameKey(game))"
        :value="game"
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
