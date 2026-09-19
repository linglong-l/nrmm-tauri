//! 模组删除域：remove_mod / remove_group_ex / 移至移除目录
//!
//! 从 mod_manager.rs 拆出（2026-09-19 Phase 4-8）。
//! 依赖 mod_scanner、mod_path_utils（locate_mods_root/get_available_folder_name）、
//! mod_restore（restore_managed_mod 用于还原）、constants、mod_results。

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::constants;
use crate::core::mod_path_utils::{get_available_folder_name, locate_mods_root};
use crate::core::mod_restore::restore_managed_mod;
use crate::core::mod_results::RemoveModResult;

use crate::models::mod_data::GroupDelta;

/// 移除模组（NRMM 对齐：移至 DISABLED_MANAGED_REMOVED + 还原 INI）
///
/// 严格复刻 NRMM 的 `renameOrMoveFolder` + `restoreManagedMod` 流程：
/// 1. 定位 `Mods/DISABLED_MANAGED_REMOVED` 目录（与 `_MANAGED_` 同级）
/// 2. 若不存在则创建
/// 3. 构造目标路径，名称冲突时追加 `_1`、`_2`…（对齐 NRMM `_getAvailableFolderName`）
/// 4. `fs::rename` 移动模组文件夹到目标路径
/// 5. 对移动后的模组调用 `restore_managed_mod` 清理所有 NRMM 注入内容
///
/// # 参数
/// - `mod_path`: 要移除的模组文件夹路径（如 `Mods/_MANAGED_/group_1/MyMod`）
///
/// # 返回值
/// 返回 `RemoveModResult`，包含移动后路径、INI 还原状态等
///
/// # 错误
/// - 模组路径不存在
/// - 无法定位 Mods 根目录
/// - 创建 DISABLED_MANAGED_REMOVED 目录失败
/// - 移动文件夹失败（如跨卷）
pub fn remove_mod(mod_path: &Path) -> Result<RemoveModResult> {
    if !mod_path.exists() {
        return Err(anyhow::anyhow!("Mod path does not exist: {:?}", mod_path));
    }
    if !mod_path.is_dir() {
        return Err(anyhow::anyhow!(
            "Mod path is not a directory: {:?}",
            mod_path
        ));
    }

    let mod_name = mod_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // ========== 定位 Mods 根目录 ==========
    // mod_path 结构：Mods/_MANAGED_/group_X/MyMod 或 Mods/_MANAGED_/#MutexGroup/MyMod
    // 需向上查找直到找到 _MANAGED_ 的父目录（即 Mods 根目录）
    let mods_root = locate_mods_root(mod_path)?;

    // ========== 定位 DISABLED_MANAGED_REMOVED 目录 ==========
    let removed_folder = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
    if !removed_folder.exists() {
        fs::create_dir_all(&removed_folder).with_context(|| {
            format!(
                "Failed to create {}: {:?}",
                constants::MANAGED_REMOVED_FOLDER,
                removed_folder
            )
        })?;
    }

    // ========== 构造目标路径（冲突追加 _1、_2…） ==========
    let target = get_available_folder_name(&mod_name, &removed_folder);

    // ========== 移动模组文件夹 ==========
    fs::rename(mod_path, &target)
        .with_context(|| format!("Failed to move mod {:?} to {:?}", mod_path, target))?;

    // ========== 还原 INI ==========
    let (ini_count, failed_count) = restore_managed_mod(&target);

    log::info!(
        "[remove_mod] Moved '{}' to {:?}, INI restored: {} total, {} failed",
        mod_name,
        target,
        ini_count,
        failed_count
    );

    Ok(RemoveModResult {
        mod_name,
        moved_to: target,
        restored: failed_count == 0,
        ini_count,
        failed_count,
        // 占位：命令层局部重扫受影响分组后覆盖为真实 delta
        group_delta: GroupDelta::default(),
        degraded: false,
    })
}

