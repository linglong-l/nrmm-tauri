//! 符号链接安全的统一目录遍历工具。
//!
//! 提供 BFS（广度优先）目录遍历，具备以下特性：
//! - 解析符号链接并同时保留访问路径与真实路径（canonical path）
//! - 通过 [`VisitedPathPool`] 防止符号链接导致的循环访问
//! - 支持线程安全的共享路径池，可跨多次遍历复用
//! - 可配置深度限制、文件扩展名过滤、隐藏文件过滤、是否跟随符号链接
//!
//! # 示例
//! ```ignore
//! use crate::utils::dir_walker::{DirWalker, VisitedPathPool};
//!
//! let pool = VisitedPathPool::new();
//! let entries = DirWalker::new()
//!     .follow_symlinks(true)
//!     .file_ext("ini")
//!     .walk_bfs_with_pool(Path::new("/some/mods"), &pool);
//! for entry in &entries {
//!     println!("access={:?} real={:?}", entry.path, entry.real_path);
//! }
//! ```

use std::collections::{HashSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

/// 默认最大遍历深度，防止极端深层目录导致性能问题。
pub const DEFAULT_MAX_TRAVERSAL_DEPTH: usize = 64;

/// 遍历过程中产生的单个条目，同时携带访问路径和符号链接解析后的真实路径。
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// 遍历时使用的路径（可能包含符号链接原始形式）。
    /// 当 `follow_symlinks=true` 且条目为符号链接时，此路径保持为链接路径，
    /// `real_path` 为解析后的真实目标路径。
    pub path: PathBuf,
    /// 符号链接解析后的真实绝对路径（canonicalize 结果）。
    /// 若 canonicalize 失败则降级为绝对化路径。
    pub real_path: PathBuf,
    /// 当前遍历深度（起始目录为 0）。
    pub depth: usize,
    /// 条目类型（文件 / 目录 / 符号链接）。
    pub file_type: FileKind,
}

/// 条目的类型分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
}

/// 线程安全的已访问路径池，用于跨多次遍历共享去重状态。
///
/// 内部使用 canonicalized real path 作为去重键，确保：
/// - 同一真实路径不会被重复访问
/// - 多个符号链接指向同一目标时能正确去重（A→C、B→C 只访问一次）
/// - 支持跨线程共享（`Arc<Mutex<...>>` 可 clone 后传入 rayon 等并行环境）
#[derive(Debug, Clone, Default)]
pub struct VisitedPathPool {
    inner: Arc<Mutex<HashSet<PathBuf>>>,
}

#[allow(dead_code)]
impl VisitedPathPool {
    /// 创建一个新的空路径池。
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// 原子地检查路径是否已访问，若未访问则标记为已访问。
    ///
    /// 返回 `true` 表示路径已存在（应跳过），返回 `false` 表示首次访问（已标记）。
    pub fn check_and_mark(&self, real_path: &Path) -> bool {
        let mut set = self.inner.lock();
        if set.contains(real_path) {
            true
        } else {
            set.insert(real_path.to_path_buf());
            false
        }
    }

    /// 手动将路径标记为已访问。
    pub fn mark(&self, real_path: &Path) {
        self.inner.lock().insert(real_path.to_path_buf());
    }

    /// 检查路径是否已被访问（不修改状态）。
    pub fn contains(&self, real_path: &Path) -> bool {
        self.inner.lock().contains(real_path)
    }

    /// 返回当前池中已标记路径的数量（主要用于测试/调试）。
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    /// 池是否为空。
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }
}

/// 目录遍历器，采用 Builder 模式配置遍历参数。
#[derive(Debug, Clone)]
pub struct DirWalker {
    follow_symlinks: bool,
    max_depth: usize,
    include_files: bool,
    include_dirs: bool,
    file_ext_filter: Option<Vec<OsStringExt>>,
    skip_hidden: bool,
}

/// 扩展名过滤项的内部表示：保存 lowercased 版本以支持大小写不敏感匹配。
#[derive(Debug, Clone)]
struct OsStringExt {
    lowered: String,
}

impl OsStringExt {
    fn from_ext(ext: &str) -> Self {
        Self {
            lowered: ext.to_ascii_lowercase(),
        }
    }

