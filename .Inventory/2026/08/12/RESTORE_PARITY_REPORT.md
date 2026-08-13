# 模组还原逻辑 — NRMM 对齐比对报告

> 基准线：`GitProjects/No-Reload-Mod-Manager/lib/utils/mod_manager.dart`
> 当前 commit：`eef2b2b` (main)，`mod_manager.dart` 3440 行（已 `git pull` 至最新）
> 比对日期：2026-08-12
> 结论：**Rust 还原逻辑与 NRMM `restoreManagedMod` 逐行对齐，无需修复真实缺陷**；本次仅做 1 处源忠实度对齐（section 头缩进）。

---

## 1. 还原逻辑两条路径

| 路径 | Rust 入口 | Dart 等价 | 语义 |
|------|-----------|-----------|------|
| 就地清理式（UI「还原」等价） | `restore_managed_mod` → `restore_single_ini` (mod_manager.rs:2237/2355) | `restoreManagedMod` (dart:889) | 逐行清理管理器注入产物，保留用户内容与合法 orphan `endif` |
| 备份拷回式（Rust 独有） | `restore_all_inis` (mod_manager.rs:2088) | **无等价** | 从 `.ini_managed_backup` 拷回 + 清理 manager 生成文件 |

> 说明：Dart 的 `.ini_managed_backup`（常量 `managedBackupExtension`，dart:2205）**仅创建、从不回读**——Dart 用户面「还原」只有 `restoreManagedMod`。`restore_all_inis` 是 Rust 额外提供的「整盘还原到 pristine 备份」功能，无损（见 §7 测试），但与 NRMM 还原语义不同，属设计取舍，非缺陷。

---

## 2. `restore_single_ini` ↔ `restoreManagedMod` 逐行对照

循环体对每个非 section 行执行，顺序与 Dart 完全一致：

| 步骤 | Dart (`restoreManagedMod`) | Rust (`restore_single_ini`) | 对齐 |
|------|----------------------------|------------------------------|------|
| 新 section 重置 if 栈 | `if startsWith('[') { ifStack=…; continue; }`（**不**去缩进） | `if startsWith('[') { if_stack.clear(); continue; }` | ✅ 本次已对齐（Rust 原多余一次 `remove_first_four_spaces`，真实数据无影响，已移除） |
| ① 移除管理器注释 | 含 5 个关键字：`no reload mod manager` / `";-;" are errored` / `";+;" are disabled keys` / `errored conditional blocks` / `if certain syntax is only available` | 同样 5 个关键字（含 `r#"";-;" are errored"#`） | ✅ 完全一致 |
| ② 移除 `global $managed_slot_id =` | `noSpace.startsWith("global$managed_slot_id=")` | 同（去掉空格后比较） | ✅ |
| ③ 净化 `condition=` | 处理 `condition=` / `;-;condition=` / `;+;condition=`，调用 `_sanitizeKeyConditionExpressionFromModManager` | 同三种前缀，调用 `sanitize_condition_expression_public` | ✅ |
| ④ 移除管理器 `if` | `push` 行；若 `contains("if$managed_slot_id==$\modmanageragl\group_")` 则标记删除 | `push(is_manager_if)`；若含同样子串则删除 | ✅ |
| ⑤ 移除配对 `endif` | `pop()`；若弹出行为管理器 `if` 则删 `endif` | `pop()`；若 `is_manager` 则删 `endif` | ✅ |
| ⑥ 去前 4 空格缩进 | 每行末调用 `_removeFirstFourSpaces`（section 头除外，因③/⑤/④的 `continue` 已跳过） | 每行末调用 `remove_first_four_spaces`（同上跳过） | ✅ |

**关键一致点：condition 行改写后 `continue`，不被去缩进**——Dart (dart:960) 与 Rust (mod_manager.rs:2417) 行为相同。

---

## 3. `sanitize_condition_expression` ↔ `_sanitizeKeyConditionExpressionFromModManager`

