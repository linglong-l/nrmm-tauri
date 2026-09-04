// vendored 副本（基线 = crates.io unrar-ng-sys 0.7.7），唯一改动见下方注释。
// 上游原版用 `.flag_if_supported("-fPIC")`，依赖 cc 的编译器探测；
// 探测失败会静默丢弃该 flag，最终在 zigbuild 链接 cdylib 时报 -fPIC 错误。
// 这里改为强制 + 清洗环境注入的 -fno-pic，确保在 zig cc 下稳定产出 PIC 对象。
use std::env;

/// 从 CFLAGS/CXXFLAGS 中剔除 `-fno-pic` / `-fno-PIC`。
///
/// 根因：构建环境（CI / shell 全局 CFLAGS）若注入 `-fno-pic`，
/// zig cc 会在【编译期】硬报错 `the selected target requires position independent code`，
/// 而 gcc 只会静默产出非 PIC 对象、最后链接才报 `recompile with -fPIC`。
/// 这正是「仅 zigbuild 炸、gcc 不炸」的真正原因。
fn clean_pic_flags(key: &str) -> bool {
    if let Ok(val) = env::var(key) {
        let cleaned: Vec<&str> = val
            .split_whitespace()
            .filter(|t| !t.eq_ignore_ascii_case("-fno-pic"))
            .collect();
        let joined = cleaned.join(" ");
        if joined != val {
            // SAFETY: build.rs 是单线程入口，set_var 仅影响本进程的环境探测。
            unsafe {
                env::set_var(key, &joined);
            }
            println!(
                "cargo:warning=unrar_ng_sys (vendored): 从 {} 剔除关闭 PIC 的开关 -> {:?}",
                key, joined
            );
            return true;
        }
    }
    false
}

