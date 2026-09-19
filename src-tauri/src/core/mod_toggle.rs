//! 模组启停域：单模组启停、互斥组启停、批量启停
//!
//! 从 mod_manager.rs 拆出（2026-09-19 Phase 4-3）。
//! 依赖 mod_scanner（DISABLED 前缀识别/剥离、目录扫描）、mod_path_utils（路径比较）、
//! constants、file_watcher::WatcherGuard；被 commands/toggle_mod_disabled、
//! batch_toggle_mods、disable/enable_all 消费。

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::constants;
use crate::core::file_watcher::WatcherGuard;
use crate::core::mod_path_utils::path_eq_ignore_case;
use crate::core::mod_scanner;
use crate::sel_dbg;

/// 切换单个模组的启用/禁用状态（独立开关，不影响同组其他模组）
///
/// 检查磁盘实际状态，支持幂等操作：
/// - 传入路径不存在时，检查父目录下是否有对应的启用/禁用版本
/// - 传入路径存在时，检查是否需要重命名
/// - 已经是目标状态时，直接返回（幂等）
///
/// # 参数
/// - `mod_path`: 模组目录路径
/// - `enable`: true = 启用（移除 DISABLED 前缀），false = 禁用（添加 DISABLED 前缀）
pub fn toggle_mod(mod_path: &Path, enable: bool) -> Result<()> {
    let dir_name = mod_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let parent = mod_path.parent().unwrap_or(mod_path);
    let is_disabled = mod_scanner::is_disabled_dir(&dir_name);

    if enable {
        if mod_path.exists() {
            // 传入路径存在 → 检查是否需要启用
            if is_disabled {
                let new_name = mod_scanner::strip_disabled_prefix(&dir_name);
                let new_path = parent.join(new_name);
                if mod_path != new_path {
                    fs::rename(mod_path, &new_path)
                        .with_context(|| format!("Failed to enable mod: {:?}", mod_path))?;
                }
            }
            // 已启用 → 幂等，直接返回
        } else {
            // 传入路径不存在 → 检查父目录下是否有启用版本
            if is_disabled {
                let enabled_name = mod_scanner::strip_disabled_prefix(&dir_name);
                let enabled_path = parent.join(enabled_name);
                if enabled_path.exists() {
                    // 已启用 → 幂等，直接返回
                    return Ok(());
                }
            }
            // 路径不存在且无启用版本 → 返回错误
            return Err(anyhow::anyhow!("Mod path does not exist: {:?}", mod_path));
        }
    } else {
        // disable
        if mod_path.exists() {
            // 传入路径存在 → 检查是否需要禁用
            if !is_disabled {
                let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                let new_path = parent.join(new_name);
                if mod_path != new_path {
                    if new_path.exists() {
                        log::warn!(
                            "Target path already exists, skipping disable: {:?}",
                            new_path
                        );
                    } else {
                        fs::rename(mod_path, &new_path)
                            .with_context(|| format!("Failed to disable mod: {:?}", mod_path))?;
                    }
                }
            }
            // 已禁用 → 幂等，直接返回
        } else {
            // 传入路径不存在 → 检查父目录下是否有禁用版本
            if !is_disabled {
                let disabled_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                let disabled_path = parent.join(disabled_name);
                if disabled_path.exists() {
                    // 已禁用 → 幂等，直接返回
                    return Ok(());
                }
            }
            // 路径不存在且无禁用版本 → 返回错误
            return Err(anyhow::anyhow!("Mod path does not exist: {:?}", mod_path));
        }
    }

    Ok(())
}

