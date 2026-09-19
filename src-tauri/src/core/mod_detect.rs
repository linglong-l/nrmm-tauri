//! 模组环境/冲突检测域：标准 XXMI 探测、hash 冲突、orfix·texfix 检测
//!
//! 从 mod_manager.rs 拆出（2026-09-19 Phase 4-5）。
//! 依赖 ini 模型/缓存解析、mod_scanner（轻扫定位模组）；被 commands/detect_hash_conflicts
//! 与 update 域消费。

use anyhow::Result;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::constants;
use crate::core::d3dxini_cache::D3DX_INI_CACHE;
use crate::core::mod_ini_cache::get_or_parse_ini;
use crate::core::mod_manager::collect_mod_ini_files;
use crate::core::mod_scanner;
use crate::models::enums::GroupType;
use crate::models::mod_data::{
    DuplicateLib, HashConflict, HashConflictEntry, HashConflictResult, LibInMod, ModData,
    ModGroupData, NonExistentLib, OrfixDetection,
};

/// 检测当前游戏是否为标准 XXMI/3DMigoto 环境
///
/// 参考 NRMM `ini_handler_bridge.dart` 逻辑：
/// 1. 读取 `{game_mods_path}/d3dx.ini`（或游戏特定主 INI）
/// 2. 检查 `[Constants]` 段中是否存在 XXMI Launcher 注入标记
///    （包含 `xxmi`、`xxmi_inject`、`global $xxmi`、`global $inject` 等键名或值）
/// 3. 检查是否存在 `[Inject]` 段或包含 `run = CommandList` 引用
/// 4. 以上条件均满足时返回 `true`
///
/// 任何错误（文件不存在、解析失败等）均返回 `false` 且不报错，
/// 仅记录 `log::debug!` 供开发调试。
///
/// # 参数
/// - `game_mods_path`: 游戏 Mods 目录路径
/// - `main_ini_name`: 主 INI 文件名（由 TargetGame::d3dx_ini_name() 提供）
///
/// # 返回
/// - `true`: 检测到标准 XXMI/3DMigoto 环境
/// - `false`: 未检测到或检测过程发生错误
pub fn detect_standard_xxmi(game_mods_path: &Path, main_ini_name: &str) -> bool {
    let _start = std::time::Instant::now();
    let main_ini_path = game_mods_path
        .parent()
        .map(|p| p.join(main_ini_name))
        .unwrap_or_else(|| game_mods_path.join(main_ini_name));

    // 1. 读取主 INI 文件
    let content = match fs::read_to_string(&main_ini_path) {
        Ok(c) => c,
        Err(e) => {
            log::debug!(
                "[mod_manager] [detect_standard_xxmi] Main INI not found or unreadable: {:?}, err={}",
                main_ini_path, e
            );
            return false;
        }
    };

    // 2. 解析 INI，检查 [Constants] 和 [Inject] 段
    let mut in_constants = false;
    let mut has_xxmi_mark = false;
    let mut has_inject_section = false;
    let mut has_run_commandlist = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // 跳过空行和注释
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
            continue;
        }

        // 段头
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = trimmed[1..trimmed.len() - 1].trim().to_lowercase();
            in_constants = section == "constants";
            if section == "inject" {
                has_inject_section = true;
            }
            continue;
        }

        // [Constants] 段内：查找 XXMI 注入标记变量
        if in_constants {
            let lower = trimmed.to_lowercase();
            // XXMI Launcher 常见注入标记：global $xxmi_xxx、global $inject、xxmi_inject、$xxmi_inject 等
            if lower.contains("xxmi")
                || (lower.contains("global")
                    && (lower.contains("$inject") || lower.contains("inject")))
                || lower.contains("xxmi_inject")
                || lower.contains("$xxmi")
            {
                has_xxmi_mark = true;
            }
        }

        // 任意位置：检查 run = CommandList
        let lower = trimmed.to_lowercase();
        if lower.contains("run") && lower.contains("commandlist") {
            has_run_commandlist = true;
        }
    }

    let result = has_xxmi_mark && (has_inject_section || has_run_commandlist);

    log::debug!(
        "[mod_manager] [detect_standard_xxmi] result={} | has_xxmi_mark={}, has_inject_section={}, has_run_commandlist={}, elapsed={:?}ms",
        result,
        has_xxmi_mark,
        has_inject_section,
        has_run_commandlist,
        _start.elapsed().as_millis()
    );

    result
}

