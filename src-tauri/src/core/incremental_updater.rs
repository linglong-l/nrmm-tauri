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
                    p.parent().map(|pp| pp.to_path_buf()).unwrap_or_else(|| p.clone())
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
            let bytes = name.as_bytes();
            let prefix = b"group_";
            if bytes.len() > prefix.len() && &bytes[..prefix.len()] == prefix {
                let rest = &bytes[prefix.len()..];
                if !rest.is_empty() && rest.iter().all(|b| b.is_ascii_digit()) {
                    return cur;
                }
            }
            let has_ini = std::fs::read_dir(&cur)
                .ok()
                .and_then(|rd| {
                    rd.filter_map(|e| e.ok())
                        .find(|e| {
                            e.path()
                                .extension()
                                .and_then(|x| x.to_str())
                                .map(|ext| ext.eq_ignore_ascii_case("ini"))
                                .unwrap_or(false)
                        })
                        .map(|_| true)
                });
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
}