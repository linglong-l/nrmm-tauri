<script setup lang="ts">
/**
 * 高亮文本组件
 *
 * 根据匹配区间数组在文本中渲染 <mark> 高亮标签，完全避免使用 v-html，
 * 从根本上消除 XSS 风险。
 *
 * 使用 v-for + :key 渲染文本片段，性能与 v-html 相当，
 * 但安全性更高（文本内容由 Vue 自动转义）。
 *
 * @example
 * <HighlightText text="Hello World" :spans="[[0, 5]]" />
 * // 渲染: <mark>Hello</mark> World
 */
import { computed } from 'vue'

/**
 * 文本片段接口
 */
interface TextSegment {
  /** 片段文本内容 */
  text: string
  /** 是否为高亮匹配片段 */
  highlight: boolean
}

const props = defineProps<{
  /** 原始文本内容 */
  text: string
  /** 匹配区间数组，每个元素为 [start, end] 元组，表示 text 中需要高亮的区间 */
  spans?: [number, number][]
}>()

/**
 * 将原始文本按匹配区间拆分为文本片段数组
 *
 * 例如：text="Hello World", spans=[[0,5]]
 * 拆分为：[{ text: "Hello", highlight: true }, { text: " World", highlight: false }]
 */
const segments = computed<TextSegment[]>(() => {
  const text = props.text || ''
  const spans = props.spans

  if (!spans || spans.length === 0) {
    return [{ text, highlight: false }]
  }

  const result: TextSegment[] = []
  let cursor = 0

  for (const [start, end] of spans) {
    // 匹配区间前的普通文本
    if (start > cursor) {
      result.push({ text: text.slice(cursor, start), highlight: false })
    }
    // 匹配区间内的高亮文本
    result.push({ text: text.slice(start, end), highlight: true })
    cursor = end
  }

  // 最后一个匹配区间后的普通文本
  if (cursor < text.length) {
    result.push({ text: text.slice(cursor), highlight: false })
  }

  return result
})
</script>

<template>
  <span>
    <template v-for="(seg, i) in segments" :key="i">
      <mark v-if="seg.highlight" class="highlight-mark">{{ seg.text }}</mark>
      <template v-else>{{ seg.text }}</template>
    </template>
  </span>
</template>

<style scoped>
/* 搜索命中字符高亮：金黄色底色 + 加粗 */
.highlight-mark {
  background: rgba(245, 195, 90, 0.35);
  color: #ffe7a3;
  font-weight: 700;
  border-radius: 2px;
  padding: 0 1px;
}
</style>

<script lang="ts">
export default { name: 'HighlightText' }
</script>
