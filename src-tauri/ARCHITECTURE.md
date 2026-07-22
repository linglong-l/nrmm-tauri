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
