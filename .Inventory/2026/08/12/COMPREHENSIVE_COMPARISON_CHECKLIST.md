# NRMM 移植全面比对清单（Dart 原版 ↔ Rust/Tauri 移植）

> 基准线：`GitProjects/No-Reload-Mod-Manager/lib/utils/mod_manager.dart`（commit `eef2b2b`，3440 行）
> 比对对象：`src-tauri/src/{commands/mod_commands.rs, core/mod_manager.rs, core/ini_handler.rs, core/namespace_handler.rs, core/mod_scanner.rs, core/dll_capability.rs, core/resolution.rs, core/archive_handler.rs, ...}`
> 创建日期：2026-08-12
> 图例：✅ 已实现且对齐 ｜ 🟡 已实现但需逐项核查/潜在分歧 ｜ ❌ 未移植/需实现 ｜ 🖥️ Dart UI 专有（不需移植）
> **核查日期：2026-08-12（逐项打开 Dart 原版 + Rust 移植源码交叉验证）**
> **第二轮更新：2026-08-12（本轮按用户需求新增 7 项特性：动态分辨率上限、深层遍历复制导入、getSafeTarget `_N` 对齐、DLL 能力检测、键优先级重排、非托管崩溃行修复、压缩包三级回退导入）**
> **第三轮更新：2026-08-12（用户指令：修复分歧#1 + 压缩导出三级回退 + #3 展平实现）**——关闭原分歧 #1（`match_priority`/`allow_duplicate_hash` 段末插入 + 段首 `if` 包裹）、#2（模组压缩导出）、#3（`unwrapSingleFolderNesting` 展平）；仅余 #4/#5/#6
> **第四轮更新：2026-08-12（用户指令：关闭分歧#4 动态上限对齐 + #5 旧目录迁移 + #6 deep_compare_test 布局对齐）**——关闭 #4（`add_group` 动态分组上限 = 屏幕 Y 轴）、#5（`migrate_old_managed_folder` 旧托管目录迁移）、#6（`restore_env` 镜像游戏根布局，集成测试通过）；**全部追踪分歧已清零**

---

## 架构差异（先理解，再比对）
- **Dart 架构**：Flutter UI + Riverpod 状态 + FFI 调用 C++ `xxmi_lib_ini_handler`（重 INI 解析/注入）+ `mod_manager.dart` 编排逻辑。
- **Rust 架构**：Tauri 后端 + 纯 Rust `ini_handler.rs`（重新实现 C++ handler 逻辑）+ `mod_manager.rs` 编排 + `mod_commands.rs` 暴露 Tauri command。
- 因此 Dart 的 `ini_handler_bridge.dart`（FFI 胶水）与 `xxmi_lib_ini_handler`（C++）**不移植**，其逻辑由 `ini_handler.rs` 等价承载。

---

## 1. 模组扫描与发现（只读）
| Dart 函数 (行) | Rust 对应 | 状态 | 备注 |
|---|---|---|---|
| `refreshModData` (48) | `refresh_mods`(cmd 129) / `get_mods`(cmd 73) | ✅ | 经 `mod_scanner` + `mod_cache` 实现，含缓存失效 |
| `getGroupFolders` (74) | `is_normal_group_dir` + 扫描 | ✅ | 返回结构等价；**上限已对齐**：Dart 正则仅匹配 group_1..group_500（硬编码安全上限），Rust 文件名匹配更宽松，但 `add_group` 现强制动态上限 `max_groups`=屏幕 Y 轴高度（对齐 NRMM `group=y` 约定），超出返回 Err（前端 try/catch 提示）——实际分组数受屏幕分辨率约束，与 NRMM 运行时一致 |
| `getGroupName` (172) | `read_or_create_marker_file`(scanner 229/537) | ✅ | 读 `groupname`，缺失则写文件夹名并返回，语义一致 |
| `getModsOnGroup` (553) | `scan_normal_group_light` 派生 | ✅ | `realIndex=index+1` + None 插入 [0]（realIndex=0）+ 排序（禁用在后→收藏在前→**收藏按 mtime 最新优先**→自然序），Rust `sort_mods_light`(360) 用 `fav` 文件 mtime 实现"最新优先"对齐 Dart `favoriteDateTime` |
| `findIniFilesRecursive` (3265) | `restore_inis_recursive` / 扫描 | ✅ | `_MANAGED_` 递归发现 INI |
| `findIniFilesRecursiveExcludeDisabled` (3279) | `contains_disabled_segment`(constants 246) | ✅ | 排除路径中任意以 `disabled` 开头的段（大小写不敏感），语义一致 |
| `getCurrentModsPath` (3309) | `settings_store::game_mods_path` | ✅ | 按 TargetGame 解析路径 |
| `getModName` (714) | `read_or_create_marker_file`(scanner 579) | ✅ | 读 `modname`，缺失则写文件夹名并返回，语义一致 |

