import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import type { CloudData, CloudLinks, CloudMessages, AutoIconData } from '../types';
import { CONSTANTS } from '../utils/constants';
import { invokeFetchCloudData, invokeSyncCloudData, invokeGetCloudData } from '../utils/invoke';
import { EventNames, eventManager } from '../utils/events';
import { createLogger } from '../utils/logger';

/**
 * 云数据 Store
 *
 * 管理从远端（Gitee 仓库）拉取的各类动态数据，包括：
 * - 支持链接 / 教程链接 / 联系链接及其图标（默认图标先内置，链接后由云端填充）
 * - 各游戏的公告消息（wuwa / genshin / hsr / zzz / endfield）
 * - 各游戏的自动图标数据（用于自动为模组匹配图标）
 * - 已知 Mod 命名空间到友好名称的映射表（用于展示更可读的库名）
 *
 * 拉取/同步通过 invoke 调用 Tauri 后端完成；后端返回的最新数据会再被读取并合并入前端状态。
 * 同时维护加载、同步、错误等运行时状态，便于 UI 反馈。
 */
export const useCloudDataStore = defineStore('cloudData', () => {
  const log = createLogger('CloudDataStore');
  // 支持 / 教程 / 联系三类入口的图标与跳转链接。图标先以内置 CONSTANTS 默认值填充，链接默认空。
  const links = ref<CloudLinks>({
    supportIcon: CONSTANTS.cloudDataUrls.supportIcon,
    supportIconOnHover: CONSTANTS.cloudDataUrls.supportIconOnHover,
    supportLink: '',
    tutorialIcon: CONSTANTS.cloudDataUrls.tutorialIcon,
    tutorialIconOnHover: CONSTANTS.cloudDataUrls.tutorialIconOnHover,
    tutorialLink: '',
    contactIcon: CONSTANTS.cloudDataUrls.contactIcon,
    contactIconOnHover: CONSTANTS.cloudDataUrls.contactIconOnHover,
    contactLink: ''
  });

  // 各游戏的公告消息文本，默认均为空字符串，由云端数据填充。
  const messages = ref<CloudMessages>({
    wuwa: '',
    genshin: '',
    hsr: '',
    zzz: '',
    endfield: ''
  });

  // 各游戏的自动图标条目列表，默认均为空数组。
  const autoIcons = ref<AutoIconData>({
    wuwa: [],
    genshin: [],
    hsr: [],
    zzz: [],
    endfield: []
  });

  // 已知 Mod 命名空间 -> 友好名称 的映射，先以内置 CONSTANTS.knownModdingLibraries 作为兜底。
  const knownModLibraries = ref<Record<string, string>>({ ...CONSTANTS.knownModdingLibraries });
  // 云数据是否已成功加载过至少一次。
  const isCloudDataLoaded = ref(false);
  // 是否正在向云端同步数据（invokeSyncCloudData 进行中）。
  const isSyncing = ref(false);
  // 是否正在拉取云端数据（invokeFetchCloudData 进行中）。
  const isFetching = ref(false);
  // 最近一次成功拉取云数据的时间戳；尚未拉取过时为 null。
  const lastFetchTime = ref<number | null>(null);
  // 最近一次拉取失败的错误信息；无错误时为 null。
  const fetchError = ref<string | null>(null);

  /** 支持（赞助）入口的图标 URL。 */
  const supportIcon = computed(() => links.value.supportIcon);
  /** 支持（赞助）入口的跳转链接。 */
  const supportLink = computed(() => links.value.supportLink);
  /** 教程入口的跳转链接。 */
  const tutorialLink = computed(() => links.value.tutorialLink);
  /** 联系入口的跳转链接。 */
  const contactLink = computed(() => links.value.contactLink);
  /** 已知 Mod 库映射的 JSON 字符串形式，便于做变更对比或传递。 */
  const updatedKnownModdingLibs = computed(() => JSON.stringify(knownModLibraries.value));

  /**
   * 以浅合并方式将云端数据合并到当前状态。
   * 各子对象（links / messages / autoIcons / knownModLibraries）按字段分别合并，
   * 仅更新传入部分，未传入字段保持原值。
   * @param data 部分云数据字段
   */
  function setCloudData(data: Partial<CloudData>) {
    if (data.links) {
      links.value = { ...links.value, ...data.links };
    }
    if (data.messages) {
      messages.value = { ...messages.value, ...data.messages };
    }
    if (data.autoIcons) {
      autoIcons.value = { ...autoIcons.value, ...data.autoIcons };
    }
    if (data.knownModLibraries) {
      knownModLibraries.value = { ...knownModLibraries.value, ...data.knownModLibraries };
    }
  }

  /** 设置云数据是否已加载完成的标志。 */
  function setCloudDataLoaded(loaded: boolean) {
    isCloudDataLoaded.value = loaded;
  }

  /** 设置是否正在同步云数据。 */
  function setSyncing(syncing: boolean) {
    isSyncing.value = syncing;
  }

  /** 设置是否正在拉取云数据。 */
  function setFetching(fetching: boolean) {
    isFetching.value = fetching;
  }

  /** 设置最近一次拉取云数据的时间戳。 */
  function setLastFetchTime(time: number) {
    lastFetchTime.value = time;
  }

  /** 设置最近一次拉取失败的错误信息，传 null 表示清除错误。 */
  function setFetchError(error: string | null) {
    fetchError.value = error;
  }

  /**
   * 从云端拉取数据。
   * 业务逻辑：
   * 1. 置 isFetching 为 true 并清空之前的错误；
   * 2. 调用 invokeFetchCloudData 触发后端拉取；
   * 3. 再调用 invokeGetCloudData 读取后端已缓存的最新数据并合并入状态（读取失败时静默忽略，仅保留 fetch 的副作用）；
   * 4. 标记 isCloudDataLoaded 为 true 并记录 lastFetchTime；
   * 5. 广播 CLOUD_DATA_UPDATED 事件，通知其它模块刷新；
   * 6. 拉取过程抛错时记录 fetchError 并返回 false；
   * 7. finally 中复位 isFetching。
   * @returns 是否拉取成功
   */
  async function fetchCloudData(): Promise<boolean> {
    isFetching.value = true;
    fetchError.value = null;
    try {
      const fetchResult = await invokeFetchCloudData();
      if (!fetchResult.ok) {
        log.error(`Failed to fetch cloud data: ${fetchResult.error}`);
        return false;
      }
      try {
        // 后端拉取完成后，再次读取后端缓存的数据并合并到前端状态
        const getResult = await invokeGetCloudData();
        if (getResult.ok) {
          setCloudData(getResult.data);
        } else {
          log.error(`Failed to get cloud data after fetch: ${getResult.error}`);
        }
      } catch {
        // 读取阶段失败不影响整体流程，忽略即可
        // ignore
      }
      isCloudDataLoaded.value = true;
      lastFetchTime.value = Date.now();
      eventManager.emit(EventNames.CLOUD_DATA_UPDATED, {
        links: links.value,
        messages: messages.value,
        autoIcons: autoIcons.value,
        knownModLibraries: knownModLibraries.value
      });
      return true;
    } catch (error) {
      fetchError.value = error instanceof Error ? error.message : 'Unknown error';
      return false;
    } finally {
      isFetching.value = false;
    }
  }

  /**
   * 同步云端数据。
   * 业务逻辑：
   * 1. 置 isSyncing 为 true；
   * 2. 调用 invokeSyncCloudData 触发后端同步；
   * 3. 再调用 invokeGetCloudData 读取同步后的最新数据并合并（读取失败时静默忽略）；
   * 4. 标记 isCloudDataLoaded 为 true；
   * 5. 同步失败返回 false（不抛出），finally 中复位 isSyncing。
   *
   * 与 fetchCloudData 的区别：sync 通常用于主动同步本地变更或重新对齐远端，
   * 不会更新 lastFetchTime 与 fetchError。
   * @returns 是否同步成功
   */
  async function syncCloudData(): Promise<boolean> {
    isSyncing.value = true;
    try {
      const syncResult = await invokeSyncCloudData();
      if (!syncResult.ok) {
        log.error(`Failed to sync cloud data: ${syncResult.error}`);
        return false;
      }
      try {
        const getResult = await invokeGetCloudData();
        if (getResult.ok) {
          setCloudData(getResult.data);
        } else {
          log.error(`Failed to get cloud data after sync: ${getResult.error}`);
        }
      } catch {
        // 读取阶段失败不影响整体流程，忽略即可
        // ignore
      }
      isCloudDataLoaded.value = true;
      return true;
    } catch {
      return false;
    } finally {
      isSyncing.value = false;
    }
  }

  /**
   * 根据游戏标识获取对应的云端公告消息。
   * 同时兼容完整枚举名（如 'Wuthering_Waves'）与短名（如 'wuwa'）。
   * @param game 游戏标识
   * @returns 公告文本；未匹配时返回空字符串
   */
  function getMessageForGame(game: string): string {
    switch (game) {
      case 'Wuthering_Waves':
      case 'wuwa':
        return messages.value.wuwa;
      case 'Genshin_Impact':
      case 'genshin':
        return messages.value.genshin;
      case 'Honkai_Star_Rail':
      case 'hsr':
        return messages.value.hsr;
      case 'Zenless_Zone_Zero':
      case 'zzz':
        return messages.value.zzz;
      case 'Arknights_Endfield':
      case 'endfield':
        return messages.value.endfield;
      default:
        return '';
    }
  }

  /**
   * 根据游戏标识获取对应的自动图标条目列表。
   * 同时兼容完整枚举名与短名。
   * @param game 游戏标识
   * @returns 自动图标条目数组；未匹配时返回空数组
   */
  function getAutoIconsForGame(game: string) {
    switch (game) {
      case 'Wuthering_Waves':
      case 'wuwa':
        return autoIcons.value.wuwa;
      case 'Genshin_Impact':
      case 'genshin':
        return autoIcons.value.genshin;
      case 'Honkai_Star_Rail':
      case 'hsr':
        return autoIcons.value.hsr;
      case 'Zenless_Zone_Zero':
      case 'zzz':
        return autoIcons.value.zzz;
      case 'Arknights_Endfield':
      case 'endfield':
        return autoIcons.value.endfield;
      default:
        return [];
    }
  }

  /**
   * 根据 Mod 命名空间返回其友好显示名称。
   * 查找时命名空间统一转小写匹配；未命中时原样返回 namespace。
   * @param namespace Mod 命名空间字符串
   * @returns 友好名称或原命名空间
   */
  function getModLibraryDisplayName(namespace: string): string {
    const lowerNamespace = namespace.toLowerCase();
    return knownModLibraries.value[lowerNamespace] || namespace;
  }

  /**
   * 将云数据相关状态重置为内置默认值。
   * 包括 links、messages、autoIcons、knownModLibraries 以及加载/错误状态，
   * 但不会主动调用后端删除已缓存数据。
   */
  function resetCloudData() {
    links.value = {
      supportIcon: CONSTANTS.cloudDataUrls.supportIcon,
      supportIconOnHover: CONSTANTS.cloudDataUrls.supportIconOnHover,
      supportLink: '',
      tutorialIcon: CONSTANTS.cloudDataUrls.tutorialIcon,
      tutorialIconOnHover: CONSTANTS.cloudDataUrls.tutorialIconOnHover,
      tutorialLink: '',
      contactIcon: CONSTANTS.cloudDataUrls.contactIcon,
      contactIconOnHover: CONSTANTS.cloudDataUrls.contactIconOnHover,
      contactLink: ''
    };
    messages.value = {
      wuwa: '',
      genshin: '',
      hsr: '',
      zzz: '',
      endfield: ''
    };
    autoIcons.value = {
      wuwa: [],
      genshin: [],
      hsr: [],
      zzz: [],
      endfield: []
    };
    knownModLibraries.value = { ...CONSTANTS.knownModdingLibraries };
    isCloudDataLoaded.value = false;
    lastFetchTime.value = null;
    fetchError.value = null;
  }

  return {
    links,
    messages,
    autoIcons,
    knownModLibraries,
    isCloudDataLoaded,
    isSyncing,
    isFetching,
    lastFetchTime,
    fetchError,
    supportIcon,
    supportLink,
    tutorialLink,
    contactLink,
    updatedKnownModdingLibs,
    setCloudData,
    setCloudDataLoaded,
    setSyncing,
    setFetching,
    setLastFetchTime,
    setFetchError,
    fetchCloudData,
    syncCloudData,
    getMessageForGame,
    getAutoIconsForGame,
    getModLibraryDisplayName,
    resetCloudData
  };
});
