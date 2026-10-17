// Runs the reference (the Play CDN bundle in node:vm) over every case in tests/corpus/*.json and
// writes the expected output into tests/fixtures/<name>.json.
//
// A case is { config, blocks, candidates }. Expected output is { css } or { error }.
// Candidates that make the bundle itself throw (the JS build crashes on them; the port drops
// them) are found by bisection and excluded from the reference run; they stay in the case
// (`dropped` lists them) so the port is checked to ignore them.
import fs from 'node:fs'
import path from 'node:path'
import { runPlay } from './play.mjs'

const corpusDir = new URL('../tests/corpus/', import.meta.url).pathname
const fixturesDir = new URL('../tests/fixtures/', import.meta.url).pathname
fs.mkdirSync(fixturesDir, { recursive: true })

const only = process.argv.slice(2)

async function crashers(c) {
  // bisect candidates whose presence alone makes the bundle throw a non-CssSyntaxError
  const bad = []
  async function crashes(list) {
    const r = await runPlay({ config: c.config, blocks: c.blocks, candidates: list })
    return r.error && r.error.name !== 'CssSyntaxError'
  }
  async function search(list) {
    if (list.length === 0) return
    if (!(await crashes(list))) return
    if (list.length === 1) {
      bad.push(list[0])
      return
    }
    const mid = list.length >> 1
    await search(list.slice(0, mid))
    await search(list.slice(mid))
  }
  await search(c.candidates)
  return bad
}

for (const file of fs.readdirSync(corpusDir).sort()) {
  if (!file.endsWith('.json')) continue
  const name = path.basename(file, '.json')
  if (only.length && !only.includes(name)) continue
  const cases = JSON.parse(fs.readFileSync(corpusDir + file, 'utf8'))
  const out = []
  let dropped = 0
  for (const c of cases) {
    const input = { config: c.config ?? {}, blocks: c.blocks ?? '', candidates: c.candidates ?? [] }
    let r = await runPlay(input)
    let drop = []
    if (r.error && r.error.name !== 'CssSyntaxError' && input.candidates.length) {
      // is it the candidates? (an error without any candidates is a genuine expected error)
      const base = await runPlay({ ...input, candidates: [] })
      if (!base.error) {
        drop = await crashers(input)
        if (drop.length) {
          const kept = input.candidates.filter(x => !drop.includes(x))
          r = await runPlay({ ...input, candidates: kept })
          dropped += drop.length
        }
      }
    }
    out.push({ ...c, ...input, ...(drop.length ? { dropped: drop } : {}), expected: r })
  }
  fs.writeFileSync(fixturesDir + name + '.json', JSON.stringify(out))
  console.log(`${name}: ${out.length} cases${dropped ? `, ${dropped} crashing candidates dropped` : ''}`)
}
