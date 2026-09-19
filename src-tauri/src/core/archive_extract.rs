//! 压缩包处理 —— 解压实现 / 三级回退编排 / 后处理域
//!
//! 从 archive_handler.rs 拆出（2026-09-19 剩余巨石拆分 PA-3）。
//! 承载 7z CLI / 自维护解压、同盘判定、重名处理、临时区清理与解压后
//! 原子移动到最终位置（含单层包裹展平编排）。类型契约与格式探测见
//! [`crate::core::archive_detect`]，扁平化/导入见 [`crate::core::archive_import`]，
//! 导出见 [`crate::core::archive_export`]。

use crate::core::archive_detect::{
    detect_archive_type_robust, get_bundled_7z_path, get_system_7z_path, ArchiveType, ExtractResult,
};
use crate::core::archive_import::{copy_dir_deep, unwrap_single_folder_nesting, visit_files};
use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 判断两个路径是否在同一磁盘（卷）上
///
/// 用于决定目录导入策略：同盘使用 `move_dir`（快速重命名），跨盘使用 `copy` + `trash::delete`。
///
/// Windows：比较路径的 `Prefix` 组件（盘符/卷GUID/UNC 路径），忽略大小写
/// 非 Windows：保守返回 `false`，调用方将使用 copy + trash 策略
///
/// # 参数
/// - `a`: 第一个路径
/// - `b`: 第二个路径
///
/// # 返回
/// `true` 表示在同一磁盘上
pub fn paths_on_same_disk(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use std::path::Prefix;
        fn get_prefix(path: &Path) -> Option<String> {
            let comp = path.components().next()?;
            if let std::path::Component::Prefix(prefix) = comp {
                let kind = prefix.kind();
                match kind {
                    Prefix::Disk(disk) => {
                        return Some(format!("disk_{}", (disk as char).to_ascii_lowercase()));
                    }
                    Prefix::VerbatimDisk(disk) => {
                        return Some(format!("vdisk_{}", (disk as char).to_ascii_lowercase()));
                    }
                    Prefix::UNC(server, share) => {
                        let s: String = server
                            .encode_wide()
                            .chain(share.encode_wide())
                            .map(|c| (c as u32).to_string())
                            .collect();
                        return Some(format!("unc_{}", s.to_ascii_lowercase()));
                    }
                    Prefix::VerbatimUNC(server, share) => {
                        let s: String = server
                            .encode_wide()
                            .chain(share.encode_wide())
                            .map(|c| (c as u32).to_string())
                            .collect();
                        return Some(format!("vunc_{}", s.to_ascii_lowercase()));
                    }
                    Prefix::Verbatim(_) | Prefix::DeviceNS(_) => return None,
                }
            }
            None
        }
        let pa = get_prefix(a);
        let pb = get_prefix(b);
        pa.is_some() && pa == pb
    }
    #[cfg(not(windows))]
    {
        let _ = (a, b);
        false
    }
}

/// 使用 7z CLI 解压压缩包到临时目录
///
/// 提取 `extract_archive` 中 7z CLI 调用逻辑为独立函数，供三级回退链复用。
/// 仅负责调用 7z 命令并返回临时目录路径，不执行后续的扁平化、统计、移动等操作。
///
/// # 参数
/// - `seven_zip_path`: 7z 可执行文件路径（系统 PATH 中的命令名或内置二进制完整路径）
/// - `archive_path`: 压缩包路径
/// - `tmp_parent`: 临时目录的父目录（解压临时区，如 `<Mods>/.tmp`）
/// - `password`: 可选压缩包密码，`None` 表示尝试无密码解压
///
/// # 返回
/// 成功时返回临时目录 `PathBuf`，失败时返回错误
///
/// # Errors
/// - 临时目录创建失败
/// - 7z 命令执行失败（密码错误返回 `PasswordRequired` 变体错误消息）
/// - 7z 命令返回非零退出码
fn extract_with_7z_cli(
    seven_zip_path: &Path,
    archive_path: &Path,
    tmp_parent: &Path,
    password: Option<&str>,
) -> Result<PathBuf> {
    let temp_dir = tmp_parent.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    let mut cmd = Command::new(seven_zip_path);
    cmd.arg("x")
        .arg("-y")
        .arg(format!("-o{}", temp_dir.to_string_lossy()));

    if let Some(pw) = password {
        let sanitized: String = pw.chars().filter(|c| !c.is_control()).collect();
        if sanitized.is_empty() {
            cmd.arg("-p");
        } else {
            cmd.arg(format!("-p{}", sanitized));
        }
    } else {
        cmd.arg("-p");
    }

    cmd.arg(archive_path.to_string_lossy().to_string());

    let output = cmd.output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, stderr);

    if !output.status.success() {
        if combined.contains("password")
            || combined.contains("Wrong password")
            || combined.contains("Encrypted")
        {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(anyhow!("PASSWORD_REQUIRED"));
        }
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(anyhow!("7z CLI extraction failed: {}", combined));
    }

    Ok(temp_dir)
}

