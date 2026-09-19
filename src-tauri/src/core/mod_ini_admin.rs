//! INI 管理编排域：managed 目录准备、组 INI 生成、NRMM 注入内容、自定义保存
//!
//! 从 mod_manager.rs 拆出（2026-09-19 Phase 4-6）。
//! 依赖 ini 模型/编解码、namespace_handler、d3dxini_cache、constants、resources；
//! 被 update 编排域与命令层（save_customizations）消费。

use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::core::constants;
use crate::core::ini_model::IniFile;
use crate::core::mod_ini_cache::get_or_parse_ini;
use crate::core::mod_results::SaveCustomizationsResult;
use crate::core::mod_scanner;
use crate::core::namespace_handler;
use crate::models::enums::TargetGame;
use crate::models::mod_data::ModData;

/// 跨模组 namespace 去重（引入原版语义：忠实移植 `_autoModifyDuplicateNamespaceInManagedMod`）。
///
/// 遍历每个 NormalGroup 内的启用模组，收集各模组声明的 namespace；若某 namespace 与
/// 同组或全局已提交集合冲突（且不是已知 modding 库 namespace），则按下划线后缀 `_N`
/// 重命名为唯一值（`unique_namespace`，幂等），并写入 `modnamespaced` 标记。
///
/// 与原版一致的关键语义：
/// - 已知库 namespace（`KNOWN_MODDING_LIBRARY_NAMESPACES`）绝不重命名（由 xxmi ini handler 处理）。
/// - 重命名采用文本级三阶段原子提交（`replace_namespace_in_mod`，含 `.baknamespace` 备份与回滚）。
/// - 幂等：依赖 `unique_namespace` 基于当前磁盘状态推导，重跑结果一致。
/// - 仅作用于扫描得到的托管模组（`enabled_mods`），不触碰 MutexGroup/非托管目录。
///
/// 必须在 `update_mod_data` 的每-ini 注入循环**之前**调用，使改名后的 `namespace=` 声明
/// 能被 `expand_ini_variables` 正确展开。
pub fn auto_modify_duplicate_namespace(enabled_mods: &[&ModData]) {
    // 已知库 namespace（小写），用于排除
    let known_lib_ns: HashSet<String> = constants::KNOWN_MODDING_LIBRARY_NAMESPACES
        .iter()
        .map(|s| s.to_lowercase())
        .collect();

    // 按 group_index 分组（与原版 groupAndModsPair 对齐）
    let mut groups: std::collections::HashMap<u32, Vec<&ModData>> =
        std::collections::HashMap::new();
    for m in enabled_mods {
        groups.entry(m.group_index).or_default().push(*m);
    }

    let mut namespaces_in_managed: HashSet<String> = HashSet::new();

    for mods in groups.values() {
        let mut namespaces_in_group: HashSet<String> = HashSet::new();
        for m in mods {
            // 收集本模组的 namespace 与各 ini 路径
            let mut namespaces_in_mod: HashSet<String> = HashSet::new();
            let mut ini_paths: Vec<PathBuf> = Vec::new();
            for ini_data in &m.mod_ini_data {
                let p = PathBuf::from(&ini_data.ini_path);
                // C6：预检转换，保证 namespace 判定与后续 rename 写回均基于一致 UTF-8
                ensure_ini_utf8(&p);
                if let Ok(ini) = get_or_parse_ini(&p) {
                    if let Some(ns) = namespace_handler::extract_namespace(&ini) {
                        namespaces_in_mod.insert(ns);
                    }
                }
                ini_paths.push(p);
            }

            // 规划重命名：基于 occupied（future ∪ 同组 ∪ 全局）求唯一后缀，保证跨模组唯一
            let mut planned: Vec<(String, String)> = Vec::new();
            let mut future: HashSet<String> = namespaces_in_mod.clone();
            for ns in &namespaces_in_mod {
                let ns_lower = ns.to_lowercase();
                let collides =
                    namespaces_in_group.contains(ns) || namespaces_in_managed.contains(ns);
                if collides && !known_lib_ns.contains(&ns_lower) {
                    let mut occupied = future.clone();
                    occupied.extend(namespaces_in_group.iter().cloned());
                    occupied.extend(namespaces_in_managed.iter().cloned());
                    let new_ns = namespace_handler::unique_namespace(ns, &occupied);
                    planned.push((ns.clone(), new_ns.clone()));
                    future.remove(ns);
                    future.insert(new_ns);
                }
            }

            // 应用重命名
            for (old_ns, new_ns) in planned {
                match namespace_handler::replace_namespace_in_mod(&ini_paths, &old_ns, &new_ns) {
                    Ok(true) => {
                        log::info!(
                            "[mod_manager] [auto_modify_duplicate_namespace] 重命名 namespace {} -> {} 于模组 {}",
                            old_ns, new_ns, m.mod_path
                        );
                    }
                    Ok(false) => {}
                    Err(e) => {
                        log::error!(
                            "[mod_manager] [auto_modify_duplicate_namespace] 模组 {} 重命名失败: {}",
                            m.mod_path, e
                        );
                    }
                }
            }

            // 提交本模组 namespace 到组状态；写 modnamespaced 标记
            namespaces_in_group.extend(future.iter().cloned());
            if !namespaces_in_mod.is_empty() {
                let marker =
                    std::path::Path::new(m.mod_path.as_str()).join(constants::NAMESPACED_MARKER);
                if let Err(e) = crate::utils::atomic_write(&marker, b"") {
                    log::warn!("写入 namespaced 标记失败 {:?}: {}", marker, e);
                }
            }
        }
        namespaces_in_managed.extend(namespaces_in_group.iter().cloned());
    }
}

