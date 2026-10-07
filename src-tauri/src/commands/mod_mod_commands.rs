//! 模组启停 / 恢复 / 操作域命令
//!
//! 从 mod_commands.rs 拆出（2026-09-19 Phase 6-2）。
//! 承载单模组启停、收藏切换、目录打开、INI 还原、自定义保存与批量启停。
//! 命令契约（ToggleModResult）经 `mod_commands`，共享辅助经
//! `use crate::commands::mod_commands_util::{...}` 引用。

use crate::commands::mod_commands::ToggleModResult;
use crate::commands::mod_commands_util::{
    build_group_delta, derive_toggled_path, open_path_in_explorer, parse_game,
    resolve_managed_context, UPDATE_LOCK,
};
use crate::config::settings_store;
use crate::core::constants;
use crate::core::mod_cache_core;
use crate::ipc::typed_paths::ManagedPath;
use crate::models::enums::TargetGame;
use crate::models::mod_data::{BatchGroupDelta, GroupDelta};
use std::fs;
use std::path::PathBuf;

/// 错误前缀标识：路径不存在，前端捕获后需执行清除缓存+重读模组
const ERR_PREFIX_PATH_NOT_FOUND: &str = "[PATH_NOT_FOUND]";

/// 切换单模组启用/禁用状态
///
/// 零扫描（对齐 NRMM `completeDisableMod`/`enableMod`）：仅目录重命名（`DISABLED_`
/// 前缀增删）+ MOD_CACHE 条目就地 patch，不触发任何扫描/重载。
/// 异常（缓存条目未命中 / 重命名失败）→ 降级组级增量重扫，`degraded=true` +
/// `group_delta`（前端 patch 后告知用户发生异常）。
#[tauri::command]
pub async fn toggle_mod_disabled(
    mod_path: ManagedPath,
    enable: bool,
    is_mutex: bool,
) -> Result<ToggleModResult, String> {
    // P2：启停通过 DISABLED 前缀重命名改写 _MANAGED_ 目录树。此前未持锁，
    // 与 select_mod（写 selectedindex + 清缓存）可并发交错，是「选择 vs 启用状态」
    // 竞态的核心缺口——统一在此入口串行化。
    let _update_guard = UPDATE_LOCK.lock().await;
    // 组目录原用于降级路径的组级重扫；P1 起降级不再同步重扫（见下），故不再需要
    let path = mod_path.into_canonical();
    // 归属校验由 ManagedPath 反序列化完成；此处只需定位 mods_root 上下文（降级/缓存 patch 用）
    let (mods_root, _old_group_dir) = resolve_managed_context(&path).ok_or_else(|| {
        format!("[NOT_IN_MANAGED_GROUP] Path is not inside a managed group: {path:?}")
    })?;
    let game = settings_store::get_settings().target_game;

    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<ToggleModResult, String> {
            // 抑制 watcher 批次在目录重命名中途读取中间态（C11：暂停期事件保留，恢复后幂等合并）
            let _guard = crate::core::watcher_ring::WatcherGuard::new();
            if is_mutex {
                if enable {
                    crate::core::mod_toggle::enable_mutex_mod(&path).map_err(|e| e.to_string())?;
                } else {
                    crate::core::mod_toggle::disable_mutex_mod(&path).map_err(|e| e.to_string())?;
                }
            } else {
                crate::core::mod_toggle::toggle_mod(&path, enable).map_err(|e| e.to_string())?;
            }
            // 推导操作后路径（与 toggle_mod 同口径；幂等时返回原路径）
            let new_path = derive_toggled_path(&path, enable);
            // 操作后的禁用态：enable=true（目标启用）→ 新禁用态 false；enable=false（目标禁用）→ true。
            // 注意不可直接传 `enable`（它是"目标启用"语义；曾以反相交入缓存导致启用后缓存态为禁用、
            // 禁用后为启用的三方一致性污染）。
            let target_disabled = !enable;
            // 零扫描缓存 patch；未命中或大树被外部持有（COW 保护）→ 标记该游戏缓存失效，
            // **不同步重扫**：交互路径不得因扫描阻塞（原同步组级重扫在连续操作时叠加为
            // 秒级卡顿 + 命令排队 + 通知齐发）。磁盘操作已成功、前端乐观更新已即时呈现，
            // 缓存由 watcher 增量 / 下次 get_mods 全量兜底修正。
            {
                let mut cache = crate::core::mod_cache_core::MOD_CACHE.write();
                if !cache.patch_mod_entry(game, &mods_root, &path, &new_path, target_disabled) {
                    cache.invalidate_game(game);
                }
            } // 释放写锁
            Ok(ToggleModResult {
                new_path: new_path.to_string_lossy().to_string(),
                disabled: target_disabled,
                degraded: false,
                group_delta: GroupDelta::default(),
            })
        })
        .await
        .map_err(|e| e.to_string())??;

    Ok(result)
}

