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
mod tests_disable_all {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// 创建测试辅助：在指定目录下创建含 .ini 的模组目录
    fn create_mod_dir(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        fs::create_dir_all(&dir).unwrap();
        // 创建一个 .ini 文件使该目录被识别为叶子模组节点
        fs::write(dir.join(format!("{}.ini", name)), "[ShaderOverride]").unwrap();
        dir
    }

    /// 正常场景：禁用3个启用模组，跳过1个已禁用模组，跳过无子分组目录
    #[test]
    fn test_disable_all_basic() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();

        let _mod_a = create_mod_dir(&group, "ModA"); // 启用
        let _mod_b = create_mod_dir(&group, "DISABLEDModB"); // 已禁用
        let _mod_c = create_mod_dir(&group, "ModC"); // 启用
                                                     // 子分组目录（无 .ini），应跳过
        let subgroup = group.join("SubGroup");
        fs::create_dir_all(&subgroup).unwrap();
        let _mod_d = create_mod_dir(&group, "ModD"); // 启用

        let count = disable_all_mods_in_group(&group).unwrap().len() as u32;
        assert_eq!(count, 3); // ModA, ModC, ModD

        // 验证目录状态
        let entries: Vec<String> = fs::read_dir(&group)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.iter().any(|n| n == "DISABLEDModA"), "ModA 应被禁用");
        assert!(
            entries.iter().any(|n| n == "DISABLEDModB"),
            "DisabledModB 应保持不变"
        );
        assert!(entries.iter().any(|n| n == "DISABLEDModC"), "ModC 应被禁用");
        assert!(entries.iter().any(|n| n == "DISABLEDModD"), "ModD 应被禁用");
        assert!(
            entries.iter().any(|n| n == "SubGroup"),
            "子分组目录应保持不变"
        );
        assert_eq!(entries.len(), 5, "目录数量不变（重命名不改变数量）");
    }

    /// 幂等性：对已全部禁用的目录不产生任何变化
    #[test]
    fn test_disable_all_idempotent() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();

        create_mod_dir(&group, "DISABLEDModA");
        create_mod_dir(&group, "DISABLEDModB");

        let count = disable_all_mods_in_group(&group).unwrap().len() as u32;
        assert_eq!(count, 0, "全部已禁用时计数为0");
    }

    /// 幂等性：对已含 DISABLED 前缀的目录不重复加前缀（仍为 DISABLEDxxx 而非 DISABLEDDISABLEDxxx）
    #[test]
    fn test_disable_all_no_duplicate_prefix_on_disabled() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();

        create_mod_dir(&group, "DISABLEDModA");
        create_mod_dir(&group, "DISABLEDModB");
        create_mod_dir(&group, "ModC");

        let count = disable_all_mods_in_group(&group).unwrap().len() as u32;
        assert_eq!(count, 1, "仅未禁用的 ModC 应被禁用");

        let entries: Vec<String> = fs::read_dir(&group)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            entries.iter().any(|n| n == "DISABLEDModA"),
            "DISABLEDModA 应保持不变"
        );
        assert!(
            entries.iter().any(|n| n == "DISABLEDModB"),
            "DISABLEDModB 应保持不变"
        );
        assert!(
            entries.iter().all(|n| !n.starts_with("DISABLEDDISABLED")),
            "不应产生 DISABLEDDISABLEDxxx 重复前缀目录"
        );
    }

    /// 无模组场景：仅有子分组或空目录
    #[test]
    fn test_disable_all_no_mods() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();

        let subgroup = group.join("SubGroup");
        fs::create_dir_all(&subgroup).unwrap();

        let count = disable_all_mods_in_group(&group).unwrap().len() as u32;
        assert_eq!(count, 0);
    }

    /// 组目录含 `groupname` 标记文件时，批量禁用不得将其重命名为 `DISABLEDgroupname`
    ///
    /// 真实磁盘上 group_xx 目录必有 `groupname` 标记文件（与模组目录并存）。
    /// 批量禁用仅处理「目录 + 含 .ini」的叶子模组节点；标记文件（groupname/
    /// selectedindex/modname）虽位于组目录下，但为普通文件，必须原样保留——
    /// 若逻辑误按条目名加 DISABLED 前缀，会产生 `DISABLEDgroupname` 垃圾文件。
    #[test]
    fn test_disable_all_keeps_groupname_marker() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();

        // 模拟真实组目录：标记文件 + 模组子目录并存
        fs::write(group.join("groupname"), "My Custom Group\n").unwrap();
        fs::write(group.join("selectedindex"), "1\n").unwrap();
        create_mod_dir(&group, "ModA");

        let count = disable_all_mods_in_group(&group).unwrap().len() as u32;
        assert_eq!(count, 1, "仅 ModA 应被禁用");

        assert!(
            fs::read_to_string(group.join("groupname")).unwrap() == "My Custom Group\n",
            "groupname 标记文件内容应原样保留"
        );
        assert!(
            group.join("selectedindex").exists(),
            "selectedindex 标记文件应原样保留"
        );
        assert!(
            !group.join("DISABLEDgroupname").exists(),
            "不得把 groupname 标记文件重命名为 DISABLEDgroupname"
        );
        assert!(group.join("DISABLEDModA").exists(), "ModA 应被禁用");
    }
}