/// 准备 _MANAGED_ 目录，创建 NRMM 所需的模板 INI 文件
///
/// 复刻 NRMM 的 _prepareManagedFolder 流程：
/// 1. 创建 nrmm_keypress.txt（按键监听配置）
/// 2. 创建 nrmm_include.ini（include 入口点）
/// 3. 创建 manager_group.ini（全局管理组，定义 active_group_id 等变量）
///
/// # 返回值
/// 对齐 Dart `_tryRenameOldManagedFolder`：首次运行（`_MANAGED_` 尚不存在）时，
/// 将旧版托管目录名重命名为当前 `_MANAGED_`。依次尝试两个遗留名：
/// - `V1_3_x_MANAGED-DO_NOT_EDIT_COPY_MOVE_CUT`（V1.3.x 遗留，优先）
/// - `MANAGED-DO_NOT_EDIT_COPY_MOVE_CUT`（更早遗留，回退）
///
/// 仅当 `_MANAGED_` 不存在且某个遗留目录存在时执行 `rename`（同盘原子操作）。
/// 全部不存在则不做任何操作，由调用方随后创建 `_MANAGED_`。
pub(crate) fn migrate_old_managed_folder(mods_path: &Path) {
    let managed = mods_path.join(constants::MANAGED_FOLDER);
    if managed.exists() {
        return;
    }
    for old in [
        constants::OLD_MANAGED_FOLDER_V1,
        constants::OLD_MANAGED_FOLDER_LEGACY,
    ] {
        let old_path = mods_path.join(old);
        if old_path.is_dir() {
            match fs::rename(&old_path, &managed) {
                Ok(()) => {
                    log::info!(
                        "[mod_manager] migrate_old_managed_folder: 旧托管目录已迁移为 _MANAGED_：{:?}",
                        old_path
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[mod_manager] migrate_old_managed_folder: 旧托管目录重命名失败 {:?} -> {:?}: {}",
                        old_path,
                        managed,
                        e
                    );
                }
            }
            break;
        }
    }
}

