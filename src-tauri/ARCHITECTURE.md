# 项目架构

## 职责划分

项目采用**分层架构**，从工具层到业务层逐级依赖，禁止逆向调用。

```

┌────────────────────────────────────────────────────────────────────┐
│                     UI / 视图层                                    │
│  Vue Components (*.vue)                                            │
│  App.vue, ModsTab.vue, SettingsView.vue, GroupTreeNode.vue ...    │
│  职责: 用户交互、事件处理、UI 渲染                                 │
│  依赖: Store 层、Composables、Utils 层                            │
├────────────────────────────────────────────────────────────────────┤
│                     Store / Composable 层                          │
│  Pinia Stores (game.ts, settings.ts, hotkey.ts)                   │
│  Composables (useWindow, useCloudData, useHashConflict)            │
│  职责: 状态管理、业务逻辑编排                                       │
│  依赖: invoke.ts、Utils 层                                         │
├────────────────────────────────────────────────────────────────────┤
│                     IPC 封装层                                      │
│  invoke.ts                                                        │
│  职责: Tauri IPC 命令封装，统一的 invoke 入口                       │
│  依赖: @tauri-apps/api/core                                        │
├────────────────────────────────────────────────────────────────────┤
│                     Tauri 命令层 (Rust)                             │
│  commands/mod.rs                                                   │
│  职责: 命令注册、参数校验、TaskQueue 调度                           │
│  依赖: 后端业务层、基础设施层                                       │
├────────────────────────────────────────────────────────────────────┤
│                     后端业务层 (Rust)                               │
│  mod_manager/    (模组管理核心 + game_interaction)                 │
│  ini_handler/    (INI 解析 + 错误检测)                             │
│  file_watcher/   (文件监听)                                        │
│  window_manager/ (窗口状态管理)                                    │
│  hotkey/         (全局热键)                                        │
│  settings/       (设置持久化)                                      │
│  职责: 领域逻辑、业务流程编排                                       │
│  依赖: 后端工具层、基础设施层                                       │
├────────────────────────────────────────────────────────────────────┤
│                     基础设施层 (Rust)                                │
│  task_queue/     (通用异步任务队列)                                 │
│  process/        (进程检测)                                        │
│  keypress_simulator/ (通用键鼠模拟，无业务逻辑)                    │
│  职责: 通用服务、跨领域基础设施                                     │
│  依赖: 工具层                                                      │
├────────────────────────────────────────────────────────────────────┤
│                     工具层 (Rust)                                   │
│  utils/          (平台检测 is_linux/is_wsl + 日志采样)             │
│  ini_handler/    (INI 解析引擎，纯解析不包含业务语义)               │
│  mod_manager/path_utils.rs (路径操作工具)                          │
│  职责: 纯函数、无状态、无领域概念                                  │
│  依赖: 无（或仅依赖标准库/第三方 crate）                           │
└────────────────────────────────────────────────────────────────────┘
```

## 核心原则

### 1. 单一职责原则 (SRP)

每个模块只负责一个关注点，不混合多层职责：

| ✅ 应当放在工具层 | ❌ 不应当放在工具层 |
|---|---|
| 键鼠模拟 (key_press, key_down, key_up) | NRMM 特有的按键序列协议 |
| INI 文件解析 (parse, write, serialize) | 模组管理流程编排 |
| 路径操作 (检查、提取名称) | 路径安全校验 (is_valid_mod_path) |
| 通用防抖/节流 | 设置保存流程 |
| 日志工具 | 更新模组数据的业务逻辑 |

### 2. 依赖方向

```
工具层 ← 基础设施层 ← 后端业务层 ← Tauri 命令层
                                                  → IPC → 前端
```

- 上层可以依赖下层，下层不能依赖上层
- 工具层不能依赖任何业务模块
- 基础设施层不能依赖具体业务逻辑
- 同层之间尽量通过接口/抽象解耦

### 3. 跨层调用规则

- **禁止** 业务层直接调用 OS API（需通过工具层或基础设施层封装）
- **禁止** 工具层包含业务判断逻辑（如 `if is_disabled { ... }`)
- **禁止** UI 层直接调用后端 `invoke`（必须通过 `invoke.ts` 封装）
- **允许** 业务层调用多个工具层服务进行编排

## 模块职责说明

### 前端