    fn matches(&self, other: &OsStr) -> bool {
        let s = other.to_string_lossy();
        s.eq_ignore_ascii_case(&self.lowered)
    }
}

impl Default for DirWalker {
    fn default() -> Self {
        Self::new()
    }
}

impl DirWalker {
    /// 创建一个默认配置的遍历器：
    /// - 跟随符号链接（follow_symlinks = true）
    /// - 最大深度 64
    /// - 包含文件和目录
    /// - 不设置扩展名过滤
    /// - 跳过隐藏文件/目录（以 `.` 开头）
    pub fn new() -> Self {
        Self {
            follow_symlinks: true,
            max_depth: DEFAULT_MAX_TRAVERSAL_DEPTH,
            include_files: true,
            include_dirs: true,
            file_ext_filter: None,
            skip_hidden: true,
        }
    }

    /// 设置是否跟随符号链接。
    ///
    /// - `true`：解析符号链接并进入链接目标目录，通过 VisitedPathPool 防止循环。
    /// - `false`：遇到符号链接子目录时不进入（链接文件仍作为文件条目返回）。
    pub fn follow_symlinks(mut self, follow: bool) -> Self {
        self.follow_symlinks = follow;
        self
    }

    /// 设置最大遍历深度（0 表示仅起始目录自身）。
    #[allow(dead_code)]
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    /// 设置是否在结果中包含文件条目。
    pub fn include_files(mut self, include: bool) -> Self {
        self.include_files = include;
        self
    }

    /// 设置是否在结果中包含目录条目。
    pub fn include_dirs(mut self, include: bool) -> Self {
        self.include_dirs = include;
        self
    }

    /// 设置文件扩展名过滤（不区分大小写）。
    /// 仅匹配后缀为该扩展名的文件；对目录条目无影响。
    pub fn file_ext(mut self, ext: &str) -> Self {
        let ext = ext.trim_start_matches('.');
        self.file_ext_filter = Some(vec![OsStringExt::from_ext(ext)]);
        self
    }

    /// 设置是否跳过隐藏文件/目录（以 `.` 开头的名称）。
    pub fn skip_hidden(mut self, skip: bool) -> Self {
        self.skip_hidden = skip;
        self
    }

    /// 执行 BFS 遍历，返回所有符合条件的条目（内部创建独立的 VisitedPathPool）。
    pub fn walk_bfs(&self, root: &Path) -> Vec<DirEntry> {
        let pool = VisitedPathPool::new();
        self.walk_bfs_with_pool(root, &pool)
    }

    /// 执行 BFS 遍历，使用外部共享的 VisitedPathPool。
    /// 适合需要跨多次遍历去重的场景（如批量更新模组数据）。
    pub fn walk_bfs_with_pool(&self, root: &Path, pool: &VisitedPathPool) -> Vec<DirEntry> {
        let mut results = Vec::new();
        self.walk(root, Some(pool), |entry| {
            results.push(entry.clone());
            true
        });
        results
    }

