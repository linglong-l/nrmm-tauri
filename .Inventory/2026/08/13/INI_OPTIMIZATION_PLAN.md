# INI 解析 I/O 与内存优化方案

> 目标：在**完全保持解析语义不变**（与 Dart/NRMM 基线逐字节一致）的前提下，
> 通过内存复用、缓冲池、流式读取与解析缓存，降低 INI 解析的 I/O 次数/ syscall 开销与内存峰值。

---

## 1. 现状与瓶颈分析

INI 解析相关的核心代码位于 `src-tauri/src/core/ini_handler.rs`（`IniFile::parse` /
`force_read_as_utf8`），调用方散布在 `mod_scanner.rs`、`mod_manager.rs`、`namespace_handler.rs`。

经代码审查，I/O 与内存的主要开销点：

| 瓶颈 | 位置 | 问题 |
|------|------|------|
| 整文件读入内存 | `IniFile::parse` → `force_read_as_utf8` → `fs::read` | 每个文件一次性 `read` 进 `Vec<u8>` 再转 `String`，峰值内存 = 整个文件；大文件尤为明显 |
| 每次读都重新分配缓冲 | `fs::read` 内部新建 `Vec<u8>` | 扫描成百上千个 INI 时产生海量短生命周期分配 |
| 逐行/逐键重复 `String` 分配 | `parse` 循环内 `line.to_string()` 等 | 不可完全避免（解析结果需拥有所有权以支持后续注入/写出），但读取缓冲可复用 |
| 模组 INI 无解析缓存（历史瓶颈） | `mod_scanner` / `namespace_handler` / `mod_manager` 的只读解析站点曾直接 `IniFile::parse` | 每次扫描/刷新都重新读取并解析全部模组 INI；仅 `d3dx.ini` 有 `D3dxIniCache` —— 已由 2.4 的 `mod_ini_cache` 解决 |
| 命令错误直接透传原始文本 | `mod_commands.rs` 中 `e.to_string()` | 路径、操作系统错误码、内部细节直接暴露给非专业用户 |

---

## 2. 优化策略（四大机制）

### 2.1 缓冲池（Buffer Pool）—— 跨调用复用读缓冲
- 新增线程级缓冲池（`thread_local!`）：`READ_BUF: RefCell<Vec<u8>>`、`LINE_BUF`/`DECODED_BUF: RefCell<String>`。
- `read_file_bytes` / `parse` 从池中取缓冲，用完归还（`RefCell::take` / 赋值回写），避免每个文件重新分配大块内存。
- 单线程内多次解析的总分配次数显著下降；多文件扫描场景收益最大。

### 2.2 流式读取（Streaming Read）—— 避免整文件驻留
- `IniFile::parse` 由「`fs::read` 整文件 + `content.lines()`」改为：
  `File::open` → `BufReader::with_capacity(256 KiB)` → `read_line` 逐行读入**复用的行缓冲**，
  逐行做有损 UTF-8 解码。
- **峰值内存从「整个文件」降为「单行」**，对超大 INI（含巨型 `[ShaderOverride]` 段）效果明显。
- 语义等价证明：
  - 换行处理：`read_line` 去除 `\n` 与行尾 `\r`，与 `str::lines()` + `trim_trailing_whitespace`（`\r\n \n 空格 \t` 均去除）结果一致；
  - BOM：首行剥离 UTF-8 BOM（`\u{FEFF}`），等价原 `force_read_as_utf8` 的 `[EF BB BF]` 剥离；
  - 编码：逐行 `String::from_utf8_lossy` 与整文件 `from_utf8_lossy` 等价（多字节字符不会跨行拆分）。
- `BufReader` 的 256 KiB 缓冲把底层 `read` syscall 次数降到最低（接近一次大块预读），直接降低 I/O 开销。