| 模块 | 文件 | 职责 | 是否纯工具 |
|------|------|------|-----------|
| `utils/cache.ts` | localStorage 封装 | 纯工具 |
| `utils/constants.ts` | 静态常量与映射表 | 纯工具 |
| `utils/debounce.ts` | 通用防抖函数 | 纯工具 |
| `utils/format.ts` | 通用格式化函数 | 纯工具 |
| `utils/fuzzyMatch.ts` | 子序列模糊匹配 | 纯工具 |
| `utils/hotkeyValidator.ts` | 快捷键冲突检测 | 纯工具 |
| `utils/logger.ts` | 前端日志工具 | 纯工具 |
| `utils/events.ts` | 事件总线 (Tauri + 原生) | 基础设施 |
| `utils/invoke.ts` | Tauri IPC 封装 | 基础设施 |
| `stores/` | Pinia 状态管理 | 业务层 |
| `composables/` | 组合式功能封装 | 业务层 |
| `pages/` | 页面组件 + 交互逻辑 | 业务层 |

### 后端

| 模块 | 文件 | 职责 | 是否纯工具 |
|------|------|------|-----------|
| `utils/` | 平台检测 + 日志采样 | 纯工具 |
| `mod_manager/path_utils.rs` | 路径操作 | 纯工具 |
| `ini_handler/` | INI 解析引擎 (不含业务) | 纯工具 |
| `ini_handler/error_detection.rs` | INI 错误检测 | 纯工具 |
| `task_queue/` | 异步任务队列 | 基础设施 |
| `process/` | 进程检测 | 基础设施 |
| `keypress_simulator/` | 通用键鼠模拟 (不含业务逻辑) | 基础设施 |
| `mod_manager/game_interaction.rs` | NRMM 游戏交互协议 | **业务层** |
| `mod_manager/` | 模组管理核心 | 业务层 |
| `commands/` | Tauri 命令入口 | 业务层 |

## 调用规范

### 新增函数的放置原则

1. 是否为纯通用算法（不涉及模组、分组、游戏等概念）？
   - 是 → 放 `utils/`
2. 是否为通用服务（不依赖具体业务但涉及 OS/平台能力）？
   - 是 → 放基础设施层（如 `keypress_simulator/`、`process/`）
3. 是否为领域逻辑（涉及模组、分组、INI 管理等概念）？
   - 是 → 放 `mod_manager/` 或对应业务模块

### 错误处理规范

- 工具层函数返回通用错误类型（`anyhow::Error` 或 `Option`/`Result`）
- 业务层函数在调用工具层时添加领域上下文（使用 `.with_context()`）
- 命令层将内部错误转换为用户友好的字符串消息

## IPC 边界类型（路径校验前移到反序列化）

### 动机

`#[tauri::command]` 的路径入参此前一律是 `String`，边界校验靠每个命令**自己记得**调用
`settings_store::validate_managed_path`——实测 85 个命令只有 10 处调用。漏掉的每一处都是
任意文件读/写/改名/执行面（例如 `save_mod_keybinds` 可覆写任意文本文件的任意行）。
更糟的是校验锚点本身可被前端改写：`save_settings` 接受完整 `AppSettings`，其中就包含
被用作锚点的 `game_mods_path`。

### 机制

Tauri v2 为任何实现 `serde::Deserialize` 的类型自动提供 `tauri::CommandArg`
（`tauri/src/ipc/command.rs`），因此命令入参在**进入函数体之前**完成解析。把不变量写进
`Deserialize`，「忘记校验」在类型层面即不可能。

选 `Deserialize` 而不是手写 `CommandArg`：后者会失去 blanket 实现，导致嵌在 DTO 里的
路径字段（`KeybindFile.file_path`、`ImportItemRequest.items`）拿不到同等保护——而那正是
任意文件覆写的实际入口。

### 类型分层（`src/ipc/typed_paths.rs`）

| 类型 | 语义 | 保证的不变量 |
| :--- | :--- | :--- |
| `ManagedPath` | 会被写/改名/删除/执行的**目标** | 绝对、无 `..`、实际存在、canonical 形态落在已登记 Mods 根内 |
| `SourcePath` | **只读来源**（拖入的压缩包、导入的目录） | 绝对、无 `..`、实际存在；不要求落在根内（否则拖拽导入直接失效） |

