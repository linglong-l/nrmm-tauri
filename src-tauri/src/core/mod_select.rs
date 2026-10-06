//! 模组选择域：selectedindex 持久化、切换选中、缓存索引校正、取消选中
//!
//! 从 mod_manager.rs 拆出（2026-09-19 Phase 4-4）。
//! 依赖 mod_scanner（轻扫定位分组）、mod_path_utils（路径匹配）、
//! watcher_ring::WatcherGuard、mod_results（UpdateResult）。

use anyhow::{anyhow, bail, Result};
use std::path::Path;

use crate::core::constants;
use crate::core::mod_path_utils::{mod_path_matches, path_eq_ignore_case};
use crate::core::mod_results::UpdateResult;
use crate::core::watcher_ring::WatcherGuard;

use crate::models::enums::{GroupType, TargetGame};
use crate::models::settings::AppSettings;
use crate::sel_dbg;

/// 直写分组的 selectedindex 标记文件（选择快路径，免全量轻扫）。
///
/// 与 `switch_mod` 的区别：调用方已持有 group_path（前端 group.groupPath），
/// 无需 `scan_mods_light` 全量扫描定位分组目录——单次 O(1) 原子写。
///
/// 守卫：目录名必须为严格 group_xx（`is_normal_group_dir`，拒绝 group_0/前导零/
/// 互斥目录/任意路径），防止将标记文件写入非分组位置；调用方须另行校验路径
/// 位于 `_MANAGED_` 下。
///
/// # 参数
/// - `group_path`: 分组目录绝对路径（group_xx）
/// - `index`: 待写入的选中索引（0 = None 槽位；真实模组从 1 起）
///
/// # 错误
/// 目录名非 group_xx、或原子写失败（如目录不存在）时返回 `Err`。
pub fn write_selectedindex_direct(group_path: &Path, index: u32) -> Result<()> {
    let dir_name = group_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| anyhow!("group_path has no file name: {:?}", group_path))?;
    if crate::core::mod_scan_util::is_normal_group_dir(&dir_name).is_none() {
        bail!("not a group_xx directory: {:?}", group_path);
    }
    let selectedindex_path = group_path.join(constants::SELECTED_INDEX_FILE);
    // 对齐 NRMM / switch_mod：写 selectedindex 前暂停文件监听器（引用计数守卫），
    // 防止写入触发缓存增量更新竞态
    let _guard = WatcherGuard::new();
    crate::utils::atomic_write(&selectedindex_path, index.to_string().as_bytes())?;
    log::debug!(
        "[core::mod_manager] [write_selectedindex_direct] wrote selectedindex | path={} content={}",
        selectedindex_path.display(),
        index
    );
    Ok(())
}

