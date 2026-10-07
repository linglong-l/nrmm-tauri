//! 类型化的命令入参路径：把边界校验前移到反序列化
//!
//! [`ManagedPath`] 与 [`SourcePath`] 在 Tauri 解析命令入参时完成校验，函数体拿到的
//! 值**已经**满足不变量，因此「忘记调用 `validate_managed_path`」在类型层面不再可能。
//! 之所以走 [`serde::Deserialize`] 而非手写 `tauri::ipc::CommandArg`：后者会失去
//! blanket 实现，导致嵌在 DTO 里的路径字段（`KeybindFile.file_path`、
//! `ImportItemRequest.items`）无法获得同等保护——而那正是任意文件覆写的入口之一。
//!
//! # 两类语义，不做四类
//! - [`ManagedPath`]：会被**写/改名/删除/执行**的目标，必须落在已登记 Mods 根内。
//! - [`SourcePath`]：**只读来源**（用户拖入的压缩包、导入的目录），路径任意但必须存在。
//!
//! 纯展示用字段仍用 `String`，由 `arch_gate` 的静态门禁按「是否参与 IO」放行，
//! 避免为不需要保证的类型再造包装。

use std::fmt;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};

use serde::Deserializer;

use super::ipc_errors::{
    reject, E_EMPTY, E_OUT_OF_MANAGED_ROOT, E_PATH_NOT_ABSOLUTE, E_PATH_NOT_FOUND,
    E_PATH_PARENT_TRAVERSAL, E_ROOT_NOT_REGISTERED,
};
use super::mods_root_registry;

/// 路径参数中允许出现上跳组件的字段不参与本模块校验，故此处一律拒绝 `..`
const PARENT_DIR_HINT: &str = "path must not contain `..` components";

/// 词法校验：非空、绝对、不含 `..`。
///
/// 只做字符串层面的判断，不触碰文件系统；真正的落盘判定由 [`resolve_existing`] 完成。
fn check_lexical(raw: &str) -> Result<(), (&'static str, String)> {
    if raw.trim().is_empty() {
        return Err((E_EMPTY, "empty path argument".to_string()));
    }
    let p = Path::new(raw);
    if !p.is_absolute() {
        return Err((E_PATH_NOT_ABSOLUTE, format!("path is not absolute: {raw}")));
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err((E_PATH_PARENT_TRAVERSAL, format!("{PARENT_DIR_HINT}: {raw}")));
    }
    Ok(())
}

/// 词法校验 + 规范化。规范化同时完成了「存在性」判定：不存在的路径无法 canonicalize。
///
/// 规范化（而非直接使用前端字符串）是必需的：Windows 下它解析符号链接/junction 并
/// 归一大小写与 verbatim 前缀，类 Unix 下它解析符号链接，使 [`mods_root_registry`]
/// 的 `starts_with` 比较两侧形态一致。
fn resolve_existing(raw: &str) -> Result<PathBuf, (&'static str, String)> {
    check_lexical(raw)?;
    std::fs::canonicalize(Path::new(raw))
        .map_err(|e| (E_PATH_NOT_FOUND, format!("cannot resolve path {raw}: {e}")))
}

/// 位于已登记 Mods 根内的路径：写、改名、删除、打开执行类入参一律用它。
///
/// # 不变量
/// - 构造成功即代表：绝对路径、无 `..`、实际存在、规范化形态落在
///   [`mods_root_registry`] 某个根内（含根自身）。
/// - 表为空时**拒绝**（`[ROOT_NOT_REGISTERED]`），不静默放行。
///
/// # 用途
/// `Deref<Target = Path>` 直接给出规范化形态，因此既有 `&Path` 形参的调用点无需改写；
/// 需要保留前端原形态时（如按原名推导 `DISABLED_` 目标）用 [`ManagedPath::requested`]。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ManagedPath {
    /// 前端传入的原始字符串（保留用于日志与「按原名推导目标」）
    requested: String,
    /// 规范化后的形态，所有比较与 IO 一律用它
    canonical: PathBuf,
}

impl ManagedPath {
    /// 规范化形态（不变量已由反序列化保证）
    pub fn canonical(&self) -> &Path {
        &self.canonical
    }

    /// 前端原始字符串，仅用于日志与名称推导，**不得**直接用于 IO
    pub fn requested(&self) -> &str {
        &self.requested
    }

    /// 消耗自身，交出规范化路径所有权
    pub fn into_canonical(self) -> PathBuf {
        self.canonical
    }

    /// 父目录（规范化形态）。对 Mods 根本身调用会返回其上一级，调用方不应依赖
    pub fn parent(&self) -> Option<&Path> {
        self.canonical.parent()
    }

    /// 末段名称（规范化形态）
    pub fn file_name(&self) -> Option<&std::ffi::OsStr> {
        self.canonical.file_name()
    }
}

impl Deref for ManagedPath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.canonical
    }
}

impl AsRef<Path> for ManagedPath {
    fn as_ref(&self) -> &Path {
        &self.canonical
    }
}