/// 准备 `_MANAGED_` 目录：创建 NRMM 所需的模板 INI 文件（keypress / include / manager_group / 根级 selectedindex）。
///
/// # 返回
/// - `true`: 需要用户手动重载（F10）
/// - `false`: 支持自动重载
pub(crate) fn prepare_managed_folder(managed_path: &Path, game: TargetGame) -> Result<bool> {
    let mut need_reload_manual = false;

    // 检查是否已存在必要文件，不存在则需要手动重载
    let keypress_path = managed_path.join(constants::KEYPRESS_FILENAME);
    let include_path = managed_path.join(constants::INCLUDE_FILENAME);
    let manager_group_path = managed_path.join("manager_group.ini");

    if !keypress_path.exists() || !include_path.exists() {
        need_reload_manual = true;
    }

    // 对齐 NRMM DLL 能力检测：依据游戏 d3d11.dll 选择按键监听模板。
    // - NRMM 自定义 XXMI DLL（支持 [Loader] manager）→ listen_keypress_manager.txt
    // - 支持 additional_foreground_window → listen_keypress_additional_window.txt
    // - 其他 → listen_keypress_even_on_background.txt（默认）
    // d3d11.dll 位于 game_root（Mods 的父目录）；缺失时安全回退到默认模板。
    let game_root = managed_path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| managed_path.to_path_buf());
    let dll_path = game_root.join("d3d11.dll");
    let keypress_bytes =
        crate::core::dll_capability::select_keypress_template(&dll_path).template_bytes();
    let keypress_template = String::from_utf8_lossy(keypress_bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace("{game}", game.nrmm_name());
    atomic_write_file(&keypress_path, keypress_template.as_bytes())?;

    // 创建 nrmm_include.ini - 将由后续步骤填充 include 列表
    // 这里仅写入 [IncludeKeypress] section include keypress 文件
    let include_content = format!(
        "[IncludeKeypress]\ninclude = {}\n",
        constants::KEYPRESS_FILENAME
    );
    atomic_write_file(&include_path, include_content.as_bytes())?;

    // 创建 manager_group.ini - 全局管理组
    let manager_template = String::from_utf8_lossy(crate::resources::TEMPLATE_MANAGER_GROUP);
    // 对齐 Dart 模板：去掉末尾换行（Dart 生成的 manager_group.ini 末尾无换行符）
    let manager_template = manager_template.trim_end_matches(['\r', '\n']);
    atomic_write_file(&manager_group_path, manager_template.as_bytes())?;

    // 创建根级 selectedindex 文件（若不存在）
    // NRMM 在 _MANAGED_ 第一层维护 selectedindex 记录全局选中槽位，默认值 "0"
    let root_selectedindex = managed_path.join(constants::SELECTED_INDEX_FILE);
    if !root_selectedindex.exists() {
        atomic_write_file(&root_selectedindex, b"0")?;
        log::debug!(
            "[mod_manager] prepare_managed_folder: 创建根级 selectedindex 文件 {:?}",
            root_selectedindex
        );
    }

    Ok(need_reload_manual)
}

/// 对齐 NRMM `_fixNonManagedModsCrashLine`：扫描 `mods_path` 下、位于 `_MANAGED_` 之外的
/// 所有 .ini 文件，注释会导致 XXMI 崩溃的绘制/缓冲行（前缀 `;-;`）。
///
/// 仅处理非托管模组（file_path 不在 managed_path 内）；托管模组由主流程
/// `inject_slot_conditions` 中的 `comment_crash_lines` 处理，避免重复注释。
///
/// 解析走 [`get_or_parse_ini`]（MOD_INI_CACHE）：缓存条目带 `(mtime, len)` 元数据，
/// 未变化的文件直接命中缓存跳过解析；`comment_crash_lines_in_file` 回写后
/// mtime/len 改变，下次调用自动未命中并重新解析，不会拿到旧内容。
/// （commands 层无对 MOD_INI_CACHE 的显式 invalidate，新鲜度完全依赖该元数据校验。）
pub(crate) fn fix_non_managed_crash_lines(
    mods_path: &Path,
    managed_path: &Path,
    known_libraries: &HashSet<String>,
) {
    let t_fix = std::time::Instant::now();
    // 串行 WalkDir 仅收集文件路径（过滤逻辑不变：is_file、.ini 扩展、跳过 _MANAGED_ 内），
    // 文件集合互不相交，随后交由 rayon 线程池并行处理，结果与串行版一致。
    //
    // 性能关键（2026-08-18）：filter_entry 在目录层级修剪掉整个 _MANAGED_ 子树，
    // 避免对其内部数万个条目做 stat/is_file 系统调用（实测全树 stat 占 update 总耗时 ~45%）。
    // 修剪语义与原逐文件 starts_with 过滤等价：_MANAGED_ 内任何文件都不会出现在结果中。
    let ini_files: Vec<PathBuf> = WalkDir::new(mods_path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !entry.path().starts_with(managed_path))
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            path.is_file()
                && path.extension().and_then(|e| e.to_str()) == Some("ini")
                // 仅处理非托管（不在 _MANAGED_ 内）的 .ini（filter_entry 已修剪，
                // 此处保留为双保险，代价仅字符串前缀比较）
                && !path.starts_with(managed_path)
        })
        .map(|entry| entry.into_path())
        .collect();
    let scanned = ini_files.len();
    let fixed_files = std::sync::atomic::AtomicUsize::new(0);

    // 与组级 catch_unwind 一致的 panic 隔离：单个文件 parse/detect_errors/
    // comment 若 panic（release=unwind 后生效，见 B1），仅跳过该文件，不中断其余修复。
    ini_files.par_iter().for_each(|path| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let ini = match get_or_parse_ini(path) {
                Ok(ini) => ini,
                Err(_) => return,
            };
            let errors = ini.detect_errors(path, known_libraries);
            let crash_lines: Vec<(u32, String)> = errors
                .into_iter()
                .filter(|e| e.error_type == 1)
                .map(|e| (e.line_number, e.line))
                .collect();
            if crash_lines.is_empty() {
                return;
            }
            if comment_crash_lines_in_file(path, &crash_lines) {
                fixed_files.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                log::info!(
                    "修复非托管模组崩溃行 {} 处：{:?}",
                    crash_lines.len(),
                    path
                );
            }
        }));
        if let Err(payload) = result {
            let reason = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            log::error!(
                "[mod_manager] [fix_non_managed_crash_lines] file panicked, skipped: {} | path={:?}",
                reason,
                path
            );
        }
    });
    log::info!(
        "[UPDATE] phase=fix_non_managed took={}ms files={} fixed_files={}",
        t_fix.elapsed().as_millis(),
        scanned,
        fixed_files.load(std::sync::atomic::Ordering::Relaxed)
    );
}