/// 切换普通分组选中的模组
///
/// 普通分组（group_xx）是互斥的：选中一个模组会自动禁用同组其他模组。
/// 实现方式：重命名目录（移除目标的 DISABLED 前缀，给其他模组添加前缀），然后调用 update_mod_data。
///
/// # 参数
/// - `game`: 目标游戏
/// - `game_mods_path`: 游戏 Mods 目录
/// - `settings`: 应用设置
/// - `group_index`: 分组索引
/// - `mod_index`: 模组索引（前端视角的组内下标；仅当 `mod_path` 缺失或未命中时作为回退）
/// - `mod_path`: 目标模组绝对路径（可选）。传入时**优先按路径**在后端扫描结果中解析
///   真实 `mod_index`，见下方「P3 索引空间对齐」。
///
/// # P3 索引空间对齐（为何必须按路径定位）
/// 前端展示顺序存在「本地近似重排」（收藏前置 / 新增置末位等零扫描 patch），
/// 与后端 `mod_scanner` 的确定性排序序（`mod_order_cmp`）可分叉，因此前端传入的
/// 整数 `mod_index` 并非可靠真相源——直接用它会把 selectedindex 写到错误模组上。
/// 本函数以 `mod_path` 在 `scan_mods_light` 结果中定位模组并取其 `mod_index`
/// （后端排序后的真实下标）；未传 `mod_path` 或路径未命中时回退到传入下标，行为不劣化。
///
/// # P4 失败显式化
/// 找不到对应 NormalGroup（分组已被删除/重命名）时返回 `Err`——此前仅 `log::warn`
/// 仍返回 `Ok`，前端据此乐观更新 UI 显示已选中而磁盘 selectedindex 实际未写，
/// 重载后选择丢失且无任何报错。
pub fn switch_mod(
    _game: TargetGame,
    game_mods_path: &Path,
    _settings: &AppSettings,
    group_index: u32,
    mod_index: u32,
    mod_path: Option<&str>,
) -> Result<UpdateResult> {
    sel_dbg!(
        "mod_manager",
        "switch_mod",
        "调用链: select_mod → switch_mod | 入口 | 分组索引={} 模组索引={} 模组路径={:?} 路径={:?}",
        group_index,
        mod_index,
        mod_path,
        game_mods_path
    );
    log::debug!(
        "[core::mod_manager] [switch_mod] Starting | group={} mod={} path={:?}",
        group_index,
        mod_index,
        game_mods_path
    );
    let _s = std::time::Instant::now();
    // 使用轻量扫描以匹配前端索引（None 在 mod_index=0，真实模组从 1 开始）
    let scan_result = crate::core::mod_scan_light::scan_mods_light(game_mods_path)?;
    log::debug!(
        "[core::mod_manager] [switch_mod] scan completed | total_groups={} total_mods={}",
        scan_result.groups.len(),
        scan_result.mods.len()
    );
    sel_dbg!(
        "mod_manager",
        "switch_mod",
        "步骤=扫描完成 | 分组数={} 模组数={}",
        scan_result.groups.len(),
        scan_result.mods.len()
    );

    // 先找出分组目录路径（用于写入 selectedindex 文件）
    // 严格限制：仅 NormalGroup（group_xx 目录）允许写入 selectedindex 标记文件。
    // MutexGroup（非group目录）不参与标记文件生成，仅参与模组读取和 hash 冲突检测。
    // 通过 group_type == NormalGroup 校验避免误在非group目录下写入标记文件。
    let group_dir = scan_result
        .groups
        .iter()
        .find(|g| g.group_index == group_index && g.group_type == GroupType::NormalGroup)
        .map(|g| g.full_path.clone());
    log::debug!(
        "[core::mod_manager] [switch_mod] looking for NormalGroup by group_index={} found={}",
        group_index,
        group_dir.is_some()
    );
    sel_dbg!(
        "mod_manager",
        "switch_mod",
        "步骤=定位 NormalGroup | 分组索引={} 找到分组目录={:?}",
        group_index,
        group_dir
    );

    // 注意：group 分组下选择模组不使用互斥逻辑，不得自动禁用/启用同组其他模组。
    // 各模组的启用/禁用状态仅由用户通过开关显式抉择，选择模组只更新选中状态。

    // P3 修复：按路径解析后端排序后的真实 mod_index。
    // 前端展示序与后端 mod_order_cmp 排序序可能分叉（收藏前置/新增置末位等本地近似
    // 重排），故优先以 mod_path 定位；未传或路径未命中时回退到传入下标（行为不劣化）。
    // 槽位身份（P0，2026-09-26 最佳修复）：同一次定位同时取出 slot_id（组内目录
    // 枚举序，与 update_mod_data 深扫注入的 $managed_slot_id 同源）。组内含禁用/
    // 收藏模组时展示序 ≠ 目录序——按键模拟光标 X 必须取 slot_id，否则游戏内
    // `$managed_slot_id == $active_slot` 守卫永不命中（游戏内选择失效根因）。
    let (resolved_mod_index, resolved_slot_id) = match mod_path {
        Some(p) if !p.is_empty() => scan_result
            .mods
            .iter()
            .find(|m| m.group_index == group_index && mod_path_matches(&m.full_path, p))
            .map(|m| (Some(m.mod_index), Some(m.slot_id)))
            .unwrap_or((None, None)),
        _ => (None, None),
    };
    let effective_mod_index = resolved_mod_index.unwrap_or(mod_index);
    sel_dbg!(
        "mod_manager",
        "switch_mod",
        "步骤=按路径解析真实下标 | 传入下标={} 路径解析结果={:?} 采用下标={}",
        mod_index,
        resolved_mod_index,
        effective_mod_index
    );
    if resolved_mod_index.is_some() && resolved_mod_index != Some(mod_index) {
        log::warn!(
            "[core::mod_manager] [switch_mod] mod_index corrected by path | group={} frontend_index={} backend_index={} mod_path={:?}",
            group_index,
            mod_index,
            effective_mod_index,
            mod_path
        );
    }

    // mod_index 安全转换：扫描器使用 1-based 索引（idx + 1），$managed_slot_id 也是 1-based。
    // 若前端传入 0（旧版 0-based 兼容），转为 1 以匹配 $managed_slot_id。
    let safe_mod_index = if effective_mod_index == 0 {
        1
    } else {
        effective_mod_index
    };
    sel_dbg!(
        "mod_manager",
        "switch_mod",
        "步骤=模组索引安全转换 | 原始={} → 安全值={}（mod_index==0 时按 1 处理以匹配 $managed_slot_id）",
        effective_mod_index,
        safe_mod_index
    );

    // 将选中的 mod_index 写入该分组的 selectedindex 文件，使 is_active 状态持久化
    // 对齐 NRMM：写 selectedindex 前暂停文件监听器，写完后恢复，防止触发增量更新竞态
    let sel_idx_i32 = safe_mod_index as i32;
    if let Some(g_dir) = group_dir {
        let selectedindex_path = g_dir.join(constants::SELECTED_INDEX_FILE);
        // 暂停文件监听器（引用计数守卫），防止 selectedindex 写入触发缓存增量更新竞态
        {
            let _guard = WatcherGuard::new();
            if let Err(e) = crate::utils::atomic_write(
                &selectedindex_path,
                safe_mod_index.to_string().as_bytes(),
            ) {
                log::warn!(
                    "Failed to write selectedindex file {:?}: {}",
                    selectedindex_path,
                    e
                );
            } else {
                log::debug!(
                    "[core::mod_manager] [switch_mod] wrote selectedindex file | path={} content={}",
                    selectedindex_path.display(),
                    safe_mod_index
                );
                sel_dbg!(
                    "mod_manager",
                    "switch_mod",
                    "步骤=写入 selectedindex | 路径={} 内容={}",
                    selectedindex_path.display(),
                    safe_mod_index
                );
            }
        } // _guard drop：计数递减，仅当无外层守卫时才恢复监听
    } else {
        // P4 修复：组定位失败显式返回错误。
        // 原实现仅 log::warn 后仍返回 Ok(selected_mod_index = Some(...))，前端据此
        // 乐观更新 UI 显示已选中，而磁盘 selectedindex 实际未写 → 重载后选择丢失且无报错。
        log::error!(
            "[core::mod_manager] [switch_mod] NormalGroup not found by group_index={} (selectedindex not written)",
            group_index
        );
        return Err(anyhow!(
            "NormalGroup not found by group_index={} (selectedindex not written)",
            group_index
        ));
    }

    let result = UpdateResult {
        selected_mod_index: Some(sel_idx_i32),
        selected_slot_id: resolved_slot_id,
        ..Default::default()
    };
    log::debug!(
        "[core::mod_manager] [switch_mod] done | elapsed={:?}ms | selected={:?}",
        _s.elapsed().as_millis(),
        result.selected_mod_index
    );
    sel_dbg!(
        "mod_manager",
        "switch_mod",
        "调用链: select_mod → switch_mod → 完成(返回 select_mod 继续按键模拟) | 选中模组索引={:?} 耗时={:?}ms",
        result.selected_mod_index,
        _s.elapsed().as_millis()
    );
    Ok(result)
}

