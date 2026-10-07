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

/// 3Dmigoto / XXMI 游戏安装根目录（`d3dx.ini`、`d3dx_user.ini`、`d3d11.dll` 所在层）
///
/// # 为什么需要一个类型而不是一行 `parent()`
/// `d3dx*.ini` 位于 Mods 的**父目录**，即校验锚点之外——这三处写入是全项目唯一
/// 越过 Mods 根的写路径。此前每处都各写一遍 `game_mods_path.parent().unwrap_or(game_mods_path)`：
/// 一旦 Mods 根本身就是盘根/无父目录，兜底会把「游戏根」退化成 Mods 目录本身，
/// 于是主 INI 被写进模组树里，且没有任何报错。收敛成类型后：
/// - 推导规则只有一份，三处不可能各自漂移；
/// - 无父目录 / 父目录不存在一律 `Err`，不再静默换位置；
/// - 类型本身表明「由服务端从已校验锚点推导」，**不是** IPC 入参，
///   因此前端无法经由任何命令指定它（mods 根仍由 `set_game_mods_path` 登记）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRoot(PathBuf);

impl GameRoot {
    /// 从已校验的 Mods 根推导游戏根（其父目录）。
    ///
    /// # 参数
    /// - `mods_root`: 已登记的 Mods 根（调用方须保证已通过 `ManagedPath` 校验）
    ///
    /// # Errors
    /// - `[NO_GAME_ROOT]` 无父目录（路径已是盘根或根本身）
    /// - `[GAME_ROOT_NOT_FOUND]` 父目录不存在或不是目录
    ///
    /// # 返回
    /// 父目录的规范化无关形态（不做 canonicalize，避免为一次 join 触发额外系统调用）
    pub fn from_mods_root(mods_root: &Path) -> Result<Self> {
        let parent = mods_root.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "[NO_GAME_ROOT] mods root has no parent directory, cannot locate game root: {mods_root:?}"
            )
        })?;
        if !parent.is_dir() {
            return Err(anyhow::anyhow!(
                "[GAME_ROOT_NOT_FOUND] game root does not exist or is not a directory: {parent:?}"
            ));
        }
        Ok(Self(parent.to_path_buf()))
    }

    /// 游戏根路径
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// 主 INI 路径（文件名按游戏而定，见 `TargetGame::d3dx_ini_name`）
    pub fn main_ini(&self, main_ini_name: &str) -> PathBuf {
        self.0.join(main_ini_name)
    }

    /// 用户自定义 INI 路径（3Dmigoto 若存在则自动加载，NRMM 不覆盖用户段）
    pub fn user_ini(&self) -> PathBuf {
        self.0.join(constants::D3DX_USER_INI)
    }
}

/// 直接 deref 到 `Path`：既有 `game_root.join(...)` 形态的调用点无需改写即可迁移
impl std::ops::Deref for GameRoot {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<Path> for GameRoot {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::GameRoot;
    use std::fs;

    #[test]
    fn derives_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path().join("Mods");
        fs::create_dir_all(&mods).unwrap();

        let root = GameRoot::from_mods_root(&mods).unwrap();
        assert_eq!(root.as_path(), dir.path());
        assert_eq!(root.main_ini("d3dx.ini"), dir.path().join("d3dx.ini"));
        assert_eq!(root.user_ini(), dir.path().join("d3dx_user.ini"));
    }

    /// 核心防御：无父目录时绝不退化成「Mods 目录本身」
    #[test]
    fn rejects_root_without_parent_instead_of_falling_back() {
        let err = GameRoot::from_mods_root(std::path::Path::new("/"))
            .expect_err("盘根没有父目录，应报错而不是把 Mods 当游戏根");
        assert!(err.to_string().contains("[NO_GAME_ROOT]"), "实际：{err}");
    }

    /// Mods 根尚未落盘（如刚配置、目录还没建）不影响推导：判据只看父目录
    #[test]
    fn derives_from_parent_even_when_mods_root_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let ghost = dir.path().join("Mods");
        assert!(!ghost.exists());
        let root = GameRoot::from_mods_root(&ghost).expect("父目录存在即可确立游戏根");
        assert_eq!(root.as_path(), dir.path());
    }

    /// 父路径不是目录（此处是一个文件）时不得被当作游戏根
    #[test]
    fn rejects_non_directory_parent() {
        let dir = tempfile::tempdir().unwrap();
        let fileish = dir.path().join("a-file");
        fs::write(&fileish, b"x").unwrap();
        let err = GameRoot::from_mods_root(&fileish.join("Mods"))
            .expect_err("父路径为文件时不得当作游戏根");
        assert!(
            err.to_string().contains("[GAME_ROOT_NOT_FOUND]"),
            "实际：{err}"
        );
    }
}
