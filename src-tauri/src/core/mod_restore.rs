//! INI 恢复域：restore_all_inis / restore_managed_mod 及恢复底层
//!
//! 从 mod_manager.rs 拆出（2026-09-19 Phase 4-7）。
//! 依赖 ini 编解码、constants、mod_results（RestoredCount）；被命令层与
//! mod_remove（restore_managed_mod）消费。

use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::constants;
use crate::core::mod_results::RestoredCount;

/// 从备份恢复所有 INI 文件
///
/// 恢复顺序：
/// 1. 主 INI 文件（d3dx.ini, RatioShot.ini）
/// 2. 递归恢复 _MANAGED_ 目录下所有模组的 INI
/// 3. 清理 NRMM 生成的管理文件（nrmm_include.ini, manager_group.ini, nrmm_keypress.txt, group_*.ini）
///
/// 恢复成功后删除备份文件。
///
/// # 参数
/// - `game_mods_path`: 游戏 Mods 目录
///
/// # 返回
/// 恢复统计（成功数/失败数）
pub fn restore_all_inis(game_mods_path: &Path) -> Result<RestoredCount> {
    // 还原安全窗口：随后 watcher 防抖触发的重扫对管理标记只读不建
    // （还原后的目录通常会被删除/导出，专属配置文件会干扰其他管理器）
    crate::core::mod_scanner::mark_restore_safe_window();
    let mut restored = 0u32;
    let mut failed = 0u32;

    // d3dx.ini 位于游戏根（Mods 父目录），不在 Mods/ 内
    let game_root = game_mods_path.parent().unwrap_or(game_mods_path);
    let ini_path = game_root.join("d3dx.ini");
    let backup_path = ini_path.with_extension(constants::BACKUP_EXTENSION);
    if backup_path.exists() {
        match fs::copy(&backup_path, &ini_path) {
            Ok(_) => {
                restored += 1;
                fs::remove_file(&backup_path).ok();
            }
            Err(e) => {
                log::error!("Failed to restore main INI {}: {}", ini_path.display(), e);
                failed += 1;
            }
        }
    }

    let managed_folder = game_mods_path.join(constants::MANAGED_FOLDER);
    if managed_folder.exists() {
        restore_inis_recursive(&managed_folder, &mut restored, &mut failed)?;

        // 清理 NRMM 生成的管理文件
        let _ = fs::remove_file(managed_folder.join(constants::INCLUDE_FILENAME));
        let _ = fs::remove_file(managed_folder.join(constants::KEYPRESS_FILENAME));
        let _ = fs::remove_file(managed_folder.join("manager_group.ini"));

        // 清理 group_*.ini 文件
        if let Ok(entries) = fs::read_dir(&managed_folder) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if let Some(rest) = name.strip_prefix("group_") {
                            if rest.ends_with(".ini") {
                                let _ = fs::remove_file(&path);
                            }
                        }
                    }
                }
                // 清理子目录下的 group_X.ini 和 ModFolder.ini
                // 用 file_type().is_dir() 避免 junction 逃逸（R3）
                if path.is_dir() {
                    let is_real_dir = path
                        .symlink_metadata()
                        .map(|m| m.is_dir() && !m.file_type().is_symlink())
                        .unwrap_or(false);
                    if !is_real_dir {
                        continue;
                    }
                    if let Ok(sub_entries) = fs::read_dir(&path) {
                        for sub_entry in sub_entries.flatten() {
                            let sub_path = sub_entry.path();
                            if sub_path.is_file() {
                                if let Some(name) = sub_path.file_name().and_then(|n| n.to_str()) {
                                    if name.starts_with("group_") && name.ends_with(".ini") {
                                        let _ = fs::remove_file(&sub_path);
                                    }
                                    if name == "ModFolder.ini" {
                                        let _ = fs::remove_file(&sub_path);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(RestoredCount { restored, failed })
}

/// 还原目录树内全部 INI（显式栈 DFS + 路径池，防爆栈/防 symlink 循环）
///
/// 原实现为原生递归 + `entry.file_type()`（不跟随 symlink/junction，防逃逸）。
/// 改造后保持「不跟随 symlink」语义（真实目录才递归下降），改用显式栈 + visited 路径池
/// 避免深目录爆栈与重复遍历；非法 UTF-8 名称条目跳过并记录日志。
///
/// # 参数
/// - `dir`: 遍历根目录（`_MANAGED_` 目录）
/// - `restored`: 成功还原计数（累加）
/// - `failed`: 失败计数（累加）
///
/// # Errors
/// `read_dir` 失败时传播 IO 错误（与递归版一致）
fn restore_inis_recursive(dir: &Path, restored: &mut u32, failed: &mut u32) -> Result<()> {
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    // 路径池（visited）：仅真实目录入栈，symlink 不解析（保持原「不跟随」语义，防 junction 逃逸）
    let mut pool = crate::core::fs_walk::PathPool::new();
    pool.try_visit(dir);
    // 非法 UTF-8 名称计数（restore 场景仅日志汇总，不上报前端）
    let stats = crate::core::fs_walk::TraversalStats::default();

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            // UTF-8 合法化：非法名称条目跳过（结构性路径）
            if crate::core::fs_walk::valid_entry_name(
                &entry,
                &dir.to_string_lossy(),
                crate::core::fs_walk::kind::FILE,
                &stats,
            )
            .is_none()
            {
                continue;
            }
            // 用 entry.file_type()（不跟随 symlink/junction）判断是否为真实目录，
            // 避免 junction 逃逸到 Mods 根外（R3）
            if ft.is_dir() {
                // 真实目录：路径池判重后入栈（symlink/junction 为 false，天然不跟随）
                if pool.try_visit(&path) {
                    stack.push(path);
                }
            } else if ft.is_symlink() {
                log::warn!(
                    "restore_inis_recursive: skip symlink/junction (not followed): {:?}",
                    path
                );
            } else if let Some(ext) = path.extension() {
                if ext.eq_ignore_ascii_case("ini") && !constants::is_desktop_ini(&path) {
                    let backup_path = path.with_extension(constants::BACKUP_EXTENSION);
                    if backup_path.exists() {
                        match fs::copy(&backup_path, &path) {
                            Ok(_) => {
                                *restored += 1;
                                fs::remove_file(&backup_path).ok();
                            }
                            Err(e) => {
                                log::error!("Failed to restore mod INI {}: {}", path.display(), e);
                                *failed += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    let (skipped, _entries) = stats.into_parts();
    if skipped > 0 {
        log::warn!(
            "[restore_inis_recursive] 跳过 {} 个非法 UTF-8 名称条目",
            skipped
        );
    }
    Ok(())
}

/// 还原模组 INI 到管理前状态（NRMM 对齐：`restoreManagedMod`）
///
/// 严格复刻 NRMM 的 `restoreManagedMod` 逻辑，对指定目录下递归找到的每个 INI 文件执行：
/// 1. 移除 NRMM 管理注释（含 `no reload mod manager`、`";-;" are errored`、
///    `";+;" are disabled keys`、`errored conditional blocks`、
///    `if certain syntax is only available` 关键字的注释行）
/// 2. 移除 `global $managed_slot_id =` 变量声明
/// 3. 净化 `condition=` 表达式（移除 `$managed_slot_id == $\modmanageragl\group_X\...` 段，
///    清理孤立的 `&&`/`||`/空括号等）
/// 4. 移除管理器 `if $managed_slot_id == ... endif` 块（栈匹配 if/endif）
/// 5. 移除 NRMM 管理时添加的前 4 个空格缩进
///
/// # 参数
/// - `mod_dir`: 已移动到回收目录的模组文件夹路径
///
/// # 返回值
/// 返回 `(处理的 INI 文件数, 失败数)`。单个文件失败不会中断整体流程。
pub fn restore_managed_mod(mod_dir: &Path) -> (u32, u32) {
    // 还原安全窗口：随后 watcher 防抖触发的重扫对管理标记只读不建
    // （还原后的目录通常会被删除/导出，专属配置文件会干扰其他管理器）
    crate::core::mod_scanner::mark_restore_safe_window();
    let mut total = 0u32;
    let mut failed = 0u32;

    // 收集待还原的 .ini 文件：
    // - 目录：递归收集其下所有 .ini 文件
    // - 单个文件：仅当为 .ini 文件（且非备份）时纳入，非 .ini 文件直接忽略（pass）
    let ini_files: Vec<PathBuf> = if mod_dir.is_dir() {
        match collect_ini_files_recursive(mod_dir) {
            Ok(files) => files,
            Err(e) => {
                log::error!(
                    "[restore_managed_mod] Failed to collect INI files in {:?}: {}",
                    mod_dir,
                    e
                );
                return (0, 1);
            }
        }
    } else if is_restorable_ini(mod_dir) {
        vec![mod_dir.to_path_buf()]
    } else {
        log::info!("[restore_managed_mod] Skip non-ini file: {:?}", mod_dir);
        Vec::new()
    };

    for ini_path in &ini_files {
        total += 1;
        if let Err(e) = restore_single_ini(ini_path) {
            log::warn!(
                "[restore_managed_mod] Failed to restore {:?}: {}",
                ini_path,
                e
            );
            failed += 1;
        }
    }

    (total, failed)
}

/// 判断路径是否为可还原的 .ini 文件（扩展名为 ini 且非备份文件）
fn is_restorable_ini(path: &Path) -> bool {
    if let Some(ext) = path.extension() {
        if ext.eq_ignore_ascii_case("ini") {
            // 跳过 .ini_managed_backup 备份文件
            return !path
                .to_string_lossy()
                .ends_with(&format!(".{}", constants::BACKUP_EXTENSION));
        }
    }
    false
}

/// 递归收集目录下所有 .ini 文件（对齐 NRMM `_findIniFilesRecursive`）
fn collect_ini_files_recursive(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    collect_ini_files_recursive_inner(dir, &mut result)?;
    Ok(result)
}

fn collect_ini_files_recursive_inner(dir: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // 用 entry.file_type()（不跟随 symlink/junction）判断是否为真实目录，
        // 避免 junction 逃逸到 Mods 根外创建 / 改写 INI（R3）
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_ini_files_recursive_inner(&path, result)?;
        } else if ft.is_symlink() {
            log::warn!(
                "collect_ini_files_recursive_inner: skip symlink/junction (not followed): {:?}",
                path
            );
        } else if is_restorable_ini(&path) {
            result.push(path);
        }
    }
    Ok(())
}

/// 收集模组目录下的直接 .ini 文件（不递归子目录）
///
/// 用于 `detect_hash_conflicts` 中从轻量扫描结果（`ModData.full_path`）查找
/// 模组的 INI 文件。轻量扫描不解析 INI 内容，故需按需查找并解析。
///
/// 过滤规则：
/// - 仅扫描目录下的直接文件，不递归子目录（与深度扫描 `check_directory_for_mod_deep` 一致）
/// - 排除 `desktop.ini` 系统配置文件（NRMM 对齐）
/// - 排除 `.ini_managed_backup` 备份文件
///
/// # 参数
/// - `mod_dir`: 模组目录路径
///
/// # 返回
/// 目录下所有符合条件的 .ini 文件路径列表（无序）
pub(crate) fn collect_mod_ini_files(mod_dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let entries = match fs::read_dir(mod_dir) {
        Ok(e) => e,
        Err(_) => return result,
    };
    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(f) => f,
            Err(_) => continue,
        };
        if !ft.is_file() && !ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        // 排除 desktop.ini 系统配置文件
        if constants::is_desktop_ini(&path) {
            continue;
        }
        if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("ini") {
                // 排除 .ini_managed_backup 备份文件
                if !path
                    .to_string_lossy()
                    .ends_with(&format!(".{}", constants::BACKUP_EXTENSION))
                {
                    result.push(path);
                }
            }
        }
    }
    result
}

/// 还原单个 INI 文件（对齐 NRMM `restoreManagedMod` 内层循环）
///
/// 处理流程严格对齐 NRMM：
/// 1. 读取文件（UTF-8 强制转换）
/// 2. 逐行扫描，标记需删除的行（管理注释、变量声明、管理器 if/endif）
/// 3. 净化 condition= 表达式
/// 4. 移除前 4 个空格缩进
/// 5. 若有修改则写回文件
fn restore_single_ini(ini_path: &Path) -> Result<()> {
    // C6：写回转换前先二进制备份原文件（atomic_copy 字节级保真），已有备份则复用；
    // 再经统一编码管线读取（非 UTF-8 无损转为 UTF-8）。
    let backup_path = ini_path.with_extension(constants::BACKUP_EXTENSION);
    if !backup_path.exists() {
        if let Err(e) = crate::utils::atomic_copy(ini_path, &backup_path) {
            log::warn!(
                "[restore_single_ini] 备份失败 {:?} -> {:?}: {}",
                ini_path,
                backup_path,
                e
            );
        }
    }
    let (content, info) = crate::core::ini_codec::read_ini_safe(ini_path)?;
    let raw_lines: Vec<&str> = content.lines().collect();
    let mut lines: Vec<String> = raw_lines.iter().map(|s| s.to_string()).collect();
    let mut modified = false;

    // if 栈：跟踪 if/endif 配对，用于识别管理器 if 块的 endif
    let mut if_stack: Vec<bool> = Vec::new(); // true = 管理器 if

    for line in lines.iter_mut() {
        let trimmed_lower = line.trim().to_lowercase();

        // 新 section 重置 if 栈
        // 对齐 Dart _restoreManagedMod：section 头行直接 continue，不调用 _removeFirstFourSpaces，
        // 因此 section 头（始终位于列 0）的缩进保持原样、绝不被移除。
        if trimmed_lower.starts_with('[') {
            if_stack.clear();
            continue;
        }

        // 1. 移除 NRMM 管理注释
        if trimmed_lower.starts_with(';')
            && (trimmed_lower.contains("no reload mod manager")
                || trimmed_lower.contains(r#"";-;" are errored"#)
                || trimmed_lower.contains(r#"";+;" are disabled keys"#)
                || trimmed_lower.contains("errored conditional blocks")
                || trimmed_lower.contains("if certain syntax is only available"))
        {
            *line = "-----".to_string(); // 标记删除
            modified = true;
            continue;
        }

        // 2. 移除 `global $managed_slot_id =` 变量声明
        let no_space = trimmed_lower.replace(' ', "");
        if no_space.starts_with("global$managed_slot_id=") {
            *line = "-----".to_string();
            modified = true;
            continue;
        }

        // 3. 净化 condition= 表达式（含 ;-;condition= 和 ;+;condition= 变体）
        let starts_with_condition = no_space.starts_with("condition=");
        let starts_with_special_comment1 = no_space.starts_with(";-;condition=");
        let starts_with_special_comment2 = no_space.starts_with(";+;condition=");

        if starts_with_condition || starts_with_special_comment1 || starts_with_special_comment2 {
            if let Some(equal_idx) = line.find('=') {
                let expression = line[equal_idx + 1..].trim();
                let modified_expr = sanitize_condition_expression_inline(expression);

                if expression != modified_expr {
                    if modified_expr.is_empty() {
                        *line = "-----".to_string();
                    } else if starts_with_special_comment1 {
                        *line = format!(";-;condition = {}", modified_expr);
                    } else if starts_with_special_comment2 {
                        *line = format!(";+;condition = {}", modified_expr);
                    } else {
                        *line = format!("condition = {}", modified_expr);
                    }
                    modified = true;
                    continue;
                }
            }
        }

        // 4. 移除管理器 if 行 + 跟踪 if 栈
        if trimmed_lower.starts_with("if ") {
            let is_manager_if = no_space.contains("if$managed_slot_id==$\\modmanageragl\\group_");
            if_stack.push(is_manager_if);
            if is_manager_if {
                *line = "-----".to_string();
                modified = true;
                continue;
            }
        }

        // 5. 移除与管理器 if 配对的 endif
        if trimmed_lower == "endif" {
            if let Some(is_manager) = if_stack.pop() {
                if is_manager {
                    *line = "-----".to_string();
                    modified = true;
                    continue;
                }
            }
        }

        // 移除前 4 个空格缩进（NRMM 管理时添加的缩进）
        *line = remove_first_four_spaces(line);
    }

    if modified {
        let filtered: Vec<&String> = lines.iter().filter(|s| s.as_str() != "-----").collect();
        let new_content = filtered
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<&str>>()
            .join("\n");
        crate::core::ini_codec::write_utf8_atomic(ini_path, &new_content, info)?;
    }

    Ok(())
}

/// 从行首移除最多 4 个空格（对齐 NRMM `_removeFirstFourSpaces`）
fn remove_first_four_spaces(line: &str) -> String {
    let mut count = 0;
    let mut idx = 0;
    for (i, ch) in line.chars().enumerate() {
        if ch == ' ' && count < 4 {
            count += 1;
            idx = i + 1;
        } else {
            break;
        }
    }
    line[idx..].to_string()
}

/// 净化 condition 表达式中的管理器注入部分（对齐 NRMM `_sanitizeKeyConditionExpressionFromModManager`）
///
/// 复用 `ini_condition::sanitize_condition_expression` 的核心逻辑，通过公开包装调用。
fn sanitize_condition_expression_inline(expression: &str) -> String {
    crate::core::ini_condition::sanitize_condition_expression_public(expression)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mod_update_pipeline::update_mod_data;
    use crate::models::enums::TargetGame;
    use crate::models::settings::AppSettings;
    use tempfile::TempDir;

    /// 还原安全窗口全局状态互斥 + RAII 清除。
    ///
    /// 调用 `restore_all_inis` / `restore_managed_mod`（内部开启 60s 全局还原安全窗口，
    /// `mod_scanner::mark_restore_safe_window`）的测试必须持有 `mod_scanner::TEST_WINDOW_LOCK`，
    /// 并在退出 / panic 时通过本 guard 清除窗口——否则窗口会泄漏到并发执行的
    /// mod_scanner 标记创建断言测试（窗口期内标记不被创建而误报失败）。
    struct RestoreWindowGuard;
    impl Drop for RestoreWindowGuard {
        fn drop(&mut self) {
            crate::core::mod_scanner::test_clear_restore_safe_window();
        }
    }

    fn setup_test_env() -> TempDir {
        let dir = TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        dir
    }

    fn create_group_dir(base: &Path, group_name: &str) -> PathBuf {
        let group_path = base.join("_MANAGED_").join(group_name);
        fs::create_dir_all(&group_path).unwrap();
        group_path
    }

    fn create_mod_with_ini(group_path: &Path, mod_name: &str, ini_content: &str) -> PathBuf {
        let mod_path = group_path.join(mod_name);
        fs::create_dir_all(&mod_path).unwrap();
        let ini_path = mod_path.join("mod.ini");
        fs::write(&ini_path, ini_content).unwrap();
        mod_path
    }

    fn create_main_ini(base: &Path, game: TargetGame) {
        let ini_name = game.d3dx_ini_name();
        // 包含 [Include] 段的 include_recursive，对齐标准 XXMI 环境
        // 使用实际目录名确保 detect_include_recursive 返回 true，不注入 ;NRMM_INI_START 块
        let dir_name = base.file_name().and_then(|n| n.to_str()).unwrap_or("Mods");
        let content = format!(
            "; original main ini\n[Include]\ninclude_recursive = {}\n",
            dir_name
        );
        fs::write(base.join(ini_name), content).unwrap();
    }

    #[test]
    fn test_restore_all_inis() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        // d3dx.ini 在游戏根（Mods 父目录）；_MANAGED_ 在 Mods/ 下
        let dir = tempfile::TempDir::new().unwrap();
        let mods_path = dir.path().join("Mods");
        let managed = mods_path.join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        // 覆盖 include_recursive 值为 "Mods"，使 detect_include_recursive 匹配
        fs::write(
            dir.path().join("d3dx.ini"),
            "; original main ini\n[Include]\ninclude_recursive = Mods\n",
        )
        .unwrap();
        let settings = AppSettings::default();

        let group_path = managed.join("group_1");
        fs::create_dir_all(&group_path).unwrap();
        let _mod_path = {
            let p = group_path.join("RestoreMod");
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("mod.ini"), "[TextureOverrideTest]\nhash = 0x1\n").unwrap();
            p
        };

        update_mod_data(TargetGame::GenshinImpact, &mods_path, &settings).unwrap();

        let main_ini_path = dir.path().join("d3dx.ini");
        let modified_content = fs::read_to_string(&main_ini_path).unwrap();
        // 对齐 NRMM 原版：d3dx.ini 有 include_recursive=Mods 时不注入 ;NRMM_INI_START 块
        assert!(!modified_content.contains(";NRMM_INI_START"));

        let result = restore_all_inis(&mods_path).unwrap();
        assert!(result.restored >= 1);
        assert_eq!(result.failed, 0);

        let restored_content = fs::read_to_string(&main_ini_path).unwrap();
        assert!(!restored_content.contains(";NRMM_INI_START"));

        let backup_path = main_ini_path.with_extension(constants::BACKUP_EXTENSION);
        assert!(!backup_path.exists());

        // 验证管理文件已清理
        assert!(!managed.join("nrmm_include.ini").exists());
    }

    #[test]
    fn test_restore_managed_mod_single_ini_file() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        let dir = setup_test_env();
        // 构造一个含 NRMM 管理注入的单个 .ini 文件
        let ini_path = dir.path().join("mod.ini");
        fs::write(
            &ini_path,
            "[TextureOverrideTest]\nhash = 0x1\n; no reload mod manager\n",
        )
        .unwrap();

        // 单 .ini 文件应被还原
        let (restored, failed) = restore_managed_mod(&ini_path);
        assert_eq!(restored, 1);
        assert_eq!(failed, 0);
        let content = fs::read_to_string(&ini_path).unwrap();
        assert!(!content.contains("no reload mod manager"));
    }

    #[test]
    fn test_restore_managed_mod_skips_non_ini_file() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        let dir = setup_test_env();
        // 非 .ini 文件应被忽略（pass），不报错也不处理
        let txt_path = dir.path().join("readme.txt");
        fs::write(&txt_path, "hello").unwrap();

        let (restored, failed) = restore_managed_mod(&txt_path);
        assert_eq!(restored, 0);
        assert_eq!(failed, 0);
    }

    #[test]
    fn test_restore_managed_mod_directory_recursive() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        let dir = setup_test_env();
        let group_path = create_group_dir(dir.path(), "group_1");
        create_mod_with_ini(
            &group_path,
            "ModA",
            "[TextureOverrideA]\nhash = 0x1\n; no reload mod manager\n",
        );
        create_mod_with_ini(
            &group_path,
            "ModB",
            "[TextureOverrideB]\nhash = 0x2\nglobal $managed_slot_id = 0\n",
        );

        // 目录应递归还原其下所有 .ini 文件
        let (restored, failed) = restore_managed_mod(&group_path);
        assert_eq!(restored, 2);
        assert_eq!(failed, 0);
        let a = fs::read_to_string(group_path.join("ModA").join("mod.ini")).unwrap();
        assert!(!a.contains("no reload mod manager"));
        let b = fs::read_to_string(group_path.join("ModB").join("mod.ini")).unwrap();
        assert!(!b.contains("global $managed_slot_id"));
    }

    // ============================================================================
    // 还原逻辑对齐测试（NRMM restoreManagedMod / restoreAllInis）
    // 验证「备份式还原（Restore All）」无损回放，以及「就地清理式还原（还原区/移除）」
    // 正确清除管理器注入的所有产物（manager if/endif、global $managed_slot_id、
    // condition 槽位段、4 空格缩进、管理器头注释），并保留非管理器内容。
    // ============================================================================

    /// 备份式还原（Restore All）必须是 update_mod_data 的「无损逆操作」：
    /// 每个被管理的 mod INI 都应从 .ini_managed_backup 精确恢复到更新前字节。
    #[test]
    fn test_restore_all_inis_lossless_roundtrip() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        // 结构与真实游戏一致：game_root/Mods/_MANAGED_/group_X/Mod/mod.ini
        // 且 game_root/d3dx.ini 位于 Mods 父目录。
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().join("Mods");
        let managed = mods_path.join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let group_path = managed.join("group_1");
        fs::create_dir_all(&group_path).unwrap();
        let original = "\
[TextureOverrideBody]
hash = 0x1
drawindexed = auto
condition = orig_cond

[KeySection]
condition = (orig_cond) && other

[Resource]
hash = 0x2
";
        let mod_dir = create_mod_with_ini(&group_path, "ModA", original);
        let mod_ini = mod_dir.join("mod.ini");

        // 更新（注入管理器内容 + 创建备份）
        let result = update_mod_data(TargetGame::GenshinImpact, &mods_path, &settings).unwrap();
        assert_eq!(result.processed_mods, 1);
        // 更新后文件应已被注入（与原始不同）
        let updated = fs::read_to_string(&mod_ini).unwrap();
        assert_ne!(updated, original, "更新后应注入管理器内容");
        // 备份必须存在且为更新前字节
        let backup = mod_ini.with_extension(constants::BACKUP_EXTENSION);
        assert!(backup.exists());
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);

        // 还原（Restore All）
        let rc = restore_all_inis(&mods_path).unwrap();
        assert_eq!(rc.failed, 0);

        // 还原后 mod INI 必须精确等于更新前字节（无损回放）
        let restored = fs::read_to_string(&mod_ini).unwrap();
        assert_eq!(restored, original, "备份式还原必须无损恢复到更新前状态");

        // 备份应被删除
        assert!(!backup.exists());
        // 管理器生成文件应被清理
        assert!(!managed.join("nrmm_include.ini").exists());
        assert!(!managed.join("manager_group.ini").exists());
        assert!(!managed.join("group_1.ini").exists());
    }

    /// 就地清理式还原：对一份「已经过 update_mod_data」的 INI，验证 restore_managed_mod
    /// 清除全部管理器注入产物，但保留用户原始内容与合法悬空 endif。
    #[test]
    fn test_restore_managed_mod_cleans_injected_artifacts() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        let dir = setup_test_env();
        let ini = dir.path().join("mod.ini");
        // 模拟 update_mod_data 输出的真实形态（头注释 + Constants 全局 + 包裹段 + 增强 condition + 悬空 endif）
        let managed = "\
; \";-;\" are errored conditional lines.
; \";+;\" are disabled keys.
; Errored conditional blocks (if/else/elif/endif) are handled correctly (newer syntax may require further testing), including namespaced variables.
; If certain syntax is only available in newer XXMI versions, make sure to use the latest XXMI.
[Constants]
global $managed_slot_id = 0
[TextureOverrideBody]
hash = 0x1
match_priority = 0
if $managed_slot_id == $\\modmanageragl\\group_1\\active_slot
    drawindexed = auto
    ; inner note
endif
[KeySection]
condition = (orig_cond) && $managed_slot_id == $\\modmanageragl\\group_1\\active_slot
[Resource]
hash = 0x2
endif
";
        fs::write(&ini, managed).unwrap();

        let (total, failed) = restore_managed_mod(&ini);
        assert_eq!(total, 1);
        assert_eq!(failed, 0);

        let out = fs::read_to_string(&ini).unwrap();

        // 1) 管理器头注释被移除
        assert!(!out.contains("are errored"), "管理器头注释应被移除");
        assert!(!out.contains("are disabled keys"), "管理器头注释应被移除");
        // 2) global $managed_slot_id 被移除
        assert!(
            !out.contains("global $managed_slot_id"),
            "global 声明应被移除"
        );
        // 3) manager if 与该段 endif 被移除，且不含任何管理器残留
        assert!(!out.contains("managed_slot_id"), "管理器槽位残留应被清除");
        assert!(!out.contains("active_slot"), "管理器槽位残留应被清除");
        // 4) 4 空格缩进被移除（段体行去缩进）
        assert!(
            out.contains("drawindexed = auto"),
            "缩进后的命令应去缩进保留"
        );
        assert!(!out.contains("    drawindexed"), "不应残留 4 空格缩进");
        // 5) 非管理器注释保留
        assert!(out.contains("; inner note"), "非管理器注释应保留");
        // 6) 增强 condition 中管理器段被清除，仅留用户原条件（圆括号为 sanitize 对称解包，详见报告）
        assert!(out.contains("orig_cond"), "用户原 condition 应保留");
        // 7) 合法悬空 endif 必须保留（对齐 Dart：不删除 INPUT 遗留悬空 endif）
        assert!(out.contains("endif"), "合法悬空 endif 应被保留");
        // 8) match_priority=0 由 update 注入，restore 不移除（非管理器专属标记；
        //    作为独立 3Dmigoto 指令在脱管后依然合法。此行为是否为 NRMM 严格一致项待 Dart 基线确认。）
        assert!(
            out.contains("match_priority = 0"),
            "match_priority 注入项当前保留（对齐待确认）"
        );
    }

    /// 真实管线回放：update_mod_data 产出后，对 mod 目录执行 restore_managed_mod，
    /// 确认管理器 if/endif/global/condition 全部清除。
    #[test]
    fn test_restore_managed_mod_after_real_update() {
        let _window_lock = crate::core::mod_scanner::TEST_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _w = RestoreWindowGuard;
        let dir = TempDir::new().unwrap();
        let mods_path = dir.path().join("Mods");
        let managed = mods_path.join("_MANAGED_");
        fs::create_dir_all(&managed).unwrap();
        create_main_ini(dir.path(), TargetGame::GenshinImpact);
        let settings = AppSettings::default();

        let group_path = managed.join("group_1");
        fs::create_dir_all(&group_path).unwrap();
        let mod_dir = create_mod_with_ini(
            &group_path,
            "ModA",
            "[TextureOverrideBody]\nhash = 0x1\ndrawindexed = auto\n",
        );

        update_mod_data(TargetGame::GenshinImpact, &mods_path, &settings).unwrap();

        let (total, failed) = restore_managed_mod(&mod_dir);
        assert_eq!(total, 1);
        assert_eq!(failed, 0);

        let out = fs::read_to_string(mod_dir.join("mod.ini")).unwrap();
        assert!(
            !out.contains("managed_slot_id"),
            "真实管线还原后不应残留管理器槽位"
        );
        assert!(
            !out.contains("active_slot"),
            "真实管线还原后不应残留管理器槽位"
        );
        // 段体命令保留且去缩进
        assert!(out.contains("drawindexed = auto"));
    }

    /// 测试：collect_mod_ini_files 正确收集模组目录下的 .ini 文件
    ///
    /// 验证：
    /// - 收集直接子 .ini 文件
    /// - 不递归子目录
    /// - 排除 desktop.ini
    /// - 排除 .ini_managed_backup 备份文件
    #[test]
    fn test_collect_mod_ini_files() {
        let dir = TempDir::new().unwrap();
        let mod_dir = dir.path().join("TestMod");
        fs::create_dir_all(&mod_dir).unwrap();

        // 正常 .ini 文件
        fs::write(mod_dir.join("mod.ini"), "[Section]\n").unwrap();
        fs::write(mod_dir.join("config.ini"), "[Other]\n").unwrap();
        // desktop.ini 应被排除
        fs::write(mod_dir.join("desktop.ini"), "[System]\n").unwrap();
        // .ini_managed_backup 备份文件应被排除
        fs::write(mod_dir.join("mod.ini_managed_backup"), "[Backup]\n").unwrap();
        // 子目录中的 .ini 应被排除（不递归）
        let subdir = mod_dir.join("SubDir");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(subdir.join("nested.ini"), "[Nested]\n").unwrap();
        // 非 .ini 文件应被排除
        fs::write(mod_dir.join("readme.txt"), "text\n").unwrap();

        let ini_files = collect_mod_ini_files(&mod_dir);
        // 应收集到 mod.ini 和 config.ini（2 个文件）
        assert_eq!(
            ini_files.len(),
            2,
            "应仅收集直接 .ini 文件，排除 desktop/backup/nested"
        );
        let file_names: Vec<String> = ini_files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(file_names.contains(&"mod.ini".to_string()));
        assert!(file_names.contains(&"config.ini".to_string()));
    }
}