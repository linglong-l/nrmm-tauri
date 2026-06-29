/**
 * Store 模块统一导出入口
 *
 * 作用：
 *  - 集中导出所有 Pinia Store，便于组件按需引入
 *  - 包含：设置、UI 状态、游戏数据、热键、云数据等核心状态管理模块
 */

export { useSettingsStore } from './settings';
export { useUiStore } from './ui';
export { useGameStore } from './game';
export { useHotkeyStore } from './hotkey';
export { useCloudDataStore } from './cloudData';
