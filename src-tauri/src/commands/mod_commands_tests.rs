//! mod_commands 单元测试（由 mod_commands.rs 以 `#[path] mod tests;` 挂载）
//!
//! 承载 update_mod_data 逻辑整合测试、还原数据测试与命令级 Delta 组装（R1）测试。
//! 迁移自 mod_commands.rs（2026-09-19 Phase 6-3）；被调用的共享辅助
//! `build_group_delta` / `build_removed_group_delta` 已迁至 mod_commands_util.rs，
//! 此处经完整路径引用。

use crate::core::constants;
use crate::models::enums::TargetGame;
use crate::models::settings::AppSettings;
use std::fs;
use std::path::{Path, PathBuf};

/// 持久化运行时目录：cargo test 后可直接在磁盘查看产物，验证目录确实变化
const RUNTIME_DIR: &str = "tests/runtime/update_mod_data_runtime";

/// 串行化对共享 `RUNTIME_DIR` 的访问：多个测试并行执行时会互相 `remove_dir_all`
/// 同一目录（Windows 下目录句柄占用 → PermissionDenied）。引用该目录的测试入口须持有此锁。
static RUNTIME_DIR_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

/// 输入数据集（原始，未经 update_mod_data 处理）
fn input_dataset() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("datasets")
        .join("NRMM-Rust-test");
    if !manifest_dir.exists() {
        panic!(
            "Input dataset directory does not exist: {}",
            manifest_dir.display()
        );
    }
    manifest_dir
}

/// 预期输出基线（Dart 原版 NRMM 实测产物）
fn baseline_managed() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("datasets")
        .join("NRMM-test")
        .join("Mods")
        .join("_MANAGED_")
}

fn read_file(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_default()
}

fn normalize_paths(s: &str) -> String {
    s.replace('\\', "/")
}

/// 全量读入内存后消除行尾差异（CRLF → LF）的字节序列，用于逐字节比对。
///
/// 数据集夹具取自 Windows 端 NRMM 实测（`group_N.ini` 等为 CRLF），我方由 LF 模板
/// 生成同一内容（剥 CR 后逐字节相同），行尾不是本测试要钉的语义；比对统一在内存里
/// 折行尾，**磁盘字节保持原样**，不改生产端任何写入行为。
///
/// 只在 `\r` 后确实跟 `\n` 时丢弃 CR，其余字节原样保留，避免把真差异折成相等。
fn read_lf_bytes(p: &Path) -> Vec<u8> {
    let raw = fs::read(p).unwrap_or_default();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'\r' && raw.get(i + 1) == Some(&b'\n') {
            i += 1;
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

/// 规范化 INI 内容，忽略绝对路径 / 注释噪声 / $managed_slot_id 值，保留结构
fn normalize_ini_content(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_lowercase();
        if lower.starts_with("ini_path_absolute") {
            continue;
        }
        if lower.starts_with("global $managed_slot_id") {
            out.push_str("global $managed_slot_id = <SLOT>\n");
            continue;
        }
        if trimmed.starts_with(';') {
            if trimmed.contains("NRMM")
                || trimmed.contains("DISABLED")
                || trimmed.starts_with("; \"")
            {
                out.push_str(line.trim_end());
                out.push('\n');
            }
            continue;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.replace("\r\n", "\n")
}

/// 递归复制目录
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    // 确保源目录存在且为目录
    if !src.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is not a directory", src.display()),
        ));
    }

    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry.map_err(|e| std::io::Error::other(e.to_string()))?;

        // ✅ 核心：计算相对于源根的路径
        let relative: PathBuf = entry
            .path()
            .strip_prefix(src)
            .map_err(std::io::Error::other)?
            .to_path_buf();

        // 跳过根目录本身（relative == ""）
        if relative.as_os_str().is_empty() {
            continue;
        }

        let target = dst.join(&relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_symlink() {
            // 符号链接：读取原始目标并重建
            #[cfg(unix)]
            {
                let link_target = fs::read_link(entry.path())?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                let _ = fs::remove_file(&target);
                std::os::unix::fs::symlink(link_target, &target)?;
            }
            #[cfg(windows)]
            {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(entry.path(), &target)?;
            }
        } else {
            // 普通文件：确保父目录存在后复制
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target)?;
        }
    }

    Ok(())
}

