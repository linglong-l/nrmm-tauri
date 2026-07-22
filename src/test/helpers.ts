/**
 * 测试辅助函数模块
 *
 * 提供可复用的测试数据构造器，供各 Store 测试文件使用。
 * 避免重复定义相同的 makeGroup / makeMod 辅助函数。
 */
import type { ModGroupData, ModData } from '../types';

/**
 * 构造最小化的测试用分组数据。
 * @param overrides 覆盖默认字段的对象
 * @returns 测试用 ModGroupData
 */
export function makeGroup(overrides: Partial<ModGroupData> = {}): ModGroupData {
  return {
    groupPath: '/test/group',
    iconPath: null,
    groupName: 'Test Group',
    favoriteDateTime: null,
    modsInGroup: [],
    realIndex: 1,
    previousSelectedModOnGroup: -1,
    children: [],
    isTreeNode: false,
    isVirtual: false,
    isDisabled: false,
    ...overrides,
  };
}

/**
 * 构造最小化的测试用 ModData。
 * @param overrides 覆盖默认字段的对象
 * @returns 测试用 ModData
 */
export function makeMod(overrides: Partial<ModData> = {}): ModData {
  return {
    modPath: '/mod',
    iconPath: null,
    modName: 'Mod',
    realIndex: 1,
    isOldAutoFixed: false,
    isSyntaxErrorRemoved: false,
    isUnoptimized: false,
    isNamespaced: false,
    isDisabled: false,
    favoriteDateTime: null,
    ...overrides,
  };
}