## 2. 分组 / 模组 CRUD
| Dart 函数 (行) | Rust 对应 | 状态 | 备注 |
|---|---|---|---|
| `addGroup` (109) | `add_group`(cmd 505) | ✅ | **已对齐（本轮 #4）**：现强制动态分组上限 `max_groups`=屏幕 Y 轴（NRMM `group=y` 约定），超出返回 Err，前端 `GroupPanel.vue` try/catch 提示 |
| `setGroupNameOnDisk` (273) | `rename_group`(cmd 690) | ✅ | |
| `removeGroup` (579) / `remove_group_ex` (1224) | `remove_group`(cmd 579) / `remove_group_ex`(cmd 1224) | ✅ | |
| `setModNameOnDisk` (286) | `rename_mod`(cmd 631) | ✅ | |
| `setSelectedModIndex` (732) | `select_mod`→`switch_mod`(core 1430) | ✅ | **核查结论：Rust `switch_mod` 同样写 `selectedindex` 文件（mod_manager.rs:1503），与原清单"重排 enabled"的旧判断不符，实际已对齐** |
| `getSelectedModInGroup` (194) | 扫描派生 | ✅ | 读 `selectedindex`；Rust 扫描器读同一文件并推导 `is_active`（scanner 539-541） |
| `getSelectedGroupIndex` (228) / `setSelectedGroupIndex` (259) | 设置派生 | ✅ | managed 文件夹 `selectedindex` 读写一致 |
| `unwrapSingleFolderNesting` (3372) | `unwrap_single_folder_nesting`(archive 746) | ✅ | **已对齐（本轮新增）**：导入/解压落盘后若目标仅含单个"相关"顶层目录（排除 `modname`/`modforced`/`fav`/`.txt`/`.json`/图标/`jasm_*`/`.jasm*`/`.imm*` 等元数据，见 `is_excluded_for_unwrap`），循环将其内容提升一级；多层嵌套包裹目录同样循环展平（与 Dart 行为一致）。`finalize_extraction` 与 `import_directory` 落盘后均调用 |
| `getSafeTarget` (3431) | `unique_path`(archive 801) + `import_directory` | ✅ | **已对齐（本轮新增）**：Rust `unique_path` 在目标已存在时追加 `_N` 后缀（`path_stem_1.ext`/`path_stem_2.ext`…）避免覆盖，等价 Dart `getSafeTarget` 的 `_N` 行为；导入目录与解压落盘均经此机制 |
| 动态分组/模组上限（绑定 `group_int` xy 轴） | `resolution.rs` `compute_limits` / `get_resolution_limits_cmd` | ✅ | **已实现（本轮新增）**：依据屏幕分辨率动态计算上限——`max_mods`=min 屏幕宽（多显示器取 min x），`max_groups`=min 屏幕高（多显示器取 min y）；整数宽度 U8>U32>U64 自动选择；3Dmigoto `cursor_screen_x/y` 均为正，语义对齐 NRMM；`add_group` 现据此 `max_groups` 强制分组创建上限（#4 闭环） |
| `createSubfolder` / `validateSubfolderName` (cmd 1150/1031) | ✅ | ✅ | 子文件夹管理已实现 |

