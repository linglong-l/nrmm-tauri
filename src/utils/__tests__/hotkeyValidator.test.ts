/**
 * hotkeyValidator 单元测试
 *
 * 覆盖范围：
 * - validateHotkeys：两类快捷键（窗口切换、搜索）冲突检测
 * - 无冲突场景返回 valid: true
 * - 窗口切换与搜索冲突场景
 * - windowHotkey 为 'none' 时跳过窗口相关检测
 *
 * 对应测试需求：TR-1.1、TR-1.2
 */
import { describe, it, expect } from 'vitest';
import { validateHotkeys } from '../hotkeyValidator';

describe('validateHotkeys', () => {
  it('无冲突场景返回 valid: true 与空冲突列表', () => {
    const result = validateHotkeys('altW', 'altF');
    expect(result.valid).toBe(true);
    expect(result.conflicts).toEqual([]);
  });

  it('窗口切换与搜索冲突时返回 valid: false', () => {
    const result = validateHotkeys('altF', 'altF');
    expect(result.valid).toBe(false);
    expect(result.conflicts.length).toBe(1);
    expect(result.conflicts[0].message).toMatch(/altF|Alt\+F/i);
    expect(result.conflicts[0].keys).toEqual(['altF', 'altF']);
  });

  it("windowHotkey 为 'none' 时跳过窗口相关检测", () => {
    const result = validateHotkeys('none', 'altF');
    expect(result.valid).toBe(true);
    expect(result.conflicts).toEqual([]);
  });

  it("windowHotkey 为 'none' 且搜索快捷键相同（无窗口）时无冲突", () => {
    const result = validateHotkeys('none', 'none');
    expect(result.valid).toBe(true);
    expect(result.conflicts).toEqual([]);
  });
});
