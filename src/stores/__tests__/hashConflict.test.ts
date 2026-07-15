/**
 * hashConflictStore 单元测试
 *
 * 覆盖范围：
 * - 初始状态：status=idle, lastReport=null, conflictCount=0, hasConflicts=false
 * - checkHashConflicts 成功：status=done, lastReport 填充
 * - checkHashConflicts 失败：status=error, errorMessage 填充
 * - checkHashConflicts 取消：status=cancelled, lastReport 保留
 * - ignoreHash：忽略后 conflictCount 减少, ignoredHashes 包含
 * - visibleConflicts：computed 正确过滤 ignoredHashes
 * - setReport：事件回调写入
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { setActivePinia, createPinia } from 'pinia';
import { useHashConflictStore } from '../hashConflict';
import { invokeCheckHashConflicts } from '../../utils/invoke';
import type { HashConflictReport } from '../../types';

// Mock invoke module
vi.mock('../../utils/invoke', () => ({
  invokeCheckHashConflicts: vi.fn()
}));

const mockedInvokeCheckHashConflicts = vi.mocked(invokeCheckHashConflicts);

/**
 * 构造测试用冲突报告
 */
function makeReport(conflictCount: number): HashConflictReport {
  const conflicts = Array.from({ length: conflictCount }, (_, i) => ({
    hash: `hash-${i}-${'a'.repeat(40)}`,
    modNames: [`mod_a_${i}`, `mod_b_${i}`],
    modPaths: [`/path/a/${i}`, `/path/b/${i}`],
    groupNames: [`group_${i}`, `group_${i}`]
  }));
  return {
    enabledModHashes: {},
    namespaceHashes: {},
    conflicts
  };
}

describe('hashConflictStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  it('初始状态为 idle，lastReport 为 null，conflictCount 为 0', () => {
    const store = useHashConflictStore();
    expect(store.status).toBe('idle');
    expect(store.lastReport).toBeNull();
    expect(store.conflictCount).toBe(0);
    expect(store.hasConflicts).toBe(false);
    expect(store.ignoredHashes.size).toBe(0);
  });

  it('checkHashConflicts 成功时状态变 done，lastReport 正确填充', async () => {
    const report = makeReport(2);
    mockedInvokeCheckHashConflicts.mockResolvedValueOnce(report);
    const store = useHashConflictStore();
    await store.checkHashConflicts();
    expect(store.status).toBe('done');
    expect(store.lastReport).toEqual(report);
    expect(store.conflictCount).toBe(2);
    expect(store.hasConflicts).toBe(true);
  });

  it('checkHashConflicts 失败时状态变 error，errorMessage 填充', async () => {
    const errMessage = 'Some backend error';
    mockedInvokeCheckHashConflicts.mockRejectedValueOnce(new Error(errMessage));
    const store = useHashConflictStore();
    await store.checkHashConflicts();
    expect(store.status).toBe('error');
    // String(Error) 包含 'Error: ' 前缀，但应至少包含原始消息
    expect(store.errorMessage).toContain(errMessage);
  });

  it('checkHashConflicts 取消时状态变 cancelled，lastReport 保留', async () => {
    const report = makeReport(1);
    // 第一次成功
    mockedInvokeCheckHashConflicts.mockResolvedValueOnce(report);
    const store = useHashConflictStore();
    await store.checkHashConflicts();
    expect(store.lastReport).toEqual(report);
    // 第二次被取消
    mockedInvokeCheckHashConflicts.mockRejectedValueOnce(
      new Error("Task 'check_hash_conflicts' was cancelled")
    );
    await store.checkHashConflicts();
    expect(store.status).toBe('cancelled');
    // 取消时保留 lastReport
    expect(store.lastReport).toEqual(report);
  });

  it('ignoreHash 后 conflictCount 减少，ignoredHashes 包含该 hash', async () => {
    const report = makeReport(3);
    mockedInvokeCheckHashConflicts.mockResolvedValueOnce(report);
    const store = useHashConflictStore();
    await store.checkHashConflicts();
    expect(store.conflictCount).toBe(3);
    // 忽略第一个冲突
    const firstHash = report.conflicts[0].hash;
    store.ignoreHash(firstHash);
    expect(store.ignoredHashes.has(firstHash)).toBe(true);
    expect(store.conflictCount).toBe(2);
    // 忽略全部后 hasConflicts = false
    store.ignoreHash(report.conflicts[1].hash);
    store.ignoreHash(report.conflicts[2].hash);
    expect(store.conflictCount).toBe(0);
    expect(store.hasConflicts).toBe(false);
  });

  it('ignoreHash 已存在的 hash 时不会重复添加', async () => {
    const store = useHashConflictStore();
    store.ignoreHash('duplicate-hash');
    expect(store.ignoredHashes.size).toBe(1);
    store.ignoreHash('duplicate-hash');
    expect(store.ignoredHashes.size).toBe(1);
  });

  it('unignoreHash 解除忽略后 conflictCount 恢复', async () => {
    const report = makeReport(1);
    mockedInvokeCheckHashConflicts.mockResolvedValueOnce(report);
    const store = useHashConflictStore();
    await store.checkHashConflicts();
    const hash = report.conflicts[0].hash;
    store.ignoreHash(hash);
    expect(store.conflictCount).toBe(0);
    store.unignoreHash(hash);
    expect(store.ignoredHashes.size).toBe(0);
    expect(store.conflictCount).toBe(1);
  });

  it('clearIgnored 清空忽略集合后 conflictCount 恢复', async () => {
    const report = makeReport(2);
    mockedInvokeCheckHashConflicts.mockResolvedValueOnce(report);
    const store = useHashConflictStore();
    await store.checkHashConflicts();
    store.ignoreHash(report.conflicts[0].hash);
    store.ignoreHash(report.conflicts[1].hash);
    expect(store.conflictCount).toBe(0);
    store.clearIgnored();
    expect(store.ignoredHashes.size).toBe(0);
    expect(store.conflictCount).toBe(2);
  });

  it('setReport 写入报告并设置 status 为 done', () => {
    const store = useHashConflictStore();
    const report = makeReport(1);
    store.setReport(report);
    expect(store.lastReport).toEqual(report);
    expect(store.status).toBe('done');
  });

  it('reset 重置所有状态', async () => {
    const report = makeReport(1);
    mockedInvokeCheckHashConflicts.mockResolvedValueOnce(report);
    const store = useHashConflictStore();
    await store.checkHashConflicts();
    store.ignoreHash(report.conflicts[0].hash);
    store.reset();
    expect(store.status).toBe('idle');
    expect(store.lastReport).toBeNull();
    expect(store.ignoredHashes.size).toBe(0);
  });
});
