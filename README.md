# XXMI-NRMM

## 基于 [No Reload Mod Manager](https://github.com/Aglglg/No-Reload-Mod-Manager)

> XXMI 模组管理器（NR Mod Manager）的桌面客户端 v0.5.0

一款基于 Tauri 2 + Vue 3 的轻量级模组管理工具，为游戏玩家提供模组的浏览、启用/禁用、分组管理、收藏、压缩包导入与 INI 备份等核心功能。支持 Windows 与 Linux 平台。

> **注意：工具链版本限制（2026-09 起生效）**
>
> 构建与运行本项目要求 **Node.js ≥ 22.22.2**、**Rust ≥ 1.93**（MSRV）。
> 下限由现有依赖直接钉定：
> - Node.js：前端依赖（`abbrev`/`nopt` 等）最低支持本机 `^22.22.2`，低于此版本 `npm install` 会报 `EBADENGINE` 警告；
> - Rust：`sevenz-rust2` 0.22 要求 Rust 1.93（`tauri` 系 1.77.2、`notify-rust` 1.89 均更低）。
>
> 仓库已相应设置 `package.json engines >=22.22.2`、`Cargo.toml rust-version = "1.93"`，CI 使用 Node 22 与 stable 工具链。详见[环境要求](#环境要求)。

* 游戏内按键模拟（NRMM 交互协议）目前仅在 Windows 完整支持；Linux 走 xdotool/ydotool/wtype/wlrctl；macOS 明确不支持，相关实现与依赖已移除。

## 平台支持

| 平台 | 状态 | 说明 |
|------|------|------|
| Windows 11 | ✅ 已测试 | 完全支持 |
| Linux | ✅ 已测试 | 通过条件编译支持，使用 xdg-open 打开文件管理器 |
| Windows 10 | ⚠️ 未测试 | 可能兼容，但未进行测试 |
| macOS | ❌ 不支持 | 明确不支持；平台实现与专属依赖已移除，不产出 macOS 构建 |

## 支持的游戏

| 游戏 | 枚举值 | 进程名 |
|------|--------|--------|
| 原神 | `GenshinImpact` | `GenshinImpact.exe` / `YuanShen.exe` |
| 崩坏：星穹铁道 | `HonkaiStarRail` | `StarRail.exe` |
| 鸣潮 | `Wuwa` | `Wuthering Waves.exe` / `WutheringWaves.exe` |
| 绝区零 | `ZZZ` | `ZenlessZoneZero.exe` |
| 崩坏3 | `HonkaiImpact3rd` | `BH3.exe` |
| 明日方舟：终末地 | `ArknightsEndfield` | — |

## 技术栈

### 前端

| 技术 | 版本 | 用途 |
|------|------|------|
| Vue | ^3 | 前端框架，使用 `<script setup>` SFC |
| TypeScript | ~6.0 | 类型安全的 JavaScript 超集 |
| Vite | ^8.2 | 构建工具与开发服务器 |
| Pinia | ^4 | 状态管理 |
| Element Plus | ^2 | UI 组件库 |
| vue-i18n | ^11 | 多语言国际化 |

### 后端

| 技术 | 版本 | 用途 |
|------|------|------|
| Rust | 1.93+ | 系统级编程语言（MSRV，见「环境要求」） |
| Tauri | ~2.11.5 | 桌面应用框架（tray-icon / rustls-tls / protocol-asset） |
| tokio | ^1 | 异步运行时 |
| rayon | 1.12 | 数据并行（扫描/解析多核加速） |
| notify | 8.2 | 文件系统监听（200ms 防抖） |
| serde / serde_json | ^1 | 序列化 |
| anyhow | 1 | 错误处理 |
| zip / sevenz-rust / unrar / infer | 8.6 / 0.6 / 0.5 / 0.16 | 压缩包解压与类型推断 |
| trash | 5 | 回收站安全删除 |
| image | 0.25 | 缩略图生成 |
| reqwest | 0.13 | 云端数据 / 更新检查 |
| parking_lot | 0.12 | 高效锁（RwLock/Mutex） |
| `windows` crate | 0.62 | Win32 API（进程检测、键盘模拟、窗口控制） |
| Tauri 插件 | shell / dialog / notification / updater / window-state / os / global-shortcut / opener | 系统能力集成 |

### 核心模块

- **core/mod_manager**：模组扫描、启用/禁用、互斥/分组管理核心逻辑（`mod_scanner` / `mod_cache` / `incremental_updater` / `namespace_handler`）
- **core/ini_handler**：INI 文件解析与重写（含 `error_normalizer` 错误归一化、`d3dxini_cache` / `mod_ini_cache` 结果缓存）
- **core/archive_handler**：压缩包（7z/zip/rar）解压与目录导入（`.tmp` 临时区 + 同盘原子移动）
- **core/ini_backup** / **ini_backup_scheduler**：INI 备份执行体与调度（白名单 + 幂等）
- **core/file_lock** + **operation_registry**：OS 级 advisory lock 与后台操作注册表（跨进程互斥、退出保护）
- **core/exit_log**：程序退出原因记录（正常/取消/信号/强杀）
- **core/file_watcher**：文件系统监听（notify + 200ms 防抖）
- **core/resolution** / **core/dll_capability**：分辨率解析与 DLL 能力检测
- **core/thumbnail**：模组预览图缩略图生成（WebView 内存优化）
- **commands**：Tauri IPC 命令注册与处理（`mod_commands` / `settings_commands` / `thumbnail_commands`）
- **config**：应用路径与设置持久化（`app_paths` / `settings_store`）
- **platform**：跨平台按键模拟与前台检测（windows / linux / macos 占位）
- **window** / **tray** / **hotkey** / **updater** / **resources** / **cloud_data**：窗口、托盘、热键、更新、资源、云端数据

## 环境要求

### 通用要求

- **Node.js**：≥ 22.22.2（仓库 `.nvmrc` = 22 线，`package.json` 的 `engines` 要求 `>=22.22.2`；下限由前端依赖最低支持版本钉定，低于此版本 npm 会报 `EBADENGINE`）
- **Rust**：≥ 1.93（MSRV 由后端依赖中最高 `rust_version` 钉定——`sevenz-rust2` 0.22 要求 1.93，`notify-rust` 1.89、`tauri` 系 1.77.2 均更低；本项目与 CI 均使用 stable toolchain，满足即可）
- **Tauri CLI**：通过 `npm install` 自动安装（`@tauri-apps/cli`）

### Windows 特定要求

- **MSVC Build Tools**：安装 "Desktop development with C++" 工作负载
- **WebView2 Runtime**：Windows 11 自带，无需额外安装

### Linux 特定要求

- **libwebkit2gtk-4.1-dev**：WebView 组件依赖（Tauri 2 要求 4.1，非 4.0）
- **libayatana-appindicator3-dev**：系统托盘依赖
- **librsvg2-dev**：图标渲染依赖
- **xdg-utils**：文件管理器打开功能
- **最低运行环境**：glibc >= 2.35（Ubuntu 22.04 及以上）；低于此版本的发行版无法运行。
  Linux 构建需在 **Ubuntu 22.04** 环境（WSL2 或 `ubuntu:22.04` 容器）完成，宿主 glibc 即兼容下限。

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

产物统一命名规范：`nrmm-tauri_<版本>_x64-<系统>.<ext>`，**仅支持 x86_64**（不兼容 x86_32 / x32 / aarch64，不做交叉编译）。

| 平台 | 产物 | 包名 | 安装作用域 | 说明 |
|------|------|------|-----------|------|
| Windows | NSIS 安装包 | `nrmm-tauri_<ver>_x64-Win.exe` | 用户级（currentUser） | 双击安装，不写系统目录 |
| Windows | 免安装包 | `nrmm-tauri_<ver>_x64-Win.zip` | 用户级 | 解压即用，内置 7z |
| Linux | deb | `nrmm-tauri_<ver>_x64-Linux.deb` | 系统（需 sudo） | 兜底，避免用户环境异常 |
| Linux | rpm | `nrmm-tauri_<ver>_x64-Linux.rpm` | 系统（需 sudo） | 兜底，避免用户环境异常 |
| Linux | 免安装包 | `nrmm-tauri_<ver>_x64-Linux.tar.gz` | 用户级 | **推荐**，解压即用，内置 7z |

> 推荐使用 **免安装包**（用户级、零安装、可放任意目录）。
> deb/rpm 需系统级安装（sudo），仅作为用户环境异常时的兜底。
>
> 所有免安装包**均内置 7z CLI**，不存在「无 7z」变体。

### 关于内置 7-Zip

所有分发产物（Windows 免安装 zip / NSIS 安装包 / Linux tar.gz 免安装包 / deb / rpm）**均内置 7-Zip CLI**，
落在程序同级的 `7z/` 目录下（Windows：`7za.exe` + `7za.dll` + `7zxa.dll`；Linux：`7zz`）。

内置 7z 是「完整归档格式支持」的组成部分，程序启动时自动探测该目录。
**请勿移除或改动 `7z/` 目录下的任何文件**：一旦缺失，解压回退链将降级为仅支持 `zip / 7z / rar` 三种格式，
并优先调用系统已安装的 7-Zip；若系统也未安装 7-Zip，则进一步回退到内置纯 Rust 解压（支持格式更少、性能较弱）。
由此产生的任何异常由使用者自行承担。

### 构建命令

> **Windows 构建依赖 PowerShell 7+（`pwsh`）**：`scripts/build-windows.ps1` 含中文提示，
> 而 Windows PowerShell 5.1 对**无 BOM** 的 UTF-8 脚本按系统 ANSI 代码页解码（中文变乱码 →
> 引号配对错乱 → 解析失败）。`chcp 65001` **不能**解决：脚本读取编码由 .NET 默认编码
> （系统区域）决定，与控制台代码页无关。请勿改用 `powershell` 调用。

```bash
# Windows（NSIS 安装包 + 免安装 zip，均内置 7z）：
npm run build:windows

# Linux（在 Ubuntu 22.04 环境，宿主 glibc 即兼容下限；CI 用 ubuntu:22.04 容器）：
npm run build:linux
```

> ⚠️ 不要写成 `npm run tauri build -- --target <triple>`：npm 会把 `--target` 当成本身配置吞掉，
> 只把 triple 当位置参数传给 tauri，最终 cargo 报 `unexpected argument`。
> 需要手动调用时统一用 `npx tauri build --target <triple>` 直传。

产物位于工作区根 `target/<triple>/release/bundle/`（Tauri 安装包；旧布局回退 `src-tauri/target/<triple>/release/bundle/`），
免安装包与 updater manifest 位于 `dist-win/`、`dist-linux/`。

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

- ✅ 全局热键切换窗口（键盘全平台；手柄组合仅 Windows 轮询 XInput，Linux 侧为占位待实现）
- ✅ 系统托盘菜单
- ✅ 目标进程检测（仅在指定游戏运行时响应热键）
- ✅ 文件系统监听（200ms 防抖）
- ✅ 在文件管理器中打开模组目录（支持 Windows 和 Linux）

### 多语言支持

- ✅ 界面（WebView 内全部文案）：简体中文、繁体中文、English、Русский、Bahasa Indonesia
- ⚠ 系统托盘菜单目前仅中文/英文：托盘文案在 Rust 侧另有一张 `TRAY_ZH`/`TRAY_EN` 表，
  其它语言会回落英文（尚未并入界面语言包）

### 数据与归档

- ✅ 压缩包导入（7z / zip / rar，`.tmp` 临时区 + 同盘原子移动 + 重命名）
- ✅ INI 备份（白名单 + 幂等，删除走回收站）
- ✅ 增量更新模组数据（路径收敛 + 局部扫描 + 子树替换）
- ✅ 进程级互斥锁（更新 / 导入 / 备份跨进程串行）
- ✅ 退出原因记录（正常 / 取消 / 信号 / 强杀）
- ✅ INI 流式读取（≤20MB 直读，更大分块读取，防内存峰值）

## 项目结构

```
xxmi-nrmm/
├── src/                          # Vue 3 + TypeScript 前端
│   ├── App.vue                   # 应用根组件
│   ├── main.ts                   # 应用入口
│   ├── types/
│   │   └── index.ts              # 全局类型定义
│   ├── stores/                   # Pinia 状态管理
│   │   ├── index.ts
│   │   ├── mods.ts               # 模组数据 Store
│   │   ├── settings.ts           # 设置 Store
│   │   └── platform.ts           # 平台 Store
│   ├── composables/              # 组合式函数
│   │   ├── useDragScroll.ts
│   │   ├── useImageLazyLoad.ts
│   │   ├── useLoadingDots.ts
│   │   ├── useModContextMenu.ts
│   │   ├── useThumbnailUrl.ts
│   │   └── useVirtualGrid.ts     # 虚拟滚动网格
│   ├── components/
│   │   ├── HashConflictOverlay.vue
│   │   ├── UpdateModDataOverlay.vue
│   │   ├── common/               # 通用组件（标题栏/搜索/对话框/窗口控制等）
│   │   ├── mod/                  # 模组组件（卡片/网格/分组树/删除对话框）
│   │   └── nav/                  # 导航组件（PillTabs）
│   ├── views/                    # 页面视图
│   │   ├── ModsView.vue          # 模组管理主页面
│   │   ├── SettingsView.vue      # 设置页面
│   │   └── KeybindsView.vue      # 快捷键页面
│   ├── utils/                    # 工具函数
│   │   ├── constants.ts
│   │   ├── env.ts
│   │   ├── html.ts
│   │   ├── i18n/                 # 国际化（i18n 加载）
│   │   ├── logger.ts
│   │   ├── refreshGuard.ts
│   │   └── tauri.ts              # Tauri 后端调用封装
│   └── assets/                   # 静态资源（字体/图标/模板）
│
├── src-tauri/                    # Rust 后端
│   ├── ARCHITECTURE.md           # 项目架构文档
│   ├── Cargo.toml / Cargo.lock   # Rust 项目配置与依赖锁定
│   ├── build.rs                  # 构建脚本
│   ├── tauri.conf.json           # Tauri 配置
│   ├── capabilities/             # Tauri 权限配置
│   ├── icons/                    # 应用图标（多尺寸）
│   ├── tests/                    # 集成测试（dataset_parity / deep_compare）
│   └── src/
│       ├── lib.rs                # 库入口：插件初始化、Tauri 命令注册
│       ├── main.rs               # 二进制入口
│       ├── debug.rs              # 调试辅助
│       ├── commands/             # Tauri IPC 命令
│       │   ├── mod_commands.rs       # 模组命令（扫描/切换/导入/收藏/更新）
│       │   ├── settings_commands.rs  # 设置命令
│       │   └── thumbnail_commands.rs # 缩略图命令
│       ├── config/               # 路径与设置持久化
│       │   ├── app_paths.rs
│       │   └── settings_store.rs
│       ├── core/                 # 核心业务逻辑
│       │   ├── mod_manager.rs / mod_scanner.rs / mod_cache.rs
│       │   ├── incremental_updater.rs / namespace_handler.rs
│       │   ├── ini_handler.rs / error_normalizer.rs
│       │   ├── d3dxini_cache.rs / mod_ini_cache.rs
│       │   ├── archive_handler.rs        # 压缩包解压与导入
│       │   ├── ini_backup.rs / ini_backup_scheduler.rs
│       │   ├── file_lock.rs / operation_registry.rs
│       │   ├── exit_log.rs / file_logger.rs / file_watcher.rs
│       │   ├── resolution.rs / dll_capability.rs / thumbnail.rs
│       │   ├── cloud_data.rs / constants.rs
│       │   └── mod.rs
│       ├── models/               # 数据模型
│       │   ├── enums.rs          # TargetGame 等枚举
│       │   ├── mod_data.rs
│       │   └── settings.rs
│       ├── platform/             # 跨平台（windows / linux / macos 占位）
│       ├── window/               # 窗口管理
│       ├── tray/                 # 系统托盘
│       ├── hotkey/               # 全局热键
│       ├── updater/              # 应用更新
│       ├── resources/            # 嵌入资源
│       └── utils/                # 工具函数
│
├── public/                       # 静态资源（Vite 公共目录）
├── index.html                    # HTML 入口文件
├── package.json                  # 前端依赖配置
├── tsconfig.json                 # TypeScript 配置
├── tsconfig.node.json            # TypeScript Node 配置
├── vite.config.ts                # Vite 配置
├── .gitignore                    # Git 忽略配置
├── LICENSE                       # 双授权说明（指向以下两份许可证）
├── LICENSE-MIT                   # MIT 许可证全文
├── LICENSE-APACHE                # Apache-2.0 许可证全文
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
> ⚖️ 本项目采用 **MIT / Apache-2.0 双授权**（Dual License），使用者可任选其一。

- [MIT License](./LICENSE-MIT)
- [Apache License, Version 2.0](./LICENSE-APACHE)
- 双授权说明见 [LICENSE](./LICENSE)

* 本项目为基于 [No-Reload-Mod-Manager](https://github.com/Aglglg/No-Reload-Mod-Manager) 的衍生作品，上游版权归属原作者，本仓库代码以双授权发布。
* 本软件按"原样"提供，不提供任何明示或暗示的保证。作者不对因使用本软件而产生的任何索赔、损害或其他责任承担责任。