/// 将输入数据集还原到持久化运行时目录。
/// 路径约定（与原版 NRMM / 集成测试一致）：
/// - d3dx.ini → <runtime>/d3dx.ini（游戏根目录）
/// - Mods/_MANAGED_ → <runtime>/Mods/_MANAGED_/
/// - 其余目录/文件 → <runtime>/Mods/<name>
///
/// 返回 game_mods_path（必须以 "Mods" 结尾）
fn restore_dataset_to(runtime: &Path) -> PathBuf {
    // fail-fast：remove 静默失败会让旧运行产物（manager_group.ini 等）残留，
    // 后续「运行前不应存在生成文件」断言确定性失败且极难排查（Windows 下
    // 目录被杀软/索引/上一进程句柄短暂占用时 remove_dir_all 会失败）。
    fs::remove_dir_all(runtime)
        .or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(e)
            }
        })
        .expect("清理运行时目录失败（可能被占用），请手动删除后重试");
    fs::create_dir_all(runtime).expect("创建运行时根目录失败");

    let src_root = input_dataset();
    copy_dir(&src_root, runtime).unwrap();

    runtime.join("Mods")
}

/// 验证测试目录确实发生变化：运行后必须生成以下文件（运行前不存在），
/// 且关键文件与 NRMM-test 基线语义一致。
fn assert_managed_outputs(managed: &Path) {
    // 1) 目录变化验证：以下文件在运行前不存在，运行后必须存在且非空
    for f in [
        "manager_group.ini",
        "nrmm_include.ini",
        "nrmm_keypress.txt",
        "selectedindex",
    ] {
        let p = managed.join(f);
        assert!(p.exists(), "[目录变化] 缺失生成文件: {}", f);
        let c = read_file(&p);
        assert!(!c.trim().is_empty(), "[目录变化] 生成文件为空: {}", f);
    }
    // group_1 下由 update_mod_data 生成的分组文件
    let g1_ini = managed.join("group_1").join("group_1.ini");
    let g1_sel = managed.join("group_1").join("selectedindex");
    assert!(g1_ini.exists(), "[目录变化] group_1.ini 应被生成");
    assert!(g1_sel.exists(), "[目录变化] group_1/selectedindex 应被生成");

    // 2) 与 NRMM-test 基线语义比对（已知对齐项）
    let base = baseline_managed();
    for f in ["nrmm_include.ini", "nrmm_keypress.txt"] {
        let a = normalize_ini_content(&normalize_paths(&read_file(&managed.join(f))));
        let e = normalize_ini_content(&normalize_paths(&read_file(&base.join(f))));
        assert_eq!(a, e, "[基线比对] {} 与 NRMM-test 不一致", f);
    }
    let g1 = normalize_ini_content(&normalize_paths(&read_file(&g1_ini)));
    let g1b = normalize_ini_content(&normalize_paths(&read_file(
        &base.join("group_1").join("group_1.ini"),
    )));
    assert_eq!(g1, g1b, "[基线比对] group_1.ini 与 NRMM-test 不一致");

    let sel = read_file(&managed.join("selectedindex")).trim().to_string();
    assert_eq!(sel, "0", "[基线比对] 根 selectedindex 应为 0");
}

