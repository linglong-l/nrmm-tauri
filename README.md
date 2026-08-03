# XXMI-NRMM

## 基于 [No Reload Mod Manager](https://github.com/Aglglg/No-Reload-Mod-Manager)

> XXMI 模组管理器（NR Mod Manager）的桌面客户端 v0.1.1

一款基于 Tauri 2 + Vue 3 的轻量级模组管理工具，为游戏玩家提供模组的浏览、启用/禁用、分组管理、收藏等核心功能。支持 Windows 11 和 Linux 平台。

* 快捷键功能支持不完善 等于完全不支持，当然是所有快捷键啦

## 平台支持

| 平台 | 状态 | 说明 |
|------|------|------|
| Windows 11 | ✅ 已测试 | 完全支持 |
| Linux | ✅ 已测试 | 通过条件编译支持，使用 xdg-open 打开文件管理器 |
| Windows 10 | ⚠️ 未测试 | 可能兼容，但未进行测试 |
| macOS | ❌ 不支持 | 未适配 macOS 平台 |

## 技术栈

### 前端

| 技术 | 版本 | 用途 |
|------|------|------|
| Vue 3 | 3.x | 前端框架，使用 `<script setup>` SFC |
| TypeScript | 6.x | 类型安全的 JavaScript 超集 |
| Vite | 6 | 构建工具与开发服务器 |
| Pinia | 3 | 状态管理 |
| Element Plus | 2 | UI 组件库 |
| vue-i18n | 9 | 多语言国际化 |

### 后端

| 技术 | 版本 | 用途 |
|------|------|------|
| Rust | 1.96+ | 系统级编程语言 |
| Tauri | 2 | 桌面应用框架（含 `tray-icon` feature） |
| tokio | 1.x | 异步运行时 |
| global-shortcut | - | Tauri 插件，全局热键 |
| dialog | - | Tauri 插件，对话框 |
| `windows` crate | 0.62 | Win32 API（进程检测、键盘模拟、窗口控制） |

### 核心模块

- **mod_manager**：模组扫描、启用/禁用、分组管理核心逻辑
- **mod_manager/game_interaction**：NRMM 游戏交互协议（按键序列）
- **commands**：Tauri IPC 命令注册与处理
- **settings**：应用设置持久化（JSON 文件）
- **window_manager**：窗口位置、尺寸、置顶状态管理
- **hotkey**：全局热键注册与处理
- **file_watcher**：文件系统监听（500ms 防抖）
- **process**：目标游戏进程检测
- **keypress_simulator**：通用键鼠模拟（无业务逻辑）
- **task_queue**：异步任务队列（保证全局单一性）

## 环境要求

### 通用要求

- **Node.js**：≥ 18
- **Rust**：≥ 1.96（stable toolchain）
- **Tauri CLI**：通过 `npm install` 自动安装（`@tauri-apps/cli`）

### Windows 特定要求

- **MSVC Build Tools**：安装 "Desktop development with C++" 工作负载
- **WebView2 Runtime**：Windows 11 自带，无需额外安装

### Linux 特定要求

- **libwebkit2gtk-4.0-dev**：WebView 组件依赖
- **libappindicator3-dev**：系统托盘依赖
- **xdg-utils**：文件管理器打开功能

## 开发

```bash
# 安装依赖
npm install

# 启动开发服务器（Vite + Tauri 热重载）
npm run tauri dev

# 类型检查（仅前端）
vue-tsc --noEmit

# 仅构建前端（产物在 dist/）
npm run build
```

## 构建发布版本

```bash
# Windows 构建
npm run tauri build

# Linux 构建（需先安装系统依赖）
npm run tauri build
```

产物位于 `src-tauri/target/release/bundle/`，支持 MSI、NSIS（Windows）和 deb/rpm（Linux）等安装包格式。

## 多语言

界面支持以下语言（`src/utils/i18n/`）：

| 语言 | 代码 | 文件 |
|------|------|------|
| 简体中文 | zh-CN | zh-CN.json |
| 繁体中文 | zh-TW | zh-TW.json |
| English | en | en.json |
| Русский | ru | ru.json |
| Bahasa Indonesia | id | id.json |

切换语言后需重启应用才能完全生效。

## 使用方法

### 1. 选择游戏

在顶部游戏选择器中选择目标游戏，应用会自动扫描该游戏的 Mods 目录。

### 2. 浏览模组

左侧导航栏显示分组树状结构，点击分组后右侧展示该分组下的所有模组。支持：
- 网格布局与行列布局切换
- 鼠标拖动滚动（手机风格操作）
- 滚动条拖动与鼠标滚轮

### 3. 启用/禁用模组

- 点击模组卡片即可启用或禁用
- 禁用的模组会显示红色描边
- 默认启用的模组显示蓝色描边
- **# 目录互斥模式**：在以 `#` 开头的目录下，启用某模组会自动禁用同目录下的其他所有模组

### 4. 分组管理