    /// 底层遍历方法，通过回调处理每个条目，回调返回 `false` 可提前终止。
    ///
    /// 参数：
    /// - `root`：起始路径（可以是文件或目录）
    /// - `pool`：可选的共享路径池；传入 `None` 时使用内部临时池
    /// - `callback`：每个条目调用一次，返回 `true` 继续遍历，`false` 终止遍历
    pub fn walk<F>(&self, root: &Path, pool: Option<&VisitedPathPool>, mut callback: F)
    where
        F: FnMut(&DirEntry) -> bool,
    {
        let internal_pool = VisitedPathPool::new();
        let pool = pool.unwrap_or(&internal_pool);

        let abs_root = match absolutize(root) {
            Some(p) => p,
            None => return,
        };

        let (real_root, root_kind) = match resolve_path(&abs_root, self.follow_symlinks) {
            Some(v) => v,
            None => return,
        };

        if !pool.check_and_mark(&real_root) {
            let root_entry = DirEntry {
                path: abs_root.clone(),
                real_path: real_root.clone(),
                depth: 0,
                file_type: root_kind,
            };
            if !self.accept_entry(&root_entry) {
                // root 自身不满足条件，但仍需遍历其子目录
            } else {
                if !callback(&root_entry) {
                    return;
                }
            }
        }

        if root_kind != FileKind::Dir {
            return;
        }

        let mut queue: VecDeque<(PathBuf, PathBuf, usize)> = VecDeque::new();
        queue.push_back((abs_root, real_root, 0));

        while let Some((current_path, current_real, depth)) = queue.pop_front() {
            if depth >= self.max_depth {
                log::warn!("Max traversal depth reached at {:?}, skipping deeper", current_path);
                continue;
            }

            let entries = match fs::read_dir(&current_real) {
                Ok(e) => e,
                Err(err) => {
                    log::debug!("Failed to read directory {:?}: {}", current_real, err);
                    continue;
                }
            };

            for entry in entries.flatten() {
                let entry_path = current_path.join(entry.file_name());
                let name = entry.file_name();
                let name_str = name.to_string_lossy();

                if self.skip_hidden && name_str.starts_with('.') {
                    continue;
                }

                let entry_abs = match absolutize(&entry_path) {
                    Some(p) => p,
                    None => continue,
                };

                let (entry_real, kind) = match resolve_path(&entry_abs, self.follow_symlinks) {
                    Some(v) => v,
                    None => continue,
                };

                if pool.check_and_mark(&entry_real) {
                    continue;
                }

                let dir_entry = DirEntry {
                    path: entry_abs.clone(),
                    real_path: entry_real.clone(),
                    depth: depth + 1,
                    file_type: kind,
                };

                let is_dir_entry = kind == FileKind::Dir;

                if self.accept_entry(&dir_entry) && !callback(&dir_entry) {
                    return;
                }

                if is_dir_entry {
                    queue.push_back((entry_abs, entry_real, depth + 1));
                }
            }
        }
    }

