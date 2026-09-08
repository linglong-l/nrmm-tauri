/**
 * 组内模组排序（前端镜像，与后端 `mod_order_cmp` / `sort_mods_light` 口径一致）
 *
 * 规范源：src-tauri/src/core/mod_scanner.rs（轻量扫描构建期与增量合并共用的排序真源）：
 * 1. 启用优先（未禁用在前）：`!disabled && !modDisabled` 判定启用
 * 2. 启用组内收藏优先（isFavorite 前置）
 * 3. 名称自然序：`naturalCompare` 逐行镜像后端 `natural_compare`（数字段按数值比较）
 *
 * ⚠️ 已知残留分歧：后端 `mod_order_cmp` 在收藏键之后还有第 3 键「fav 标记文件 mtime 倒序
 * （最新收藏在前）」，该数据前端不可得，无法本地镜像。仅当**同组内存在多个收藏模组**
 * 时才可能分叉，最终由 watcher 权威 delta / 下次全量扫描收敛（不再是数字段那种系统性分叉）。
 *
 * None 空槽位恒为组内第 0 位（mod_index 以 None=0 为基准，不得顶替）。
 *
 * 用途：启停/收藏等零扫描本地 patch 后，无需重扫即可让组内展示序与
 * 「下次全量轻量扫描」收敛一致（消除"启用/禁用后排序不更新"的三态分叉）。
 */

/** 排序所需的最小模组字段（结构兼容 ModData） */
export interface SortableMod {
  name?: string | null
  modPath?: string
  modName?: string
  disabled?: boolean
  modDisabled?: boolean
  isFavorite?: boolean
}

/** 判定 None 空槽位（与后端 name == "None" / mod_path == "None" 口径一致） */
export function isNoneSlotMod(m: { name?: string | null; modPath?: string }): boolean {
  return m.name === 'None' || m.modPath === 'None'
}

/** 是否为启用态（镜像后端：!disabled && !modDisabled；两字段必须同源同步） */
export function isModEnabled(m: SortableMod): boolean {
  return !m.disabled && !m.modDisabled
}

/** 字符是否为 ASCII 数字（镜像后端 `char::is_ascii_digit`） */
function isAsciiDigit(cp: number): boolean {
  return cp >= 48 && cp <= 57
}

/** 统计码点数（镜像后端 `chars().count()`；JS 的 `length` 是 UTF-16 长度，非代理对安全） */
function countCodePoints(s: string): number {
  // 无代理对时 length 即码点数（常见路径零开销）
  let n = 0
  for (let i = 0; i < s.length; i++) {
    n++
    const c = s.charCodeAt(i)
    if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) i++ // 跳过低代理项
  }
  return n
}

/**
 * 自然序字符串比较（**逐行镜像**后端 `mod_scanner::natural_compare`）
 *
 * 后端实现（`src-tauri/src/core/mod_scanner.rs::natural_compare`）：
 * 1. 两侧 `to_lowercase()` 后按**码点**逐位比较；
 * 2. 双方当前位同为 ASCII 数字 → 各解析完整数字串，按**数值**比较（相等则跳过该数字段继续）；
 * 3. 否则按码点值比较（相等则同步前进）；
 * 4. 任一方耗尽 → 比较**码点总数**。
 *
 * 为何必须镜像：此前前端用 `localeCompare`，数字段按字符比较，与后端数值比较相反 ——
 * natural 序 `mod2 < mod10`，locale 序 `mod10 < mod2`。模组名带数字（如「女主-异界联动 4.3」）
 * 时两序必然分叉，导致本地重排与后端权威序不一致 → 卡片可见重排、选中错位。
 *
 * @returns <0 a 在前 / 0 相等 / >0 b 在前
 */
export function naturalCompare(a: string, b: string): number {
  const sa = a.toLowerCase()
  const sb = b.toLowerCase()
  let i = 0
  let j = 0
  while (i < sa.length && j < sb.length) {
    const ca = sa.codePointAt(i) as number
    const cb = sb.codePointAt(j) as number
    const aDigit = isAsciiDigit(ca)
    const bDigit = isAsciiDigit(cb)
    if (aDigit && bDigit) {
      // 解析完整数字段并按数值比较（后端为 u64 累加；JS 用 double，超大数不溢出更安全）
      let an = 0
      while (i < sa.length) {
        const c = sa.charCodeAt(i)
        if (!isAsciiDigit(c)) break
        an = an * 10 + (c - 48)
        i++
      }
      let bn = 0
      while (j < sb.length) {
        const c = sb.charCodeAt(j)
        if (!isAsciiDigit(c)) break
        bn = bn * 10 + (c - 48)
        j++
      }
      if (an !== bn) return an < bn ? -1 : 1
      continue // 数值相等 → 跳过该数字段继续（i/j 已前移）
    }
    if (ca !== cb) return ca < cb ? -1 : 1
    i++
    j++
  }
  // 一方耗尽：比较码点总数（与后端 `a_chars.len().cmp(&b_chars.len())` 对齐）
  const la = countCodePoints(sa)
  const lb = countCodePoints(sb)
  if (la === lb) return 0
  return la < lb ? -1 : 1
}

/**
 * 镜像后端 `mod_order_cmp` 的比较器：启用优先 → 收藏优先 → 名称自然序。
 * 搭配稳定排序使用（V8/Array.prototype.sort 稳定），保证同键相对序不漂移。
 */
export function compareModsLikeLightScan(a: SortableMod, b: SortableMod): number {
  const aEnabled = isModEnabled(a)
  const bEnabled = isModEnabled(b)
  if (aEnabled !== bEnabled) return aEnabled ? -1 : 1
  const aFav = !!a.isFavorite
  const bFav = !!b.isFavorite
  if (aFav !== bFav) return aFav ? -1 : 1
  // 名称自然序：必须用 naturalCompare（数字段按数值），不可用 localeCompare（按字符）——
  // 两者对「mod2 / mod10」这类带数字名给出相反结果，会导致与后端权威序分叉
  return naturalCompare(String(a.name ?? a.modName ?? ''), String(b.name ?? b.modName ?? ''))
}

/**
 * 就地按轻量扫描口径排序（None 空槽位锁定第 0 位，其余稳定排序）。
 *
 * @param mods 容器内的模组数组（就地重排）。None 已在 0 位时零变动。
 */
export function sortModsLikeLightScan<T extends SortableMod>(mods: T[]): void {
  const noneIdx = mods.findIndex(isNoneSlotMod)
  const none = noneIdx >= 0 ? mods.splice(noneIdx, 1)[0] : undefined
  mods.sort(compareModsLikeLightScan)
  if (none !== undefined) mods.unshift(none)
}

/**
 * 重编号组内 mod_index（None=0，真实模组从 1 起；与后端扫描器分配口径一致）。
 *
 * 容器重排后调用，保证位次语义（selectedindex / $managed_slot_id 基准）不漂移。
 */
export function reindexMods<T extends { modIndex?: number; name?: string | null; modPath?: string }>(
  mods: T[],
): void {
  for (let i = 0; i < mods.length; i++) {
    mods[i].modIndex = i
  }
}