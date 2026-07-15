// 全局事件总线模块。
// 该模块基于 Tauri 事件系统封装了一层类型安全的事件订阅/分发机制，
// 同时保留了对前端进程内（非跨进程）自定义事件的支持，便于在组件解耦场景下通信。
import { listen, emit as tauriEmit, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  FileWatcherEvent,
  ModGroupData,
  Notification,
  WindowPosition,
  CloudData,
  IniSyntaxError
} from '../types';
import type { HashConflictReport } from '../types';

/**
 * 全局事件名称常量集合。
 * 这些名称既用于 Tauri 跨进程事件，也用于前端进程内自定义事件，
 * 通过统一命名避免硬编码字符串导致的拼写错误。
 */
export const EventNames = {
  /** Mod 列表已更新（数据刷新） */
  MODS_UPDATED: 'mods-updated',
  /** Mod 分组列表已更新 */
  MOD_GROUPS_UPDATED: 'mod-groups-updated',
  /** 文件监听器捕获到一次文件系统变化（与后端 file_watcher 的 emit 名称一致） */
  FILE_WATCHER_EVENT: 'mods-directory-changed',
  /** 全局热键被按下 */
  HOTKEY_PRESSED: 'hotkey-pressed',
  /** 热键注册结果（成功/失败） */
  HOTKEY_REGISTERED: 'hotkey-registered',
  /** 热键注销结果（成功/失败） */
  HOTKEY_UNREGISTERED: 'hotkey-unregistered',
  /** 主窗口已显示 */
  WINDOW_SHOWN: 'window-shown',
  /** 主窗口已隐藏 */
  WINDOW_HIDDEN: 'window-hidden',
  /** 主窗口位置已变化 */
  WINDOW_POSITION_CHANGED: 'window-position-changed',
  /** 主窗口尺寸已变化 */
  WINDOW_SIZE_CHANGED: 'window-size-changed',
  /** 系统托盘菜单项被点击 */
  TRAY_MENU_CLICKED: 'tray-menu-clicked',
  /** 云端数据已更新 */
  CLOUD_DATA_UPDATED: 'cloud-data-updated',
  /** 应用设置已更新 */
  SETTINGS_UPDATED: 'settings-updated',
  /** 新通知产生 */
  NOTIFICATION: 'notification',
  /** INI 文件中发现语法错误 */
  INI_ERRORS_FOUND: 'ini-errors-found',
  /** 后台任务开始执行 */
  TASK_STARTED: 'task-started',
  /** 后台任务执行完成 */
  TASK_COMPLETED: 'task-completed',
  /** 后台任务执行失败 */
  TASK_FAILED: 'task-failed',
  /** 目标进程已启动 */
  PROCESS_STARTED: 'process-started',
  /** 目标进程已退出 */
  PROCESS_STOPPED: 'process-stopped',
  /** 当前目标游戏已切换 */
  GAME_SWITCHED: 'game-switched',
  /** Hash 冲突检测完成（与后端 HASH_CONFLICTS_DETECTED_EVENT 常量保持一致） */
  HASH_CONFLICTS_DETECTED: 'hash-conflicts-detected'
} as const;

/** 事件名称类型，取自 EventNames 常量的所有值的联合类型。 */
export type EventName = typeof EventNames[keyof typeof EventNames];

/**
 * 事件名到其对应载荷类型的映射表。
 * 该映射为事件订阅/分发提供类型安全的载荷推断，避免在使用处手动断言类型。
 */
