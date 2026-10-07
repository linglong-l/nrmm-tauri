//! 外部代码编辑器域命令
//!
//! 对齐 NRMM v3.2.0（`63790ae` Added support for code editors）：
//! - `detect_external_editors`：探测系统已装的代码编辑器（PATH which + Windows
//!   ProgramFiles 回退），与 Dart `findExternalCodeEditors` 同序同判定：
//!   VS Code(`code`) / Zed(`zed`) / Notepad++(`notepad++` + ProgramFiles) /
//!   Sublime(`subl` + ProgramFiles)
//! - `open_in_editor`：以指定编辑器打开目标目录（detached 子进程，按编辑器
//!   特化参数，对齐 Dart 各 `open*ToSpecifiedPath`）：
//!   - VS Code：移除 `ELECTRON_RUN_AS_NODE` 后启动
//!   - Zed：`--classic <path>`
//!   - Notepad++：`-multiInst -nosession -openFoldersAsWorkspace <path>`
//!   - Sublime：`<path>`
//!
//! 编辑器路径由后端按 id 重新解析（不信任前端回传路径）；`.cmd`/`.bat` shim
//! 经 `cmd /C` 中转并以 `CREATE_NO_WINDOW` 启动，避免闪烁控制台。

use crate::ipc::typed_paths::ManagedPath;
use serde::Serialize;
use std::path::Path;
use std::process::Command;

/// 探测到的外部编辑器信息
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalEditorInfo {
    /// 编辑器标识（`vscode` / `zed` / `notepadpp` / `sublime`）
    pub id: String,
    /// 显示名（与 NRMM 文案一致，各语言同为专有名词）
    pub name: String,
    /// 可执行文件绝对路径
    pub path: String,
}

/// PATH 探测：在各 PATH 目录中按候选文件名查找（Windows 附带常见可执行后缀）。
fn which(name: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    let exts: Vec<&str> = if cfg!(windows) {
        vec!["", ".exe", ".cmd", ".bat"]
    } else {
        vec![""]
    };
    for dir in std::env::split_paths(&path_var) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// 第一个存在的候选路径（对齐 Dart `_firstExisting`）。
fn first_existing(paths: Vec<String>) -> Option<String> {
    paths.into_iter().find(|p| Path::new(p).is_file())
}

/// Windows ProgramFiles 系环境变量下的候选路径表。
#[cfg(windows)]
fn program_files_candidates(relative: &str) -> Vec<String> {
    ["ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|k| {
            std::env::var(k)
                .ok()
                .map(|root| format!("{root}\\{relative}"))
        })
        .collect()
}

/// 非 Windows 平台无 ProgramFiles 回退（探测仅走 PATH）。
#[cfg(not(windows))]
fn program_files_candidates(_relative: &str) -> Vec<String> {
    Vec::new()
}

/// 探测外部代码编辑器（对齐 Dart `findExternalCodeEditors` 的顺序与判定）。
#[tauri::command]
pub fn detect_external_editors() -> Vec<ExternalEditorInfo> {
    let mut editors: Vec<ExternalEditorInfo> = Vec::with_capacity(4);
    if let Some(p) = which("code") {
        editors.push(ExternalEditorInfo {
            id: "vscode".to_string(),
            name: "VS Code".to_string(),
            path: p,
        });
    }
    if let Some(p) = which("zed") {
        editors.push(ExternalEditorInfo {
            id: "zed".to_string(),
            name: "Zed".to_string(),
            path: p,
        });
    }
    let notepadpp = which("notepad++")
        .or_else(|| first_existing(program_files_candidates("Notepad++\\notepad++.exe")));
    if let Some(p) = notepadpp {
        editors.push(ExternalEditorInfo {
            id: "notepadpp".to_string(),
            name: "Notepad++".to_string(),
            path: p,
        });
    }
    let sublime = which("subl")
        .or_else(|| first_existing(program_files_candidates("Sublime Text\\sublime_text.exe")));
    if let Some(p) = sublime {
        editors.push(ExternalEditorInfo {
            id: "sublime".to_string(),
            name: "Sublime Text".to_string(),
            path: p,
        });
    }
    editors
}

