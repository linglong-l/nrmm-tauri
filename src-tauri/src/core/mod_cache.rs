//! 模组数据缓存模块
//!
//! 提供内存缓存功能，避免重复扫描文件系统。
//! 缓存键为 (TargetGame, 规范化路径字符串)，值为轻量扫描结果 ScanResult。
//! 使用 parking_lot::RwLock 保证线程安全的并发读写。

use crate::core::mod_scanner::{
    derive_group_active_state, group_order_cmp, recount_mods, ScanResult,
};
use crate::models::enums::{GroupType, TargetGame};
use crate::models::mod_data::{ModData, ModGroupData};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// 常驻缓存条目上限（LRU 淘汰阈值）。
///
/// 缓存仅保留最近交互的 3 个（游戏, 路径）结果；`set()` 插入后若超过该上限，
/// 淘汰 `_timestamp` 最旧的条目，以限制多游戏扫描结果在常驻内存中的累积量。
/// 被淘汰条目再次进入该游戏/路径时需触发一次全量重扫（~3-4s），内存优先。
const MAX_CACHE_ENTRIES: usize = 3;

/// 测试专用：写全局 [`MOD_CACHE`] 的 delta 组装类测试的串行锁。
///
/// 全局缓存上限 [`MAX_CACHE_ENTRIES`] = 3，而使用全局缓存的测试共 4 个
/// （mod_commands 的 3 个 R1 delta 测试 + file_watcher 的 R2 delta 组装测试）。
/// 并行执行时第 4 个 `set()` 触发 LRU 淘汰，可能在他测「set → 读旧分组状态」
/// 窗口内淘汰其条目，导致 old_mod_paths 随机为空的偶发断言失败；
/// 相关测试持此锁串行（顺序执行下各自的读紧跟自己的 set，无跨测试淘汰）。
#[cfg(test)]
pub static CACHE_WRITER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 缓存键类型，由目标游戏和规范化后的 Mods 目录路径字符串组成的二元组。
///
/// - `TargetGame`：区分不同游戏的缓存条目。
/// - `String`：`canonicalize` 规范化后的路径字符串，确保不同表示形式的同一路径命中同一缓存。
type CacheKey = (TargetGame, String);

/// 缓存条目，包含扫描结果和时间戳。
#[derive(Debug, Clone)]
struct CachedEntry {
    /// 轻量扫描结果（`Arc<ScanResult>`，缓存为唯一长期持有者）。
    ///
    /// 使用 `Arc` 使 `get()` 通过 `Arc::clone` 零拷贝共享给调用方（序列化/访问），
    /// 避免对整树做深克隆。`subtree_replace` 需要对子字段就地改时用
    /// `Arc::make_mut`：缓存仅唯一持有时原地可变（零拷贝），若调用方克隆了
    /// Arc 则写时拷贝（语义安全）。
    result: Arc<ScanResult>,
    /// 缓存插入时间戳，用于 LRU 淘汰时选择最旧条目剔除。
    _timestamp: Instant,
}

/// 模组数据缓存管理器。
///
/// 使用 `RwLock` 包裹 `HashMap`，支持多读单写。
/// 全局单例通过 `MOD_CACHE` 访问。
///
/// `cache` 保留所有缓存条目，`invalidated_games` 作为快速失效标记，
/// 避免在 `is_valid()` 中全量扫描 `cache` 来判断游戏是否失效。
#[derive(Debug)]
pub struct ModCache {
    /// 内部缓存存储，键为 `CacheKey`，值为 `CachedEntry`。
    cache: HashMap<CacheKey, CachedEntry>,
    /// 已被文件监控标记失效的游戏集合。
    ///
    /// `window-shown` 时查此集合快速判断是否可直接用缓存，
    /// 无需遍历 `cache` 中该游戏的所有条目。
    invalidated_games: HashSet<TargetGame>,
}

