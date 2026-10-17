// Reference outputs for the local production corpus (tests/corpus-local/accounts.json, built by
// extract-corpus.mjs). Runs the Play bundle for every case in parallel worker processes and writes
// tests/fixtures-local/accounts/<index>.json = { name, account, expected, dropped? }.
//
//   node tools/gen-fixtures-local.mjs [--jobs N] [--only substring]
import fs from 'node:fs'
import os from 'node:os'
import { fork } from 'node:child_process'
import { runPlay } from './play.mjs'

const corpusPath = new URL('../tests/corpus-local/accounts.json', import.meta.url)
const outDir = new URL('../tests/fixtures-local/accounts/', import.meta.url)

async function reference(config, blocks, candidates) {
  let r = await runPlay({ config, blocks, candidates })
  let dropped = []
  if (r.error && r.error.name !== 'CssSyntaxError' && candidates.length) {
    const base = await runPlay({ config, blocks, candidates: [] })
    if (!base.error) {
      const crashes = async list => {
        const x = await runPlay({ config, blocks, candidates: list })
        return x.error && x.error.name !== 'CssSyntaxError'
      }
      const search = async list => {
        if (!list.length || !(await crashes(list))) return
        if (list.length === 1) return void dropped.push(list[0])
        const mid = list.length >> 1
        await search(list.slice(0, mid))
        await search(list.slice(mid))
      }
      await search(candidates)
      const kept = candidates.filter(c => !dropped.includes(c))
      r = await runPlay({ config, blocks, candidates: kept })
    }
  }
  return { expected: r, dropped }
}

if (process.argv[2] === '--worker') {
  const corpus = JSON.parse(fs.readFileSync(corpusPath, 'utf8'))
  process.on('message', async ({ index }) => {
    const c = corpus.cases[index]
    const { expected, dropped } = await reference(c.config, c.blocks, corpus.candidates[c.account])
    fs.writeFileSync(new URL(`${index}.json`, outDir), JSON.stringify({ name: c.name, account: c.account, expected, ...(dropped.length ? { dropped } : {}) }))
    process.send({ index, ok: !expected.error, dropped: dropped.length })
  })
  process.send({ ready: true })
} else {
  const args = process.argv.slice(2)
  const jobs = Number(args[args.indexOf('--jobs') + 1]) || Math.max(1, os.cpus().length - 2)
  const only = args.includes('--only') ? args[args.indexOf('--only') + 1] : null
  const corpus = JSON.parse(fs.readFileSync(corpusPath, 'utf8'))
  fs.mkdirSync(outDir, { recursive: true })
  // largest candidate sets first so the long cases start early
  const queue = corpus.cases
    .map((c, i) => i)
    .filter(i => !only || corpus.cases[i].name.includes(only))
    .filter(i => !fs.existsSync(new URL(`${i}.json`, outDir)))
    .sort((a, b) => corpus.candidates[corpus.cases[b].account].length - corpus.candidates[corpus.cases[a].account].length)
  let done = 0
  const total = queue.length
  const t0 = Date.now()
  await Promise.all(
    Array.from({ length: Math.min(jobs, total) }, () => new Promise(resolve => {
      const w = fork(new URL(import.meta.url).pathname, ['--worker'], { execArgv: ['--max-old-space-size=8192'] })
      const next = () => {
        const index = queue.shift()
        if (index === undefined) {
          w.kill()
          return resolve()
        }
        w.send({ index })
      }
      w.on('message', m => {
        if (m.ready) return next()
        done++
        if (done % 20 === 0 || done === total) console.log(`${done}/${total} (${((Date.now() - t0) / 1000).toFixed(0)}s)`)
        next()
      })
    }))
  )
  console.log('done')
}
