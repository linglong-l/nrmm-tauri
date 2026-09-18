//! 目录级增量更新模块
//!
//! 核心流程：
//! 1. 收集事件路径到 HashSet<PathBuf>（防抖 300ms）
//! 2. 路径收敛：找到 HashSet 中所有路径的公共父级（非 group_ 目录级别）
//! 3. 局部 DFS：只对收敛后的父级目录执行扫描
//! 4. 子树替换：将局部扫描结果 patch 到 ModCache 和 ScanResult 对应子树
//!
//! # 注意
//! 仅用于 notify 监听时的增量更新。update_mod_data 完成后必须走全量更新。

use std::collections::{BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// 目录级增量更新器，用于防抖合并文件系统变更事件并收敛到最小重扫范围。
///
/// 在 notify 监听线程中持有一个实例，通过 `collect()` 收集事件路径，
/// 300ms 防抖到期后调用 `consolidate()` 计算需要重新扫描的目录入口列表。
pub struct IncrementalUpdater {
    /// 本轮收集到的变更路径集合（来自 notify 回调）。
    ///
    /// 使用 `HashSet` 自动去重，同一路径在防抖窗口内多次触发仅保留一份。
    pending_paths: HashSet<PathBuf>,
    /// 最后一次收到事件的时间戳，用于防抖到期判定。
    ///
    /// 每次调用 `collect()` 时更新为 `Instant::now()`，
    /// `is_ready()` 据此判断距上次事件是否已超过 `debounce_duration`。
    last_event_at: Option<Instant>,
    /// 防抖时长（通常 300ms，从 constants 模块传入）。
    debounce_duration: Duration,
}

impl IncrementalUpdater {
    /// 创建一个新的增量更新器。
    ///
    /// # 参数
    /// - `debounce_ms`: 防抖时长（毫秒），通常为 300。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    pub fn new(debounce_ms: u64) -> Self {
        Self {
            pending_paths: HashSet::new(),
            last_event_at: None,
            debounce_duration: Duration::from_millis(debounce_ms),
        }
    }

    /// 收集一个文件系统变更事件路径。
    ///
    /// 路径被插入 `pending_paths`（`HashSet` 自动去重），
    /// 同时更新 `last_event_at` 为当前时刻以刷新防抖计时器。
    ///
    /// # 参数
    /// - `path`: 来自 notify 回调的变更路径。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn collect(&mut self, path: PathBuf) {
        self.pending_paths.insert(path);
        self.last_event_at = Some(Instant::now());
    }

    /// 检查防抖是否已到期。
    ///
    /// 当且仅当存在 pending 事件，且距 `last_event_at` 已超过 `debounce_duration` 时返回 `true`。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn is_ready(&self) -> bool {
        match self.last_event_at {
            None => false,
            Some(t) => !self.pending_paths.is_empty() && t.elapsed() >= self.debounce_duration,
        }
    }

    /// 检查是否已完全空闲（无任何 pending 事件）。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn is_idle(&self) -> bool {
        self.pending_paths.is_empty()
    }

    /// 返回当前 pending 路径数量，用于调试和日志。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn pending_count(&self) -> usize {
        self.pending_paths.len()
    }

    /// 执行路径收敛，将 `pending_paths` 中所有路径合并为最少重扫入口列表。
    ///
    /// # 算法（4 步）
    ///
    /// 1. **规范化**：所有路径取 parent（文件取其所在目录），调用 `normalize_path` 规范化，
    ///    排序去重得到 `dirs` 列表。
    /// 2. **最长公共前缀**：对 `dirs` 调用 `longest_common_prefix` 计算所有目录的公共祖先路径。
    /// 3. **向上收敛**：对公共前缀调用 `snap_to_group_or_mod_boundary`，
    ///    将其向上提升到最近的 `group_xx` 目录或存在 `.ini` 文件的 mod 目录边界。
    /// 4. **发散分支处理**：若收敛结果仍是 `managed_root`（所有变更过于发散），
    ///    则对每个 `dir` 独立执行 `snap_to_group_or_mod_boundary` 后去重返回列表；
    ///    否则返回单一收敛入口。
    ///
    /// # 参数
    /// - `managed_root`: `_MANAGED_` 目录的绝对路径，作为向上收敛的上限边界。
    ///
    /// # 返回
    ///
    /// 需要重新扫描的目录入口列表。空列表表示无 pending 事件。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    ///
    /// # Errors
    ///
    /// 此方法不会返回错误。
    pub fn consolidate(&self, managed_root: &Path) -> Vec<PathBuf> {
        if self.pending_paths.is_empty() {
            return Vec::new();
        }

        // Step 1: 规范化 - 所有路径都取 parent（目录），相对 managed_root 考虑
        let mut dirs: Vec<PathBuf> = self
            .pending_paths
            .iter()
            .map(|p| {
                if p.is_file() {
                    p.parent()
                        .map(|pp| pp.to_path_buf())
                        .unwrap_or_else(|| p.clone())
                } else {
                    p.clone()
                }
            })
            .map(|p| normalize_path(&p))
            .collect();
        dirs.sort();
        dirs.dedup();

        if dirs.is_empty() {
            return Vec::new();
        }

        // P1 修复（Step 1.5）：同父模组级目录群不做 LCP 归并，逐个 snap 后返回。
        //
        // 启用/禁用重命名产生「同一组内两个模组目录」的事件（`group_1/x` +
        // `group_1/DISABLED_x`，或单模组删除仅旧端一个），LCP 会收敛到组级根
        // `group_1` → 旧端已消失 → `scan_partial_path` 全量退化（6542 模组实测
        // 2×1.1s 全扫 + 220 delta/轮）。跳过归并保留模组级身份：新端走
        // `scan_single_mod` + patch、旧端（snap 的 NotFound 分支保留）走删除
        // patch；同组多模组由路由层 `patch_mod_batch_and_build_deltas` 按顶层
        // 组归并成一次 delta，无批次放大。
        //
        // 深层文件变更（`group_1/ModA/textures` + `group_1/ModA/models`）parent
        // 不同不命中，维持既有 LCP 收敛；`group_xx`/selectedindex 事件深度 1
        // 不命中，维持组级语义。
        let managed_norm = normalize_path(managed_root);
        let head_parent = parent_segments(&dirs[0]);
        let same_parent_mod_level = head_parent.is_some()
            && dirs.iter().all(|d| {
                parent_segments(d) == head_parent && is_mod_level_depth(d, &managed_norm)
            });
        if same_parent_mod_level {
            let mut set = BTreeSet::new();
            for d in &dirs {
                set.insert(snap_to_group_or_mod_boundary(d, managed_root));
            }
            return set.into_iter().collect();
        }

        // Step 2: 计算最长公共前缀
        let common = longest_common_prefix(&dirs);

        // Step 3: 向上收敛到 group 目录（或 mod 目录）
        let snapped = snap_to_group_or_mod_boundary(&common, managed_root);

        // Step 4: 如果 snapped 就是 managed_root（所有变更发散），
        // 则对每个 dir 单独 snap 后做去重，返回去重后的列表
        if snapped == normalize_path(managed_root) {
            let mut set = BTreeSet::new();
            for d in &dirs {
                set.insert(snap_to_group_or_mod_boundary(d, managed_root));
            }
            set.into_iter().collect()
        } else {
            vec![snapped]
        }
    }

    /// 清除本轮所有 pending 状态。
    ///
    /// 清空 `pending_paths` 并将 `last_event_at` 重置为 `None`。
    /// 通常在 `consolidate()` 取出收敛结果后调用，以准备下一轮事件收集。
    ///
    /// # Panics
    ///
    /// 此方法不会 panic。
    pub fn reset(&mut self) {
        self.pending_paths.clear();
        self.last_event_at = None;
    }
}

