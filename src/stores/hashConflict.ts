/**
 * Hash 冲突检测状态 Store
 *
 * 用于管理前端 hash 冲突检测的状态、报告与用户主动忽略的冲突集合。
 *
 * 状态流转：
 *  - idle: 初始状态，无正在进行的检测
 *  - checking: 检测中
 *  - cancelled: 检测被新请求取消
 *  - done: 检测完成
 *  - error: 检测失败
 *
 * 该 Store 在以下场景被调用：
 *  - 模组启用/禁用/更新/刷新后自动触发检测
 *  - 后端通过 HASH_CONFLICTS_DETECTED 事件推送结果时由 setReport 写入
 *  - 用户点击「忽略」按钮时通过 ignoreHash 过滤冲突
 */
import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import type { HashConflictReport, HashConflictEntry } from '../types';
import { invokeCheckHashConflicts } from '../utils/invoke';
import { createLogger } from '../utils/logger';

/** Hash 冲突检测状态枚举 */
export type HashConflictStatus =
  | 'idle'
  | 'checking'
  | 'cancelled'
  | 'done'
  | 'error';

export const useHashConflictStore = defineStore('hashConflict', () => {
  const log = createLogger('HashConflictStore');
  // 当前检测状态
  const status = ref<HashConflictStatus>('idle');
  // 最近一次检测报告（未经忽略过滤）
  const lastReport = ref<HashConflictReport | null>(null);
  // 检测失败时的错误信息
  const errorMessage = ref<string>('');
  // 用户主动忽略的 hash 集合（用于过滤弹层显示）
  // 使用 Set 但通过创建新实例触发响应式更新（参考项目约定）
  const ignoredHashes = ref<Set<string>>(new Set());

  /**
   * 过滤忽略后的冲突条目列表（响应式计算）。
   */
  const visibleConflicts = computed<HashConflictEntry[]>(() => {
    if (!lastReport.value) return [];
    const ignored = ignoredHashes.value;
    return lastReport.value.conflicts.filter((entry) => !ignored.has(entry.hash));
  });

  /**
   * 可见冲突条目数量。
   */
  const conflictCount = computed<number>(() => visibleConflicts.value.length);

  /**
   * 是否存在可见冲突（用于控制浮动入口组件的渲染）。
   */
  const hasConflicts = computed<boolean>(() => conflictCount.value > 0);

  /**
   * 调用后端执行独立 hash 冲突检测，并管理状态流转。
   *
   * 状态流转：idle -> checking -> done / cancelled / error
   * - 成功：status='done'，写入 lastReport
   * - 取消：status='cancelled'，保留 lastReport
   * - 失败：status='error'，写入 errorMessage
   */
  async function checkHashConflicts(): Promise<void> {
    log.debug('Hash conflict check started', { status: status.value });
    status.value = 'checking';
    errorMessage.value = '';
    try {
      const report = await invokeCheckHashConflicts();
      lastReport.value = report;
      status.value = 'done';
      log.debug('Hash conflict check completed', { status: status.value, conflictCount: conflictCount.value });
    } catch (e) {
      const message = String(e);
      if (message.includes('cancelled')) {
        status.value = 'cancelled';
        // 取消时保留 lastReport
      } else {
        errorMessage.value = message;
        status.value = 'error';
        log.error('Hash conflict check failed', e, { trigger: 'checkHashConflicts', suggestion: 'Retry or check backend logs' });
      }
    }
  }

  /**
   * 由事件回调调用，写入最新检测报告。
   *
   * 该方法不会改变 status（仅由 checkHashConflicts 控制状态流转），
   * 但若当前状态为 'idle'（如初次收到事件），则置为 'done'。
   */
  function setReport(report: HashConflictReport): void {
    lastReport.value = report;
    if (status.value === 'idle' || status.value === 'checking') {
      status.value = 'done';
    }
  }

  /**
   * 将指定 hash 加入忽略集合。
   *
   * 通过创建新 Set 实例触发响应式更新（避免原地修改导致 UI 不刷新）。
   * 忽略后若所有冲突都被忽略，hasConflicts 变 false，浮动按钮自动隐藏。
   *
   * @param hash 待忽略的冲突 hash
   */
  function ignoreHash(hash: string): void {
    if (ignoredHashes.value.has(hash)) return;
    const next = new Set(ignoredHashes.value);
    next.add(hash);
    ignoredHashes.value = next;
  }

  /**
   * 取消某个 hash 的忽略状态。
   */
  function unignoreHash(hash: string): void {
    if (!ignoredHashes.value.has(hash)) return;
    const next = new Set(ignoredHashes.value);
    next.delete(hash);
    ignoredHashes.value = next;
  }

  /**
   * 清空所有忽略状态，恢复显示全部冲突。
   */
  function clearIgnored(): void {
    if (ignoredHashes.value.size === 0) return;
    ignoredHashes.value = new Set();
  }

  /**
   * 重置整个 store 状态（用于测试或主动刷新场景）。
   */
  function reset(): void {
    status.value = 'idle';
    lastReport.value = null;
    errorMessage.value = '';
    ignoredHashes.value = new Set();
  }

  return {
    status,
    lastReport,
    errorMessage,
    ignoredHashes,
    visibleConflicts,
    conflictCount,
    hasConflicts,
    checkHashConflicts,
    setReport,
    ignoreHash,
    unignoreHash,
    clearIgnored,
    reset
  };
});
