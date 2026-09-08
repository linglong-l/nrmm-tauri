/**
 * 模组目录路径预测（启停链路乐观更新用）
 *
 * 镜像后端 `derive_toggled_path` / `mod_scanner::strip_disabled_prefix` 的重命名口径
 * （规范源：src-tauri/src/commands/mod_commands.rs / core/mod_scanner.rs），供前端在
 * 调用后端目录重命名前预测目标路径、先行呈现 UI：
 * - 禁用 = 目录名加精确全大写 `DISABLED` 前缀（无分隔符）
 * - 启用 = 剥离连续的 DISABLED 前缀（不区分大小写，容忍 `_`/`-`/空格分隔符）
 * - 幂等：已处于目标状态时返回原路径
 *
 * 单测（modPath.test.ts）与后端口径双向锁定（S5：预测端同口径幂等）。
 */

/** DISABLED 前缀匹配（与后端 DISABLED_PREFIX_RE `^(?i:disabled)[_\- ]*` 一致） */
const DISABLED_PREFIX_RE = /^(?:disabled)[_\- ]*/i

/** 后端常量 `constants::DISABLED_PREFIX`（精确全大写、无分隔符） */
const DISABLED_PREFIX = 'DISABLED'

/** 判断目录名是否带 DISABLED 前缀（不区分大小写，含 `_`/`-`/空格分隔符） */
export function isDisabledDir(name: string): boolean {
  return DISABLED_PREFIX_RE.test(name)
}

/**
 * 剥离连续的 DISABLED 前缀（一次移除全部累积前缀，如 `DISABLEDDISABLEDMod` → `Mod`）。
 *
 * 与后端一致的分寸：仅当剥离后剩余部分以精确全大写 `DISABLED` 开头才继续循环，
 * 避免把模组名中自然的大小写混合 "Disabled" 误删。
 */
export function stripDisabledPrefix(name: string): string {
  let result = name
  for (;;) {
    const stripped = result.replace(DISABLED_PREFIX_RE, '')
    if (stripped === result) return result // 无可剥前缀
    if (!stripped.startsWith(DISABLED_PREFIX)) return stripped
    result = stripped
  }
}

/**
 * 预测启用/禁用后的模组目录路径（与后端 `derive_toggled_path` 同口径；幂等时返回原路径）。
 *
 * @param path 模组目录绝对路径（容忍 `/` 或 `\\` 分隔符，保留原分隔符形态）
 * @param targetDisabled 目标状态：true = 禁用（加前缀），false = 启用（剥前缀）
 * @returns 预测的目标路径；已处于目标状态（幂等）时返回原路径
 */
export function predictToggledPath(path: string, targetDisabled: boolean): string {
  const sepIdx = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  const parent = sepIdx >= 0 ? path.slice(0, sepIdx) : ''
  const sep = sepIdx >= 0 ? path[sepIdx] : ''
  const name = sepIdx >= 0 ? path.slice(sepIdx + 1) : path
  const join = (n: string): string => (parent ? `${parent}${sep}${n}` : n)

  if (targetDisabled) {
    // 禁用：已禁用（幂等）→ 原路径；否则加精确全大写 DISABLED 前缀
    return isDisabledDir(name) ? path : join(DISABLED_PREFIX + name)
  }
  // 启用：剥前缀；无可剥（幂等）或剥后为空 → 原路径
  const stripped = stripDisabledPrefix(name)
  if (stripped === name || stripped === '') return path
  return join(stripped)
}
