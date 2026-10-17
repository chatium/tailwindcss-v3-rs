// Compares the Rust `extract_candidates` with tailwind's own default extractor (npm
// tailwindcss@3.4.16) on every non-media file under the given directories.
//   node tools/check-extractor.mjs <dir>…
import fs from 'node:fs'
import path from 'node:path'
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
const require = createRequire(import.meta.url)
const { defaultExtractor } = require('tailwindcss/lib/lib/defaultExtractor')

export const MEDIA = /\.(png|jpe?g|gif|webp|avif|ico|bmp|tiff?|svgz|mp4|webm|mov|avi|mkv|mp3|wav|ogg|m4a|flac|aac|woff2?|ttf|otf|eot|zip|gz|tgz|rar|7z|pdf|docx?|xlsx?|pptx?|wasm|bin|exe|dll|so|dylib|db|sqlite|lock)$/i
export function listFiles(dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    if (e.name === 'node_modules' || e.name === '.git') continue
    const p = path.join(dir, e.name)
    if (e.isDirectory()) listFiles(p, out)
    else if (e.isFile() && !MEDIA.test(e.name)) out.push(p)
  }
  return out
}

export function jsCandidates(text) {
  const extractor = defaultExtractor({ tailwindConfig: { separator: ':', prefix: '' } })
  const seenLines = new Set()
  const out = []
  const seen = new Set()
  for (let line of text.split('\n')) {
    line = line.trim()
    if (seenLines.has(line)) continue
    seenLines.add(line)
    for (const m of extractor(line)) {
      if (m === '!*' || seen.has(m)) continue
      seen.add(m)
      out.push(m)
    }
  }
  return out
}

if (process.argv[1] === new URL(import.meta.url).pathname) {
  const files = process.argv.slice(2).flatMap(d => listFiles(d))
  const rust = JSON.parse(
    execFileSync('cargo', ['run', '-q', '--release', '--example', 'extract'], {
      cwd: new URL('..', import.meta.url).pathname,
      input: files.join('\n'),
      maxBuffer: 1 << 30,
    }).toString()
  )
  let ok = 0
  let bad = 0
  const t = Date.now()
  for (const f of files) {
    const text = fs.readFileSync(f, 'utf8')
    const want = jsCandidates(text)
    const got = rust[f] ?? []
    if (JSON.stringify(want) === JSON.stringify(got)) ok++
    else {
      bad++
      if (bad <= 10) {
        const ws = new Set(want), gs = new Set(got)
        console.log('MISMATCH', f, 'missing:', want.filter(x => !gs.has(x)).slice(0, 5), 'extra:', got.filter(x => !ws.has(x)).slice(0, 5), want.length, got.length)
      }
    }
  }
  console.log(`extractor: ${ok}/${files.length} files identical, JS took ${Date.now() - t}ms`)
}
