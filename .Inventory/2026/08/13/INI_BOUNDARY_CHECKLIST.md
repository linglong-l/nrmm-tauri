# INI 解析边界条件处理清单

> 全面审查 `src-tauri/src/core/ini_handler.rs` 及相关调用方的边界处理情况。
> 分类：`正常`= 已知且正确解析；`容错`= 已知异常输入被安全兜底；`缺口`= 与基准(Dart/NRMM)可能存在差异，需对齐确认；`异常`= 未知/偶发，已记录待分类。

---

## 一、文件级边界

| # | 边界条件 | 当前处理 | 分类 | 备注 / 动作 |
|---|----------|----------|------|-------------|
| 1 | 空文件 | `lines()` 为空 → `preamble=[] sections=[]` | 正常 | 解析为空结构，无 panic |
| 2 | 仅含换行符（`\n`/`\r\n`） | 逐行 `Empty` 行 | 正常 | 与基准一致 |
| 3 | UTF-8 BOM (`EF BB BF`) | `force_read_as_utf8` 剥离首 3 字节；流式 `parse` 首行剥离 `\u{FEFF}` | 正常 | 两路径均已覆盖 |
| 4 | CRLF / LF / 旧版 Mac(`\r`) | `trim_trailing_whitespace` 去除 `\r\n 空格 \t`；流式 `read_line` 去除行尾 `\n`/`\r` | 正常 | 三种换行均已覆盖 |
| 5 | 文件末尾无换行 | 末行正常解析（`read_line` 返回不含换行的串） | 正常 | — |
| 6 | 超长单行（数十 KB） | `read_line` 按需增长缓冲，单行完整读入 | 正常 | 仅该单行驻留内存（流式优化后峰值可控） |
| 7 | 非 UTF-8 编码（GBK/损坏字节） | `from_utf8_lossy` 将非法字节替换为 `U+FFFD`（有损） | 容错 | 避免 panic；但可能改变原文语义，建议在 UI 提示编码异常（见错误规范化） |
| 8 | 文件被占用/无权限/不存在 | `File::open` 返回 `io::Error` → 命令层转为友好提示 | 容错 | 不再透传原始路径/错误码 |

---

## 二、行/段级边界

| # | 边界条件 | 当前处理 | 分类 | 备注 / 动作 |
|---|----------|----------|------|-------------|
| 9 | 纯注释 `;...` | `Comment` | 正常 | — |
| 10 | 禁用注释 `;-; key = value` | `DisabledKeyValue`（含注释尾） | 正常 | 与 Dart 对齐 |
| 11 | **`;+; key = value` 禁用键** | **当作普通 `Comment` 处理**（未识别为 `DisabledKeyValue`） | **缺口** | 代码注释（`prepend_header_comment`）称 `;+;` 为 disabled keys；当前 parser 仅识别 `;-;`。**需对齐 Dart/NRMM 基准确认是否应解析为 `DisabledKeyValue`**，并记录分类 |
| 12 | 段头 `[name]` | `SectionHeader`（解析时 `name` 经 `trim()`） | 正常 | — |
| 13 | 段头含空格 `[ name ]` | `line[1..len-1].trim()` 去除首尾空格 | 正常 | — |
| 14 | 段头缺右括号 `[name`（无 `]`） | 不命中段头规则，因含 `=` 走 `KeyValue`（键为 `[name`）；不含 `=` 走 `Command` | 容错 | 非标准写法被降级处理，未报错；建议列为已知异常输入 |
| 15 | 条件段前缀（`[KeyXxx]`/`[ShaderXxx]`…） | `is_conditional_section` 严格映射 `constants::CONDITIONAL_SECTION_PREFIXES` | 正常 | 与常量定义单一来源对齐 |
| 16 | `if`/`elif`/`else`/`endif` | 大小写**敏感**（`starts_with("if ")`） | 正常 | 3Dmigoto 关键字本就小写；大写 `IF` 视为命令/注释（与基准一致） |
| 17 | 嵌套/多层 `if` | `if_stack` 跟踪，可正确处理多层 | 正常 | — |
| 18 | 未闭合 `if`（缺 `endif`） | `detect_errors` 报 `MISSING ENDIF`，并标记 `missing_endif` | 容错 | 前端可展示友好提示 |
| 19 | 孤儿 `endif`（无匹配 `if`） | `detect_errors` 报 `FLOW CONTROL`（orphan endif） | 容错 | — |
| 20 | 引号值含 `;`：`value="a"; note` | `parse_key_value` 识别引号内 `;` 作为注释 | 正常 | — |
| 21 | 引号内转义引号 `value="a\"b"` | 首个 `"` 即闭合（`\"` 不被识别为转义） | **缺口** | 与 Dart 行为需对齐确认；当前会早闭合并可能截断，建议记录并补充转义用例 |
| 22 | `include = path` 与 `include path` | 均识别为 `Include`（首键 `include` 也识别） | 正常 | — |
| 23 | 键为空（`=value`、` ;=x`） | `parse_key_value` 键为空 → 返回 `None` → 视为 `Command` | 容错 | 非标准写法的兜底 |
| 24 | 大小写不敏感键比较 | `to_lowercase()` 比较 `include`/`hash`/`draw` 等 | 正常 | — |
| 25 | 行尾注释 `key = v ; comment` | 值去尾空格；`;` 后作为 `comment` | 正常 | — |
| 26 | 空行 | `Empty { indent:0 }`（节内/节外分别归属） | 正常 | — |
| 27 | 多字节/全角/emoji（合法 UTF-8） | `from_utf8_lossy` 原样保留 | 正常 | — |

