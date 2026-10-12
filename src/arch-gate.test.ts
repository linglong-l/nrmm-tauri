// @ts-nocheck -- 项目未引入 @types/node 且规则禁止改动 tsconfig/package.json。
// 本门禁含文件 IO，需 node:fs/node:path；vitest 下 import.meta.url 非 file: 协议、
// __dirname 运行时有效（vite-node 注入）。后续若引入 @types/node 可移除本行并恢复类型检查。
/**
 * 前端架构健身函数：F4 行数棘轮、F3 utils 依赖方向、F5 事件名常量化、
 * F6 i18n key 登记、F7 五语言包键树一致（含空命名空间）、F8 词条必须有可达出口。
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
      'stores/mods.ts': 1683,
      'views/SettingsView.vue': 1506,
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
    // 词条表两种风格并存：含点的扁平键（整串就是一个键）须先整串命中，
    // 未命中再按点走嵌套查找，否则会误报一堆存在的键。
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

  it('F7: 五个语言包键树必须完全一致（含空命名空间，不止叶子键）', () => {
    const locales = ['en', 'zh-CN', 'zh-TW', 'id', 'ru']
    // 结构签名 = 所有对象节点路径 ∪ 所有叶子路径。旧口径只比叶子集合，
    // 看不见 `"app": {}` 这类空命名空间的有无差异（本仓库实测：en/zh-CN 有、其余三包没有）。
    const structureOf = (obj: unknown, pre = '', out: string[] = []): string[] => {
      for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
        const key = pre ? `${pre}.${k}` : k
        if (v && typeof v === 'object' && !Array.isArray(v)) {
          out.push(key + '/')
          if (Object.keys(v).length) structureOf(v, key, out)
        } else out.push(key)
      }
      return out
    }
    const sets: Record<string, Set<string>> = {}
    for (const l of locales) {
      sets[l] = new Set(structureOf(JSON.parse(readFileSync(join(srcRoot, `utils/i18n/${l}.json`), 'utf8'))))
    }
    const all = new Set(locales.flatMap((l) => [...sets[l]]))
    const offenders = locales.flatMap((l) => [...all].filter((k) => !sets[l].has(k)).map((k) => `[${l}] ${k}`))
    expect(offenders, offenders.join('\n')).toHaveLength(0)
  })

  it('F8: 语言包每个键都必须有可达出口（无人引用的词条等于漏做功能）', () => {
    // 口径：只有**非测试**的 .ts/.vue 里的引用才算用户可见出口——测试断言里的键不代表功能存在。
    // 注释行不算出口：否则门禁文档里提一句键名，就能让一条死词条永远清不掉。
    // WHITELIST 只收「已决定实现、但尚未接线」的簇，且每条必须写理由；空表是最强口径。
    const WHITELIST: string[] = []

    const en = JSON.parse(readFileSync(join(srcRoot, 'utils/i18n/en.json'), 'utf8'))
    const leaves: string[] = []
    const flatLeaves = (obj: unknown, pre = '') => {
      for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
        const key = pre ? `${pre}.${k}` : k
        if (v && typeof v === 'object' && !Array.isArray(v)) flatLeaves(v, key)
        else leaves.push(key)
      }
    }
    flatLeaves(en)

    const COMMENT_LINE = /^\s*(\/\/|\/\*|\*|<!--)/
    // 与 F6 同一套调用形态，另加「裸字符串字面量」通道，兜住先取 const label = 'k' 再传参的写法
    const callRe = /\b(?:\$t|t|i18n\.t|global\.t|locale\.t)\(\s*(['"])((?:[^'"\n\\]|\\.)*)\1\s*[,)]/g
    const reachable = new Set<string>()
    const files = collectFiles(srcRoot, ['.ts', '.vue']).filter((p) => !p.endsWith('.test.ts'))
    for (const f of files) {
      const text = readFileSync(f, 'utf8')
        .split('\n')
        .filter((l) => !COMMENT_LINE.test(l))
        .join('\n')
      let m: RegExpExecArray | null
      while ((m = callRe.exec(text)) !== null) reachable.add(m[2].replace(/\\(['"])/g, '$1'))
      for (const key of leaves) {
        if (!reachable.has(key) && (text.includes(`'${key}'`) || text.includes(`"${key}"`))) reachable.add(key)
      }
    }

    const offenders = leaves.filter((k) => !reachable.has(k) && !WHITELIST.includes(k))
    expect(
      offenders,
      `F8 无出口词条（${offenders.length} 条，会随语言包一直背着）:\n${offenders.join('\n')}\n` +
        `确属「已决定实现但尚未接线」的，请带理由加进 WHITELIST`
    ).toHaveLength(0)
  })
})