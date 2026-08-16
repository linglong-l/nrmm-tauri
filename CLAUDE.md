# 项目 AI 协作契约 (Rust + Vue3/TS6)

## 1. 核心架构原则 (Absolute Rules)

- **Vue 状态管理**：仅使用 Pinia (Setup Store 语法)。禁止 Vuex、Mixin、EventBus。
- **Vue 组件**：强制使用 `<script setup lang="ts">`。禁止 Options API。
- **Rust 错误处理**：库代码使用 `thiserror`，应用代码使用 `anyhow`。禁止在业务逻辑中使用 `unwrap()` 或 `expect()`，必须使用 `?` 传播或显式 `match`。
- **依赖注入**：TS 业务逻辑优先使用纯函数 + 接口参数，避免单例类和隐式全局状态。

## 2. 代码风格与命名 (Strict Conventions)

- **Rust**：
  - 遵循 Rust API Guidelines。
  - 变量/函数：`snake_case`；类型/Trait：`PascalCase`；常量：`SCREAMING_SNAKE_CASE`。
  - 必须为所有 `pub` 函数/结构体编写 rustdoc 注释。
- **TypeScript/Vue**：
  - 变量/函数：`camelCase`；组件/接口/类型：`PascalCase`。
  - Vue 组件名必须是多个单词（如 `UserProfile`，禁止 `User`）。
  - Props 必须使用 `defineProps<T>()` 显式定义类型，禁止运行时声明。
  - 禁止使用 `any`，必要时使用 `unknown` 并配合类型守卫 (Type Guard)。

## 3. 文件组织 (Directory Structure)

- Rust: 按领域划分 module (如 `domain/user.rs`, `infra/db.rs`)，尽量避免单个 `lib.rs` 超过 500 行。
- Vue: `src/components` (UI), `src/composables` (逻辑), `src/stores` (状态), `src/services` (API)，`src/utils`（工具）。

## 4. 工作流要求 (Workflow)

- 在修改任何代码前，先阅读相关的现有实现，保持风格一致。
- 生成代码后，必须确保能通过 `cargo clippy` 和 `eslint` 的严格检查。
- 不要生成冗余的注释，代码应自解释。
- 提交前，确保所有单元测试通过。

## 5. 代码风格格式化工具 (Tools)

### Rust 修复

- 防止Clone滥用

  ```bash
  cargo clippy -- -W clippy::redundant_clone
  ```

- 静态分析

  ```bash
  cargo clippy --all-targets --all-features -- -D warnings -W clippy::pedantic -W clippy::nursery
  ```

- 依赖检查

  ```bash
  cargo machete
  ```

- 文档检查

  ```bash
  RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
  ```

- 最终格式化并修复所有警告

  ```bash
  cargo +nightly fmt --all
  cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
  ```

### TS/Vue 修复

- 类型检查

  TS 官方建议。Vue 项目必须用 vue-tsc 替代原生 tsc，确保 .vue 文件中的模板类型和 Props 被严格校验。

  ```bash
    vue-tsc --noEmit
  ```

- Lint & 修复

  ```bash
  eslint . --fix
  ```

- 格式化

  ```bash
  prettier --write "**/*.{js,ts,vue,css,html,json}"
  ```

- 样式检查

  ```bash
  stylelint "**/*.{css,scss,vue}" --fix
  ```

- TS: 隐式 Any / 类型断言

  ```bash
  tsc --noEmit --strict
  ```

- Vue: 模板过于复杂

  强制将模板里的复杂计算抽离到 `<script setup>` 的 computed 中。

  ```bash
  eslint-plugin-vue (vue/max-attributes-per-line, vue/no-template-shadow)
  ```

- 死代码/未使用变量

  ```bash
  eslint-plugin-unused-imports
  ```

- 最终格式化并修复所有警告

  ```bash
  pnpm eslint . --fix
  pnpm prettier --write .
  ```

### 最终类型检查 (不修复，只报错)

```bash
pnpm vue-tsc --noEmit
cargo check
```

### 提交前检查

- Rust 修复

  ```bash
  cargo +nightly fmt --all
  cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
  ```

- TS/Vue 修复

  ```bash
  pnpm eslint . --fix
  pnpm prettier --write .
  ```

- 最终类型检查 (不修复，只报错)

  ```bash
  pnpm vue-tsc --noEmit
  cargo check
  ```