impl ModCache {
    /// 创建新的空缓存实例。
    ///
    /// 初始化空的 `HashMap` 和 `HashSet`，不分配任何缓存条目。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            invalidated_games: HashSet::new(),
        }
    }

    /// 查询缓存，命中返回共享的 `Arc<ScanResult>`（零拷贝，共享内部数据）。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径（将被规范化）。
    ///
    /// # 返回
    ///
    /// - `Some(Arc<ScanResult>)`: 缓存命中，返回结果的共享引用。
    /// - `None`: 缓存未命中。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn get(&self, game: TargetGame, mods_path: &Path) -> Option<Arc<ScanResult>> {
        let key = Self::make_key(game, mods_path);
        let result = self.cache.get(&key).map(|entry| Arc::clone(&entry.result));
        log::debug!(
            "[core::mod_cache] [get] game={:?} hit={}",
            game,
            result.is_some()
        );
        result
    }

    /// 写入缓存条目。
    ///
    /// 将 `result` 以 `(game, 规范化路径)` 为键存入 `cache`，
    /// 同时从 `invalidated_games` 中移除该游戏的失效标记，
    /// 表示该游戏的缓存已重新生效。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径。
    /// - `result`: 要缓存的轻量扫描结果。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn set(&mut self, game: TargetGame, mods_path: &Path, result: ScanResult) {
        let key = Self::make_key(game, mods_path);
        let mods_count = result.mods.len();
        self.cache.insert(
            key,
            CachedEntry {
                result: Arc::new(result),
                _timestamp: Instant::now(),
            },
        );
        // LRU 淘汰：超过上限时，按 _timestamp 淘汰最旧的溢出条目（逐次取全局最旧）。
        // 仅影响 set 时的内存上限；invalidate_* 显式失效语义不变。
        if self.cache.len() > MAX_CACHE_ENTRIES {
            let overflow = self.cache.len() - MAX_CACHE_ENTRIES;
            for _ in 0..overflow {
                let oldest_key = self
                    .cache
                    .iter()
                    .min_by_key(|(_, entry)| entry._timestamp)
                    .map(|(k, _)| k.clone());
                if let Some(k) = oldest_key {
                    self.cache.remove(&k);
                }
            }
        }
        // 清除该游戏的失效标记，表示缓存已更新
        self.invalidated_games.remove(&game);
        log::debug!(
            "[core::mod_cache] [set] game={:?} mods={}",
            game,
            mods_count
        );
    }

    /// 精确移除指定游戏 + 路径的一个缓存条目。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn invalidate(&mut self, game: TargetGame, mods_path: &Path) {
        let key = Self::make_key(game, mods_path);
        let removed = self.cache.remove(&key);
        log::debug!(
            "[core::mod_cache] [invalidate] game={:?} removed={}",
            game,
            removed.is_some()
        );
    }

    /// 失效指定游戏的所有缓存条目。
    ///
    /// 使用 `retain` 过滤掉所有 `CacheKey` 中 `game` 匹配的条目，
    /// 同时将该游戏加入 `invalidated_games` 集合，
    /// 以便后续 `is_valid()` 快速判断。
    ///
    /// # 参数
    /// - `game`: 要失效的目标游戏。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn invalidate_game(&mut self, game: TargetGame) {
        self.cache.retain(|(g, _), _| *g != game);
        self.invalidated_games.insert(game);
    }

    /// 按路径前缀批量失效缓存条目，用于文件监控。
    ///
    /// 遍历所有缓存键，如果路径字符串（大小写不敏感）包含 `managed_path` 前缀则移除。
    /// 用于文件监控检测到 `_MANAGED_` 目录变化时，失效受影响游戏的缓存。
    ///
    /// 同时扫描 `cache` 中剩余的所有游戏，将它们加入 `invalidated_games` 集合，
    /// 确保 `is_valid()` 能感知到部分条目可能已被删除。
    ///
    /// # 参数
    /// - `managed_path`: `_MANAGED_` 目录的路径，将被规范化为字符串进行前缀匹配。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn invalidate_by_prefix(&mut self, managed_path: &Path) {
        let prefix = match managed_path.canonicalize() {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => managed_path.to_string_lossy().to_string(),
        };
        let prefix_lower = prefix.to_lowercase();

        // 先收集受影响的游戏（路径包含前缀的条目），再移除条目
        // 避免误标所有剩余游戏为失效
        let affected_games: HashSet<TargetGame> = self
            .cache
            .keys()
            .filter(|(_, path_str)| path_str.to_lowercase().contains(&prefix_lower))
            .map(|(g, _)| *g)
            .collect();

        self.cache
            .retain(|(_, path_str), _| !path_str.to_lowercase().contains(&prefix_lower));

        for g in affected_games {
            self.invalidated_games.insert(g);
        }
    }

    /// 清空所有缓存条目和失效标记。
    ///
    /// 清空 `cache` 和 `invalidated_games`，恢复到初始空状态。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn invalidate_all(&mut self) {
        self.cache.clear();
        self.invalidated_games.clear();
    }

    /// 生成缓存键（规范化路径）。
    ///
    /// 先调用 `canonicalize` 规范化路径，失败时回退到原始路径，
    /// 再将路径转换为 `String` 作为 `CacheKey` 的第二元素。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 待规范化的 Mods 目录路径。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    fn make_key(game: TargetGame, mods_path: &Path) -> CacheKey {
        let normalized = mods_path
            .canonicalize()
            .unwrap_or_else(|_| mods_path.to_path_buf())
            .to_string_lossy()
            .to_string();
        (game, normalized)
    }

    /// 检查指定游戏的缓存是否有效。
    ///
    /// 先检查 `invalidated_games` 中是否包含该游戏（快速失效标记），
    /// 若包含则直接返回 `false`；否则再查 `cache` 中是否存在对应条目。
    ///
    /// 用于 `window-shown` 时快速判断能否直接使用缓存，
    /// 避免不必要的全量重新扫描。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径。
    ///
    /// # 返回
    ///
    /// - `true`: 缓存有效，可直接使用。
    /// - `false`: 缓存已失效，需要重新扫描。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn is_valid(&self, game: TargetGame, mods_path: &Path) -> bool {
        if self.invalidated_games.contains(&game) {
            return false;
        }
        let key = Self::make_key(game, mods_path);
        self.cache.contains_key(&key)
    }

    /// 增量更新子树替换：将局部扫描结果合并/覆盖进已有缓存中。
    ///
    /// 当增量更新器检测到局部文件变更后，调用此方法将 `partial_result` 的
    /// groups 和 mods 合并到该游戏+路径的现有缓存条目中。
    ///
    /// # 合并策略
    ///
    /// - **groups**：就地合并，按 `group_path`（路径等价、大小写不敏感）原位替换；
    ///   不存在则追加。合并后按全量扫描一致的规则排序（NormalGroup 数值序 / MutexGroup 名称序）。
    /// - **mods**：扁平列表仅保留真实模组（跳过 None 空槽位），按 `full_path`（路径等价、
    ///   大小写不敏感）原位替换；partial 分组子树下未出现的旧 mod 视为删除（删除传播）。
    /// - **签名复用（R3/C-3b）**：组节点替换时，新组内磁盘签名非 0 且与旧组同
    ///   `full_path` 条目签名相等的模组，复用旧 `Arc` 实例（零重分配，drop 新实例）；
    ///   嵌套 `children` 同款递归；扁平 mods 合并对旧扁平列表同款判定。
    ///   复用条目保持旧签名值（即命中值），后续合并仍可命中。
    /// - 合并后按全量轻量扫描同口径重算 `total_mods_count` / `enabled_mods_count` / `disabled_mods_count`
    ///   （排除 None；enabled = `!disabled && !mod_disabled`）。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径。
    /// - `partial_result`: 局部扫描结果，仅包含变更的 groups 和 mods。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn subtree_replace(
        &mut self,
        game: TargetGame,
        mods_path: &Path,
        partial_result: ScanResult,
    ) {
        let key = Self::make_key(game, mods_path);
        let existing = match self.cache.get_mut(&key) {
            Some(e) => e,
            None => {
                // 缓存未命中时跳过：partial_result 仅包含变更的 groups/mods，
                // 直接创建缓存条目会导致残缺数据。下次 get_mods 会全量扫描填充完整缓存。
                log::debug!("[mod_cache] subtree_replace: cache miss for {:?}, skipping (will full-scan on next get_mods)", game);
                return;
            }
        };

        // 记录 partial 各分组子树根（用于删除传播；必须在消费 partial_result.groups 之前计算）
        let partial_roots: Vec<std::path::PathBuf> = partial_result
            .groups
            .iter()
            .map(|g| std::path::PathBuf::from(&g.full_path))
            .collect();

        // Arc::make_mut：缓存是 Arc 的唯一长期持有者，通常原地可变（零拷贝）；
        // 若调用方已通过 get() 克隆了该 Arc，则此处写时拷贝一份再改（语义安全）。
        let owned = Arc::make_mut(&mut existing.result);

        // 将现有 groups 就地合并：按 group_path（路径等价、大小写不敏感）原位替换，
        // 避免 BTreeMap 按字典序破坏分组数值序、或大小写路径差异产生重复分组。
        // 替换前对新组节点做 C-3b 签名复用（须在覆盖旧组节点之前读取其条目）。
        let mut group_reused = 0usize;
        for mut pg in partial_result.groups {
            match owned
                .groups
                .iter()
                .position(|g| path_string_eq(&g.group_path, &pg.group_path))
            {
                Some(i) => {
                    reuse_unchanged_group_entries(&mut pg, &owned.groups[i], &mut group_reused);
                    owned.groups[i] = pg;
                }
                None => owned.groups.push(pg),
            }
        }
        // 合并后按与全量轻量扫描一致的规则排序（NormalGroup 置顶数值序 / MutexGroup 名称序）
        owned.groups.sort_by(group_order_cmp);

        // 扁平 mods 就地合并：None 空槽位仅存于 groups[i].mods 内用于槽位展示，不进扁平列表；
        // 其余 mod 按 full_path（路径等价、大小写不敏感）原位替换，否则追加。
        let partial_mods: Vec<Arc<ModData>> = partial_result
            .mods
            .into_iter()
            .filter(|m| m.name != "None" && m.mod_path != "None")
            .collect();
        // 删除传播前快照旧扁平条目（full_path 归一键 → Arc）：删除传播会移除
        // partial 子树下的旧条目，签名复用须先捕获这些旧实例。
        let old_flat: HashMap<String, Arc<ModData>> = owned
            .mods
            .iter()
            .map(|m| (path_cmp_key(Path::new(m.full_path.as_ref())), Arc::clone(m)))
            .collect();
        // 删除传播：partial 扫描粒度到分组，partial 中出现的分组代表该子树当前完整状态，
        // 其子树下未出现在 partial 中的旧 mod 视为已被删除（文件/目录被移除）。
        for gp in &partial_roots {
            owned
                .mods
                .retain(|m| !Path::new(m.full_path.as_ref()).starts_with(gp));
        }
        // 合并/覆盖 partial 的真实模组（C-3b：与旧扁平同 full_path 且签名相等 → 复用旧 Arc）
        let mut flat_reused = 0usize;
        for pm in partial_mods {
            let effective = reuse_flat_entry(&pm, &old_flat, &mut flat_reused);
            match owned
                .mods
                .iter()
                .position(|m| path_string_eq(m.full_path.as_ref(), effective.full_path.as_ref()))
            {
                Some(i) => owned.mods[i] = effective,
                None => owned.mods.push(effective),
            }
        }

        // 重算计数（与全量轻量扫描同口径：排除 None；enabled = !disabled && !mod_disabled）
        let (total, enabled, disabled) = recount_mods(&owned.mods);
        owned.total_mods_count = total;
        owned.enabled_mods_count = enabled;
        owned.disabled_mods_count = disabled;

        log::debug!(
            "[mod_cache] subtree_replace 签名复用 | 组内复用={} 条，扁平复用={} 条",
            group_reused,
            flat_reused
        );
        log::debug!(
            "[mod_cache] subtree_replace 合并守卫 | groups={} mods={} total={} enabled={} disabled={}",
            owned.groups.len(),
            owned.mods.len(),
            owned.total_mods_count,
            owned.enabled_mods_count,
            owned.disabled_mods_count,
        );
    }

    /// 从缓存移除指定分组子树（组节点 + 其全部 mods + 计数重算）
    ///
    /// 用于 remove_group 等分组级删除命令（磁盘目录已移除，直接裁剪缓存，无需重扫）。
    /// 仅匹配顶层组节点（此类命令删除的是 `_MANAGED_` 一级子目录）；扁平 `mods`
    /// 中 `full_path` 位于该组子树下的条目一并移除（覆盖嵌套 mutex children 内的模组）。
    ///
    /// # 参数
    /// - `game`: 目标游戏。
    /// - `mods_path`: 游戏 Mods 目录路径。
    /// - `group_full_path`: 被删除分组的完整目录路径。
    ///
    /// # 返回
    ///
    /// 被移除的模组数（扁平列表口径，不含 None 空槽位）。缓存未命中时返回 0。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn subtree_remove(
        &mut self,
        game: TargetGame,
        mods_path: &Path,
        group_full_path: &Path,
    ) -> usize {
        let key = Self::make_key(game, mods_path);
        let existing = match self.cache.get_mut(&key) {
            Some(e) => e,
            None => {
                // 缓存未命中：无旧状态可裁剪，下次 get_mods 全量扫描自然不含已删分组
                log::debug!(
                    "[mod_cache] subtree_remove: cache miss for {:?}, skipping",
                    game
                );
                return 0;
            }
        };

        // Arc::make_mut：缓存为唯一长期持有者，通常原地可变（零拷贝）
        let owned = Arc::make_mut(&mut existing.result);

        // 1. 移除顶层组节点（按路径等价比较，容忍 verbatim 前缀 / 大小写形态差异）
        let group_before = owned.groups.len();
        owned
            .groups
            .retain(|g| !path_equivalent(&g.full_path, group_full_path));
        let group_removed = group_before - owned.groups.len();

        // 2. 扁平 mods 移除该子树下全部条目（full_path 前缀匹配，含嵌套子分组内模组）
        let removed = owned
            .mods
            .iter()
            .filter(|m| path_within(Path::new(m.full_path.as_ref()), group_full_path))
            .count();
        owned
            .mods
            .retain(|m| !path_within(Path::new(m.full_path.as_ref()), group_full_path));

        // 3. 计数重算（与全量轻量扫描同口径：排除 None；enabled = !disabled && !mod_disabled）
        let (total, enabled, disabled) = recount_mods(&owned.mods);
        owned.total_mods_count = total;
        owned.enabled_mods_count = enabled;
        owned.disabled_mods_count = disabled;

        log::debug!(
            "[mod_cache] subtree_remove 裁剪守卫 | group_matched={} removed_mods={} total={} enabled={} disabled={}",
            group_removed,
            removed,
            owned.total_mods_count,
            owned.enabled_mods_count,
            owned.disabled_mods_count,
        );
        removed
    }

    /// 模组级增量 patch（C-3a）：单模组替换/新增/删除，兄弟条目 `Arc` 复用（零重扫零深拷贝）。
    ///
    /// 对缓存中「包含该模组的组子树」做最小变更：
    /// - **树定位**：顶层 `groups` 起，经 `children` 递归以路径前缀（[`path_within`]）
    ///   下钻到模组的直接父组；定位链上节点经 `Arc::make_mut` 重建（链外兄弟子树
    ///   指针原样复用——R3a `children: Vec<Arc<ModGroupData>>` 类型基础）。
    /// - **组内**：`Some` → 按 `full_path` 位置匹配替换（`mod_index` 保留旧实例值——
    ///   单模组扫描无法独立得出组内索引，位置不变则索引语义不变；无匹配 → 末位追加，
    ///   组内排序位由下次组级扫描校正）；`None` → 按位置移除（后续条目索引漂移同样
    ///   由下次组级扫描校正）。
    /// - **组级派生重算**：`mod_count` / `active_mod_index`（组内 `is_active` 条目索引，
    ///   None 槽位占 0 基准）/ `is_active` / `has_child` / `mod_paths`。
    /// - **扁平 mods 同步**：同 `full_path` 位置替换/新增/移除（与组内共享同一 `Arc`
    ///   实例），`total/enabled/disabled` 计数经 [`recount_mods`] 重算。
    ///
    /// # 参数
    /// - `game`: 目标游戏（缓存键）。
    /// - `mods_path`: Mods 根目录。
    /// - `mod_full_path`: 模组目录完整路径（容忍 verbatim / 大小写形态差异）。
    /// - `new_mod`: 新模组数据（`None` 表示删除 patch）。
    ///
    /// # 返回
    ///
    /// 受影响的**顶层**组节点（含 patch 后子树），供 delta 组装（与组级 partial 的
    /// `new_group` 同形状，前端 applyGroupDelta 无感）。缓存 miss、组子树内无定位
    /// 或删除无匹配条目时返回 `None`（调用方降级组级重扫）。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn patch_single_mod(
        &mut self,
        game: TargetGame,
        mods_path: &Path,
        mod_full_path: &Path,
        new_mod: Option<Arc<ModData>>,
    ) -> Option<ModGroupData> {
        let key = Self::make_key(game, mods_path);
        let existing = self.cache.get_mut(&key)?;

        // Arc::make_mut：缓存为唯一长期持有者，通常原地可变（零拷贝）；
        // 若调用方已通过 get() 克隆了该 Arc，则写时拷贝（此时深层 children 经
        // make_mut 逐级写时拷贝，链外兄弟仍共享旧 Arc 指针）
        let owned = Arc::make_mut(&mut existing.result);

        // 树定位 + 组内 patch：含该模组的顶层组唯一（_MANAGED_ 一级子目录互斥）。
        // 返回组内最终条目（删除为 None），供扁平同步复用同一 Arc 实例。
        let mut patched: Option<(ModGroupData, Option<Arc<ModData>>)> = None;
        for g in owned.groups.iter_mut() {
            if path_within(mod_full_path, &g.full_path) {
                if let Some(entry) = patch_mod_in_node(g, mod_full_path, new_mod) {
                    patched = Some((g.clone(), entry));
                }
                break;
            }
        }
        let (group_node, entry) = patched?;

        // 扁平 mods 同步（与组内共享同一 Arc 实例，保持 R3/C-3b 共享性不变量）
        match entry {
            Some(arc) => {
                let pos = owned.mods.iter().position(|m| {
                    path_equivalent(
                        Path::new(m.full_path.as_ref()),
                        Path::new(arc.full_path.as_ref()),
                    )
                });
                match pos {
                    Some(i) => owned.mods[i] = arc,
                    None => owned.mods.push(arc),
                }
            }
            None => owned
                .mods
                .retain(|m| !path_equivalent(Path::new(m.full_path.as_ref()), mod_full_path)),
        }

        // 计数重算（与全量轻量扫描同口径：排除 None；enabled = !disabled && !mod_disabled）
        let (total, enabled, disabled) = recount_mods(&owned.mods);
        owned.total_mods_count = total;
        owned.enabled_mods_count = enabled;
        owned.disabled_mods_count = disabled;

        Some(group_node)
    }

    /// 模组级就地 patch（零扫描）：目录重命名（启用/禁用）后直接更新缓存中该模组的
    /// 绝对路径与禁用标志。组树与扁平列表同步修改（同一 `Arc<ModData>` 经
    /// `Arc::make_mut` 就地更新，保持 R3/C-3b 共享实例不变量）。
    ///
    /// # 参数
    /// - `game`: 目标游戏（缓存键）。
    /// - `mods_path`: Mods 根目录。
    /// - `old_full_path`: 修改前的模组目录绝对路径。
    /// - `new_full_path`: 修改后的模组目录绝对路径（rename 目标）。
    /// - `disabled`: 修改后的禁用状态（`DISABLED_` 前缀增删）。
    ///
    /// # 返回
    /// 组树与扁平列表均定位并更新返回 `true`；任一侧未命中返回 `false`
    /// （调用方应降级组级局部重扫）。
    pub fn patch_mod_entry(
        &mut self,
        game: TargetGame,
        mods_path: &Path,
        old_full_path: &Path,
        new_full_path: &Path,
        disabled: bool,
    ) -> bool {
        let key = Self::make_key(game, mods_path);
        let existing = match self.cache.get_mut(&key) {
            Some(e) => e,
            None => return false,
        };
        let owned = Arc::make_mut(&mut existing.result);

        // 组树递归定位并就地修改（含 children 递归，深度优先唯一包含）
        fn patch_in_node(
            node: &mut ModGroupData,
            old: &Path,
            new_abs: &Path,
            disabled: bool,
        ) -> bool {
            for c in node.children.iter_mut() {
                if path_within(old, &c.full_path)
                    && patch_in_node(Arc::make_mut(c), old, new_abs, disabled)
                {
                    return true;
                }
            }
            for m in node.mods.iter_mut() {
                if path_equivalent(Path::new(m.full_path.as_ref()), old) {
                    let mm = Arc::make_mut(m);
                    mm.full_path = Arc::from(new_abs.to_string_lossy().as_ref());
                    mm.mod_path = new_abs.to_string_lossy().to_string();
                    // 双字段同步：兼容字段 `disabled` 与 `mod_disabled` 必须一致
                    //（扫描期两者同源；patch 只写其一会导致缓存内字段分叉，
                    // 派生逻辑（derive_group_active_state/recount）与序列化消费点口径错位）
                    mm.disabled = disabled;
                    mm.mod_disabled = disabled;
                    // 同步派生路径列表（与 mods 平行索引）
                    node.mod_paths = node.mods.iter().map(|x| x.full_path.clone()).collect();
                    return true;
                }
            }
            false
        }

        let mut group_hit = false;
        for g in owned.groups.iter_mut() {
            if path_within(old_full_path, &g.full_path) {
                group_hit = patch_in_node(g, old_full_path, new_full_path, disabled);
                break;
            }
        }

        let flat_hit = owned.mods.iter_mut().any(|m| {
            if !path_equivalent(Path::new(m.full_path.as_ref()), old_full_path) {
                return false;
            }
            let mm = Arc::make_mut(m);
            mm.full_path = Arc::from(new_full_path.to_string_lossy().as_ref());
            mm.mod_path = new_full_path.to_string_lossy().to_string();
            // 双字段同步（同组树分支注释：disabled/config 兼容字段不可分叉）
            mm.disabled = disabled;
            mm.mod_disabled = disabled;
            true
        });

        let ok = group_hit && flat_hit;
        if ok {
            let (total, enabled, disabled) = recount_mods(&owned.mods);
            owned.total_mods_count = total;
            owned.enabled_mods_count = enabled;
            owned.disabled_mods_count = disabled;
        }
        ok
    }

    /// 模组级就地移除（零扫描）：移除模组后从缓存剔除该条目（组树 + 扁平列表同步），
    /// 重算组级与总计数。
    ///
    /// # 参数
    /// - `game`: 目标游戏（缓存键）。
    /// - `mods_path`: Mods 根目录。
    /// - `mod_full_path`: 被移除模组的目录绝对路径。
    ///
    /// # 返回
    /// 组树与扁平列表均定位并剔除返回 `true`；未命中（缓存无此模组）返回 `false`
    /// （调用方应降级组级局部重扫）。
    pub fn remove_mod_entry(
        &mut self,
        game: TargetGame,
        mods_path: &Path,
        mod_full_path: &Path,
    ) -> bool {
        let key = Self::make_key(game, mods_path);
        let existing = match self.cache.get_mut(&key) {
            Some(e) => e,
            None => return false,
        };
        let owned = Arc::make_mut(&mut existing.result);
        let mut group_hit = false;
        for g in owned.groups.iter_mut() {
            if path_within(mod_full_path, &g.full_path) {
                group_hit = remove_mod_in_node(g, mod_full_path);
                break;
            }
        }
        let flat_hit = {
            let before = owned.mods.len();
            owned
                .mods
                .retain(|m| !path_equivalent(Path::new(m.full_path.as_ref()), mod_full_path));
            before != owned.mods.len()
        };
        let ok = group_hit && flat_hit;
        if ok {
            let (total, enabled, disabled) = recount_mods(&owned.mods);
            owned.total_mods_count = total;
            owned.enabled_mods_count = enabled;
            owned.disabled_mods_count = disabled;
        }
        ok
    }

    /// 选择直写快路径的缓存定向 patch（零扫描）：`write_selectedindex_direct` 落盘后，
    /// 将缓存中该分组的选中/激活态同步为「以新 selectedindex 重扫」的等价结果，
    /// 替代 `invalidate_by_prefix` 整缓存失效（避免下次 `get_mods` 全量重扫的秒级连锁）。
    ///
    /// 派生口径与全量轻量扫描完全一致（[`derive_group_active_state`] 单一真源）：
    /// 组节点 `active_mod_index` / `is_active`、组内 mods 与扁平列表（该组子树内
    /// 条目）的 `is_active` 同步更新。选择不改变目录结构与禁用态，计数无需重算。
    ///
    /// # 参数
    /// - `game`: 目标游戏（缓存键）。
    /// - `mods_path`: Mods 根目录。
    /// - `group_path`: 分组目录绝对路径（group_xx，容忍 verbatim / 大小写形态差异）。
    /// - `index`: 新写入的 selectedindex 值（0 = None 槽位）。
    ///
    /// # 返回
    /// 缓存命中且定位到该分组返回 `true`；缓存未命中或分组不在缓存中返回 `false`
    /// （调用方不失效整缓存，由 watcher 增量 / 下次全量扫描对账兜底）。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn patch_selected_index(
        &mut self,
        game: TargetGame,
        mods_path: &Path,
        group_path: &Path,
        index: u32,
    ) -> bool {
        let key = Self::make_key(game, mods_path);
        let existing = match self.cache.get_mut(&key) {
            Some(e) => e,
            None => return false,
        };

        // 快路径：缓存 Arc 无外部持有（strong_count==1）→ 原地改，零拷贝。
        // Arc 被外部持有（如 get_mods 序列化在途 clone）时，`Arc::make_mut` 会深拷贝
        // 整棵万级模组树（实测数秒～数十秒），绝不可走默认 COW。
        // 慢路径降级返回 false：调用方跳过缓存 patch，selectedindex 文件变更必然触发
        // watcher 增量重扫修正缓存 + 推送组 delta 至前端（~1s 静默期，远优于阻塞数十秒）。
        let start = Instant::now();
        let Some(owned) = Arc::get_mut(&mut existing.result) else {
            log::debug!(
                "[mod_cache] patch_selected_index skip COW | group={} strong_count={} elapsed_ms={}",
                group_path.display(),
                Arc::strong_count(&existing.result),
                start.elapsed().as_millis()
            );
            return false;
        };

        // 定位顶层分组节点（selectedindex 仅存在于 group_xx；路径等价容忍形态差异）
        let Some(group) = owned
            .groups
            .iter_mut()
            .find(|g| path_equivalent(&g.full_path, group_path))
        else {
            log::debug!(
                "[mod_cache] patch_selected_index: group not in cache | group={}",
                group_path.display()
            );
            return false;
        };

        // 派生 is_active / active_mod_index（与轻量扫描同口径的单一真源）
        let active_idx = derive_group_active_state(&mut group.mods, index as i32);
        group.active_mod_index = active_idx;
        group.is_active = active_idx >= 0;

        // 扁平列表同步：该组子树内条目按 full_path 匹配组内派生结果
        //（None 不在扁平列表；NormalGroup 无 children，仅顶层组自身 mods）
        let active_by_path: HashMap<String, bool> = group
            .mods
            .iter()
            .map(|m| (path_cmp_key(Path::new(m.full_path.as_ref())), m.is_active))
            .collect();
        for m in owned.mods.iter_mut() {
            if !path_within(Path::new(m.full_path.as_ref()), group_path) {
                continue;
            }
            if let Some(active) = active_by_path.get(&path_cmp_key(Path::new(m.full_path.as_ref())))
            {
                Arc::make_mut(m).is_active = *active;
            }
        }

        log::debug!(
            "[mod_cache] patch_selected_index | group={} index={} active_mod_index={} elapsed_ms={}",
            group_path.display(),
            index,
            active_idx,
            start.elapsed().as_millis()
        );
        true
    }
}

