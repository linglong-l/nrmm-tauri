/**
 * 组内激活态派生（前端镜像，与后端 `derive_group_active_state` 单一真源口径一致）
 *
 * 规范源：src-tauri/src/core/mod_scanner.rs（扫描与缓存定向 patch 共用的派生逻辑），
 * 供前端乐观更新时同步计算「取消选择/切换选中后」的组内 is_active 展示态，
 * 保证 UI 先行状态与后端缓存 patch / 全量重扫结果完全一致。
 *
 * 判定口径（与后端逐字对齐）：
 * - 恰好一个启用（非 None 且 !disabled）→ 该模组为 active；
 * - 零个启用 → None 槽位为 active（返回 0）；
 * - 多个启用 → 回退到 selectedindex 值定位；指向禁用模组时 fallback 到第一个启用模组。
 *
 * 注意：`disabled` 为兼容字段（后端轻量扫描恒 false，模组禁用前缀见 `modDisabled`），
 * 此处沿用后端口径仅读 `disabled`，勿改成 `modDisabled`（口径分叉会造成 UI↔缓存不一致）。
 */

/** 派生所需的最小模组字段（结构兼容 ModData；name 与 ModData 一致为可选） */
export interface ActiveStateMod {
  name?: string | undefined
  disabled: boolean
  isActive: boolean
}

/**
 * 基于选中索引就地派生组内 `isActive`，返回派生后的 `activeModIndex`。
 *
 * @param mods 组内模组列表（含 None 空槽位，None 恒在下标 0；就地写入 `isActive`）
 * @param selectedIndex selectedindex 标记文件当前值（None 槽位 = 0）
 * @returns 派生后的 `activeModIndex`（无激活条目为 -1）
 */
export function deriveGroupActiveState<M extends ActiveStateMod>(
  mods: M[],
  selectedIndex: number,
): number {
  const enabledNonNone: number[] = []
  for (let i = 0; i < mods.length; i++) {
    if (mods[i].name !== 'None' && !mods[i].disabled) {
      enabledNonNone.push(i)
    }
  }

  if (enabledNonNone.length === 1) {
    const idx = enabledNonNone[0]
    mods[idx].isActive = true
    // None 槽位设置为不激活
    const none = mods.find((m) => m.name === 'None')
    if (none) none.isActive = false
    return idx
  }
  if (enabledNonNone.length === 0) {
    // 没有启用的模组 → None 激活
    for (const m of mods) {
      m.isActive = m.name === 'None'
    }
    return 0
  }
  // 多个模组启用（异常），回退到 selectedindex 文件
  let activeIdx = -1
  for (let idx = 0; idx < mods.length; idx++) {
    const m = mods[idx]
    if (m.name === 'None') {
      m.isActive = selectedIndex === 0
      if (selectedIndex === 0) activeIdx = 0
    } else {
      m.isActive = idx === selectedIndex
      if (idx === selectedIndex) activeIdx = idx
    }
  }
  // 如果 selectedindex 指向的模组是禁用的，则 fallback 到第一个启用的
  if (activeIdx < 0 || mods[activeIdx]?.disabled) {
    const firstEnabled = enabledNonNone[0]
    if (firstEnabled !== undefined) {
      for (const m of mods) {
        m.isActive = false
      }
      mods[firstEnabled].isActive = true
      activeIdx = firstEnabled
    }
  }
  return activeIdx
}
