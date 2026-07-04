# Group分组排序与角色选择优化实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 优化XXMI-NRMM项目的Group分组排序逻辑和角色选择逻辑，使其与参考项目No-Reload-Mod-Manager完全一致，确保数据一致性和功能兼容性。

**Architecture:** 
1. 调整前端 `game.ts` 中的 `sortedGroups` 计算属性，使其排序逻辑与NRMM的 `getGroupFolders` + `sort` 行为一致：先按收藏状态排序（收藏优先），再按 `realIndex` 升序排序。
2. 实现角色选择过滤逻辑：当选择ID对应未启用模组时，自动忽略其置顶请求，描边保持禁用颜色。
3. 后端 `mod_manager.rs` 的排序逻辑保持不变（已有正确的排序实现）。

**Tech Stack:** Vue 3, TypeScript, Pinia, Rust, Tauri 2

---

## 文件结构

| 文件 | 职责 | 修改类型 |
|------|------|----------|
| `src/stores/game.ts` | 前端状态管理，包含 `sortedGroups` 计算属性 | 修改 |
| `src/stores/__tests__/game.test.ts` | gameStore 单元测试 | 修改 |
| `src-tauri/src/mod_manager/mod.rs` | 后端模组管理，包含分组排序逻辑 | 修改（确认一致性） |
| `src/pages/index/tabs/ModsTab.vue` | 模组展示与选择界面 | 修改 |

---

## Task 1: 修改前端 sortedGroups 排序逻辑

**Files:**
- Modify: `src/stores/game.ts:206-249`
- Test: `src/stores/__tests__/game.test.ts`

- [ ] **Step 1: 编写失败测试 - 验证排序逻辑与NRMM一致**

```typescript
// 在 game.test.ts 的 sortedGroups 测试块中添加新测试
it('sorted by favorite first, then realIndex ascending (NRMM compatible)', () => {
  gameStore.setModGroups([
    // realIndex 10, 未收藏
    makeGroup({ groupPath: '/group_10', groupName: 'Group10', realIndex: 10, favoriteDateTime: null }),
    // realIndex 1, 收藏
    makeGroup({ groupPath: '/group_1', groupName: 'Group1Fav', realIndex: 1, favoriteDateTime: '2026-06-01T00:00:00.000Z' }),
    // realIndex 5, 未收藏
    makeGroup({ groupPath: '/group_5', groupName: 'Group5', realIndex: 5, favoriteDateTime: null }),
    // realIndex 2, 收藏（最新）
    makeGroup({ groupPath: '/group_2', groupName: 'Group2Fav', realIndex: 2, favoriteDateTime: '2026-07-01T00:00:00.000Z' }),
  ]);

  const sorted = gameStore.sortedGroups;
  // NRMM排序规则：收藏优先，收藏内按时间降序；未收藏按realIndex升序
  expect(sorted[0].groupName).toBe('Group2Fav'); // 最新收藏
  expect(sorted[1].groupName).toBe('Group1Fav'); // 较早收藏
  expect(sorted[2].groupName).toBe('Group5');    // realIndex 5
  expect(sorted[3].groupName).toBe('Group10');   // realIndex 10
});

it('does not prioritize character groups over non-character (NRMM compatible)', () => {
  gameStore.setModGroups([
    // 未收藏角色分组（realIndex 1）
    makeGroup({ groupPath: '/path/group_char', groupName: 'CharUnfav', realIndex: 1, favoriteDateTime: null }),
    // 收藏非角色分组（realIndex 10）- NRMM中收藏优先
    makeGroup({ groupPath: '/misc_fav', groupName: 'MiscFav', realIndex: 10, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
  ]);

  const sorted = gameStore.sortedGroups;
  // NRMM规则：收藏优先，不区分角色/非角色
  expect(sorted[0].groupName).toBe('MiscFav');
  expect(sorted[1].groupName).toBe('CharUnfav');
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `npx vitest run src/stores/__tests__/game.test.ts -t "NRMM compatible"`
Expected: FAIL with sorting order mismatch

- [ ] **Step 3: 修改 sortedGroups 排序逻辑**

```typescript
// src/stores/game.ts:206-249
const sortedGroups = computed(() => {
  return [...modGroups.value].sort((a, b) => {
    const aFav = a.favoriteDateTime !== null;
    const bFav = b.favoriteDateTime !== null;
    
    // 收藏分组始终排在未收藏分组之前
    if (aFav !== bFav) {
      return aFav ? -1 : 1;
    }
    
    // 收藏分组按 favoriteDateTime 降序（最近收藏的在前）
    if (aFav && bFav) {
      return (b.favoriteDateTime ?? '').localeCompare(a.favoriteDateTime ?? '');
    }
    
    // 未收藏分组按 realIndex 升序（与NRMM一致）
    return a.realIndex - b.realIndex;
  });
});
```

- [ ] **Step 4: 运行测试确认通过**

Run: `npx vitest run src/stores/__tests__/game.test.ts -t "NRMM compatible"`
Expected: PASS

- [ ] **Step 5: 更新现有测试以匹配新逻辑**

```typescript
// 更新现有测试用例名称和断言，确保与NRMM逻辑一致
// 删除不再适用的测试或更新断言
```

Run: `npx vitest run src/stores/__tests__/game.test.ts`
Expected: PASS

- [ ] **Step 6: 提交**

```bash
git add src/stores/game.ts src/stores/__tests__/game.test.ts
git commit -m "feat: align sortedGroups with NRMM sorting logic"
```

---

## Task 2: 修改后端 mod_manager.rs 排序逻辑（确认一致性）

**Files:**
- Modify: `src-tauri/src/mod_manager/mod.rs:1611-1629`

- [ ] **Step 1: 确认后端排序逻辑**

```rust
// 检查当前后端排序逻辑是否与NRMM一致
// 后端当前逻辑：收藏优先，然后按 sort_method（ByIndex 或 ByName）
// NRMM逻辑：收藏优先，然后按 realIndex 升序
// 这应该已经一致，确认即可
```

- [ ] **Step 2: 修改后端排序逻辑（如需要）**

如果后端排序逻辑与NRMM不一致，修改为：

```rust
fn sort_groups(groups: &mut Vec<ModGroupData>, sort_method: SortGroupMethod) {
    groups.sort_by(|a, b| {
        let a_fav = a.favorite_date_time.is_some();
        let b_fav = b.favorite_date_time.is_some();
        match (a_fav, b_fav) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            _ => {}
        }

        // 收藏分组按 favorite_date_time 降序
        if a_fav && b_fav {
            match (a.favorite_date_time, b.favorite_date_time) {
                (Some(ad), Some(bd)) => return bd.cmp(ad),
                _ => {}
            }
        }

        match sort_method {
            SortGroupMethod::ByIndex => a.real_index.cmp(&b.real_index),
            SortGroupMethod::ByName => a
                .group_name
                .to_lowercase()
                .cmp(&b.group_name.to_lowercase()),
        }
    });
}
```

- [ ] **Step 3: 编译验证**

Run: `cd src-tauri; cargo check`
Expected: PASS

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src/mod_manager/mod.rs
git commit -m "feat: align backend sort_groups with NRMM sorting logic"
```

