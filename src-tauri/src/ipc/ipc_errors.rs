//! IPC 解析期错误码
//!
//! 错误串统一以 `[CODE]` 前缀开头，沿用仓库既有约定（如
//! `commands/mod_mod_commands.rs` 的 `ERR_PREFIX_PATH_NOT_FOUND`），
//! 使前端 `safeInvoke` 与按前缀分支的处理逻辑不因类型化改造而失效。

/// 入参不是绝对路径
pub const E_PATH_NOT_ABSOLUTE: &str = "[PATH_NOT_ABSOLUTE]";
/// 入参含 `..` 上跳组件
pub const E_PATH_PARENT_TRAVERSAL: &str = "[PATH_PARENT_TRAVERSAL]";
/// 路径不存在或无法 canonicalize（与既有 `[PATH_NOT_FOUND]` 同义）
pub const E_PATH_NOT_FOUND: &str = "[PATH_NOT_FOUND]";
/// 路径规范后不在任何已登记 Mods 根内
pub const E_OUT_OF_MANAGED_ROOT: &str = "[OUT_OF_MANAGED_ROOT]";
/// 尚无任何已登记 Mods 根（fail-closed：一律拒绝，不静默放行）
pub const E_ROOT_NOT_REGISTERED: &str = "[ROOT_NOT_REGISTERED]";
/// 来源路径不可读（`SourcePath` 专用）
pub const E_SOURCE_NOT_FOUND: &str = "[SOURCE_NOT_FOUND]";
/// 入参为空字符串
pub const E_EMPTY: &str = "[EMPTY_PATH]";

/// 以 `code + 消息` 构造 serde 解析错误。
///
/// `serde` 会把该串包进 `tauri::Error::InvalidArgs`，最终作为字符串错误回传前端，
/// 与现有 `Result<T, String>` 命令错误通道一致。
///
/// # 参数
/// - `code`: 本模块的 `[CODE]` 前缀常量
/// - `msg`: 面向日志的可读说明（含具体路径，便于排障；不含密钥）
pub fn reject<T: serde::de::Error>(code: &str, msg: impl AsRef<str>) -> T {
    T::custom(format!("{code} {}", msg.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_prefixed_and_unique() {
        let codes = [
            E_PATH_NOT_ABSOLUTE,
            E_PATH_PARENT_TRAVERSAL,
            E_PATH_NOT_FOUND,
            E_OUT_OF_MANAGED_ROOT,
            E_ROOT_NOT_REGISTERED,
            E_SOURCE_NOT_FOUND,
            E_EMPTY,
        ];
        for c in codes.iter() {
            assert!(
                c.starts_with('[') && c.ends_with(']'),
                "非法错误码形状：{c}"
            );
        }
        for (i, a) in codes.iter().enumerate() {
            for b in codes.iter().skip(i + 1) {
                assert_ne!(a, b, "错误码重复：{a}");
            }
        }
    }

    /// 错误串必须原样带上 `[CODE]` 前缀，前端据此分支
    #[test]
    fn reject_keeps_code_prefix() {
        let e = reject::<serde_json::Error>(E_OUT_OF_MANAGED_ROOT, "path is outside any root");
        let s = e.to_string();
        assert!(s.starts_with(E_OUT_OF_MANAGED_ROOT), "实际错误串：{s}");
        assert!(s.contains("outside any root"), "实际错误串：{s}");
    }
}