/// 安全拼接 7z 条目名到目标目录，防止路径穿越（zip-slip / 7z-slip，RUSTSEC-2026-0245）
///
/// 拒绝含 `..`、绝对路径前缀（`/`、`C:`）、反斜杠逃逸的条目名。
/// 反斜杠统一按分隔符处理（Windows 兼容），仅保留 `Normal` 组件。
fn safe_join_entry(base: &Path, name: &str) -> std::result::Result<PathBuf, sevenz_rust::Error> {
    if name.is_empty() {
        return Err(sevenz_rust::Error::Other("empty 7z entry name".into()));
    }
    // 反斜杠统一为正斜杠，避免 Windows 下 `\..` 被 components 当作普通文件名段
    let normalized = name.replace('\\', "/");
    let p = Path::new(&normalized);
    let mut safe = base.to_path_buf();
    for comp in p.components() {
        match comp {
            std::path::Component::Normal(s) => safe = safe.join(s),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(sevenz_rust::Error::Other(
                    format!(
                        "unsafe 7z entry name rejected (traversal/absolute/prefix): {}",
                        name
                    )
                    .into(),
                ));
            }
        }
    }
    Ok(safe)
}

/// 使用 `sevenz-rust` crate 自实现解压 7z 压缩包到临时目录
///
/// 三级回退链的第三级（7z 格式）：纯 Rust 实现的 7z 解压，不依赖外部 CLI。
/// 支持密码保护的压缩包。
///
/// # 安全
/// 使用自定义 extract 回调 + `safe_join_entry` 对每个条目名做路径穿越净化，
/// 修复 RUSTSEC-2026-0245（sevenz-rust 0.6.1 `decompress_file` 内部
/// `dest.join(entry.name())` 无净化，恶意条目可覆盖任意可写文件）。
///
/// # 参数
/// - `archive_path`: 7z 压缩包路径
/// - `tmp_parent`: 临时目录的父目录（解压临时区，如 `<Mods>/.tmp`）
/// - `password`: 可选压缩包密码
///
/// # 返回
/// 成功时返回临时目录 `PathBuf`
///
/// # Errors
/// - 临时目录创建失败
/// - `sevenz_rust::decompress_file_with_extract_fn` 解压失败
fn extract_7z_internal(
    archive_path: &Path,
    tmp_parent: &Path,
    password: Option<&str>,
) -> Result<PathBuf> {
    let temp_dir = tmp_parent.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    log::info!(
        "extract_7z_internal: falling back to sevenz-rust for {:?}",
        archive_path
    );

    if password.is_some() {
        // sevenz-rust2 未启用 aes256 feature：密码保护的 7z 应在前两级 7z CLI 中处理
        return Err(anyhow!(
            "Password-protected 7z not supported by internal extractor"
        ));
    }

    let dest_base = temp_dir.clone();
    sevenz_rust::decompress_file_with_extract_fn(
        archive_path,
        &temp_dir,
        |entry, reader, _unsafe_path| {
            // 跳过空名条目：sevenz-rust 自维护压缩产物可能含空名根/目录占位，
            // 无法安全映射路径且无实际文件数据，跳过不丢失内容、无穿越风险。
            if entry.name().is_empty() {
                return Ok(true);
            }
            // 安全修复（RUSTSEC-2026-0245）：忽略 decompress_impl 内部已 join 的
            // _unsafe_path（其用 dest.join(entry.name())，对含 `..` 的条目名会穿越），
            // 改用 entry.name() 重新做安全 join，仅保留 Normal 组件。
            let safe_path = safe_join_entry(&dest_base, entry.name())?;
            if entry.is_directory() {
                if !safe_path.exists() {
                    fs::create_dir_all(&safe_path)
                        .map_err(|e| sevenz_rust::Error::Io(e, "".into()))?;
                }
            } else {
                if let Some(parent) = safe_path.parent() {
                    if !parent.exists() {
                        fs::create_dir_all(parent)
                            .map_err(|e| sevenz_rust::Error::Io(e, "".into()))?;
                    }
                }
                let mut file = fs::File::create(&safe_path).map_err(|e| {
                    sevenz_rust::Error::Io(
                        e,
                        format!("create file: {}", safe_path.display()).into(),
                    )
                })?;
                std::io::copy(reader, &mut file)
                    .map_err(|e| sevenz_rust::Error::Io(e, "".into()))?;
            }
            Ok(true)
        },
    )
    .context("sevenz-rust decompress_file failed (with path traversal protection)")?;

    Ok(temp_dir)
}