/// 主测试：构造参数 → 调用 update_mod_data → 验证目录变化 + 基线一致
///
/// 使用持久化运行时目录，测试执行后可直接在磁盘检查
/// `src-tauri/tests/update_mod_data_runtime/` 下的 _MANAGED_ 产物。
///
/// **需要本地夹具**：读 `src-tauri/tests/datasets/NRMM-Rust-test`，该目录被 .gitignore 忽略，
/// 干净 checkout（CI）上不存在，`input_dataset()` 会 panic——这是正确行为（没夹具就不是这个测试，
/// 绝不假装通过）。因此默认跑批里 `#[ignore]` 它，CI 汇总显示为 ignored 而非 failed；
/// 本地跑法：`cargo test --lib -- --ignored`。
#[test]
#[ignore = "需要本地 tests/datasets 夹具（已 gitignore）；跑法：cargo test --lib -- --ignored"]
fn update_mod_data_test() {
    let _runtime_lock = RUNTIME_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    println!("\n=== update_mod_data_test（集成到 mod_commands.rs）===");
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(RUNTIME_DIR);
    let game_mods_path = restore_dataset_to(&runtime);
    let managed = game_mods_path.join(constants::MANAGED_FOLDER);

    // 运行前快照：输入数据集仅有 group_1 目录，尚无生成文件
    assert!(
        !managed.join("manager_group.ini").exists(),
        "运行前不应存在 manager_group.ini"
    );
    assert!(
        !managed.join("nrmm_include.ini").exists(),
        "运行前不应存在 nrmm_include.ini"
    );

    // 构造参数并调用 update_mod_data（核心函数，命令层仅为其异步包装）
    let settings = AppSettings::default();
    // 注意：NRMM-test 基线由 NRMM 以 Zenless（nrmm_name = "Zenless_Zone_Zero"）生成，
    // keypress 模板 `{game}` 替换依赖游戏标识，故此处必须用同款游戏以保证 parity 比对一致。
    let result = crate::core::mod_update_pipeline::update_mod_data(
        TargetGame::ZZZ,
        &game_mods_path,
        &settings,
    )
    .expect("update_mod_data 执行失败");

    println!(
        "  [result] enabled={} disabled={} processed={} errors={} groups={}",
        result.enabled_mods,
        result.disabled_mods,
        result.processed_mods,
        result.errors.len(),
        result.total_groups
    );

    // 验证返回结果合理
    assert!(
        result.processed_mods >= 1,
        "至少应处理 1 个模组，实际 {}",
        result.processed_mods
    );

    // 验证测试目录确实发生变化（生成产物存在且与基线一致）
    assert_managed_outputs(&managed);

    println!("  [PASS] update_mod_data_test");
    println!("  运行时产物目录: {:?}", runtime);
    println!("=== update_mod_data_test 完成 ===\n");
}

/// 场景：空 Mods 目录（无模组）
#[test]
fn update_mod_data_test_empty_mods() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mods = tmp.path().join("Mods");
    fs::create_dir_all(&mods).unwrap();
    let managed = mods.join(constants::MANAGED_FOLDER);
    fs::create_dir_all(&managed).unwrap();

    let settings = AppSettings::default();
    let result = crate::core::mod_update_pipeline::update_mod_data(
        TargetGame::GenshinImpact,
        &mods,
        &settings,
    )
    .expect("update_mod_data 失败");
    assert_eq!(result.processed_mods, 0, "空目录不应处理任何模组");
    // 仍应生成 managed 基础文件（目录发生变化）
    assert!(managed.join("nrmm_include.ini").exists());
    assert!(managed.join("nrmm_keypress.txt").exists());
    assert!(managed.join("manager_group.ini").exists());
    assert!(managed.join("selectedindex").exists());
    println!("[PASS] update_mod_data_test_empty_mods");
}