/// 组节点内递归剔除指定模组（含 children），并保持派生字段一致。
///
/// # 返回
/// 是否剔除命中（任一树节点中该模组被移除）。
fn remove_mod_in_node(node: &mut ModGroupData, mod_full_path: &Path) -> bool {
    let mut hit = false;
    for c in node.children.iter_mut() {
        if remove_mod_in_node(Arc::make_mut(c), mod_full_path) {
            hit = true;
        }
    }
    let before = node.mods.len();
    node.mods
        .retain(|m| !path_equivalent(Path::new(m.full_path.as_ref()), mod_full_path));
    if node.mods.len() != before {
        node.mod_paths = node.mods.iter().map(|m| m.full_path.clone()).collect();
        // 组级计数与轻量扫描同口径：模组数 = mods.len() 减 None 槽位（None 恒位于首位）
        node.mod_count = node.mods.len().saturating_sub(1) as u32;
        hit = true;
    }
    hit
}

/// C-3b 组节点签名复用：新组节点替换旧组节点（`group_path` 等价）前，
/// 组内 mods 与嵌套 children 中磁盘签名命中的条目回退为旧 `Arc` 实例。
///
/// 判定：新条目签名非 0，且旧组同 `full_path`（路径等价、大小写不敏感）
/// 条目存在且签名相等 → 三源 mtime（mod 目录 / fav / 组 selectedindex）均
/// 未变，磁盘派生状态与旧实例一致 → 用旧 `Arc` 替换新实例（零重分配）。
/// None 空槽位签名恒 0，天然不参与复用。
///
/// `children` 递归同款：新子组按 `full_path` 匹配旧子组后对其内部 mods 复用；
/// `Arc::make_mut` 在子组 Arc 被外部共享时写时拷贝（语义安全）。
///
/// # 参数
/// - `new_group`: 即将替换入缓存的局部重扫组节点。
/// - `old_group`: 缓存中的旧组节点（替换后丢弃，其模组 Arc 被复用续命）。
/// - `reused`: 复用计数（含 children 递归命中），供埋点。
fn reuse_unchanged_group_entries(
    new_group: &mut ModGroupData,
    old_group: &ModGroupData,
    reused: &mut usize,
) {
    for m in new_group.mods.iter_mut() {
        let sig = m.disk_signature;
        if sig == 0 {
            continue;
        }
        if let Some(old_m) = old_group.mods.iter().find(|om| {
            om.disk_signature == sig && path_string_eq(om.full_path.as_ref(), m.full_path.as_ref())
        }) {
            *m = Arc::clone(old_m);
            *reused += 1;
        }
    }
    for nc in new_group.children.iter_mut() {
        if let Some(oc) = old_group
            .children
            .iter()
            .find(|oc| path_equivalent(&oc.full_path, &nc.full_path))
        {
            reuse_unchanged_group_entries(Arc::make_mut(nc), oc, reused);
        }
    }
}

/// C-3b 扁平条目签名复用：partial 扁平条目与旧扁平快照（删除传播前捕获）
/// 同 `full_path` 且签名相等时返回旧 `Arc`，否则返回新实例。
///
/// 快照键为 `full_path` 归一形态（统一分隔符 / verbatim 剥离 / Windows
/// 大小写不敏感），与组内判定的 `path_string_eq` 口径一致。
fn reuse_flat_entry(
    pm: &Arc<ModData>,
    old_flat: &HashMap<String, Arc<ModData>>,
    reused: &mut usize,
) -> Arc<ModData> {
    let sig = pm.disk_signature;
    if sig == 0 {
        return Arc::clone(pm);
    }
    let key = path_cmp_key(Path::new(pm.full_path.as_ref()));
    if let Some(old_m) = old_flat.get(&key) {
        if old_m.disk_signature == sig {
            *reused += 1;
            return Arc::clone(old_m);
        }
    }
    Arc::clone(pm)
}