/// 使用 `zip` crate 自实现解压 ZIP 压缩包到临时目录
///
/// 三级回退链的第三级（ZIP 格式）：纯 Rust 实现的 ZIP 解压，不依赖外部 CLI。
/// 注意：自实现 ZIP 解压不支持密码保护（密码保护的 ZIP 应在前两级 7z CLI 中处理）。
///
/// # 参数
/// - `archive_path`: ZIP 压缩包路径
/// - `tmp_parent`: 临时目录的父目录（解压临时区，如 `<Mods>/.tmp`）
///
/// # 返回
/// 成功时返回临时目录 `PathBuf`
///
/// # Errors
/// - 临时目录创建失败
/// - 文件打开失败
/// - `zip::ZipArchive::extract` 解压失败
fn extract_zip_internal(archive_path: &Path, tmp_parent: &Path) -> Result<PathBuf> {
    let temp_dir = tmp_parent.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    log::info!(
        "extract_zip_internal: falling back to zip crate for {:?}",
        archive_path
    );

    let file =
        fs::File::open(archive_path).context("extract_zip_internal: failed to open archive")?;
    let mut archive =
        zip::ZipArchive::new(file).context("extract_zip_internal: failed to read ZIP archive")?;
    archive
        .extract(&temp_dir)
        .context("extract_zip_internal: ZIP extraction failed")?;

    Ok(temp_dir)
}