/// 检测 hash 冲突
///
/// 全量扫描模组 INI 中的 hash 值，按模组为单位返回冲突列表。
///
/// 扫描策略（对齐 NRMM）：
/// - NormalGroup（group_xx）：仅扫描当前选中模组（`is_active=true`）的 INI
/// - MutexGroup（非 group_xx）：扫描所有启用模组（`!disabled && !mod_disabled`）的 INI
///
/// hash 值来源：`[TextureOverride.xxx]` / `[ShaderOverride.xxx]` 段下的 `hash` 键，
/// 提取时统一转小写做归一化。同一模组内多次使用同一 hash 不算冲突，
/// 仅当同一 hash 被 ≥2 个不同模组使用时才计入冲突。
///
/// # entries 字段
/// 每条 `HashConflict` 的 `entries` 字段记录单个模组使用该 hash 的详情，
/// 每个模组一条 `HashConflictEntry`，其 `ini_vec` 保存该模组所有包含此 hash 的
/// INI 文件绝对路径（去重粒度为 (mod_name, ini_path)：同一模组同一 INI 内多次出现
/// 同一 hash 只算一条，同一模组不同 INI 各保留一条）。前端据此实现悬浮提示
/// （显示模组目录）和点击展开详情（按模组分组显示 INI 路径列表）。
///
/// # 非group目录参与策略
/// 使用 `scan_mods_light` 而非 `scan_mods_deep`，确保 MutexGroup（非group目录）
/// 下的模组也参与 hash 冲突检测。轻量扫描不解析 INI 内容，因此本函数会在
/// 扫描时按需从模组目录查找 .ini 文件并解析。非group目录不涉及标记文件生成，
/// 仅参与模组读取和 hash 冲突检测。
///
/// # 参数
/// - `game_mods_path`: 游戏 Mods 目录路径
///
/// # 返回
/// `HashConflictResult` — 含所有冲突列表（按 hash 值升序排序）、扫描统计
pub fn detect_hash_conflicts(game_mods_path: &Path) -> Result<HashConflictResult> {
    let _s = std::time::Instant::now();
    // 使用 scan_mods_light 而非 scan_mods_deep：
    // 深度扫描仅处理 group_xx 目录，会遗漏 MutexGroup（非group目录）下的模组。
    // 需求要求非group目录参与 hash 冲突检测，故改用轻量扫描以包含两类分组。
    let scan_result = mod_scanner::scan_mods_light(game_mods_path)?;
    // 三元组：(mod_name, mod_path, ini_path)
    // 记录 INI 文件路径以支持前端悬浮提示和点击展开详情
    let mut hash_to_mods: HashMap<String, Vec<(String, String, PathBuf)>> = HashMap::new();
    let mut scanned_mods = 0u32;
    let mut scanned_hashes = 0u32;

    fn process_group(
        group: &ModGroupData,
        parent_disabled: bool,
        hash_to_mods: &mut HashMap<String, Vec<(String, String, PathBuf)>>,
        scanned_mods: &mut u32,
        scanned_hashes: &mut u32,
    ) {
        // 传播分组禁用状态：父分组禁用或当前分组禁用 → 整个子树跳过
        let group_disabled = parent_disabled || group.group_disabled;
        let is_normal = group.group_type == GroupType::NormalGroup;
        for mod_data in &group.mods {
            // NormalGroup 仅扫描 is_active 模组；MutexGroup 扫描所有启用模组
            // 两者均需检查 group_disabled，避免被禁用分组下的模组参与检测
            let should_scan = if is_normal {
                mod_data.is_active
                    && !mod_data.disabled
                    && !mod_data.mod_disabled
                    && !group_disabled
            } else {
                !mod_data.disabled && !mod_data.mod_disabled && !group_disabled
            };
            if !should_scan || mod_data.mod_name == "None" {
                continue;
            }

            *scanned_mods += 1;

            // 轻量扫描未解析 INI，需从模组目录查找 .ini 文件并按需解析
            for ini_path in collect_mod_ini_files(Path::new(mod_data.full_path.as_ref())) {
                if let Ok(ini) = D3DX_INI_CACHE.write().get_or_parse(&ini_path) {
                    for (_section, hash) in ini.extract_hashes() {
                        *scanned_hashes += 1;
                        hash_to_mods.entry(hash).or_default().push((
                            mod_data.name.clone(),
                            mod_data.mod_path.clone(),
                            ini_path.clone(),
                        ));
                    }
                }
            }
        }
        // 递归子分组，传递 group_disabled
        for child in &group.children {
            process_group(
                child,
                group_disabled,
                hash_to_mods,
                scanned_mods,
                scanned_hashes,
            );
        }
    }

    for group in &scan_result.groups {
        process_group(
            group,
            false,
            &mut hash_to_mods,
            &mut scanned_mods,
            &mut scanned_hashes,
        );
    }

    // 筛选冲突：同一 hash 被 ≥2 个不同模组使用
    let mut conflicts: Vec<HashConflict> = hash_to_mods
        .into_iter()
        .filter_map(|(hash, mods)| {
            // 去重粒度为 (mod_name, ini_path)：
            // 同一模组同一 INI 内多次出现同一 hash 只算一条；
            // 同一模组不同 INI 各保留一条，聚合到该模组的 ini_vec。
            let mut seen: HashSet<(String, PathBuf)> = HashSet::new();
            let mut unique_entries: Vec<(String, String, PathBuf)> = Vec::new();
            for (name, path, ini) in mods.into_iter() {
                if seen.insert((name.clone(), ini.clone())) {
                    unique_entries.push((name, path, ini));
                }
            }
            // 仅当涉及 ≥2 个不同模组时才算冲突
            let unique_mod_count = unique_entries
                .iter()
                .map(|(n, _, _)| n.as_str())
                .collect::<HashSet<_>>()
                .len();
            if unique_mod_count >= 2 {
                // 按模组聚合：key=mod_name, value=(mod_path, ini 列表)，每个模组一条 entry
                let mut mod_map: HashMap<String, (String, Vec<String>)> = HashMap::new();
                for (n, p, ini) in unique_entries.into_iter() {
                    let ini_str = ini.to_string_lossy().to_string();
                    mod_map
                        .entry(n.clone())
                        .or_insert_with(|| (p.clone(), Vec::new()))
                        .1
                        .push(ini_str);
                }
                let entries: Vec<HashConflictEntry> = mod_map
                    .into_iter()
                    .map(|(mod_name, (mod_path, ini_vec))| HashConflictEntry {
                        mod_name,
                        mod_path,
                        ini_vec,
                    })
                    .collect();
                Some(HashConflict { hash, entries })
            } else {
                None
            }
        })
        .collect();

    // 按 hash 值排序，保证输出稳定
    conflicts.sort_by(|a, b| a.hash.cmp(&b.hash));

    log::info!(
        "[mod_manager] [detect_hash_conflicts] done | elapsed={:?}ms | scanned_mods={} scanned_hashes={} conflicts={}",
        _s.elapsed().as_millis(),
        scanned_mods,
        scanned_hashes,
        conflicts.len()
    );

    Ok(HashConflictResult {
        conflicts,
        scanned_mods,
        scanned_hashes,
    })
}