fn main() {
    // 1) 先清洗环境注入的 -fno-pic，避免与后续强制的 -fPIC 冲突。
    clean_pic_flags("CFLAGS");
    clean_pic_flags("CXXFLAGS");

    // 以【目标】三元组（而非宿主）决定平台相关行为，使交叉编译（如 Windows 宿主 → Linux 目标）
    // 与 WSL2（Linux 宿主 = Linux 目标）都正确。上游原版用 `cfg!(windows)`（宿主）做下面这些判断，
    // 在交叉编译时会误包含 Windows 专属源文件 / 误选链接库 / 误关 -fPIC。
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_vendor = std::env::var("CARGO_CFG_TARGET_VENDOR").unwrap_or_default();
    let target_windows = target_os == "windows";

    // Watch the whole vendored UnRAR tree so additions/removals — and edits
    // to files outside the `#include` graph that `cc` already tracks via
    // its own per-source `rerun-if-changed` emissions — trigger a rebuild.
    // (Cargo automatically tracks `build.rs` itself, so listing it here is
    // unnecessary.) Without this directive, an upstream upgrade that adds
    // a new `.cpp` we forget to register, or a `.hpp` not yet picked up by
    // any compiled translation unit, would silently use stale object files.
    println!("cargo:rerun-if-changed=vendor/unrar");

    if target_windows {
        println!("cargo:rustc-flags=-lpowrprof");
        println!("cargo:rustc-link-lib=shell32");
        println!("cargo:rustc-link-lib=advapi32");
        if target_env == "gnu" {
            println!("cargo:rustc-link-lib=pthread");
        }
    } else {
        println!("cargo:rustc-link-lib=pthread");
    }

    // 以【目标】OS 决定 Windows 专属源文件（isnt/motw），交叉编译时不再误包含。
    let mut names: Vec<&str> = vec![
        "strlist",
        "strfn",
        "pathfn",
        "smallfn",
        "global",
        "file",
        "filefn",
        "filcreat",
        "archive",
        "arcread",
        "unicode",
        "system",
        "crypt",
        "crc",
        "rawread",
        "encname",
        "match",
        "timefn",
        "rdwrfn",
        "consio",
        "options",
        "errhnd",
        "rarvm",
        "secpassword",
        "rijndael",
        "getbits",
        "sha1",
        "sha256",
        "blake2s",
        "hash",
        "extinfo",
        "extract",
        "volume",
        "list",
        "find",
        "unpack",
        "headers",
        "threadpool",
        "rs16",
        "cmddata",
        "ui",
        "filestr",
        "scantree",
        "dll",
        "qopen",
        "largepage", // New in unrar 7.x for large page memory allocation
    ];
    if target_windows {
        names.push("isnt"); // Windows-only
        names.push("motw"); // New in unrar 7.x for Mark of the Web support (Windows only)
    }
    let files: Vec<String> = names.iter().map(|&s| format!("vendor/unrar/{s}.cpp")).collect();

    let mut build = cc::Build::new();
    build
        .cpp(true) // Switch to C++ library compilation.
        .opt_level(2)
        .std("c++14")
        // by default cc crate tries to link against dynamic stdlib, which causes problems on windows-gnu target
        .cpp_link_stdlib(None)
        .warnings(false)
        .extra_warnings(false)
        .flag_if_supported("-stdlib=libc++")
        // 上游原 `.flag_if_supported("-fPIC")` 已移除；改为下方无条件强制（见 force_pic 分支）。
        .flag_if_supported("-Wno-switch")
        .flag_if_supported("-Wno-parentheses")
        .flag_if_supported("-Wno-macro-redefined")
        .flag_if_supported("-Wno-dangling-else")
        .flag_if_supported("-Wno-logical-op-parentheses")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-variable")
        .flag_if_supported("-Wno-unused-function")
        .flag_if_supported("-Wno-missing-braces")
        .flag_if_supported("-Wno-unknown-pragmas")
        .flag_if_supported("-Wno-deprecated-declarations")
        .define("_FILE_OFFSET_BITS", Some("64"))
        .define("_LARGEFILE_SOURCE", None)
        .define("RAR_SMP", None)
        .define("RARDLL", None);

    // 2) 强制 -fPIC：覆盖 cc 默认行为并兜底，确保 zig cc 一定收到 -fPIC。
    //    非 MSVC / wasi / emscripten 目标（即 Linux/macOS 等需 PIC 的平台）强制 PIC；
    //    MSVC 不认 -fPIC（cl.exe 会直接编译失败），故跳过。
    let force_pic = !target_windows && target_os != "wasi" && target_os != "emscripten";
    if force_pic {
        build.pic(true).flag("-fPIC");
        println!("cargo:warning=unrar_ng_sys (vendored): -fPIC forced (target needs PIC for cdylib)");
    } else {
        let tgt = if target_windows {
            "windows"
        } else if target_os == "wasi" {
            "wasi"
        } else {
            "emscripten"
        };
        println!("cargo:warning=unrar_ng_sys (vendored): -fPIC skipped (target_os=\"{tgt}\")");
    }

    // UNRAR_NG_FORCE_UTF8 commits Linux/BSD wide<->8bit filename conversions to
    // the same locale-independent WideToUtf / UtfToWide path that macOS has
    // used for years (raros.hpp:18-20 auto-defines _APPLE on __APPLE__ targets).
    //
    // Gated behind cargo feature `linux-batch-extract-utf8` (default-on). When
    // the feature is disabled, this define is NOT set and libunrar's
    // MBFUNCTIONS branch in unicode.cpp (`wcsrtombs` / `mbsrtowcs`) takes over.
    // In that mode the caller is responsible for `setlocale(LC_CTYPE, "")` —
    // either by calling it themselves before invoking this crate, or by
    // also enabling the high-level crate's `linux-batch-extract-setlocale`
    // cargo feature, which provides a Rust-side `OnceLock`-managed lazy init.
    //
    // Apple is excluded from the gate because raros.hpp already auto-defines
    // _APPLE on __APPLE__ targets — the WideToUtf path runs regardless of
    // this feature. Windows is excluded because the `_WIN_ALL` branch in
    // unicode.cpp uses `WideCharToMultiByte(CP_ACP, ...)` (OS-level system
    // codepage; CP936 zh-CN, CP932 ja-JP, CP65001 if user opted into the
    // Win10 >= 1803 / 11 "Beta UTF-8 ACP" toggle) and writes via
    // `CreateFile(LPCWSTR)` (wide-native NTFS). Vendor patch 0007 is a
    // no-op on both Apple and Windows builds.
    //
    // Cargo translates feature `linux-batch-extract-utf8` into the env var
    // `CARGO_FEATURE_LINUX_BATCH_EXTRACT_UTF8` (uppercase, hyphen → underscore).
    let feature_linux_batch_extract_utf8 =
        std::env::var("CARGO_FEATURE_LINUX_BATCH_EXTRACT_UTF8").is_ok();
    let force_utf8 = feature_linux_batch_extract_utf8
        && !target_windows
        && target_vendor != "apple";
    if force_utf8 {
        build.define("UNRAR_NG_FORCE_UTF8", None);
    }

    build.files(&files).compile("libunrar.a");
}