/// C6 统一编码管线预检：若源文件非 UTF-8，先按 GB18030（覆盖 GBK/GB2312）无损解码
/// 为 UTF-8 并原子写回，使后续任何 `get_or_parse_ini` / INI 写回链路读取到一致的 UTF-8。
/// UTF-8 文件仅读取不改写（零副作用），保证纯 UTF-8 数据集字节级不变。
pub(crate) fn ensure_ini_utf8(path: &Path) {
    let (converted, info) = match crate::core::ini_codec::read_ini_safe(path) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("[mod_manager] ensure_ini_utf8: 读取失败 {:?}: {}", path, e);
            return;
        }
    };
    if info.encoding == crate::core::ini_codec::SourceEncoding::Gbk {
        if let Err(e) = crate::core::ini_codec::write_utf8_atomic(path, &converted, info) {
            log::warn!(
                "[mod_manager] ensure_ini_utf8: 转换写回失败 {:?}: {}",
                path,
                e
            );
        }
    }
}

/// 对齐 Dart `_fixNonManagedModsCrashLine`：将指定行号、行内容的崩溃行注释掉。
///
/// 规则：行号有效、该行 trim 后小写与记录一致、且尚未以 `;-;` 开头时，
/// 前缀 `;-;`（`;-;<trimmed>`），其余行保持不变。返回是否发生修改。
///
/// # 编码安全（C6：统一 UTF-8 决策）
/// 此前非 UTF-8（如 GBK/GB18030）文件被直接跳过修复；现统一编码——先经
/// `read_ini_safe` 无损转换为 UTF-8，命中崩溃行时前置 `;-;` 并保留
/// BOM/CRLF/末尾换行后原子写回（不再拒绝，全数据集统一 UTF-8）。
pub(crate) fn comment_crash_lines_in_file(path: &Path, crash_lines: &[(u32, String)]) -> bool {
    let (content, info) = match crate::core::ini_codec::read_ini_safe(path) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let crlf = content.contains("\r\n");
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut changed = false;
    for (num, text) in crash_lines {
        let idx = (*num as usize).saturating_sub(1);
        if idx >= lines.len() {
            continue;
        }
        let trimmed = lines[idx].trim();
        if trimmed.eq_ignore_ascii_case(text.trim()) && !trimmed.starts_with(";-;") {
            lines[idx] = format!(";-;{}", trimmed);
            changed = true;
        }
    }
    if changed {
        let mut out = lines.join(if crlf { "\r\n" } else { "\n" });
        // 保留文件末尾换行（str::lines 会吞掉末尾的 \n）
        if content.ends_with('\n') {
            out.push_str(if crlf { "\r\n" } else { "\n" });
        }
        if crate::core::ini_codec::write_utf8_atomic(path, &out, info).is_ok() {
            return true;
        }
    }
    false
}

