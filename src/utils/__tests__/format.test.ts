/**
 * formatDuration 单元测试
 *
 * 覆盖范围：
 * - 正常毫秒数格式化
 * - 零值
 * - 小数毫秒
 * - 大数值
 * - 负值行为
 */
import { describe, it, expect } from 'vitest';
import { formatDuration } from '../format';

describe('formatDuration', () => {
  it('1000ms 格式化为 "1.00 秒"', () => {
    expect(formatDuration(1000)).toBe('1.00 秒');
  });

  it('0ms 格式化为 "0.00 秒"', () => {
    expect(formatDuration(0)).toBe('0.00 秒');
  });

  it('500ms 格式化为 "0.50 秒"', () => {
    expect(formatDuration(500)).toBe('0.50 秒');
  });

  it('1523ms 格式化为 "1.52 秒"', () => {
    expect(formatDuration(1523)).toBe('1.52 秒');
  });

  it('100ms 格式化为 "0.10 秒"', () => {
    expect(formatDuration(100)).toBe('0.10 秒');
  });

  it('大数值 3600000ms 格式化正确', () => {
    expect(formatDuration(3600000)).toBe('3600.00 秒');
  });

  it('负值 -1000ms 格式化为 "-1.00 秒"', () => {
    expect(formatDuration(-1000)).toBe('-1.00 秒');
  });

  it('极小值 1ms 格式化为 "0.00 秒"', () => {
    expect(formatDuration(1)).toBe('0.00 秒');
  });
});