/// 使用 `unrar` crate 自实现解压 RAR 压缩包到临时目录
///
/// 三级回退链的第三级（RAR 格式）：基于 UnRAR C 库的 RAR 解压，不依赖外部 CLI。
/// 注意：`unrar` crate 仅支持解压，不支持创建 RAR 压缩包。
///
/// # 参数
/// - `archive_path`: RAR 压缩包路径
/// - `tmp_parent`: 临时目录的父目录（解压临时区，如 `<Mods>/.tmp`）
/// - `password`: 可选压缩包密码
///
/// # 返回
/// 成功时返回临时目录 `PathBuf`
///
/// # Errors
/// - 临时目录创建失败
/// - `unrar::Archive` 打开或解压失败
fn extract_rar_internal(
    archive_path: &Path,
    tmp_parent: &Path,
    password: Option<&str>,
) -> Result<PathBuf> {
    let temp_dir = tmp_parent.join(format!(".extract_tmp_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    log::info!(
        "extract_rar_internal: falling back to unrar crate for {:?}",
        archive_path
    );

    let mut archive = if let Some(pw) = password {
        unrar::Archive::with_password(archive_path, pw)
            .open_for_processing()
            .context("extract_rar_internal: failed to open RAR archive with password")?
    } else {
        unrar::Archive::new(archive_path)
            .open_for_processing()
            .context("extract_rar_internal: failed to open RAR archive")?
    };

    // 使用状态机模式逐文件解压
    while let Some(header) = archive
        .read_header()
        .context("extract_rar_internal: failed to read RAR header")?
    {
        archive = if header.entry().is_file() {
            header
                .extract_with_base(&temp_dir)
                .context("extract_rar_internal: failed to extract RAR entry")?
        } else {
            header
                .skip()
                .context("extract_rar_internal: failed to skip RAR entry")?
        };
    }

    Ok(temp_dir)
}

/// 解压压缩包到目标目录（三级回退链）
///
/// # 三级回退链设计
///
/// 按优先级依次尝试以下解压方式，任一成功即返回：
///
/// **第一级：系统 7z CLI**（最高优先级）
/// - 搜索系统 PATH 中的 7z 命令（`7z.exe`/`7zz`/`7za`/`7zr`/`7z`）
/// - 用户自行安装的 7z 版本更新、性能更好
///
/// **第二级：内置打包 7z CLI**（中级回退）
/// - 使用应用 bundle 内打包的 7z 二进制（`resources/7z/...`）
/// - 当用户系统无 7z 或版本过低导致解压失败时使用
///
/// **第三级：自实现解压**（最终回退）
/// - 7z 格式 → `sevenz-rust` crate（纯 Rust 实现）
/// - ZIP 格式 → `zip` crate（纯 Rust 实现）
/// - RAR 格式 → `unrar` crate（基于 UnRAR C 库，仅解压）
///
/// # 解压后处理
///
/// 无论使用哪一级解压，成功后均执行以下步骤：
/// 1. **展平单层包裹目录**：`unwrap_single_folder_nesting` 处理压缩包内单根目录情况（循环展平 + 排除元数据文件）
/// 2. **统计文件数 & 检查 INI**：`visit_files` 递归遍历
/// 3. **移动到最终位置**：`unique_mod_dir_name` 处理重名，`move_temp_to_target` 同盘原子移动
/// 4. **清理临时目录**：`remove_dir_all`
///
/// # 参数
/// - `archive_path`: 压缩包路径
/// - `target_dir`: 目标父目录（解压后在此目录下创建模组目录）
/// - `password`: 可选压缩包密码，`None` 表示尝试无密码解压
///
/// # 返回
/// `ExtractResult` 枚举，包含成功或各类失败详情
///
/// # Panics
/// 不会 panic
///
/// # Errors
/// - 所有三级解压均失败时返回 `ExtractFailed`
/// - 密码错误时返回 `PasswordRequired`
pub fn extract_archive(
    archive_path: &Path,
    target_dir: &Path,
    password: Option<&str>,
) -> Result<ExtractResult> {
    let archive_type = detect_archive_type_robust(archive_path);

    if let ArchiveType::Unsupported(ext) = archive_type {
        return Ok(ExtractResult::UnsupportedFormat {
            ext,
            message: "Unsupported archive format. Please manually extract .zip/.rar/.7z files."
                .to_string(),
        });
    }

    // 解压临时区：`<Mods>/.tmp`（隐藏 dotdir，扫描器自动跳过），与分组目录同处一卷，
    // 保证后续「移动到分组目录」为同盘原子 rename（高效且原子，避免解压中断污染分组目录）。
    let tmp_parent = extract_tmp_dir(target_dir);

    let mut last_error: Option<String> = None;

    // --- 第一级：系统 7z CLI ---
    if let Some(sys_7z) = get_system_7z_path() {
        match extract_with_7z_cli(&sys_7z, archive_path, &tmp_parent, password) {
            Ok(temp_dir) => {
                log::info!(
                    "extract_archive: system 7z succeeded for {:?}",
                    archive_path
                );
                return finalize_extraction(archive_path, target_dir, &temp_dir);
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("PASSWORD_REQUIRED") {
                    return Ok(ExtractResult::PasswordRequired {
                        archive_path: archive_path.to_path_buf(),
                    });
                }
                log::warn!("extract_archive: system 7z failed: {}", msg);
                last_error = Some(format!("system 7z: {}", msg));
            }
        }
    }

    // --- 第二级：内置打包 7z CLI ---
    if let Some(bundled_7z) = get_bundled_7z_path() {
        match extract_with_7z_cli(&bundled_7z, archive_path, &tmp_parent, password) {
            Ok(temp_dir) => {
                log::info!(
                    "extract_archive: bundled 7z succeeded for {:?}",
                    archive_path
                );
                return finalize_extraction(archive_path, target_dir, &temp_dir);
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("PASSWORD_REQUIRED") {
                    return Ok(ExtractResult::PasswordRequired {
                        archive_path: archive_path.to_path_buf(),
                    });
                }
                log::warn!("extract_archive: bundled 7z failed: {}", msg);
                last_error = Some(format!("bundled 7z: {}", msg));
            }
        }
    }

    // --- 第三级：自实现解压（按格式分派） ---
    let internal_result = match archive_type {
        ArchiveType::SevenZip => extract_7z_internal(archive_path, &tmp_parent, password),
        ArchiveType::Zip => {
            if password.is_some() {
                // 自实现 ZIP 不支持密码，直接返回错误
                Err(anyhow!("ZIP password not supported by internal extractor"))
            } else {
                extract_zip_internal(archive_path, &tmp_parent)
            }
        }
        ArchiveType::Rar => extract_rar_internal(archive_path, &tmp_parent, password),
        _ => unreachable!(),
    };

    match internal_result {
        Ok(temp_dir) => {
            log::info!(
                "extract_archive: internal extractor succeeded for {:?}",
                archive_path
            );
            finalize_extraction(archive_path, target_dir, &temp_dir)
        }
        Err(e) => {
            let msg = e.to_string();
            log::error!(
                "extract_archive: all tiers failed for {:?}: {}",
                archive_path,
                msg
            );
            let combined = if let Some(prev) = last_error {
                format!("{}; internal: {}", prev, msg)
            } else {
                format!("internal: {}", msg)
            };
            Ok(ExtractResult::ExtractFailed {
                message: format!("All extraction methods failed: {}", combined),
            })
        }
    }
}

/// 解析目标分组目录对应的「游戏 Mods 根目录」（用于放置 `.tmp` 临时解压区）。
///
/// `target_dir` 形如 `<Mods>/_MANAGED_/group_X` → Mods 根为其祖父目录；
/// 若层级不足则回退为 `target_dir` 自身。
pub(crate) fn mods_root_of(target_dir: &Path) -> PathBuf {
    target_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| target_dir.to_path_buf())
}