/// 按 `mod_path` 解析组内模组的后端 `mod_index`（**缓存快照**版，供直写快路径使用）
///
/// 直写快路径（[`write_selectedindex_direct`]）不做磁盘扫描，但仍必须写入**后端排序序**
/// 下的下标：前端展示序因「本地近似重排」（收藏前置 / 新增置末位）与后端
/// `mod_order_cmp` 可分叉，直接写前端传入的 `mod_index` 会把 selectedindex 落到
/// 错误模组上（即「选错模组」）。缓存快照即最近一次后端扫描结果，是权威的后端序。
///
/// # 参数
/// - `game`: 目标游戏（缓存键）
/// - `mods_path`: Mods 根目录（缓存键）
/// - `group_path`: 分组目录绝对路径（定位组内）
/// - `mod_path`: 目标模组绝对路径
///
/// # 返回
/// 命中返回后端 `mod_index`；`mod_path` 为空 / 缓存缺失 / 路径未命中均返回 `None`，
/// 调用方回退到前端传入下标（行为不劣化）。
pub fn resolve_mod_index_by_path_from_cache(
    game: TargetGame,
    mods_path: &Path,
    group_path: &Path,
    mod_path: &str,
) -> Option<u32> {
    // [selection-latency] 入口计时：与最终 debug 日志的 elapsed_ms 配对，
    // 供用户按「组内模组数量」对比线性查找段耗时（验证延迟线性增长假设）
    let t0 = std::time::Instant::now();
    if mod_path.is_empty() {
        return None;
    }
    // 非阻塞取读锁：watcher 增量合并（subtree_replace）会长期持有 MOD_CACHE 写锁
    // （实测 6542 模组下可达数秒）。此处若用阻塞 read() 会陪等同样时长，把
    // 「启用后选择延迟」从写锁段搬到读锁段 —— 与修复初衷相悖。
    // 拿不到锁即放弃纠正，回退到调用方传入的下标（行为等同修复前，不引入新延迟）。
    let cache = match crate::core::mod_cache_core::MOD_CACHE.try_read() {
        Some(c) => c,
        None => {
            log::warn!(
                "[core::mod_manager] [resolve_mod_index_by_path_from_cache] cache read lock busy, skip path correction (mod_path={:?})",
                mod_path
            );
            return None;
        }
    };
    let scan = cache.get(game, mods_path)?;
    let group = scan
        .groups
        .iter()
        .find(|g| path_eq_ignore_case(&g.full_path, group_path))?;
    let resolved = group
        .mods
        .iter()
        .find(|m| mod_path_matches(&m.full_path, mod_path))
        .map(|m| m.mod_index);
    log::debug!(
        "[core::mod_manager] [resolve_mod_index_by_path_from_cache] group={:?} mod_path={:?} resolved={:?} group_mods={} elapsed_ms={} (linear-scan probe)",
        group_path,
        mod_path,
        resolved,
        group.mods.len(),
        t0.elapsed().as_millis()
    );
    resolved
}

