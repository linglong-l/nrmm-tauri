use super::*;
use std::fs;
use tempfile::TempDir;

/// 测试：`strip_disabled_prefix` 一次移除连续的全部 DISABLED 前缀及分隔符
#[test]
fn test_strip_disabled_prefix() {
    // 大小写 + 不同分隔符
    assert_eq!(strip_disabled_prefix("DISABLED_Mod"), "Mod");
    assert_eq!(strip_disabled_prefix("Disabled-Mod"), "Mod");
    assert_eq!(strip_disabled_prefix("disabled_mod"), "mod");
    assert_eq!(strip_disabled_prefix("DISABLED Mod"), "Mod");
    assert_eq!(strip_disabled_prefix("DISABLEDMod"), "Mod");
    // 连续多个前缀：必须一次全部移除
    assert_eq!(strip_disabled_prefix("DISABLEDDISABLEDMod"), "Mod");
    assert_eq!(strip_disabled_prefix("DISABLED_DISABLED_Mod"), "Mod");
    // 前缀 DISABLED + 模组名以 Disabled（大小写混合）开头：只剥前缀，保留模组名
    assert_eq!(strip_disabled_prefix("DISABLEDDisabledMod"), "DisabledMod");
    assert_eq!(
        strip_disabled_prefix("DISABLEDDISABLEDDisabledMod"),
        "DisabledMod"
    );
    // 无前缀时原样返回
    assert_eq!(strip_disabled_prefix("Mod"), "Mod");
    // 整个名称就是前缀 → 空串
    assert_eq!(strip_disabled_prefix("DISABLED"), "");
}

/// 测试：`natural_compare` 的自然排序功能
///
/// 验证：`mod2` < `mod10`（数字段按数值比较而非字典序）；不区分大小写；相同字符串返回 Equal
#[test]
fn test_natural_compare() {
    assert_eq!(natural_compare("mod2", "mod10"), CmpOrdering::Less);
    assert_eq!(natural_compare("mod10", "mod2"), CmpOrdering::Greater);
    assert_eq!(natural_compare("mod2", "mod2"), CmpOrdering::Equal);
    assert_eq!(natural_compare("a1b2", "a1b10"), CmpOrdering::Less);
    assert_eq!(natural_compare("ModA", "moda"), CmpOrdering::Equal);
    assert_eq!(natural_compare("mod1", "mod2"), CmpOrdering::Less);
    assert_eq!(natural_compare("mod10", "mod11"), CmpOrdering::Less);
    assert_eq!(natural_compare("abc", "def"), CmpOrdering::Less);
}

/// 测试：`is_disabled_dir` 对 DISABLED 前缀的检测
///
/// 验证：`DISABLED_`、`disabled_`、`Disabled-`、`DISABLED `、`DISABLEDMod` 均被识别；`NormalMod` 和 `mod_disabled`（后缀）不被识别
#[test]
fn test_is_disabled_dir() {
    assert!(is_disabled_dir("DISABLED_Mod"));
    assert!(is_disabled_dir("disabled_mod"));
    assert!(is_disabled_dir("Disabled-Mod"));
    assert!(is_disabled_dir("DISABLED Mod"));
    assert!(is_disabled_dir("DISABLEDMod"));
    assert!(!is_disabled_dir("NormalMod"));
    assert!(!is_disabled_dir("mod_disabled"));
}

/// 测试：`dir_has_ini_file` 检测目录中是否存在 .ini 文件
///
/// 验证：空目录返回 false；写入 .ini 后返回 true；其他扩展名文件不影响结果
#[test]
fn test_dir_has_ini_file() {
    let dir = TempDir::new().unwrap();
    assert!(!dir_has_ini_file(dir.path()).unwrap());

    let ini_path = dir.path().join("test.ini");
    fs::write(&ini_path, "").unwrap();
    assert!(dir_has_ini_file(dir.path()).unwrap());

    let txt_path = dir.path().join("test.txt");
    fs::write(&txt_path, "").unwrap();
    assert!(dir_has_ini_file(dir.path()).unwrap());
}

/// 测试：`is_normal_group_dir` 对 group_xx 的匹配和拒绝规则
///
/// 验证：正确匹配 `group_1`、`group_12` 等；拒绝 `group_0`、`group_01`（前导零）、非数字后缀
#[test]
fn test_is_normal_group_dir() {
    // 正确匹配
    assert_eq!(is_normal_group_dir("group_1"), Some(1));
    assert_eq!(is_normal_group_dir("group_12"), Some(12));
    assert_eq!(is_normal_group_dir("group_123"), Some(123));
    assert_eq!(is_normal_group_dir("group_999"), Some(999));
    // 9 位合法上限
    assert_eq!(is_normal_group_dir("group_999999999"), Some(999_999_999));

    // 拒绝前导零
    assert_eq!(is_normal_group_dir("group_0"), None);
    assert_eq!(is_normal_group_dir("group_01"), None);
    assert_eq!(is_normal_group_dir("group_012"), None);
    assert_eq!(is_normal_group_dir("group_00"), None);

    // 拒绝非数字
    assert_eq!(is_normal_group_dir("group_abc"), None);
    assert_eq!(is_normal_group_dir("group_"), None);
    assert_eq!(is_normal_group_dir("group"), None);
    assert_eq!(is_normal_group_dir("#MutexMods"), None);
    assert_eq!(is_normal_group_dir("OtherFolder"), None);

    // 拒绝非严格 group_int 命名（避免误删/误管）
    assert_eq!(is_normal_group_dir("groupxx"), None);
    assert_eq!(is_normal_group_dir("groupsxx"), None);
    assert_eq!(is_normal_group_dir("XX"), None);

    // 拒绝 10 位及以上（超出安全索引范围）
    assert_eq!(is_normal_group_dir("group_1000000000"), None);
    assert_eq!(is_normal_group_dir("group_4294967295"), None);
}

/// is_group_xx_dir 正确性测试
#[test]
fn test_is_group_xx_dir() {
    assert!(is_group_xx_dir("group_1"));
    assert!(is_group_xx_dir("group_123"));
    assert!(!is_group_xx_dir("group_")); // 无数字
    assert!(!is_group_xx_dir("group_abc")); // 非数字
    assert!(!is_group_xx_dir("Group_1")); // 大小写敏感
    assert!(!is_group_xx_dir("#MyGroup")); // mutexGroup
    assert!(!is_group_xx_dir("SubGroup")); // 自定义子分组
    assert!(!is_group_xx_dir("DISABLEDgroup_1")); // 禁用前缀
}