/// 在组节点子树内定位模组的直接父组并应用模组级 patch（C-3a 私有）。
///
/// 深度优先下钻：`children` 中包含该模组路径的子分组唯一（同层子分组目录互斥），
/// 对其递归（定位链上节点经 `Arc::make_mut` 重建，链外兄弟子树指针原样复用）；
/// 无子分组包含时，本节点即直接父组（`mod_full_path` 的父目录等于本组路径）。
///
/// # 返回（双层 `Option`）
/// - 外层 `None`：子树内无定位（如目标实为子分组节点自身），调用方降级组级重扫；
/// - 外层 `Some(None)`：已应用删除 patch；
/// - 外层 `Some(Some(entry))`：已应用替换/新增，`entry` 为组内最终条目
///   （扁平列表同步复用该实例，保持与组内共享同一 `Arc`）。
fn patch_mod_in_node(
    node: &mut ModGroupData,
    mod_full_path: &Path,
    new_mod: Option<Arc<ModData>>,
) -> Option<Option<Arc<ModData>>> {
    // 深度优先：包含目标的子分组唯一，直接下钻（无其他兄弟可同时包含）
    for c in node.children.iter_mut() {
        if path_within(mod_full_path, &c.full_path) {
            return patch_mod_in_node(Arc::make_mut(c), mod_full_path, new_mod);
        }
    }
    // 本节点为直接父组判定：模组路径的父目录 == 本组路径（路径等价，容忍形态差异）
    if !mod_full_path
        .parent()
        .is_some_and(|p| path_equivalent(p, &node.full_path))
    {
        return None;
    }
    apply_mod_patch(node, mod_full_path, new_mod)
}

/// 对直接父组节点应用单模组替换/新增/删除，并重算组级派生字段（C-3a 私有）。
///
/// # 返回（语义同 [`patch_mod_in_node`] 内层，删除无匹配返回 `None` 表示未变更）
fn apply_mod_patch(
    group: &mut ModGroupData,
    mod_full_path: &Path,
    new_mod: Option<Arc<ModData>>,
) -> Option<Option<Arc<ModData>>> {
    let entry: Option<Arc<ModData>> = match new_mod {
        Some(new_arc) => {
            let pos = group
                .mods
                .iter()
                .position(|m| path_equivalent(Path::new(m.full_path.as_ref()), mod_full_path));
            let mut arc = new_arc;
            match pos {
                Some(pos) => {
                    // 替换：保留旧 mod_index（单模组扫描无法得出组内索引；位置不变 → 索引语义不变）
                    Arc::make_mut(&mut arc).mod_index = group.mods[pos].mod_index;
                    group.mods[pos] = Arc::clone(&arc);
                    log::debug!(
                        "[mod_cache] mod 级 patch | mod={} op=replace",
                        mod_full_path.display()
                    );
                }
                None => {
                    // 新增：组内末位（排序位/兄弟索引漂移由下次组级扫描校正）
                    Arc::make_mut(&mut arc).mod_index = group.mods.len() as u32;
                    group.mods.push(Arc::clone(&arc));
                    log::debug!(
                        "[mod_cache] mod 级 patch | mod={} op=insert",
                        mod_full_path.display()
                    );
                }
            }
            Some(arc)
        }
        None => {
            let before = group.mods.len();
            group
                .mods
                .retain(|m| !path_equivalent(Path::new(m.full_path.as_ref()), mod_full_path));
            if group.mods.len() == before {
                // 删除无匹配（如路径实为子分组节点）：未变更，调用方降级组级重扫
                return None;
            }
            log::debug!(
                "[mod_cache] mod 级 patch | mod={} op=remove",
                mod_full_path.display()
            );
            None
        }
    };
    recompute_group_derived(group);
    Some(entry)
}

/// 重算组节点派生字段（C-3a 私有），口径对齐轻量扫描：
/// - `mod_count`：直接子模组数（排除 None 空槽位；mutex 树各节点仅计直接子项）；
/// - `active_mod_index`：组内 `is_active` 条目索引（None 槽位占 0 基准，无激活条目为 -1）；
/// - `is_active`：`active_mod_index >= 0`（NormalGroup 组级语义；mutex 树各节点扫描恒
///   无激活条目 → false，与扫描口径一致）；
/// - `has_child`：NormalGroup 恒 false（扫描口径），MutexGroup 按 children/mods 非空判定；
/// - `mod_paths`：非 None 直接子模组的 `full_path` 镜像。
///
/// `child_groups` 为扫描期内部镜像（`skip_serializing`，缓存无消费点），模组级 patch
/// 不回写该字段，下次组级扫描自然重建。
fn recompute_group_derived(group: &mut ModGroupData) {
    group.mod_count = group.mods.iter().filter(|m| m.name != "None").count() as u32;
    group.mod_paths = group
        .mods
        .iter()
        .filter(|m| m.name != "None")
        .map(|m| m.full_path.clone())
        .collect();
    group.active_mod_index = group
        .mods
        .iter()
        .position(|m| m.is_active)
        .map_or(-1, |i| i as i32);
    group.is_active = group.active_mod_index >= 0;
    group.has_child = if group.group_type == GroupType::MutexGroup {
        !group.children.is_empty() || !group.mods.is_empty()
    } else {
        false
    };
}

/// 路径字符串等价比较（Windows 下大小写不敏感，其他平台精确比较）。
///
/// 用于增量合并时按路径原位识别分组/模组，避免大小写路径差异产生重复条目。
/// 语义对齐 `mod_manager::path_eq_ignore_case`。
fn path_string_eq(a: &str, b: &str) -> bool {
    #[cfg(windows)]
    {
        a.to_lowercase() == b.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// 路径比较键：统一分隔符为 `/`、剥离 Windows verbatim 前缀（`\\?\`），
/// Windows 下再转小写实现大小写不敏感比较。
///
/// 命令层路径经 `canonicalize` 后携带 verbatim 前缀，而缓存中的
/// `full_path` / `group_path` 保持 `get_mods` 入参的普通形态；
/// 统一归一后再比较，避免形态差异导致匹配失败。
fn path_cmp_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").map(str::to_string).unwrap_or(s);
    #[cfg(windows)]
    {
        s.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        s
    }
}

/// 路径等价判断（语义同 [`path_string_eq`]，额外容忍 verbatim 前缀差异）。
///
/// # 参数
/// - `a`/`b`: 待比较的两个路径。
///
/// # 返回
///
/// 两路径指向同一位置时返回 `true`。
fn path_equivalent(a: &Path, b: &Path) -> bool {
    path_cmp_key(a) == path_cmp_key(b)
}

/// 判断 `child` 是否位于 `ancestor` 目录之下（含相等；比较口径同 [`path_equivalent`]）。
///
/// 命令级 delta 组装用它在新分组节点中按 `full_path` 前缀匹配局部重扫入口目录。
///
/// # 参数
/// - `child`: 子路径。
/// - `ancestor`: 祖先目录路径。
///
/// # 返回
///
/// `child` 等于 `ancestor` 或位于其下（以路径分隔符为界，避免 `group_1` 误匹配 `group_10`）时返回 `true`。
pub fn path_within(child: &Path, ancestor: &Path) -> bool {
    let (c, a) = (path_cmp_key(child), path_cmp_key(ancestor));
    c == a || c.starts_with(&format!("{a}/"))
}

/// 在扫描结果顶层分组中按完整路径定位分组节点。
///
/// 仅匹配顶层（`_MANAGED_` 一级子目录）：命令级 delta 的粒度恒为顶层分组
/// （`scan_partial_path` 亦收敛到该粒度），限制在顶层可避免「旧收集范围与
/// 新分组范围不一致」的偏差。
///
/// # 参数
/// - `result`: 扫描结果（缓存或局部重扫产物）。
/// - `group_full_path`: 分组完整目录路径（容忍 verbatim 前缀 / 大小写形态差异）。
///
/// # 返回
///
/// 匹配的分组节点引用；顶层无匹配时返回 `None`。
pub fn find_group_node<'a>(
    result: &'a ScanResult,
    group_full_path: &Path,
) -> Option<&'a ModGroupData> {
    result
        .groups
        .iter()
        .find(|g| path_equivalent(&g.full_path, group_full_path))
}

/// 从缓存收集旧分组状态（old_group_path + 组内全部真实模组的 mod_path）。
///
/// 读锁内定位分组节点（路径等价比较，容忍 verbatim 前缀 / 大小写形态差异）：
/// 命中时返回缓存节点的 `group_path`（前端树定位键原值）与组子树内全部
/// 真实模组的 mod_path（递归 mutex children、排除 None 空槽位）；
/// 缓存未命中或节点不存在时 mod_path 列表为空、分组键回退为目录路径串。
///
/// 命令级 delta（R1）与 watcher 增量 delta（R2）共用的旧状态收集入口，
/// 供命令层（mod_commands）与监听层（file_watcher）组装
/// [`crate::models::mod_data::GroupDelta`] 的 old_group_path / old_mod_paths。
///
/// # 参数
/// - `game`: 目标游戏（缓存键）。
/// - `mods_path`: Mods 根目录。
/// - `old_group_dir`: 旧分组目录路径。
///
/// # 返回
///
/// `(old_group_path, old_mod_paths)` 二元组。
pub fn collect_old_group_state(
    game: TargetGame,
    mods_path: &Path,
    old_group_dir: &Path,
) -> (String, Vec<String>) {
    let cache = MOD_CACHE.read();
    match cache
        .get(game, mods_path)
        .as_ref()
        .and_then(|scan| find_group_node(scan, old_group_dir))
    {
        Some(node) => (node.group_path.clone(), collect_group_mod_paths(node)),
        None => (old_group_dir.to_string_lossy().to_string(), Vec::new()),
    }
}

/// 收集分组子树内全部真实模组的 mod_path 列表。
///
/// 递归遍历 `mods` 与 `children`（嵌套 mutex 子分组），排除 `None` 空槽位
/// （`name == "None"` 或 `mod_path == "None"`）。用于命令级 delta 的
/// `old_mod_paths`（前端从扁平 mods 列表剔除旧条目）与 `affected_mods` 埋点统计。
///
/// # 参数
/// - `group`: 分组节点（含子树）。
///
/// # 返回
///
/// 该子树内全部真实模组的 `mod_path`，按树序返回。
pub fn collect_group_mod_paths(group: &ModGroupData) -> Vec<String> {
    let mut out = Vec::new();
    collect_mod_paths_inner(group, &mut out);
    out
}

/// [`collect_group_mod_paths`] 的递归实现。
fn collect_mod_paths_inner(group: &ModGroupData, out: &mut Vec<String>) {
    for m in &group.mods {
        if m.name != "None" && m.mod_path != "None" {
            out.push(m.mod_path.clone());
        }
    }
    for child in &group.children {
        collect_mod_paths_inner(child, out);
    }
}

impl Default for ModCache {
    /// 创建默认的空缓存实例（等价于 `new()`）。
    fn default() -> Self {
        Self::new()
    }
}

/// 全局模组缓存单例。
///
/// 使用 `once_cell::sync::Lazy` 实现首次访问时自动初始化为空缓存，
/// 以 `parking_lot::RwLock` 包装支持并发读和独占写。
pub static MOD_CACHE: Lazy<RwLock<ModCache>> = Lazy::new(|| RwLock::new(ModCache::new()));

