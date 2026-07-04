/**
 * 快捷键校验工具模块
 *
 * 提供快捷键冲突检测功能，确保窗口切换、分组搜索、模组搜索三类快捷键之间不重复。
 */

/** 快捷键冲突信息 */
export interface HotkeyConflict {
  /** 冲突的快捷键值（如 "altF"） */
  keys: string[];
  /** 冲突描述消息 */
  message: string;
}

/** 快捷键校验结果 */
export interface HotkeyValidationResult {
  /** 是否有效（无冲突） */
  valid: boolean;
  /** 冲突列表（valid 为 false 时非空） */
  conflicts: HotkeyConflict[];
}

/**
 * 校验三类快捷键之间是否存在冲突。
 *
 * @param windowHotkey 窗口切换快捷键值（如 "altW"）
 * @param groupSearchHotkey 分组搜索快捷键值（如 "altG"）
 * @param modSearchHotkey 模组搜索快捷键值（如 "altF"）
 * @returns 校验结果，含冲突信息
 */
export function validateHotkeys(
  windowHotkey: string,
  groupSearchHotkey: string,
  modSearchHotkey: string
): HotkeyValidationResult {
  const conflicts: HotkeyConflict[] = [];

  // 检测窗口切换与分组搜索冲突
  if (windowHotkey !== 'none' && windowHotkey === groupSearchHotkey) {
    conflicts.push({
      keys: [windowHotkey, groupSearchHotkey],
      message: `快捷键冲突：${hotkeyToDisplayName(windowHotkey)} 被窗口切换和分组搜索同时使用`,
    });
  }

  // 检测窗口切换与模组搜索冲突
  if (windowHotkey !== 'none' && windowHotkey === modSearchHotkey) {
    conflicts.push({
      keys: [windowHotkey, modSearchHotkey],
      message: `快捷键冲突：${hotkeyToDisplayName(windowHotkey)} 被窗口切换和模组搜索同时使用`,
    });
  }

  // 检测分组搜索与模组搜索冲突
  if (groupSearchHotkey === modSearchHotkey) {
    conflicts.push({
      keys: [groupSearchHotkey, modSearchHotkey],
      message: `快捷键冲突：${hotkeyToDisplayName(groupSearchHotkey)} 被分组搜索和模组搜索同时使用`,
    });
  }

  return {
    valid: conflicts.length === 0,
    conflicts,
  };
}

/**
 * 将内部快捷键标识转换为显示名称。
 *
 * @param hotkey 快捷键内部标识（如 "altF"）
 * @returns 显示名称（如 "Alt+F"），未知值原样返回
 */
function hotkeyToDisplayName(hotkey: string): string {
  if (hotkey.startsWith('alt') && hotkey.length === 4) {
    return `Alt+${hotkey[3].toUpperCase()}`;
  }
  return hotkey;
}