纯展示字段继续用 `String`（由静态门禁按「是否参与 IO」放行），不为其再造包装。
两者都 `Deref<Target = Path>` 到规范化形态，既有 `&Path` 形参的调用点无需改写；需要原
字符串时用 `.requested()`（日志、按原名推导 `DISABLED_` 目标）。

### 锚点：`src/ipc/mods_root_registry.rs`

- 全局集合，只由 `register()` / `unregister()` / `sync_from()` 三条后端通道改变；
  命令入参只读该表。
- `sync_from()` 会剔除无法规范化或不是目录的条目并告警——宁可拒绝命令，也不把指向未知
  位置的路径当可信锚点。
- **fail-closed**：表为空时 `is_within_any_root()` 恒 `false`，所有 `ManagedPath`
  解析失败并返回 `[ROOT_NOT_REGISTERED]`（与 `E_OUT_OF_MANAGED_ROOT` 区分，避免误导排障）。
- 比较两侧均为 `canonicalize` 产物，故符号链接/junction 指向根外时其真实形态不匹配任何根，
  被自然拦下（有用例钉住）。
- 本模块只依赖 `std`/`serde`/`anyhow`，不引用 `core`/`commands`/`window`/`tray`/
  `hotkey`/`updater`，以便纳入分层冻结表。

### 错误契约

解析错误串统一带 `[CODE]` 前缀（`src/ipc/ipc_errors.rs`），沿用仓库既有约定，使前端
`safeInvoke` 与按前缀分支的逻辑（如 `[PATH_NOT_FOUND]` 不置位 `loadError`）不因类型化而失效。

### 当前进度与新增命令的规矩

- Phase 0（已完成）：类型、锚点表、错误码与 15 项用例落地；**命令签名尚未迁移**，
  现有 `validate_managed_path` 调用点保持原样，行为零变化。
- 迁移进行中：写/执行类命令优先（改名、删除、启停、还原、导出目标、按键 DTO），
  随后读/执行类（打开目录、缩略图、外部编辑器）。
- 迁移期内新增命令的规矩：路径入参**必须**用 `ManagedPath`/`SourcePath`，禁止再用
  `String` 收路径；计划以 `tests/arch/arch_gate.rs` 的静态门禁冻结该规矩
  （F6：命令签名不得出现名为 `*_path`/`*paths`/`mod_dir` 的 `String`；
  F7：`commands/` 内禁止直接调用 `validate_managed_path`，防止半迁移状态固化）。

### 游戏根：唯一越过 Mods 锚点的写路径（`core::mod_path_utils::GameRoot`）

`d3dx.ini` / `d3dx_user.ini` 位于 **Mods 的父目录**，即路径校验锚点之外。此前四处各自
写一遍 `parent().unwrap_or(...)`，其中三处是写路径：一旦 Mods 根无父目录（盘根），
兜底会把「游戏根」退化成 Mods 目录本身，主 INI 就被写进模组树且毫无提示。

现收敛为一个由**服务端推导**的类型：`GameRoot::from_mods_root()` 要求父目录存在且是目录，
否则 `Err`（`[NO_GAME_ROOT]` / `[GAME_ROOT_NOT_FOUND]`）。它不是 IPC 入参，任何命令都
无法由前端指定它，锚点本身仍只能经 `set_game_mods_path` 登记。`Deref<Target = Path>`
使既有 `game_root.join(..)` 调用点无需改写。

采用口径按语义分档：

| 位置 | 语义 | 推导失败时 |
| :--- | :--- | :--- |
| `mod_update_pipeline::update_mod_data` | 写主 INI 与 include 基准 | 传播 `Err`（宁可失败也不写错位置） |
| `mod_ini_admin::save_customizations` | 写 `d3dx_user.ini` | 传播 `Err` |
| `mod_restore::restore_all_inis` | 尽力还原主 INI | 跳过该步 + `warn`，模组级还原照常 |

已知残留（有意未在本批处理）：`mod_ini_admin.rs` 里 `d3d11.dll` 能力探测仍用
`managed_path.parent().and_then(parent).unwrap_or(managed_path)`，失败时静默退回默认按键
模板。它是**读探测**而非越界写入，危害等级低，但同样该收进 `GameRoot`；改造需要
`dll_capability` 先暴露「默认模板」入口，避免在此处硬编码模板名。