/// Tauri IPC 命令：检查指定游戏的模组缓存是否有效。
///
/// 供前端通过 `checkModCacheValid` 调用，返回 `bool` 表示缓存是否可用。
///
/// # 参数
/// - `game`: 游戏名称的字符串，将调用 `parse_game` 解析为 `TargetGame`。
/// - `mods_path`: Mods 目录路径的字符串。
///
/// # 返回
///
/// - `true`: 缓存有效，可直接使用。
/// - `false`: 缓存已失效或不存在。
///
/// # 注意
///
/// 若 `parse_game` 解析失败（未知游戏名称），使用 `unwrap_or(TargetGame::GenshinImpact)` 作为兜底。
#[tauri::command]
pub fn check_mod_cache_valid(game: String, mods_path: String) -> bool {
    use crate::models::enums::TargetGame;
    let parsed = crate::hotkey::parse_game(&game).unwrap_or(TargetGame::GenshinImpact);
    let cache = MOD_CACHE.read();
    cache.is_valid(parsed, Path::new(&mods_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mod_scanner;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// 创建测试用的临时 Mods 目录结构：
    /// - `_MANAGED_/group_1/TestMod/mod.ini`
    /// - `d3dx.ini`
    fn setup_test_mods() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        // 创建一个group_1目录和一个mod
        let group1 = managed.join("group_1");
        fs::create_dir_all(&group1).unwrap();
        let mod_dir = group1.join("TestMod");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("mod.ini"), "[Section]\nx=1\n").unwrap();
        // 创建d3dx.ini
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        (dir, mods_path)
    }

    /// 测试 `set` 后能通过 `get` 正确获取缓存结果。
    #[test]
    fn test_cache_set_and_get() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());

        let cached = cache.get(TargetGame::GenshinImpact, &mods_path);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().total_mods_count, result.total_mods_count);
    }

    /// 测试缓存未命中：不存在的路径返回 `None`。
    #[test]
    fn test_cache_miss() {
        let cache = ModCache::new();
        let fake_path = PathBuf::from("/nonexistent/path");
        assert!(cache.get(TargetGame::GenshinImpact, &fake_path).is_none());
    }

    /// 测试精确失效：`invalidate` 后 `get` 返回 `None`。
    #[test]
    fn test_cache_invalidate() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result);
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_some());

        cache.invalidate(TargetGame::GenshinImpact, &mods_path);
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_none());
    }

    /// 测试按游戏失效：`invalidate_game` 只移除指定游戏的缓存，其他游戏不受影响。
    #[test]
    fn test_cache_invalidate_game() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());
        cache.set(TargetGame::Wuwa, &mods_path, result);

        cache.invalidate_game(TargetGame::GenshinImpact);
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_none());
        assert!(cache.get(TargetGame::Wuwa, &mods_path).is_some());
    }

    /// 测试全量清空：`invalidate_all` 后所有缓存条目消失。
    #[test]
    fn test_cache_invalidate_all() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());
        cache.set(TargetGame::Wuwa, &mods_path, result);

        cache.invalidate_all();
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_none());
        assert!(cache.get(TargetGame::Wuwa, &mods_path).is_none());
    }

    /// 测试不同游戏的缓存隔离：设置 GenshinImpact 后，Wuwa 查询不应命中。
    #[test]
    fn test_cache_different_games_isolated() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result);

        assert!(cache.get(TargetGame::Wuwa, &mods_path).is_none());
        assert!(cache.get(TargetGame::GenshinImpact, &mods_path).is_some());
    }

    /// patch_mod_entry：目录重命名（禁用）后组树 + 扁平列表路径与禁用标志同步更新，零扫描。
    #[test]
    fn test_patch_mod_entry_updates_path_and_disabled() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();
        let old_full = result
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .and_then(|g| g.mods.iter().find(|m| m.mod_name == "TestMod"))
            .map(|m| Path::new(m.full_path.as_ref()).to_path_buf())
            .expect("存在 TestMod 模组");

        let new_full = old_full
            .parent()
            .expect("模组有父目录")
            .join("DISABLEDTestMod");

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());
        assert!(cache.patch_mod_entry(
            TargetGame::GenshinImpact,
            &mods_path,
            &old_full,
            &new_full,
            true
        ));

        let cached = cache
            .get(TargetGame::GenshinImpact, &mods_path)
            .expect("缓存存在");
        // 组树内：路径更新 + 禁用标志 + 名称不变
        let entry = cached
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .and_then(|g| g.mods.iter().find(|m| m.mod_name == "TestMod"))
            .expect("组内仍存在该模组");
        assert!(
            path_equivalent(Path::new(entry.full_path.as_ref()), &new_full),
            "组内 full_path 应更新为 DISABLED 路径"
        );
        assert!(entry.mod_disabled, "mod_disabled 应置 true");
        assert!(
            entry.disabled,
            "兼容字段 disabled 应与 mod_disabled 同步置 true（防字段分叉）"
        );
        assert_eq!(entry.mod_name, "TestMod", "mod_name 应保持不变");
        // 扁平列表同步（共享 Arc，路径一致）
        assert!(
            cached
                .mods
                .iter()
                .any(|m| path_equivalent(Path::new(m.full_path.as_ref()), &new_full)),
            "扁平列表应同步更新路径"
        );
        let flat = cached
            .mods
            .iter()
            .find(|m| path_equivalent(Path::new(m.full_path.as_ref()), &new_full))
            .expect("扁平列表含 DISABLED 路径条目");
        assert!(
            flat.disabled,
            "扁平条目 disabled 字段同步声明（与组内共享同一 Arc）"
        );
        // 计数重算：禁用 1，启用 0
        assert_eq!(cached.total_mods_count, 1);
        assert_eq!(cached.enabled_mods_count, 0);
        assert_eq!(cached.disabled_mods_count, 1);

        // 再次 patch（启用还原）同样命中
        assert!(cache.patch_mod_entry(
            TargetGame::GenshinImpact,
            &mods_path,
            &new_full,
            &old_full,
            false
        ));
        let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let entry = cached
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .and_then(|g| g.mods.iter().find(|m| m.mod_name == "TestMod"))
            .unwrap();
        assert!(!entry.mod_disabled, "再次 patch 后应恢复启用");
        assert!(!entry.disabled, "兼容字段 disabled 应同步恢复启用态");
        assert_eq!(cached.enabled_mods_count, 1);
    }

    /// patch_mod_entry：命中不存在的模组路径返回 false（调用方据此降级重扫）。
    #[test]
    fn test_patch_mod_entry_miss_returns_false() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();
        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result);

        let miss = mods_path.join("_MANAGED_/group_1/NoSuchMod");
        let new = mods_path.join("_MANAGED_/group_1/DISABLEDNoSuchMod");
        assert!(!cache.patch_mod_entry(TargetGame::GenshinImpact, &mods_path, &miss, &new, true));

        // 缓存未命中（无该游戏条目）也返回 false
        assert!(!cache.patch_mod_entry(
            TargetGame::GenshinImpact,
            Path::new("/no/mods"),
            &miss,
            &new,
            true
        ));
    }

    /// remove_mod_entry：组树 + 扁平列表同步剔除并重算计数；二次移除返回 false。
    #[test]
    fn test_remove_mod_entry_removes_from_tree_and_flat() {
        let (_dir, mods_path) = setup_test_mods();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();
        let full = result
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .and_then(|g| g.mods.iter().find(|m| m.mod_name == "TestMod"))
            .map(|m| Path::new(m.full_path.as_ref()).to_path_buf())
            .expect("存在 TestMod 模组");

        let mut cache = ModCache::new();
        cache.set(TargetGame::GenshinImpact, &mods_path, result.clone());
        assert!(cache.remove_mod_entry(TargetGame::GenshinImpact, &mods_path, &full));

        let cached = cache
            .get(TargetGame::GenshinImpact, &mods_path)
            .expect("缓存存在");
        assert_eq!(cached.total_mods_count, 0, "总模组数应清零");
        assert!(
            !cached.mods.iter().any(|m| m.mod_name == "TestMod"),
            "扁平列表应剔除该模组"
        );
        let group = cached
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .expect("分组仍存在");
        assert!(
            !group.mods.iter().any(|m| m.mod_name == "TestMod"),
            "组树内应剔除该模组"
        );
        assert_eq!(group.mod_count, 0, "组内模组计数应清零");

        // 二次移除（已无此模组）返回 false
        assert!(!cache.remove_mod_entry(TargetGame::GenshinImpact, &mods_path, &full));
    }

    /// 测试全局单例 `MOD_CACHE` 可正常访问且初始状态为空。
    #[test]
    fn test_global_cache_singleton() {
        // 测试全局单例可正常访问
        let cache = MOD_CACHE.read();
        assert!(cache
            .get(TargetGame::GenshinImpact, Path::new("/nonexistent"))
            .is_none());
    }

    /// 测试删除传播：局部扫描合并后，磁盘上已删除的模组应从缓存移除。
    ///
    /// 回归场景：用户删除 group_1 下某模组目录，文件监听器触发局部重扫 →
    /// `subtree_replace` 合并 partial 结果。原实现只增改不删，导致已删除模组
    /// 在 UI 中残留至全量重扫；修复后应即时消失。
    #[test]
    fn test_subtree_replace_propagates_deletion() {
        use crate::core::mod_scanner::scan_partial_path;
        let (dir, mods_path) = setup_test_mods();
        let group1 = dir.path().join("_MANAGED_").join("group_1");
        let mod_gone = group1.join("ModGone");
        fs::create_dir_all(&mod_gone).unwrap();
        fs::write(mod_gone.join("mod.ini"), "[Section]\nx=1\n").unwrap();

        let mut cache = ModCache::new();
        let result = mod_scanner::scan_mods_light(&mods_path).unwrap();
        cache.set(TargetGame::GenshinImpact, &mods_path, result);
        assert!(
            cache
                .get(TargetGame::GenshinImpact, &mods_path)
                .unwrap()
                .mods
                .iter()
                .any(|m| m.mod_name == "ModGone"),
            "前置条件：ModGone 应已进入缓存"
        );

        // 删除磁盘上的 ModGone 目录后，对 group_1 做局部扫描并合并
        fs::remove_dir_all(&mod_gone).unwrap();
        let partial = scan_partial_path(&mods_path, &group1).unwrap();
        cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        assert!(
            !updated.mods.iter().any(|m| m.mod_name == "ModGone"),
            "已删除的模组不应残留在缓存中"
        );
        assert!(
            updated.mods.iter().any(|m| m.mod_name == "TestMod"),
            "未删除的模组应保留"
        );
    }

    /// 测试：subtree_replace 合并后计数与扁平列表需与全量轻量扫描口径一致。
    ///
    /// 回归场景：原实现 groups 用 BTreeMap 按字典序合并破坏数值分组顺序、扁平 mods 含 None
    /// 空槽位、enabled 用 is_active 冒充 disabled 语义 → 计数/排序/扁平列表与全量扫描不一致。
    #[test]
    fn subtree_replace_matches_light_scan_semantics() {
        use crate::core::mod_scanner::scan_partial_path;
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(managed.join("group_2").join("EnabledMod")).unwrap();
        fs::write(
            managed.join("group_2").join("EnabledMod").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        fs::create_dir_all(managed.join("group_2").join("DISABLED_Off")).unwrap();
        fs::write(
            managed.join("group_2").join("DISABLED_Off").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        fs::create_dir_all(managed.join("group_10").join("ModB")).unwrap();
        fs::write(
            managed.join("group_10").join("ModB").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();

        let mut cache = ModCache::new();
        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        // 前置：全量扁平 mods 不含 None，计数口径（total/enabled/disabled）
        assert!(full.mods.iter().all(|m| m.name != "None"));
        assert_eq!(
            (
                full.total_mods_count,
                full.enabled_mods_count,
                full.disabled_mods_count
            ),
            (3, 2, 1)
        );
        cache.set(TargetGame::GenshinImpact, &mods_path, full);

        // 局部重扫 group_10 并合并
        let g10 = managed.join("group_10");
        let partial = scan_partial_path(&mods_path, &g10).unwrap();
        cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        // 计数与全量口径一致
        assert_eq!(
            (
                updated.total_mods_count,
                updated.enabled_mods_count,
                updated.disabled_mods_count
            ),
            (3, 2, 1)
        );
        // 扁平 mods 无 None，且仅为三个真实模组
        assert!(updated.mods.iter().all(|m| m.name != "None"));
        assert_eq!(updated.mods.len(), 3);
        // 分组顺序为数值序：group_2（索引 2）在 group_10（索引 10）前
        let idx: Vec<u32> = updated.groups.iter().map(|g| g.group_index).collect();
        assert_eq!(idx, vec![2, 10]);
    }

    /// 测试：subtree_replace 遇到仅大小写不同的 group_path 时原位替换，不产生重复分组。
    ///
    /// 回归场景：原实现按 group_path 字典序 key 去重，大小写路径差异被当作不同分组追加，
    /// 产生重复分组。修复后按路径等价（大小写不敏感）比较原位替换。
    #[test]
    fn subtree_replace_no_duplicate_group_on_case_diff() {
        use crate::core::mod_scanner::scan_partial_path;
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let g2 = dir.path().join("_MANAGED_").join("group_2");
        fs::create_dir_all(g2.join("ModB")).unwrap();
        fs::write(g2.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();

        let mut cache = ModCache::new();
        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        cache.set(TargetGame::GenshinImpact, &mods_path, full);

        // 构造仅大小写不同的 partial（group_path 大写化，模拟 Windows 大小写不敏感路径差异）
        let mut partial = scan_partial_path(&mods_path, &g2).unwrap();
        for g in partial.groups.iter_mut() {
            g.group_path = g.group_path.to_uppercase();
        }
        cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        // 不应因大小写差异产生重复分组
        assert_eq!(updated.groups.len(), 1, "大小写路径差异不应产生重复分组");
        // 原位替换后 group_path 保持大写形态（仅一处，未被追加为第二项）
        assert_eq!(
            updated.groups[0].group_path,
            updated.groups[0].group_path.to_uppercase()
        );
    }

    /// 测试：subtree_remove 移除顶层组节点 + 其全部扁平 mods + 计数重算 + 返回移除数。
    ///
    /// 场景：remove_group 等分组级删除命令（磁盘目录已移除）直接裁剪缓存。
    #[test]
    fn subtree_remove_removes_group_subtree_and_recounts() {
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join("_MANAGED_");
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        // group_1：一启一禁两个模组；group_2：一个启用模组
        fs::create_dir_all(managed.join("group_1").join("ModA")).unwrap();
        fs::write(
            managed.join("group_1").join("ModA").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        fs::create_dir_all(managed.join("group_1").join("DISABLED_ModB")).unwrap();
        fs::write(
            managed
                .join("group_1")
                .join("DISABLED_ModB")
                .join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        fs::create_dir_all(managed.join("group_2").join("ModC")).unwrap();
        fs::write(
            managed.join("group_2").join("ModC").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();

        let mut cache = ModCache::new();
        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        cache.set(TargetGame::GenshinImpact, &mods_path, full);
        assert_eq!(
            cache
                .get(TargetGame::GenshinImpact, &mods_path)
                .unwrap()
                .mods
                .len(),
            3,
            "前置条件：三个模组均进入缓存"
        );

        let removed = cache.subtree_remove(
            TargetGame::GenshinImpact,
            &mods_path,
            &managed.join("group_1"),
        );

        assert_eq!(removed, 2, "应返回被移除的模组数");
        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        assert!(
            updated.groups.iter().all(|g| g.group_name != "group_1"),
            "group_1 组节点应被移除"
        );
        assert!(
            updated.groups.iter().any(|g| g.group_name == "group_2"),
            "group_2 组节点应保留"
        );
        assert!(
            !updated
                .mods
                .iter()
                .any(|m| m.mod_name == "ModA" || m.mod_name == "DISABLED_ModB"),
            "group_1 子树下的模组应从扁平列表移除"
        );
        assert!(
            updated.mods.iter().any(|m| m.mod_name == "ModC"),
            "group_2 的模组应保留"
        );
        // 计数按全量轻量扫描口径重算：total=1 enabled=1 disabled=0
        assert_eq!(
            (
                updated.total_mods_count,
                updated.enabled_mods_count,
                updated.disabled_mods_count
            ),
            (1, 1, 0)
        );
    }

    /// 测试：subtree_remove 缓存未命中时返回 0，不 panic。
    #[test]
    fn subtree_remove_cache_miss_returns_zero() {
        let mut cache = ModCache::new();
        let fake_path = PathBuf::from("/nonexistent/path");
        let removed = cache.subtree_remove(
            TargetGame::GenshinImpact,
            &fake_path,
            &fake_path.join("_MANAGED_").join("group_1"),
        );
        assert_eq!(removed, 0, "缓存未命中应返回 0");
    }

    /// 测试：find_group_node 按完整路径在顶层分组中定位节点（含互斥组）。
    #[test]
    fn find_group_node_matches_by_full_path() {
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join("_MANAGED_");
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        fs::create_dir_all(managed.join("group_1").join("ModA")).unwrap();
        fs::write(
            managed.join("group_1").join("ModA").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        // 互斥组根：根下直接一个模组 + 嵌套子分组一个模组
        fs::create_dir_all(managed.join("#Mutex").join("ModY")).unwrap();
        fs::write(
            managed.join("#Mutex").join("ModY").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        fs::create_dir_all(managed.join("#Mutex").join("Sub").join("ModX")).unwrap();
        fs::write(
            managed
                .join("#Mutex")
                .join("Sub")
                .join("ModX")
                .join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();

        let scan = mod_scanner::scan_mods_light(&mods_path).unwrap();

        let g1 = find_group_node(&scan, &managed.join("group_1"));
        assert!(g1.is_some(), "应能定位 group_1 节点");
        let mutex = find_group_node(&scan, &managed.join("#Mutex"));
        assert!(mutex.is_some(), "应能定位互斥组根节点");
        assert_eq!(
            mutex.unwrap().group_type,
            crate::models::enums::GroupType::MutexGroup
        );
        assert!(
            find_group_node(&scan, &managed.join("group_9")).is_none(),
            "不存在的分组应返回 None"
        );
    }

    /// 测试：find_group_node 在 Windows 下容忍 verbatim 前缀（`\\?\`）与大小写差异。
    ///
    /// 命令层传入的路径经 canonicalize 为 verbatim 形态，而缓存中 full_path
    /// 为 get_mods 时前端传入的普通形态，两者必须仍能匹配。
    #[cfg(windows)]
    #[test]
    fn find_group_node_tolerates_verbatim_and_case_diff() {
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(managed.join("group_1").join("ModA")).unwrap();
        fs::write(
            managed.join("group_1").join("ModA").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();

        let scan = mod_scanner::scan_mods_light(&mods_path).unwrap();
        let g1 = managed.join("group_1");
        // verbatim 前缀形态（canonicalize 产物）
        let verbatim = PathBuf::from(format!("\\\\?\\{}", g1.to_string_lossy()));
        assert!(
            find_group_node(&scan, &verbatim).is_some(),
            "verbatim 前缀形态应能匹配缓存节点"
        );
        // 大小写差异形态
        let upper = PathBuf::from(g1.to_string_lossy().to_uppercase());
        assert!(
            find_group_node(&scan, &upper).is_some(),
            "大小写差异形态应能匹配缓存节点"
        );
    }

    /// 测试：collect_group_mod_paths 递归收集互斥组 children 内模组、排除 None 空槽位。
    #[test]
    fn collect_group_mod_paths_recurses_children_and_excludes_none() {
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join("_MANAGED_");
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        fs::create_dir_all(managed.join("group_1").join("ModA")).unwrap();
        fs::write(
            managed.join("group_1").join("ModA").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        fs::create_dir_all(managed.join("#Mutex").join("ModY")).unwrap();
        fs::write(
            managed.join("#Mutex").join("ModY").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        fs::create_dir_all(managed.join("#Mutex").join("Sub").join("ModX")).unwrap();
        fs::write(
            managed
                .join("#Mutex")
                .join("Sub")
                .join("ModX")
                .join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();

        let scan = mod_scanner::scan_mods_light(&mods_path).unwrap();

        // 普通分组：None 空槽位被排除，仅收集真实模组
        let g1 = find_group_node(&scan, &managed.join("group_1")).unwrap();
        assert!(
            g1.mods.iter().any(|m| m.name == "None"),
            "前置条件：普通分组 light 扫描含 None 空槽位"
        );
        let g1_paths = collect_group_mod_paths(g1);
        assert_eq!(g1_paths.len(), 1, "None 空槽位不应计入");
        assert!(
            g1_paths[0].ends_with("ModA"),
            "应收集真实模组 mod_path，实际 {}",
            g1_paths[0]
        );

        // 互斥组：递归收集嵌套 children（Sub/ModX）内的模组
        let mutex = find_group_node(&scan, &managed.join("#Mutex")).unwrap();
        let mutex_paths = collect_group_mod_paths(mutex);
        assert_eq!(mutex_paths.len(), 2, "根下 + 嵌套子分组内模组均应收集");
        assert!(mutex_paths.iter().any(|p| p.ends_with("ModY")));
        assert!(mutex_paths.iter().any(|p| p.ends_with("ModX")));
    }

    // =====================================================================
    // C-3b：subtree_replace 未变条目签名复用（Arc 指针级零重分配）
    // =====================================================================

    /// C-3b 测试辅助：group_1/{TestMod, ModB} fixture，返回分组目录路径。
    fn setup_two_mod_group(dir: &tempfile::TempDir) -> PathBuf {
        let group1 = dir.path().join("_MANAGED_").join("group_1");
        fs::create_dir_all(group1.join("TestMod")).unwrap();
        fs::write(group1.join("TestMod").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        fs::create_dir_all(group1.join("ModB")).unwrap();
        fs::write(group1.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        group1
    }

    /// C-3b 测试辅助：取组内指定模组的 Arc 引用。
    fn group_mod_arc<'a>(
        scan: &'a ScanResult,
        group_name: &str,
        mod_name: &str,
    ) -> &'a Arc<ModData> {
        scan.groups
            .iter()
            .find(|g| g.group_name == group_name)
            .unwrap_or_else(|| panic!("应存在分组 {group_name}"))
            .mods
            .iter()
            .find(|m| m.name == mod_name)
            .unwrap_or_else(|| panic!("组 {group_name} 应含模组 {mod_name}"))
    }

    /// C-3b 测试辅助：取扁平列表指定模组的 Arc 引用。
    fn flat_mod_arc<'a>(scan: &'a ScanResult, mod_name: &str) -> &'a Arc<ModData> {
        scan.mods
            .iter()
            .find(|m| m.name == mod_name)
            .unwrap_or_else(|| panic!("扁平列表应含模组 {mod_name}"))
    }

    /// C-3b：同组 partial 重扫（磁盘无变化）合并后，未变模组复用旧 Arc 实例
    ///（`Arc::ptr_eq` 证明零重分配），且签名保持 → 再次合并仍命中。
    #[test]
    fn subtree_replace_reuses_unchanged_mods_by_signature() {
        use crate::core::mod_scanner::scan_partial_path;
        let (dir, mods_path) = setup_test_mods();
        let group1 = setup_two_mod_group(&dir);

        let mut cache = ModCache::new();
        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        cache.set(TargetGame::GenshinImpact, &mods_path, full);

        // 快照旧实例（get 返回共享 Arc，subtree_replace 的 make_mut 对其 COW 保护）
        let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let old_group_test = group_mod_arc(&old, "group_1", "TestMod");
        let old_group_b = group_mod_arc(&old, "group_1", "ModB");
        let old_flat_test = flat_mod_arc(&old, "TestMod");
        assert!(
            Arc::ptr_eq(old_group_test, old_flat_test),
            "前置：set 时组内条目与扁平条目共享同一 Arc"
        );

        // 同组 partial 重扫（未修改任何文件）
        let partial = scan_partial_path(&mods_path, &group1).unwrap();
        cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let new_group_test = group_mod_arc(&updated, "group_1", "TestMod");
        let new_group_b = group_mod_arc(&updated, "group_1", "ModB");
        let new_flat_test = flat_mod_arc(&updated, "TestMod");
        let new_flat_b = flat_mod_arc(&updated, "ModB");
        assert!(
            Arc::ptr_eq(new_group_test, old_group_test),
            "未变模组 TestMod 组内条目应复用旧 Arc（零重分配）"
        );
        assert!(
            Arc::ptr_eq(new_group_b, old_group_b),
            "未变模组 ModB 组内条目应复用旧 Arc（零重分配）"
        );
        assert!(
            Arc::ptr_eq(new_flat_test, old_group_test),
            "扁平条目应复用同一旧 Arc"
        );
        assert!(
            Arc::ptr_eq(new_flat_b, old_group_b),
            "扁平条目应复用同一旧 Arc"
        );

        // 复用后签名保持（旧实例签名即命中值）→ 二次合并仍命中
        let partial2 = scan_partial_path(&mods_path, &group1).unwrap();
        cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial2);
        let updated2 = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        assert!(
            Arc::ptr_eq(
                group_mod_arc(&updated2, "group_1", "TestMod"),
                old_group_test
            ),
            "签名保持 → 二次合并仍复用旧 Arc"
        );
    }

    /// C-3b：fav 状态变化（mtime 源变化）→ 签名不等 → 该条目不复用（新实例
    /// 反映新收藏状态），未变兄弟仍复用。
    #[test]
    fn subtree_replace_replaces_entry_when_fav_changes() {
        use crate::core::mod_scanner::scan_partial_path;
        let (dir, mods_path) = setup_test_mods();
        let group1 = setup_two_mod_group(&dir);

        let mut cache = ModCache::new();
        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        cache.set(TargetGame::GenshinImpact, &mods_path, full);
        let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let old_group_test = group_mod_arc(&old, "group_1", "TestMod");
        let old_group_b = group_mod_arc(&old, "group_1", "ModB");

        // 变化：TestMod 创建 fav 文件
        fs::write(group1.join("TestMod").join("fav"), "").unwrap();
        let partial = scan_partial_path(&mods_path, &group1).unwrap();
        cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let new_group_test = group_mod_arc(&updated, "group_1", "TestMod");
        let new_group_b = group_mod_arc(&updated, "group_1", "ModB");
        assert!(
            !Arc::ptr_eq(new_group_test, old_group_test),
            "fav 变化的模组不得复用旧实例"
        );
        assert!(new_group_test.is_favorite, "新实例应反映 fav 状态");
        assert!(
            Arc::ptr_eq(new_group_b, old_group_b),
            "未变兄弟模组仍复用旧 Arc"
        );
    }

    // =====================================================================
    // C-3a：patch_single_mod 模组级增量 patch（单模组替换/新增/删除 + 兄弟 Arc 复用）
    // =====================================================================

    /// C-3a 测试辅助：构造互斥树 fixture `#M/{Sub1/ModX, ModW, Sub2/ModY}`，返回各目录。
    fn setup_mutex_tree(dir: &tempfile::TempDir) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
        let managed = dir.path().join("_MANAGED_");
        let root = managed.join("#M");
        let sub1 = root.join("Sub1");
        let sub2 = root.join("Sub2");
        fs::create_dir_all(sub1.join("ModX")).unwrap();
        fs::write(sub1.join("ModX").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        fs::create_dir_all(root.join("ModW")).unwrap();
        fs::write(root.join("ModW").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        fs::create_dir_all(sub2.join("ModY")).unwrap();
        fs::write(sub2.join("ModY").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        let mod_x = sub1.join("ModX");
        (managed, root, sub1, sub2, mod_x)
    }

    /// C-3a：替换——位置匹配替换、mod_index 保留旧值、兄弟 Arc 复用、
    /// 扁平列表同步（共享同一实例）、组级派生与计数重算。
    #[test]
    fn patch_single_mod_replaces_preserving_mod_index_and_siblings() {
        use crate::core::mod_scanner::scan_single_mod;
        let (dir, mods_path) = setup_test_mods();
        let group1 = setup_two_mod_group(&dir);

        let mut cache = ModCache::new();
        cache.set(
            TargetGame::GenshinImpact,
            &mods_path,
            mod_scanner::scan_mods_light(&mods_path).unwrap(),
        );
        // 旧实例快照（持有共享 Arc → patch 的 make_mut 走写时拷贝路径，指针断言有效）
        let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let old_test = group_mod_arc(&old, "group_1", "TestMod");
        let old_index = old_test.mod_index;

        // 磁盘变化：TestMod 加 fav（深部变更场景）
        fs::write(group1.join("TestMod").join("fav"), "").unwrap();
        let (arc, _) = scan_single_mod(&mods_path, &group1.join("TestMod")).unwrap();
        assert!(arc.is_favorite, "前置：新扫描应反映 fav 状态");

        let patched = cache.patch_single_mod(
            TargetGame::GenshinImpact,
            &mods_path,
            &group1.join("TestMod"),
            Some(arc),
        );
        let group_node = patched.expect("patch 应返回受影响组节点");
        assert_eq!(group_node.group_name, "group_1");

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let g1 = updated
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .unwrap();
        // 位置不变 + mod_index 保留 + 新状态生效
        assert_eq!(g1.mods.len(), 3, "None + TestMod + ModB");
        let pos = g1
            .mods
            .iter()
            .position(|m| m.name == "TestMod")
            .expect("TestMod 应在组内");
        assert_eq!(
            g1.mods[pos].mod_index, old_index,
            "替换时 mod_index 保留旧实例值"
        );
        assert!(g1.mods[pos].is_favorite, "组内新实例应反映 fav");
        assert!(!Arc::ptr_eq(&g1.mods[pos], old_test), "被替换条目为新实例");
        // 兄弟 Arc 复用（零重扫零深拷贝）
        assert!(
            Arc::ptr_eq(
                group_mod_arc(&updated, "group_1", "ModB"),
                group_mod_arc(&old, "group_1", "ModB")
            ),
            "未变兄弟 ModB 组内条目 Arc 复用"
        );
        // 扁平列表同步：同位置替换且与组内共享同一 Arc 实例
        let flat_test = flat_mod_arc(&updated, "TestMod");
        assert!(
            Arc::ptr_eq(flat_test, &g1.mods[pos]),
            "扁平条目与组内条目共享同一 Arc"
        );
        assert!(flat_test.is_favorite);
        assert_eq!(updated.mods.len(), old.mods.len(), "扁平列表长度不变");
        assert_eq!(updated.total_mods_count, 2);
    }

    /// C-3a：新增——无匹配条目时追加组内末位、mod_index 为末位、计数/扁平同步。
    #[test]
    fn patch_single_mod_inserts_at_end() {
        use crate::core::mod_scanner::scan_single_mod;
        let (dir, mods_path) = setup_test_mods();
        let group1 = setup_two_mod_group(&dir);

        let mut cache = ModCache::new();
        cache.set(
            TargetGame::GenshinImpact,
            &mods_path,
            mod_scanner::scan_mods_light(&mods_path).unwrap(),
        );

        // 磁盘新增 ModC
        let mod_c = group1.join("ModC");
        fs::create_dir_all(&mod_c).unwrap();
        fs::write(mod_c.join("mod.ini"), "[Section]\nx=1\n").unwrap();
        let (arc, _) = scan_single_mod(&mods_path, &mod_c).unwrap();

        let group_node = cache
            .patch_single_mod(TargetGame::GenshinImpact, &mods_path, &mod_c, Some(arc))
            .expect("新增 patch 应成功");

        assert_eq!(
            group_node.mod_count, 3,
            "mod_count 重算（TestMod + ModB + ModC）"
        );
        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let g1 = updated
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .unwrap();
        assert_eq!(g1.mods.len(), 4, "None + TestMod + ModB + ModC");
        let last = g1.mods.last().expect("组内应有条目");
        assert_eq!(
            last.name, "ModC",
            "新增模组置于组内末位（排序位由下次组级扫描校正）"
        );
        assert_eq!(last.mod_index, 3, "新增条目 mod_index 为组内末位");
        assert_eq!(updated.total_mods_count, 3, "扁平计数重算");
        assert!(
            updated.mods.iter().any(|m| m.name == "ModC"),
            "扁平列表应含新增模组"
        );
    }

    /// C-3a：删除——组内 + 扁平移除、计数重算、兄弟条目不动。
    #[test]
    fn patch_single_mod_removes_entry() {
        let (dir, mods_path) = setup_test_mods();
        let group1 = setup_two_mod_group(&dir);

        let mut cache = ModCache::new();
        cache.set(
            TargetGame::GenshinImpact,
            &mods_path,
            mod_scanner::scan_mods_light(&mods_path).unwrap(),
        );
        let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let old_test = group_mod_arc(&old, "group_1", "TestMod");

        let group_node = cache
            .patch_single_mod(
                TargetGame::GenshinImpact,
                &mods_path,
                &group1.join("ModB"),
                None,
            )
            .expect("删除 patch 应成功");
        assert_eq!(group_node.mod_count, 1, "mod_count 重算");

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let g1 = updated
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .unwrap();
        assert!(g1.mods.iter().all(|m| m.name != "ModB"), "组内应移除 ModB");
        assert!(
            updated.mods.iter().all(|m| m.name != "ModB"),
            "扁平列表应移除 ModB"
        );
        assert_eq!(updated.total_mods_count, 1, "计数重算");
        assert_eq!(updated.mods.len(), 1, "扁平列表长度同步");
        // 兄弟条目不动（Arc 复用）
        assert!(
            Arc::ptr_eq(group_mod_arc(&updated, "group_1", "TestMod"), old_test),
            "删除的兄弟条目（TestMod）Arc 复用不动"
        );
    }

    /// C-3a：深层 mutex 树定位——2 层嵌套下 patch 链上节点重建、链外兄弟子树
    /// Arc 指针不变（make_mut 语义验证）。
    #[test]
    fn patch_single_mod_deep_mutex_chain_rebuild_and_sibling_reuse() {
        use crate::core::mod_scanner::scan_single_mod;
        let dir = tempfile::TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let (_managed, _root, _sub1, _sub2, mod_x) = setup_mutex_tree(&dir);

        let mut cache = ModCache::new();
        cache.set(
            TargetGame::GenshinImpact,
            &mods_path,
            mod_scanner::scan_mods_light(&mods_path).unwrap(),
        );
        let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let old_mutex = old
            .groups
            .iter()
            .find(|g| g.group_name == "#M")
            .expect("应含互斥组根节点");
        let old_sub2 = old_mutex
            .children
            .iter()
            .find(|c| c.group_name == "Sub2")
            .expect("应含 Sub2 子分组");
        let old_mod_w = old_mutex
            .mods
            .iter()
            .find(|m| m.name == "ModW")
            .expect("根下应含 ModW");

        // 磁盘变化：ModX 加 fav
        fs::write(mod_x.join("fav"), "").unwrap();
        let (arc, _) = scan_single_mod(&mods_path, &mod_x).unwrap();

        let group_node = cache
            .patch_single_mod(TargetGame::GenshinImpact, &mods_path, &mod_x, Some(arc))
            .expect("深层 patch 应成功");
        assert_eq!(group_node.group_name, "#M", "返回顶层组节点（delta 形状）");

        // 结构断言辅助：定位子分组（嵌套 fn 规避闭包生命周期推断）
        fn find_child<'a>(node: &'a ModGroupData, name: &str) -> &'a Arc<ModGroupData> {
            node.children
                .iter()
                .find(|c| c.group_name == name)
                .unwrap_or_else(|| panic!("应含子分组 {name}"))
        }
        let new_sub1 = find_child(&group_node, "Sub1");
        let new_sub2 = find_child(&group_node, "Sub2");
        let patched_mod_x = new_sub1
            .mods
            .iter()
            .find(|m| m.name == "ModX")
            .expect("Sub1 内应含 ModX");
        assert!(patched_mod_x.is_favorite, "深部 patch 生效（fav 反映）");

        // 缓存内验证：链外兄弟 Sub2 子树 Arc 指针不变（复用）；根下 ModW 条目复用
        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let new_mutex = updated
            .groups
            .iter()
            .find(|g| g.group_name == "#M")
            .unwrap();
        let cached_sub2 = new_mutex
            .children
            .iter()
            .find(|c| c.group_name == "Sub2")
            .expect("缓存应保留 Sub2");
        assert!(
            Arc::ptr_eq(cached_sub2, old_sub2),
            "链外兄弟子树（Sub2）Arc 指针不变——make_mut 仅重建定位链"
        );
        let cached_mod_w = new_mutex
            .mods
            .iter()
            .find(|m| m.name == "ModW")
            .expect("根下应含 ModW");
        assert!(
            Arc::ptr_eq(cached_mod_w, old_mod_w),
            "同组内未 patch 的兄弟条目（ModW）Arc 复用"
        );
        // 扁平同步
        assert!(
            updated
                .mods
                .iter()
                .any(|m| m.name == "ModX" && m.is_favorite),
            "扁平列表同步深层 patch"
        );
        assert_eq!(updated.total_mods_count, 3);
        assert_eq!(new_sub2.mods.len(), 1, "Sub2 子树内容不变");
    }

    /// C-3a：缓存 miss / 无定位（路径实为子分组节点）/ 删除无匹配 → None。
    #[test]
    fn patch_single_mod_miss_and_no_match_return_none() {
        use crate::core::mod_scanner::scan_single_mod;
        // 缓存 miss
        let mut empty_cache = ModCache::new();
        let fake = PathBuf::from("/nonexistent");
        assert!(empty_cache
            .patch_single_mod(TargetGame::GenshinImpact, &fake, &fake.join("x"), None)
            .is_none());

        // 有缓存但目标为子分组节点自身（非模组目录）→ 无定位
        let dir = tempfile::TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let (_managed, root, _sub1, _sub2, _mod_x) = setup_mutex_tree(&dir);
        let mut cache = ModCache::new();
        cache.set(
            TargetGame::GenshinImpact,
            &mods_path,
            mod_scanner::scan_mods_light(&mods_path).unwrap(),
        );
        assert!(
            cache
                .patch_single_mod(
                    TargetGame::GenshinImpact,
                    &mods_path,
                    &root.join("Sub1"),
                    None
                )
                .is_none(),
            "子分组节点路径（删除语义）在组内 mods 无匹配 → None 降级"
        );

        // 删除缓存中不存在的模组路径 → 无匹配
        assert!(cache
            .patch_single_mod(
                TargetGame::GenshinImpact,
                &mods_path,
                &root.join("Sub1").join("Ghost"),
                None
            )
            .is_none());

        // 合法模组 patch 不受影响（对照）
        let (_arc, _) = scan_single_mod(&mods_path, &root.join("Sub1").join("ModX")).unwrap();
        assert!(cache
            .patch_single_mod(
                TargetGame::GenshinImpact,
                &mods_path,
                &root.join("Sub1").join("ModX"),
                Some(_arc)
            )
            .is_some());
    }

    /// C-3b 正确性边界：签名必须覆盖 is_active 派生源（组 selectedindex）。
    ///
    /// 两启用模组场景下 is_active 回退到 selectedindex 文件派生；select 改写
    /// selectedindex 后增量合并，模组必须以新实例反映新激活态——若签名漏掉
    /// selectedindex 源，未动模组被复用旧实例，is_active 停留在旧值。
    #[test]
    fn subtree_replace_signature_covers_selectedindex_source() {
        use crate::core::mod_scanner::scan_partial_path;
        let dir = tempfile::TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join("_MANAGED_");
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let group1 = managed.join("group_1");
        fs::create_dir_all(group1.join("ModA")).unwrap();
        fs::write(group1.join("ModA").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        fs::create_dir_all(group1.join("ModB")).unwrap();
        fs::write(group1.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();

        let mut cache = ModCache::new();
        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        cache.set(TargetGame::GenshinImpact, &mods_path, full);
        // 前置：两启用 → 多启用回退 selectedindex（首扫创建为 0）→ 无模组激活
        let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let old_a = group_mod_arc(&old, "group_1", "ModA");
        assert!(
            !old_a.is_active,
            "前置：初始 selectedindex=0 时 ModA 未激活"
        );

        // select：改写 selectedindex=1（组内排序 [None, ModA, ModB] → 选中 ModA）
        fs::write(group1.join("selectedindex"), "1").unwrap();
        let partial = scan_partial_path(&mods_path, &group1).unwrap();
        cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let new_a = group_mod_arc(&updated, "group_1", "ModA");
        let new_b = group_mod_arc(&updated, "group_1", "ModB");
        assert!(
            !Arc::ptr_eq(new_a, old_a),
            "selectedindex 源变化 → ModA 不得复用旧实例"
        );
        assert!(
            new_a.is_active,
            "selectedindex=1 → ModA 应激活（新实例派生）"
        );
        assert!(!new_b.is_active, "ModB 不应激活");
    }

    /// C-3b：互斥组嵌套 children 的深层组内模组同样按签名递归复用。
    #[test]
    fn subtree_replace_reuses_mutex_children_entries() {
        use crate::core::mod_scanner::scan_partial_path;
        let dir = tempfile::TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join("_MANAGED_");
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let mutex_root = managed.join("#Mutex");
        fs::create_dir_all(mutex_root.join("Sub").join("ModX")).unwrap();
        fs::write(
            mutex_root.join("Sub").join("ModX").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        fs::create_dir_all(mutex_root.join("ModY")).unwrap();
        fs::write(mutex_root.join("ModY").join("mod.ini"), "[Section]\nx=1\n").unwrap();

        let mut cache = ModCache::new();
        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        cache.set(TargetGame::GenshinImpact, &mods_path, full);
        let old = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let old_mutex = old
            .groups
            .iter()
            .find(|g| g.group_name == "#Mutex")
            .expect("应含 #Mutex 根节点");
        let old_sub_x = old_mutex
            .children
            .iter()
            .find(|c| c.group_name == "Sub")
            .expect("应含嵌套子分组 Sub")
            .mods
            .iter()
            .find(|m| m.name == "ModX")
            .expect("Sub 内应含 ModX");
        let old_flat_x = flat_mod_arc(&old, "ModX");
        assert!(
            Arc::ptr_eq(old_sub_x, old_flat_x),
            "前置：互斥组深层条目与扁平条目共享同一 Arc"
        );

        // #Mutex 根整组局部重扫（无任何磁盘变化）
        let partial = scan_partial_path(&mods_path, &mutex_root).unwrap();
        cache.subtree_replace(TargetGame::GenshinImpact, &mods_path, partial);

        let updated = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let new_mutex = updated
            .groups
            .iter()
            .find(|g| g.group_name == "#Mutex")
            .expect("合并后应含 #Mutex 根节点");
        let new_sub_x = new_mutex
            .children
            .iter()
            .find(|c| c.group_name == "Sub")
            .expect("合并后应含嵌套子分组 Sub")
            .mods
            .iter()
            .find(|m| m.name == "ModX")
            .expect("Sub 内应含 ModX");
        assert!(
            Arc::ptr_eq(new_sub_x, old_sub_x),
            "嵌套 children 深层组内模组应复用旧 Arc"
        );
        assert!(
            Arc::ptr_eq(flat_mod_arc(&updated, "ModX"), old_sub_x),
            "互斥组扁平条目应复用同一旧 Arc"
        );
    }

    // =====================================================================
    // 选择直写快路径：patch_selected_index 缓存定向 patch（零扫描）
    // =====================================================================

    /// patch_selected_index：两启用模组（多启用回退 selectedindex）场景下，
    /// patch 结果与「落盘 selectedindex 后全量轻扫」完全一致（组级字段 +
    /// 组内 is_active + 扁平列表 is_active），幂等重复写收敛。
    #[test]
    fn patch_selected_index_matches_light_scan_semantics() {
        let dir = tempfile::TempDir::new().unwrap();
        let mods_path = dir.path().to_path_buf();
        let managed = dir.path().join("_MANAGED_");
        fs::write(dir.path().join("d3dx.ini"), "; test\n").unwrap();
        let group1 = managed.join("group_1");
        fs::create_dir_all(group1.join("ModA")).unwrap();
        fs::write(group1.join("ModA").join("mod.ini"), "[Section]\nx=1\n").unwrap();
        fs::create_dir_all(group1.join("ModB")).unwrap();
        fs::write(group1.join("ModB").join("mod.ini"), "[Section]\nx=1\n").unwrap();

        let mut cache = ModCache::new();
        let full = mod_scanner::scan_mods_light(&mods_path).unwrap();
        cache.set(TargetGame::GenshinImpact, &mods_path, full);
        // 前置：首扫 selectedindex=0（多启用回退）→ 无模组激活
        {
            let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
            let g = cached
                .groups
                .iter()
                .find(|g| g.group_name == "group_1")
                .unwrap();
            assert!(!g.mods.iter().any(|m| m.name != "None" && m.is_active));
        }

        // patch selectedindex=1（组内排序 [None, ModA, ModB] → 选中 ModA）
        assert!(cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 1));

        // 与全量轻扫等价性校验：落盘同一 selectedindex 后重扫对比
        fs::write(group1.join("selectedindex"), "1").unwrap();
        let rescan = mod_scanner::scan_mods_light(&mods_path).unwrap();
        let rescan_g = rescan
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .unwrap();

        let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let g = cached
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .unwrap();
        assert_eq!(g.active_mod_index, rescan_g.active_mod_index);
        assert_eq!(g.is_active, rescan_g.is_active);
        for (cm, rm) in g.mods.iter().zip(rescan_g.mods.iter()) {
            assert_eq!(
                cm.is_active, rm.is_active,
                "组内 {} is_active 应与重扫一致",
                cm.name
            );
        }
        assert_eq!(g.active_mod_index, 1, "selectedindex=1 → ModA 激活");
        assert!(g.mods.iter().find(|m| m.name == "ModA").unwrap().is_active);
        assert!(!g.mods.iter().find(|m| m.name == "ModB").unwrap().is_active);
        // 扁平列表同步
        let flat_a = flat_mod_arc(&cached, "ModA");
        assert!(flat_a.is_active, "扁平列表 ModA is_active 应同步");
        assert!(!flat_mod_arc(&cached, "ModB").is_active);
        // 释放对缓存 Arc 的外部持有（否则下一次 patch 触发快路径守卫返回 false）
        drop(cached);

        // 幂等重复写：切到 2 → ModB 激活、ModA 失活，再次收敛一致
        assert!(cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 2));
        let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let g = cached
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .unwrap();
        assert_eq!(g.active_mod_index, 2);
        assert!(!g.mods.iter().find(|m| m.name == "ModA").unwrap().is_active);
        assert!(g.mods.iter().find(|m| m.name == "ModB").unwrap().is_active);
        assert!(flat_mod_arc(&cached, "ModB").is_active);
    }

    /// patch_selected_index：单启用模组场景（派生口径=该启用模组激活，
    /// selectedindex 不改变激活位——与轻量扫描一致）。
    #[test]
    fn patch_selected_index_single_enabled_keeps_scan_semantics() {
        let (_dir, mods_path) = setup_test_mods();
        // setup_test_mods：group_1/TestMod 单启用模组
        let group1 = mods_path.join("_MANAGED_").join("group_1");

        let mut cache = ModCache::new();
        cache.set(
            TargetGame::GenshinImpact,
            &mods_path,
            mod_scanner::scan_mods_light(&mods_path).unwrap(),
        );

        // 单启用 → 无论 selectedindex 写何值，激活位恒为该启用模组（索引 1）
        assert!(cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 1));
        let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let g = cached
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .unwrap();
        assert_eq!(g.active_mod_index, 1);
        assert!(
            g.mods
                .iter()
                .find(|m| m.mod_name == "TestMod")
                .unwrap()
                .is_active
        );
    }

    /// patch_selected_index：缓存未命中 / 分组不在缓存 → false（不 panic、不失效）。
    #[test]
    fn patch_selected_index_miss_returns_false() {
        let (_dir, mods_path) = setup_test_mods();
        let group1 = mods_path.join("_MANAGED_").join("group_1");

        // 缓存未命中（空缓存）
        let mut cache = ModCache::new();
        assert!(!cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 1));

        // 有缓存但分组不在缓存（group_2 不存在）
        cache.set(
            TargetGame::GenshinImpact,
            &mods_path,
            mod_scanner::scan_mods_light(&mods_path).unwrap(),
        );
        assert!(!cache.patch_selected_index(
            TargetGame::GenshinImpact,
            &mods_path,
            &mods_path.join("_MANAGED_").join("group_2"),
            1
        ));
    }

    /// patch_selected_index：缓存 Arc 被外部持有（strong_count>1）时不走整树 COW，
    /// 直接降级返回 false（由 watcher 增量兜底）；释放外部引用后恢复快路径生效。
    ///
    /// 回归场景（2026-09-06）：get_mods 序列化在途 clone 缓存 Arc 时，`Arc::make_mut`
    /// 深拷贝整棵万级模组树（实测数十秒），选择命令整体 24s+。快路径守卫阻止该行为。
    #[test]
    fn patch_selected_index_skips_cow_when_arc_externally_held() {
        let (_dir, mods_path) = setup_test_mods();
        let group1 = mods_path.join("_MANAGED_").join("group_1");

        let mut cache = ModCache::new();
        cache.set(
            TargetGame::GenshinImpact,
            &mods_path,
            mod_scanner::scan_mods_light(&mods_path).unwrap(),
        );

        // 模拟外部持有：get() 克隆 Arc 并保活（strong_count>1）
        {
            let _held = cache
                .get(TargetGame::GenshinImpact, &mods_path)
                .expect("缓存存在");
            // 外部 Arc 存活期间 patch → 拒绝深拷贝，返回 false
            assert!(!cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 1));
        }
        // 外部引用释放后恢复快路径
        assert!(cache.patch_selected_index(TargetGame::GenshinImpact, &mods_path, &group1, 1));
        let cached = cache.get(TargetGame::GenshinImpact, &mods_path).unwrap();
        let g = cached
            .groups
            .iter()
            .find(|g| g.group_name == "group_1")
            .unwrap();
        assert_eq!(g.active_mod_index, 1, "快路径恢复后派生生效");
    }
}
