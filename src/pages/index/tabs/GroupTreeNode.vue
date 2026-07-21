<script setup lang="ts">
/**
 * GroupTreeNode.vue - 树形分组节点组件（递归组件）
 *
 * 作用：
 *  - 递归渲染树形分组结构，支持折叠/展开功能
 *  - 仅 isTreeNode=true 的分组会显示子节点
 *  - 支持点击选中、右键菜单、收藏图标等交互
 *
 * Props:
 *  - group: 当前分组数据
 *  - depth: 当前节点深度（用于缩进）
 *  - isActive: 是否为当前激活分组
 *  - isExpanded: 当前节点是否展开
 *  - expandedPaths: 已展开的分组路径集合
 *
 * Events:
 *  - select: 点击节点时触发
 *  - contextmenu: 右键点击时触发
 *  - toggle-expand: 切换展开/折叠状态
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { FolderOpened, Star, CaretRight } from '@element-plus/icons-vue';
import type { ModGroupData } from '../../../types';
import { convertToAssetUrl } from '../../../utils/invoke';
import { fuzzyMatch, splitByIndices } from '../../../utils/fuzzyMatch';

const props = defineProps<{
  group: ModGroupData;
  depth: number;
  isActive: boolean;
  isExpanded: boolean;
  expandedPaths: Set<string>;
  currentGroupPath: string;
  searchKeyword?: string;
}>();

const emit = defineEmits<{
  (e: 'select', group: ModGroupData): void;
  (e: 'contextmenu', event: MouseEvent, group: ModGroupData): void;
  (e: 'toggle-expand', groupPath: string): void;
}>();

const { t } = useI18n();

// 是否有子节点（仅 isTreeNode=true 的分组才有子节点）
const hasChildren = computed(() => {
  return props.group.isTreeNode && props.group.children && props.group.children.length > 0;
});

// 判断当前分组是否匹配搜索关键字（用于整项边框高亮）
const isHighlighted = computed(() => {
  if (!props.searchKeyword) return false;
  return fuzzyMatch(props.searchKeyword, props.group.groupName).matched;
});

// 根据搜索关键字拆分分组名为高亮片段
const groupNameSegments = computed(() => {
  if (!props.searchKeyword) return [{ text: props.group.groupName, highlight: false }];
  const result = fuzzyMatch(props.searchKeyword, props.group.groupName);
  if (!result.matched) return [{ text: props.group.groupName, highlight: false }];
  return splitByIndices(props.group.groupName, result.indices);
});

// 判断指定分组是否为当前激活的分组（用于高亮显示）
function isGroupActive(group: ModGroupData): boolean {
  return group.groupPath === props.currentGroupPath;
}

// 子节点是否应该显示（展开状态且有子节点）
const showChildren = computed(() => {
  return hasChildren.value && props.isExpanded;
});

// 缩进样式（根据深度计算）
const indentStyle = computed(() => {
  const indent = props.depth * 16; // 每层缩进 16px
  return {
    paddingLeft: `${indent}px`
  };
});

// 点击节点：有 mods 时选中并渲染右侧模组（叶子节点逻辑）；无 mods 但有 children 时展开/折叠
// 当节点同时有 children 和 mods 时：选中并自动展开 children（保留展开功能）
function handleClick(_event: MouseEvent) {
  if (props.group.modsInGroup.length > 0) {
    // 有 mods：按叶子节点逻辑选中并渲染右侧模组
    emit('select', props.group);
    // 同时有 children 时自动展开（不折叠，避免误操作隐藏 children）
    if (hasChildren.value && !props.isExpanded) {
      emit('toggle-expand', props.group.groupPath);
    }
  } else if (hasChildren.value) {
    emit('toggle-expand', props.group.groupPath);
  }
}

// 右键点击（虚拟节点不触发右键菜单）
function handleContextMenu(event: MouseEvent) {
  if (props.group.isVirtual) return;
  emit('contextmenu', event, props.group);
}

// 切换展开/折叠
function toggleExpand(event: MouseEvent) {
  event.stopPropagation(); // 阻止触发 select 事件
  emit('toggle-expand', props.group.groupPath);
}
</script>

<template>
  <div class="tree-node-wrapper">
    <!-- 分组节点项 -->
    <div
      class="group-item"
      :class="{ active: isActive, 'tree-node': group.isTreeNode, 'virtual-node': group.isVirtual, 'group-highlight': isHighlighted, 'group-disabled': group.isDisabled }"
      :style="indentStyle"
      @click="handleClick"
      @contextmenu="handleContextMenu($event)"
    >
      <!-- 左侧：图标 + 名称 + 计数 + 收藏 -->
      <div class="group-left">
        <!-- 分组图标区域 -->
        <div class="group-icon">
          <img
            v-if="group.iconPath"
            :src="convertToAssetUrl(group.iconPath)"
            alt="group icon"
            loading="lazy"
            @error="(e: Event) => (e.target as HTMLImageElement).style.display = 'none'"
          />
          <el-icon v-else>
            <FolderOpened />
          </el-icon>
        </div>

        <!-- 分组信息区域 -->
        <div class="group-info">
          <span class="group-name">
            <template v-for="(segment, idx) in groupNameSegments" :key="idx">
              <span :class="{ 'highlight-text': segment.highlight }">{{ segment.text }}</span>
            </template>
          </span>
          <span v-if="group.isDisabled" class="group-disabled-tag">{{ t('Disabled') }}</span>
          <!-- 模组数量统计（虚拟节点不显示） -->
          <span v-if="!group.isVirtual" class="group-count">{{ group.modsInGroup.length }} {{ t('Mods') }}</span>
        </div>

        <!-- 分组收藏图标（虚拟节点不显示） -->
        <el-icon v-if="group.favoriteDateTime && !group.isVirtual" class="group-favorite" color="#f59e0b">
          <Star />
        </el-icon>
      </div>

      <!-- 右侧：展开/折叠按钮（仅有子节点时显示） -->
      <div
        v-if="hasChildren"
        class="expand-toggle"
        :class="{ expanded: isExpanded }"
        @click="toggleExpand"
      >
        <el-icon>
          <CaretRight />
        </el-icon>
      </div>
    </div>

    <!-- 子节点列表（递归渲染） -->
    <div v-if="showChildren" class="children-container">
      <GroupTreeNode
        v-for="child in group.children"
        :key="child.groupPath"
        :group="child"
        :depth="depth + 1"
        :is-active="isGroupActive(child)"
        :is-expanded="expandedPaths.has(child.groupPath)"
        :expanded-paths="expandedPaths"
        :current-group-path="currentGroupPath"
        :search-keyword="searchKeyword"
        @select="emit('select', $event)"
        @contextmenu="(e, g) => emit('contextmenu', e, g)"
        @toggle-expand="emit('toggle-expand', $event)"
      />
    </div>
  </div>
</template>

<style scoped>
.tree-node-wrapper {
  display: flex;
  flex-direction: column;
}

.group-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  padding: 8px;
  padding-left: 8px; /* 基础左内边距，会被 indentStyle 覆盖 */
  border-radius: 12px;
  cursor: pointer;
  transition: all 0.2s ease;
  position: relative;
}

