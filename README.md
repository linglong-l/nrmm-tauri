# XXMI-NRMM
## [基于no reload mod manager项目](https://github.com/Aglglg/No-Reload-Mod-Manager)
> XXMI 模组管理器（NR Mod Manager）的桌面客户端 v0.1.0

一款基于 Tauri 2 + Vue 3 的轻量级模组管理工具，为游戏玩家提供模组的浏览、启用/禁用、分组管理、收藏等核心功能。

## 平台支持

- ✅ **Windows 11**（已测试）
- ⚠️ **Windows 10**：未测试
- ❌ **macOS / Linux**：不支持（后端依赖 Win32 API，依赖 `windows` crate，未考虑其他平台，等XXMI适配我也适配【怎么可能，我都不一定能稳定跟新一年嘞】）

## 技术栈

### 前端

- Vue 3（`<script setup>` SFC）
- TypeScript 6.x
- Vite 6
- Pinia 3（状态管理）
- Vue Router 4
- Element Plus 2（UI 组件）
- vue-i18n 9（多语言）

### 后端

- Rust 2021 edition
- Tauri 2（含 `tray-icon` feature）
- tokio 1.52（异步运行时）
- Tauri 插件：global-shortcut、dialog、fs、opener、shell、store
- `windows` crate 0.62（Win32 API：进程检测、键盘模拟、窗口控制）

## 环境要求

- **操作系统**：Windows 11（必需）
- **Node.js**：≥ 18
- **Rust**：≥ 1.96（stable toolchain，MSVC 目标）
- **MSVC Build Tools**：安装 "Desktop development with C++" 工作负载
- **WebView2 Runtime**：Windows 11 自带，无需额外安装
- **Tauri CLI**：通过 `npm install` 自动安装（`@tauri-apps/cli`）

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
npm run tauri build
```

产物位于 `src-tauri/target/release/bundle/`，支持 MSI、NSIS 等 Windows 安装包格式。

## 多语言

界面支持以下语言（`src/assets/translations/`）：

- 简体中文（zh-CN）
- 繁体中文（zh-TW）
- English（en）
- Русский（ru）
- Bahasa Indonesia（id）

切换语言后需重启应用才能完全生效。

## 项目结构

```
xxmi-nrmm/
├── src/                    # Vue 前端
│   ├── components/         # 通用组件（TitleBar、SideNav、StatusBar、GameSelector）
│   ├── pages/              # 页面
│   │   └── index/
│   │       ├── index.vue   #   - 主页面
│   │       └── tabs/       #   - 标签页
│   │           └── ModsTab.vue  # 模组管理标签页
│   ├── views/              # 视图（SettingsView.vue）
│   ├── stores/             # Pinia 状态（game、settings、hotkey、ui、cloudData）
│   ├── composables/        # 组合式函数
│   ├── utils/              # 工具（invoke 封装、events、constants）
│   ├── types/              # TypeScript 类型定义
│   └── assets/             # 静态资源（图标、字体、翻译文件）
│
├── src-tauri/              # Rust 后端
│   ├── src/
│   │   ├── cloud_data/         # 云数据同步
│   │   ├── commands/           # Tauri Command 注册
│   │   ├── file_watcher/       # 文件监听（notify crate）
│   │   ├── hotkey/             # 全局热键
│   │   ├── ini_handler/        # INI 文件解析
│   │   ├── init_xx/            # XXMI 初始化 & 日志
│   │   ├── keypress_simulator/ # 按键模拟
│   │   ├── mod_manager/        # 模组管理核心
│   │   ├── process/            # 进程检测
│   │   ├── settings/           # 设置持久化
│   │   ├── task_queue/         # 任务队列（保证全局单一性）
│   │   ├── tray/               # 系统托盘
│   │   ├── window_manager/     # 窗口状态管理
│   │   ├── lib.rs
│   │   └── main.rs
│   ├── capabilities/           # Tauri 权限配置
│   └── tauri.conf.json
│
├── index.html
├── package.json
├── tsconfig.json
├── vite.config.ts
└── .gitignore
```

## 功能特性

- 模组与分组的浏览、启用/禁用、收藏、重命名、删除
- 支持 Grid 与 Carousel 两种布局模式，可实时切换
- 文件系统监听（500ms 防抖）
- 全局热键切换窗口（支持键盘与手柄）
- 设置导入/导出/重置
- 多语言界面
- 圆角无边框窗口 + 磨砂黑背景
- 系统托盘菜单
- 目标进程检测（仅在指定游戏运行时响应热键）
- 模组路径有效性实时校验
- 收藏筛选（按分组或模组）

## 许可

仅供学习与个人使用。