/// 底层移动：将目录移动到目标删除目录（仅负责移动，不做任何业务判断）
///
/// 单一职责：仅接受「源目录」与「目标删除目录」，执行目录移动。
/// - 若目标删除目录不存在，先创建
/// - 目标名冲突时追加 `_1`、`_2`… 后缀
/// - 不关心源目录从何而来、为何移动，也不负责定位 Mods 根目录
///
/// # 参数
/// - `src`: 待移动的源目录路径
/// - `removed_dir`: 目标删除目录（模组/分组将被移动到此目录下）
///
/// # 返回值
/// 返回移动后的实际路径（冲突时含后缀）
///
/// # 错误
/// - 创建目标删除目录失败
/// - 目录移动失败（如跨卷、权限不足）
pub fn move_dir_to_removed(src: &Path, removed_dir: &Path) -> Result<PathBuf> {
    if !removed_dir.exists() {
        fs::create_dir_all(removed_dir)
            .with_context(|| format!("Failed to create removed dir: {:?}", removed_dir))?;
    }

    let name = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let target = get_available_folder_name(&name, removed_dir);

    fs::rename(src, &target)
        .with_context(|| format!("Failed to move {:?} to {:?}", src, target))?;

    Ok(target)
}

/// 移除分组（NRMM 对齐：移至 DISABLED_MANAGED_REMOVED 目录，非group先移子分组再移除）
///
/// 本函数为「编排层」，仅决定哪些目录移动到哪里，具体移动操作委托给底层 `move_dir_to_removed`。
/// Mods 根目录由调用方（顶层命令）从设置数据传入，本函数不做路径推导。
///
/// # 参数
/// - `group_path`: 要移除的分组目录路径
/// - `is_group_xx`: true = group_1/group_2 等普通分组，false = mutexGroup 非group目录
/// - `mods_root`: Mods 根目录（与 `_MANAGED_` 同级），由调用方从设置数据获取
///
/// # 对于 group_xx：
/// 1. 将整个分组目录移至 `mods_root/DISABLED_MANAGED_REMOVED/原名`
///    - 名称冲突追加 `_1`、`_2`…
///
/// # 对于非group：
/// 1. 先将分组下的**一级子目录且不含 .ini 的目录**（即子分组目录）移至 `mods_root`
///    - 子分组目录重名时追加 `_1`、`_2`… 后缀
/// 2. 然后将被清空的分组目录移至 `mods_root/DISABLED_MANAGED_REMOVED/`
///
/// # 错误
/// 路径不存在或目录移动失败时返回错误
pub fn remove_group_ex(group_path: &Path, is_group_xx: bool, mods_root: &Path) -> Result<()> {
    if !group_path.exists() {
        return Err(anyhow::anyhow!(
            "Group path does not exist: {:?}",
            group_path
        ));
    }
    if !group_path.is_dir() {
        return Err(anyhow::anyhow!(
            "Group path is not a directory: {:?}",
            group_path
        ));
    }

    // ========== 非group：先将一级子分组（无 .ini 的子目录）移至 mods_root ==========
    if !is_group_xx {
        // 收集所有一级子分组（子目录且不含 .ini 的目录）
        let entries = match fs::read_dir(group_path) {
            Ok(e) => e,
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Failed to read group directory: {:?}",
                    group_path
                ))
            }
        };

        // 先收集再处理（避免在迭代期间修改目录）
        let mut to_move: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let child_path = entry.path();
            if !child_path.is_dir() {
                continue;
            }
            let child_name = entry.file_name().to_string_lossy().to_string();
            if child_name.starts_with('.') {
                continue;
            }
            // 含 .ini → 叶子模组节点，不是子分组，跳过（会随父目录一起被移除）
            let has_ini = match crate::core::mod_scan_util::dir_has_ini_file(&child_path) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if has_ini {
                continue;
            }
            // 不含 .ini 的目录 → 子分组目录，需移至 mods_root
            to_move.push(child_path);
        }

        for src in to_move {
            move_dir_to_removed(&src, mods_root)?;
        }
    }

    // ========== 将分组目录移至 DISABLED_MANAGED_REMOVED ==========
    let removed_folder = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
    move_dir_to_removed(group_path, &removed_folder)?;

    Ok(())
}

#[cfg(test)]
mod tests_remove_group_ex {
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