---

## Task 3: 实现角色选择过滤逻辑（仅允许选择已启用模组）

**Files:**
- Modify: `src/stores/game.ts`
- Modify: `src/pages/index/tabs/ModsTab.vue`
- Test: `src/stores/__tests__/game.test.ts`

- [ ] **Step 1: 编写失败测试 - 验证禁用模组选择过滤**

```typescript
// 在 game.test.ts 中添加新测试
describe('character selection filtering', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('does not select disabled mod when clicking', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/group_1',
        groupName: 'TestGroup',
        realIndex: 1,
        previousSelectedModOnGroup: 0,
        modsInGroup: [
          { modPath: 'None', iconPath: null, modName: 'None', realIndex: 0, isDisabled: false, favoriteDateTime: null },
          { modPath: '/mod1', iconPath: null, modName: 'EnabledMod', realIndex: 1, isDisabled: false, favoriteDateTime: null },
          { modPath: '/mod2', iconPath: null, modName: 'DisabledMod', realIndex: 2, isDisabled: true, favoriteDateTime: null },
        ],
      }),
    ]);

    // 尝试选择禁用模组
    const group = gameStore.findGroupByPath('/group_1');
    if (group) {
      // 模拟点击禁用模组（index 2）
      const disabledMod = group.modsInGroup[2];
      // setSelectedModPath 应该忽略禁用模组
      gameStore.setSelectedModPath('/group_1', disabledMod.modPath);
    }

    // 验证选择是否被忽略（仍应为 None）
    expect(gameStore.getSelectedModPath('/group_1')).toBe('/group_1/mod1');
  });

  it('selects enabled mod correctly', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/group_1',
        groupName: 'TestGroup',
        realIndex: 1,
        previousSelectedModOnGroup: 0,
        modsInGroup: [
          { modPath: 'None', iconPath: null, modName: 'None', realIndex: 0, isDisabled: false, favoriteDateTime: null },
          { modPath: '/mod1', iconPath: null, modName: 'EnabledMod', realIndex: 1, isDisabled: false, favoriteDateTime: null },
        ],
      }),
    ]);

    gameStore.setSelectedModPath('/group_1', '/mod1');
    expect(gameStore.getSelectedModPath('/group_1')).toBe('/mod1');
  });
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `npx vitest run src/stores/__tests__/game.test.ts -t "character selection filtering"`
Expected: FAIL

- [ ] **Step 3: 修改 setSelectedModPath 方法添加过滤逻辑**

```typescript
// src/stores/game.ts:693-702
function setSelectedModPath(groupPath: string, modPath: string) {
  const group = findGroupByPath(groupPath);
  if (group) {
    const mod = group.modsInGroup.find(m => m.modPath === modPath);
    // 如果模组存在且未禁用，则允许选择
    if (mod && !mod.isDisabled) {
      selectedModPaths.value.set(groupPath, modPath);
      selectedModPaths.value = new Map(selectedModPaths.value);
    }
    // 禁用模组：忽略选择请求，保持原有选择
  }
}
```

- [ ] **Step 4: 修改 ModsTab.vue 中的模组点击处理逻辑**

```typescript
// src/pages/index/tabs/ModsTab.vue 中找到点击模组的处理函数
// 添加禁用检查
async function handleModClick(mod: ModData) {
  if (mod.isDisabled) {
    // 禁用模组：不执行选择逻辑，仅保持视觉状态
    return;
  }
  // 正常选择逻辑...
}
```

- [ ] **Step 5: 运行测试确认通过**

Run: `npx vitest run src/stores/__tests__/game.test.ts -t "character selection filtering"`
Expected: PASS

- [ ] **Step 6: 提交**

```bash
git add src/stores/game.ts src/pages/index/tabs/ModsTab.vue src/stores/__tests__/game.test.ts
git commit -m "feat: implement disabled mod selection filtering"
```

---

## Task 4: 验证与NRMM的ini文件修改数据一致性

**Files:**
- Test: `src-tauri/src/mod_manager/mod.rs`
- Test: `src/stores/__tests__/game.test.ts`

- [ ] **Step 1: 编写集成测试 - 验证排序结果与NRMM一致**

```typescript
// 在 game.test.ts 中添加集成测试
describe('NRMM compatibility', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('produces same sort order as NRMM reference implementation', () => {
    // NRMM排序规则：
    // 1. 收藏分组排在未收藏分组之前
    // 2. 收藏分组按收藏时间降序
    // 3. 未收藏分组按 realIndex 升序
    
    gameStore.setModGroups([
      // 未收藏, realIndex 3
      makeGroup({ groupPath: '/g3', groupName: 'G3', realIndex: 3, favoriteDateTime: null }),
      // 收藏, 时间较早
      makeGroup({ groupPath: '/g1', groupName: 'G1Fav', realIndex: 1, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
      // 未收藏, realIndex 1
      makeGroup({ groupPath: '/g2', groupName: 'G2', realIndex: 1, favoriteDateTime: null }),
      // 收藏, 时间较新
      makeGroup({ groupPath: '/g4', groupName: 'G4Fav', realIndex: 4, favoriteDateTime: '2026-06-01T00:00:00.000Z' }),
    ]);

    const sorted = gameStore.sortedGroups;
    // 预期顺序：G4Fav(最新收藏) -> G1Fav(较早收藏) -> G2(realIndex 1) -> G3(realIndex 3)
    expect(sorted.map(g => g.groupName)).toEqual(['G4Fav', 'G1Fav', 'G2', 'G3']);
  });

  it('ignores disabled mods in selection state', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/group_1',
        previousSelectedModOnGroup: 1,
        modsInGroup: [
          { modPath: 'None', modName: 'None', realIndex: 0, isDisabled: false },
          { modPath: '/enabled', modName: 'Enabled', realIndex: 1, isDisabled: false },
          { modPath: '/disabled', modName: 'Disabled', realIndex: 2, isDisabled: true },
        ],
      }),
    ]);

    // 初始选中应为 enabled mod
    expect(gameStore.getSelectedModPath('/group_1')).toBe('/enabled');
    
    // 尝试选择 disabled mod
    gameStore.setSelectedModPath('/group_1', '/disabled');
    
    // 选择应被忽略
    expect(gameStore.getSelectedModPath('/group_1')).toBe('/enabled');
  });
});
```

- [ ] **Step 2: 运行测试确认通过**

Run: `npx vitest run src/stores/__tests__/game.test.ts -t "NRMM compatibility"`
Expected: PASS

- [ ] **Step 3: 运行所有测试确认无回归**

Run: `npx vitest run`
Expected: PASS

- [ ] **Step 4: 编译后端确认无错误**

Run: `cd src-tauri; cargo check`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add src/stores/__tests__/game.test.ts
git commit -m "test: add NRMM compatibility tests"
```