/// 切换模组收藏状态
///
/// 设计说明（R1 命令级增量）：fav 是模组目录内的空标记文件（创建/删除），
/// 不产生目录重命名或路径变化——**不做局部重扫也不返回组 delta**：
/// - 前端按返回的 `bool` 本地翻转 `isFavorite`（单条目状态，无级联）；
/// - 缓存由 fav 文件的 watcher 事件增量修正（事件收敛到 mod 目录 →
///   `scan_partial_path` 整组局部重扫，`subtree_replace` 幂等合并）。
#[tauri::command]
pub async fn toggle_favorite(mod_path: ManagedPath) -> Result<bool, String> {
    let path = mod_path.into_canonical();

    let is_fav = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
        let fav_path = path.join(constants::FAV_MARKER);
        if fav_path.exists() {
            fs::remove_file(&fav_path).map_err(|e| e.to_string())?;
            Ok(false)
        } else {
            crate::utils::atomic_write(&fav_path, b"").map_err(|e| e.to_string())?;
            Ok(true)
        }
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(is_fav)
}

#[tauri::command]
pub fn is_favorite(mod_path: String) -> bool {
    // 仍返回布尔（不改变前端契约），但先用 ManagedPath 收口径：
    // 越出已登记 Mods 根 / 不存在 / 相对路径一律 false，
    // 使本命令不再充当「本机任意路径是否存在」的探测 oracle
    ManagedPath::parse(mod_path)
        .map(|p| p.canonical().join(constants::FAV_MARKER).exists())
        .unwrap_or(false)
}