    /// 判断条目是否满足当前过滤器的条件（扩展名、类型等）。
    fn accept_entry(&self, entry: &DirEntry) -> bool {
        match entry.file_type {
            FileKind::File | FileKind::Symlink => {
                if !self.include_files {
                    return false;
                }
                if let Some(filters) = &self.file_ext_filter {
                    let ext = entry.path.extension();
                    match ext {
                        Some(e) => {
                            if !filters.iter().any(|f| f.matches(e)) {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
                true
            }
            FileKind::Dir => self.include_dirs,
        }
    }
}

/// 将路径绝对化（不解析符号链接，仅确保以根路径开头）。
/// canonicalize 会要求路径存在且解析符号链接，此函数仅做路径拼接。
fn absolutize(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        let cwd = std::env::current_dir().ok()?;
        Some(cwd.join(path))
    }
}

/// 解析路径：尝试 canonicalize 获取真实路径，同时判断文件类型。
///
/// 若路径不存在（symlink_metadata 失败）返回 None。
/// 若 canonicalize 失败（权限不足等），降级使用绝对路径，
/// 并通过 `fs::symlink_metadata` 判断类型。
///
/// 返回 Some((real_path, FileKind)) 或 None（路径不存在）。
fn resolve_path(abs_path: &Path, follow_symlinks: bool) -> Option<(PathBuf, FileKind)> {
    let metadata = match fs::symlink_metadata(abs_path) {
        Ok(m) => m,
        Err(_) => {
            return None;
        }
    };

    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        if follow_symlinks {
            match fs::canonicalize(abs_path) {
                Ok(real) => {
                    let kind = match fs::metadata(&real) {
                        Ok(m) if m.is_dir() => FileKind::Dir,
                        Ok(_) => FileKind::File,
                        Err(_) => FileKind::File,
                    };
                    return Some((real, kind));
                }
                Err(_) => {
                    return Some((abs_path.to_path_buf(), FileKind::Symlink));
                }
            }
        } else {
            return Some((abs_path.to_path_buf(), FileKind::Symlink));
        }
    }

    if file_type.is_dir() {
        let real = fs::canonicalize(abs_path).unwrap_or_else(|_| abs_path.to_path_buf());
        Some((real, FileKind::Dir))
    } else {
        let real = fs::canonicalize(abs_path).unwrap_or_else(|_| abs_path.to_path_buf());
        Some((real, FileKind::File))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_tree() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("sub1")).unwrap();
        fs::create_dir_all(root.join("sub2")).unwrap();
        fs::create_dir_all(root.join(".hidden_dir")).unwrap();
        fs::write(root.join("a.ini"), "[section]\nkey=val").unwrap();
        fs::write(root.join("b.INI"), "[s]\nk=v").unwrap();
        fs::write(root.join("c.txt"), "hello").unwrap();
        fs::write(root.join(".hidden.ini"), "secret").unwrap();
        fs::write(root.join("sub1").join("d.ini"), "sub1 d").unwrap();
        fs::write(root.join("sub1").join("e.txt"), "sub1 e").unwrap();
        fs::write(root.join("sub2").join("f.ini"), "sub2 f").unwrap();

        tmp
    }

    #[test]
    fn test_basic_bfs_includes_all_by_default() {
        let tmp = setup_tree();
        let entries = DirWalker::new().skip_hidden(false).walk_bfs(tmp.path());
        let files: Vec<_> = entries
            .iter()
            .filter(|e| e.file_type == FileKind::File)
            .map(|e| e.path.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert!(files.contains(&"a.ini".to_string()));
        assert!(files.contains(&"b.INI".to_string()));
        assert!(files.contains(&"c.txt".to_string()));
        assert!(files.contains(&"d.ini".to_string()));
        assert!(files.contains(&"e.txt".to_string()));
        assert!(files.contains(&"f.ini".to_string()));
    }

    #[test]
    fn test_max_depth_limit() {
        let tmp = setup_tree();
        let entries = DirWalker::new()
            .max_depth(0)
            .skip_hidden(false)
            .include_files(true)
            .walk_bfs(tmp.path());
        let files: Vec<_> = entries
            .iter()
            .filter(|e| e.file_type == FileKind::File)
            .collect();
        assert!(files.is_empty(), "depth=0 should not return child files");

        let dirs: Vec<_> = entries
            .iter()
            .filter(|e| e.file_type == FileKind::Dir)
            .collect();
        assert_eq!(dirs.len(), 1, "depth=0 should only return root dir");
    }

    #[test]
    fn test_file_extension_filter() {
        let tmp = setup_tree();
        let entries = DirWalker::new()
            .file_ext("ini")
            .skip_hidden(true)
            .walk_bfs(tmp.path());
        let files: Vec<_> = entries
            .iter()
            .filter(|e| e.file_type == FileKind::File)
            .map(|e| e.path.file_name().unwrap().to_str().unwrap().to_lowercase())
            .collect();
        assert_eq!(files.len(), 4, "should find a/b/d/f.ini (4 ini files)");
        assert!(files.iter().all(|f| f.ends_with(".ini")));
        assert!(!files.contains(&"c.txt".to_string()));
        assert!(!files.contains(&"e.txt".to_string()));
    }

    #[test]
    fn test_skip_hidden_entries() {
        let tmp = setup_tree();
        let entries = DirWalker::new().skip_hidden(true).walk_bfs(tmp.path());
        for e in &entries {
            let name = e.path.file_name().unwrap().to_string_lossy();
            assert!(
                !name.starts_with('.') || e.depth == 0,
                "hidden entry {:?} should be skipped",
                e.path
            );
        }
    }

    #[test]
    fn test_skip_symlinks_when_disabled() {
        let tmp = setup_tree();
        let sub1 = tmp.path().join("sub1");
        let link_path = tmp.path().join("link_to_sub1");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&sub1, &link_path).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&sub1, &link_path).is_err() {
            return;
        }

        let entries = DirWalker::new()
            .follow_symlinks(false)
            .file_ext("ini")
            .skip_hidden(true)
            .walk_bfs(tmp.path());

        let files: Vec<_> = entries
            .iter()
            .filter(|e| e.file_type == FileKind::File)
            .map(|e| e.path.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(files.len(), 4, "without following symlinks, still 4 ini files");
    }

    #[test]
    fn test_follow_symlinks_no_cycle() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let real_dir = root.join("real");
        fs::create_dir_all(&real_dir).unwrap();
        fs::write(real_dir.join("x.ini"), "x").unwrap();

        let link_dir = root.join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&real_dir, &link_dir).is_err() {
            return;
        }

