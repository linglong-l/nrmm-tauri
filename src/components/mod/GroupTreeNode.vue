<template>
  <div v-if="shouldShow" class="group-tree-node">
    <!-- 分组项 -->
    <div
      class="group-item"
      :class="{
        active: isSelected,
        disabled: group.groupDisabled,
        'has-children': hasChildren,
        'virtual-root': isVirtualRoot,
        'search-hit-item': !!props.isGroupHit,
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
        <img v-if="avatarUrl" :src="avatarUrl" class="avatar-image" :alt="displayName" />
        <span v-else class="avatar-text">{{ initialText }}</span>
      </div>
    </div>

    <!-- 分组名称（悬浮显示） -->
    <span class="group-name" :class="{ 'virtual-label': isVirtualRoot }" :title="displayName"><HighlightText :text="displayName" :spans="highlightSpans" /></span>

    <!-- 子分组（递归渲染） -->
    <Transition name="tree-expand">
      <div v-if="expanded && hasChildren" class="children-wrapper">
        <GroupTreeNode
          v-for="(child, idx) in group.children"
          :key="child.groupPath || idx"
          :group="child"
          :depth="depth + 1"
          :selected-group-path="selectedGroupPath"
          :search-query="props.searchQuery"
          :is-group-hit="modsStore.isGroupMatch(child.groupPath)"
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
import { ref, computed, watch, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { ArrowRight } from '@element-plus/icons-vue'
import { convertFileSrc } from '@tauri-apps/api/core'
import type { ModGroupData } from '@/types'
import { useModsStore } from '@/stores/mods'
import HighlightText from '@/components/common/HighlightText.vue'

const { t } = useI18n()
const modsStore = useModsStore()

const props = defineProps<{
  /** 当前分组数据 */
  group: ModGroupData
  /** 递归深度（用于缩进） */
  depth?: number
  /** 当前选中分组的路径 */
  selectedGroupPath: string
  /** 默认展开状态（虚拟根节点需要默认展开） */
  defaultExpanded?: boolean
  /** 搜索关键词（空则不高亮） */
  searchQuery?: string
  /** 当前分组是否为搜索命中项 */
  isGroupHit?: boolean
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

/**
 * 监听搜索词及 defaultExpanded（autoExpandGroupPaths 变化会触发父组件重算 defaultExpanded）
 * 当搜索命中需要展开该分组（或其后代命中）时，自动确保 expanded=true
 * 用户手动折叠后再次输入新搜索词也能重新展开命中分支
 */
watch(
  () => [props.searchQuery, props.defaultExpanded, props.group.groupPath, isVirtualRoot.value] as const,
  ([, newDefaultExpanded, , vRoot], [oldQuery] ,) => {
    const queryToggled = !!props.searchQuery?.trim() !== !!oldQuery?.trim()
    // 搜索激活或defaultExpanded变为true → 强制展开
    if (newDefaultExpanded === true && !expanded.value) {
      expanded.value = true
    } else if (queryToggled && !props.searchQuery?.trim() && vRoot) {
      // 搜索退出且是虚拟根，保持默认展开
      expanded.value = true
    }
  }
)

/** 当前分组是否被选中 */
const isSelected = computed(() => !isVirtualRoot.value && props.selectedGroupPath === props.group.groupPath)

/**
 * 监听选中状态变化，确保选中分组在可视区域内
 * 退出搜索后布局恢复时，自动滚动到选中的分组节点
 */
watch(isSelected, (selected) => {
  if (selected) {
    nextTick(() => {
      const el = document.querySelector(`.group-item.active`)
      if (el) {
        el.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
      }
    })
  }
})

/** 分组显示名称（优先使用name字段，去掉DISABLED_前缀后的名称） */
const displayName = computed(() => props.group.name || props.group.groupName)

const highlightSpans = computed<[number, number][]>(() => {
  const name = displayName.value || ''
  const q = props.searchQuery?.trim()
  if (!q || !name) return []
  const { matched, spans } = modsStore.fuzzyMatchWithSpansSimple(name, q)
  if (!matched || !spans.length) return []
  return spans
})

/** 头像显示文字：虚拟根使用"G"标识，其他使用首字母大写 */
const initialText = computed(() => {
  if (isVirtualRoot.value) return 'G'
  const name = displayName.value
  if (name && name.length > 0) {
    return name.charAt(0).toUpperCase()
  }
  return '?'
})

/** 分组头像URL：将previewImagePath转换为webview可访问的asset:// URL */
const avatarUrl = computed(() => {
  if (isVirtualRoot.value) return ''
  const p = props.group.previewImagePath
  if (!p) return ''
  try {
    return convertFileSrc(p)
  } catch {
    return ''
  }
})

/**
 * 搜索时是否显示当前分组
 * - 无搜索词：显示所有分组
 * - 有搜索词：显示匹配的分组、包含匹配模组的分组、虚拟根节点
 */
const shouldShow = computed(() => {
  const q = props.searchQuery?.trim()
  if (!q) return true
  // 虚拟根节点始终显示（否则整个树连根都没了）
  if (isVirtualRoot.value) return true
  // 关键修复：全局无任何命中 → 保持分组可见，避免导航栏视觉清空
  if (modsStore.globalNoHit) return true
  // 当前分组匹配搜索词
  if (props.isGroupHit) return true
  // 当前分组的子分组中有匹配的
  if (hasChildren.value) {
    return hasMatchingDescendant(props.group)
  }
  return false
})

/**
 * 递归检查分组是否有匹配搜索词的后代
 */
function hasMatchingDescendant(group: ModGroupData): boolean {
  const q = props.searchQuery?.trim()
  if (!q) return false
  if (!group.children || group.children.length === 0) return false
  for (const child of group.children) {
    if (modsStore.isGroupMatch(child.groupPath)) return true
    if (hasMatchingDescendant(child)) return true
  }
  return false
}

/** 切换展开/折叠状态 */
function toggleExpand() {
  expanded.value = !expanded.value
}

/**
 * 点击分组项
 * - 虚拟根：只切换展开/折叠，不选择
 * - 有直接模组：触发选择（emit select）+ 关闭搜索框（如果打开）
 * - 无直接模组（仅有子分组或空）：仅触发展开/收起，不选择
 */
function handleClick(e: MouseEvent) {
  // 阻止事件冒泡到 GroupPanel，避免 handlePanelClick 干扰
  e.stopPropagation()

  if (isVirtualRoot.value) {
    toggleExpand()
    return
  }

  // 无直接模组（仅子分组或空）：仅展开/收起，不触发选择
  if (!props.group.mods || props.group.mods.length === 0) {
    toggleExpand()
    return
  }

  // 有直接模组：先触发展开+分组选择
  if (!expanded.value) {
    toggleExpand()
  }
  emit('select', props.group)

  // 关闭搜索框（如可见）
  if (modsStore.searchVisible) {
    modsStore.setSearchVisible(false)
    modsStore.clearSearch()
  }
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

.avatar-image {
  width: 100%;
  height: 100%;
  border-radius: 50%;
  object-fit: cover;
  display: block;
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

/* 搜索命中的分组节点（发光光环 + 放大） */
.search-hit-item {
  background: rgba(245, 195, 90, 0.12) !important;
  box-shadow: 0 0 10px rgba(245, 195, 90, 0.35);
  transition: all 0.2s ease;
}
.search-hit-item .group-avatar {
  border-color: rgba(245, 195, 90, 0.8) !important;
}
</style>
