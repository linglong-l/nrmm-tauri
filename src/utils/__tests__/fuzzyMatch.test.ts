/**
 * fuzzyMatch 单元测试
 *
 * 覆盖范围：
 * - fuzzyMatch：subsequence 模糊匹配（按顺序匹配关键字字符）
 * - 空关键字匹配全部
 * - 不区分大小写
 * - 关键字比文本长时匹配失败
 * - splitByIndices：根据匹配索引将文本拆分为高亮/非高亮片段
 * - 空索引数组返回单个非高亮片段
 * - 相邻高亮索引合并为同一片段
 *
 * 对应测试需求：TR-6.1、TR-6.2、TR-6.3、TR-9.4
 */
import { describe, it, expect } from 'vitest';
import { fuzzyMatch, splitByIndices } from '../fuzzyMatch';

describe('fuzzyMatch', () => {
  it('子序列匹配成功时返回匹配索引数组', () => {
    // 'h' 在位置 0，'d' 在位置 2（'HUD Mod'）
    expect(fuzzyMatch('hd', 'HUD Mod')).toEqual({ matched: true, indices: [0, 2] });
  });

  it('无法按顺序匹配所有字符时返回 matched: false', () => {
    expect(fuzzyMatch('xyz', 'HUD Mod')).toEqual({ matched: false, indices: [] });
  });

  it('空关键字匹配全部，返回空索引数组', () => {
    expect(fuzzyMatch('', 'HUD Mod')).toEqual({ matched: true, indices: [] });
  });

  it('连续匹配返回连续索引数组', () => {
    expect(fuzzyMatch('hud', 'HUD Mod')).toEqual({ matched: true, indices: [0, 1, 2] });
  });

  it('匹配不区分大小写', () => {
    // 大写关键字匹配小写文本
    expect(fuzzyMatch('HUD', 'hud mod')).toEqual({ matched: true, indices: [0, 1, 2] });
  });

  it('关键字比文本长时返回 matched: false', () => {
    expect(fuzzyMatch('hudd', 'HUD')).toEqual({ matched: false, indices: [] });
  });
});

describe('splitByIndices', () => {
  it('根据索引数组拆分为高亮与非高亮交替片段', () => {
    // 'HUD'，高亮索引 [0, 2] -> H(高亮)、U(非高亮)、D(高亮)
    expect(splitByIndices('HUD', [0, 2])).toEqual([
      { text: 'H', highlight: true },
      { text: 'U', highlight: false },
      { text: 'D', highlight: true },
    ]);
  });

  it('空索引数组返回单个非高亮片段（完整文本）', () => {
    expect(splitByIndices('HUD', [])).toEqual([{ text: 'HUD', highlight: false }]);
  });

  it('相邻高亮索引合并为同一高亮片段', () => {
    // 'ABCD'，高亮索引 [1, 2] -> A(非高亮)、BC(高亮)、D(非高亮)
    expect(splitByIndices('ABCD', [1, 2])).toEqual([
      { text: 'A', highlight: false },
      { text: 'BC', highlight: true },
      { text: 'D', highlight: false },
    ]);
  });
});