impl fmt::Debug for ManagedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 同时打印两种形态：canonical 用于定位实际落点，requested 用于复现前端入参
        f.debug_struct("ManagedPath")
            .field("canonical", &self.canonical)
            .field("requested", &self.requested)
            .finish()
    }
}

impl fmt::Display for ManagedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.canonical.display())
    }
}

impl ManagedPath {
    /// 校验并构造。命令入参的 `Deserialize` 与测试/内部调用共用这一条路径。
    ///
    /// # Errors
    /// - 空串 `[EMPTY_PATH]`、相对路径 `[PATH_NOT_ABSOLUTE]`、含 `..` `[PATH_PARENT_TRAVERSAL]`
    /// - 无法规范化（不存在/无权限）`[PATH_NOT_FOUND]`
    /// - 尚无任何登记根 `[ROOT_NOT_REGISTERED]`
    /// - 规范化后落在所有根之外 `[OUT_OF_MANAGED_ROOT]`
    pub fn parse(raw: String) -> Result<Self, (&'static str, String)> {
        let canonical = resolve_existing(&raw)?;
        // 顺序要紧：先判「有没有锚点」，再判「是否越界」，否则未配置游戏时
        // 报出来的会是误导性的 OUT_OF_MANAGED_ROOT
        if !mods_root_registry::has_any_root() {
            return Err((
                E_ROOT_NOT_REGISTERED,
                format!("no mods_root registered while resolving {raw}"),
            ));
        }
        if !mods_root_registry::is_within_any_root(&canonical) {
            let roots = mods_root_registry::roots();
            return Err((
                E_OUT_OF_MANAGED_ROOT,
                format!("{raw} → {canonical:?} is outside registered roots {roots:?}"),
            ));
        }
        Ok(Self {
            requested: raw,
            canonical,
        })
    }
}

impl<'de> serde::Deserialize<'de> for ManagedPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(raw).map_err(|(code, msg)| reject::<D::Error>(code, msg))
    }
}

/// 只读来源路径：用户从文件管理器拖入的压缩包/目录、导入的设置文件等。
///
/// # 不变量
/// 绝对路径、无 `..`、实际存在。**不**要求落在 Mods 根内——导入来源本就在根外，
/// 把它收进 `ManagedPath` 会直接打坏拖拽导入。
///
/// # 安全边界
/// 只读语义由**调用方**保证：本类型不提供写接口，若某命令要把它作为复制/解压的
/// 目标，应改用 [`ManagedPath`]。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SourcePath {
    requested: String,
    canonical: PathBuf,
}

impl SourcePath {
    /// 规范化形态
    pub fn canonical(&self) -> &Path {
        &self.canonical
    }

    /// 前端原始字符串（日志/命名推导用）
    pub fn requested(&self) -> &str {
        &self.requested
    }

    /// 消耗自身，交出规范化路径所有权
    pub fn into_canonical(self) -> PathBuf {
        self.canonical
    }
}

impl Deref for SourcePath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.canonical
    }
}

impl AsRef<Path> for SourcePath {
    fn as_ref(&self) -> &Path {
        &self.canonical
    }
}

impl fmt::Debug for SourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourcePath")
            .field("canonical", &self.canonical)
            .field("requested", &self.requested)
            .finish()
    }
}

impl fmt::Display for SourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.canonical.display())
    }
}

impl SourcePath {
    /// 校验并构造（只读来源：绝对、无 `..`、存在，但不要求落在 Mods 根内）。
    ///
    /// # Errors
    /// 同 [`ManagedPath::parse`] 的词法与存在性错误码，但不产生根相关错误码
    pub fn parse(raw: String) -> Result<Self, (&'static str, String)> {
        let canonical = resolve_existing(&raw)?;
        Ok(Self {
            requested: raw,
            canonical,
        })
    }
}

