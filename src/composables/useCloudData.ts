// 云数据操作组合式函数模块。
// 该模块对 cloudData store 进行二次封装，统一暴露云端数据（链接、消息、自动图标、已知模组库等）
// 的响应式状态与拉取/同步/重置等方法，并桥接云端数据更新事件的订阅能力。
import { storeToRefs } from 'pinia';
import { useCloudDataStore } from '../stores/cloudData';
import type { CloudData } from '../types';
import { useEvent, EventNames } from '../utils/events';

/**
 * 云端数据组合式函数。
 *
 * 作用：
 * - 通过 `storeToRefs` 把 cloudData store 中的状态转为可响应式引用，便于在组件中解构使用；
 * - 集中提供云端数据的拉取（fetch）、同步（sync）、设置与重置等能力；
 * - 提供按游戏查询公告消息/自动图标、按命名空间查询模组库展示名称等便捷方法；
 * - 桥接全局事件总线，提供云端数据更新事件的订阅入口。
 *
 * 业务逻辑：
 * - 云端数据由后端 `fetch_cloud_data` 命令拉取，包含入口链接、各游戏公告、自动图标与已知模组库映射；
 * - 拉取/同步结果会缓存到本地状态，并通过 CLOUD_DATA_UPDATED 事件通知其他模块；
 * - 模组库展示名称查找用于在 UI 上将命名空间字符串渲染为更友好的名称。
 *
 * 限制条件：
 * - 必须在 Pinia 已初始化的上下文中调用（通常在 setup 函数内）；
 * - 拉取/同步为网络请求，调用方需处理失败场景（fetchError）；
 * - 缓存数据仅在应用生命周期内有效，重启后需重新拉取。
 *
 * @returns 包含云端数据响应式状态及一组操作方法的对象
 */
export function useCloudData() {
  // 获取 cloudData store 实例，所有云端数据与后端交互由该 store 承载
  const cloudDataStore = useCloudDataStore();
  // 取出事件订阅方法，用于订阅云端数据更新事件
  const { on } = useEvent();

  // 将 store 中的 state/getter 转为 ref，保证解构后仍具响应性
  const {
    // 入口链接集合（支持/教程/联系入口的图标与跳转链接）
    links,
    // 各游戏公告消息集合
    messages,
    // 各游戏自动图标数据集合
    autoIcons,
    // 已知模组库的键值映射（识别字符串 -> 展示名称）
    knownModLibraries,
    // 云端数据是否已加载完成
    isCloudDataLoaded,
    // 是否正在同步（拉取后写入本地的过程）
    isSyncing,
    // 是否正在拉取（从远端获取数据的过程）
    isFetching,
    // 最近一次拉取成功的时间戳
    lastFetchTime,
    // 拉取过程中的错误信息（成功时为 null）
    fetchError,
    // 支持入口常态图标 URL（getter，便于直接绑定）
    supportIcon,
    // 支持入口跳转链接（getter）
    supportLink,
    // 教程入口跳转链接（getter）
    tutorialLink,
    // 联系入口跳转链接（getter）
    contactLink,
    // 已更新的已知模组库列表（getter，包含最新数据）
    updatedKnownModdingLibs
  } = storeToRefs(cloudDataStore);

  /**
   * 从远端拉取云端数据并写入本地状态。
   * @returns 是否拉取成功
   */
  async function fetchCloudData(): Promise<boolean> {
    return cloudDataStore.fetchCloudData();
  }

  /**
   * 同步云端数据（通常在拉取后调用，将数据合并/写入到本地缓存）。
   * @returns 是否同步成功
   */
  async function syncCloudData(): Promise<boolean> {
    return cloudDataStore.syncCloudData();
  }

  /**
   * 直接设置云端数据（覆盖式赋值）。
   * 通常用于在收到事件推送或本地构造后整体写入。
   * @param data 待覆盖的云端数据字段集合（部分字段）
   */
  function setCloudData(data: Partial<CloudData>) {
    cloudDataStore.setCloudData(data);
  }

  /**
   * 设置云端数据是否已加载完成的标记。
   * @param loaded 是否已加载
   */
  function setCloudDataLoaded(loaded: boolean) {
    cloudDataStore.setCloudDataLoaded(loaded);
  }

  /**
   * 设置同步中标记。
   * @param syncing 是否同步中
   */
  function setSyncing(syncing: boolean) {
    cloudDataStore.setSyncing(syncing);
  }

  /**
   * 设置拉取中标记。
   * @param fetching 是否拉取中
   */
  function setFetching(fetching: boolean) {
    cloudDataStore.setFetching(fetching);
  }

  /**
   * 获取指定游戏的公告消息。
   * @param game 游戏标识字符串
   * @returns 公告文本
   */
  function getMessageForGame(game: string): string {
    return cloudDataStore.getMessageForGame(game);
  }

  /**
   * 获取指定游戏的自动图标列表。
   * 用于在生成分组图标时按名称匹配远程图标。
   * @param game 游戏标识字符串
   * @returns 自动图标条目数组
   */
  function getAutoIconsForGame(game: string) {
    return cloudDataStore.getAutoIconsForGame(game);
  }

  /**
   * 根据命名空间字符串查询已知模组库的展示名称。
   * 若未找到匹配项，则由 store 返回默认值（通常为原命名空间字符串）。
   * @param namespace 模组库的识别字符串
   * @returns 展示名称
   */
  function getModLibraryDisplayName(namespace: string): string {
    return cloudDataStore.getModLibraryDisplayName(namespace);
  }

  /**
   * 重置云端数据为初始空状态。
   * 通常在退出登录或清理缓存时调用。
   */
  function resetCloudData() {
    cloudDataStore.resetCloudData();
  }

  /**
   * 订阅云端数据更新事件。
   * 当云端数据被拉取/同步/设置后会分发该事件。
   * @param callback 回调函数，接收最新的云端数据
   * @returns 取消订阅函数
   */
  async function onCloudDataUpdated(callback: (data: CloudData) => void): Promise<() => void> {
    return on(EventNames.CLOUD_DATA_UPDATED, (data) => {
      callback(data);
    });
  }

  // 统一返回响应式状态与方法，供调用方按需解构使用
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
    fetchCloudData,
    syncCloudData,
    setCloudData,
    setCloudDataLoaded,
    setSyncing,
    setFetching,
    getMessageForGame,
    getAutoIconsForGame,
    getModLibraryDisplayName,
    resetCloudData,
    onCloudDataUpdated
  };
}
