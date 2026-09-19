//! 模组分组管理域命令
//!
//! 从 mod_commands.rs 拆出（2026-09-19 Phase 6-2）。
//! 承载分组增删改、模组重命名、批量移除分组、F10 按键模拟与子文件夹合法性校验。
//! 选择类命令 select_mod / deselect_group_mod 已迁 mod_selection_commands。
//! 共享辅助经 `use crate::commands::mod_commands::{...}` 引用。

use crate::commands::mod_commands::{
    build_group_delta, build_removed_group_delta, log_delta, parse_game, resolve_managed_context,
    trash_delete, UPDATE_LOCK,
};
use crate::config::settings_store;
use crate::core::constants;
use crate::core::resolution;
use crate::models::mod_data::GroupDelta;
use crate::sel_dbg;
use std::fs;
use std::path::{Path, PathBuf};

/// 添加分组
#[tauri::command]
pub async fn add_group(
    game: String,
    mods_path: String,
    group_name: Option<String>,
) -> Result<crate::core::mod_results::UpdateResult, String> {
    let game = parse_game(&game)?;
    let mods_path = PathBuf::from(mods_path);

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::core::mod_results::UpdateResult, String> {
            let managed_folder = mods_path.join(constants::MANAGED_FOLDER);
            if !managed_folder.exists() {
                fs::create_dir_all(&managed_folder).map_err(|e| e.to_string())?;
            }

            let mut used_numbers: Vec<u32> = Vec::new();
            if let Ok(entries) = fs::read_dir(&managed_folder) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if let Some(rest) = name.strip_prefix("group_") {
                        if let Ok(num) = rest.parse::<u32>() {
                            used_numbers.push(num);
                        }
                    }
                }
            }
            used_numbers.sort();
            let mut group_num = 1u32;
            for used in &used_numbers {
                if *used == group_num {
                    group_num += 1;
                } else if *used > group_num {
                    break;
                }
            }

            // 对齐 NRMM 约定：分组上限 = 屏幕 Y 轴（高度）上限，由动态分辨率上限决定。
            // Dart 以 500 作为硬编码安全上限；NRMM 实际约定为基于主屏幕分辨率的动态上限
            // （group=y / mod=x），故此处采用 resolution::compute_limits().max_groups 作为权威上限。
            let max_groups = resolution::compute_limits().max_groups;
            if group_num > max_groups {
                return Err(format!(
                    "分组数量已达动态上限（{}，基于屏幕分辨率 Y 轴）",
                    max_groups
                ));
            }

            let dir_name = format!("group_{}", group_num);
            let group_path = managed_folder.join(&dir_name);
            fs::create_dir(&group_path).map_err(|e| e.to_string())?;

            let template_str = String::from_utf8_lossy(crate::resources::TEMPLATE_GROUP);
            let ini_content = template_str
                .replace("{group_x}", &dir_name)
                .replace("{x}", &group_num.to_string());
            let ini_path = group_path.join("ModFolder.ini");
            crate::utils::atomic_write(&ini_path, ini_content.as_bytes())
                .map_err(|e| e.to_string())?;

            if let Some(custom_name) = group_name {
                let trimmed = custom_name.trim();
                if !trimmed.is_empty() && trimmed != dir_name {
                    let new_group_path = managed_folder.join(trimmed);
                    if !new_group_path.exists() {
                        fs::rename(&group_path, &new_group_path).map_err(|e| e.to_string())?;
                    }
                }
            }

            Ok(crate::core::mod_results::UpdateResult::default())
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut cache = crate::core::mod_cache_core::MOD_CACHE.write();
        cache.invalidate_all();
    }

    let _ = game;
    Ok(result)
}

