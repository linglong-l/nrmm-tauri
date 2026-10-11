// @ts-nocheck -- 项目未引入 @types/node 且规则禁止改动 tsconfig/package.json。
// 本门禁含文件 IO，需 node:fs/node:path；vitest 下 import.meta.url 非 file: 协议、
// __dirname 运行时有效（vite-node 注入）。后续若引入 @types/node 可移除本行并恢复类型检查。
/**
 * 前端架构健身函数：F4 行数棘轮、F3 utils 依赖方向、F5 事件名常量化、
 * F6 i18n key 登记、F7 五语言包键集一致。
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
      'stores/mods.ts': 1796,
      'views/SettingsView.vue': 1509,
      'components/mod/GroupPanel.vue': 961,
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

  it('F5: listen 调用不得使用裸字符串事件名（须引用 src/events.ts 的 IpcEvent）', () => {
    const files = collectFiles(srcRoot, ['.ts', '.vue']).filter((p) => !p.endsWith('.test.ts'))
    const offenders: string[] = []
    const re = /listen(?:<[^>]*>)?\(\s*['"`]([^'"`]+)['"`]/g
    for (const f of files) {
      const content = readFileSync(f, 'utf8')
      let m: RegExpExecArray | null
      while ((m = re.exec(content)) !== null) offenders.push(`${rel(f)}: '${m[1]}'`)
    }
    expect(offenders, offenders.join('\n')).toHaveLength(0)
  })

  it('F6: 字面量 t() key 必须在 en.json 有定义（缺 key 会回显原文，等于漏登记翻译）', () => {
    const en = JSON.parse(readFileSync(join(srcRoot, 'utils/i18n/en.json'), 'utf8'))
    // 词条表两种风格并存："Updater.title"/"Loading..." 这类含点的扁平键须先整串命中，
    // 未命中再按点走嵌套（"mods.title"），否则会误报一堆存在的键。
    const resolves = (key: string) => {
      if (en[key] !== undefined) return true
      const walked = key
        .split('.')
        .reduce((o, p) => (o && typeof o === 'object' ? o[p] : undefined), en)
      return walked !== undefined
    }
    // 只认字面量键（t(`模板串`) 与 t(varKey) 交给运行期兜底），key 后紧跟 , 或 ) 即算命中，
    // 以覆盖 t('key', { 插值 }) 形态
    const re = /\bt\(\s*(['"])((?:[^'"\n\\]|\\.)*)\1\s*[,)]/g
    const files = collectFiles(srcRoot, ['.ts', '.vue']).filter((p) => !p.endsWith('.test.ts'))
    const offenders: string[] = []
    for (const f of files) {
      const content = readFileSync(f, 'utf8')
      let m: RegExpExecArray | null
      while ((m = re.exec(content)) !== null) {
        const key = m[2].replace(/\\(['"])/g, '$1')
        if (!resolves(key)) offenders.push(`${rel(f)}: ${m[1]}${key}${m[1]}`)
      }
    }
    expect(offenders, offenders.join('\n')).toHaveLength(0)
  })

  it('F7: 五个语言包键集必须完全一致（缺键静默回退英文，多键则永无出口）', () => {
    const locales = ['en', 'zh-CN', 'zh-TW', 'id', 'ru']
    const flatOf = (obj: unknown, pre = '', out: string[] = []): string[] => {
      for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
        const key = pre ? `${pre}.${k}` : k
        if (v && typeof v === 'object' && !Array.isArray(v)) flatOf(v, key, out)
        else out.push(key)
      }
      return out
    }
    const sets: Record<string, Set<string>> = {}
    for (const l of locales) {
      sets[l] = new Set(flatOf(JSON.parse(readFileSync(join(srcRoot, `utils/i18n/${l}.json`), 'utf8'))))
    }
    const all = new Set(locales.flatMap((l) => [...sets[l]]))
    const offenders = locales.flatMap((l) => [...all].filter((k) => !sets[l].has(k)).map((k) => `[${l}] ${k}`))
    expect(offenders, offenders.join('\n')).toHaveLength(0)
  })
})