// ---------- helpers ----------

/// 规范化路径：优先尝试 `canonicalize`，失败则手动规范化。
///
/// 手动规范化策略：逐段解析 `Path::components()`，处理 `CurDir`（`.` 忽略）、
/// `ParentDir`（`..` 弹出上一段）等特殊组件，生成一个干净的标准路径。
///
/// # Panics
///
/// 此函数不会 panic。
///
/// # Errors
///
/// 此函数不会返回错误；`canonicalize` 失败时静默回退到手动规范化。
fn normalize_path(p: &Path) -> PathBuf {
    if let Ok(can) = p.canonicalize() {
        return can;
    }
    let mut buf = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Prefix(x) => buf.push(x.as_os_str()),
            Component::RootDir => buf.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                if !buf.pop() {
                    buf.push(comp.as_os_str());
                }
            }
            Component::Normal(x) => buf.push(x),
        }
    }
    buf
}

/// 计算路径列表的最长公共前缀（逐段比较 `Path::components`）。
///
/// 以第一个路径为基准，遍历后续路径，逐段比较各 component 是否相等，
/// 取所有比较结果的最小公共长度。空输入时返回空 `PathBuf`。
///
/// # 参数
/// - `paths`: 要计算公共前缀的路径列表。
///
/// # 返回
///
/// 所有路径的公共前缀路径。若 `paths` 为空，返回空 `PathBuf`。
///
/// # Panics
///
/// 此函数不会 panic。
///
/// # Errors
///
/// 此函数不会返回错误。
fn longest_common_prefix(paths: &[PathBuf]) -> PathBuf {
    if paths.is_empty() {
        return PathBuf::new();
    }
    let first: Vec<_> = paths[0].components().collect();
    let mut end = first.len();
    for other in &paths[1..] {
        let other_comps: Vec<_> = other.components().collect();
        let mut common = 0usize;
        for (i, c) in first.iter().enumerate() {
            if other_comps.get(i) == Some(c) {
                common = i + 1;
            } else {
                break;
            }
        }
        if common < end {
            end = common;
        }
    }
    let mut result = PathBuf::new();
    for c in first.iter().take(end) {
        match c {
            Component::Prefix(x) => result.push(x.as_os_str()),
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// 将路径向上收敛到最近的 `group_xx` 目录或存在 `.ini` 文件的 mod 目录边界。
///
/// # 三元边界判定
///
/// 循环向上遍历父目录，依次检查：
/// 1. **group_xx 匹配**：目录名以 `group_` 开头且后续字符全为 ASCII 数字，立即返回。
/// 2. **存在 .ini 的 mod 目录**：`read_dir` 扫描当前目录，若包含扩展名为 `.ini` 的文件，立即返回。
/// 3. **上限边界**：若当前目录已到达 `managed_root`，直接返回。
///
/// 如果既不是 group 目录也不是 mod 目录，则继续向父目录提升。
///
/// # 参数
/// - `path`: 待收敛的路径。
/// - `managed_root`: `_MANAGED_` 目录的绝对路径，作为向上收敛的上限边界。
///
/// # 返回
///
/// 收敛后的路径，最多不超过 `managed_root`。
///
/// # Panics
///
/// 此函数不会 panic。
///
/// # Errors
///
/// 此函数不会返回错误；`read_dir` 失败时仅视为无 `.ini` 文件，不会传播错误。
fn snap_to_group_or_mod_boundary(path: &Path, managed_root: &Path) -> PathBuf {
    let norm_managed = normalize_path(managed_root);
    let mut cur = normalize_path(path);
    loop {
        if cur == norm_managed {
            return cur;
        }
        if let Some(name) = cur.file_name().and_then(|n| n.to_str()) {
            if is_group_dir_name(name) {
                return cur;
            }
            let has_ini = match std::fs::read_dir(&cur) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok())
                    .find(|e| {
                        e.path()
                            .extension()
                            .and_then(|x| x.to_str())
                            .map(|ext| ext.eq_ignore_ascii_case("ini"))
                            .unwrap_or(false)
                    })
                    .map(|_| true),
                // P1 修复：已消失的模组级目录（启用/禁用重命名的旧端、单模组删除）
                // 保留自身身份、不再向上归并到组级——向上归并会让路由拿到组级根，
                // 旧端不存在时 `scan_partial_path` 全量退化（6542 模组实测
                // 2×1.1s 全扫 + 220 delta）。保留后路由层 `is_mod_level_root` +
                // `exists()==false` 走删除 patch（file_watcher 增量批次已备好该分支）。
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if is_mod_level_depth(&cur, &norm_managed) {
                        return cur;
                    }
                    None
                }
                Err(_) => None,
            };
            if has_ini == Some(true) {
                return cur;
            }
        }
        if let Some(parent) = cur.parent() {
            if parent == cur {
                return cur;
            }
            cur = parent.to_path_buf();
        } else {
            return cur;
        }
    }
}