#[cfg(test)]
mod tests_enable_all {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_mod_dir(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{}.ini", name.trim_start_matches("DISABLED"))),
            "[ShaderOverride]",
        )
        .unwrap();
        dir
    }

    /// 正常场景：启用3个禁用模组，跳过1个启用模组，跳过子分组
    #[test]
    fn test_enable_all_basic() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();

        create_mod_dir(&group, "DISABLEDModA"); // 需启用
        create_mod_dir(&group, "ModB"); // 已启用，跳过
        create_mod_dir(&group, "DISABLEDModC"); // 需启用
                                                // 子分组目录（无 .ini），跳过
        let subgroup = group.join("SubGroup");
        fs::create_dir_all(&subgroup).unwrap();
        create_mod_dir(&group, "DISABLEDModD"); // 需启用

        let count = enable_all_mods_in_group(&group).unwrap().len() as u32;
        assert_eq!(count, 3); // ModA, ModC, ModD

        let entries: Vec<String> = fs::read_dir(&group)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.iter().any(|n| n == "ModA"), "ModA 应被启用");
        assert!(entries.iter().any(|n| n == "ModB"), "ModB 保持已启用");
        assert!(entries.iter().any(|n| n == "ModC"), "ModC 应被启用");
        assert!(entries.iter().any(|n| n == "ModD"), "ModD 应被启用");
        assert!(entries.iter().any(|n| n == "SubGroup"), "子分组不变");
    }

    /// 幂等性：全部已启用计数为0
    #[test]
    fn test_enable_all_idempotent() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();
        create_mod_dir(&group, "ModA");
        create_mod_dir(&group, "ModB");

        let count = enable_all_mods_in_group(&group).unwrap().len() as u32;
        assert_eq!(count, 0);
    }

    /// 前缀后跟分隔符场景：DISABLED_MyMod 应为 MyMod
    #[test]
    fn test_enable_all_prefix_with_separator() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();
        create_mod_dir(&group, "DISABLED_MyMod"); // 下划线分隔
        create_mod_dir(&group, "DISABLED-OtherMod"); // 横杠分隔
        create_mod_dir(&group, "DISABLED Third"); // 空格分隔

        let count = enable_all_mods_in_group(&group).unwrap().len() as u32;
        assert_eq!(count, 3);

        let entries: Vec<String> = fs::read_dir(&group)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.iter().any(|n| n == "MyMod"));
        assert!(entries.iter().any(|n| n == "OtherMod"));
        assert!(entries.iter().any(|n| n == "Third"));
    }

    /// 组目录含 `groupname` 标记文件时，批量启用不得误将其当模组名处理
    ///
    /// 与 `tests_disable_all::test_disable_all_keeps_groupname_marker` 对称：
    /// 启用侧同样必须保留组内标记文件（groupname/selectedindex/modname），
    /// 不得把它们当作 `DISABLED*` 模组目录去剥前缀或改名。
    #[test]
    fn test_enable_all_keeps_groupname_marker() {
        let tmp = TempDir::new().unwrap();
        let group = tmp.path().join("group_1");
        fs::create_dir_all(&group).unwrap();

        // 标记文件 + 已禁用模组并存（真实组目录形态）
        fs::write(group.join("groupname"), "My Custom Group\n").unwrap();
        fs::write(group.join("selectedindex"), "1\n").unwrap();
        create_mod_dir(&group, "DISABLEDModA");

        let count = enable_all_mods_in_group(&group).unwrap().len() as u32;
        assert_eq!(count, 1, "仅 DISABLEDModA 应被启用");

        assert!(
            fs::read_to_string(group.join("groupname")).unwrap() == "My Custom Group\n",
            "groupname 标记文件内容应原样保留"
        );
        assert!(
            group.join("selectedindex").exists(),
            "selectedindex 标记文件应原样保留"
        );
        assert!(
            !group.join("DISABLEDgroupname").exists(),
            "不得把 groupname 标记文件重命名为 DISABLEDgroupname"
        );
        assert!(group.join("ModA").exists(), "ModA 应被启用");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_mod(parent: &Path, name: &str) -> PathBuf {
        let mod_path = parent.join(name);
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "").unwrap();
        mod_path
    }

    #[test]
    fn test_toggle_mod_enable_disable() {
        let dir = TempDir::new().unwrap();
        let enabled_path = dir.path().join("MyMod");
        fs::create_dir_all(&enabled_path).unwrap();

        toggle_mod(&enabled_path, false).unwrap();
        let disabled_name = enabled_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let new_path = dir.path().join(format!("DISABLED{}", disabled_name));
        assert!(new_path.exists());
        assert!(!enabled_path.exists());

        toggle_mod(&new_path, true).unwrap();
        assert!(enabled_path.exists());
        assert!(!new_path.exists());
    }

    #[test]
    fn test_toggle_mod_already_in_state() {
        let dir = TempDir::new().unwrap();

        let enabled_path = dir.path().join("EnabledMod");
        fs::create_dir_all(&enabled_path).unwrap();
        toggle_mod(&enabled_path, true).unwrap();
        assert!(enabled_path.exists());

        let disabled_path = dir.path().join("DISABLEDDisabledMod");
        fs::create_dir_all(&disabled_path).unwrap();
        toggle_mod(&disabled_path, false).unwrap();
        assert!(disabled_path.exists());
    }

    #[test]
    fn test_enable_mutex_mod_disables_siblings() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("MutexGroup");
        fs::create_dir_all(&parent).unwrap();

        let mod1 = create_test_mod(&parent, "DISABLEDModA");
        let mod2 = create_test_mod(&parent, "ModB");
        let mod3 = create_test_mod(&parent, "ModC");

        let target_mod = parent.join("ModA");
        enable_mutex_mod(&mod1).unwrap();

        assert!(target_mod.exists());
        assert!(!mod1.exists());
        assert!(!mod2.exists());
        assert!(parent.join("DISABLEDModB").exists());
        assert!(!mod3.exists());
        assert!(parent.join("DISABLEDModC").exists());
    }

    #[test]
    fn test_enable_mutex_idempotent() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("MutexGroup");
        fs::create_dir_all(&parent).unwrap();

        let mod1 = create_test_mod(&parent, "ModA");
        let _mod2 = create_test_mod(&parent, "DISABLEDModB");
        let _mod3 = create_test_mod(&parent, "DISABLEDModC");

        enable_mutex_mod(&mod1).unwrap();

        assert!(mod1.exists());
        assert!(parent.join("DISABLEDModB").exists());
        assert!(parent.join("DISABLEDModC").exists());

        enable_mutex_mod(&mod1).unwrap();

        assert!(mod1.exists());
        assert!(parent.join("DISABLEDModB").exists());
        assert!(parent.join("DISABLEDModC").exists());
    }

    #[test]
    fn test_disable_mutex_mod() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("MutexGroup");
        fs::create_dir_all(&parent).unwrap();

        let mod_path = create_test_mod(&parent, "MyMod");
        assert!(mod_path.exists());

        disable_mutex_mod(&mod_path).unwrap();

        assert!(!mod_path.exists());
        assert!(parent.join("DISABLEDMyMod").exists());
    }

    #[test]
    fn test_disable_mutex_idempotent() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("MutexGroup");
        fs::create_dir_all(&parent).unwrap();

        let disabled_path = create_test_mod(&parent, "DISABLEDMyMod");
        assert!(disabled_path.exists());

        disable_mutex_mod(&disabled_path).unwrap();

        assert!(disabled_path.exists());
    }

    #[test]
    fn test_mutex_does_not_affect_subgroups() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("MutexGroup");
        fs::create_dir_all(&parent).unwrap();

        let mod1 = create_test_mod(&parent, "ModA");
        let mod2 = create_test_mod(&parent, "ModB");

        let subgroup = parent.join("SubGroup");
        fs::create_dir_all(&subgroup).unwrap();
        let submod = create_test_mod(&subgroup, "SubMod1");
        let submod2 = create_test_mod(&subgroup, "SubMod2");

        enable_mutex_mod(&mod1).unwrap();

        assert!(mod1.exists());
        assert!(!mod2.exists());
        assert!(parent.join("DISABLEDModB").exists());
        assert!(submod.exists());
        assert!(submod2.exists());
    }

    #[test]
    fn test_is_mutex_mod_detection() {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();

        let normal_group = managed.join("group_1");
        fs::create_dir_all(&normal_group).unwrap();
        let normal_mod = create_test_mod(&normal_group, "NormalMod");

        let mutex_group = managed.join("#MutexMods");
        fs::create_dir_all(&mutex_group).unwrap();
        let mutex_mod = create_test_mod(&mutex_group, "MutexMod");

        assert!(!is_mutex_mod(&normal_mod, &managed));
        assert!(is_mutex_mod(&mutex_mod, &managed));
    }

    /// 测试：toggle_mod 目标为 `DISABLEDDISABLEDMod` 时启用应一次移除全部前缀 → `Mod`
    #[test]
    fn test_toggle_mod_enable_removes_all_prefixes() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path();
        let mod_path = parent.join("DISABLEDDISABLEDMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[ShaderOverride]").unwrap();

        toggle_mod(&mod_path, true).unwrap();

        assert!(!mod_path.exists(), "原 DISABLEDDISABLEDMod 目录应被重命名");
        assert!(
            parent.join("Mod").exists(),
            "启用后应为 Mod（一次移除全部连续前缀）"
        );
    }

    /// 测试：toggle_mod 对已禁用目录重复禁用不重复加前缀（幂等）
    #[test]
    fn test_toggle_mod_disable_idempotent_no_extra_prefix() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path();
        let mod_path = parent.join("DISABLEDMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[ShaderOverride]").unwrap();

        // 已禁用 → 应幂等跳过，不新增前缀
        toggle_mod(&mod_path, false).unwrap();

        let entries: Vec<String> = fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            entries.iter().any(|n| n == "DISABLEDMod"),
            "目录仍应为 DISABLEDMod"
        );
        assert!(
            entries.iter().all(|n| !n.starts_with("DISABLEDDISABLED")),
            "不应重复加前缀产生 DISABLEDDISABLEDxxx"
        );
    }

    /// 测试：enable_mutex_mod 目标匹配大小写不敏感，目标正确启用、兄弟正确禁用
    ///
    /// Windows 下路径大小写不敏感，用大小写差异的 mod_path 调用时目标应被识别并启用，
    /// 而非被误判为"兄弟"禁用（缺陷 B 回归测试）。
    #[test]
    fn test_enable_mutex_mod_case_insensitive_target_match() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path();
        // 目标 ModA、兄弟 ModB（均含 .ini）
        let mod_a = parent.join("ModA");
        fs::create_dir_all(&mod_a).unwrap();
        fs::write(mod_a.join("mod.ini"), "[ShaderOverride]").unwrap();
        let mod_b = parent.join("ModB");
        fs::create_dir_all(&mod_b).unwrap();
        fs::write(mod_b.join("mod.ini"), "[ShaderOverride]").unwrap();

        // 以大小写差异的路径调用（Windows 下视为同一目录）
        let target = parent.join("moda");
        enable_mutex_mod(&target).unwrap();

        assert!(
            parent.join("ModA").exists(),
            "目标 ModA 应被识别并保持启用（未被改名）"
        );
        assert!(
            parent.join("DISABLEDModB").exists(),
            "兄弟 ModB 应被禁用（加 DISABLED 前缀）"
        );
    }

    /// 测试：rename_mod 互斥分支 final_name 前缀防重复
    ///
    /// 复刻 mod_commands::rename_mod 的 core 逻辑（validate + spawn_blocking 为 Tauri 命令
    /// 封装不便直接调用），验证 new_name 带 DISABLED 前缀时最终只保有一个前缀。
    #[test]
    fn test_rename_mod_final_name_no_duplicate_prefix() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path();
        let mod_path = parent.join("DISABLEDMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("mod.ini"), "[ShaderOverride]").unwrap();

        // 危险场景：用户重命名 new_name 已带 DISABLED 前缀
        let is_disabled =
            mod_scanner::is_disabled_dir(&mod_path.file_name().unwrap().to_string_lossy());
        let new_name = "DISABLEDNewName".to_string();
        let final_name = if is_disabled {
            format!(
                "{}{}",
                constants::DISABLED_PREFIX,
                mod_scanner::strip_disabled_prefix(&new_name)
            )
        } else {
            new_name
        };

        assert_eq!(
            final_name, "DISABLEDNewName",
            "重命名结果应只含一个 DISABLED 前缀，而非 DISABLEDDISABLEDNewName"
        );
    }
}