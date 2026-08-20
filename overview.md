# NRMM-Tauri 四项优先级修复 — 完成总结

按用户优先级顺序修复的四个问题，本次会话完成的是**收尾集成 + 验证**，并通过全部质量门禁。

## 1. OS 级 advisory lock（进程级互斥与一致性）✅
- 核心实现 `src-tauri/src/core/file_lock.rs`（`AdvisoryLock`）：原子创建隐藏锁文件 `.nrmm_process.lock`（`OpenOptions::create_new`）+ `Drop` 释放 + stale-PID / 6h 超时检测崩溃恢复；Windows 用 `windows` crate `OpenProcess`/`GetExitCodeProcess` 探测 PID 存活。
- **本次集成**：
  - `mod_manager::update_mod_data` 开头加 `AdvisoryLock::acquire_blocking(game_mods_path, 30s)` + `OperationGuard("update_mod_data")`。
  - `archive_handler::import_item` / `import_mod_auto` 加 `OperationGuard("import_mod")`（与既有 advisory lock 并列）。
  - 备份侧锁放在 `ini_backup_scheduler::worker`：best-effort `acquire_blocking` 2s，锁根按任务类型推导（Full=传入根；Incremental=祖父目录 = Mods 根）。**未**放进 `backup_ini_files` 本身——避免测试临时目录产生 stray 锁文件、且避免同进程并发备份互相串行化/跳过。
- 同一 Mods 根共享一把锁 → 更新/导入/备份跨进程互斥。

## 2. INI 流式读取（≤20MB 直读，>20MB 流式）✅
- `ini_handler.rs`：常量 `MAX_DIRECT_READ_BYTES = 20*1024*1024`；`read_file_bytes` 先 `metadata` 取长度，≤ 阈值走 `read_to_end` 快速路径，否则按 20MB 分块 `read` 拼接（带大文件 debug 日志）。（上一会话已完成，本次确认编译通过）

## 3. 模组导入流程（.tmp 临时区 + 同盘原子移动 + 重命名）✅
- 解压到 `<Mods>/.tmp`（隐藏 dotdir，扫描器 `starts_with('.')` 自动跳过，避免中断污染分组目录）。
- `finalize_extraction`：`unique_mod_dir_name` 处理同名（`{modname} (N)`）→ `move_temp_to_target` 同盘优先 `rename`（原子/高效），否则 `copy_dir_deep`+删除 → `unwrap_single_folder_nesting` → 清理 `.tmp`。
- 导入入口（`import_item`/`import_mod_auto`）已加 advisory lock（30s 阻塞）。（上一会话已完成，本次补充 OperationGuard）

## 4. 程序退出记录（四种触发方式）✅
- `src-tauri/src/core/exit_log.rs`：`ExitReason::{UserQuit, UserQuitCancelled, SystemSignal, Forced}`，落盘 `<data_local>/nrmm-rust/exit_log.jsonl` + `run_state.json`（跨启动推断强制终止）。
- **本次集成**：
  - `window::hard_quit_app`（用户退出按钮）：退出前查 `operation_registry::has_active_operations()`；**有活跃后台操作 → 取消退出**并 `record_exit(UserQuitCancelled, 含活跃操作与已运行秒数)`；否则 `mark_clean_exit(UserQuit)` 后 `app.exit(0)`。
  - `lib.rs::run()` 早期调 `mark_app_started()`（下次启动据 `last_exit_clean==false` 补记 `Forced`）。
  - 系统信号（Ctrl+C 等）：Windows 平台用既有 `windows` crate `SetConsoleCtrlHandler` 注册控制台控制信号处理器，记录 `SystemSignal` 并退出（非 Windows 为空调用，未引入新依赖）。

## 验证结果
| 门禁 | 结果 |
|------|------|
| `cargo clippy --all-targets --all-features -- -D warnings` | ✅ 通过（重试一次越过 `.fingerprint` 瞬态 `os error 5`） |
| `cargo fmt --check` | ✅ 通过 |
| `cargo machete` | ✅ 无未使用依赖 |

附带清理：删除冗余死函数 `move_directory_contents`（已被 `move_temp_to_target` 取代）。Windows API 修正：`STILL_ACTIVE` 按 `u32=259` 比较、`BOOL` 走 `windows::core::BOOL`、handler 参数类型为 `u32`。前端未改动。

## 说明 / 取舍
- 备份锁采用「best-effort 短超时」而非强制串行：避免同进程并发备份互相阻塞/跳过，同时仍与跨进程更新/导入正确互斥。
- 系统信号退出记录目前仅在 Windows 控制台场景生效（无新依赖）；Unix 由 Tauri 运行循环处理，未单独挂钩。