#[tauri::command]
pub fn open_mod_folder(mod_path: ManagedPath) -> Result<(), String> {
    // 原实现零校验地把前端字符串交给 ShellExecute（opener），指向可执行文件时等价于
    // 双击运行；ManagedPath 保证存在且落在已登记 Mods 根内
    let path = mod_path.into_canonical();
    if !path.exists() {
        // 路径不存在 → 清除后端模组缓存，返回带标识的错误通知前端刷新数据
        let mut cache = mod_cache_core::MOD_CACHE.write();
        cache.invalidate_all();
        drop(cache);
        log::warn!("Open mod folder: path not found, invalidated cache and requesting frontend refresh: {:?}", path);
        return Err(format!(
            "{}{}",
            ERR_PREFIX_PATH_NOT_FOUND, "Path does not exist"
        ));
    }
    open_path_in_explorer(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_group_folder(group_path: ManagedPath) -> Result<(), String> {
    let path = group_path.into_canonical();
    if !path.exists() {
        // 路径不存在 → 清除后端模组缓存，返回带标识的错误通知前端刷新数据
        let mut cache = mod_cache_core::MOD_CACHE.write();
        cache.invalidate_all();
        drop(cache);
        log::warn!("Open group folder: path not found, invalidated cache and requesting frontend refresh: {:?}", path);
        return Err(format!(
            "{}{}",
            ERR_PREFIX_PATH_NOT_FOUND, "Path does not exist"
        ));
    }
    open_path_in_explorer(&path).map_err(|e| e.to_string())
}

/// 恢复所有INI文件备份
#[tauri::command]
pub async fn restore_all_inis(
    mods_path: ManagedPath,
) -> Result<crate::core::mod_results::RestoredCount, String> {
    let _busy = crate::window::lifecycle::BusyOpGuard::new();
    let path = mods_path.into_canonical();

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::core::mod_results::RestoredCount, String> {
            crate::core::mod_restore::restore_all_inis(&path).map_err(|e| e.to_string())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache_core::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(result)
}

/// 还原模组 INI 到管理前状态（还原区功能）
///
/// 对用户拖拽/选择的目录递归清理 NRMM 管理器注入的专属行，
/// 使模组无需模组管理器即可在 3Dmigoto 中使用。
///
/// # 参数
/// - `path`: 要还原的模组文件夹路径
///
/// # 返回值
/// 返回 `RestoreManagedResult`，包含路径、INI 处理数、失败数与成功标志。
///
/// # 错误
/// - 路径不存在（带 `[PATH_NOT_FOUND]` 前缀，供前端识别）
#[tauri::command]
pub async fn restore_managed_folder(
    path: ManagedPath,
) -> Result<crate::core::mod_results::RestoreManagedResult, String> {
    // ManagedPath 已保证存在且落在已登记 Mods 根内，原 exists() 分支不可达
    let path_buf = path.into_canonical();

    let result = tauri::async_runtime::spawn_blocking(
        move || -> crate::core::mod_results::RestoreManagedResult {
            let (ini_count, failed_count) =
                crate::core::mod_restore::restore_managed_mod(&path_buf);
            crate::core::mod_results::RestoreManagedResult {
                path: path_buf,
                ini_count,
                failed_count,
                success: failed_count == 0,
            }
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    Ok(result)
}

/// Save Mod Customizations：保存用户自定义INI设置到d3dx_user.ini
#[tauri::command]
pub async fn save_customizations(
    game: String,
    mods_path: ManagedPath,
) -> Result<crate::core::mod_results::SaveCustomizationsResult, String> {
    let _busy = crate::window::lifecycle::BusyOpGuard::new();
    let game = parse_game(&game)?;
    let path = mods_path.into_canonical();

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::core::mod_results::SaveCustomizationsResult, String> {
            crate::core::mod_ini_admin::save_customizations(&path, game).map_err(|e| e.to_string())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    Ok(result)
}

/// 批量切换模组启用/禁用状态
#[tauri::command]
pub async fn batch_toggle_mods(
    mod_paths: Vec<ManagedPath>,
    enable: bool,
    is_mutex: bool,
) -> Result<u32, String> {
    // P2：批量启停会重命名 _MANAGED_ 下多个模组目录，须与选择、更新串行。
    let _update_guard = UPDATE_LOCK.lock().await;
    // 逐元素校验：任一越界即整体拒绝，core 层仍收字符串（其签名不在本批变更范围）
    let mod_paths: Vec<String> = mod_paths
        .iter()
        .map(|p| p.canonical().to_string_lossy().into_owned())
        .collect();
    let count = tauri::async_runtime::spawn_blocking(move || -> Result<u32, String> {
        crate::core::mod_toggle::batch_toggle_mods(&mod_paths, enable, is_mutex)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache_core::MOD_CACHE.write();
        cache.invalidate_all();
    }

    Ok(count)
}

/// 禁用指定分组下所有一级模组（不含 .ini 的子分组目录不处理）
///
/// 零扫描（对齐 NRMM）：目录重命名后对缓存逐条 `patch_mod_entry` 就地更新，
/// 不触发任何扫描/重载，返回路径对列表供前端本地 patch。
/// 异常（rename 失败 / 缓存条目未命中）→ 降级增量扫描，`degraded=true` + 完整分组增量。
#[tauri::command]
pub async fn disable_all_mods_in_group(group_path: ManagedPath) -> Result<BatchGroupDelta, String> {
    // 整组启停会重命名组内全部一级模组，此前该入口无任何路径校验
    let path = group_path.into_canonical();
    let (mods_root, group_dir) = resolve_managed_context(&path).ok_or_else(|| {
        format!("[NOT_IN_MANAGED_GROUP] Path is not inside a managed group: {path:?}")
    })?;
    let game = settings_store::get_settings().target_game;

    batch_toggle_group_impl(game, mods_root, group_dir, path, true).await
}

/// 启用指定分组下所有一级禁用模组（不含 .ini 的子分组目录不处理）
///
/// 零扫描（对齐 NRMM）：目录重命名后对缓存逐条 `patch_mod_entry` 就地更新。
/// 异常 → 降级增量扫描，`degraded=true` + 完整分组增量。
#[tauri::command]
pub async fn enable_all_mods_in_group(group_path: ManagedPath) -> Result<BatchGroupDelta, String> {
    // 整组启停会重命名组内全部一级模组，此前该入口无任何路径校验
    let path = group_path.into_canonical();
    let (mods_root, group_dir) = resolve_managed_context(&path).ok_or_else(|| {
        format!("[NOT_IN_MANAGED_GROUP] Path is not inside a managed group: {path:?}")
    })?;
    let game = settings_store::get_settings().target_game;

    batch_toggle_group_impl(game, mods_root, group_dir, path, false).await
}

/// 批量启停实现（禁用/启用共用）：先目录重命名（返回路径对），再零扫描 patch 缓存；
/// 任一条缓存 patch 失败 → 降级组级增量重扫（degraded=true）。
async fn batch_toggle_group_impl(
    game: TargetGame,
    mods_root: PathBuf,
    group_dir: PathBuf,
    group_path: PathBuf,
    disable: bool,
) -> Result<BatchGroupDelta, String> {
    // P2：批量禁用/启用整组会重命名组内全部一级模组目录，属 _MANAGED_ 写路径。
    // 锁加在此共用实现上（而非两个外层命令），既覆盖两条入口又避免重复加锁。
    let _update_guard = UPDATE_LOCK.lock().await;
    let dir_display = group_path.display().to_string();
    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<BatchGroupDelta, String> {
            // 抑制 watcher 批次在批量重命名中途读取中间态（C11：暂停期事件保留，恢复后幂等合并）
            let _guard = crate::core::watcher_ring::WatcherGuard::new();
            let renamed = if disable {
                crate::core::mod_toggle::disable_all_mods_in_group(&group_path)
                    .map_err(|e| e.to_string())?
            } else {
                crate::core::mod_toggle::enable_all_mods_in_group(&group_path)
                    .map_err(|e| e.to_string())?
            };
            let changed = renamed.len() as u32;

            // 零扫描缓存 patch：逐条就地更新（组树 + 扁平列表）；任一条未命中 → 降级
            let mut degraded = false;
            {
                let mut cache = crate::core::mod_cache_core::MOD_CACHE.write();
                for (old, new) in &renamed {
                    if !cache.patch_mod_entry(game, &mods_root, old, new, disable) {
                        degraded = true;
                        break;
                    }
                }
            } // 释放写锁（build_group_delta 内部需获取读/写锁）

            let delta = if degraded {
                build_group_delta(game, &mods_root, &group_dir, &group_dir)
                    .map_err(|e| e.to_string())?
            } else {
                // 正常路径不消费 delta（toggled 路径对足够），返回空增量占位
                GroupDelta::default()
            };

            let toggled = renamed
                .into_iter()
                .map(|(old, new)| crate::models::mod_data::PathPair {
                    old_path: old.to_string_lossy().to_string(),
                    new_path: new.to_string_lossy().to_string(),
                })
                .collect();
            Ok(BatchGroupDelta {
                changed,
                toggled,
                degraded,
                delta,
            })
        })
        .await
        .map_err(|e| e.to_string())??;
    log::info!(
        "[{}] changed={} degraded={} dir={}",
        if disable {
            "disable_all_mods_in_group"
        } else {
            "enable_all_mods_in_group"
        },
        result.changed,
        result.degraded,
        dir_display
    );
    Ok(result)
}