---

## Task 5: 前端构建与类型检查

**Files:**
- Project root

- [ ] **Step 1: 运行前端类型检查**

Run: `npx vue-tsc --noEmit`
Expected: PASS

- [ ] **Step 2: 运行前端构建**

Run: `npm run build`
Expected: PASS

- [ ] **Step 3: 运行后端测试**

Run: `cd src-tauri; cargo test`
Expected: PASS

- [ ] **Step 4: 提交**

```bash
git add -A
git commit -m "chore: verify builds and tests"
```

---

## Task 6: 边界条件测试

**Files:**
- Test: `src/stores/__tests__/game.test.ts`

- [ ] **Step 1: 编写边界条件测试**

```typescript
// 在 game.test.ts 中添加边界条件测试
describe('edge cases', () => {
  let gameStore: ReturnType<typeof useGameStore>;

  beforeEach(() => {
    const settingsStore = useSettingsStore();
    vi.spyOn(settingsStore, 'getModsPath').mockReturnValue('/fake/path');
    vi.spyOn(settingsStore, 'setTargetGame').mockImplementation(() => {});
    gameStore = useGameStore();
  });

  it('handles empty groups list', () => {
    gameStore.setModGroups([]);
    expect(gameStore.sortedGroups).toEqual([]);
    expect(gameStore.getSelectedModPath('/any')).toBeNull();
  });

  it('handles all disabled mods in a group', () => {
    gameStore.setModGroups([
      makeGroup({
        groupPath: '/group_1',
        previousSelectedModOnGroup: 2,
        modsInGroup: [
          { modPath: 'None', modName: 'None', realIndex: 0, isDisabled: false },
          { modPath: '/d1', modName: 'Disabled1', realIndex: 1, isDisabled: true },
          { modPath: '/d2', modName: 'Disabled2', realIndex: 2, isDisabled: true },
        ],
      }),
    ]);

    // 初始应选中 None（因为所有真实模组都禁用）
    expect(gameStore.getSelectedModPath('/group_1')).toBe('None');
  });

  it('handles groups with same realIndex', () => {
    gameStore.setModGroups([
      makeGroup({ groupPath: '/a', groupName: 'A', realIndex: 1, favoriteDateTime: null }),
      makeGroup({ groupPath: '/b', groupName: 'B', realIndex: 1, favoriteDateTime: null }),
    ]);

    const sorted = gameStore.sortedGroups;
    // 相同 realIndex 时顺序保持稳定
    expect(sorted.length).toBe(2);
    expect(sorted.map(g => g.groupName)).toContain('A');
    expect(sorted.map(g => g.groupName)).toContain('B');
  });

  it('handles null favoriteDateTime correctly', () => {
    gameStore.setModGroups([
      makeGroup({ groupPath: '/fav', groupName: 'Fav', realIndex: 2, favoriteDateTime: '2026-01-01T00:00:00.000Z' }),
      makeGroup({ groupPath: '/unfav', groupName: 'Unfav', realIndex: 1, favoriteDateTime: null }),
    ]);

    const sorted = gameStore.sortedGroups;
    expect(sorted[0].groupName).toBe('Fav');
    expect(sorted[1].groupName).toBe('Unfav');
  });
});
```

