//! 模组路径工具域：路径等价/规范化匹配、Mods 根定位、可用文件夹名生成
//!
//! 从 mod_manager.rs 拆出（2026-09-19 Phase 4-1）。
//! 零领域依赖（仅 anyhow/Path）；被选择域（resolve_mod_index_by_path_from_cache）
//! 与删除域（locate_mods_root/get_available_folder_name）消费。

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::core::constants;


/// 路径比较（Windows 大小写不敏感，其他平台敏感）
///
/// 用于 `enable_mutex_mod` 目标模组匹配，避免 Windows 下路径大小写/规范化
/// 不一致时目标被误判为"兄弟"而禁用。
///
/// # 参数
/// - `a`: 待比较路径
/// - `b`: 待比较路径
///
/// # 返回
/// Windows 下按小写比对，其他平台直接比较
pub(crate) fn path_eq_ignore_case(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// 路径比较归一化：统一分隔符、去尾部斜杠、剥离 Windows `\\?\` verbatim 前缀
///
/// Windows 额外小写化（文件系统大小写不敏感）。用于 [`mod_path_matches`]，
/// 消除前端（`SelectModArgs::mod_path`）与后端（`ModData::full_path`）两侧
/// 路径字符串在分隔符、大小写、UNC/verbatim 前缀上的差异。
pub(crate) fn normalize_path_key(p: &str) -> String {
    let mut s = p.replace('\\', "/");
    while s.ends_with('/') {
        s.pop();
    }
    // `\\?\C:\x` → `//?/C:/x` → `C:/x`，避免与同路径的非 verbatim 形式失配
    if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    #[cfg(windows)]
    {
        s.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        s
    }
}

/// 模组路径匹配（P3 修复用）：先精确匹配，再按「忽略 DISABLED 前缀」匹配
///
/// 两侧路径均取自模组目录的磁盘绝对路径，正常情况应字符串相等。但选择请求发出到
/// 后端扫描之间存在时间窗，若期间该模组被启停（目录名增删 `DISABLED` 前缀），
/// 纯字符串比较会失配。故精确匹配失败后降级为「父目录一致 + 末段名去掉 DISABLED
/// 前缀后一致」，仍能定位到同一个模组。
///
/// # 参数
/// - `disk_path`: 后端扫描得到的模组绝对路径（`ModData::full_path`）
/// - `want_path`: 前端传入的目标模组绝对路径
pub(crate) fn mod_path_matches(disk_path: &str, want_path: &str) -> bool {
    if disk_path.is_empty() || want_path.is_empty() {
        return false;
    }
    let d = normalize_path_key(disk_path);
    let w = normalize_path_key(want_path);
    if d == w {
        return true;
    }
    // 降级：父目录必须一致（避免跨组/跨目录误配），末段名忽略 DISABLED 前缀后比较
    let (d_parent, d_name) = match d.rsplit_once('/') {
        Some(v) => v,
        None => return false,
    };
    let (w_parent, w_name) = match w.rsplit_once('/') {
        Some(v) => v,
        None => return false,
    };
    if d_parent != w_parent {
        return false;
    }
    let d_name = crate::core::mod_scan_util::strip_disabled_prefix(d_name);
    let w_name = crate::core::mod_scan_util::strip_disabled_prefix(w_name);
    !d_name.is_empty() && d_name == w_name
}

/// 向上查找 Mods 根目录（_MANAGED_ 的父目录）
///
/// 从 `start_path` 开始逐级向上，定位路径中第一个名为 `_MANAGED_` 的目录，
/// 并返回其父目录作为 Mods 根目录。
/// 这样 `DISABLED_MANAGED_REMOVED` 一定与 `_MANAGED_` 同级（即 Mods 根目录），
/// 不会误创建到 `Mods/_MANAGED_/` 之下。
///
/// 若路径中不含 `_MANAGED_` 组件，则兜底向上查找「包含 `_MANAGED_` 子目录」的祖先；
/// 仍找不到时返回 `start_path` 的父目录作为最后兜底。
pub(crate) fn locate_mods_root(start_path: &Path) -> Result<PathBuf> {
    // 确定性定位：找到路径中第一个名为 _MANAGED_ 的目录，取父目录
    let mut current = Some(start_path);
    for _ in 0..8 {
        let dir = match current {
            Some(d) => d,
            None => break,
        };
        if dir.file_name() == Some(constants::MANAGED_FOLDER.as_ref()) {
            if let Some(parent) = dir.parent() {
                return Ok(parent.to_path_buf());
            }
        }
        current = dir.parent();
    }

    // 兜底：向上查找包含 _MANAGED_ 子目录的祖先（兼容旧路径结构）
    let mut ancestor = start_path.parent();
    for _ in 0..8 {
        if let Some(dir) = ancestor {
            if dir.join(constants::MANAGED_FOLDER).exists() {
                return Ok(dir.to_path_buf());
            }
            ancestor = dir.parent();
        } else {
            break;
        }
    }

    // 最终兜底：使用 start_path 的父目录
    start_path
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Cannot locate Mods root from {:?}", start_path))
}

/// 构造可用文件夹名（冲突时追加 _1、_2…）
///
/// 对齐 NRMM `_getAvailableFolderName`：在 `base_dir` 下查找不冲突的文件夹名。
pub(crate) fn get_available_folder_name(base_name: &str, base_dir: &Path) -> PathBuf {
    let target = base_dir.join(base_name);
    if !target.exists() {
        return target;
    }

    let mut i = 1u32;
    loop {
        let resolved = base_dir.join(format!("{}_{}", base_name, i));
        if !resolved.exists() {
            return resolved;
        }
        i += 1;
    }
}