/// 场景：DISABLED 模组被跳过
#[test]
fn update_mod_data_test_disabled_mod_skipped() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mods = tmp.path().join("Mods");
    fs::create_dir_all(&mods).unwrap();
    let managed = mods.join(constants::MANAGED_FOLDER);
    fs::create_dir_all(&managed).unwrap();

    let group_dir = managed.join("group_1");
    fs::create_dir_all(&group_dir).unwrap();
    fs::write(
        group_dir.join("group_1.ini"),
        ";template\n[Constants]\nglobal $group_id = 1\n",
    )
    .unwrap();

    // 启用模组
    let enabled = group_dir.join("EnabledMod");
    fs::create_dir_all(&enabled).unwrap();
    fs::write(
        enabled.join("mod.ini"),
        "[TextureOverride]\nhash = 0xAAAA\n",
    )
    .unwrap();

    // 禁用模组（DISABLED 前缀）
    let disabled = group_dir.join("DISABLED_TestMod");
    fs::create_dir_all(&disabled).unwrap();
    fs::write(
        disabled.join("mod.ini"),
        "[TextureOverride]\nhash = 0xBBBB\n",
    )
    .unwrap();

    let settings = AppSettings::default();
    let result = crate::core::mod_update_pipeline::update_mod_data(
        TargetGame::GenshinImpact,
        &mods,
        &settings,
    )
    .expect("update_mod_data 失败");
    assert_eq!(result.enabled_mods, 1, "应只有 1 个启用模组");
    assert_eq!(result.disabled_mods, 1, "应有 1 个禁用模组");
    assert_eq!(result.processed_mods, 1, "应仅处理启用的模组");
    println!("[PASS] update_mod_data_test_disabled_mod_skipped");
}

/// 场景：多分组更新
#[test]
fn update_mod_data_test_multiple_groups() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mods = tmp.path().join("Mods");
    fs::create_dir_all(&mods).unwrap();
    let managed = mods.join(constants::MANAGED_FOLDER);
    fs::create_dir_all(&managed).unwrap();

    for g in 1..=2u32 {
        let gd = managed.join(format!("group_{}", g));
        fs::create_dir_all(&gd).unwrap();
        fs::write(
            gd.join(format!("group_{}.ini", g)),
            format!(";t\n[Constants]\nglobal $group_id = {}\n", g),
        )
        .unwrap();
        for m in 1..=2u32 {
            let md = gd.join(format!("Mod_{}", m));
            fs::create_dir_all(&md).unwrap();
            fs::write(
                md.join("mod.ini"),
                format!(
                    "[TextureOverride_G{}_M{}]\nhash = 0x{:08X}\n",
                    g,
                    m,
                    0x12345678 + g * 256 + m * 16
                ),
            )
            .unwrap();
        }
    }

    let settings = AppSettings::default();
    let result = crate::core::mod_update_pipeline::update_mod_data(
        TargetGame::GenshinImpact,
        &mods,
        &settings,
    )
    .expect("update_mod_data 失败");
    assert!(result.total_groups >= 2, "应检测到至少 2 个分组");
    assert!(result.processed_mods >= 4, "应处理至少 4 个模组");
    assert!(managed.join("group_1/group_1.ini").exists());
    assert!(managed.join("group_2/group_2.ini").exists());
    println!("[PASS] update_mod_data_test_multiple_groups");
}

/// 场景：幂等性（连续两次更新结果一致）
#[test]
fn update_mod_data_test_idempotent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mods = tmp.path().join("Mods");
    fs::create_dir_all(&mods).unwrap();
    let managed = mods.join(constants::MANAGED_FOLDER);
    fs::create_dir_all(&managed).unwrap();

    let group_dir = managed.join("group_1");
    fs::create_dir_all(&group_dir).unwrap();
    fs::write(
        group_dir.join("group_1.ini"),
        ";template\n[Constants]\nglobal $group_id = 1\n",
    )
    .unwrap();
    let en = group_dir.join("EnabledMod");
    fs::create_dir_all(&en).unwrap();
    fs::write(en.join("mod.ini"), "[TextureOverride]\nhash = 0xAAAA\n").unwrap();

    let settings = AppSettings::default();
    let r1 = crate::core::mod_update_pipeline::update_mod_data(
        TargetGame::GenshinImpact,
        &mods,
        &settings,
    )
    .expect("第一次 update_mod_data 失败");
    let r2 = crate::core::mod_update_pipeline::update_mod_data(
        TargetGame::GenshinImpact,
        &mods,
        &settings,
    )
    .expect("第二次 update_mod_data 失败");
    assert_eq!(
        r1.processed_mods, r2.processed_mods,
        "两次更新 processed_mods 应一致"
    );
    assert_eq!(
        r1.enabled_mods, r2.enabled_mods,
        "两次更新 enabled_mods 应一致"
    );
    println!("[PASS] update_mod_data_test_idempotent");
}

