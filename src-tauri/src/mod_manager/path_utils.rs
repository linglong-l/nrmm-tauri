//! 路径验证和工具函数模块
//!
//! 该模块提供纯函数形式的路径验证和只读名称获取工具，供上层调用方使用。
//! 所有函数均保持功能单一，不包含任何业务逻辑校验（校验由上层调用方负责）。

use std::fs;
use std::path::{Component, Path};

use anyhow::Result;

use super::{DISABLED_PREFIX, MOD_NAME_FILE};

/// 检测路径是否位于以 # 开头的目录（树形目录结构）下。
///
/// 该函数向上遍历路径的所有组件，检查是否存在以 # 开头的目录名，
/// 同时支持处理带 DISABLED_ 前缀的禁用状态 # 目录。
///
/// # 参数
/// - `path`: 待检测的文件或目录路径。
///
/// # 返回值
/// - `true`: 路径位于某个 # 目录下。
/// - `false`: 路径不位于任何 # 目录下。
///
/// # 示例
/// ```
/// use std::path::Path;
/// // 假设存在路径 Mods/_MANAGED_/group_1/modA → 返回 false
/// // 假设存在路径 Mods/#Character/modB → 返回 true
/// // 假设存在路径 Mods/DISABLED_#Character/modC → 返回 true
/// ```
pub fn is_path_under_hash_dir(path: &Path) -> bool {
    for component in path.components() {
        if let Component::Normal(os_str) = component {
            if let Some(name) = os_str.to_str() {
                let stripped = if let Some(stripped) = name.strip_prefix(DISABLED_PREFIX) {
                    stripped.trim_start_matches('_')
                } else {
                    name
                };
                if stripped.starts_with('#') {
                    return true;
                }
            }
        }
    }
    false
}

/// 从目录名中提取显示名称（去除 DISABLED 前缀）。
///
/// # 参数
/// - `dir_name`: 原始目录名字符串。
///
/// # 返回值
/// - 去除 DISABLED 前缀后的显示名称。
fn extract_display_name_from_dir(dir_name: &str) -> String {
    if let Some(stripped) = dir_name.strip_prefix(DISABLED_PREFIX) {
        stripped.trim_start_matches('_').to_string()
    } else {
        dir_name.to_string()
    }
}

/// 只读方式获取模组的显示名称，不会写入 modname 文件。
///
/// 优先读取目录下已存在的 modname 文件；若不存在，则从目录名提取（去除 DISABLED 前缀）。
/// 该函数不会创建或修改任何文件，适用于不允许写入元数据的场景（如 # 目录下的模组）。
///
/// # 参数
/// - `mod_path`: 模组目录的路径。
///
/// # 返回值
/// - `Ok(String)`: 模组的显示名称。
/// - `Err(anyhow::Error)`: 读取过程中发生 IO 错误。
pub fn get_mod_display_name_readonly(mod_path: &Path) -> Result<String> {
    let name_file = mod_path.join(MOD_NAME_FILE);

    if name_file.exists() {
        let content = fs::read_to_string(&name_file)?;
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    let dir_name = mod_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("mod");

    Ok(extract_display_name_from_dir(dir_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_is_path_under_hash_dir_normal_group() {
        let path = Path::new("C:/Games/Mods/_MANAGED_/group_1/modA");
        assert!(!is_path_under_hash_dir(path));
    }

    #[test]
    fn test_is_path_under_hash_dir_hash_root() {
        let path = Path::new("C:/Games/Mods/#Character");
        assert!(is_path_under_hash_dir(path));
    }

    #[test]
    fn test_is_path_under_hash_dir_nested_hash() {
        let path = Path::new("C:/Games/Mods/#Character/Outfits/modB");
        assert!(is_path_under_hash_dir(path));
    }

    #[test]
    fn test_is_path_under_hash_dir_disabled_hash() {
        let path = Path::new("C:/Games/Mods/DISABLED_#Weapons/modC");
        assert!(is_path_under_hash_dir(path));
    }

    #[test]
    fn test_is_path_under_hash_dir_no_hash() {
        let path = Path::new("C:/Games/Mods/RegularFolder/modD");
        assert!(!is_path_under_hash_dir(path));
    }

    #[test]
    fn test_is_path_under_hash_dir_hash_in_middle() {
        let path = Path::new("C:/Games/Mods/Group1/#Nested/modE");
        assert!(is_path_under_hash_dir(path));
    }

    #[test]
    fn test_get_mod_display_name_readonly_from_existing_modname() {
        let dir = TempDir::new().unwrap();
        let mod_path = dir.path().join("some_mod");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join(MOD_NAME_FILE), "My Cool Mod\n").unwrap();

        let name = get_mod_display_name_readonly(&mod_path).unwrap();
        assert_eq!(name, "My Cool Mod");
    }

    #[test]
    fn test_get_mod_display_name_readonly_from_dir_name() {
        let dir = TempDir::new().unwrap();
        let mod_path = dir.path().join("plain_mod_folder");
        fs::create_dir_all(&mod_path).unwrap();

        let name = get_mod_display_name_readonly(&mod_path).unwrap();
        assert_eq!(name, "plain_mod_folder");

        assert!(!mod_path.join(MOD_NAME_FILE).exists());
    }

    #[test]
    fn test_get_mod_display_name_readonly_disabled_prefix() {
        let dir = TempDir::new().unwrap();
        let mod_path = dir.path().join("DISABLED_my_disabled_mod");
        fs::create_dir_all(&mod_path).unwrap();

        let name = get_mod_display_name_readonly(&mod_path).unwrap();
        assert_eq!(name, "my_disabled_mod");
    }

    #[test]
    fn test_get_mod_display_name_readonly_empty_modname_falls_back() {
        let dir = TempDir::new().unwrap();
        let mod_path = dir.path().join("mod_with_empty_name");
        fs::create_dir_all(&mod_path).unwrap();
        fs::write(mod_path.join(MOD_NAME_FILE), "   \n  ").unwrap();

        let name = get_mod_display_name_readonly(&mod_path).unwrap();
        assert_eq!(name, "mod_with_empty_name");
    }
}