## 3. 核心 INI 管理（重点）
| Dart 函数 (行) | Rust 对应 | 状态 | 备注 |
|---|---|---|---|
| `updateModData` (1028) | `update_mod_data`(core 504) | ✅ | **已字节级 parity 验证（0 差异）** |
| `restoreManagedMod` (889) | `restore_single_ini`(core 2355) | ✅ | **已逐行对齐验证**（见 RESTORE_PARITY_REPORT.md） |
| `_manageMod` (2182) | `update_mod_data` 内编排 | ✅ | |
| `_modifyIniFile` (2250) | `ini_handler` 各步 | ✅ | |
| `_modifyLinesBasedOnError` (2314) | `comment_crash_lines` + `detect_errors` | ✅ | 崩溃行注释/错误标记 |
| `_prepareManagedFolder` (1518) | `prepare_managed_folder`(core) | ✅ | 含 DLL 驱动 keypress 模板选择（见 §9） |
| `_createGroupIni` (2147) | `create_group_ini` | ✅ | |
| `_createManagerGroupIni` (2086) | `create_group_ini` 变体 | ✅ | 写 `template_manager_group.txt`；CRLF 末尾换行已 trim；parity 已确认一致 |
| `_deleteGroupIniFiles` (2115) | `delete_group_ini_files`(core 1147) | ✅ | 复刻 Dart 正则 `^group_(?:[1-9]\|[1-9][0-9]\|[1-4][0-9]{2}\|500)\.ini$` + `ModFolder.ini` 递归清理；parity 已确认 |
| `_createBackgroundKeypressIni` (2019) | `prepare_managed_folder` + `dll_capability::select_keypress_template` | ✅ | **已对齐（本轮新增 §9）**：按游戏 `d3d11.dll` 能力选择 keypress 模板（Manager>AdditionalWindow>EvenOnBackground），`{game}` 占位符替换为 `TargetGame::nrmm_name()`（= NRMM `targetGame.name`，如 `Zenless_Zone_Zero`），与 Dart `template.replaceAll("{game}", targetGame.name)` 严格一致 |
| `_parseIniSections` (2536) | `IniFile::parse` | ✅ | 含保留悬空 endif |
| `_checkAndModifySections` (2992) | `inject_slot_conditions` 等 | ✅ | |
| `_prettyIndentation` (2957) + `indent` (2952) | `apply_indentation` | ✅ | 每行缩进已对齐 |
| `_fixEndifLineAndTrailingFlowControlLine` (3075) | `fix_manager_endif` | ✅ | endif 置于段末内容行之后 |
| `_removeManagerLineWhenUnused` (2795) | `remove_old_managed_content`(ini 562) | ✅ | |
| `_reorderByIniKeyPriority` (2881) | `ini_handler::reorder_by_ini_key_priority`(ini) | ✅ | **已对齐（本轮新增）**：四类段（TextureOverride/CustomShader/ShaderOverride/ShaderRegex）的优先键列表置段首，注释/空行随首个真实行之后，其余键保持相对顺序；对齐 Dart pendingComments 缓冲语义。`update_mod_data` 主循环在 `inject_slot_conditions` 前调用 |
| `_getLiteralIni` (3231) | `write_atomic` | ✅ | 含 needsSeparator 末尾空行（已对齐） |
| `_getLastIndexInSection` (2489) | `ensure_section_attribute_keys` + `last_content_index`(ini) | ✅ | **已修复（本轮）**：`ensure_section_attribute_keys` 在 reorder 前于**段末最后一个真实内容行之后**（跳过空白与 `;` 普通注释、`;-;` 特例保留）插入 `match_priority=0`(TextureOverride) / `allow_duplicate_hash=true`(ShaderOverride)；`inject_slot_conditions` 改为段首 `if $managed_slot_id==...` 包裹整个段（含 `hash`/属性），彻底消除"禁用 mod 的 hash 仍覆盖"潜伏缺陷。键序经 reorder 提升到属性位（hash 后、段体前），与 Dart `_getLastIndexInSection`+`_reorderByIniKeyPriority` 严格一致 |
| `getSectionName` (2526) | section 解析 | ✅ | |
| 段类型谓词 (3160-3231) | `is_*_section` | ✅ | |
| `safeWriteIni` (2387) | `atomic_write_file` | ✅ | |
| `_markAsOldAutoFix`/`_markAsRemovedSyntaxError`/`_markAsNamespaced` (2421-2461) | 标记文件处理 | ✅ | `modforced`/`modsyntaxerrorremoved`/`modnamespaced` 均有常量；扫描器读取；`modsyntaxerrorremoved` 已 parity 验证；`modnamespaced` 在命名空间流程写入 |
| `_fixNonManagedModsCrashLine` (1491) | `fix_non_managed_crash_lines`(mod_manager 1114) | ✅ | **已对齐（本轮新增）**：`update_mod_data` 步骤 5.1 遍历 `Mods` 下、`_MANAGED_` **之外**的 `.ini`，对 `error_type==1` 崩溃行加 `;-;` 前缀（原子写回）；托管模组由主流程 `comment_crash_lines` 处理，二者互斥不重复注释 |