        let loop_link = real_dir.join("loopback");
        #[cfg(unix)]
        std::os::unix::fs::symlink(root, &loop_link).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(root, &loop_link).is_err() {
            return;
        }

        let entries = DirWalker::new()
            .follow_symlinks(true)
            .file_ext("ini")
            .skip_hidden(false)
            .walk_bfs(root);

        let files: Vec<_> = entries
            .iter()
            .filter(|e| e.file_type == FileKind::File)
            .collect();
        assert_eq!(files.len(), 1, "should only find x.ini once even with symlink loop");
    }

    #[test]
    fn test_shared_visited_pool() {
        let tmp = setup_tree();
        let pool = VisitedPathPool::new();

        let first = DirWalker::new()
            .file_ext("ini")
            .skip_hidden(false)
            .walk_bfs_with_pool(tmp.path(), &pool);
        let first_count = first.len();

        let second = DirWalker::new()
            .file_ext("ini")
            .skip_hidden(false)
            .walk_bfs_with_pool(tmp.path(), &pool);
        assert!(
            second.is_empty(),
            "second walk with same pool should return 0 entries (all visited), got {}",
            second.len()
        );

        assert!(first_count > 0);
    }

    #[test]
    fn test_include_dirs_and_files_options() {
        let tmp = setup_tree();
        let only_files = DirWalker::new()
            .include_dirs(false)
            .skip_hidden(false)
            .walk_bfs(tmp.path());
        assert!(only_files.iter().all(|e| e.file_type != FileKind::Dir));

        let only_dirs = DirWalker::new()
            .include_files(false)
            .skip_hidden(false)
            .walk_bfs(tmp.path());
        assert!(only_dirs.iter().all(|e| e.file_type == FileKind::Dir));
    }

    #[test]
    fn test_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let entries = DirWalker::new().walk_bfs(tmp.path());
        assert_eq!(entries.len(), 1, "only the root dir entry");
        assert_eq!(entries[0].file_type, FileKind::Dir);
    }

    #[test]
    fn test_single_file_input() {
        let tmp = setup_tree();
        let file_path = tmp.path().join("a.ini");
        let entries = DirWalker::new().walk_bfs(&file_path);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_type, FileKind::File);
        assert_eq!(entries[0].path.file_name().unwrap(), "a.ini");
    }

    #[test]
    fn test_nonexistent_path() {
        let entries = DirWalker::new().walk_bfs(Path::new("/nonexistent/path/xyz"));
        assert!(entries.is_empty());
    }

    #[test]
    fn test_visited_pool_check_and_mark() {
        let pool = VisitedPathPool::new();
        let p = PathBuf::from("/some/real/path");
        assert!(!pool.check_and_mark(&p));
        assert!(pool.check_and_mark(&p));
        assert!(pool.contains(&p));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_callback_false_stops_early() {
        let tmp = setup_tree();
        let mut count = 0;
        DirWalker::new()
            .file_ext("ini")
            .skip_hidden(false)
            .walk(tmp.path(), None, |_entry| {
                count += 1;
                count < 2
            });
        assert_eq!(count, 2, "should stop after first 2 entries");
    }

    #[test]
    fn test_depth_field_is_set_correctly() {
        let tmp = setup_tree();
        let entries = DirWalker::new()
            .file_ext("ini")
            .skip_hidden(false)
            .walk_bfs(tmp.path());
        let root_depth_zero = entries.iter().find(|e| e.depth == 0 && e.file_type == FileKind::Dir);
        assert!(root_depth_zero.is_some(), "root should have depth=0");
        let sub_files: Vec<_> = entries
            .iter()
            .filter(|e| e.file_type == FileKind::File && e.depth == 1)
            .collect();
        assert!(!sub_files.is_empty(), "top-level files should have depth=1");
        let nested = entries
            .iter()
            .find(|e| e.depth == 2 && e.file_type == FileKind::File);
        assert!(nested.is_some(), "sub1/d.ini and sub2/f.ini should have depth=2");
    }
}
