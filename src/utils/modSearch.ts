/**
 * 全局搜索的纯文本匹配工具
 *
 * 从 stores/mods.ts 外移：这三个函数不依赖任何 store 状态，纯入参出参，
 * 便于独立单测（行为断言见 src/composables/edge_cases.test.ts）。
 * mods.ts 仍按同名再导出（`store.normalizeText` / `store.fuzzyMatchWithSpansSimple` 调用点不变）。
 */
import type { ModGroupData } from '../types'

/**
 * 文本归一化：Unicode NFKC + 大小写 + 空白统一
 */
export function normalizeText(str: string): string {
  if (!str) return ''
  return str
    .normalize('NFKC')
    .replace(/\s|\u00A0|\u202F/g, '')
    .toLowerCase()
}

/**
 * 简化正确版本：找到 query 每个字符在 text 中按顺序出现的原始下标
 * 将连续命中的下标合并为 [start, end) 区间
 */
export function fuzzyMatchWithSpansSimple(text: string, query: string): { matched: boolean; spans: [number, number][] } {
  if (!query) return { matched: false, spans: [] }
  const normText = normalizeText(text)
  const normQuery = normalizeText(query)
  if (!normText || !normQuery) return { matched: false, spans: [] }

  // Step 1: 构造 textIdxToNormIdx 映射
  const textIdxToNormIdx: number[] = []
  let normPos = 0
  for (let i = 0; i < text.length; i++) {
    const origChar = text[i]
    const normChar = normalizeText(origChar)
    if (normChar) {
      textIdxToNormIdx.push(normPos)
      normPos += normChar.length
    } else {
      textIdxToNormIdx.push(-1)
    }
  }

  // Step 2: 在 normText 上双指针，记录命中的 normText 下标数组
  const hitNormIndices: number[] = []
  let q = 0
  for (let n = 0; n < normText.length && q < normQuery.length; n++) {
    if (normText[n] === normQuery[q]) {
      hitNormIndices.push(n)
      q++
    }
  }
  if (q < normQuery.length) return { matched: false, spans: [] }

  // Step 3: 将命中的 normText 下标映射回原 text 下标
  const hitTextIndices: number[] = []
  for (const hitNorm of hitNormIndices) {
    for (let ti = 0; ti < textIdxToNormIdx.length; ti++) {
      if (textIdxToNormIdx[ti] !== -1 && textIdxToNormIdx[ti] <= hitNorm && hitNorm < textIdxToNormIdx[ti] + 1) {
        hitTextIndices.push(ti)
        break
      }
    }
  }

  // Step 4: 合并连续下标为 [start, end) 区间
  const spans: [number, number][] = []
  if (hitTextIndices.length === 0) return { matched: true, spans: [] }
  let runStart = hitTextIndices[0]
  let prev = runStart
  for (let i = 1; i < hitTextIndices.length; i++) {
    const cur = hitTextIndices[i]
    if (cur === prev + 1) {
      prev = cur
    } else {
      spans.push([runStart, prev + 1])
      runStart = cur
      prev = cur
    }
  }
  spans.push([runStart, prev + 1])
  return { matched: true, spans }
}

/**
 * 递归遍历分组树，收集命中搜索词的分组路径及需要自动展开的祖先路径
 */
export function collectGroupMatchesRecursive(
  groupList: ModGroupData[],
  q: string,
  matchedPaths: Set<string>,
  expandPaths: Set<string>,
  ancestors: string[] = []
): void {
  for (const g of groupList) {
    const name = g.name || g.groupName || ''
    const { matched } = fuzzyMatchWithSpansSimple(name, q)
    if (matched) {
      matchedPaths.add(g.groupPath)
      // 命中分组 → 所有祖先都需要自动展开
      for (const anc of ancestors) expandPaths.add(anc)
    }
    if (g.children && g.children.length > 0) {
      collectGroupMatchesRecursive(
        g.children,
        q,
        matchedPaths,
        expandPaths,
        [...ancestors, g.groupPath]
      )
      // 如果子树中有命中 → 当前节点也需要展开并保留
      const subtreeHasMatch = g.children.some(
        (c) => matchedPaths.has(c.groupPath) || expandPaths.has(c.groupPath)
      )
      if (subtreeHasMatch) {
        for (const anc of ancestors) expandPaths.add(anc)
        expandPaths.add(g.groupPath)
      }
    }
  }
}
