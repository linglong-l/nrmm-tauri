/**
 * useUiStore 单元测试
 *
 * 覆盖范围：
 * - 初始状态
 * - setActiveTab 切换标签页
 * - setLoading 加载状态
 * - setWindowVisible 窗口可见性
 * - setWindowPinned / toggleWindowPinned 置顶
 * - setTraySetup 托盘状态
 * - 对话框操作
 * - 通知操作
 * - 键盘输入状态
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { setActivePinia, createPinia } from 'pinia';
import { useUiStore } from '../ui';

describe('useUiStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it('初始状态 activeTab 已定义', () => {
    const store = useUiStore();
    expect(store.activeTab).toBeDefined();
    expect(typeof store.activeTab).toBe('string');
  });

  it('setActiveTab 切换标签页', () => {
    const store = useUiStore();
    store.setActiveTab('settings');
    expect(store.activeTab).toBe('settings');
  });

  it('isLoading 初始为 false', () => {
    const store = useUiStore();
    expect(store.isLoading).toBe(false);
  });

  it('setLoading 设置加载状态', () => {
    const store = useUiStore();
    store.setLoading(true, 'Loading...');
    expect(store.isLoading).toBe(true);
    expect(store.loadingMessage).toBe('Loading...');
    store.setLoading(false);
    expect(store.isLoading).toBe(false);
  });

  it('isWindowVisible 初始为 true', () => {
    const store = useUiStore();
    expect(store.isWindowVisible).toBe(true);
  });

  it('setWindowVisible 隐藏窗口', () => {
    const store = useUiStore();
    store.setWindowVisible(false);
    expect(store.isWindowVisible).toBe(false);
  });

  it('isWindowPinned 初始为 false', () => {
    const store = useUiStore();
    expect(store.isWindowPinned).toBe(false);
  });

  it('setWindowPinned 设置置顶', () => {
    const store = useUiStore();
    store.setWindowPinned(true);
    expect(store.isWindowPinned).toBe(true);
  });

  it('toggleWindowPinned 切换置顶', () => {
    const store = useUiStore();
    store.toggleWindowPinned();
    expect(store.isWindowPinned).toBe(true);
    store.toggleWindowPinned();
    expect(store.isWindowPinned).toBe(false);
  });

  it('isTraySetup 初始为 false', () => {
    const store = useUiStore();
    expect(store.isTraySetup).toBe(false);
  });

  it('setTraySetup 设置托盘状态', () => {
    const store = useUiStore();
    store.setTraySetup(true);
    expect(store.isTraySetup).toBe(true);
  });

  it('showDialog 打开对话框', () => {
    const store = useUiStore();
    store.showDialog('settings');
    expect(store.dialogs.settings).toBe(true);
  });

  it('hideDialog 关闭对话框', () => {
    const store = useUiStore();
    store.showDialog('settings');
    store.hideDialog('settings');
    expect(store.dialogs.settings).toBe(false);
  });

  it('addNotification 添加通知', () => {
    const store = useUiStore();
    const id = store.addNotification('info', 'Title', 'Message');
    expect(store.notifications.length).toBe(1);
    expect(store.notifications[0].title).toBe('Title');
    expect(typeof id).toBe('string');
  });

  it('clearNotifications 清空通知', () => {
    const store = useUiStore();
    store.addNotification('info', 'Title', 'Message');
    store.clearNotifications();
    expect(store.notifications.length).toBe(0);
  });

  it('setFocusedOnTextField 设置输入焦点状态', () => {
    const store = useUiStore();
    store.setFocusedOnTextField(true);
    expect(store.isFocusedOnTextField).toBe(true);
  });
});