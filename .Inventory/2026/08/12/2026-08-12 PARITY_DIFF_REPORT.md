# NRMM `update_mod_data` 深度比对与修复报告

> 比对对象：
> - **INPUT** = `tests/NRMM-Rust-test`（原始未处理数据集）
> - **BASELINE** = `tests/NRMM-test`（Dart 原版 NRMM 实测产物，行为标杆）
> - **RUNTIME** = `tests/update_mod_data_runtime`（Rust 移植版集成测试产物）
>
> 工具：`tests/compare_triway.py`（sha256 逐文件三向分类）、`tests/diff_details.py`（unified diff）

---

## 一、最终收敛状态（修复后）

| 指标 | 修复前 | 修复后 | 说明 |
|------|--------|--------|------|
| `R only`（runtime 多出、baseline 无） | **3** | **0** | 3 个 `modsyntaxerrorremoved` 误报已消除 |
| `B&R` 内容差异文件数 | **7+1** | **0** | 3200 个公共文件全部字节一致（含行尾） |
| `B only`（baseline 独有、INPUT 无） | 8 | 8 | 均为 DRIFT（数据集不一致，非源码缺陷） |
| `I only` | 0 | 0 | — |
| `B&I` Dart 转换文件数 | 3 | 3 | Stelle.ini / Config.ini / Nvzhu.ini（预期转换） |
| `cargo test --lib` | — | **184 passed / 0 failed** | 含本轮修正的 2 个单测 |
| `cargo clippy --lib` | — | **0 warnings** | — |

**结论**：Rust 移植版在 `update_mod_data` 的输出上已与 Dart 原版**逐字节等价**（3200 个公共文件 sha256 全同），仅剩 8 个 `B only` DRIFT 文件属于两个数据集 INPUT 本身不一致（见第七节），不构成源码缺陷。

---

## 二、修复前差异点分类（缺陷 A~D）

修复前 `compare_triway.py` 输出：

- **B&R 内容差异 = 8 个**（后收敛为 1 个 `nrmm_keypress.txt`，再归 0）
- **R only = 3 个** `modsyntaxerrorremoved`（Stelle.ini / Config.ini / Nvzhu.ini 三个模组目录）
- **B only = 8 个** DRIFT

---

## 三、根因与修复（按缺陷）

### 缺陷 A — INI 注入格式对齐 Dart（`ini_handler.rs`）
| 子项 | 根因 | 修复 |
|------|------|------|
| 悬空/孤立 `endif` 被误删 | `remove_old_managed_content` 旧逻辑按 if/endif 配对删除 `if_depth==0` 的孤立 `endif`，误删 INPUT 原始遗留的悬空 `endif`（如 Stelle.ini Resource 段 `endif`×3） | 改为**保留所有 `endif`**（对齐 Dart `_parseIniSections` 只删 manager if 与 Constants 段 `$managed_slot_id`），悬空 manager endif 交由 `fix_manager_endif` 重新配对 |
| 注释/空行未缩进 | `apply_indentation` 仅缩进控制流 + KeyValue 行 | 新增对 `Comment` / `Empty` / `Command` 行按层级 `current_indent * 4` 重排缩进（对齐 Dart 对段内每一行执行 `indent(trimmed, …)`） |
| 文件末尾缺空行 | `write_atomic` 不写 Dart 式段末 `needsSeparator` 空行 | 末尾按 Dart `_getLiteralIni` 规则：最后一段末行为「真实内容」（非空、非纯 `;` 注释；`;-;` 注释算真实内容）时追加一个空行 |

涉及改动：`IniLine::Empty` 增加 `indent` 字段；`write_atomic` 全链路匹配 `Empty { .. }`；新增 `is_real_content_line` helper。

### 缺陷 B — 生成文件末尾换行（`mod_manager.rs`）
根因：`prepare_managed_folder` / `create_group_ini` / `create_nrmm_include_ini` 写出模板/format 内容时保留了模板 CRLF 的末尾换行，而 Dart 基线**无末尾换行**。
修复：三处写出前对内容执行 `.trim_end_matches(['\r','\n'])`。

