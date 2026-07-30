<template>
  <div class="group-tree-node">
    <!-- 分组项 -->
    <div
      class="group-item"
      :class="{
        active: isSelected,
        disabled: group.groupDisabled,
        'has-children': hasChildren,
        'virtual-root': isVirtualRoot,
      }"
      @click="handleClick"
      @contextmenu.prevent="onContextMenu"
    >
      <!-- 展开/折叠箭头（仅当有子分组时显示） -->
      <button
        v-if="hasChildren"
        class="expand-btn"
        :class="{ expanded: expanded }"
        @click.stop="toggleExpand"
        :title="expanded ? t('common.collapse') : t('common.expand')"
      >
        <el-icon :size="10"><ArrowRight /></el-icon>
      </button>
      <div v-else class="expand-placeholder"></div>

      <!-- 圆形头像 -->
      <div class="group-avatar" :class="{ 'virtual-avatar': isVirtualRoot }">
        <span class="avatar-text">{{ initialText }}</span>
      </div>
    </div>

    <!-- 分组名称（悬浮显示） -->
    <span class="group-name" :class="{ 'virtual-label': isVirtualRoot }" :title="group.groupName">{{ displayName }}</span>

    <!-- 子分组（递归渲染） -->
    <Transition name="tree-expand">
      <div v-if="expanded && hasChildren" class="children-wrapper">
        <GroupTreeNode
          v-for="(child, idx) in group.children"
          :key="child.groupPath || idx"
          :group="child"
          :depth="depth + 1"
          :selected-group-path="selectedGroupPath"
          @select="$emit('select', $event)"
          @context-menu="$emit('context-menu', $event)"
        />
      </div>
    </Transition>
  </div>
</template>

<script setup lang="ts">
/**
 * 分组树节点递归组件
 * 用于渲染左侧导航栏的树状分组结构，支持：
 * - 递归渲染子分组（children）
 * - 展开/折叠箭头
 * - 选中状态高亮
 * - 右键菜单
 * - 不同层级缩进
 */
import { ref, computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { ArrowRight } from '@element-plus/icons-vue'
import type { ModGroupData } from '@/types'

const { t } = useI18n()

const props = defineProps<{
  /** 当前分组数据 */
  group: ModGroupData
  /** 递归深度（用于缩进） */
  depth?: number
  /** 当前选中分组的路径 */
  selectedGroupPath: string
  /** 默认展开状态（虚拟根节点需要默认展开） */
  defaultExpanded?: boolean
}>()

const emit = defineEmits<{
  /** 选中分组事件 */
  select: [group: ModGroupData]
  /** 右键菜单事件 */
  'context-menu': [payload: { event: MouseEvent; group: ModGroupData }]
}>()

/** 递归深度，默认0 */
const depth = computed(() => props.depth ?? 0)

/** 是否是虚拟Groups根节点（仅UI用，不响应选择/右键菜单） */
const isVirtualRoot = computed(() => props.group.isVirtualRoot === true)

/** 是否有子分组 */
const hasChildren = computed(() => props.group.hasChild && props.group.children && props.group.children.length > 0)

/** 是否展开子分组：虚拟根默认展开，否则默认收起 */
const expanded = ref(props.defaultExpanded ?? isVirtualRoot.value)

/** 当前分组是否被选中 */
const isSelected = computed(() => !isVirtualRoot.value && props.selectedGroupPath === props.group.groupPath)

/** 分组显示名称（优先使用name字段，去掉DISABLED_前缀后的名称） */
const displayName = computed(() => props.group.name || props.group.groupName)

/** 头像显示文字：虚拟根使用"G"标识，其他使用首字母大写 */
const initialText = computed(() => {
  if (isVirtualRoot.value) return 'G'
  const name = displayName.value
  if (name && name.length > 0) {
    return name.charAt(0).toUpperCase()
  }
  return '?'
})

/** 切换展开/折叠状态 */
function toggleExpand() {
  expanded.value = !expanded.value
}

/** 点击分组项：虚拟根只切换展开/折叠，不选择；其他正常emit select */
function handleClick() {
  if (isVirtualRoot.value) {
    toggleExpand()
    return
  }
  emit('select', props.group)
}

/** 右键菜单：虚拟根屏蔽，避免误操作（因为不是真实目录） */
function onContextMenu(e: MouseEvent) {
  if (isVirtualRoot.value) return
  emit('context-menu', { event: e, group: props.group })
}
</script>

<style scoped>
.group-tree-node {
  display: flex;
  flex-direction: column;
  align-items: center;
  width: 100%;
}

.group-item {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 2px;
  cursor: pointer;
  padding: 4px;
  border-radius: 8px;
  transition: background-color 0.15s ease, opacity 0.15s ease;
  position: relative;
  width: 76px;
}

.group-item:hover {
  background: rgba(255, 255, 255, 0.06);
}

.group-item:focus-visible {
  outline: 2px solid var(--accent-primary);
  outline-offset: 1px;
}

.group-item.active {
  background: rgba(74, 158, 255, 0.12);
}

.group-item.disabled {
  opacity: 0.4;
}

.expand-btn {
  position: absolute;
  left: -2px;
  top: 50%;
  transform: translateY(-50%);
  width: 14px;
  height: 14px;
  border-radius: 50%;
  border: none;
  background: rgba(255, 255, 255, 0.08);
  color: rgba(255, 255, 255, 0.6);
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  padding: 0;
  transition: transform 0.2s ease, background-color 0.15s ease, color 0.15s ease;
  z-index: 2;
}

.expand-btn:hover {
  background: rgba(74, 158, 255, 0.3);
  color: #fff;
}

.expand-btn.expanded {
  transform: translateY(-50%) rotate(90deg);
}

.expand-placeholder {
  width: 14px;
  height: 14px;
  flex-shrink: 0;
}

.group-avatar {
  width: 40px;
  height: 40px;
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.06);
  display: flex;
  align-items: center;
  justify-content: center;
  border: 2px solid transparent;
  transition: border-color 0.2s ease, box-shadow 0.2s ease, background-color 0.2s ease;
  flex-shrink: 0;
}

