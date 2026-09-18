// @ts-nocheck -- 项目未引入 @types/node 且规则禁止改动 tsconfig/package.json。
// 本门禁含文件 IO，需 node:fs/node:path；vitest 下 import.meta.url 非 file: 协议、
// __dirname 运行时有效（vite-node 注入）。后续若引入 @types/node 可移除本行并恢复类型检查。
/**
 * 前端架构健身函数：行数棘轮（F4）。
 * F3（utils 反向依赖）与 F5（事件常量化）用例由后续任务以 TDD 追加。
 *
 * 设计文档：docs/superpowers/specs/2026-09-18-arch-fitness-and-monolith-split-design.md
 */
import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

const srcRoot = __dirname

function collectFiles(dir: string, exts: string[], out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) collectFiles(p, exts, out)
    else if (exts.some((e) => p.endsWith(e))) out.push(p)
  }
  return out
}

const rel = (p: string) => relative(srcRoot, p).replaceAll('\\', '/')

describe('前端架构健身函数', () => {
  it('F4: 文件行数棘轮（新文件 ≤800；存量豁免只减不增）', () => {
    const LEGACY_CAPS: Record<string, number> = {
      'stores/mods.ts': 1823,
      'views/SettingsView.vue': 1509,
      'components/mod/GroupPanel.vue': 953,
      'utils/tauri.ts': 902,
    }
    const MAX = 800
    const files = collectFiles(srcRoot, ['.ts', '.vue']).filter((p) => !p.endsWith('.test.ts'))
    const offenders: string[] = []
    for (const f of files) {
      const r = rel(f)
      const lines = readFileSync(f, 'utf8').split('\n').length
      const cap = LEGACY_CAPS[r] ?? MAX
      if (lines > cap) offenders.push(`${r}: ${lines} 行 > 上限 ${cap}`)
    }
    expect(offenders, offenders.join('\n')).toHaveLength(0)
  })

  it('F3: src/utils 不得导入 stores（依赖方向：stores/composables → utils）', () => {
    const files = collectFiles(join(srcRoot, 'utils'), ['.ts']).filter((p) => !p.endsWith('.test.ts'))
    const offenders: string[] = []
    for (const f of files) {
      const content = readFileSync(f, 'utf8')
      const m = content.match(/from\s+['"](@\/stores\/|\.\.?\/(?:\.\.?\/)*stores\/)/)
      if (m) offenders.push(`${rel(f)}: ${m[0]}`)
    }
    expect(offenders, offenders.join('\n')).toHaveLength(0)
  })
})