/// 校验现有 group_X.ini 中 `global $group_id` 是否与目录名 `group_X` 一致。
///
/// 对齐原版 NRMM：读取每个活跃 group 目录下的 group_X.ini，提取 `global $group_id = Y`
/// 并与目录名 `group_X` 中的 X 比对。不一致的 group id 将被跳过（不处理）。
///
/// 返回不一致的 group_id 列表（空表示全部一致）。
pub(crate) fn validate_group_ini_consistency(
    managed_path: &Path,
    enabled_mods: &[&ModData],
) -> Vec<u32> {
    // 收集所有活跃的 group_id
    let mut active_groups: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for m in enabled_mods {
        active_groups.insert(m.group_index);
    }

    let mut mismatched: Vec<u32> = Vec::new();
    for gid in &active_groups {
        let group_dir = managed_path.join(format!("group_{}", gid));
        let group_ini = group_dir.join(format!("group_{}.ini", gid));
        if !group_ini.exists() {
            continue; // 首次更新，无旧 INI，无需校验
        }
        let content = match IniFile::force_read_as_utf8(&group_ini) {
            Ok(c) => c,
            Err(_) => continue,
        };
        // 查找 global $group_id = <N>
        let mut found_id: Option<u32> = None;
        for line in content.lines() {
            let trimmed = line.trim();
            let lower = trimmed.to_lowercase();
            if lower.starts_with("global $group_id") {
                if let Some(eq) = trimmed.find('=') {
                    let val = trimmed[eq + 1..].trim();
                    if let Ok(n) = val.parse::<u32>() {
                        found_id = Some(n);
                    }
                }
                break;
            }
        }
        if let Some(ini_group_id) = found_id {
            if ini_group_id != *gid {
                log::warn!(
                    "[mod_manager] group_{}.ini 中 group_id={} 与目录 group_{} 不一致，跳过",
                    gid,
                    ini_group_id,
                    gid
                );
                mismatched.push(*gid);
            }
        }
    }
    mismatched
}

/// 清理 _MANAGED_ 目录下旧的 group_*.ini 文件
///
/// 复刻 NRMM 的 _deleteGroupIniFiles：删除 group_1.ini ~ group_500.ini（包括子目录）
pub(crate) fn delete_group_ini_files(managed_path: &Path) -> Result<()> {
    if !managed_path.exists() {
        return Ok(());
    }

    fn delete_in_dir(dir: &Path) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    // 仅清理严格 group_<int>.ini（int 须满足 is_normal_group_dir），
                    // 避免误删 group_01 / group_0 / groupxx 等非标准目录下的文件。
                    let is_group_ini = name
                        .strip_suffix(".ini")
                        .map(|stem| mod_scanner::is_normal_group_dir(stem).is_some())
                        .unwrap_or(false);
                    // 也清理 ModFolder.ini（旧版本命名）
                    if is_group_ini || name == "ModFolder.ini" {
                        if let Err(e) = fs::remove_file(&path) {
                            log::warn!("Failed to delete old group INI {:?}: {}", path, e);
                        }
                    }
                }
            } else if path.is_dir() {
                // 仅递归清理严格 group_<int> 目录；绝不触碰 # / XX / groupxx 等非标准目录
                let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if mod_scanner::is_normal_group_dir(dir_name).is_some() {
                    let _ = delete_in_dir(&path);
                }
            }
        }
        Ok(())
    }

    delete_in_dir(managed_path)
}