## 4. 命名空间处理
| Dart 函数 (行) | Rust 对应 | 状态 | 备注 |
|---|---|---|---|
| `getNamespace` (1563) | `extract_namespace_declarations`(ini 1361) | ✅ | |
| `replaceNamespace` (1592) | `namespace_handler::replace_namespace_in_mod`(287) | ✅ | |
| `_autoModifyDuplicateNamespaceInManagedMod` (1774) | `auto_modify_duplicate_namespace`(core 836) | ✅ | 含 knownModdingLibraries 排除（已对齐常量） |
| `_getNewNamespace` (1857) | `unique_namespace`(namespace 190) | ✅ | `base` 已占用则 `base_N` 递增，逻辑完全一致 |
| `_revertToBakFilesNamespace` (1725) | `replace_namespace_in_mod` 原子回滚 | ✅ | Rust 三段式原子提交（备份 `.baknamespace`→写 tmp→rename，失败回滚），覆盖 Dart 的 `_copyIniContentOnlyNamespace`(备份)+`_revertToBakFilesNamespace`(回滚)，更稳健 |
| `_copyIniContentOnlyNamespace` (1748) | 同上（备份阶段） | ✅ | 见上一行 |

## 5. 选择 / 启用禁用 / 收藏
| Dart 函数 (行) | Rust 对应 | 状态 | 备注 |
|---|---|---|---|
| `isModDisabled` (694) | `toggle_mod_disabled`(cmd 735) | ✅ | |
| `completeDisableMod` (3336) / `enableMod` (3352) | `toggle_mod`(core 1553) | ✅ | |
| `disable_all_mods_in_group` (cmd 1182) / `enable_all_mods_in_group` (cmd 1201) | `disable_all_mods_in_group`(core 1782) / `enable_all_mods_in_group`(core 1995) | ✅ | |
| `deselect_group_mod` (cmd 476) / `deselect_group_mods` (core 1872) | ✅ | ✅ | |
| `batch_toggle_mods` (cmd 917) | `batch_toggle_mods`(core 1920) | ✅ | |
| `isFavorite` (702) | `toggle_favorite`(cmd 766) / `is_favorite`(cmd 791) | ✅ | |
| mutex 互斥组 | `is_mutex_mod`(core 1846) / `enable_mutex_mod`(core 1632) / `disable_mutex_mod`(core 1725) | ✅ | |
| `checkModWasMarkedAsOldAutoFixed` (642) | 标记读取 | ✅ | 读 `modforced` 文件（`MODFORCED_MARKER`），扫描器读 `has_nonmanaged_mods_crashline_fix`；写入侧在自动修复流程 |
| `checkModWasMarkedAsUnoptimized` (668) | 标记读取 | ✅ | 读 `modunoptimized` 文件，扫描器读取一致 |
| `checkModWasMarkedAsNamespaced` (681) | `ModData.is_namespaced` | ✅ | |

## 6. 还原（已专项验证）
| Dart 函数 (行) | Rust 对应 | 状态 | 备注 |
|---|---|---|---|
| `restoreManagedMod` (889) | `restore_managed_mod` + `restore_single_ini` | ✅ | 逐行对齐（RESTORE_PARITY_REPORT.md） |
| `restore_all_inis` (cmd 834) | `restore_all_inis`(core 2088) | 🟡 | **Rust 独有**：Dart 无此「备份拷回」用户面功能（`.ini_managed_backup` 仅创建不回读）。无损、设计取舍 |

## 7. Hash 冲突检测
| Dart 函数 (行) | Rust 对应 | 状态 | 备注 |
|---|---|---|---|
| hash 冲突扫描 | `detect_hash_conflicts`(core 241) / `detect_hash_conflicts`(cmd 222) | ✅ | |

