// Runs the reference (tools/ref.mjs: tailwindcss 3.4.19 + autoprefixer under postcss) over every
// case in tests/corpus/*.json and writes the expected output into tests/fixtures/<name>.json.
//
// A case is { config, css, candidates }. Expected output is { css } or { error }.
// Candidates that make tailwind itself throw (the JS build crashes on them; the port drops
// them) are found by bisection and excluded from the reference run; they stay in the case
// (`dropped` lists them) so the port is checked to ignore them.
//
//   node tools/gen-fixtures.mjs [corpus name…]
import fs from 'node:fs'
import path from 'node:path'
import { runRef } from './ref.mjs'

const corpusDir = new URL('../tests/corpus/', import.meta.url).pathname
const fixturesDir = new URL('../tests/fixtures/', import.meta.url).pathname
fs.mkdirSync(fixturesDir, { recursive: true })

const only = process.argv.slice(2)

/** The expected output, with the candidates that crash tailwind on their own left out. */
export async function reference(input) {
  let r = await runRef(input)
  const dropped = []
  if (r.error && r.error.name !== 'CssSyntaxError' && input.candidates.length) {
    // is it the candidates? (an error without any candidates is a genuine expected error)
    const base = await runRef({ ...input, candidates: [] })
    if (!base.error) {
      const crashes = async list => {
        const x = await runRef({ ...input, candidates: list })
        return x.error && x.error.name !== 'CssSyntaxError'
      }
      const search = async list => {
        if (!list.length || !(await crashes(list))) return
        if (list.length === 1) return void dropped.push(list[0])
        const mid = list.length >> 1
        await search(list.slice(0, mid))
        await search(list.slice(mid))
      }
      await search(input.candidates)
      r = await runRef({ ...input, candidates: input.candidates.filter(x => !dropped.includes(x)) })
    }
  }
  return { expected: r, dropped }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  for (const file of fs.readdirSync(corpusDir).sort()) {
    if (!file.endsWith('.json')) continue
    const name = path.basename(file, '.json')
    if (only.length && !only.includes(name)) continue
    const cases = JSON.parse(fs.readFileSync(corpusDir + file, 'utf8'))
    const out = []
    let dropped = 0
    for (const c of cases) {
      const input = { config: c.config ?? {}, css: c.css ?? '', candidates: c.candidates ?? [] }
      const r = await reference(input)
      dropped += r.dropped.length
      out.push({ ...c, expected: r.expected, ...(r.dropped.length ? { dropped: r.dropped } : {}) })
    }
    fs.writeFileSync(fixturesDir + name + '.json', JSON.stringify(out))
    const errors = out.filter(c => c.expected.error).length
    console.log(`${name}: ${out.length} cases, ${errors} expected errors, ${dropped} crashing candidates dropped`)
  }
  process.exit(0)
}