// =====================================================================
// 还原数据测试（update 目录 → 还原 → 全树比对）
//
// 流程：输入数据集 → update_mod_data 注入 → restore_all_inis 还原 →
//   1) 还原数据 = 注入前原始内容（与输入数据集内容一致；行尾差异经 read_lf_bytes 归一）
//   2) 管理生成文件被清理（manager_group/nrmm_include/keypress/group_X.ini/备份）
//   3) 还原安全窗口内重扫不新建管理标记（groupname/modname/selectedindex）
//
// 持有 TEST_WINDOW_LOCK：restore_all_inis 内部开启还原安全窗口（全局状态），
// 与依赖标记创建的并发测试互斥；结束时显式清除窗口，防止 60s 泄漏。
//
// 与 update_mod_data_test 同：需要 tests/datasets 夹具（gitignore，CI 上不存在），
// 缺夹具即 panic 属正确行为，故默认跑批 #[ignore]，本地用 `-- --ignored` 显式跑。
// =====================================================================
#[test]
#[ignore = "需要本地 tests/datasets 夹具（已 gitignore）；跑法：cargo test --lib -- --ignored"]
fn restore_after_update_test() {
    use std::collections::BTreeSet;
    use walkdir::WalkDir;

    // 共享运行时目录互斥：与 update_mod_data_test 并行时不得互删同一目录
    let _runtime_lock = RUNTIME_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // 窗口全局状态互斥（含 mod_scanner 内的标记创建断言测试）
    let _window_lock = crate::core::mod_scan_marker::TEST_WINDOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // panic 安全：测试退出（含断言失败）即清除窗口
    struct ClearWindow;
    impl Drop for ClearWindow {
        fn drop(&mut self) {
            crate::core::mod_scan_marker::test_clear_restore_safe_window();
        }
    }
    let _clear = ClearWindow;

    println!("\n=== restore_after_update_test ===");
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(RUNTIME_DIR);
    let game_mods_path = restore_dataset_to(&runtime);
    let managed = game_mods_path.join(constants::MANAGED_FOLDER);
    let input_root = input_dataset();

    // 收集管理标记文件相对路径清单（groupname/modname/selectedindex）
    fn marker_inventory(root: &Path) -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for entry in WalkDir::new(root).follow_links(false) {
            let e = entry.unwrap();
            if !e.file_type().is_file() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name == "groupname" || name == "modname" || name == "selectedindex" {
                let rel = e
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                set.insert(rel);
            }
        }
        set
    }

    // 1) 注入（与 update_mod_data_test 同款参数保证 parity）
    let settings = AppSettings::default();
    let result = crate::core::mod_update_pipeline::update_mod_data(
        TargetGame::ZZZ,
        &game_mods_path,
        &settings,
    )
    .expect("update_mod_data 执行失败");
    assert!(result.processed_mods >= 1, "至少应处理 1 个模组");
    assert_managed_outputs(&managed);

    let markers_before = marker_inventory(&runtime);

    // 2) 还原
    let restored = crate::core::mod_restore::restore_all_inis(&game_mods_path)
        .expect("restore_all_inis 执行失败");
    println!(
        "  [restore] restored={} failed={}",
        restored.restored, restored.failed
    );
    assert!(restored.restored >= 1, "至少应还原 1 个 INI");
    assert_eq!(restored.failed, 0, "还原不应有失败项");

    // 3) 管理生成文件在还原后保留（对齐 NRMM restoreManagedMod：仅还原模组 INI，
    //    不清理管理器文件——2026-09-26 用户决策，废除移植端原有的还原全清理语义）
    for f in ["manager_group.ini", "nrmm_include.ini", "nrmm_keypress.txt"] {
        assert!(
            managed.join(f).exists(),
            "[对齐NRMM] 还原后管理文件缺失: {}",
            f
        );
    }
    // group_N.ini 生成于组目录内（managed/group_1/group_1.ini），非 _MANAGED_ 根
    assert!(
        managed.join("group_1").join("group_1.ini").exists(),
        "[对齐NRMM] 还原后管理文件缺失: group_1/group_1.ini"
    );
    // 注入备份应全部被消费；例外 = 输入数据集自带的孤儿备份
    // （原 .ini 已被用户删除而备份残留，如 bakup (2).ini_managed_backup，
    //  Dart 基线同样保留 → 非本次还原产生，不得误报）
    let mut leftover_backups = Vec::new();
    for entry in WalkDir::new(&runtime).follow_links(false) {
        let e = entry.unwrap();
        if e.file_type().is_file()
            && e.file_name()
                .to_string_lossy()
                .ends_with(constants::BACKUP_EXTENSION)
        {
            leftover_backups.push(e.path().to_path_buf());
        }
    }
    let orphan_backups: Vec<&PathBuf> = leftover_backups
        .iter()
        .filter(|p| !input_root.join(p.strip_prefix(&runtime).unwrap()).exists())
        .collect();
    assert!(
        orphan_backups.is_empty(),
        "还原残留非输入数据集自带的注入备份: {:?}",
        orphan_backups
    );

    // 4) 还原数据正确性：全树与输入数据集比对
    //    语义：还原 = 回到「注入前原始内容」。
    //    - 输入无备份 → 期望运行时 == 输入（update 注入后还原应回到原样）
    //    - 输入自带备份 → 输入文件本身是上次 NRMM 会话的已注入形态，
    //      期望运行时 == 输入备份内容（真·原始；备份已存在时 update 不覆盖，
    //      还原回到备份记录的原始版本，Dart 同语义）
    //    备份命名与 mod_manager 一致：Path::with_extension（替换 .ini 后缀），
    //    即 X.ini → X.ini_managed_backup（非追加）。
    //    行尾口径：双方都经 read_lf_bytes 全量读入内存，仅把 CRLF 折成 LF，其余字节
    //    严格相等——夹具是 Windows 端 NRMM 实测（CRLF），我方由 LF 模板生成同内容。
    let mut content_diff: Vec<String> = Vec::new();
    for entry in WalkDir::new(&input_root).follow_links(false) {
        let e = entry.unwrap();
        if !e.file_type().is_file() {
            continue;
        }
        let rel = e.path().strip_prefix(&input_root).unwrap();
        let rt_path = runtime.join(rel);
        let file_name = e.file_name().to_string_lossy().to_string();
        let parent = e.path().parent().unwrap();
        let backup_suffix = format!(".{}", constants::BACKUP_EXTENSION);

        // 输入自带的备份文件（X.ini_managed_backup）：
        //   主文件存在 → 还原消费备份（拷回主文件后删除）→ 期望运行时缺失
        //   主文件不存在（孤儿备份，如 bakup (2).ini_managed_backup）→ 原样保留
        if file_name.ends_with(&backup_suffix) {
            // 主文件名反推：with_extension 是「替换 .ini」→ Herta.ini_managed_backup
            // 的主文件是 Herta + ".ini" = Herta.ini（restore 仅对 .ini 建备份）
            let main_name = format!("{}.ini", file_name.trim_end_matches(&backup_suffix));
            if parent.join(&main_name).exists() {
                if rt_path.exists() {
                    content_diff.push(format!("备份应被还原消费但仍存在: {}", rel.display()));
                }
            } else if !rt_path.exists() {
                content_diff.push(format!("孤儿备份运行时缺失: {}", rel.display()));
            } else {
                let a = read_lf_bytes(e.path());
                let b = read_lf_bytes(&rt_path);
                if a != b {
                    content_diff.push(format!("孤儿备份内容不一致: {}", rel.display()));
                }
            }
            continue;
        }

        if !rt_path.exists() {
            content_diff.push(format!("运行时缺失: {}", rel.display()));
            continue;
        }

        // 非备份文件：仅 .ini 参与注入/还原（对齐 restore_inis_recursive 条件）；
        // 输入自带同主名备份 → 期望还原到备份内容
        let is_ini = Path::new(&file_name)
            .extension()
            .map(|x| x.eq_ignore_ascii_case("ini"))
            .unwrap_or(false)
            && !constants::is_desktop_ini(e.path());
        let backup_path = Path::new(&file_name)
            .with_extension(constants::BACKUP_EXTENSION)
            .to_string_lossy()
            .to_string();
        let has_backup = is_ini && parent.join(&backup_path).exists();
        let expected = if has_backup {
            parent.join(&backup_path)
        } else {
            e.path().to_path_buf()
        };
        let a = read_lf_bytes(&expected);
        let b = read_lf_bytes(&rt_path);
        if a != b {
            content_diff.push(format!(
                "内容不一致: {}（期望 {}）",
                rel.display(),
                if has_backup {
                    "输入备份内容（注入前原始）".to_string()
                } else {
                    "输入内容".to_string()
                }
            ));
        }
    }
    assert!(
        content_diff.is_empty(),
        "还原后与输入数据集存在差异（{} 处）:\n{}",
        content_diff.len(),
        content_diff.join("\n")
    );

    let mut extra_files: Vec<String> = Vec::new();
    for entry in WalkDir::new(&runtime).follow_links(false) {
        let e = entry.unwrap();
        if !e.file_type().is_file() {
            continue;
        }
        let rel = e.path().strip_prefix(&runtime).unwrap();
        if !input_root.join(rel).exists() {
            extra_files.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    // 白名单：update 生成、还原按 NRMM 对齐语义（restoreManagedMod 不动管理器文件）
    // 合法存续的管理器生成文件——输入数据集未必携带（清洗后输入仅含历史形态）：
    // - 根/组级 selectedindex
    // - 根级三件套：manager_group.ini / nrmm_include.ini / nrmm_keypress.txt
    // - group_N.ini（任意层级；仅含启用模组的分组才生成）
    let extra_unexpected: Vec<&String> = extra_files
        .iter()
        .filter(|p| {
            let name = p.rsplit('/').next().unwrap_or("");
            !(name == "selectedindex"
                || name == "manager_group.ini"
                || name == "nrmm_include.ini"
                || name == "nrmm_keypress.txt"
                || (name.starts_with("group_") && name.ends_with(".ini")))
        })
        .collect();
    assert!(
        extra_unexpected.is_empty(),
        "还原后存在白名单之外的运行时新增文件: {:?}",
        extra_unexpected
    );
    println!(
        "  [extra] 白名单内新增文件 {} 个: {:?}",
        extra_files.len(),
        extra_files
    );

    // 5) 还原安全窗口内重扫：不新建任何管理标记
    //    （模拟 watcher 防抖 1s 后的重扫，窗口 60s 内必然处于活跃态）
    crate::core::mod_scan_light::scan_mods_light(&game_mods_path).expect("还原后重扫失败");
    let markers_after = marker_inventory(&runtime);
    let new_markers: Vec<&String> = markers_after.difference(&markers_before).collect();
    assert!(
        new_markers.is_empty(),
        "还原安全窗口内重扫新建了管理标记（违反还原不回写语义）: {:?}",
        new_markers
    );

    println!("  [PASS] restore_after_update_test");
    println!("  运行时产物目录: {:?}", runtime);
    println!("=== restore_after_update_test 完成 ===\n");
}

// 命令级 Delta 组装（R1）测试见 mod_commands_delta_tests.rs（同由 mod_commands.rs 以
// `#[path] mod delta_tests;` 挂载）。