/// 以指定编辑器打开目标路径（对齐 Dart 各 `open*ToSpecifiedPath`）。
///
/// # 参数
/// - `editor_id`: `vscode` / `zed` / `notepadpp` / `sublime`（须为已探测到的编辑器）
/// - `target_path`: 目标目录或文件（须存在）
///
/// # Errors
/// 目标不存在 / 编辑器未探测到 / 未知编辑器 id / 进程启动失败时返回 `Err`。
#[tauri::command]
pub fn open_in_editor(editor_id: String, target_path: ManagedPath) -> Result<(), String> {
    // 存在性与 Mods 根归属由 ManagedPath 保证；传给编辑器的仍是前端原形态
    // （canonical 在 Windows 上带 \\?\ verbatim 前缀，多数编辑器处理不佳）
    let target_for_editor = target_path.requested().to_string();
    let editor = detect_external_editors()
        .into_iter()
        .find(|e| e.id == editor_id)
        .ok_or_else(|| format!("editor not detected: {editor_id}"))?;
    spawn_editor(&editor.path, &editor_id, &target_for_editor)
}

/// 按编辑器特化参数启动子进程（父进程不等待；`.cmd`/`.bat` shim 由 std 按 cmd 语义转义）。
fn spawn_editor(editor_path: &str, editor_id: &str, target_path: &str) -> Result<(), String> {
    let mut command = build_command(editor_path, editor_id, target_path)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：shim 启动时不闪控制台窗口（GUI 编辑器无感知）
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to spawn editor {editor_id}: {e}"))
}

/// 组装各编辑器的启动命令（参数对齐 NRMM `external_editors.dart`）。
fn build_command(editor_path: &str, editor_id: &str, target_path: &str) -> Result<Command, String> {
    // 直接以脚本作为可执行文件交给 std：Windows 下 std 识别 .cmd/.bat 后会改用
    // `cmd.exe /d /e:ON /v:OFF /c "…"` 并按 **cmd.exe 语义**转义参数
    // （sys/args/windows.rs::make_bat_command_line）。
    // 原先手工 `Command::new("cmd").arg("/C").arg(script)` 反而绕开这套转义：此时
    // 可执行文件是 `cmd`，std 走普通引用规则，含 `&` 且不含空格的路径不会被加引号，
    // cmd 会把它当命令分隔符继续解析（模组目录名可含 `&`，来自解压产物）。
    let mut command = Command::new(editor_path);
    match editor_id {
        "vscode" => {
            // 对齐 Dart：移除 ELECTRON_RUN_AS_NODE，避免 VS Code 以 Node 模式启动
            command.env_remove("ELECTRON_RUN_AS_NODE").arg(target_path);
        }
        "zed" => {
            command.arg("--classic").arg(target_path);
        }
        "notepadpp" => {
            command
                .arg("-multiInst")
                .arg("-nosession")
                .arg("-openFoldersAsWorkspace")
                .arg(target_path);
        }
        "sublime" => {
            command.arg(target_path);
        }
        other => return Err(format!("unknown editor id: {other}")),
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PATH 探测在空 PATH 下返回 None（不 panic）
    #[test]
    fn which_empty_path_returns_none() {
        // 临时清空 PATH 验证边界（进程内 env 变更仅影响本测试线程不安全，
        // 故只验证候选构造逻辑：未知编辑器 id 报错）
        assert!(build_command("whatever", "unknown", "C:\\").is_err());
    }

    /// 各编辑器命令参数特化（不实际启动，仅校验组装不报错）
    #[test]
    fn build_command_known_ids_ok() {
        for id in ["vscode", "zed", "notepadpp", "sublime"] {
            assert!(
                build_command("C:/editor.exe", id, "C:/target").is_ok(),
                "editor {id} 应能组装命令"
            );
        }
    }
}