/// 解压临时区目录：`<Mods>/.tmp`（隐藏 dotdir，扫描器自动跳过），
/// 与分组目录同处 Mods 卷，保证后续移动到分组目录为同盘原子 rename。
fn extract_tmp_dir(target_dir: &Path) -> PathBuf {
    mods_root_of(target_dir).join(".tmp")
}

/// 生成唯一模组目录名（Windows 资源管理器风格）：若 `base` 不存在返回 `base`，
/// 否则依次尝试 `base (1)`、`base (2)` ... 直达找到不存在的名称。
///
/// 用于导入解压后移动到分组目录时的重名处理（对齐 `{modname} (1)` 语义）：
/// 若目标分组目录下已存在同名模组，则对移动后的模组重命名以避免覆盖。
fn unique_mod_dir_name(parent: &Path, base: &str) -> String {
    if !parent.join(base).exists() {
        return base.to_string();
    }
    let mut counter = 1u32;
    loop {
        let name = format!("{} ({})", base, counter);
        if !parent.join(&name).exists() {
            return name;
        }
        counter += 1;
    }
}

/// 将解压临时目录整体移动到最终模组目录：同盘优先 `rename`（原子、高效），
/// 失败回退 `copy_dir_deep` + 删除源。目标由调用方保证不冲突（unique_mod_dir_name）。
fn move_temp_to_target(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        // 防御性兜底：unique_mod_dir_name 已保证不冲突，此处仅清理残留
        if dst.is_dir() {
            fs::remove_dir_all(dst)?;
        } else {
            fs::remove_file(dst)?;
        }
    }
    if paths_on_same_disk(src, dst) && fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    if src.is_dir() {
        copy_dir_deep(src, dst)?;
        let _ = fs::remove_dir_all(src);
    } else {
        fs::copy(src, dst)?;
        let _ = fs::remove_file(src);
    }
    Ok(())
}