/// 按 `mod_path` 解析组内模组的「槽位身份」`slot_id`（**缓存快照**版，供选择快路径）
///
/// 槽位身份 = 组内目录枚举序 1-based（与 `update_mod_data` 深扫注入的
/// `$managed_slot_id` / Dart `realIndex` 同源）。按键模拟光标 X 必须取此值：
/// 展示序（`mod_order_cmp` 重排后）与目录序在组内含禁用/收藏模组时分叉，
/// 用展示序会令游戏内 `$managed_slot_id == $active_slot` 守卫落空（选择失效根因）。
///
/// # 返回
/// 命中返回 `slot_id`（>0）；`mod_path` 为空 / 缓存缺失 / 路径未命中 / 读锁忙
/// 均返回 `None`，调用方回退展示序 `mod_index`（降级，行为不劣于修复前）。
pub fn resolve_slot_id_by_path_from_cache(
    game: TargetGame,
    mods_path: &Path,
    group_path: &Path,
    mod_path: &str,
) -> Option<u32> {
    if mod_path.is_empty() {
        return None;
    }
    // 非阻塞取读锁：与 [`resolve_mod_index_by_path_from_cache`] 同口径——
    // watcher 增量合并（subtree_replace）长期持有写锁时快速放弃，不阻塞选择链路
    let cache = match crate::core::mod_cache_core::MOD_CACHE.try_read() {
        Some(c) => c,
        None => {
            log::warn!(
                "[core::mod_manager] [resolve_slot_id_by_path_from_cache] cache read lock busy, skip slot resolution (mod_path={:?})",
                mod_path
            );
            return None;
        }
    };
    let scan = cache.get(game, mods_path)?;
    let group = scan
        .groups
        .iter()
        .find(|g| path_eq_ignore_case(&g.full_path, group_path))?;
    group
        .mods
        .iter()
        .find(|m| mod_path_matches(&m.full_path, mod_path))
        .map(|m| m.slot_id)
        .filter(|&s| s > 0)
}

