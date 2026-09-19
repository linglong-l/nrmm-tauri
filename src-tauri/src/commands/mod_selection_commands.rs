//! 模组选择域命令
//!
//! 从 mod_commands.rs 拆出（2026-09-19 Phase 6-2）。
//! 承载 select_mod / deselect_group_mod 两个选择类命令（含按键模拟），
//! 因体量较大独立成文件以满足 F4 新文件 ≤800 行棘轮。
//! 命令契约（SelectModArgs）经 `mod_commands`，共享辅助经
//! `use crate::commands::mod_commands_util::{...}` 引用。

use crate::commands::mod_commands::SelectModArgs;
use crate::commands::mod_commands_util::{
    is_direct_writable_group, parse_game, selection_debounce_hit, UPDATE_LOCK,
};
use crate::config::settings_store;
use crate::core::constants;
use crate::core::mod_cache_core;
use crate::sel_dbg;
use std::path::{Path, PathBuf};

/// 选择模组（支持互斥组）
#[tauri::command]
pub async fn select_mod(
    args: SelectModArgs,
) -> Result<crate::core::mod_results::UpdateResult, String> {
    // 「_MANAGED_ 写操作全局锁」（C12）：select/switch 与 update/导入互斥，
    // 防止 update 批量写入期间全量扫描读到中间态，并交错写 selectedindex。
    // 命令体入口即获取，跨下方两个分支的所有 spawn_blocking + .await 持锁。
    let _update_guard = UPDATE_LOCK.lock().await;
    let SelectModArgs {
        game,
        mods_path,
        group_index,
        mod_index,
        is_mutex,
        group_path,
        mod_path,
        cursor_x,
        cursor_y,
    } = args;
    // mod_index 归一化（与 switch_mod 的 safe_mod_index 语义保持一致）：
    // 扫描器约定 None 空槽位占据 mod_index=0，真实模组从 1 开始（$managed_slot_id 亦为 1-based）。
    // 若前端因任何路径误传 0（如收藏模式显示列表下标错位），统一转为 1，
    // 否则会出现「持久化 selectedindex=1 而按键模拟仍用 x=0」的分歧——
    // 分组 Y 坐标正确、模组 X 坐标($active_slot)为 0，导致无模组匹配而失效。
    sel_dbg!(
        "mod_commands",
        "select_mod",
        "mod_index 归一化前 | mod_index_before_normalize={}",
        mod_index
    );
    let mod_index = if mod_index == 0 { 1 } else { mod_index };
    sel_dbg!(
        "mod_commands",
        "select_mod",
        "mod_index 归一化后 | mod_index_after_normalize={}",
        mod_index
    );
    // 提取模组名称与分组名称，便于在调试日志中直接定位所属模组
    let mod_name = Path::new(&mod_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let group_name = Path::new(&group_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    sel_dbg!(
        "mod_commands",
        "select_mod",
        "入口 | 所属模组名称={} 所属分组名称={} game={} 分组索引={} 模组索引={} 是否互斥组={} 传入光标坐标=({:?},{:?}) 模组路径={} 分组路径={}",
        mod_name, group_name, game, group_index, mod_index, is_mutex, cursor_x, cursor_y, mod_path, group_path
    );
    log::debug!(
        "[commands::mod_commands] [select_mod] game={} group={} mod={} mutex={}",
        game,
        group_index,
        mod_index,
        is_mutex
    );
    let _start = std::time::Instant::now();
    let game_enum = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    sel_dbg!(
        "mod_commands",
        "select_mod",
        "分支判定 | is_mutex={} → 进入 {} 分支",
        is_mutex,
        if is_mutex {
            "互斥组(enable_mutex_mod，无按键模拟)"
        } else {
            "普通组(switch_mod 持久化 + 按键模拟)"
        }
    );
    if is_mutex {
        // Mutex 分支防抖（与非 Mutex 分支一致，防止快速重复点击触发多次互斥切换）
        // P1：复合键（分组 + 模组），同组不同模组的连续选择不再被误判为重复触发
        if selection_debounce_hit(&group_path, &mod_path)? {
            return Err("debounced".to_string());
        }

        let mod_path_buf = PathBuf::from(mod_path.clone());
        let managed_path = mods_path.join(constants::MANAGED_FOLDER);
        // 守卫：mod_path 必须位于 _MANAGED_ 下且第一段非 group_xx（互斥模组），
        // 否则禁止调用 enable_mutex_mod，避免作用于错误父目录成批禁用其下含 ini 的目录。
        if !crate::core::mod_toggle::is_mutex_mod(&mod_path_buf, &managed_path) {
            return Err(format!(
                "Invalid mutex mod path (not under _MANAGED_ or is a group_xx): {:?}",
                mod_path_buf
            ));
        }
        sel_dbg!(
            "mod_commands",
            "select_mod",
            "互斥组分支 | 准备调用 enable_mutex_mod | 模组名称={} 模组路径={}",
            mod_name,
            mod_path
        );

        tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            crate::core::mod_toggle::enable_mutex_mod(&mod_path_buf).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;

        {
            let mut cache = crate::core::mod_cache_core::MOD_CACHE.write();
            cache.invalidate_game(game_enum); // 彻底失效该游戏缓存（invalidate_by_prefix 是 no-op 陷阱）
        }

        Ok(crate::core::mod_results::UpdateResult {
            need_reload_manual: false,
            ..Default::default()
        })
    } else {
        // === NormalGroup 分支 ===
        // 执行顺序对齐用户要求："先持久化再按键模拟"
        // 1. 防抖检查
        // P1：复合键（分组 + 模组），同组不同模组的连续选择不再被误判为重复触发
        if selection_debounce_hit(&group_path, &mod_path)? {
            return Err("debounced".to_string());
        }

        // 2. 先持久化：优先直写快路径（O(1) 原子写 selectedindex + 缓存定向 patch，
        //    免 switch_mod 全量轻扫），group_path 无效时回退全量路径（防御）
        let settings = settings_store::get_settings();
        let simulate_enabled = settings.simulate_key_on_selection;
        let managed_path = mods_path.join(constants::MANAGED_FOLDER);
        // TargetGame 是 Copy，提前复制用于后续按键模拟
        let game_enum_for_sim = game_enum;

        let group_path_buf = PathBuf::from(&group_path);
        // P3：直写快路径同样必须按 mod_path 解析后端排序后的真实下标。
        // 前端展示序因「收藏前置 / 新增置末位」等本地近似重排与后端 mod_order_cmp
        // 分叉，直接写前端传入的 mod_index 会把 selectedindex 落到错误模组上。
        // 缓存快照即最近一次后端扫描结果（权威后端序，无磁盘 I/O，保持 O(1) 意图）；
        // 缓存缺失 / 路径未命中时回退到传入下标（行为不劣化）。
        let resolved_index = crate::core::mod_select::resolve_mod_index_by_path_from_cache(
            game_enum,
            &mods_path,
            &group_path_buf,
            &mod_path,
        );
        let direct_index = resolved_index.unwrap_or(mod_index);
        if let Some(r) = resolved_index {
            if r != mod_index {
                log::warn!(
                    "[select_mod] direct path: mod_index corrected by mod_path | frontend={} backend={} mod_path={}",
                    mod_index,
                    r,
                    mod_path
                );
            }
        }
        let result = if is_direct_writable_group(&group_path_buf, &managed_path) {
            sel_dbg!(
                "mod_commands",
                "select_mod",
                "普通组直写快路径 | group_path={} mod_index={}（免全量轻扫）",
                group_path,
                direct_index
            );
            // O(1) 原子写 selectedindex（内部含 group_xx 守卫与 WatcherGuard）
            let write_path = group_path_buf.clone();
            let write_start = std::time::Instant::now();
            tauri::async_runtime::spawn_blocking(move || {
                crate::core::mod_select::write_selectedindex_direct(&write_path, direct_index)
            })
            .await
            .map_err(|e| {
                log::error!("[select_mod] spawn_blocking join error: {}", e);
                format!("switch task failed: {}", e)
            })?
            .map_err(|e| {
                log::error!("[select_mod] write_selectedindex_direct error: {}", e);
                e.to_string()
            })?;
            log::debug!(
                "[select_mod] direct write_selectedindex done | elapsed_ms={}",
                write_start.elapsed().as_millis()
            );
            // 缓存定向 patch：命中则免整缓存失效；未命中不失效（watcher / 下次全量扫描对账）
            // 注意须用按路径解析后的 direct_index（与磁盘写入值同源），否则缓存与磁盘不一致。
            //
            // 改为 try_write（非阻塞）：实测启用模组的目录重命名必然触发 watcher 增量合并，
            // 其 `subtree_replace` 写锁段在 6542 模组/109 分组下长达数秒；此处若用阻塞
            // write() 会排在它后面空等（实测 6.7s），连带把后续按键模拟（游戏内真正生效的
            // 动作）一起推迟，用户感知为「启用后选择延迟 ~7 秒」。
            // 拿不到锁即跳过本次 patch —— 降级语义是现成的：磁盘 selectedindex 已由
            // write_selectedindex_direct 写成功，缓存对账由 watcher 增量 / 下次全量扫描完成。
            // MOD_CACHE 为 parking_lot::RwLock：try_write() 返回 Option（持锁时为 None）
            let patched = match mod_cache_core::MOD_CACHE.try_write() {
                Some(mut cache) => {
                    cache.patch_selected_index(game_enum, &mods_path, &group_path_buf, direct_index)
                }
                None => {
                    log::warn!(
                        "[select_mod] cache patch skipped: MOD_CACHE write lock busy (watcher merge in progress); \
                         selectedindex already persisted, cache reconciled by watcher/next full scan"
                    );
                    false
                }
            };
            log::debug!(
                "[commands::mod_commands] [select_mod] direct write done | patched={} selected={}",
                patched,
                direct_index
            );
            crate::core::mod_results::UpdateResult {
                selected_mod_index: Some(direct_index as i32),
                ..Default::default()
            }
        } else {
            sel_dbg!(
                "mod_commands",
                "select_mod",
                "普通组回退全量路径 | group_path={} 无效，走 switch_mod 全量轻扫",
                group_path
            );
            // P3：把前端 mod_path 透传给 switch_mod，使其按路径解析后端排序后的真实
            // mod_index（前端近似重排后的整数下标不可信），路径未命中时后端自动回退。
            let mod_path_for_switch = mod_path.clone();
            let result = tauri::async_runtime::spawn_blocking(
                move || -> Result<crate::core::mod_results::UpdateResult, String> {
                    crate::core::mod_select::switch_mod(
                        game_enum,
                        &mods_path,
                        &settings,
                        group_index,
                        mod_index,
                        Some(mod_path_for_switch.as_str()),
                    )
                    .map_err(|e| e.to_string())
                },
            )
            .await
            .map_err(|e| {
                log::error!("[select_mod] spawn_blocking join error: {}", e);
                format!("switch task failed: {}", e)
            })?
            .map_err(|e| {
                log::error!("[select_mod] switch_mod error: {}", e);
                e
            })?;

            // 整缓存失效（旧行为，确保下次 get_mods 全量扫描）
            {
                let mut cache = mod_cache_core::MOD_CACHE.write();
                cache.invalidate_game(game_enum); // 彻底失效该游戏缓存（invalidate_by_prefix 是 no-op 陷阱）
            }
            log::debug!(
                "[commands::mod_commands] [select_mod] cache invalidated | game={}",
                game.as_str()
            );
            result
        };
        log::debug!(
            "[commands::mod_commands] [select_mod] switch_result={:?}",
            result
        );

        // 4. 再按键模拟（确保文件状态已就绪）
        // 对齐 NRMM：选择后不发送 F10，$active_slot 通过 persist 跨重载保持，
        // 3Dmigoto 会在下次自然重载时重新求值 if/endif 条件
        log::debug!(
            "[commands::mod_commands] [select_mod] simulate_key_on_selection={} (after persist)",
            simulate_enabled
        );
        // 选择逻辑(按键模拟)触发前：记录当前光标位置（屏幕坐标），用于对比模拟前后变化
        let cursor_before = crate::platform::get_foreground_detector()
            .get_cursor_position()
            .ok();
        sel_dbg!(
            "mod_commands",
            "select_mod",
            "选择逻辑(按键模拟)触发前光标位置={:?}（屏幕坐标，simulate_key_on_selection={}）",
            cursor_before,
            simulate_enabled
        );
        if simulate_enabled {
            let mut simulator = crate::platform::get_key_simulator();
            let process_names = game_enum_for_sim.process_names();
            for pn in process_names {
                if simulator.set_target_process(pn).is_ok() {
                    break;
                }
            }
            // 参数语义对齐NRMM：优先使用调用方传入的屏幕坐标（cursor_x/cursor_y 为实际像素时），
            // 否则 fallback 到 (mod_index, group_index) 作为虚拟坐标（3Dmigoto/xxmi 据此识别）。
            // 注意：NRMM 将 x=realModIndex y=realGroupIndex 直接传入 SetCursorPos，
            // 因此当未传像素坐标时，索引值本身即作为坐标输入。
            // P3 合并修正：x 坐标必须与磁盘写入的 selectedindex **同源**——
            // 取本命令两条路径（直写快路径 / switch_mod 全量路径）返回的权威
            // selected_mod_index（均已按 mod_path 校正），否则会出现「持久化 X 而
            // 按键模拟用旧 x」的分歧：分组 Y 坐标正确、模组 X 坐标无模组匹配而失效。
            let sim_mod_index = result
                .selected_mod_index
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(mod_index);
            let (sim_g, sim_m) = match (cursor_x, cursor_y) {
                (Some(px), Some(py)) => {
                    let g = u32::try_from(py.max(0)).unwrap_or(group_index);
                    let m = u32::try_from(px.max(0)).unwrap_or(sim_mod_index);
                    (g, m)
                }
                _ => (group_index, sim_mod_index),
            };
            sel_dbg!(
                "mod_commands",
                "select_mod",
                "坐标传递 | cursor_x_input={:?} cursor_y_input={:?} sim_g={} sim_m={} coordinate_mode={} 模组名称={}",
                cursor_x,
                cursor_y,
                sim_g,
                sim_m,
                if cursor_x.is_some() && cursor_y.is_some() { "屏幕像素坐标" } else { "索引虚拟坐标" },
                mod_name
            );
            let sim_start = std::time::Instant::now();
            let result: Result<(), String> = simulator
                .simulate_select_full(sim_g, sim_m)
                .map_err(|e| e.to_string());
            let sim_elapsed = sim_start.elapsed();
            sel_dbg!(
                "mod_commands",
                "select_mod",
                "simulate_select_full 调用完成 | sim_g={} sim_m={} 耗时={}μs result={:?}",
                sim_g,
                sim_m,
                sim_elapsed.as_micros(),
                result
            );
            log::debug!(
                "[commands::mod_commands] [select_mod] simulate_result={:?}",
                result
            );
            if let Err(e) = result {
                log::warn!(
                    "select_mod: simulate_select_full failed (g={}, m={}): {}",
                    group_index,
                    mod_index,
                    e
                );
            }
        }

        // 选择逻辑(按键模拟)触发后：再次记录光标位置，与触发前对比，验证光标是否被正确还原
        let cursor_after = crate::platform::get_foreground_detector()
            .get_cursor_position()
            .ok();
        sel_dbg!(
            "mod_commands",
            "select_mod",
            "选择逻辑(按键模拟)触发后光标位置={:?}（屏幕坐标）| 触发前={:?} 是否一致={}",
            cursor_after,
            cursor_before,
            cursor_before == cursor_after
        );

        log::debug!("[commands::mod_commands] [select_mod] completed | elapsed={:?}ms | selected_mod_index={:?}", _start.elapsed().as_millis(), result.selected_mod_index);
        Ok(result)
    }
}

/// 取消选中分组内模组
///
/// `group_path` 提供且有效（位于 `_MANAGED_` 下 + group_xx）时走直写快路径：
/// O(1) 原子写 selectedindex=0 + 缓存定向 patch，免全量轻扫与整缓存失效；
/// 未提供或无效时回退 `deselect_group_mods` 全量扫描路径（旧行为）。
#[tauri::command]
pub async fn deselect_group_mod(
    game: String,
    mods_path: String,
    group_index: u32,
    group_path: Option<String>,
) -> Result<crate::core::mod_results::UpdateResult, String> {
    // P2：「_MANAGED_ 写操作全局锁」覆盖不全的补齐。取消选择同样写 selectedindex
    // 标记文件，此前游离于锁外，可与 select_mod / update_mod_data 并发交错。
    let _update_guard = UPDATE_LOCK.lock().await;
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);
    let managed_path = mods_path.join(constants::MANAGED_FOLDER);

    let group_path_buf = group_path.map(PathBuf::from);
    if let Some(gp) = group_path_buf.filter(|gp| is_direct_writable_group(gp, &managed_path)) {
        sel_dbg!(
            "mod_commands",
            "deselect_group_mod",
            "直写快路径 | group_path={} group_index={}（写 selectedindex=0，免全量轻扫）",
            gp.display(),
            group_index
        );
        let write_path = gp.clone();
        tauri::async_runtime::spawn_blocking(move || {
            crate::core::mod_select::write_selectedindex_direct(&write_path, 0)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| {
            log::error!(
                "[deselect_group_mod] write_selectedindex_direct error: {}",
                e
            );
            e.to_string()
        })?;
        // 同 select_mod：非阻塞 try_write，避免排在 watcher 增量合并的写锁段后空等
        // （selectedindex=0 已落盘，缓存对账交给 watcher / 下次全量扫描）
        let patched = match mod_cache_core::MOD_CACHE.try_write() {
            Some(mut cache) => cache.patch_selected_index(game, &mods_path, &gp, 0),
            None => {
                log::warn!(
                    "[deselect_group_mod] cache patch skipped: MOD_CACHE write lock busy (watcher merge in progress)"
                );
                false
            }
        };
        log::debug!(
            "[commands::mod_commands] [deselect_group_mod] direct write done | patched={}",
            patched
        );
        return Ok(crate::core::mod_results::UpdateResult {
            selected_mod_index: Some(0),
            ..Default::default()
        });
    }

    // 回退：全量轻扫定位分组（旧行为，含整缓存失效）
    let settings = settings_store::get_settings();
    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::core::mod_results::UpdateResult, String> {
            crate::core::mod_select::deselect_group_mods(game, &mods_path, &settings, group_index)
                .map_err(|e| e.to_string())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = mod_cache_core::MOD_CACHE.write();
        cache.invalidate_game(game); // 彻底失效该游戏缓存（invalidate_by_prefix 是 no-op 陷阱）
    }

    Ok(result)
}
