//! 核心功能模块
//!
//! 本模块包含 NRMM 的核心业务逻辑：
//! - `constants`: 全局常量定义（文件名、前缀、正则等）
//! - `watcher_ring`/`watcher_incremental`/`watcher_emit`/`watcher_core`: 文件系统监控四域（事件环形缓冲/暂停守卫 / 增量根分类与批次路由 / 事件发射 / FileWatcher 主体，由 file_watcher.rs 拆分）
//! - `ini_codec`/`ini_condition`/`ini_model`/`ini_transform`/`ini_maintain`/`ini_analysis`: INI 处理六域（2026-09-18 由 ini_handler.rs 拆分）
//! - `mod_path_utils`: 模组路径工具（路径匹配、Mods 根定位、可用文件夹名）
//! - `mod_remove`: 模组删除域（remove_mod / remove_group_ex / 移至移除目录）
//! - `mod_results`: 模组操作结果/状态类型（纯数据）
//! - `mod_select`: 模组选择/selectedindex 切换与取消选中
//! - `mod_toggle`: 模组启停/互斥组/批量启停
//! - `mod_update_pipeline`: update_mod_data 全量更新编排域（mod_update_lowlevel 为并行底层）
//! - `mod_scan_util`: 模组扫描公共基础（正则/目录判定/排序/计数真源）
//! - `mod_scan_marker`: 还原安全窗口与标记文件读写（只读不建，mod_scanner 六域拆分）
//! - `mod_scan_light`: 轻量扫描域（ScanResult / scan_mods_light / scan_single_mod，mod_scanner 六域拆分）
//! - `mod_scan_mutex`: 互斥组扫描域（栈式 DFS / build_mutex_mod_light，mod_scanner 六域拆分）
//! - `mod_scan_deep`: 深度扫描域（完整解析 INI / build_mod_data_deep，mod_scanner 六域拆分）
//! - `mod_scan_partial`: 局部扫描域（scan_partial_path / normalize_subpath，mod_scanner 六域拆分）
//! - `namespace_handler`: 命名空间处理（namespace 变量展开）
//! - `archive_detect`/`archive_extract`/`archive_import`/`archive_export`: 压缩包处理四域（类型契约与探测 / 解压与编排 / 扁平化与导入 / 导出，由 archive_handler.rs 拆分）
//! - `cloud_data`: 云端数据管理（链接、消息、图标库等远程资源）
//! - `mod_cache_core`/`mod_cache_path`/`mod_cache_patch`/`mod_cache_delta`: 模组数据内存缓存四域（结构+生命周期 / 路径判断 / 子树与模组级 patch / 旧状态收集，由 mod_cache.rs 拆分）
//! - `mod_detect`: 模组环境/冲突检测（标准 XXMI、hash 冲突、orfix·texfx）
//! - `mod_ini_admin`: INI 管理编排（managed 准备、组 INI 生成、自定义保存）
//! - `mod_restore`: INI 恢复（restore_all_inis / restore_managed_mod）

pub mod archive_detect;
pub mod archive_export;
pub mod archive_extract;
pub mod archive_import;
pub mod cloud_data;
pub mod constants;
pub mod d3dxini_cache;
pub mod dll_capability;
pub mod error_normalizer;
pub mod events;
pub mod exit_log;
pub mod file_lock;
pub mod file_logger;
pub mod fs_walk;
pub mod incremental_updater;
pub mod ini_analysis;
pub mod ini_backup;
pub mod ini_backup_scheduler;
pub mod ini_codec;
pub mod ini_condition;
pub mod ini_maintain;
pub mod ini_model;
pub mod ini_transform;
pub mod key_combo;
pub mod mod_cache_core;
pub mod mod_cache_delta;
pub mod mod_cache_patch;
pub mod mod_cache_path;
pub mod mod_detect;
pub mod mod_ini_admin;
pub mod mod_ini_cache;
pub mod mod_path_utils;
pub mod mod_remove;
pub mod mod_restore;
pub mod mod_results;
pub mod mod_scan_deep;
pub mod mod_scan_light;
pub mod mod_scan_marker;
pub mod mod_scan_mutex;
pub mod mod_scan_partial;
pub mod mod_scan_util;
pub mod mod_select;
pub mod mod_toggle;
pub mod mod_update_lowlevel;
pub mod mod_update_pipeline;
pub mod namespace_handler;
pub mod nrmm_prefs;
pub mod operation_registry;
pub mod resolution;
pub mod thumbnail;
pub mod watcher_core;
pub mod watcher_emit;
pub mod watcher_incremental;
pub mod watcher_ring;
