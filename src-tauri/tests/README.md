# 测试目录说明

## 目录结构
- `arch/`：架构健身函数门禁（arch_gate.rs，入库）
- `compare/`：NRMM 基线对比测试（deep_compare_test/dataset_parity/mod_logic_edge 及辅助 .py，入库）
- `datasets/`：测试数据集（git 忽略，不入库）
- `runtime/`：update_mod_data 测试运行时产物（git 忽略，可清理）

## 数据集准备
`deep_compare_test` 与 `dataset_parity` 依赖 NRMM-test（预期基线）与
NRMM-Rust-test（输入）两个数据集目录，请从数据源拷贝至 `src-tauri/tests/datasets/`。
缺失时对应测试会 panic 并打印本说明。