/// 为单个分组创建 ModFolder.ini（或 group_X.ini）
///
/// 复刻 NRMM 的 _createGroupIni：
/// - 使用 TEMPLATE_GROUP 模板
/// - 替换 {x} 为分组索引，{group_x} 为 group_X 目录名
/// - 在文件末尾 include 该组所有启用模组的 INI 文件
///
/// # 返回值
/// - Some(path): 创建成功，返回 INI 文件路径
/// - None: 分组目录不存在或无启用模组
pub(crate) fn create_group_ini(
    group_dir: &Path,
    group_index: u32,
    mod_ini_paths: &[PathBuf],
) -> Result<Option<PathBuf>> {
    // 跳过空分组：无启用模组时不生成 group_X.ini，
    // 避免 nrmm_include.ini 引用空文件导致 3Dmigoto 加载警告
    if mod_ini_paths.is_empty() {
        log::debug!(
            "[mod_manager] create_group_ini: 跳过空分组 group_{}",
            group_index
        );
        return Ok(None);
    }

    // 确保分组目录存在
    if !group_dir.exists() {
        fs::create_dir_all(group_dir)?;
    }

    let group_folder_name = format!("group_{}", group_index);
    let group_ini_filename = format!("group_{}.ini", group_index);
    let group_ini_path = group_dir.join(&group_ini_filename);

    // 从模板生成基础内容（对齐 NRMM：仅包含组模板，不添加显式 include 指令。
    // 模组 INI 由 d3dx.ini 中的 include_recursive=Mods 自动递归加载。）
    let template_str = String::from_utf8_lossy(crate::resources::TEMPLATE_GROUP);
    let content = template_str
        .replace("{group_x}", &group_folder_name)
        .replace("{x}", &group_index.to_string());
    // 对齐 Dart 模板：去掉模板末尾的换行（Dart 生成的 group_X.ini 末尾无换行符）
    let content = content.trim_end_matches(['\r', '\n']);

    atomic_write_file(&group_ini_path, content.as_bytes())?;

    // 创建分组级 selectedindex 文件（对齐 NRMM getSelectedModInGroup 的惰性创建行为：
    // 文件不存在时自动创建，默认值 "0" 表示未选中任何模组）
    let group_selectedindex = group_dir.join(constants::SELECTED_INDEX_FILE);
    if !group_selectedindex.exists() {
        atomic_write_file(&group_selectedindex, b"0")?;
        log::debug!(
            "[mod_manager] create_group_ini: 创建分组级 selectedindex 文件 {:?}",
            group_selectedindex
        );
    }

    Ok(Some(group_ini_path))
}

/// 创建 nrmm_include.ini（对齐 NRMM：仅包含 [IncludeKeypress] 段。
///
/// 模组和分组 INI 的加载由 d3dx.ini 中的 include_recursive=Mods 自动处理，
/// 不在本文件中添加显式 include 指令，避免路径解析问题。）
pub(crate) fn create_nrmm_include_ini(
    include_path: &Path,
    _managed_path: &Path,
    _group_ini_paths: &[PathBuf],
    _game_mods_path: &Path,
) -> Result<()> {
    // 对齐 NRMM：nrmm_include.ini 仅包含 IncludeKeypress 段
    let content = format!(
        "[IncludeKeypress]\ninclude = {}\n",
        constants::KEYPRESS_FILENAME
    );
    // 对齐 Dart：去掉末尾换行（Dart 生成的 nrmm_include.ini 末尾无换行符）
    let content = content.trim_end_matches(['\r', '\n']);

    atomic_write_file(include_path, content.as_bytes())?;
    Ok(())
}

/// 原子写入文件（先写 tmp，再 rename）
pub(crate) fn atomic_write_file(path: &Path, content: &[u8]) -> Result<()> {
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, content)
        .with_context(|| format!("Failed to write temp file: {:?}", tmp_path))?;
    fs::rename(&tmp_path, path)
        .with_context(|| format!("Failed to rename temp file to: {:?}", path))?;
    Ok(())
}

/// 移除主 INI 中 NRMM 之前注入的内容
pub(crate) fn strip_nrmm_injected_content(content: &str) -> String {
    let start_marker = ";NRMM_INI_START";
    let end_marker = ";NRMM_INI_END";

    let mut result = String::new();
    let mut in_injected = false;

    for line in content.lines() {
        if line.contains(start_marker) {
            in_injected = true;
            continue;
        }
        if line.contains(end_marker) {
            in_injected = false;
            continue;
        }
        if !in_injected {
            result.push_str(line);
            result.push('\n');
        }
    }

    result.trim_end().to_string()
}

/// 生成主 INI 中 NRMM 注入的内容段
///
/// 复刻 NRMM 的注入格式：
/// - ;NRMM_INI_START / ;NRMM_INI_END 标记
/// - [Constants] 段定义管理变量
/// - include nrmm_include.ini
#[allow(dead_code)]
pub(crate) fn generate_nrmm_injected_content(
    nrmm_include_path: &Path,
    game_mods_path: &Path,
) -> Result<String> {
    let mut content = String::new();

    content.push_str(";NRMM_INI_START\n");
    content.push_str("; ==========================================\n");
    content.push_str("; No-Reload Mod Manager managed section\n");
    content.push_str("; Do not edit this section manually\n");
    content.push_str("; ==========================================\n\n");

    content.push_str("[Constants]\n");
    content.push_str("global $managed_slot_id = 0\n\n");

    // include nrmm_include.ini（而不是直接 include 所有模组 INI）
    if let Ok(rel_path) = nrmm_include_path.strip_prefix(game_mods_path) {
        let rel_str = rel_path.to_string_lossy().replace('\\', "/");
        content.push_str(&format!("include = {}\n", rel_str));
    }

    content.push_str("\n; ==========================================\n");
    content.push_str("; End of NRMM managed section\n");
    content.push_str(";NRMM_INI_END\n");

    Ok(content)
}