export interface EventPayloadMap {
  /** Mod 列表更新事件载荷：分组数据数组 */
  [EventNames.MODS_UPDATED]: ModGroupData[];
  /** Mod 分组更新事件载荷：分组数据数组 */
  [EventNames.MOD_GROUPS_UPDATED]: ModGroupData[];
  /** 文件监听事件载荷：单次文件系统变化信息 */
  [EventNames.FILE_WATCHER_EVENT]: FileWatcherEvent;
  /** 热键按下事件载荷：按键标识、来源（游戏中/游戏外）、时间戳与匹配的游戏名 */
  [EventNames.HOTKEY_PRESSED]: { key: string; source: 'in-game' | 'outside-game'; timestamp?: number; matchedGame?: string };
  /** 热键注册结果载荷：按键标识与是否成功 */
  [EventNames.HOTKEY_REGISTERED]: { key: string; success: boolean };
  /** 热键注销结果载荷：按键标识与是否成功 */
  [EventNames.HOTKEY_UNREGISTERED]: { key: string; success: boolean };
  /** 窗口显示事件载荷：可见状态与来源 */
  [EventNames.WINDOW_SHOWN]: { visible: boolean; source: string };
  /** 窗口隐藏事件载荷：可见状态与来源 */
  [EventNames.WINDOW_HIDDEN]: { visible: boolean; source: string };
  /** 窗口位置变化载荷：完整位置与尺寸信息 */
  [EventNames.WINDOW_POSITION_CHANGED]: WindowPosition;
  /** 窗口尺寸变化载荷：宽高 */
  [EventNames.WINDOW_SIZE_CHANGED]: { width: number; height: number };
  /** 托盘菜单点击载荷：被点击项的 id */
  [EventNames.TRAY_MENU_CLICKED]: { id: string };
  /** 云端数据更新载荷：云端数据聚合对象 */
  [EventNames.CLOUD_DATA_UPDATED]: CloudData;
  /** 设置更新事件载荷：无 */
  [EventNames.SETTINGS_UPDATED]: void;
  /** 通知事件载荷：通知对象 */
  [EventNames.NOTIFICATION]: Notification;
  /** INI 错误发现载荷：文件路径与错误列表 */
  [EventNames.INI_ERRORS_FOUND]: { filePath: string; errors: IniSyntaxError[] };
  /** 任务开始载荷：任务名称 */
  [EventNames.TASK_STARTED]: { taskName: string };
  /** 任务完成载荷：任务名称与可选结果 */
  [EventNames.TASK_COMPLETED]: { taskName: string; result?: unknown };
  /** 任务失败载荷：任务名称与错误信息 */
  [EventNames.TASK_FAILED]: { taskName: string; error: string };
  /** 进程启动载荷：进程名 */
  [EventNames.PROCESS_STARTED]: { processName: string };
  /** 进程退出载荷：进程名 */
  [EventNames.PROCESS_STOPPED]: { processName: string };
  /** 游戏切换载荷：游戏标识字符串 */
  [EventNames.GAME_SWITCHED]: { game: string };
  /** Hash 冲突检测完成（由后端在 toggle_mod_disabled 等操作完成后 emit） */
  [EventNames.HASH_CONFLICTS_DETECTED]: {
    game: string;
    report: HashConflictReport;
    completedAt: number;
  };
}

/** 通用事件回调类型，接收一个载荷参数。 */
type EventCallback<T = unknown> = (payload: T) => void;

/**
 * 事件管理器单例类。
 *
 * 同时管理两类监听：
 * 1. Tauri 跨进程事件监听（通过 `listen` 注册，存储于 `listeners`），用于接收后端 emit 的事件；
 * 2. 前端进程内自定义事件监听（存储于 `customListeners`），用于可靠接收前端 emit 的内部事件。
 *
 * 在 `on` 中会同时注册两种监听，确保：
 * - 后端 emit 的事件能通过 Tauri 监听接收到；
 * - 前端 emit 的事件能通过自定义监听同步、可靠地接收到。
 * 两种监听机制互不干扰，各自独立。
 */
class EventManager {
  /** Tauri 跨进程事件监听的取消函数集合，按事件名分组 */
  private listeners: Map<string, UnlistenFn[]> = new Map();
  /** 前端进程内自定义事件监听集合，按事件名分组 */
  private customListeners: Map<string, Set<EventCallback>> = new Map();

  /**
   * 订阅指定事件。
   * 同时注册 Tauri 跨进程监听和前端进程内自定义监听：
   * - Tauri 监听用于接收后端 emit 的事件；
   * - 自定义监听用于可靠接收前端 emit 的内部事件（同步触发，无延迟）。
   * 两种监听各自独立，不会重复触发同一回调。
   * @param event 事件名称
   * @param callback 事件回调（接收类型安全的载荷）
   * @returns 取消订阅函数，调用后移除该监听
   */
  async on<T extends EventName>(
    event: T,
    callback: (payload: EventPayloadMap[T]) => void
  ): Promise<() => void> {
    const removeCustom = this.addCustomListener(event, callback as EventCallback);

    let removeTauri: (() => void) | null = null;
    try {
      const unlisten = await listen(event, (eventData) => {
        callback(eventData.payload as EventPayloadMap[T]);
      });

      if (!this.listeners.has(event)) {
        this.listeners.set(event, []);
      }
      this.listeners.get(event)!.push(unlisten);
      removeTauri = () => {
        this.removeTauriListener(event, unlisten);
      };
    } catch {
      // Tauri 监听注册失败时忽略，自定义监听仍已注册
    }

    return () => {
      if (removeTauri) {
        removeTauri();
      }
      removeCustom();
    };
  }