    /// 底层移动：正常移动目录到目标删除目录
    #[test]
    fn test_move_dir_to_removed_basic() {
        let tmp = TempDir::new().unwrap();
        let removed_dir = tmp.path().join("removed");
        let src = tmp.path().join("MyItem");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), "x").unwrap();

        let moved = move_dir_to_removed(&src, &removed_dir).unwrap();

        assert!(!src.exists(), "源目录应被移除");
        assert!(removed_dir.exists(), "目标删除目录应被创建");
        assert_eq!(moved, removed_dir.join("MyItem"));
        assert!(moved.join("a.txt").exists(), "内容应随目录移动");
    }

    /// 底层移动：目标名冲突时追加 _1 后缀
    #[test]
    fn test_move_dir_to_removed_conflict() {
        let tmp = TempDir::new().unwrap();
        let removed_dir = tmp.path().join("removed");
        fs::create_dir_all(&removed_dir).unwrap();
        fs::create_dir_all(removed_dir.join("MyItem")).unwrap();

        let src = tmp.path().join("MyItem");
        fs::create_dir_all(&src).unwrap();

        let moved = move_dir_to_removed(&src, &removed_dir).unwrap();

        assert_eq!(moved, removed_dir.join("MyItem_1"), "冲突应追加 _1 后缀");
    }

    /// group_xx 场景：移至 DISABLED_MANAGED_REMOVED 目录
    #[test]
    fn test_remove_group_xx() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let group = managed.join("group_1");
        fs::create_dir_all(&group).unwrap();
        create_mod_dir(&group, "ModA");

        remove_group_ex(&group, true, mods_root).unwrap();

        // 原分组应不存在
        assert!(!group.exists(), "原 group_1 应被移除");
        // DISABLED_MANAGED_REMOVED 下应有 group_1
        let removed_root = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
        assert!(removed_root.exists());
        let removed_group = removed_root.join("group_1");
        assert!(
            removed_group.exists(),
            "DISABLED_MANAGED_REMOVED 下应存在 group_1"
        );
        assert!(removed_group.join("ModA").exists(), "模组应随之移动");
    }

    /// 非group场景：先移子分组到父级，再移至 DISABLED_MANAGED_REMOVED
    #[test]
    fn test_remove_group_non_group_with_subgroups() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let non_group = mods_root.join("#MyMutexGroup");
        fs::create_dir_all(&non_group).unwrap();
        // 叶子模组（随父目录移除）
        create_mod_dir(&non_group, "ModA");
        // 子分组（无 .ini，应移至父级 mods_root）
        let subgroup1 = non_group.join("SubGroup1");
        fs::create_dir_all(&subgroup1).unwrap();
        create_mod_dir(&subgroup1, "SubMod1");
        let subgroup2 = non_group.join("SubGroup2");
        fs::create_dir_all(&subgroup2).unwrap();

        remove_group_ex(&non_group, false, mods_root).unwrap();

        // 原分组应不存在
        assert!(!non_group.exists());
        // 子分组应在 mods_root 下
        assert!(
            mods_root.join("SubGroup1").exists(),
            "SubGroup1 应移至 Mods 根目录"
        );
        assert!(
            mods_root.join("SubGroup1").join("SubMod1").exists(),
            "SubGroup1 下的模组应保留"
        );
        assert!(
            mods_root.join("SubGroup2").exists(),
            "SubGroup2 应移至 Mods 根目录"
        );
        // 被移除目录在 DISABLED_MANAGED_REMOVED 下
        let removed = mods_root
            .join(constants::MANAGED_REMOVED_FOLDER)
            .join("#MyMutexGroup");
        assert!(removed.exists());
        assert!(removed.join("ModA").exists(), "ModA 随原分组被移除");
        assert!(
            !removed.join("SubGroup1").exists(),
            "SubGroup1 不应随原分组被移除"
        );
    }

    /// 名称冲突场景：DISABLED_MANAGED_REMOVED/group_1 已存在应追加 _1
    #[test]
    fn test_remove_group_conflict() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let group = managed.join("group_1");
        let removed_root = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
        fs::create_dir_all(&group).unwrap();
        // 已存在的已删除组
        fs::create_dir_all(removed_root.join("group_1")).unwrap();

        remove_group_ex(&group, true, mods_root).unwrap();

        assert!(!group.exists());
        assert!(removed_root.join("group_1").exists(), "原冲突项保留");
        assert!(
            removed_root.join("group_1_1").exists(),
            "新移除项追加 _1 后缀"
        );
    }
}