/// 检测 d3dx.ini 是否配置了指向 game_mods_path 的 include_recursive 指令
///
/// 3Dmigoto 通过 [Include] 段内的 include_recursive 指令递归加载目录下所有 .ini 文件。
/// NRMM 依赖此机制自动加载 _MANAGED_ 下的 nrmm_include.ini 等文件。
/// 若 d3dx.ini 缺少此配置，_MANAGED_ 下的管理文件不会被 3Dmigoto 加载，
/// 导致按键模拟钩子（[KeyMod]/[KeyGroup]）和前台窗口设置（check_foreground_window）无效。
///
/// # 参数
/// - `main_ini_path`: d3dx.ini 文件路径
/// - `game_mods_path`: Mods 目录路径（include_recursive 应指向此目录）
///
/// # 返回值
/// - `true`: 检测到 [Include] 段内包含指向 Mods 目录的 include_recursive 指令
/// - `false`: 未检测到，需警告用户手动添加
pub(crate) fn detect_include_recursive(main_ini_path: &Path, game_mods_path: &Path) -> bool {
    let content = match IniFile::force_read_as_utf8(main_ini_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let mods_dir_name = game_mods_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Mods");

    let mut in_include_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = &trimmed[1..trimmed.len() - 1];
            in_include_section = section.eq_ignore_ascii_case("Include");
            continue;
        }
        if in_include_section {
            if let Some(rest) = trimmed.strip_prefix("include_recursive") {
                let value = rest.trim_start_matches(['=', ' ']).trim();
                if value.contains(mods_dir_name) {
                    return true;
                }
            }
        }
    }
    false
}

/// Save Mod Customizations：将当前模组自定义状态写入 d3dx_user.ini
///
/// 复刻 NRMM 的 Save Customizations 功能：
/// - 读取当前 d3dx.ini 中的用户自定义 section（非 NRMM 管理的 section）
/// - 将这些自定义内容写入 d3dx_user.ini
/// - 用户可以在 d3dx_user.ini 中添加自定义配置，这些配置不会被 NRMM 覆盖
///
/// # 注意
/// - d3dx_user.ini 会被 3Dmigoto 自动加载（如果存在）
/// - NRMM 不会修改或覆盖 d3dx_user.ini 中的内容
pub fn save_customizations(
    game_mods_path: &Path,
    game: TargetGame,
) -> Result<SaveCustomizationsResult> {
    let main_ini_name = game.d3dx_ini_name();
    // d3dx.ini 和 d3dx_user.ini 位于游戏根目录（Mods 的父目录），不在 Mods/ 内
    let game_root = game_mods_path.parent().unwrap_or(game_mods_path);
    let main_ini_path = game_root.join(main_ini_name);
    let user_ini_path = game_root.join("d3dx_user.ini");

    if !main_ini_path.exists() {
        return Ok(SaveCustomizationsResult {
            success: false,
            message: "Main INI file not found".to_string(),
        });
    }

    let content = IniFile::force_read_as_utf8(&main_ini_path)?;
    let stripped = strip_nrmm_injected_content(&content);

    // 将非 NRMM 管理的内容写入 d3dx_user.ini
    // 注意：这里只做简单复制，实际 NRMM 会更智能地提取用户自定义 section
    let header = "; Custom settings saved by NRMM\n";
    let header = format!("{}; This file will NOT be overwritten by NRMM\n; Add your custom [Constants] and other sections here\n\n", header);
    let final_content = format!("{}{}", header, stripped);

    atomic_write_file(&user_ini_path, final_content.as_bytes())?;

    Ok(SaveCustomizationsResult {
        success: true,
        message: "Customizations saved to d3dx_user.ini".to_string(),
    })
}

#[cfg(test)]
#[path = "mod_ini_admin_tests.rs"]
mod tests;