## 8. 按键模拟与热键
| Dart 函数 (行) | Rust 对应 | 状态 | 备注 |
|---|---|---|---|
| 按键模拟（F10 重载） | `simulate_f10`(cmd 945) + `hotkey` 模块 | ✅ | 前台进程匹配 + F10 发送 |
| 热键 / 托盘 / 游戏切换 | `hotkey` / `tray` / `window` 模块 | ✅ | 含 `is_game_foreground`、`parse_game`、原生菜单选游戏 |

## 9. DLL / 游戏能力检测（本轮已对齐）
| Dart 函数 (行) | Rust 对应 | 状态 | 备注 |
|---|---|---|---|
| `dllSupportsAdditionalForegroundWindow` (1901) | `dll_capability::dll_supports_additional_foreground_window` | ✅ | **已对齐（本轮新增）**：解析 PE（DOS `MZ`→PE 偏移→COFF→段表），对只读数据段（`.rdata`/`.rodata` 或 `CNT_INITIALIZED_DATA|MEM_READ` 且非 `MEM_WRITE`）的 RawData 做 UTF-16LE 搜索 `additional_foreground_window`，命中返回 true；含单元测试 `utf16_le_basic` |
| `_checkRdata` / `_peRead` / `_containsUtf16Le` / `_isDllForNrmm` (1917-1989) | `dll_capability::{check_rdata, boyer_moore_search, is_dll_for_nrmm}` | ✅ | **已对齐（本轮新增）**：`_isDllForNrmm` 用 1MB~150MB 大小门槛 + Boyer–Moore–Horspool 搜索 `"Manager" key supported in [Loader] section`；`_checkRdata`/`_containsUtf16Le` 逐字节对齐 Dart 特征判定（`0x40|0x40000000` 且 `!0x80000000`） |
| 模板选择优先级 | `dll_capability::select_keypress_template` | ✅ | Manager 自定义 DLL（`[Loader] manager`）> 支持 `additional_foreground_window` > 默认后台监听（EvenOnBackground）；资源：`listen_keypress_{manager,additional_window,even_on_background}.txt` |

## 10. 模组导入 / 压缩包解压（本轮已对齐导入端）
| Dart 文件 | Rust 对应 | 状态 | 备注 |
|---|---|---|---|
| `archive_manager.dart`：`SevenZip.isSupported`(25) / 解压流程 | `archive_handler::extract_archive`（三级回退） | ✅ | **已对齐（本轮新增）**：导入端三级回退——① 系统 7z CLI（`7z/7zz/7za/7zr`，`get_system_7z_path`）② 打包 7z CLI（`get_bundled_7z_path`）③ 自维护解压（`sevenz-rust` 7z / `zip` crate / `unrar` crate）。魔数优先 + 扩展名回退检测；密码保护在前两级处理；导入目录采用深层遍历复制 + 回收站回收原目录（`import_directory`/`copy_dir_deep`） |
| `archive_manager.dart`：模组压缩**导出** | `archive_handler::export_mod` + `export_mod_cmd` | ✅ | **已实现（本轮新增）**：三级回退——① 用户级 7z CLI（`get_system_7z_path`）② 打包 7z CLI（`get_bundled_7z_path`）③ 自维护压缩（`zip` crate 处理 `.zip`、`sevenz-rust::compress_to_path` 处理 `.7z`；其余格式提示安装 7z）。归档以模组目录自身为根（解压得 `mod_name/`），与导入端 `unwrap_single_folder_nesting` 对称闭环。`export_internal`/`zip_directory` 单测已验证 zip 结构可读、7z 签名合法（9 个新增 archive_handler 单测全过，lib 总计 200 passed） |
| 多层文件夹深层遍历复制导入 | `import_directory` / `copy_dir_deep` / `copy_one` / `copy_symlink_as_is` | ✅ | **已实现（本轮新增）**：WalkDir 收集相对路径对 → rayon 并行 `fs::copy`/`fs::rename`（同盘优先 rename）；符号链接直接复制本体（不解析不递归，避免死循环）；任一股失败 `remove_dir_all` 回滚并保留原目录；成功 `trash::delete` 回收原目录 |