| 子步骤 | Dart | Rust | 对齐 |
|--------|------|------|------|
| 移除管理器表达式 | 正则 `\$managed_slot_id\s*==\s*\$(\\modmanageragl\\group_)(\d+)(\\active_slot)`（**严格** `\active_slot`） | 正则 `\$managed_slot_id\s*==\s*\$\\modmanageragl\\group_\d+\\[A-Za-z0-9_]+`（**超集**，含数字 token） | ⚠️ 真实注入 token 均为 `active_slot`（Rust inject 657 行），两者结果一致；Rust 超集额外覆盖本项目历史数字 token，属有意更稳健 |
| `(&& x)`→`(x)` | `\(\s*&&\s*`→`(` | 同 | ✅ |
| `(|| x)`→`(x)` | `\(\s*\|\|\s*`→`(` | 同 | ✅ |
| `(x && )`→`(x)` | `\s*&&\s*\)`→`)` | 同 | ✅ |
| `(x \|\| )`→`(x)` | `\s*\|\|\s*\)`→`)` | 同 | ✅ |
| `()`→`` | `\(\s*\)`→`` | 同 | ✅ |
| 尾部 `&&`/`||` | 单次 `replaceAll`（`&&(?=\s*$)`） | `while ends_with("&&")` 循环（更彻底，真实数据等价） | ✅ |
| 头部 `&&`/`||` | `startsWith` + `replaceFirst` | `starts_with` + `replacen(..,1)` | ✅ |
| `&& &&`→`&&` 等 4 条合并 | 同 | 同 | ✅ |
| 外层括号解包 | `_isWrappedInMatchingParens` | `is_wrapped_in_matching_parens` | ✅ |

---

## 4. `remove_first_four_spaces` ↔ `_removeFirstFourSpaces`

两者均为「从行首移除**最多** 4 个空格」（计数到 4 或非空格即停）。逐字符语义一致。✅

---

## 5. 前期待确认项 —— 现已用源码关闭

**`match_priority = 0` / `allow_duplicate_hash = true`（由 `update_mod_data` 注入）在还原时是否移除？**

- Dart 注入位置：`update_mod_data` 等价函数（dart:2618-2662）对 `[TextureOverride*]` 补 `match_priority = 0`、对 `[ShaderOverride*]` 补 `allow_duplicate_hash = true`。
- Dart `restoreManagedMod`（dart:889-1025）**只**处理：管理器注释 / `global $managed_slot_id` / `condition=` / 管理器 `if`/`endif` / 4 空格缩进。**完全不触及** `match_priority` / `allow_duplicate_hash`。
- ⇒ Rust `restore_single_ini` 不移除它们，**与 NRMM 一致**。前期「待确认边界」据此关闭，无需修改。

---

## 6. 本次对齐修改

**`restore_single_ini` section 头分支**（mod_manager.rs:2367）：
- 改前：重置 if 栈后额外 `*line = remove_first_four_spaces(line)` 再 `continue`。
- 改后：仅重置 if 栈后 `continue`，与 Dart `restoreManagedMod` 的 `if startsWith('[') { ifStack=…; continue; }` 逐字一致。
- 影响：真实数据 section 头恒在列 0，去缩进本为 no-op；仅作源忠实度对齐，不改变任何输出，7 个还原测试 + 全量 187 测试仍全绿。

---

## 7. 验证结果

- `cargo test --lib`（还原相关 7 用例）：**7 passed**
  - `test_restore_all_inis_lossless_roundtrip` — 备份式还原是 `update_mod_data` 的字节无损逆操作
  - `test_restore_managed_mod_cleans_injected_artifacts` — 就地清理正确清除 manager `if`/`endif`/`global`/增强 `condition`/4 空格缩进/头注释，保留用户内容 + 合法 orphan `endif`
  - `test_restore_managed_mod_after_real_update` — 真实管线回放端到端
- `cargo test --lib`（全量）：**187 passed / 0 failed**
- `cargo clippy --lib`：**0 warnings**

---

## 8. 结论

Rust 还原逻辑（核心 `restore_single_ini`）是 `restoreManagedMod` 的忠实 1:1 移植，逐行对齐、无真实缺陷。唯一实质差异是 `restore_all_inis`（备份拷回）为 Rust 独有功能（Dart 无用户面等价），无损且经测试验证，属设计取舍。其余均为 benign 细节（正则 token 严格度、尾部 `&&` 循环彻底度），在真实注入 token (`active_slot`) 下与 Dart 结果完全一致。