- 左侧导航栏显示分组树状结构
- 支持拖拽调整导航栏宽度（160px ~ 480px）
- 点击分组展开/折叠子分组
- 右键分组可进行重命名、删除等操作

### 5. 收藏功能

- 点击模组卡片上的星星图标收藏/取消收藏
- 收藏的模组在列表中优先显示
- 支持按收藏筛选

### 6. 搜索功能

在顶部搜索框输入关键字，按 Enter 或点击搜索按钮查找模组。

### 7. 更新模组数据

在设置页面点击"更新模组数据"按钮，应用会将模组目录整理到 `_MANAGED_` 目录下。

## 核心功能

### 模组管理

- ✅ 模组浏览与筛选
- ✅ 启用/禁用模组（通过目录重命名添加/移除 DISABLED 前缀）
- ✅ 模组收藏（优先显示）
- ✅ 模组重命名（保留禁用状态）
- ✅ 模组删除
- ✅ 搜索模组

### 分组管理

- ✅ 分组树状结构展示
- ✅ 分组展开/折叠
- ✅ 分组收藏
- ✅ 分组重命名
- ✅ **# 目录互斥启用**（单选模式）

### 界面特性

- ✅ 网格布局与行列布局切换
- ✅ 整体缩放调节（支持 0.5x ~ 2.0x）
- ✅ 背景透明度调节
- ✅ 圆角无边框窗口 + 磨砂黑背景
- ✅ 手机风格拖动滚动（左侧导航栏与右侧模组区域）
- ✅ 导航栏宽度可拖拽调整
- ✅ 隐藏原生滚动条
- ✅ 右键菜单（边界检测，防止窗口外展示）

### 系统集成

- ✅ 全局热键切换窗口（支持键盘与手柄）
- ✅ 系统托盘菜单
- ✅ 目标进程检测（仅在指定游戏运行时响应热键）
- ✅ 文件系统监听（500ms 防抖）
- ✅ 设置导入/导出/重置
- ✅ 在文件管理器中打开模组目录（支持 Windows 和 Linux）

### 多语言支持

- ✅ 简体中文、繁体中文、English、Русский、Bahasa Indonesia

## 项目结构