---

## 三、语义/校验级边界

| # | 边界条件 | 当前处理 | 分类 | 备注 / 动作 |
|---|----------|----------|------|-------------|
| 28 | 重复库段（同模组多个同名 `[lib]`） | `detect_errors` 报 `DUPLICATE LIB` | 正常 | 转为友好提示「配置段重复定义」 |
| 29 | 跨模组引用不存在库（`run = UnknownNs\X`） | `detect_errors` 报 `NON EXISTENT LIB`（排除 `Resource*`/`builtincommandlist*`） | 正常 | 已排除资源引用与内建命令列表 |
| 30 | 崩溃行（`draw`/`drawindexed` 取非法值） | 报 `CRASH LINE`（已剔除 `ib`/`vb*` 缓冲引用误报） | 正常 | 历史已修复 `Stelle/Config/Nvzhu` 误报 |
| 31 | 路径过长（>260 字符） | 报 `PATH TOO LONG` | 正常 | 友好提示移动模组位置 |
| 32 | 命名空间变量展开（`$ns\var`） | `namespace_handler::expand_ini_variables` | 正常 | 已处理已限定/未限定两种形态 |
| 33 | `;-; DISABLED_BY_NRMM` 标记行 | `remove_old_managed_content` 清理识别 | 正常 | 防止重复累积 |
| 34 | 解析 I/O 失败（`namespace_handler`） | 原 `if let Ok(ini) = IniFile::parse(...)` 静默吞掉错误；**已改为 `match` + `log::warn!` 记录具体路径与错误**，仅跳过该文件、保留目录其余收集结果 | **已改进** | 不再完全静默；真实故障记入日志供排查，前端暂不直接感知（命名空间收集为辅助信息） |

---

## 四、分类汇总与后续动作

- **已知正常 / 容错（绝大多数）**：清单一~三项中除标注缺口/异常者，均已正确处理。
- **需对齐基准（缺口）**：
  - `#11` `;+;` 禁用键是否应解析为 `DisabledKeyValue`；
  - `#21` 引号内 `\"` 转义是否被 Dart 支持。
  → 在 NRMM-test 基线中补充对应用例，确认后同步 Rust 实现。
- **异常/待记录（异常）**：
  - `#34` `namespace_handler` 静默吞掉解析错误 → **已修复**：改为 `match` + `log::warn!` 记录具体路径与错误，仅跳过该文件、保留目录其余收集结果，不再隐藏真实故障（见上文）。
- **已消除的暴露面**：命令层原始错误文本（路径/错误码/堆栈）已通过 `error_normalizer` 转为友好提示（见错误规范化规则）。

> 所有「已知边界」在优化（流式/缓冲池/缓存）后均经单测 + `deep_compare_test` 门禁验证语义不变。