/// `group_xx` 目录名判定（组级 snap 边界；与 `is_normal_group_dir` 同口径）。
fn is_group_dir_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let prefix = b"group_";
    bytes.len() > prefix.len()
        && &bytes[..prefix.len()] == prefix
        && bytes[prefix.len()..].iter().all(|b| b.is_ascii_digit())
}

/// 路径段切分（P1 修复私有 helper）：统一分隔符、剥 Windows verbatim 前缀
/// （`\\?\`）、段比较统一小写。`normalize_path` 对存在路径走 `canonicalize`
/// （verbatim 形态）、对已消失路径走手动规范化（普通形态），同批次 rename
/// 两端形态可能不一致——父目录/前缀比较一律用段比较，不用字符串/组件直比。
fn path_segments_lower(p: &Path) -> Vec<String> {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").map(str::to_string).unwrap_or(s);
    s.split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .map(|seg| seg.to_lowercase())
        .collect()
}

/// 路径的父目录段序列（去末段）；根路径（无父）返回 `None`。
fn parent_segments(p: &Path) -> Option<Vec<String>> {
    let segs = path_segments_lower(p);
    if segs.len() < 2 {
        return None;
    }
    Some(segs[..segs.len() - 1].to_vec())
}

/// 模组级深度判定（P1 修复私有 helper）：相对 `_MANAGED_` 深度 ≥ 2 且末段非
/// `group_xx`——与 file_watcher `is_mod_level_root` 的结构判定同口径
/// （`group_1/x` 模组 → true；`group_1` 组 → false；`_MANAGED_` 外 → false）。
fn is_mod_level_depth(p: &Path, managed_norm: &Path) -> bool {
    let p_segs = path_segments_lower(p);
    let m_segs = path_segments_lower(managed_norm);
    if p_segs.len() < m_segs.len() + 2 {
        return false;
    }
    for (a, b) in p_segs[..m_segs.len()].iter().zip(&m_segs) {
        if a != b {
            return false;
        }
    }
    !is_group_dir_name(&p_segs[p_segs.len() - 1])
}