impl<'de> serde::Deserialize<'de> for SourcePath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(raw).map_err(|(code, msg)| reject::<D::Error>(code, msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::MutexGuard;

    /// 全局 ROOTS 的串行化统一由 registry 的共享守卫负责（见 test_guard）
    use crate::ipc::mods_root_registry::test_guard;

    /// 把全局根表设为仅含 `dir`
    fn use_root(_g: &MutexGuard<'static, ()>, dir: &Path) {
        let canon = std::fs::canonicalize(dir).unwrap();
        let mut set: HashSet<PathBuf> = HashSet::new();
        set.insert(canon);
        assert_eq!(mods_root_registry::sync_from(set), 1);
    }

    /// 走真实 IPC 口径：serde_json 反序列化命令入参
    fn from_json<T: serde::de::DeserializeOwned>(p: &Path) -> Result<T, String> {
        let json = serde_json::to_string(&p.to_string_lossy().to_string()).unwrap();
        serde_json::from_str::<T>(&json).map_err(|e| e.to_string())
    }

    fn from_json_str<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, String> {
        let json = serde_json::to_string(s).unwrap();
        serde_json::from_str::<T>(&json).map_err(|e| e.to_string())
    }

    /// 断言错误串以指定 `[CODE]` 开头（前端据此分支）
    fn assert_code(err: &str, code: &str) {
        assert!(err.starts_with(code), "期望前缀 {code}，实际错误串：{err}");
    }

    #[test]
    fn managed_path_accepts_path_inside_registered_root() {
        let g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("Mods").join("group_1");
        std::fs::create_dir_all(&inner).unwrap();
        use_root(&g, dir.path());

        let m: ManagedPath = from_json(&inner).expect("根内路径应解析成功");
        assert_eq!(m.canonical(), std::fs::canonicalize(&inner).unwrap());
        assert_eq!(m.requested(), inner.to_string_lossy());
        // Deref 到规范化形态，既有 &Path 调用点可直接复用
        assert_eq!(&*m, std::fs::canonicalize(&inner).unwrap().as_path());
    }

    #[test]
    fn managed_path_rejects_path_outside_root() {
        let g = test_guard();
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        use_root(&g, root.path());

        let err = from_json::<ManagedPath>(outside.path()).unwrap_err();
        assert_code(&err, E_OUT_OF_MANAGED_ROOT);
    }

    #[test]
    fn managed_path_fails_closed_when_no_root_registered() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(dir.path()).unwrap();
        // 表置空，但路径本身存在：应报「无锚点」而非「越界」
        assert_eq!(mods_root_registry::sync_from(Vec::<String>::new()), 0);
        let err = from_json::<ManagedPath>(&canon).unwrap_err();
        assert_code(&err, E_ROOT_NOT_REGISTERED);
    }

    #[test]
    fn relative_and_parent_traversal_and_empty_are_rejected_before_io() {
        let g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        use_root(&g, dir.path());

        assert_code(
            &from_json_str::<ManagedPath>("Mods/group_1").unwrap_err(),
            E_PATH_NOT_ABSOLUTE,
        );
        assert_code(
            &from_json_str::<ManagedPath>("./relative").unwrap_err(),
            E_PATH_NOT_ABSOLUTE,
        );
        assert_code(&from_json_str::<ManagedPath>("").unwrap_err(), E_EMPTY);

        // 绝对路径但含 `..`：词法阶段即拒，不给了 fs 解析后再放行的机会
        let trick = dir.path().join("group_1").join("..").join("..");
        assert_code(
            &from_json::<ManagedPath>(&trick).unwrap_err(),
            E_PATH_PARENT_TRAVERSAL,
        );
    }

    #[test]
    fn missing_path_is_reported_as_not_found() {
        let g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        use_root(&g, dir.path());

        let ghost = dir.path().join("Mods").join("gone");
        let err = from_json::<ManagedPath>(&ghost).unwrap_err();
        assert_code(&err, E_PATH_NOT_FOUND);
    }

    #[test]
    fn source_path_allows_location_outside_root_but_requires_existence() {
        let g = test_guard();
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let archive = outside.path().join("mod.7z");
        std::fs::write(&archive, b"PK").unwrap();
        use_root(&g, root.path());

        // 根外来源必须可用，否则拖拽导入会被打断（历史踩点）
        let s: SourcePath = from_json(&archive).expect("根外只读来源应通过");
        assert_eq!(s.canonical(), std::fs::canonicalize(&archive).unwrap());

        let missing = outside.path().join("nope.7z");
        assert_code(
            &from_json::<SourcePath>(&missing).unwrap_err(),
            E_PATH_NOT_FOUND,
        );
        assert_code(
            &from_json_str::<SourcePath>("relative.7z").unwrap_err(),
            E_PATH_NOT_ABSOLUTE,
        );
    }

    /// 批量入参（`batch_toggle_mods` 形态）逐元素校验：一个越界即整体拒绝
    #[test]
    fn vec_of_managed_paths_validates_each_element() {
        let g = test_guard();
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("group_1");
        std::fs::create_dir_all(&inside).unwrap();
        let outside = tempfile::tempdir().unwrap();
        use_root(&g, root.path());

        let json = serde_json::to_string(&vec![
            inside.to_string_lossy().to_string(),
            outside.path().to_string_lossy().to_string(),
        ])
        .unwrap();
        let err = serde_json::from_str::<Vec<ManagedPath>>(&json)
            .unwrap_err()
            .to_string();
        assert_code(&err, E_OUT_OF_MANAGED_ROOT);

        let ok_json = serde_json::to_string(&vec![inside.to_string_lossy().to_string()]).unwrap();
        let ok = serde_json::from_str::<Vec<ManagedPath>>(&ok_json).unwrap();
        assert_eq!(ok.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escaping_root_is_rejected() {
        let g = test_guard();
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("settings.json");
        std::fs::write(&secret, b"{}").unwrap();

        let link = root.path().join("looks-inside");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        use_root(&g, root.path());

        // 链接本身在根内，但 canonicalize 解析到根外 → 必须拦下
        let err = from_json::<ManagedPath>(&link).unwrap_err();
        assert_code(&err, E_OUT_OF_MANAGED_ROOT);
    }
}