/// 取消选中分组内模组（NormalGroup 专用）
///
/// 对齐 NRMM `setSelectedModIndex(ref, 0, groupPath)`：向分组目录写入 `selectedindex=0`，
/// 选择 None 槽位以取消选中，而非重命名禁用模组。
///
/// # 参数
/// - `game`: 目标游戏
/// - `game_mods_path`: 游戏 Mods 目录
/// - `settings`: 应用设置
/// - `group_index`: 要取消选中的分组索引
pub fn deselect_group_mods(
    _game: TargetGame,
    game_mods_path: &Path,
    _settings: &AppSettings,
    group_index: u32,
) -> Result<UpdateResult> {
    let scan_result = crate::core::mod_scan_light::scan_mods_light(game_mods_path)?;

    // 仅 NormalGroup 写入 selectedindex，MutexGroup 不参与标记文件
    let group_dir = scan_result
        .groups
        .iter()
        .find(|g| g.group_index == group_index && g.group_type == GroupType::NormalGroup)
        .map(|g| g.full_path.clone());

    if let Some(g_dir) = group_dir {
        let selectedindex_path = g_dir.join(constants::SELECTED_INDEX_FILE);
        // 写入 0 表示选择 None 槽位（取消选择）
        // 对齐 NRMM：写 selectedindex 前暂停文件监听器（引用计数守卫），写完后恢复
        {
            let _guard = WatcherGuard::new();
            if let Err(e) = crate::utils::atomic_write(&selectedindex_path, b"0") {
                log::warn!(
                    "Failed to write selectedindex file {:?}: {}",
                    selectedindex_path,
                    e
                );
            } else {
                log::debug!(
                    "[core::mod_manager] [deselect_group_mods] wrote selectedindex=0 | path={}",
                    selectedindex_path.display()
                );
            }
        } // _guard drop
    } else {
        log::warn!(
            "[core::mod_manager] [deselect_group_mods] NormalGroup not found by group_index={}",
            group_index
        );
    }

    Ok(UpdateResult {
        selected_mod_index: Some(0),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests_write_selectedindex_direct {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// 直写成功：group_xx 目录写入指定索引；幂等重复写以新值覆盖
    #[test]
    fn writes_index_into_group_xx() {
        let dir = TempDir::new().unwrap();
        let group = dir.path().join("_MANAGED_").join("group_3");
        fs::create_dir_all(&group).unwrap();

        write_selectedindex_direct(&group, 2).unwrap();
        assert_eq!(
            fs::read_to_string(group.join(constants::SELECTED_INDEX_FILE)).unwrap(),
            "2"
        );

        // 幂等重复写：新值覆盖旧值
        write_selectedindex_direct(&group, 5).unwrap();
        assert_eq!(
            fs::read_to_string(group.join(constants::SELECTED_INDEX_FILE)).unwrap(),
            "5"
        );
    }

    /// 守卫拒绝非 group_xx 目录：互斥目录 / group_0 / 前导零 / 任意命名，
    /// 且被拒绝目录不得产生 selectedindex 文件
    #[test]
    fn rejects_non_group_xx_dirs() {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        for name in ["#Mutex", "group_0", "group_01", "Arbitrary"] {
            let p = managed.join(name);
            fs::create_dir_all(&p).unwrap();
            assert!(
                write_selectedindex_direct(&p, 1).is_err(),
                "应拒绝非 group_xx 目录: {}",
                name
            );
            assert!(
                !p.join(constants::SELECTED_INDEX_FILE).exists(),
                "被拒绝目录不得产生 selectedindex: {}",
                name
            );
        }
    }

    /// 分组目录不存在 → 原子写失败返回 Err
    #[test]
    fn errors_when_group_dir_missing() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("_MANAGED_").join("group_9");
        assert!(write_selectedindex_direct(&missing, 1).is_err());
    }
}

#[cfg(test)]
mod tests_select_switch {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// 创建带 _MANAGED_ 子目录的临时根目录
    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        dir
    }

    /// 在分组目录下创建带 INI 文件的模组目录
    fn create_mod_with_ini(group_path: &Path, mod_name: &str, ini_content: &str) -> PathBuf {
        let mod_path = group_path.join(mod_name);
        fs::create_dir_all(&mod_path).unwrap();
        let ini_path = mod_path.join("mod.ini");
        fs::write(&ini_path, ini_content).unwrap();
        mod_path
    }

    /// 在根目录创建 d3dx.ini
    fn create_d3dx_ini(base: &Path) {
        fs::write(base.join("d3dx.ini"), "; test\n").unwrap();
    }

    /// 测试：switch_mod 不会在 MutexGroup 目录下写入 selectedindex 文件
    ///
    /// 验证：传入 group_index=0（MutexGroup 默认 group_index）时，
    /// 不会在 MutexGroup 目录下创建 selectedindex 文件。
    /// P4 修复后组定位失败改为显式报错，故断言返回 `Err`（前端据此放弃乐观更新）
    /// 且磁盘未产生标记文件——两个不变式同时成立。
    #[test]
    fn test_switch_mod_no_selectedindex_in_mutex_group() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let settings = AppSettings::default();

        // 非group目录（MutexGroup 根分组 group_index 默认为 0）
        let mutex_group = dir.path().join("_MANAGED_").join("#MutexGroup");
        fs::create_dir_all(&mutex_group).unwrap();
        create_mod_with_ini(&mutex_group, "MutexMod", "[Section]\n");

        // 调用 switch_mod 传入 group_index=0（对应 MutexGroup）
        let result = switch_mod(
            TargetGame::GenshinImpact,
            dir.path(),
            &settings,
            0, // group_index=0 对应 MutexGroup
            1,
            None,
        );

        // P4：组定位失败必须显式报错，而非静默返回成功
        assert!(
            result.is_err(),
            "找不到 NormalGroup 时应返回 Err，而不是静默成功"
        );

        // 验证 MutexGroup 目录下未创建 selectedindex 文件
        assert!(
            !mutex_group.join(constants::SELECTED_INDEX_FILE).exists(),
            "非group目录不应创建 selectedindex 文件"
        );
        // 验证 MutexGroup 目录下未创建 groupname 文件
        assert!(
            !mutex_group.join("groupname").exists(),
            "非group目录不应创建 groupname 文件"
        );
    }

    /// 测试：switch_mod 对 NormalGroup 正常写入 selectedindex 文件
    ///
    /// 验证：group_xx 目录下调用 switch_mod 仍正常工作
    #[test]
    fn test_switch_mod_writes_selectedindex_for_normal_group() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let settings = AppSettings::default();

        // group_1（NormalGroup，group_index=1）
        let group1 = dir.path().join("_MANAGED_").join("group_1");
        fs::create_dir_all(&group1).unwrap();
        create_mod_with_ini(&group1, "TestMod", "[Section]\n");

        // 调用 switch_mod 传入 group_index=1（对应 NormalGroup）
        let _result = switch_mod(
            TargetGame::GenshinImpact,
            dir.path(),
            &settings,
            1, // group_index=1 对应 group_1
            1, // mod_index=1（TestMod）
            None,
        )
        .unwrap();

        // 验证 group_1 目录下已创建/更新 selectedindex 文件
        assert!(
            group1.join(constants::SELECTED_INDEX_FILE).exists(),
            "NormalGroup 应创建 selectedindex 文件"
        );
        let content = fs::read_to_string(group1.join(constants::SELECTED_INDEX_FILE)).unwrap();
        assert_eq!(content.trim(), "1");
    }

    /// 测试（P3 修复）：switch_mod 按 mod_path 解析后端排序后的真实下标
    ///
    /// 构造「组内多模组 + 收藏」场景：后端按 `mod_order_cmp`（启用优先 → 收藏优先 →
    /// 最新收藏 → 自然名）排序后的下标与前端展示下标分叉。此时传入**错误的前端下标**
    /// 但**正确的 mod_path**，断言写入的 selectedindex 为后端真实下标——
    /// 即「路径优先、下标仅作回退」的修复语义。
    #[test]
    fn test_switch_mod_resolves_index_by_mod_path() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let settings = AppSettings::default();

        // group_1 下三个模组（目录枚举序为自然名序）
        let group1 = dir.path().join("_MANAGED_").join("group_1");
        fs::create_dir_all(&group1).unwrap();
        create_mod_with_ini(&group1, "ModA", "[Section]\n");
        create_mod_with_ini(&group1, "ModB", "[Section]\n");
        create_mod_with_ini(&group1, "ModC", "[Section]\n");

        // 先扫一次拿到后端确定序（用于推导期望下标），避免把「目录枚举序」当期望值
        let scan = crate::core::mod_scan_light::scan_mods_light(dir.path()).unwrap();
        let backend_mods: Vec<_> = scan
            .mods
            .iter()
            .filter(|m| m.group_index == 1)
            .map(|m| (m.full_path.to_string(), m.mod_index))
            .collect();
        assert!(
            backend_mods.len() == 3,
            "group_1 应有 3 个真实模组，实际 {}",
            backend_mods.len()
        );

        // 取「非首位」的模组作为目标：传入故意错误的前端下标 1，
        // 期望仍按路径解析到后端真实下标（≠1 才说明路径解析真正生效）
        let (target_path, expected_index) = backend_mods
            .iter()
            .find(|(_, idx)| *idx != 1)
            .map(|(p, idx)| (p.clone(), *idx))
            .expect("应存在 mod_index != 1 的模组");

        let _result = switch_mod(
            TargetGame::GenshinImpact,
            dir.path(),
            &settings,
            1,
            1, // 故意传入与真实下标不符的前端下标
            Some(target_path.as_str()),
        )
        .unwrap();

        let content = fs::read_to_string(group1.join(constants::SELECTED_INDEX_FILE)).unwrap();
        assert_eq!(
            content.trim(),
            expected_index.to_string(),
            "selectedindex 应等于按 mod_path 解析出的后端真实下标"
        );
    }

    /// 测试（P3 修复回退路径）：mod_path 未命中时回退到传入下标，行为不劣化
    #[test]
    fn test_switch_mod_falls_back_to_index_when_path_unmatched() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());
        let settings = AppSettings::default();

        let group1 = dir.path().join("_MANAGED_").join("group_1");
        fs::create_dir_all(&group1).unwrap();
        create_mod_with_ini(&group1, "ModA", "[Section]\n");

        // 传入一个组内不存在的路径 → 路径解析未命中 → 回退到传入下标 1
        let result = switch_mod(
            TargetGame::GenshinImpact,
            dir.path(),
            &settings,
            1,
            1,
            Some(group1.join("NotExist").to_string_lossy().as_ref()),
        )
        .unwrap();

        let content = fs::read_to_string(group1.join(constants::SELECTED_INDEX_FILE)).unwrap();
        assert_eq!(content.trim(), "1", "路径未命中时应回退到传入的 mod_index");
        // 路径未命中 → 槽位身份同样未解析，调用方回退展示序
        assert_eq!(result.selected_slot_id, None);
    }

    /// 测试（P0 最佳修复，2026-09-26）：switch_mod 按 mod_path 同时返回
    /// 展示序下标与目录序槽位身份
    ///
    /// 构造「收藏前置」重排：ModA（启用、无收藏）、ModB（启用、收藏）→ 展示序
    /// B 先 A 后。对扫描产物里的每个模组都断言：
    /// - `selected_mod_index` = 展示序（与磁盘 selectedindex 同值，UI 语义不变）；
    /// - `selected_slot_id` = 扫描产物中的目录序槽位（与 `$managed_slot_id` 同源，
    ///   按键模拟光标 X 取此值）。
    #[test]
    fn test_switch_mod_returns_slot_id_by_path() {
        let dir = setup_test_dir();
        create_d3dx_ini(dir.path());

        let group1 = dir.path().join("_MANAGED_").join("group_1");
        fs::create_dir_all(&group1).unwrap();
        let mod_a = create_mod_with_ini(&group1, "ModA", "[Section]\n");
        let mod_b = create_mod_with_ini(&group1, "ModB", "[Section]\n");
        // ModB 收藏 → 展示序 B 在前（mod_index 更小），展示序与目录序分叉
        fs::write(mod_b.join(constants::FAV_MARKER), "").unwrap();
        let _ = mod_a;

        // 先扫一次拿到后端产物（目录序 slot_id 与展示序 mod_index 同源可比）
        let scan = crate::core::mod_scan_light::scan_mods_light(dir.path()).unwrap();
        assert!(
            !scan.mods.is_empty(),
            "夹具应至少扫出一个模组，否则本用例失去意义"
        );

        // 逐模组校验「按路径切换」契约：路径命中时展示序下标与目录序槽位身份都取自
        // 路径所指模组，不受故意传入的错误前端下标影响。
        //
        // 写成全覆盖循环而非「先 find 一个 slot_id != mod_index 的分叉模组」：分叉是否
        // 出现取决于宿主的目录枚举序与排序实现（Windows 收藏重排必产生分叉，类 Unix
        // 宿主可能恰好同序），而契约本身与宿主无关——分叉存在时照样被覆盖。
        for m in &scan.mods {
            let target_path: &str = &m.full_path;
            let bogus_index = if m.mod_index == 1 { 2 } else { 1 };
            let result = switch_mod(
                TargetGame::GenshinImpact,
                dir.path(),
                &AppSettings::default(),
                m.group_index,
                bogus_index, // 故意传入与真实下标不符的前端下标
                Some(target_path),
            )
            .unwrap_or_else(|e| panic!("按路径切换 {target_path} 应成功: {e}"));

            assert_eq!(
                result.selected_mod_index,
                Some(m.mod_index as i32),
                "selected_mod_index 应为展示序（与磁盘 selectedindex 同值）：{target_path}"
            );
            assert_eq!(
                result.selected_slot_id,
                Some(m.slot_id),
                "selected_slot_id 应为目录序槽位身份（与 $managed_slot_id 同源）：{target_path}"
            );
        }
    }

    /// 测试：deselect_group_mods 写入 selectedindex=0 且不重命名禁用模组
    ///
    /// 验证：对 NormalGroup 调用 deselect_group_mods 应写入 "0"（选择 None 槽位），
    /// 且不应通过 DISABLED 前缀重命名模组目录。
    #[test]
    fn test_deselect_group_mods_writes_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let mods_path = tmp.path();
        let managed = mods_path.join("_MANAGED_");
        let group_dir = managed.join("group_1");
        fs::create_dir_all(&group_dir).unwrap();
        let mod_dir = group_dir.join("TestMod");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(mod_dir.join("test.ini"), "[Section]\nkey=val\n").unwrap();

        let settings = AppSettings::default();
        let result =
            deselect_group_mods(TargetGame::GenshinImpact, mods_path, &settings, 1).unwrap();

        assert_eq!(result.selected_mod_index, Some(0));
        let sel = fs::read_to_string(group_dir.join("selectedindex")).unwrap();
        assert_eq!(sel, "0");
        assert!(mod_dir.exists(), "mod directory should not be renamed");
        assert!(!group_dir.join("DISABLEDTestMod").exists());
    }
}