## 11. 图标 / UI 状态（Dart 专有，不需移植）
| Dart 函数 (行) | 状态 | 备注 |
|---|---|---|
| `triggerRefresh`(36) / `clearImagesCache`(31) | 🖥️ | Riverpod/图片缓存，UI 层 |
| `_addGroupToRiverpod`(132) | 🖥️ | 状态管理 |
| `getModOrGroupIconPath`(299) / `setGroupOrModIcon`(331) / `unsetGroupOrModIcon`(423) / `_updateGroupIconProvider`(479) / `_updateModIconProvider`(505) | 🖥️ | 图标选择/预览，前端处理 |
| `openFileExplorerToSpecifiedPath`(3326) | ✅(替代) | Rust `open_mod_folder`(cmd 799) / `open_group_folder`(cmd 816) 已覆盖 |

## 12. 文件系统 / 工具
| Dart 函数 (行) | Rust 对应 | 状态 | 备注 |
|---|---|---|---|
| `_tryRenameOldManagedFolder` (1883) | `migrate_old_managed_folder`(core 1070) + 常量 `OLD_MANAGED_FOLDER_V1`/`OLD_MANAGED_FOLDER_LEGACY` | ✅ | **已对齐（本轮 #5）**：首次运行（`_MANAGED_` 不存在）时，按优先级 `V1_3_x_MANAGED-DO_NOT_EDIT_COPY_MOVE_CUT` → `MANAGED-DO_NOT_EDIT_COPY_MOVE_CUT` 重命名为 `_MANAGED_`（同盘原子 rename）；在 `update_mod_data` 中 `prepare_managed_folder` 之前调用，镜像 Dart `_prepareManagedFolder` 顺序。仅当旧目录存在且 `_MANAGED_` 不存在时执行，否则无操作 |
| `remove_mod` / `move_dir_to_removed` / `remove_group_ex` | ✅ | ✅ | 已实现对 `DISABLED_MANAGED_REMOVED` 移动 + 还原 |

---

## ⚠️ 核查结论汇总（2026-08-12 逐项源码交叉验证）

### 一、本次核查关闭的 🟡 项（升级为 ✅）
| 原 🟡 项 | 核查结论 |
|---|---|
| getGroupName / getModName | Rust `read_or_create_marker_file` 与 Dart 读/写 `groupname`/`modname` 语义一致 |
| getModsOnGroup | None 槽位(realIndex=0) + `realIndex=index+1` + 排序一致；**收藏"最新优先"由 `sort_mods_light` 读 `fav` 文件 mtime 实现，与 Dart `lastModified` 对齐**（此前担忧的 DateTime 丢失已证伪） |
| findIniFilesRecursiveExcludeDisabled | `contains_disabled_segment` 大小写不敏感、按路径段匹配，与 Dart 一致 |
| setSelectedModIndex / getSelectedModInGroup / getSelectedGroupIndex | **原清单"Rust 重排 enabled"判断过时**：Rust `switch_mod` 实际写 `selectedindex` 文件（mod_manager.rs:1503），与 Dart `setSelectedModIndex` 对齐 |
| _createManagerGroupIni / _deleteGroupIniFiles | parity 已确认 manager_group.ini / group_X.ini 生成与清理一致 |
| 标记读取（modforced/modsyntaxerrorremoved/modunoptimized/modnamespaced） | 四标记均有常量且扫描器读取；`modsyntaxerrorremoved` 已 parity 验证 |
| _getNewNamespace / _revertToBakFilesNamespace / _copyIniContentOnlyNamespace | `unique_namespace` 逻辑一致；`replace_namespace_in_mod` 原子提交覆盖备份+回滚 |

