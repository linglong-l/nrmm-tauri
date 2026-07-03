/**
 * Vitest 全局测试设置
 *
 * 在每个测试文件执行前运行，负责：
 * 1. 创建 Pinia 实例（测试中需手动 setActivePinia）
 * 2. 重置 localStorage 模拟状态
 * 3. Mock Tauri invoke 调用（避免测试依赖真实后端）
 */
import { createPinia, setActivePinia } from 'pinia';
import { vi, beforeEach } from 'vitest';

// 每个测试前重置 Pinia 实例，确保 store 状态隔离
beforeEach(() => {
  setActivePinia(createPinia());
});

// 每个测试前清空 localStorage，避免测试间状态污染
beforeEach(() => {
  localStorage.clear();
});

/**
 * Mock Tauri invoke 函数。
 *
 * 测试中通过 `vi.mocked(invoke)` 或直接在测试用例内 `vi.fn()` 覆盖具体命令的返回值。
 * 默认抛出错误，强制测试显式声明期望的调用与返回值。
 */
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockRejectedValue(new Error('invoke not mocked in this test')),
}));

/**
 * Mock Tauri 事件监听 API，避免测试中实际注册跨进程监听器。
 */
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
  emit: vi.fn().mockResolvedValue(undefined),
}));