  /**
   * 触发指定事件（同时触发 Tauri 跨进程事件和前端进程内的自定义监听）。
   * 用于需要后端接收的事件：
   * - Tauri 事件供后端和通过 Tauri listen 注册的监听器接收；
   * - 自定义监听供前端内部使用，同步触发、可靠无延迟。
   * @param event 事件名称
   * @param payload 事件载荷
   */
  async emit<T extends EventName>(event: T, payload: EventPayloadMap[T]): Promise<void> {
    // 触发 Tauri 跨进程事件（供后端和通过 listen 注册的监听器接收）
    try {
      await tauriEmit(event, payload);
    } catch {
      // Tauri 环境不可用时忽略错误（如非 Tauri 环境或测试环境）
    }

    // 触发前端进程内的自定义监听（同步触发，可靠无延迟）
    const callbacks = this.customListeners.get(event);
    if (callbacks) {
      callbacks.forEach((cb) => {
        try {
          cb(payload);
        } catch {
          // 单个回调异常不影响其他回调，忽略
        }
      });
    }
  }

  /**
   * 触发前端内部事件（仅触发自定义监听，不触发 Tauri 跨进程事件）。
   * 用于前端组件间通信，避免 Tauri listen 监听器重复触发。
   * @param event 事件名称
   * @param payload 事件载荷
   */
  emitLocal<T extends EventName>(event: T, payload: EventPayloadMap[T]): void {
    const callbacks = this.customListeners.get(event);
    if (callbacks) {
      callbacks.forEach((cb) => {
        try {
          cb(payload);
        } catch {
          // 单个回调异常不影响其他回调
        }
      });
    }
  }

  /**
   * 注册一个前端进程内自定义监听。
   * @param event 事件名称
   * @param callback 事件回调
   * @returns 取消订阅函数，调用后移除该监听
   */
  private addCustomListener(event: string, callback: EventCallback): () => void {
    if (!this.customListeners.has(event)) {
      this.customListeners.set(event, new Set());
    }
    this.customListeners.get(event)!.add(callback);

    return () => {
      this.customListeners.get(event)?.delete(callback);
    };
  }

  /**
   * 移除一个已注册的 Tauri 监听。
   * @param event 事件名称
   * @param unlisten 该监听对应的取消函数
   */
  private removeTauriListener(event: string, unlisten: UnlistenFn): void {
    const listeners = this.listeners.get(event);
    if (listeners) {
      const index = listeners.indexOf(unlisten);
      if (index > -1) {
        listeners.splice(index, 1);
      }
      try {
        unlisten();
      } catch {
        // 取消监听失败时忽略
      }
    }
  }

  /**
   * 移除所有已注册的事件监听（包括 Tauri 监听与自定义监听）。
   * 通常在应用卸载或重置场景下调用，避免内存泄漏。
   */
  removeAllListeners(): void {
    this.listeners.forEach((listeners) => {
      listeners.forEach((unlisten) => {
        try {
          unlisten();
        } catch {
          // 忽略
        }
      });
    });
    this.listeners.clear();
    this.customListeners.clear();
  }
}

/** 全局事件管理器单例，供整个应用共享使用。 */
export const eventManager = new EventManager();

/**
 * 事件订阅/触发的组合式函数（composable）。
 *
 * 返回 `on` 与 `emit` 两个方法，分别代理到全局 `eventManager` 的同名方法。
 * 适合在 Vue 组件或 composable 中使用，以便在保持类型安全的同时简化事件访问。
 *
 * @returns 包含 `on` 与 `emit` 的对象
 */
export function useEvent(): {
  on: <T extends EventName>(
    event: T,
    callback: (payload: EventPayloadMap[T]) => void
  ) => Promise<() => void>;
  emit: <T extends EventName>(event: T, payload: EventPayloadMap[T]) => void;
} {
  return {
    on: async (event, callback) => {
      return eventManager.on(event, callback);
    },
    emit: (event, payload) => {
      eventManager.emit(event, payload);
    }
  };
}
