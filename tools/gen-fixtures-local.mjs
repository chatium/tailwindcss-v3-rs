// Reference outputs for the local production corpus (tests/corpus-local/accounts.json, built by
// extract-corpus.mjs): the stylesheet `@tailwind base;@tailwind components;@tailwind utilities;`
// + the setup's blocks through the reference (ref.mjs), in parallel worker processes. Writes
// tests/fixtures-local/accounts/<index>.json = { name, account, expected, dropped? }.
//
//   node tools/gen-fixtures-local.mjs [--jobs N] [--only substring]
import fs from 'node:fs'
import os from 'node:os'
import { fork } from 'node:child_process'
import { reference } from './gen-fixtures.mjs'

const PREFIX = '@tailwind base;@tailwind components;@tailwind utilities;'
const corpusPath = new URL('../tests/corpus-local/accounts.json', import.meta.url)
const outDir = new URL('../tests/fixtures-local/accounts/', import.meta.url)
if (process.argv[2] === '--worker') {
  const corpus = JSON.parse(fs.readFileSync(corpusPath, 'utf8'))
  process.on('message', async ({ index }) => {
    const c = corpus.cases[index]
    const { expected, dropped } = await reference({ config: c.config, css: PREFIX + c.blocks, candidates: corpus.candidates[c.account] })
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
