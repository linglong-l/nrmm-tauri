/**
 * createDebounce 单元测试
 *
 * 覆盖范围：
 * - 基本防抖行为（多次调用仅最后一次执行）
 * - 延迟前调用不触发
 * - 延迟后调用正常触发
 * - 参数传递正确性
 * - 连续快速调用下的计时器重置
 * - clearTimeout 边界场景
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { createDebounce } from '../debounce';

describe('createDebounce', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('多次调用仅最后一次执行', async () => {
    const fn = vi.fn().mockResolvedValue(undefined);
    const debounced = createDebounce(fn, 100);

    // 连续调用 3 次
    debounced('a');
    debounced('b');
    debounced('c');

    // 前两次被清除，fn 尚未被调用
    expect(fn).not.toHaveBeenCalled();

    // 快进 100ms
    await vi.advanceTimersByTimeAsync(100);

    expect(fn).toHaveBeenCalledTimes(1);
    expect(fn).toHaveBeenCalledWith('c');
  });

  it('延迟前调用不触发', () => {
    const fn = vi.fn().mockResolvedValue(undefined);
    const debounced = createDebounce(fn, 100);

    debounced('test');

    // 50ms 时不应触发
    vi.advanceTimersByTime(50);
    expect(fn).not.toHaveBeenCalled();
  });

  it('延迟后调用正常触发', async () => {
    const fn = vi.fn().mockResolvedValue(undefined);
    const debounced = createDebounce(fn, 100);

    debounced('test');

    await vi.advanceTimersByTimeAsync(100);
    expect(fn).toHaveBeenCalledTimes(1);
    expect(fn).toHaveBeenCalledWith('test');
  });

  it('参数正确传递', async () => {
    const fn = vi.fn().mockResolvedValue(undefined);
    const debounced = createDebounce(fn, 100);

    debounced(42, 'hello', { key: 'value' });

    await vi.advanceTimersByTimeAsync(100);
    expect(fn).toHaveBeenCalledWith(42, 'hello', { key: 'value' });
  });

  it('连续快速调用下计时器重置', async () => {
    const fn = vi.fn().mockResolvedValue(undefined);
    const debounced = createDebounce(fn, 100);

    debounced('first');
    vi.advanceTimersByTime(50);
    debounced('second');
    vi.advanceTimersByTime(50);
    // 此时距离第一次调用 100ms，但第二次调用重置了计时器
    expect(fn).not.toHaveBeenCalled();

    debounced('third');
    vi.advanceTimersByTime(50);
    expect(fn).not.toHaveBeenCalled();

    // 再等 50ms，第三次调用触发
    await vi.advanceTimersByTimeAsync(50);
    expect(fn).toHaveBeenCalledTimes(1);
    expect(fn).toHaveBeenCalledWith('third');
  });

  it('timer 为 null 时 clearTimeout 不抛异常', () => {
    const fn = vi.fn().mockResolvedValue(undefined);
    const debounced = createDebounce(fn, 100);

    // 第一次调用后等待执行完成
    debounced('first');
    vi.advanceTimersByTime(100);

    // 第二次调用（timer 为 null 时 clearTimeout 应安全）
    expect(() => {
      debounced('second');
    }).not.toThrow();
  });

  it('零延迟立即执行', async () => {
    const fn = vi.fn().mockResolvedValue(undefined);
    const debounced = createDebounce(fn, 0);

    debounced('test');

    await vi.advanceTimersByTimeAsync(0);
    expect(fn).toHaveBeenCalledTimes(1);
  });
});