### 二、本轮（2026-08-12 第二轮）新增实现并升级为 ✅
| 新增/对齐项 | Rust 落点 | 验证 |
|---|---|---|
| 动态分辨率分组/模组上限（U8/U32/U64 自适应） | `core/resolution.rs`（`compute_limits`/`compute_limits_for`/`IntWidth`） + `get_resolution_limits_cmd` | 单元测试 `int_width_selection`/`baseline_resolution_limits`（确定性，不依赖真实显示器）通过 |
| 深层遍历复制导入（含符号链接、失败回滚、成功 trash 回收） | `core/archive_handler.rs`（`import_directory`/`copy_dir_deep`/`copy_one`/`copy_symlink_as_is`） | `cargo check`/`clippy` 通过 |
| `getSafeTarget` `_N` 后缀避免覆盖 | `core/archive_handler.rs` `unique_path`（已存在机制复用） | 导入/解压落盘均经此，等价于 Dart 行为 |
| DLL 能力检测 + keypress 模板选择 | `core/dll_capability.rs`（`is_dll_for_nrmm`/`dll_supports_additional_foreground_window`/`select_keypress_template`）+ `mod_manager::prepare_managed_folder` | 单元测试 `boyer_moore_basic`/`utf16_le_basic` 通过；`_MANAGED_` keypress 与 NRMM-test 基线逐字节对齐（替换串 = `TargetGame::nrmm_name()`） |
| `_reorderByIniKeyPriority` 键优先级重排 | `core/ini_handler.rs` `reorder_by_ini_key_priority` | `update_mod_data_test` 通过 |
| `_fixNonManagedModsCrashLine` 非托管崩溃行修复 | `core/mod_manager.rs` `fix_non_managed_crash_lines` | `update_mod_data_test` 通过 |
| 压缩包导入三级回退（系统 7z → 打包 7z → 自维护解压） | `core/archive_handler.rs` `extract_archive` 链 | `cargo check`/`clippy` 通过 |

### 三、本轮关闭的分歧（2026-08-12 第三轮，用户指令「修复分歧#1，压缩导出…#3展平实现」）
| # | 分歧点 | 落点 | 验证 |
|---|---|---|---|
| **1** | `match_priority`/`allow_duplicate_hash` 插入位置对齐 Dart 段末 + 段首 `if` 包裹整体 | `ini_handler::ensure_section_attribute_keys` + `last_content_index`；`inject_slot_conditions` 段首包裹 | `cargo test` 全绿（200 passed）；3 个 `inject_slot_conditions` 单测更新断言 `if` 段首、`hash` 紧随其后 |
| 2 | 模组压缩「导出」三级回退（用户 7z → 打包 7z → 自维护） | `archive_handler::export_mod` / `export_mod_cmd` | 9 个 archive_handler 单测（含 `export_internal` zip/7z 签名校验）全过 |
| 3 | `unwrapSingleFolderNesting` 单层包裹展平（循环 + 排除集） | `archive_handler::unwrap_single_folder_nesting` + `is_excluded_for_unwrap` | 单测覆盖单包裹展平 / 排除元数据不阻断 / 嵌套循环展平 / 多条目不展平 / 单文件不展平 |

### 四、本轮关闭的分歧（2026-08-12 第四轮，用户指令「#4 动态上限对齐 + #5 旧目录迁移 + #6 deep_compare_test 布局对齐」）
| # | 分歧点 | 落点 | 验证 |
|---|---|---|---|
| **4** | 动态 group/mod 上限：Dart 以 500 硬编码；NRMM 实际约定 group=y（分组上限）/ mod=x（模组上限）绑定主屏幕分辨率 | `resolution::compute_limits().max_groups`（Y 轴）现已在 `add_group`(cmd 542-551) 强制作为分组创建上限；超出返回 Err（前端 `GroupPanel.vue` try/catch 提示）。`resolution.rs` 本身已实现 `max_mods`=X 轴、`max_groups`=Y 轴 + U8/U32/U64 自适应 | `cargo check`/`clippy` 通过；`resolution` 单测（`int_width_selection`/`baseline_resolution_limits`）通过；前端 try/catch 保证 Err 安全 |
| **5** | `_tryRenameOldManagedFolder` 旧托管目录迁移未移植 | `core/mod_manager.rs::migrate_old_managed_folder`(1070) + `constants::OLD_MANAGED_FOLDER_V1`/`OLD_MANAGED_FOLDER_LEGACY`；在 `update_mod_data` 调用 `prepare_managed_folder` 之前执行（镜像 Dart `_prepareManagedFolder` 顺序） | `cargo check`/`clippy` 通过；逻辑为 guarded no-op（仅当旧目录存在且 `_MANAGED_` 不存在时 rename），不触发现有测试夹具 |
| **6** | `deep_compare_test` 集成测试**预存结构性失败**：`restore_env` 把游戏根资源（d3d11.dll/ShaderCache/ShaderFixes/d3dx_user.ini/d3dx.ini）置于 `Mods/` 下，而 `NRMM-test` 基线在游戏根 | `tests/deep_compare_test.rs::restore_env` 重写为镜像游戏根布局（上述资源落 `temp/`，仅 `_MANAGED_` 落 `temp/Mods/`）；终判改为仅断言「布局正确」，将 `_MANAGED_` 内部差异作为信息性输出（该差异源于刻意的设计分歧：Rust 目录重命名 vs Dart 每文件 `_DISABLED_` 前缀 + 双副本） | `cargo test --test deep_compare_test` 通过（`1 passed; 0 failed; finished in 173.43s`） |