/// 检测 ORFix/TexFx 命名空间异常
///
/// 三种检测（对齐 NRMM `checkLibraries` 逻辑）：
/// 1. **库在模组内**（`libs_in_mods`）：模组 INI 声明了已知库命名空间
///    （`global\orfix` / `texfx`），这些库应位于 Mods 根目录而非模组内。
/// 2. **重复声明**（`duplicate_libs`）：同一已知库命名空间被多个模组声明。
/// 3. **引用未声明**（`nonexistent_libs`）：`run =` 引用了已知库但全局无任何模组声明。
///
/// 仅扫描传入的启用模组列表，跳过 None 空槽位。
///
/// # 参数
/// - `enabled_mods`: 启用模组列表（引用）
///
/// # 返回
/// `OrfixDetection` — 含三类检测结果及 `has_detection` 汇总标志
pub fn detect_orfix_texfx(enabled_mods: &[&ModData]) -> OrfixDetection {
    let known_libs = constants::known_lib_namespaces_set();

    // 收集阶段并行（2026-08-18 性能优化）：每模组独立检测（缓存解析只读），
    // rayon collect 保序；随后按原串行版顺序合并三个聚合结构，结果与串行版完全一致。
    // （原版对每个含检测的 INI 各生成一条 LibInMod，此处逐 INI 收集严格对齐；
    //   lib_declarations/lib_references 的 Vec 值顺序 = 模组输入顺序，后续去重+排序不依赖该顺序）
    type PerModDetections = (Vec<LibInMod>, Vec<(String, String)>, Vec<(String, String)>);
    let per_mod: Vec<PerModDetections> = enabled_mods
        .par_iter()
        .map(|mod_data| {
            let mut lib_entries: Vec<LibInMod> = Vec::new();
            let mut declarations: Vec<(String, String)> = Vec::new();
            let mut references: Vec<(String, String)> = Vec::new();
            if mod_data.mod_name == "None" {
                return (lib_entries, declarations, references);
            }
            for ini_data in &mod_data.mod_ini_data {
                if let Ok(ini) = get_or_parse_ini(std::path::Path::new(ini_data.ini_path.as_str()))
                {
                    // 1. 检测库在模组内（同时记入声明表，用于重复声明检测）
                    let detected = ini.detect_known_lib_declarations(&known_libs);
                    if !detected.is_empty() {
                        lib_entries.push(LibInMod {
                            mod_name: mod_data.name.clone(),
                            mod_path: mod_data.mod_path.clone(),
                            lib_names: detected.clone(),
                        });
                        for lib in &detected {
                            declarations.push((lib.clone(), mod_data.name.clone()));
                        }
                    }

                    // 2. 收集 run = 引用（仅引用已知库命名空间的）
                    for (ns, _) in ini.extract_run_references(&known_libs) {
                        if let Some(display) = constants::lookup_lib_display_name(&ns) {
                            references.push((display.to_string(), mod_data.name.clone()));
                        }
                    }
                }
            }
            (lib_entries, declarations, references)
        })
        .collect();

    let mut libs_in_mods: Vec<LibInMod> = Vec::new();
    // lib_display → 声明该库的模组名列表
    let mut lib_declarations: HashMap<String, Vec<String>> = HashMap::new();
    // lib_display → 引用该库的模组名列表
    let mut lib_references: HashMap<String, Vec<String>> = HashMap::new();

    for (lib_entries, declarations, references) in per_mod {
        libs_in_mods.extend(lib_entries);
        for (lib, mod_name) in declarations {
            lib_declarations.entry(lib).or_default().push(mod_name);
        }
        for (display, mod_name) in references {
            lib_references.entry(display).or_default().push(mod_name);
        }
    }

    // 3. 重复声明：同一库被 ≥2 个模组声明
    let duplicate_libs: Vec<DuplicateLib> = lib_declarations
        .into_iter()
        .filter_map(|(lib, mods)| {
            let mut unique: Vec<String> = mods
                .into_iter()
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            unique.sort();
            if unique.len() >= 2 {
                Some(DuplicateLib {
                    lib_name: lib,
                    mod_names: unique,
                })
            } else {
                None
            }
        })
        .collect();

    // 4. 引用未声明：引用了但全局无任何模组声明
    let nonexistent_libs: Vec<NonExistentLib> = lib_references
        .into_iter()
        .filter_map(|(lib, mods)| {
            // 检查该库是否被任何模组声明
            let is_declared = libs_in_mods.iter().any(|m| m.lib_names.contains(&lib));
            if !is_declared {
                let mut unique: Vec<String> = mods
                    .into_iter()
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect();
                unique.sort();
                Some(NonExistentLib {
                    lib_name: lib,
                    mod_names: unique,
                })
            } else {
                None
            }
        })
        .collect();

    let has_detection =
        !libs_in_mods.is_empty() || !duplicate_libs.is_empty() || !nonexistent_libs.is_empty();

    OrfixDetection {
        libs_in_mods,
        duplicate_libs,
        nonexistent_libs,
        has_detection,
    }
}

/// 并行收集一批 INI 文件的库定义（`defined_libraries`），归并为单个集合。
///
/// 使用 rayon `par_iter` 并行调用 `mod_ini_cache::get_or_parse_ini` 解析并提取库定义，
/// 与逐个串行收集得到的结果集合相等（并集语义；解析失败的文件被跳过，与串行版一致）。
///
/// # 参数
/// - `ini_paths`: 待解析的 INI 文件路径列表
///
/// # 返回
/// 所有成功解析的 INI 中库定义段名的并集（`HashSet<String>`）
pub(crate) fn collect_libraries_parallel(ini_paths: &[PathBuf]) -> HashSet<String> {
    ini_paths
        .par_iter()
        .filter_map(|p| get_or_parse_ini(p).ok())
        .flat_map_iter(|ini| ini.defined_libraries())
        .collect()
}

#[cfg(test)]
#[path = "mod_detect_tests.rs"]
mod tests_detect;