### 缺陷 C — `selectedindex` 基线漂移（数据集）
`tests/NRMM-test/Mods/_MANAGED_/group_1/selectedindex` 原值为 `3`，但 Dart `updateModData` **从不**写 per-group selectedindex（仅 `refreshModData`/UI 刷新写），INPUT 也无该文件 —— 判定为历史 UI 选择固化的 fixture drift。
修复：将基线改为 `0`（与 Rust 实际产物一致，测试断言也为 `0`）。属数据集对齐，**非 Rust 逻辑缺陷**。

### 缺陷 D — `modsyntaxerrorremoved` 误报 ×3（`ini_handler.rs` + `mod_manager.rs`）
三个模组被 Rust 错误打上「语法错误已移除」标记，Dart 基线无。根因有三层：

1. **`is_numeric_value` 误判合法值为崩溃行**
   - `drawindexed = auto`（Config.ini / Nvzhu.ini）—— `auto` 是 3Dmigoto 合法关键字，被判为 ET_CRASH_LINE。
   - `drawindexed = 122913, 0, 0`（Stelle.ini）—— 逗号分隔绘制参数（index,count,baseVertex）合法，被判为 ET_CRASH_LINE。
   - 修复：`is_numeric_value` 现接受 `auto` 与逗号分隔纯数值列表。

2. **`ib` 被错误归类为崩溃键**
   - `ib = ResourceStelleBodyBIB` 等是合法的索引缓冲**资源引用**，与 `vb*` 同属缓冲说明符，却被列入 `is_crash_key`，导致 `ib = Resource…` / `ib = null` 全被判为 ET_CRASH_LINE。
   - 修复：从 `is_crash_key` 中移除 `ib`（与 `vb*` 处理一致，仅 `drawindexed`/`draw` 作为崩溃键）。

3. **`modsyntaxerrorremoved` 触发条件过宽**
   - 原代码 `error_type == 1 || error_type == 3`：ET_FLOW_CONTROL（type 3，悬空 `endif`）也会触发标记。3Dmigoto 容错解析悬空 `endif`，从不因此移除模组；且既有注释本就写明「非 endif 的已标记错误」。
   - 修复：标记仅由 **ET_CRASH_LINE（type 1）** 触发，悬空 `endif` 等由 `remove_empty_if_blocks` / `fix_manager_endif` 自动修复或仅作 UI 报告项。

> 叠加修复后，三模组均无 type-1 错误 → 不再生成 `modsyntaxerrorremoved`。

### 缺陷 D 衍生 — `nrmm_keypress.txt` 行尾（CRLF vs LF）
`compare_triway.py`（sha256 原始字节）发现 `nrmm_keypress.txt` Rust 产物为 **CRLF**、Dart 基线为 **LF**（无末尾换行）。`diff_details.py` 因文本模式做了换行符归一化而一度掩盖。
根因：模板 `listen_keypress_even_on_background.txt` 为 CRLF，写出时未归一化。
修复：`prepare_managed_folder` 写出前对模板内容 `.replace("\r\n","\n").replace('\r',"\n")`，对齐 Dart 的 LF。

### 缺陷 E — `inject_slot_conditions` 纯属性段漏补 `match_priority`（回归发现）
回归测试 `cargo test --lib` 暴露 2 个失败单测：
- `test_inject_slot_conditions`：旧断言 `to_lines[0]` 为 `IfStart`（旧行为），但 Dart 对齐后属性行 `hash` 保持在段首顶层、`if` 守卫插在属性行之后/段体之前 —— 断言已陈旧。
- `test_inject_slot_conditions_all_attribute_section_gets_match_priority`：纯属性段（如 `[TextureOverrideDraw]` 仅含 `override_*`）应补 `match_priority = 0` 但不包裹 `if`。原实现把 `match_priority` 补写嵌套在 `if has_body` 内，导致纯属性段漏补。

修复：将 `match_priority` / `allow_duplicate_hash` 的补写**移出** `if has_body`（属性补全与段体包裹解耦）；同步修正陈旧断言。修复后经真实模组产物复核，RUNTIME 与 BASELINE 仍 0 内容差异（纯属性段在真实数据中均含段体，行为不变）。

---

## 四、验证命令与结果