.group-item.active .group-avatar {
  border-color: #4a9eff;
  box-shadow: 0 0 12px rgba(74, 158, 255, 0.5);
}

.group-item.has-children .group-avatar {
  width: 36px;
  height: 36px;
}

.avatar-text {
  font-size: 16px;
  font-weight: 600;
  color: #ccc;
}

.group-item.active .avatar-text {
  color: #fff;
}

.group-name {
  font-size: 9px;
  color: rgba(255, 255, 255, 0.5);
  text-align: center;
  width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  margin-top: 2px;
  max-width: 72px;
}

.group-item.active + .group-name {
  color: #fff;
}

.children-wrapper {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  width: 100%;
  padding-left: 0;
  position: relative;
}

/* 树形连接线效果 */
.children-wrapper::before {
  content: '';
  position: absolute;
  left: 50%;
  top: -2px;
  bottom: 4px;
  width: 1px;
  background: rgba(255, 255, 255, 0.08);
  transform: translateX(-50%);
}

/* 展开折叠动画 */
.tree-expand-enter-active,
.tree-expand-leave-active {
  transition: all 0.2s ease;
  overflow: hidden;
}

.tree-expand-enter-from,
.tree-expand-leave-to {
  opacity: 0;
  max-height: 0;
}

.tree-expand-enter-to,
.tree-expand-leave-from {
  opacity: 1;
  max-height: 1000px;
}

/* 虚拟"Groups"根节点：与普通角色分组区分（更强调容器感） */
.group-item.virtual-root {
  background: rgba(74, 158, 255, 0.08);
  border: 1px dashed rgba(74, 158, 255, 0.3);
}
.group-item.virtual-root:hover {
  background: rgba(74, 158, 255, 0.16);
}
.group-item.virtual-root .group-avatar.virtual-avatar {
  background: linear-gradient(135deg, rgba(74, 158, 255, 0.35), rgba(136, 85, 255, 0.3));
  border-color: rgba(74, 158, 255, 0.6);
  box-shadow: 0 0 10px rgba(74, 158, 255, 0.3);
}
.group-item.virtual-root .group-avatar.virtual-avatar .avatar-text {
  color: #e6f0ff;
  font-weight: 800;
}
.group-name.virtual-label {
  color: rgba(74, 158, 255, 0.85);
  font-weight: 700;
  letter-spacing: 0.5px;
  font-size: 10px;
}
</style>
