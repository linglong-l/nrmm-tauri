//! 模组缓存子树与模组级增量 patch 域
//!
//! 含 `ModCache` 的子树替换/裁剪（`subtree_replace`/`subtree_remove`）与
//! 模组级增量 patch（`patch_single_mod`/`patch_mod_entry`/`remove_mod_entry`/
//! `patch_selected_index`）及其私有辅助。路径原语来自 [`crate::core::mod_cache_path`]，
//! 结构与单例来自 [`crate::core::mod_cache_core`]。

use crate::core::mod_cache_core::ModCache;
use crate::core::mod_cache_path::{path_cmp_key, path_equivalent, path_string_eq, path_within};
use crate::core::mod_scan_light::ScanResult;
use crate::core::mod_scan_util::{derive_group_active_state, group_order_cmp, recount_mods};
use crate::models::enums::{GroupType, TargetGame};
use crate::models::mod_data::{ModData, ModGroupData};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

impl ModCache {
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
        let Some(owned) = self.try_get_owned_result(game, mods_path, "subtree_replace") else {
            // 缓存未命中、或大树被外部持有（深拷贝万级树代价数秒～数十秒）→ 降级：
            // partial 仅含变更集，跳过合并后旧缓存不可保留（否则前端读到陈旧数据）
            // → 标记该游戏缓存失效，下次 get_mods 全量重扫兜底（宁全量不陈旧）。
            self.invalidate_game(game);
            return;
        };

        // 记录 partial 各分组子树根（用于删除传播；必须在消费 partial_result.groups 之前计算）
        let partial_roots: Vec<std::path::PathBuf> = partial_result
            .groups
            .iter()
            .map(|g| std::path::PathBuf::from(&g.full_path))
            .collect();

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
        let Some(owned) = self.try_get_owned_result(game, mods_path, "subtree_remove") else {
            // 缓存未命中、或大树被外部持有 → 降级：磁盘分组已移除，缓存不可保留
            // 陈旧子树 → 标记失效，下次 get_mods 全量重扫兜底
            self.invalidate_game(game);
            return 0;
        };

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
        // 缓存未命中、或大树被外部持有 → None 交调用方降级组级重扫
        let owned = self.try_get_owned_result(game, mods_path, "patch_single_mod")?;

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
        // 缓存未命中、或大树被外部持有 → false 交调用方降级（组级重扫修正缓存）
        let Some(owned) = self.try_get_owned_result(game, mods_path, "patch_mod_entry") else {
            return false;
        };

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
        // 缓存未命中、或大树被外部持有 → false 交调用方降级（组级重扫修正缓存）
        let Some(owned) = self.try_get_owned_result(game, mods_path, "remove_mod_entry") else {
            return false;
        };
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
    /// （调用方不失效整缓存，由下次全量扫描按磁盘对账兜底；watcher 忽略
    /// `selectedindex` 事件，见 `watcher_core::is_ignored_watcher_path`）。
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
        // 缓存未命中、或大树被外部持有 → false（调用方跳过缓存 patch，
        // selectedindex 按磁盘对账由下次全量扫描兜底——watcher 已忽略该文件事件）
        let start = Instant::now();
        let Some(owned) = self.try_get_owned_result(game, mods_path, "patch_selected_index") else {
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
                    // 替换：保留旧 mod_index / slot_id（单模组扫描无法得出组内序；
                    // 目录位置不变 → 展示序与目录序槽位身份语义均不变）
                    let patched = Arc::make_mut(&mut arc);
                    patched.mod_index = group.mods[pos].mod_index;
                    patched.slot_id = group.mods[pos].slot_id;
                    group.mods[pos] = Arc::clone(&arc);
                    log::debug!(
                        "[mod_cache] mod 级 patch | mod={} op=replace",
                        mod_full_path.display()
                    );
                }
                None => {
                    // 新增：组内末位占位（mod_index / slot_id 同置 len；真实目录序
                    // 槽位由下次组级扫描校正。占位值与既有槽位集合 {1..len-1} 不冲突；
                    // 删除后的稀疏场景可能撞号，但新模组在下一次 update 前本无
                    // $managed_slot_id 注入，不影响已注入模组的选择判定）
                    let patched = Arc::make_mut(&mut arc);
                    patched.mod_index = group.mods.len() as u32;
                    patched.slot_id = group.mods.len() as u32;
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

#[cfg(test)]
#[path = "mod_cache_patch_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mod_cache_patch2_tests.rs"]
mod tests2;