/// 清理解压临时区：仅当 `.tmp` 目录为空时删除，避免误删并发解压的其它临时目录。
fn cleanup_extract_tmp(tmp_parent: Option<&Path>) {
    if let Some(p) = tmp_parent {
        if let Ok(mut entries) = fs::read_dir(p) {
            if entries.next().is_none() {
                let _ = fs::remove_dir(p);
            }
        }
    }
}

/// 解压后处理：统计、原子移动到最终位置、展平、清理
///
/// 将 `extract_*` 返回的临时目录（位于 `<Mods>/.tmp`）整体同盘移动到分组目录下的
/// 唯一模组目录（重名时 `{modname} (N)`），再展平单层包裹目录。
///
/// 移动发生在「解压完成之后」，且源（`.tmp`）与目的地（分组目录）同处 Mods 卷，
/// 因此 rename 为同盘原子操作——即便移动瞬间进程崩溃，分组目录也只可能处于
/// 「尚未移动」（干净）或「已完整移动」两种状态，不会出现半解压污染分组目录。
///
/// # 参数
/// - `archive_path`: 原始压缩包路径（用于提取模组名称）
/// - `target_dir`: 目标分组目录（`.../Mods/_MANAGED_/group_X`）
/// - `temp_dir`: 解压后的临时目录（位于 `<Mods>/.tmp`）
///
/// # 返回
/// `ExtractResult::Success` 包含最终安装路径、模组名称、INI 检查结果和文件数
fn finalize_extraction(
    archive_path: &Path,
    target_dir: &Path,
    temp_dir: &Path,
) -> Result<ExtractResult> {
    let final_dir = temp_dir.to_path_buf();

    let mut has_ini = false;
    let mut file_count = 0usize;
    let mut counter = |path: &Path| {
        file_count += 1;
        if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("ini") {
                has_ini = true;
            }
        }
    };
    visit_files(&final_dir, &mut counter)?;

    let mod_name = archive_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    // 展平单层包裹目录（对齐 Dart unwrapSingleFolderNesting）：压缩包内若仅包裹单一根目录，
    // 将其内容提升为模组目录直接内容。
    //
    // ⚠️ 必须在「移动到最终位置之前」于 `.tmp` 暂存目录上执行：`.tmp` 隐藏目录被文件监视器
    // 过滤（file_watcher 回调中的 `.tmp` 排除规则），不会触发轻量扫描写入 `modname` 等标记文件；
    // 若改在移动到分组目录之后执行，监视器会在 300ms 防抖窗口内写入 `modname`，使顶层「相关条目」
    // 不再是单一目录，导致展平判定失败、智能解压退化为常规解压并额外消耗 CPU（B 修复）。
    if let Err(e) = unwrap_single_folder_nesting(&final_dir) {
        log::warn!(
            "finalize_extraction: 展平单层目录失败 {:?}: {}",
            final_dir,
            e
        );
    }

    // 重名处理：目标分组目录下已存在同名模组时，重命名为 `{modname} (N)`
    let final_name = unique_mod_dir_name(target_dir, &mod_name);
    let target_mod_path = target_dir.join(&final_name);

    // 同盘原子移动到最终位置（解压完成后才移动，避免中断污染分组目录）
    move_temp_to_target(&final_dir, &target_mod_path)?;

    // 清理：temp_dir 已被 rename 移走；尝试删除空的 `.tmp` 父目录
    cleanup_extract_tmp(temp_dir.parent());

    Ok(ExtractResult::Success {
        mod_path: target_mod_path,
        mod_name: final_name,
        has_ini,
        file_count,
    })
}

#[cfg(test)]
#[path = "archive_extract_tests.rs"]
mod tests;
