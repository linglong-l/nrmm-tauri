/**
 * cache 工具模块单元测试
 *
 * 覆盖范围：
 * - getCache / setCache 基本读写
 * - 缓存过期机制（TTL 过期返回 null）
 * - 自定义 TTL
 * - removeCache 删除缓存
 * - clearAllCache 清除所有缓存
 * - getModsCacheKey 模组缓存键生成
 * - getModsCache / setModsCache / removeModsCache 模组缓存操作
 * - JSON 解析失败容错
 * - localStorage 不可用容错
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  getCache,
  setCache,
  removeCache,
  clearAllCache,
  getModsCache,
  setModsCache,
  removeModsCache,
  getModsCacheKey,
} from '../cache';

describe('cache', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('getCache / setCache', () => {
    it('基本 set/get 流程', () => {
      setCache('testKey', { value: 42 });
      const result = getCache<{ value: number }>('testKey');
      expect(result).toEqual({ value: 42 });
    });

    it('缓存不存在时返回 null', () => {
      const result = getCache('nonexistent');
      expect(result).toBeNull();
    });

    it('缓存过期后返回 null（默认 TTL 5 分钟）', () => {
      setCache('testKey', 'data', 1000); // 1 秒 TTL

      // 仍在有效期内
      vi.advanceTimersByTime(500);
      expect(getCache('testKey', 1000)).toBe('data');

      // 过期后
      vi.advanceTimersByTime(600);
      const result = getCache('testKey', 1000);
      expect(result).toBeNull();
      // 过期后应自动删除
      expect(localStorage.getItem('testKey')).toBeNull();
    });

    it('自定义 TTL', () => {
      setCache('testKey', 'data', 100); // 100ms TTL

      vi.advanceTimersByTime(50);
      expect(getCache('testKey', 100)).toBe('data');

      vi.advanceTimersByTime(60);
      expect(getCache('testKey', 100)).toBeNull();
    });

    it('支持各种数据类型', () => {
      setCache('str', 'hello');
      setCache('num', 123);
      setCache('arr', [1, 2, 3]);
      setCache('obj', { a: 1, b: { c: 2 } });

      expect(getCache<string>('str')).toBe('hello');
      expect(getCache<number>('num')).toBe(123);
      expect(getCache<number[]>('arr')).toEqual([1, 2, 3]);
      expect(getCache<object>('obj')).toEqual({ a: 1, b: { c: 2 } });
    });
  });

  describe('removeCache', () => {
    it('删除存在的缓存', () => {
      setCache('testKey', 'data');
      expect(getCache('testKey')).toBe('data');

      removeCache('testKey');
      expect(getCache('testKey')).toBeNull();
    });

    it('删除不存在的缓存不抛异常', () => {
      expect(() => removeCache('nonexistent')).not.toThrow();
    });
  });

  describe('clearAllCache', () => {
    it('清除所有缓存', () => {
      setCache('key1', 'data1');
      setCache('key2', 'data2');

      clearAllCache();

      expect(getCache('key1')).toBeNull();
      expect(getCache('key2')).toBeNull();
    });
  });

  describe('模组缓存操作', () => {
    it('getModsCacheKey 生成正确的键名', () => {
      expect(getModsCacheKey('Wuthering_Waves')).toBe('mods_Wuthering_Waves');
      expect(getModsCacheKey('Genshin_Impact')).toBe('mods_Genshin_Impact');
    });

    it('setModsCache / getModsCache 读写流程', () => {
      setModsCache('Wuthering_Waves', [{ groupPath: '/test' }]);
      const result = getModsCache('Wuthering_Waves');
      expect(result).toEqual([{ groupPath: '/test' }]);
    });

    it('removeModsCache 删除模组缓存', () => {
      setModsCache('Wuthering_Waves', 'data');
      expect(getModsCache('Wuthering_Waves')).toBe('data');

      removeModsCache('Wuthering_Waves');
      expect(getModsCache('Wuthering_Waves')).toBeNull();
    });
  });

  describe('容错处理', () => {
    it('localStorage 中存储非法 JSON 时 getCache 返回 null', () => {
      // 直接写入非法 JSON 到 localStorage
      localStorage.setItem('badJson', 'not valid json{{{');

      const result = getCache('badJson');
      expect(result).toBeNull();
    });

    it('localStorage.setItem 抛异常时 setCache 不抛出', () => {
      const originalSetItem = localStorage.setItem;
      localStorage.setItem = vi.fn(() => {
        throw new Error('QuotaExceeded');
      });

      expect(() => setCache('testKey', 'data')).not.toThrow();

      localStorage.setItem = originalSetItem;
    });

    it('localStorage.getItem 抛异常时 getCache 返回 null', () => {
      const originalGetItem = localStorage.getItem;
      localStorage.getItem = vi.fn(() => {
        throw new Error('SecurityError');
      });

      const result = getCache('testKey');
      expect(result).toBeNull();

      localStorage.getItem = originalGetItem;
    });

    it('localStorage.removeItem 抛异常时 removeCache 不抛出', () => {
      const originalRemoveItem = localStorage.removeItem;
      localStorage.removeItem = vi.fn(() => {
        throw new Error('SecurityError');
      });

      expect(() => removeCache('testKey')).not.toThrow();

      localStorage.removeItem = originalRemoveItem;
    });
  });
});