### 2.3 内存复用（Memory Reuse）
- 行缓冲 `line_buf` 与解码缓冲 `decoded` 在解析循环内通过 `clear()` 复用，**不**为每一行重新分配 `String` 承载读取内容（仅解析出的 `IniLine` 字段按需在堆上拥有副本，这是写入/注入语义所必需的，保持不变）。
- 读取字节缓冲 `READ_BUF` 在多次 `force_read_as_utf8` 调用间复用。

### 2.4 解析缓存（Parse Cache）—— 消除重复解析
- 新增 `mod_ini_cache.rs`：`get_or_parse_ini(path)` 以「规范化路径 + 修改时间 + 文件长度」为键缓存 `Arc<IniFile>`。
- **关键设计：读锁快速判定、未命中时在锁外执行真正解析、解析后再以写锁回填**（双重校验），避免把全局锁变成 I/O 串行化瓶颈。
- 命中时仅克隆 `Arc` 指针（廉价），未命中才 `IniFile::parse`。
- 已接入只读解析热路径：`mod_scanner`（`scan_mods_light`/`scan_mods`）、`namespace_handler`（命名空间收集）、`mod_manager`（库收集与命名空间规划两处只读站点）。读-改-写站点（`mod_manager` 崩溃行修复）仍走直接 `IniFile::parse` 以保证每次取得最新内容。
- 语义保证：缓存只存储与「直接 `IniFile::parse`」**完全相同**的解析结果，对写入/校验/对比零影响。

---

## 3. 错误信息规范化（见 `INI_ERROR_NORMALIZATION_RULES.md`）
- 新增 `error_normalizer.rs`：`FriendlyError{code,title,message}` + `normalize`/`err_to_ui`/`friendly_errored_line`。
- `mod_commands` 的 INI 命令边界（`get_mods`/`refresh_mods`/`update_mod_data`/`detect_hash_conflicts`）由 `e.to_string()` 改为 `err_to_ui(e)`，过滤路径/错误码/堆栈。
- `ErroredLines` 新增 `friendly_message` 字段（`detect_errors` 中由 `friendly_errored_line` 填充），前端直接展示，不暴露 `DUPLICATE LIB`/`CRASH LINE` 等技术字样。

---

## 4. 预期收益

| 维度 | 优化前 | 优化后 |
|------|--------|--------|
| 单文件峰值内存 | 整个文件 | 单行 + 固定缓冲 |
| 读取 syscall | 多次小读 | 大块预读（256 KiB BufReader） |
| 缓冲分配 | 每文件全新 `Vec<u8>` | 线程级缓冲池复用 |
| 重复扫描 I/O | 每次全量重读/重解析 | 缓存命中仅克隆 `Arc` |
| 用户可见错误 | 原始技术文本 | 中文友好提示 |

> 说明：解析缓存的命中率取决于 `mod_cache`（扫描结果缓存）是否被失效；
> 本优化在「扫描结果缓存失效、需重扫」与「命名空间收集」等重复解析场景下收益最显著。

---

## 5. 验证方法（门禁）
1. `cargo clippy --all-targets` 清零（含本次新增模块）。
2. `cargo test --lib ini_handler`：既有解析单测（基本/条件块/禁用行/往返/BOM 等）必须全部通过——这是语义等价的快速闸门。
3. `cargo test --test deep_compare_test`（NRMM 语义对齐）：作为**最终语义闸门**，确认解析/写出产物与基线逐字节一致、无新增分歧。
4. 手动冒烟：刷新模组列表、执行「更新模组数据」，确认 UI 错误提示为中文友好文本且功能正常。

---

## 6. 风险与回滚
- **语义风险**：已通过「逐行有损解码等价整文件」「BOM/换行处理对齐」论证，并由单测 + parity 测试门禁兜底。
- **并发风险**：解析缓存采用「读锁判定 + 锁外解析 + 写锁回填（双重校验）」，并发重复解析同一文件只会产生相同结果，无正确性问题。
- **回滚**：所有改动均为增量新增/局部替换；若需回滚，可 `git revert` 对应提交，不影响其它模块。
