/**
 * 模糊匹配工具模块
 * 
 * 提供 subsequence match（子序列匹配）功能，
 * 关键字字符按顺序在目标文本中匹配，返回匹配字符的索引数组。
 */

/** 模糊匹配结果 */
export interface FuzzyMatchResult {
  /** 是否匹配 */
  matched: boolean;
  /** 匹配字符在目标文本中的索引数组（0-based） */
  indices: number[];
}

/**
 * 执行 subsequence 模糊匹配。
 * 
 * 算法：遍历目标文本，按顺序查找关键字中的每个字符。若关键字的所有字符都能按顺序在文本中找到，则匹配成功。
 * 不区分大小写。空关键字匹配所有文本（返回 matched: true, indices: []）。
 * 
 * @param keyword 搜索关键字
 * @param text 目标文本
 * @returns 匹配结果，含匹配字符索引数组
 * 
 * @example
 * fuzzyMatch('hd', 'HUD Mod') // { matched: true, indices: [0, 2] }
 * fuzzyMatch('xyz', 'HUD Mod') // { matched: false, indices: [] }
 * fuzzyMatch('', 'HUD Mod') // { matched: true, indices: [] }
 */
export function fuzzyMatch(keyword: string, text: string): FuzzyMatchResult {
  // 空关键字匹配全部
  if (keyword.length === 0) {
    return { matched: true, indices: [] };
  }

  return fuzzyMatchWithLowerKeyword(keyword.toLowerCase(), text);
}

/**
 * 使用已转换为小写的关键字执行模糊匹配。
 * 当对大量文本使用同一关键字匹配时，可避免重复对关键字调用 toLowerCase()。
 *
 * @param lowerKeyword 已小写的搜索关键字
 * @param text 目标文本
 * @returns 匹配结果，含匹配字符索引数组
 */
export function fuzzyMatchWithLowerKeyword(lowerKeyword: string, text: string): FuzzyMatchResult {
  if (lowerKeyword.length === 0) {
    return { matched: true, indices: [] };
  }

  const lowerText = text.toLowerCase();
  const indices: number[] = [];
  let keywordIndex = 0;

  for (let i = 0; i < lowerText.length && keywordIndex < lowerKeyword.length; i++) {
    if (lowerText[i] === lowerKeyword[keywordIndex]) {
      indices.push(i);
      keywordIndex++;
    }
  }

  // 所有关键字字符都按顺序匹配到了
  if (keywordIndex === lowerKeyword.length) {
    return { matched: true, indices };
  }

  return { matched: false, indices: [] };
}

/** 文本片段：标记是否高亮 */
export interface TextSegment {
  text: string;
  highlight: boolean;
}

/**
 * 根据匹配索引数组，将文本拆分为高亮片段。
 * 
 * @param text 原始文本
 * @param indices 匹配字符索引数组
 * @returns 片段数组，每个片段标记是否高亮
 */
export function splitByIndices(text: string, indices: number[]): TextSegment[] {
  if (indices.length === 0) {
    return [{ text, highlight: false }];
  }

  const segments: TextSegment[] = [];
  const indexSet = new Set(indices);
  let currentText = '';
  let currentHighlight = !indexSet.has(0);

  for (let i = 0; i < text.length; i++) {
    const isHighlight = indexSet.has(i);
    if (isHighlight !== currentHighlight) {
      if (currentText) {
        segments.push({ text: currentText, highlight: currentHighlight });
      }
      currentText = '';
      currentHighlight = isHighlight;
    }
    currentText += text[i];
  }

  if (currentText) {
    segments.push({ text: currentText, highlight: currentHighlight });
  }

  return segments;
}