.group-item:hover {
  background-color: rgba(255, 255, 255, 0.06);
}

.group-item.active {
  background-color: rgba(64, 158, 255, 0.2);
}

/* 搜索关键字匹配的分组项高亮样式 */
.group-item.group-highlight {
  border: 1px solid var(--el-color-primary);
  border-radius: 12px;
}

/* 左侧内容容器（图标 + 信息 + 收藏） */
.group-left {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  flex: 1;
}

/* 右侧展开/折叠按钮 */
.expand-toggle {
  width: 20px;
  height: 20px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: transform 0.2s ease, color 0.2s ease;
  flex-shrink: 0;
  border-radius: 4px;
}

.expand-toggle :deep(.el-icon) {
  font-size: 12px;
  color: rgba(255, 255, 255, 0.5);
}

.expand-toggle:hover {
  background-color: rgba(255, 255, 255, 0.1);
}

.expand-toggle:hover :deep(.el-icon) {
  color: rgba(255, 255, 255, 0.9);
}

/* 展开状态时图标旋转 90 度 */
.expand-toggle.expanded {
  transform: rotate(90deg);
}

/* 分组图标 */
.group-icon {
  width: 48px;
  height: 48px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  border: 2px solid transparent;
  transition: border-color 0.2s;
  flex-shrink: 0;
}

.group-item.active .group-icon {
  border-color: var(--el-color-primary);
}

.group-icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.group-icon :deep(.el-icon) {
  font-size: 24px;
  color: rgba(255, 255, 255, 0.3);
}

/* 分组信息 */
.group-info {
  flex: 1;
  min-width: 0;
}

.group-name {
  font-size: 13px;
  font-weight: 500;
  color: rgba(255, 255, 255, 0.85);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.group-count {
  font-size: 11px;
  color: rgba(255, 255, 255, 0.4);
}

/* 收藏图标 */
.group-favorite {
  flex-shrink: 0;
  font-size: 16px;
}

/* 禁用状态样式 */
.group-item.group-disabled {
  opacity: 0.5;
}

.group-item.group-disabled .group-name {
  text-decoration: line-through;
  color: rgba(255, 255, 255, 0.5);
}

.group-disabled-tag {
  font-size: 10px;
  color: #f56c6c;
  background-color: rgba(245, 108, 108, 0.1);
  padding: 1px 4px;
  border-radius: 3px;
  margin-left: 4px;
}

/* 子节点容器 */
.children-container {
  display: flex;
  flex-direction: column;
}

/* 搜索关键字高亮字符 */
.highlight-text {
  color: var(--el-color-primary);
  font-weight: 600;
}
</style>