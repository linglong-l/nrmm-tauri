# INI 错误信息规范化规则

> 目标：所有**需要传递给前端**的 INI 相关错误信息，统一转化为**非专业人员可直观理解**的
> 友好提示，**避免暴露技术细节**（文件路径、操作系统错误码、堆栈、内部关键字）。

---

## 1. 总体原则

1. **只说「现象 + 用户能做什么」**，不说「底层为什么失败」。
2. **绝不**向用户展示：绝对路径 / 相对路径、`os error N`、`panic`、`unwrap`、
   `DUPLICATE LIB` / `CRASH LINE` / `NON EXISTENT LIB` 等技术字样、Rust/Cargo 内部信息。
3. 错误保留一个**机器可读 `code`**（供前端做条件展示/埋点），但 `code` 不展示给用户。
4. 与解析产物语义无关：规范化只改变「错误如何被描述」，不改变解析/写入结果。

---

## 2. 两层错误来源与规范化

### 2.1 第一层：底层技术错误（IO / 编码 / 解析）
来源：文件读取、解码、解析过程中抛出的 `anyhow::Error`。
入口：`error_normalizer::normalize` / `err_to_ui`。

| `io::ErrorKind` | code | 标题 | 用户提示（message） |
|------------------|------|------|----------------------|
| `NotFound` | `file_not_found` | 找不到文件 | 找不到指定的文件或文件夹，请确认路径是否正确，或文件是否已被移动、重命名或删除。 |
| `PermissionDenied` | `permission_denied` | 没有访问权限 | 没有权限读取该文件，请检查文件是否被其他程序占用，或以管理员身份运行后重试。 |
| `AlreadyExists` | `already_exists` | 文件已存在 | 目标文件已存在，请确认是否重复执行了相同操作。 |
| `InvalidInput` | `invalid_path` | 路径无效 | 文件路径包含无效字符，请检查路径格式后重试。 |
| `TimedOut` / `WouldBlock` | `io_timeout` | 读取超时 | 读取文件超时，请检查磁盘或网络存储是否连接正常后重试。 |
| `StorageFull` / `QuotaExceeded` | `disk_full` | 磁盘空间不足 | 磁盘空间不足，无法完成操作，请清理空间后重试。 |
| `Interrupted` | `interrupted` | 操作被中断 | 操作被系统中断，请重试。 |
| `IsADirectory` / `NotADirectory` | `is_a_directory` / `not_a_directory` | 路径类型错误 | 预期为文件/文件夹，但实际类型不符，请检查配置路径。 |
| 其它 / 编码错误 | `io_error` / `encoding_error` | 文件读取失败 / 文件编码异常 | 读取或处理文件时发生未知错误，请重试；若问题持续，请记录操作时间并联系支持。 / 文件不是有效的文本（编码异常），请将其另存为 UTF-8 编码后重试。 |
| 未知（非 IO） | `internal_error` | 操作未完成 | 处理过程中发生未知错误，请重试；若问题持续，请记录操作时间并联系支持。 |

> 编码错误：INI 走有损解码（`from_utf8_lossy`）通常不会抛出 `Utf8Error`，但其它路径可能；
> 命中时统一提示「另存为 UTF-8」。

### 2.2 第二层：结构化校验错误（`ErroredLines`）
来源：`IniFile::detect_errors` 产出的语义错误（重复库、崩溃行、缺 endif 等）。
入口：`error_normalizer::friendly_errored_line(error_type, error_message, line_number)`，
结果写入 `ErroredLines.friendly_message`。

| error_type | 原始 message（仅内部用，不展示） | 用户提示（friendly_message） |
|:----------:|-------------------------------|------------------------------|
| 0 DUPLICATE LIB | `DUPLICATE LIB: X` | 第 N 行：模组「X」的配置段被重复定义，请合并或删除多余的段，避免冲突。 |
| 1 CRASH LINE | `CRASH LINE` | 第 N 行：存在可能导致程序崩溃的绘制指令取值，建议检查该值或暂时注释此行。 |
| 2 MISSING ENDIF | `Missing "endif"` | 第 N 行：if 条件块缺少对应的 endif 结束标记，请在该块的末尾补充 endif。 |
| 3 FLOW CONTROL | `FLOW CONTROL: orphan endif` | 第 N 行：流程控制结构有误（如多余的 endif），请检查 if / else / endif 是否配对正确。 |
| 5 NON EXISTENT LIB | `NON EXISTENT LIB: X` | 第 N 行：引用了不存在的库「X」，请确认对应的依赖模组已启用，或检查名称拼写。 |
| 6 PATH TOO LONG | `PATH TOO LONG` | 配置路径过长（超过 260 字符），Windows 可能无法正常访问，请将模组移动到路径更短的位置。 |
| 其它 | — | 第 N 行：该配置项存在异常，请检查后重试。 |

> `N` 为 `line_number`；文件级错误（如路径过长）`line_number = 0`，提示不含「第 N 行」。

---

## 3. 禁止出现在用户提示中的内容（红线）

- 绝对/相对文件路径、文件名拼接细节；
- `os error N`、`errno`、系统调用名；
- `panic`、`unwrap`/`expect`、`index out of bounds` 等 Rust 内部术语；
- `DUPLICATE LIB` / `CRASH LINE` / `MISSING ENDIF` / `NON EXISTENT LIB` 等内部关键字；
- 堆栈跟踪、模块路径（`src-tauri/...`）、提交哈希。

---

## 4. 实现与接入点

- **模块**：`src-tauri/src/core/error_normalizer.rs`
  - `FriendlyError { code, title, message }`（serde 序列化，供命令返回）；
  - `normalize(&Error) -> FriendlyError`、`err_to_ui(Error) -> String`；
  - `friendly_errored_line(u8, &str, u32) -> String`；
  - `join_error_to_ui() -> String`：后台任务（`spawn_blocking`）因 panic / 取消而中断时的统一提示（不暴露 panic 栈）。
- **命令边界**（`commands/mod_commands.rs`）：
  `get_mods` / `refresh_mods` / `update_mod_data` / `detect_hash_conflicts` 已做两层规范化——
  - 内部 `anyhow::Error`：`.map_err(|e| err_to_ui(e))`；
  - 外层 `spawn_blocking` 的 `JoinError`（任务 panic / 被取消）：`.map_err(|e| { log::error!(...); join_error_to_ui() })`，
    不再把 panic 栈、`update task failed` 等原始文本透传给前端。
- **结构化错误**：`ErroredLines` 新增 `friendly_message` 字段，由 `detect_errors` 在返回前统一填充。

---

## 5. 前端使用约定

- 展示错误时**优先使用 `friendly_message`**；`error_message`（原始技术串）仅用于开发期调试，不得直接展示。
- 命令级失败：命令返回的错误文本已是友好 `message`，可直接 toast/弹窗展示。
- 需要条件化展示/埋点时，使用 `FriendlyError.code`（或保留的错误类型），不要对 `friendly_message` 做字符串匹配。

---

## 6. 校验方式

- 单测：为 `error_normalizer` 补充各 `ErrorKind` 与 `error_type` 的映射断言（建议）。
- 冒烟：构造「文件不存在」「路径过长」「重复库段」等场景，确认 UI 显示中文友好提示、无路径/错误码泄露。