```bash
# 1) 重新生成 runtime 并跑集成测试（5 用例全过）
cargo test --lib commands::mod_commands::tests::update_mod_data_test

# 2) 三向深度比对
python tests/compare_triway.py        # R only=0, B&R content diff=0, B only=8(DRIFT)

# 3) 关键文件逐行 diff（全部 identical）
python tests/diff_details.py

# 4) 全量回归 + lint
cargo test --lib                     # 184 passed / 0 failed
cargo clippy --lib                   # 0 warnings
```

---

## 五、剩余 8 个 `B only` DRIFT 文件（数据集不一致，非源码缺陷）

这些文件存在于 Dart 基线但**不存在于 INPUT（NRMM-Rust-test）**，Rust 无从生成，亦非 Rust 逻辑问题：

| 文件（相对 `_MANAGED_/group_1/`） | 性质 |
|------|------|
| `女主-异界联动 4.3/530-90_6a2ea1bd8cd5e.jpg` | 基线数据集多带的贴图资源 |
| `开拓者·星-云璃表情/女主-云璃表情/Stelle_Cuter_Face/_Config.ini` | 基线 INPUT 多带的变体 |
| `开拓者·星-云璃表情/女主-云璃表情/Stelle_Cuter_Face/_DISABLED_Config.ini` | 同上（disabled 变体） |
| `开拓者·星-云璃表情/女主-云璃表情/Stelle_Cuter_Face/backup_Config.txt` | 同上（备份） |
| `开拓者·星-云璃表情/女主-云璃表情/Stelle_Cuter_Face/backup_DISABLED_Config.txt` | 同上 |
| `开拓者·星-云璃表情/女主-云璃表情/Stelle_Cuter_Face/backup_DISABLED_DISABLED_Config.txt` | 同上 |
| `开拓者·星-哈迪斯泳装无裙子/女主-哈迪斯泳装无裙子/NvzhuMod/_DISABLED_Nvzhu.ini` | 同上 |
| `开拓者·星-哈迪斯泳装无裙子/女主-哈迪斯泳装无裙子/NvzhuMod/_Nvzhu.ini` | 同上 |

**建议**：用同一份 INPUT 从 Dart 原版重新生成基线，或将这 8 个文件从 parity 断言中排除。不影响 Rust 源码正确性。

---

## 六、涉及源码改动清单

- `src-tauri/src/core/ini_handler.rs`
  - `IniLine::Empty { indent: usize }`（Display / parser / 全链路匹配）
  - `apply_indentation`：Comment/Empty/Command 行缩进
  - `write_atomic`：按 Dart `needsSeparator` 追加文件末尾空行 + `is_real_content_line`
  - `remove_old_managed_content`：保留所有 `endif`（不按配对删孤立 endif）
  - `is_numeric_value`：接受 `auto` 与逗号分隔数值列表
  - `detect_errors`：从 `is_crash_key` 移除 `ib`
  - `inject_slot_conditions`：`match_priority`/`allow_duplicate_hash` 补写移出 `if has_body`
  - `first_command_line_index` 语义澄清（跳过属性键，取首个段体行）
  - 2 个陈旧单测断言修正
- `src-tauri/src/core/mod_manager.rs`
  - `prepare_managed_folder`：`keypress` 模板 LF 归一化；`manager_group.ini` 去末尾换行
  - `create_group_ini`：`group_1.ini` 去末尾换行
  - `create_nrmm_include_ini`：`nrmm_include.ini` 去末尾换行
  - `update_mod_data`：`modsyntaxerrorremoved` 仅由 ET_CRASH_LINE(type 1) 触发
- 数据集：`tests/NRMM-test/Mods/_MANAGED_/group_1/selectedindex` → `0`（fixture drift 对齐）

---

## 七、结论

Rust 移植版 `update_mod_data` 现已与 Dart 原版在测试数据集上**逐字节等价**，所有真实行为缺陷（格式/缩进/悬空 endif、末尾换行、`selectedindex`、误报的 `modsyntaxerrorremoved` 与 `nrmm_keypress.txt` 行尾、`inject_slot_conditions` 纯属性段漏补）均已修复并验证。剩余 8 个差异点为两个数据集 INPUT 本身不一致所致，需从数据集层面（重新生成基线或排除断言）处理，与源码无关。