```
xxmi-nrmm/
├── src/                          # Vue 前端
│   ├── components/               # 通用组件
│   │   ├── GameSelector.vue      # 游戏选择器组件
│   │   ├── SideNav.vue           # 侧边导航栏组件
│   │   ├── StatusBar.vue         # 状态栏组件
│   │   ├── TitleBar.vue          # 标题栏组件
│   │   └── index.ts              # 组件导出
│   ├── composables/              # 组合式函数
│   │   ├── useGame.ts            # 游戏状态管理封装
│   │   ├── useSettings.ts        # 设置状态管理封装
│   │   ├── useHotkey.ts          # 热键功能封装
│   │   ├── useWindow.ts          # 窗口功能封装
│   │   ├── useCloudData.ts       # 云数据功能封装
│   │   └── useInvoke.ts          # 后端调用封装
│   ├── locales/                  # 多语言配置
│   │   └── index.ts              # i18n 初始化
│   ├── pages/                    # 页面
│   │   └── index/
│   │       ├── index.vue         # 主页面（标签页容器）
│   │       └── tabs/
│   │           ├── ModsTab.vue   # 模组管理标签页（核心页面）
│   │           └── GroupTreeNode.vue  # 分组树节点组件（递归渲染）
│   ├── stores/                   # Pinia 状态管理
│   │   ├── game.ts               # 游戏/模组数据 Store
│   │   ├── settings.ts           # 应用设置 Store
│   │   ├── hotkey.ts             # 热键状态 Store
│   │   ├── ui.ts                 # UI 状态 Store
│   │   ├── cloudData.ts          # 云数据 Store
│   │   └── cloudDataStore.ts     # 云数据备用 Store
│   ├── types/                    # TypeScript 类型定义
│   │   └── index.ts              # 全局类型定义
│   ├── utils/                    # 工具函数
│   │   ├── cache.ts              # localStorage 缓存工具
│   │   ├── constants.ts          # 常量定义
│   │   ├── debounce.ts           # 通用防抖函数
│   │   ├── events.ts             # 事件系统
│   │   ├── format.ts             # 格式化工具（耗时等）
│   │   ├── fuzzyMatch.ts         # 子序列模糊匹配
│   │   ├── hotkeyValidator.ts    # 快捷键冲突检测
│   │   ├── index.ts              # 工具导出
│   │   ├── invoke.ts             # Tauri 后端调用封装
│   │   └── logger.ts             # 前端日志工具
│   ├── views/                    # 视图组件
│   │   ├── SettingsView.vue      # 设置页面
│   │   └── index.ts              # 视图导出
│   ├── assets/                   # 静态资源
│   │   ├── fonts/                # 自定义字体
│   │   ├── images/               # 应用图标
│   │   ├── keys_icon/            # 热键图标
│   │   └── template_txt/         # 模板文件
│   ├── utils/i18n/               # 翻译文件（i18n 实际加载目录）
│   ├── App.vue                   # 应用根组件
│   └── main.ts                   # 应用入口文件
│
├── src-tauri/                    # Rust 后端
│   ├── capabilities/             # Tauri 权限配置
│   │   └── default.json          # 默认权限配置
│   ├── icons/                    # 应用图标（多尺寸）
│   ├── src/
│   │   ├── cloud_data/           # 云数据同步模块
│   │   │   └── mod.rs
│   │   ├── commands/             # Tauri Command 注册
│   │   │   └── mod.rs            # 所有命令定义与处理
│   │   ├── file_watcher/         # 文件监听模块
│   │   │   └── mod.rs
│   │   ├── hotkey/               # 全局热键模块
│   │   │   └── mod.rs
│   │   ├── ini_handler/          # INI 文件解析模块
│   │   │   ├── mod.rs
│   │   │   └── error_detection.rs
│   │   ├── init_xx/              # XXMI 初始化与日志模块
│   │   │   ├── logger.rs
│   │   │   └── mod.rs
│   │   ├── keypress_simulator/   # 通用键鼠模拟模块（无业务逻辑）
│   │   │   └── mod.rs
│   │   ├── mod_manager/          # 模组管理核心模块
│   │   │   ├── mod.rs            # 扫描、启用/禁用、分组管理
│   │   │   ├── path_utils.rs     # 路径操作工具
│   │   │   └── game_interaction.rs # NRMM 游戏交互协议（按键序列）
│   │   ├── process/              # 进程检测模块（纯工具）
│   │   │   └── mod.rs
│   │   ├── settings/             # 设置持久化模块
│   │   │   └── mod.rs
│   │   ├── task_queue/           # 任务队列模块（保证全局单一性）
│   │   │   └── mod.rs
│   │   ├── tray/                 # 系统托盘模块
│   │   │   └── mod.rs
│   │   ├── window_manager/       # 窗口状态管理模块
│   │   │   └── mod.rs
│   │   ├── lib.rs                # 库入口，命令注册与模块导出
│   │   ├── main.rs               # 应用入口，启动流程
│   │   └── state.rs              # 应用全局状态
│   ├── Cargo.lock                # Rust 依赖锁定文件
│   ├── Cargo.toml                # Rust 项目配置
│   ├── build.rs                  # 构建脚本
│   └── tauri.conf.json           # Tauri 配置文件
│
├── public/                       # 静态资源（Vite 公共目录）
├── index.html                    # HTML 入口文件
├── package.json                  # 前端依赖配置
├── tsconfig.json                 # TypeScript 配置
├── tsconfig.node.json            # TypeScript Node 配置
├── vite.config.ts                # Vite 配置
├── .gitignore                    # Git 忽略配置
├── ARCHITECTURE.md               # 项目架构文档（详见 src-tauri/ARCHITECTURE.md）
├── LICENSE                       # 许可证文件
└── README.md                     # 项目文档（本文档）
```

## 常见问题（FAQ）

### Q: 为什么模组列表为空？

A: 请检查以下几点：
1. 确认已选择正确的游戏
2. 确认该游戏的 Mods 路径配置正确（在设置页面查看）
3. 确认 Mods 目录存在且包含模组文件夹
4. 模组文件夹需包含 `.ini` 文件或 `icon.*` 图片才能被识别

### Q: 启用模组后游戏中未生效？

A: 请检查：
1. 模组是否已正确启用（蓝色描边）
2. 游戏是否需要重启才能加载模组
3. 模组目录是否符合游戏的模组加载规范

### Q: 热键不生效？

A: 请检查：
1. 是否已在设置中配置热键
2. 目标游戏进程是否正在运行（默认仅在游戏运行时响应热键）
3. 是否有其他应用占用了相同的热键

### Q: 缩放功能如何使用？

A: 在设置页面调整"整体缩放"滑块，界面会实时缩放。缩放范围为 0.5x ~ 2.0x。

### Q: 如何在文件管理器中打开模组目录？

A: 右键点击模组，选择"在文件管理器中打开"。支持 Windows（explorer）和 Linux（xdg-open）。

### Q: 更新模组数据是什么意思？

A: 更新模组数据会将散落在 Mods 目录下的模组文件夹整理到 `_MANAGED_` 目录中，便于管理和查找。

### Q: 什么是 # 目录互斥模式？

A: 在以 `#` 开头的目录下，同一时间只能启用一个模组。启用某模组时，同目录下的其他模组会自动禁用。适用于互斥的模组配置（如不同的纹理包）。

### Q: 如何修改导航栏宽度？

A: 鼠标悬停在导航栏右侧边缘，当光标变为双向箭头时，拖动即可调整宽度（范围：160px ~ 480px）。

### Q: 如何使用手机风格拖动滚动？

A: 在左侧导航栏或右侧模组展示区域按住鼠标左键拖动，即可上下滚动内容。这在触摸屏设备或小窗口场景下非常方便。

## 许可协议

仅供学习与个人使用。