/// 删除分组（移至回收站）
///
/// 命令级增量（R1）：分组目录已删除无需磁盘重扫，直接 `subtree_remove`
/// 裁剪缓存，返回 `new_group = None` 的 [`GroupDelta`]（前端按 oldGroupPath
/// 剔除该组及其模组条目），不再触发全量失效与全量刷新。
#[tauri::command]
pub async fn remove_group(group_path: String) -> Result<GroupDelta, String> {
    // P2：删除分组会移走 _MANAGED_ 下整棵子树，须与选择、更新串行
    // （否则「删除分组后立刻选择」会出现组已消失但选择仍乐观成功的静默失败）。
    let _update_guard = UPDATE_LOCK.lock().await;
    let (mods_root, group_dir) = resolve_managed_context(Path::new(&group_path))
        .ok_or_else(|| format!("Path is not inside a managed group: {}", group_path))?;
    let path = settings_store::validate_managed_path(&group_path).map_err(|e| e.to_string())?;
    let game = settings_store::get_settings().target_game;

    let delta = tauri::async_runtime::spawn_blocking(move || -> Result<GroupDelta, String> {
        // 抑制 watcher 批次在删除/裁剪中途读取中间态（C11：暂停期事件保留，恢复后幂等合并）
        let _guard = crate::core::file_watcher::WatcherGuard::new();
        if !path.exists() {
            return Err("Group path does not exist".to_string());
        }
        trash_delete(&path).map_err(|e| e.to_string())?;
        let delta = build_removed_group_delta(game, &mods_root, &group_dir);
        log_delta("remove_group", &delta);
        Ok(delta)
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(delta)
}

/// 移除模组
///
/// 完整流程（后端移除逻辑不变）：
/// 1. 将模组文件夹移动到 `Mods/DISABLED_MANAGED_REMOVED/<modname>`（冲突追加 _1、_2…）
/// 2. 对移动后的模组执行 INI 还原（清理所有 NRMM 注入内容）
/// 3. 零扫描：MOD_CACHE 模组条目直接剔除（组树 + 扁平列表），前后端索引同步清除
/// 4. 缓存条目未命中（异常）→ 降级对受影响分组局部重扫 + `subtree_replace` 合并缓存，
///    `degraded=true` 返回完整组增量（前端 patch 后告知用户发生异常）
#[tauri::command]
pub async fn remove_mod(
    mod_path: String,
) -> Result<crate::core::mod_results::RemoveModResult, String> {
    // P2：移除模组会把目录移入 DISABLED_MANAGED_REMOVED 并改写组内 INI，
    // 与 select_mod / update_mod_data 同属 _MANAGED_ 写路径，须统一串行化。
    let _update_guard = UPDATE_LOCK.lock().await;
    let (mods_root, old_group_dir) = resolve_managed_context(Path::new(&mod_path))
        .ok_or_else(|| format!("Path is not inside a managed group: {}", mod_path))?;
    let path = settings_store::validate_managed_path(&mod_path).map_err(|e| e.to_string())?;
    let game = settings_store::get_settings().target_game;

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::core::mod_results::RemoveModResult, String> {
            // 抑制 watcher 批次在移动/还原中途读取中间态（C11：暂停期事件保留，恢复后幂等合并）
            let _guard = crate::core::file_watcher::WatcherGuard::new();
            let mut result =
                crate::core::mod_remove::remove_mod(&path).map_err(|e| e.to_string())?;
            // 零扫描缓存剔除：组树 + 扁平列表移除该模组条目；未命中（缓存无此条）→ 降级增量重扫
            let mut degraded = false;
            {
                let mut cache = crate::core::mod_cache_core::MOD_CACHE.write();
                if !cache.remove_mod_entry(game, &mods_root, &path) {
                    degraded = true;
                }
            } // 释放写锁
            if degraded {
                // 局部重扫入口：mod 目录已被移走（旧路径不存在），传 parent（分组目录，
                // 操作前后恒存在）——scan_partial_path 会扩展到所属分组
                let rescan_dir = path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| old_group_dir.clone());
                result.group_delta =
                    build_group_delta(game, &mods_root, &old_group_dir, &rescan_dir)
                        .map_err(|e| e.to_string())?;
                log_delta("remove_mod(degraded)", &result.group_delta);
            }
            result.degraded = degraded;
            Ok(result)
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    Ok(result)
}

/// 重命名模组
///
/// 命令级增量（R1）：重命名（或写 modname 标记文件）后对受影响分组局部重扫，
/// 返回 [`GroupDelta`]（新模组路径可从 delta.new_group 内获取），不再触发
/// 全量失效与全量刷新。
#[tauri::command]
pub async fn rename_mod(mod_path: String, new_name: String) -> Result<GroupDelta, String> {
    // P2：重命名会写 modname 标记文件 / 重命名 _MANAGED_ 下目录，须与选择、更新串行。
    let _update_guard = UPDATE_LOCK.lock().await;
    let (mods_root, old_group_dir) = resolve_managed_context(Path::new(&mod_path))
        .ok_or_else(|| format!("Path is not inside a managed group: {}", mod_path))?;
    let path = settings_store::validate_managed_path(&mod_path).map_err(|e| e.to_string())?;
    settings_store::validate_filename(&new_name).map_err(|e| e.to_string())?;
    let game = settings_store::get_settings().target_game;

    let delta = tauri::async_runtime::spawn_blocking(move || -> Result<GroupDelta, String> {
        // 抑制 watcher 批次在重命名/写标记文件中途读取中间态
        let _guard = crate::core::file_watcher::WatcherGuard::new();
        if !path.exists() {
            return Err("Mod path does not exist".to_string());
        }
        let parent = path
            .parent()
            .ok_or_else(|| "Invalid mod path".to_string())?;
        let parent_name = parent
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // NRMM 逻辑：group_xx 普通分组下的模组，重命名仅修改 modname 标记文件（展示名），文件夹名保持不变
        let new_path = if crate::core::mod_scan_util::is_group_xx_dir(&parent_name) {
            let modname_path = path.join("modname");
            crate::utils::atomic_write(&modname_path, new_name.as_bytes())
                .map_err(|e| format!("Failed to write modname file: {}", e))?;
            path.clone()
        } else {
            // 其余情况（互斥组等）：重命名文件夹
            let dir_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_disabled = crate::core::mod_scan_util::is_disabled_dir(&dir_name);
            let final_name = if is_disabled {
                format!(
                    "{}{}",
                    constants::DISABLED_PREFIX,
                    crate::core::mod_scan_util::strip_disabled_prefix(&new_name)
                )
            } else {
                new_name.clone()
            };
            let new_path = parent.join(final_name);
            fs::rename(&path, &new_path).map_err(|e| e.to_string())?;
            new_path
        };

        // 局部重扫入口（须为操作后存在的目录）：
        // - group_xx 分支 → mod_path（modname 文件变化，mod 目录未动）
        // - mutex 分支 → 重命名后的 mod 目录
        // 两分支返回值恰好即各自场景的存在目录，直接以其为重扫入口
        let delta = build_group_delta(game, &mods_root, &old_group_dir, &new_path)
            .map_err(|e| e.to_string())?;
        log_delta("rename_mod", &delta);
        Ok(delta)
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(delta)
}

/// 重命名分组
///
/// 根据分组类型采用不同策略：
/// - group_xx（is_group_xx=true）：修改 groupname 文件内容（无则创建）
/// - 非group（is_group_xx=false）：重命名目录
///
/// 路径不存在时返回异常。
///
/// 命令级增量（R1）：重命名后对受影响分组局部重扫（group_xx 分支重扫原
/// 分组目录、非 group 分支重扫新组目录），返回 [`GroupDelta`]（分组自身
/// 重命名时 oldGroupPath 与 newGroup.groupPath 不同，前端按旧键定位替换）。
#[tauri::command]
pub async fn rename_group(
    group_path: String,
    new_name: String,
    is_group_xx: bool,
) -> Result<GroupDelta, String> {
    // P2：写 groupname 标记文件 / 重命名 _MANAGED_ 下分组目录，须与选择、更新串行。
    let _update_guard = UPDATE_LOCK.lock().await;
    let (mods_root, old_group_dir) = resolve_managed_context(Path::new(&group_path))
        .ok_or_else(|| format!("Path is not inside a managed group: {}", group_path))?;
    let path = settings_store::validate_managed_path(&group_path).map_err(|e| e.to_string())?;
    settings_store::validate_filename(&new_name).map_err(|e| e.to_string())?;
    let game = settings_store::get_settings().target_game;

    let delta = tauri::async_runtime::spawn_blocking(move || -> Result<GroupDelta, String> {
        // 抑制 watcher 批次在重命名/写标记文件中途读取中间态
        let _guard = crate::core::file_watcher::WatcherGuard::new();
        if !path.exists() {
            return Err("Group path does not exist".to_string());
        }

        // 局部重扫入口（须为操作后存在的目录）：
        // - group_xx 分支 → path（仅 groupname 文件变化，目录未动）
        // - 非 group 分支 → 新组目录（旧目录已重命名不存在）
        let rescan_dir = if is_group_xx {
            let groupname_path = path.join("groupname");
            crate::utils::atomic_write(&groupname_path, new_name.as_bytes())
                .map_err(|e| format!("Failed to write groupname file: {}", e))?;
            path.clone()
        } else {
            let parent = path
                .parent()
                .ok_or_else(|| "Invalid group path".to_string())?;
            let target = parent.join(&new_name);
            if target.exists() {
                return Err(format!(
                    "Target directory already exists: {}",
                    target.display()
                ));
            }
            fs::rename(&path, &target).map_err(|e| e.to_string())?;
            target
        };

        let delta = build_group_delta(game, &mods_root, &old_group_dir, &rescan_dir)
            .map_err(|e| e.to_string())?;
        log_delta("rename_group", &delta);
        Ok(delta)
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(delta)
}

/// 模拟 F10 按键（3Dmigoto 重载快捷键）
///
/// 与 NRMM 的 simulateKeyF10() 对齐，用于 update_mod_data 完成后
/// 需要手动重载 3Dmigoto 时向游戏发送 F10 按键。
/// 若传入 game 参数则尝试定向发送到目标游戏窗口，否则全局发送。
///
/// 若用户在设置中关闭了 `simulate_key_on_selection`（触发选择功能），
/// 则跳过 F10 发送，避免关闭后仍触发刷新的异常。
#[tauri::command]
pub async fn simulate_f10(game: Option<String>) -> Result<(), String> {
    let settings = settings_store::get_settings();
    sel_dbg!(
        "mod_commands",
        "simulate_f10",
        "入口 | game={:?} simulate_key_on_selection={}",
        game,
        settings.simulate_key_on_selection
    );
    if !settings.simulate_key_on_selection {
        log::debug!("[simulate_f10] simulate_key_on_selection=false, 跳过 F10 发送");
        sel_dbg!(
            "mod_commands",
            "simulate_f10",
            "已跳过 F10 发送（simulate_key_on_selection=false）"
        );
        return Ok(());
    }
    let game_for_log = game.clone();

    let mut simulator = crate::platform::get_key_simulator();
    if let Some(game_str) = game {
        if let Ok(game_enum) = parse_game(&game_str) {
            let process_names = game_enum.process_names();
            if let Some(first_pn) = process_names.first() {
                let _ = simulator.set_target_process(first_pn);
            }
        }
    }
    simulator.simulate_f10().map_err(|e| e.to_string())?;
    sel_dbg!(
        "mod_commands",
        "simulate_f10",
        "已发送 F10 重载按键（目标游戏={:?}）",
        game_for_log
    );
    Ok(())
}

/// 校验子文件夹名称合法性
///
/// # 参数
/// - parent_path: 父目录路径
/// - folder_name: 待校验的文件夹名称
///
/// # 返回值
/// - Ok((sanitized_name, true)): 名称合法，返回清理后的名称
/// - Ok((sanitized_name, false)): 名称不合法，sanitized_name为清理后名称，附带错误信息
/// - Err: 校验过程出错
#[tauri::command]
pub fn validate_subfolder_name(
    parent_path: String,
    folder_name: String,
) -> Result<(String, bool, String), String> {
    let parent = PathBuf::from(&parent_path);

    // 1. 清理名称：trim首尾空白
    let sanitized = folder_name.trim().to_string();

    // 2. 空名检查
    if sanitized.is_empty() {
        return Ok((sanitized, false, "分组名不能为空".to_string()));
    }

    // 3. 通用禁止名称：. 和 ..
    if sanitized == "." || sanitized == ".." {
        return Ok((
            sanitized,
            false,
            "文件夹名称不能为 \".\" 或 \"..\"".to_string(),
        ));
    }

    // 4. 平台非法字符检查
    #[cfg(target_os = "windows")]
    {
        let illegal_chars: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
        let mut found_chars = Vec::new();
        for c in illegal_chars {
            if sanitized.contains(*c) {
                found_chars.push(*c);
            }
        }
        // 检查控制字符 0x00-0x1F
        for c in sanitized.chars() {
            if (c as u32) < 0x20 {
                found_chars.push(' ');
                break;
            }
        }
        if !found_chars.is_empty() {
            let chars_str: String = found_chars.iter().collect();
            return Ok((
                sanitized,
                false,
                format!("目录名包含非法字符: {}", chars_str),
            ));
        }

        // Windows: 末尾不能是点或空格
        if sanitized.ends_with('.') || sanitized.ends_with(' ') {
            return Ok((
                sanitized.trim_end_matches(&['.', ' '][..]).to_string(),
                false,
                "文件夹名称末尾不能是点或空格".to_string(),
            ));
        }

        // Windows保留名称检查（不区分大小写）
        let upper_name = sanitized.to_uppercase();
        let reserved_names = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        for name in &reserved_names {
            if upper_name == *name {
                return Ok((
                    sanitized,
                    false,
                    "该名称为系统保留名称，请换一个名称".to_string(),
                ));
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if sanitized.contains('/') || sanitized.contains('\0') {
            return Ok((sanitized, false, "目录名包含非法字符: /".to_string()));
        }
    }

    // 5. 父目录存在检查
    if !parent.exists() {
        return Ok((sanitized, false, "父目录不存在，请刷新后重试".to_string()));
    }

    // 6. 目标路径是否已存在
    let target_path = parent.join(&sanitized);
    if target_path.exists() {
        return Ok((
            sanitized,
            false,
            "该名称的文件夹已存在，请换一个名称".to_string(),
        ));
    }

    // 7. 路径长度检查（Windows MAX_PATH 限制约260字符）
    #[cfg(target_os = "windows")]
    {
        let path_len = target_path.as_os_str().len();
        if path_len > 240 {
            return Ok((sanitized, false, "文件夹路径过长，请缩短名称".to_string()));
        }
    }

    Ok((sanitized, true, String::new()))
}

/// 创建子文件夹
///
/// # 参数
/// - parent_path: 父目录路径
/// - folder_name: 文件夹名称（应先通过validate_subfolder_name校验）
///
/// # 返回值
/// - Ok(()): 创建成功
/// - Err: 错误信息（用户友好描述）
#[tauri::command]
pub fn create_subfolder(parent_path: String, folder_name: String) -> Result<(), String> {
    let parent = PathBuf::from(&parent_path);

    // 二次校验
    let (sanitized, valid, err_msg) = validate_subfolder_name(parent_path, folder_name)?;
    if !valid {
        return Err(err_msg);
    }

    let target_path = parent.join(&sanitized);

    // 创建目录（只创建最后一级，不递归创建父目录）
    match fs::create_dir(&target_path) {
        Ok(_) => Ok(()),
        Err(e) => {
            let err_msg = match e.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    "没有权限在此位置创建文件夹，请检查权限设置".to_string()
                }
                std::io::ErrorKind::AlreadyExists => {
                    "该名称的文件夹已存在，请换一个名称".to_string()
                }
                std::io::ErrorKind::InvalidFilename => "文件夹名称包含不允许的字符".to_string(),
                _ => "创建文件夹失败，请检查路径是否可访问".to_string(),
            };
            log::error!("Failed to create subfolder {:?}: {}", target_path, e);
            Err(err_msg)
        }
    }
}

/// 移除分组（NRMM 对齐：移至 DISABLED_MANAGED_REMOVED；非group先移子分组到父级再移除）
///
/// 顶层函数：负责从设置数据获取 Mods 根目录、在线程池中执行底层移动、
/// 处理错误并记录日志。具体「哪些目录移动到哪里」由底层编排层 `remove_group_ex` 决定。
///
/// 命令级增量（R1）：分组目录（含其全部子树）已移出扫描范围，无需磁盘重扫，
/// 直接 `subtree_remove` 裁剪缓存，返回 `new_group = None` 的 [`GroupDelta`]。
#[tauri::command]
pub async fn remove_group_ex(group_path: String, is_group_xx: bool) -> Result<GroupDelta, String> {
    // P2：与 remove_group 同理，移动 _MANAGED_ 下分组子树，须统一串行化
    let _update_guard = UPDATE_LOCK.lock().await;
    // 路径边界校验（R1）：确认 group_path canonicalize 后位于任一已配置 mods_root 内
    let _validated =
        settings_store::validate_managed_path(&group_path).map_err(|e| e.to_string())?;
    let path = PathBuf::from(group_path);
    // 命令级增量（R1）：定位受影响顶层分组（磁盘目录将被移出，直接裁剪缓存子树）
    let (mods_root, group_dir) = resolve_managed_context(&path)
        .ok_or_else(|| format!("Path is not inside a managed group: {}", path.display()))?;
    let game = settings_store::get_settings().target_game;
    log::info!(
        "[remove_group_ex] start | group_path={:?} is_group_xx={}",
        path,
        is_group_xx
    );

    // 从设置数据定位 Mods 根目录（与 _MANAGED_ 同级；供底层移动编排使用）
    let settings = settings_store::get_settings();
    let mods_root_for_move = settings
        .game_mods_path
        .values()
        .map(PathBuf::from)
        .find(|p| path.starts_with(p))
        .ok_or_else(|| {
            log::error!(
                "[remove_group_ex] failed to locate Mods root from settings for {:?}",
                path
            );
            "无法从设置数据定位 Mods 根目录".to_string()
        })?;
    log::debug!("[remove_group_ex] mods_root={:?}", mods_root_for_move);

    let path_for_log = path.clone();
    let delta = tauri::async_runtime::spawn_blocking(move || -> Result<GroupDelta, String> {
        // 抑制 watcher 批次在批量移动中途读取中间态（C11：暂停期事件保留，恢复后幂等合并）
        let _guard = crate::core::file_watcher::WatcherGuard::new();
        crate::core::mod_remove::remove_group_ex(&path, is_group_xx, &mods_root_for_move)
            .map_err(|e| e.to_string())?;
        let delta = build_removed_group_delta(game, &mods_root, &group_dir);
        log_delta("remove_group_ex", &delta);
        Ok(delta)
    })
    .await
    .map_err(|e| e.to_string())??;

    log::info!(
        "[remove_group_ex] completed | group_path={:?}",
        path_for_log
    );
    Ok(delta)
}