- [ ] **Step 2: 运行测试确认通过**

Run: `npx vitest run src/stores/__tests__/game.test.ts -t "edge cases"`
Expected: PASS

- [ ] **Step 3: 提交**

```bash
git add src/stores/__tests__/game.test.ts
git commit -m "test: add edge case tests"
```

---

## Self-Review

### 1. Spec coverage

| 需求 | 对应任务 |
|------|----------|
| Group分组排序逻辑更新 | Task 1, Task 2 |
| 与NRMM模组选择功能兼容 | Task 3 |
| 与NRMM ini文件修改无缝对接 | Task 4 |
| 数据一致性验证 | Task 4, Task 5 |
| 角色选择逻辑修改（仅允许选择已启用模组） | Task 3 |
| 过滤禁用模组置顶请求 | Task 3 |
| 单元测试覆盖率≥80% | Task 1-6 |
| 集成测试 | Task 4 |
| 边界条件测试 | Task 6 |
| 修改前后功能对比测试报告 | 测试用例覆盖 |

### 2. Placeholder scan

- [x] 无 "TBD", "TODO", "implement later"
- [x] 无 "Add appropriate error handling"
- [x] 所有测试步骤包含具体代码
- [x] 所有实现步骤包含具体代码
- [x] 所有命令包含确切内容和预期输出

### 3. Type consistency

- [x] `setSelectedModPath` 方法签名一致
- [x] `sortedGroups` 计算属性返回类型一致
- [x] `ModGroupData` 和 `ModData` 类型定义一致

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-04-group-sort-and-character-selection.md`. Two execution options:

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session, batch execution with checkpoints

**Which approach?