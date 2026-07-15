// Hash 冲突检测事件监听组合式函数模块。
// 该模块封装了 HASH_CONFLICTS_DETECTED 事件订阅与 store 联动逻辑，
// 在组件挂载时注册监听、卸载时清理，避免内存泄漏。
import { onMounted, onBeforeUnmount } from 'vue';
import { useHashConflictStore } from '../stores/hashConflict';
import { useEvent, EventNames } from '../utils/events';
import { createLogger } from '../utils/logger';
import type { TargetGame } from '../types';

/**
 * 手写防抖函数，避免引入 lodash-es 依赖。
 * @param fn 要防抖的函数
 * @param wait 等待时间（毫秒）
 * @returns 防抖后的函数，附带 cancel 方法用于取消防抖
 */
function debounce<T extends (...args: any[]) => any>(fn: T, wait: number): T & { cancel: () => void } {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const debounced = function (this: any, ...args: Parameters<T>) {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      fn.apply(this, args);
      timer = null;
    }, wait);
  } as T & { cancel: () => void };
  debounced.cancel = () => {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
  };
  return debounced;
}

/**
 * Hash 冲突检测 composable。
 *
 * 在组件 `setup` 中调用后：
 * 1. 通过全局事件总线订阅 `HASH_CONFLICTS_DETECTED` 事件；
 * 2. 收到事件时将 payload 中的 report 写入 hashConflict store；
 * 3. 监听 `GAME_SWITCHED` 事件，5 秒防抖后主动触发 hash 冲突检测；
 * 4. 组件卸载时（`onBeforeUnmount`）自动取消订阅与防抖定时器，避免内存泄漏。
 *
 * 通常在应用根组件（如 `App.vue`）的 setup 中调用一次即可，
 * 全局事件管理器会负责多组件共享与去重。
 */
export function useHashConflict() {
  const log = createLogger('HashConflict');
  // 获取 hash 冲突 store
  const hashConflictStore = useHashConflictStore();
  // 获取事件订阅/分发方法
  const { on } = useEvent();

  // 存储取消订阅函数
  let unlistenHashConflicts: (() => void) | null = null;
  let unlistenGameSwitched: (() => void) | null = null;

  // 防抖触发 hash 冲突检测（5 秒窗口，防止快速切换游戏导致的资源浪费）
  const debouncedCheck = debounce(async (game: TargetGame) => {
    log.debug('Debounced hash check triggered', { game: String(game), trigger: 'GAME_SWITCHED' });
    if (game === 'none') return;
    try {
      await hashConflictStore.checkHashConflicts();
    } catch (e) {
      log.warn('Debounced check failed', { reason: String(e) });
    }
  }, 5000);

  /**
   * 注册事件监听。
   *
   * 使用 `onBeforeUnmount` 钩子在组件卸载时自动清理，
   * 调用方无需手动管理 unlisten 句柄。
   */
  onMounted(async () => {
    // 监听 hash 冲突检测完成事件
    unlistenHashConflicts = await on(EventNames.HASH_CONFLICTS_DETECTED, (payload: { game: string; report: import('../types').HashConflictReport; completedAt: number }) => {
      try {
        hashConflictStore.setReport(payload.report);
      } catch (e) {
        log.error('Failed to handle HASH_CONFLICTS_DETECTED', e, { trigger: 'HASH_CONFLICTS_DETECTED' });
      }
    });

    // 监听游戏切换事件，5 秒防抖后触发 hash 检测
    unlistenGameSwitched = await on(EventNames.GAME_SWITCHED, (payload: { game: string }) => {
      try {
        debouncedCheck(payload.game as TargetGame);
      } catch (e) {
        log.error('Failed to handle GAME_SWITCHED', e, { trigger: 'GAME_SWITCHED' });
      }
    });
  });

  /**
   * 组件卸载时清理事件订阅与防抖定时器。
   */
  onBeforeUnmount(() => {
    // 取消防抖定时器
    try {
      debouncedCheck.cancel();
    } catch (e) {
      log.warn('Failed to cancel debounce', { reason: String(e) });
    }

    // 取消 HASH_CONFLICTS_DETECTED 监听
    if (unlistenHashConflicts) {
      try {
        unlistenHashConflicts();
      } catch (e) {
        log.warn('Failed to unlisten hash conflicts', { reason: String(e) });
      } finally {
        unlistenHashConflicts = null;
      }
    }

    // 取消 GAME_SWITCHED 监听
    if (unlistenGameSwitched) {
      try {
        unlistenGameSwitched();
      } catch (e) {
        log.warn('Failed to unlisten game switched', { reason: String(e) });
      } finally {
        unlistenGameSwitched = null;
      }
    }
  });
}
