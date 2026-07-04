/**
 * hotkeyValidator 单元测试
 *
 * 覆盖范围：
 * - validateHotkeys：三类快捷键（窗口切换、分组搜索、模组搜索）冲突检测
 * - 无冲突场景返回 valid: true
 * - 两两冲突场景（窗口 vs 分组、窗口 vs 模组、分组 vs 模组）
 * - 三项全部相同的冲突场景
 * - windowHotkey 为 'none' 时跳过窗口相关检测
 *
 * 对应测试需求：TR-4.1、TR-4.2、TR-9.4
 */
import { describe, it, expect } from 'vitest';
import { validateHotkeys } from '../hotkeyValidator';

describe('validateHotkeys', () => {
  it('无冲突场景返回 valid: true 与空冲突列表', () => {
    const result = validateHotkeys('altW', 'altG', 'altF');
    expect(result.valid).toBe(true);
    expect(result.conflicts).toEqual([]);
  });

  it('窗口切换与分组搜索冲突时返回 valid: false', () => {
    const result = validateHotkeys('altG', 'altG', 'altF');
    expect(result.valid).toBe(false);
    expect(result.conflicts.length).toBe(1);
    expect(result.conflicts[0].message).toMatch(/altG|Alt\+G/i);
    expect(result.conflicts[0].keys).toEqual(['altG', 'altG']);
  });

  it('窗口切换与模组搜索冲突时返回 valid: false', () => {
    const result = validateHotkeys('altF', 'altG', 'altF');
    expect(result.valid).toBe(false);
    expect(result.conflicts.length).toBe(1);
    expect(result.conflicts[0].message).toMatch(/altF|Alt\+F/i);
    expect(result.conflicts[0].keys).toEqual(['altF', 'altF']);
  });

  it('分组搜索与模组搜索冲突时返回 valid: false', () => {
    const result = validateHotkeys('altW', 'altG', 'altG');
    expect(result.valid).toBe(false);
    expect(result.conflicts.length).toBe(1);
    expect(result.conflicts[0].message).toMatch(/altG|Alt\+G/i);
    expect(result.conflicts[0].keys).toEqual(['altG', 'altG']);
  });

  it('三项全部相同返回 3 个冲突', () => {
    const result = validateHotkeys('altX', 'altX', 'altX');
    expect(result.valid).toBe(false);
    // 3 项两两组合共 C(3,2) = 3 个冲突
    expect(result.conflicts.length).toBe(3);
    for (const conflict of result.conflicts) {
      expect(conflict.message).toMatch(/altX|Alt\+X/i);
    }
  });

  it("windowHotkey 为 'none' 时仅检测非窗口冲突（窗口相关检测被跳过）", () => {
    // window='none'，分组与模组相同 -> 仅 1 个冲突（分组 vs 模组）
    const result = validateHotkeys('none', 'altG', 'altG');
    expect(result.valid).toBe(false);
    expect(result.conflicts.length).toBe(1);
    expect(result.conflicts[0].message).toMatch(/altG|Alt\+G/i);
  });

  it("windowHotkey 为 'none' 且无冲突时返回 valid: true", () => {
    const result = validateHotkeys('none', 'altG', 'altF');
    expect(result.valid).toBe(true);
    expect(result.conflicts).toEqual([]);
  });
});
