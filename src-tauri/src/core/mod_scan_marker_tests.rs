use super::*;
use crate::core::mod_scan_light::scan_mods_light;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// 测试辅助：创建临时目录，在根目录下创建 `_MANAGED_` 子目录
fn setup_test_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    let managed = dir.path().join("_MANAGED_");
    fs::create_dir_all(&managed).unwrap();
    dir
}

/// 测试辅助：在 `_MANAGED_` 下创建分组目录
fn create_group_dir(base: &Path, group_name: &str) -> PathBuf {
    let group_path = base.join("_MANAGED_").join(group_name);
    fs::create_dir_all(&group_path).unwrap();
    group_path
}

/// 测试辅助：在根目录下创建主 INI 文件（d3dx.ini）
fn create_d3dx_ini(base: &Path) {
    fs::write(base.join("d3dx.ini"), "; test").unwrap();
}

/// 还原安全窗口 RAII 守卫：测试结束（含 panic）后关闭窗口，避免污染其他测试
struct RestoreSafeWindowGuard;
impl Drop for RestoreSafeWindowGuard {
    fn drop(&mut self) {
        set_restore_safe_window(0);
    }
}

/// 测试：还原安全窗口内，扫描对 groupname/selectedindex/modname 全部只读不建
///
/// 还原（restore_all_inis / restore_managed_mod）会标记 60s 安全窗口，
/// 窗口内 watcher 防抖触发的重扫不得向目录回写任何专属配置文件——
/// 还原后的目录通常会被用户删除/导出，标记文件会干扰其他管理器。
#[test]
fn test_restore_safe_window_markers_readonly() {
    // 与依赖标记创建的测试互斥（全局窗口状态仅本测试设置）
    let _window_lock = TEST_WINDOW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = RestoreSafeWindowGuard;
    set_restore_safe_window(60_000);

    let dir = setup_test_dir();
    create_d3dx_ini(dir.path());
    let group_path = create_group_dir(dir.path(), "group_1");
    let mod_path = group_path.join("TestMod");
    fs::create_dir_all(&mod_path).unwrap();

    let result = scan_mods_light(dir.path()).unwrap();

    // 窗口内所有标记文件都不创建
    assert!(
        !group_path.join("groupname").exists(),
        "还原安全窗口内不应创建 groupname"
    );
    assert!(
        !group_path.join("selectedindex").exists(),
        "还原安全窗口内不应创建 selectedindex"
    );
    assert!(
        !mod_path.join("modname").exists(),
        "还原安全窗口内不应创建 modname"
    );
    // 显示名回退目录名
    assert_eq!(result.groups[0].name, "group_1");
}