// -------------- tests ---------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 consolidate：同一 group 下多个 mod 文件的变更应收敛到单一目录。
    ///
    /// 验证 `group_1/ModA` 下的两个文件变更（`textures/char.png` 和 `models/model.hlsl`）
    /// 经 consolidate 后只返回一个重扫入口。
    #[test]
    fn consolidate_single_mod_dir() {
        let mut u = IncrementalUpdater::new(300);
        let managed = PathBuf::from("D:/Games/HSR/Mods/_MANAGED_");
        let mod_a_textures = managed.join("group_1/ModA/textures/char.png");
        let mod_a_model = managed.join("group_1/ModA/models/model.hlsl");
        u.collect(mod_a_textures);
        u.collect(mod_a_model);
        let result = u.consolidate(&managed);
        assert_eq!(result.len(), 1);
        let s = result[0].to_string_lossy().to_ascii_lowercase();
        assert!(s.contains("moda") || s.contains("group_1"));
    }

    /// 测试 consolidate：无事件时返回空列表，且 `is_idle()` 和 `is_ready()` 状态正确。
    #[test]
    fn consolidate_keeps_empty_for_no_events() {
        let u = IncrementalUpdater::new(300);
        assert!(u.is_idle());
        assert!(!u.is_ready());
        let r = u.consolidate(Path::new("C:/x/_MANAGED_"));
        assert_eq!(r.len(), 0);
    }

    /// 测试 `longest_common_prefix`：三个路径共享 `A/B` 前缀。
    #[test]
    fn longest_common_prefix_works() {
        let a = PathBuf::from("A/B/C/D");
        let b = PathBuf::from("A/B/X/Y");
        let c = PathBuf::from("A/B/C/E");
        let r = longest_common_prefix(&[a, b, c]);
        assert_eq!(r, PathBuf::from("A/B"));
    }

    /// P1 修复回归：启用/禁用重命名的双端事件（同父模组级目录群）不归并到
    /// 组级——旧端已消失时归并会触发 `scan_partial_path` 全量退化
    /// （6542 模组实测 2×1.1s 全扫 + 220 delta/轮）。
    ///
    /// `group_1/DISABLED_x`（磁盘不存在）+ `group_1/x`（磁盘存在含 .ini）→
    /// 返回两个模组级根；`snap` 对消失端保留身份（NotFound + 深度 ≥ 2）。
    #[test]
    fn consolidate_keeps_mod_level_siblings_for_rename_pair() {
        let dir = tempfile::TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        let mod_x = managed.join("group_1").join("x");
        std::fs::create_dir_all(&mod_x).unwrap();
        std::fs::write(mod_x.join("mod.ini"), "[Section]\nx=1\n").unwrap();
        // 旧端 DISABLED_x 不创建（重命名已完成，事件晚于磁盘）

        let mut u = IncrementalUpdater::new(300);
        u.collect(mod_x.clone());
        u.collect(managed.join("group_1").join("DISABLED_x"));
        let result = u.consolidate(&managed);

        assert_eq!(result.len(), 2, "双端应保留为两个模组级根: {:?}", result);
        let lower: Vec<String> = result
            .iter()
            .map(|p| p.to_string_lossy().to_ascii_lowercase())
            .collect();
        assert!(
            lower.iter().any(|s| s.ends_with("\\x") || s.ends_with("/x")),
            "新端 x 应保留模组级: {:?}",
            lower
        );
        assert!(
            lower
                .iter()
                .any(|s| s.ends_with("disabled_x")),
            "旧端 DISABLED_x 应保留模组级（删除 patch 依赖此身份）: {:?}",
            lower
        );
    }

    /// P1 修复回归：单模组目录被删除（仅旧端一个事件）时 snap 保留该目录
    /// 身份（模组级），不向上归并到组级——否则删除操作也触发组级全量重扫。
    #[test]
    fn snap_preserves_vanished_mod_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        std::fs::create_dir_all(managed.join("group_1").join("x")).unwrap();
        std::fs::write(
            managed.join("group_1").join("x").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        // 现存模组目录：snap 返回自身（含 .ini）
        let living = snap_to_group_or_mod_boundary(
            &managed.join("group_1").join("x"),
            &managed,
        );
        assert!(living.ends_with("x"), "现存模组应 snap 到自身: {:?}", living);
        // 已消失模组目录：NotFound + 模组级深度 → 保留自身（不再向上归并）
        let vanished = snap_to_group_or_mod_boundary(
            &managed.join("group_1").join("DISABLED_x"),
            &managed,
        );
        // Path::ends_with 是组件级比较（大小写敏感），断言用实际目录名
        assert!(
            vanished.ends_with("DISABLED_x"),
            "消失的模组级目录应保留身份: {:?}",
            vanished
        );
        // 已消失组目录：末段是 group_xx → 仍 snap 到组级（组删除由路由层
        // 删除语义处理，不因本修复误判为模组级）
        let vanished_group = snap_to_group_or_mod_boundary(
            &managed.join("group_9"),
            &managed,
        );
        assert!(
            vanished_group.ends_with("group_9"),
            "组级目录（含消失）snap 到组边界: {:?}",
            vanished_group
        );
    }

    /// P1 修复边界：同模组深层子目录变更（父模组存在）仍收敛到单一模组
    /// 入口——同父检测命中后逐个 snap，两个子目录经「无 ini 向上」归并到
    /// 同一父模组，`BTreeSet` 去重后单入口（与既有 LCP 收敛结果一致）。
    #[test]
    fn consolidate_deep_files_within_same_mod_still_merges() {
        let dir = tempfile::TempDir::new().unwrap();
        let managed = dir.path().join("_MANAGED_");
        std::fs::create_dir_all(managed.join("group_1").join("ModA")).unwrap();
        std::fs::write(
            managed.join("group_1").join("ModA").join("mod.ini"),
            "[Section]\nx=1\n",
        )
        .unwrap();
        std::fs::create_dir_all(managed.join("group_1").join("ModA").join("textures")).unwrap();
        std::fs::create_dir_all(managed.join("group_1").join("ModA").join("models")).unwrap();

        let mut u = IncrementalUpdater::new(300);
        u.collect(managed.join("group_1/ModA/textures"));
        u.collect(managed.join("group_1/ModA/models"));
        let result = u.consolidate(&managed);

        assert_eq!(result.len(), 1, "深层子目录变更应收敛单入口: {:?}", result);
        assert!(
            result[0].to_string_lossy().to_lowercase().ends_with("moda"),
            "收敛到模组目录: {:?}",
            result
        );
    }
}