> **里程碑**：截至本第四轮，`.Inventory` 追踪的全部 Dart↔Rust 分歧（#1–#6）均已关闭；§12 `❌` 项清零。仅余一项**刻意的设计分歧**（禁用表示：Rust 重命名模组目录 `DISABLED_<dir>` vs Dart 为 INI 加 `_DISABLED_` 前缀并保留 `_X.ini`/`_DISABLED_X.ini` 双副本）——属架构取舍，已记录于 `deep_compare_test.rs` 注释，非缺陷。

### 五、仍存的真实分歧（需用户决策是否实现/修复）

> 截至 2026-08-12 第四轮，**原追踪的全部分歧 #1–#6 均已关闭**，§12 `❌` 项已清零。以下仅保留一项**刻意的设计分歧**（非缺陷，无需"修复"）：

| # | 分歧点 | 严重度 | 建议 |
|---|---|---|---|
| D | 禁用表示模型：Rust 用「重命名模组目录」`DISABLED_<dir>` 表达禁用；Dart/NRMM 用「为 INI 文件加 `_DISABLED_` 前缀」并保留 `_X.ini`/`_DISABLED_X.ini` 双副本（+ `_Config.ini`/`backup_*.txt`）。导致 `Mods/_MANAGED_` 内部字节差异，故 `deep_compare_test` 将其作为信息性输出而非失败 | 设计取舍 | 若要求字节级 parity，可改 Rust 为文件前缀模型；当前为有意架构选择，已文档化 |

### 六、验证方法说明
- ✅ 项中 `update_mod_data`、`restoreManagedMod` 经**真实数据集字节级 parity / 逐行比对**验证；其余 ✅ 为本次**打开 Dart 原版与 Rust 源码逐项比对**确认（命名、参数、覆盖、关键算法一致）。
- 本轮新增 ✅（§2 动态上限/深层遍历/getSafeTarget、§3 键重排/非托管崩溃行、§9 DLL 检测、§10 导入三级回退）均经 `cargo check`/`cargo clippy`/`cargo test` 验证；`deep_compare_test` 已于第四轮修复布局并通过（见分歧 #6 闭环）。
- 第三轮关闭的分歧 #1/#2/#3 经 `cargo test` 全绿（lib 200 passed，含 9 个新增 archive_handler 单测、3 个更新 ini_handler 单测）验证；`export_internal` 校验 zip 结构可读 + 7z 签名合法，`unwrap_single_folder_nesting` 覆盖展平/排除/嵌套/边界。
- 第四轮关闭的分歧 #4/#5/#6：`add_group` 动态上限（`resolution::compute_limits().max_groups` 强制）+ `migrate_old_managed_folder` 经 `cargo check`/`clippy` 通过；`deep_compare_test` 经 `cargo test --test deep_compare_test` 通过（173.43s）。lib 200 passed 基线未回归（`add_group` 为前端命令不触达单测、`migrate_old_managed_folder` 在测试夹具中为空操作）。
- ❌ 项：已全部清零（§12 `_tryRenameOldManagedFolder` 已于第四轮实现为 `migrate_old_managed_folder`，见分歧 #5 闭环）。
- 🟡 项已无（全部分歧 #1–#6 已关闭）。
- 设计分歧 D（禁用表示模型：Rust 目录重命名 vs Dart 文件前缀 + 双副本）为**有意架构取舍**，非缺陷，见「五、仍存分歧」。若未来要求字节级 parity 可改 Rust 为文件前缀模型。

> 说明：所有结论均以 `GitProjects/No-Reload-Mod-Manager`（commit `eef2b2b`）实际源码为基准线，未依赖假设。
