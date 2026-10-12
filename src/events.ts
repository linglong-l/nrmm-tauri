/**
 * 前后端 IPC 事件契约（唯一登记处）
 *
 * 镜像 src-tauri/src/core/events.rs；两侧集合相等性由后端 arch_gate F5 测试
 * 强校验（新增事件须双侧同步登记）。所有 listen 调用必须引用 IpcEvent 常量。
 */
export const IpcEvent = {
  /** 目标游戏切换（payload: TargetGame） */
  TargetGameSwitched: 'target-game-switched',
  /** 主窗口已显示 */
  WindowShown: 'window-shown',
  /** 主窗口已隐藏 */
  WindowHidden: 'window-hidden',
  /** 主窗口隐藏且需清理数据 */
  WindowHiddenClearData: 'window-hidden-clear-data',
  /** 显示主窗口并切换到检测到的游戏 */
  WindowShowWithGame: 'window-show-with-game',
  /** _MANAGED_ 目录发生变化（全量刷新兜底） */
  ManagedFolderChanged: 'managed-folder-changed',
  /** _MANAGED_ 目录增量更新（GroupDelta 批次） */
  ManagedPartialUpdate: 'managed-partial-update',
  /** 游戏外快捷键：显示游戏选择菜单（当前无前端监听，登记备用） */
  ShowGameSelect: 'show-game-select',
} as const