#[cfg(test)]
mod tests_remove_mod {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// locate_mods_root：模组位于 _MANAGED_/group_X 下时，应返回 _MANAGED_ 的父目录（Mods 根目录）
    #[test]
    fn test_locate_mods_root_from_managed_group() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let group = managed.join("group_1");
        let mod_path = group.join("MyMod");
        fs::create_dir_all(&mod_path).unwrap();

        let root = locate_mods_root(&mod_path).unwrap();
        assert_eq!(root, mods_root, "应返回 Mods 根目录（_MANAGED_ 的父目录）");
    }

    /// locate_mods_root：模组位于 _MANAGED_/#MutexGroup 下时，同样返回 Mods 根目录
    #[test]
    fn test_locate_mods_root_from_mutex_group() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let mutex_group = managed.join("#MutexGroup");
        let mod_path = mutex_group.join("MyMod");
        fs::create_dir_all(&mod_path).unwrap();

        let root = locate_mods_root(&mod_path).unwrap();
        assert_eq!(root, mods_root);
    }

    /// remove_mod：移除的模组必须位于 Mods/DISABLED_MANAGED_REMOVED 下，
    /// 而非 Mods/_MANAGED_/DISABLED_MANAGED_REMOVED 下（用户报告的关键场景）
    #[test]
    fn test_remove_mod_moved_to_mods_root_removed_folder() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let group = managed.join("group_1");
        let mod_path = group.join("MyMod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join("MyMod.ini"), "[ShaderOverride]").unwrap();

        let result = remove_mod(&mod_path).unwrap();

        // 原模组路径应不存在
        assert!(!mod_path.exists(), "原模组路径应被移除");

        // 目标必须位于 Mods/DISABLED_MANAGED_REMOVED（与 _MANAGED_ 同级）
        let expected = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
        assert!(
            expected.exists(),
            "DISABLED_MANAGED_REMOVED 应创建在 Mods 根目录"
        );
        assert_eq!(result.moved_to.parent(), Some(expected.as_path()));

        // 明确验证：不得创建在 Mods/_MANAGED_ 之下
        let wrongly_placed = managed.join(constants::MANAGED_REMOVED_FOLDER);
        assert!(
            !wrongly_placed.join("MyMod").exists(),
            "不得创建到 _MANAGED_ 之下"
        );
        assert!(
            result.moved_to.join("MyMod.ini").exists(),
            "模组文件应随目录移动"
        );
    }

    /// remove_mod：删除多个模组时，DISABLED_MANAGED_REMOVED 始终位于 Mods 根目录，冲突追加 _1
    #[test]
    fn test_remove_mod_conflict_appends_suffix() {
        let tmp = TempDir::new().unwrap();
        let mods_root = tmp.path();
        let managed = mods_root.join("_MANAGED_");
        let group = managed.join("group_1");
        let mod1 = group.join("MyMod");
        let mod2 = group.join("MyMod2");
        fs::create_dir_all(&mod1).unwrap();
        fs::create_dir_all(&mod2).unwrap();
        fs::write(mod1.join("MyMod.ini"), "[ShaderOverride]").unwrap();
        fs::write(mod2.join("MyMod2.ini"), "[ShaderOverride]").unwrap();

        let r1 = remove_mod(&mod1).unwrap();
        let r2 = remove_mod(&mod2).unwrap();

        let removed_root = mods_root.join(constants::MANAGED_REMOVED_FOLDER);
        assert_eq!(r1.moved_to, removed_root.join("MyMod"));
        assert_eq!(r2.moved_to, removed_root.join("MyMod2"));
        assert!(
            !managed.join(constants::MANAGED_REMOVED_FOLDER).exists(),
            "不得创建到 _MANAGED_ 之下"
        );
    }
}