/// 互斥启用模组：启用指定模组，禁用其父目录下同级的其他模组叶子节点（含.ini的目录）
///
/// - 只处理父目录下的一级子目录，不递归子分组
/// - 没有.ini的目录（分组节点/子分组）不处理
/// - 重命名保持幂等，检查目标路径是否存在避免覆盖
pub fn enable_mutex_mod(mod_path: &Path) -> Result<()> {
    // 暂停文件监听器，防止目录重命名触发增量更新竞态
    // WatcherGuard 在函数退出时（无论成功或出错）自动恢复监听器
    let _guard = WatcherGuard::new();
    sel_dbg!(
        "mod_manager",
        "enable_mutex_mod",
        "调用链: select_mod(互斥) → enable_mutex_mod | 入口 | 模组路径={:?} 模组名称={}",
        mod_path,
        mod_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    );

    let parent_dir = mod_path
        .parent()
        .with_context(|| format!("Failed to get parent directory of: {:?}", mod_path))?;

    let entries = fs::read_dir(parent_dir)
        .with_context(|| format!("Failed to read parent directory: {:?}", parent_dir))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }

        let has_ini = mod_scanner::dir_has_ini_file(&path)?;
        if !has_ini {
            continue;
        }

        if path_eq_ignore_case(&path, mod_path) {
            let current_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_disabled = mod_scanner::is_disabled_dir(&current_name);
            if is_disabled {
                let new_name = mod_scanner::strip_disabled_prefix(&current_name);
                let new_path = path.parent().unwrap_or(&path).join(new_name);
                if path != new_path {
                    if new_path.exists() {
                        log::warn!(
                            "Target path already exists, skipping enable: {:?}",
                            new_path
                        );
                    } else {
                        sel_dbg!(
                            "mod_manager",
                            "enable_mutex_mod",
                            "步骤=启用目标模组(移除 DISABLED 前缀) | {:?} → {:?}",
                            path,
                            new_path
                        );
                        fs::rename(&path, &new_path)
                            .with_context(|| format!("Failed to enable mod: {:?}", path))?;
                    }
                }
            }
        } else {
            let current_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_disabled = mod_scanner::is_disabled_dir(&current_name);
            if !is_disabled {
                let new_name = format!("{}{}", constants::DISABLED_PREFIX, current_name);
                let new_path = path.parent().unwrap_or(&path).join(new_name);
                if path != new_path {
                    if new_path.exists() {
                        log::warn!(
                            "Target path already exists, skipping disable: {:?}",
                            new_path
                        );
                    } else {
                        fs::rename(&path, &new_path).with_context(|| {
                            format!("Failed to disable sibling mod: {:?}", path)
                        })?;
                    }
                }
            }
        }
    }

    Ok(())
}

/// 禁用互斥模组：给模组目录添加DISABLED前缀
///
/// - 检查磁盘实际状态，支持幂等操作
/// - 传入路径不存在时，检查父目录下是否已有禁用版本
/// - 如果已经以DISABLED开头（大小写不敏感）则不做任何操作（幂等）
/// - 检查目标路径不存在才重命名
pub fn disable_mutex_mod(mod_path: &Path) -> Result<()> {
    let dir_name = mod_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let parent = mod_path.parent().unwrap_or(mod_path);
    let is_disabled = mod_scanner::is_disabled_dir(&dir_name);

    // 传入路径不存在 → 检查磁盘上是否已禁用
    if !mod_path.exists() {
        if !is_disabled {
            let disabled_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
            let disabled_path = parent.join(disabled_name);
            if disabled_path.exists() {
                // 已禁用 → 幂等，直接返回
                return Ok(());
            }
        }
        return Err(anyhow::anyhow!("Mod path does not exist: {:?}", mod_path));
    }

    // 传入路径存在 → 检查是否需要禁用
    if !is_disabled {
        let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
        let new_path = parent.join(new_name);
        if mod_path != new_path {
            if new_path.exists() {
                log::warn!(
                    "Target path already exists, skipping disable: {:?}",
                    new_path
                );
            } else {
                fs::rename(mod_path, &new_path)
                    .with_context(|| format!("Failed to disable mod: {:?}", mod_path))?;
            }
        }
    }

    Ok(())
}

/// 禁用指定分组下的所有一级模组（叶子节点，含 .ini 文件的目录）
///
/// 遍历分组目录下的所有直接子目录，对每个未禁用的模组添加 DISABLED 前缀。
/// 跳过：
/// - 非目录
/// - 已禁用的模组目录（名称以 DISABLED 开头）
/// - 不含 .ini 文件的目录（分组节点/子分组目录）
///
/// 不递归处理子分组目录，仅处理一级子目录。
///
/// # 参数
/// - `group_path`: 分组目录路径
///
/// # 返回
/// 成功禁用的模组数量（已禁用的跳过不计入）
///
/// # 错误
/// - 读取目录失败时返回错误
/// - 重命名失败时返回错误
pub fn disable_all_mods_in_group(group_path: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    if !group_path.exists() {
        return Ok(Vec::new());
    }
    if !group_path.is_dir() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(group_path)
        .with_context(|| format!("Failed to read group directory: {:?}", group_path))?;

    let mut renamed: Vec<(PathBuf, PathBuf)> = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }

        // 仅处理含 .ini 的叶子模组节点（不含 .ini 的是分组节点/子分组，跳过）
        let has_ini = match mod_scanner::dir_has_ini_file(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !has_ini {
            continue;
        }

        // 检查是否已禁用（名称以 DISABLED 开头，大小写不敏感）
        let is_disabled = mod_scanner::is_disabled_dir(&dir_name);
        if is_disabled {
            continue;
        }

        // 构建新目录名：DISABLED + 原名称
        let new_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
        let new_path = group_path.join(&new_name);

        // 检查目标路径是否已存在（避免覆盖）
        if new_path.exists() {
            log::warn!(
                "Target path already exists, skipping disable: {:?}",
                new_path
            );
            continue;
        }

        // 重命名添加前缀，并记录 (old, new) 供调用方零扫描更新缓存
        fs::rename(&path, &new_path)
            .with_context(|| format!("Failed to disable mod: {:?}", path))?;
        renamed.push((path, new_path));
    }

    Ok(renamed)
}

