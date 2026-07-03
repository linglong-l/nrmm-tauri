/**
 * 前端本地缓存工具模块
 * 
 * 使用 localStorage 存储数据，提供统一的缓存管理接口。
 * 支持缓存过期机制，确保数据不会无限期存储。
 * 
 * 缓存键命名规范：
 * - 模组数据：`mods_{game}`
 * - 其他数据：`{prefix}_{key}`
 */

/**
 * 缓存项接口定义
 */
interface CacheItem<T> {
  /** 缓存数据 */
  data: T;
  /** 缓存时间戳（毫秒） */
  timestamp: number;
  /** 过期时间（毫秒） */
  ttl: number;
}

/**
 * 默认缓存过期时间（5分钟）
 */
const DEFAULT_TTL = 5 * 60 * 1000;

/**
 * 生成游戏模组数据的缓存键
 * @param game 游戏名称
 * @returns 缓存键名
 */
export function getModsCacheKey(game: string): string {
  return `mods_${game}`;
}

/**
 * 获取缓存数据
 * @param key 缓存键名
 * @param ttl 过期时间（毫秒），默认5分钟
 * @returns 缓存数据，若不存在或过期则返回 null
 */
export function getCache<T>(key: string, ttl: number = DEFAULT_TTL): T | null {
  try {
    const item = localStorage.getItem(key);
    if (!item) return null;

    const cacheItem: CacheItem<T> = JSON.parse(item);
    const now = Date.now();

    // 检查是否过期
    if (now - cacheItem.timestamp > ttl) {
      // 已过期，删除缓存
      localStorage.removeItem(key);
      return null;
    }

    return cacheItem.data;
  } catch {
    return null;
  }
}

/**
 * 设置缓存数据
 * @param key 缓存键名
 * @param data 缓存数据
 * @param ttl 过期时间（毫秒），默认5分钟
 */
export function setCache<T>(key: string, data: T, ttl: number = DEFAULT_TTL): void {
  try {
    const cacheItem: CacheItem<T> = {
      data,
      timestamp: Date.now(),
      ttl,
    };
    localStorage.setItem(key, JSON.stringify(cacheItem));
  } catch {
    // localStorage 不可用或存储失败，静默忽略
  }
}

/**
 * 删除指定缓存
 * @param key 缓存键名
 */
export function removeCache(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    // 忽略错误
  }
}

/**
 * 清除所有缓存
 */
export function clearAllCache(): void {
  try {
    localStorage.clear();
  } catch {
    // 忽略错误
  }
}

/**
 * 获取游戏模组缓存数据
 * @param game 游戏名称
 * @param ttl 过期时间（毫秒），默认5分钟
 * @returns 模组数据，若不存在或过期则返回 null
 */
export function getModsCache<T>(game: string, ttl: number = DEFAULT_TTL): T | null {
  return getCache<T>(getModsCacheKey(game), ttl);
}

/**
 * 设置游戏模组缓存数据
 * @param game 游戏名称
 * @param data 模组数据
 * @param ttl 过期时间（毫秒），默认5分钟
 */
export function setModsCache<T>(game: string, data: T, ttl: number = DEFAULT_TTL): void {
  setCache<T>(getModsCacheKey(game), data, ttl);
}

/**
 * 删除游戏模组缓存数据
 * @param game 游戏名称
 */
export function removeModsCache(game: string): void {
  removeCache(getModsCacheKey(game));
}