/// 判断模组是否属于MutexGroup（非group_xx目录下的模组）
///
/// 逻辑：获取mod_path相对于managed_path的路径，检查第一个路径段是否匹配group_xx正则；
/// 如果第一个路径段不匹配group_xx（即不是NormalGroup），则是mutex mod。
pub fn is_mutex_mod(mod_path: &Path, managed_path: &Path) -> bool {
    let rel_path = match mod_path.strip_prefix(managed_path) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let first_component = rel_path.components().next();
    match first_component {
        Some(std::path::Component::Normal(name)) => {
            let name_str = name.to_string_lossy();
            mod_scanner::is_normal_group_dir(&name_str).is_none()
        }
        _ => false,
    }
}

/// 批量切换模组启用/禁用状态
///
/// 只计数实际发生变更的模组数量。
///
/// # 参数
/// - `mod_paths`: 模组路径列表
/// - `enable`: true=启用, false=禁用
/// - `is_mutex`: 是否为互斥组模组
pub fn batch_toggle_mods(mod_paths: &[String], enable: bool, is_mutex: bool) -> Result<u32> {
    let mut count = 0u32;
    for path_str in mod_paths {
        let path = PathBuf::from(path_str);

        // 预检查是否需要操作
        let dir_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let parent = path.parent().unwrap_or(&path);
        let is_disabled = mod_scanner::is_disabled_dir(&dir_name);

        // 判断是否需要操作
        let needs_change = if enable {
            // 启用：路径存在且已禁用，或路径不存在但父目录有禁用版本
            if path.exists() {
                is_disabled
            } else {
                !is_disabled || {
                    let enabled_name = mod_scanner::strip_disabled_prefix(&dir_name);
                    !parent.join(enabled_name).exists()
                }
            }
        } else {
            // 禁用：路径存在且未禁用，或路径不存在但父目录有启用版本
            if path.exists() {
                !is_disabled
            } else {
                is_disabled || {
                    let disabled_name = format!("{}{}", constants::DISABLED_PREFIX, dir_name);
                    !parent.join(disabled_name).exists()
                }
            }
        };

        if !needs_change {
            continue;
        }

        if is_mutex {
            if enable {
                enable_mutex_mod(&path)?;
            } else {
                disable_mutex_mod(&path)?;
            }
        } else {
            toggle_mod(&path, enable)?;
        }
        count += 1;
    }
    Ok(count)
}

/// 启用指定分组下的所有一级模组（叶子节点，含 .ini 文件的目录）
///
/// 遍历分组目录下的所有直接子目录，对每个已禁用的模组移除 DISABLED 前缀。
/// 跳过：
/// - 非目录
/// - 已启用的模组目录（名称不以 DISABLED 开头）
/// - 不含 .ini 文件的目录（分组节点/子分组目录）
///
/// 不递归处理子分组目录，仅处理一级子目录。
///
/// # 参数
/// - `group_path`: 分组目录路径
///
/// # 返回
/// 成功启用的模组数量（已启用的跳过不计入）
///
/// # 错误
/// - 读取目录失败时返回错误
/// - 重命名失败时返回错误
pub fn enable_all_mods_in_group(group_path: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    if !group_path.exists() {
        return Ok(Vec::new());
    }
    if !group_path.is_dir() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(group_path)
        .with_context(|| format!("Failed to read group directory: {:?}", group_path))?;

    let mut renamed: Vec<(PathBuf, PathBuf)> = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }

        // 仅处理含 .ini 的叶子模组节点
        let has_ini = match mod_scanner::dir_has_ini_file(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !has_ini {
            continue;
        }

        // 检查是否已禁用（名称以 DISABLED 开头，大小写不敏感）
        let is_disabled = mod_scanner::is_disabled_dir(&dir_name);
        if !is_disabled {
            continue;
        }

        // 去除 DISABLED 前缀及前导分隔符 _ 空格 - 等
        let new_name = mod_scanner::strip_disabled_prefix(&dir_name);
        let new_name = if new_name.is_empty() {
            // 极端情况：整个目录名就是前缀，跳过
            continue;
        } else {
            new_name
        };

        let new_path = group_path.join(&new_name);

        if new_path.exists() {
            log::warn!(
                "Target path already exists, skipping enable: {:?}",
                new_path
            );
            continue;
        }

        // 源路径和目标路径相同则跳过
        if path == new_path {
            continue;
        }

        fs::rename(&path, &new_path)
            .with_context(|| format!("Failed to enable mod: {:?}", path))?;

        renamed.push((path, new_path));
    }

    Ok(renamed)
}

#[cfg(test)]
#[path = "mod_toggle_tests_disable_all.rs"]
mod tests_disable_all;

#[cfg(test)]
#[path = "mod_toggle_tests_enable_all.rs"]
mod tests_enable_all;

#[cfg(test)]
#[path = "mod_toggle_tests.rs"]
mod tests;
