// The reference: tailwindcss 3.4.19 (npm) + autoprefixer 10.4.20 under postcss 8.4.49, the way a
// stylesheet with `@tailwind` directives is built. Autoprefixer keeps the Play bundle's data:
// its prefixes table (src/autoprefixer_data.json, extracted from the bundle) and its 14 target
// browsers, with `remove: false`.
//
// Candidates are passed as raw content split on spaces, so the class list is exactly the given
// strings (as with the Play bundle's DOM classes). `@config` paths are written to a temporary
// directory with the case's config.
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)

export const TAILWIND = 'tailwindcss'
const tailwindcss = require(TAILWIND)
const sharedState = require(`${TAILWIND}/lib/lib/sharedState`)
const postcss = require('postcss')
const autoprefixer = require('autoprefixer')

autoprefixer.data.prefixes = JSON.parse(fs.readFileSync(new URL('../src/autoprefixer_data.json', import.meta.url), 'utf8'))
const BROWSERS = [
  'and_chr 114',
  'and_uc 15.5',
  'chrome 114',
  'chrome 113',
  'chrome 109',
  'edge 114',
  'firefox 114',
  'ios_saf 16.5',
  'ios_saf 16.4',
  'ios_saf 16.3',
  'ios_saf 16.1',
  'opera 99',
  'safari 16.5',
  'samsung 21',
]

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'tw-ref-'))
let counter = 0

/**
 * @param {{config?: object, css?: string, candidates?: string[]}} input
 * @returns {Promise<{css: string} | {error: object}>}
 */
export async function runRef({ config = {}, css = '', candidates = [] }) {
  // the user's `content` object stays (e.g. `content.safelist`); its files are the candidates
  const own = config.content && typeof config.content === 'object' && !Array.isArray(config.content) ? config.content : {}
  const full = {
    ...config,
    content: { ...own, files: [{ raw: candidates.join(' '), extension: 'html' }], extract: { html: s => s.split(' ') } },
  }
  // `@config "<path>"`: the config file at that path (relative to the stylesheet)
  const dir = path.join(tmp, String(counter++))
  fs.mkdirSync(dir, { recursive: true })
  const from = path.join(dir, 'input.css')
  for (const m of css.matchAll(/@config\s+(['"])(.*?)\1/g)) {
    if (path.isAbsolute(m[2])) continue
    const file = path.resolve(dir, m[2])
    fs.mkdirSync(path.dirname(file), { recursive: true })
    if (file.endsWith('.json')) {
      // JSON cannot carry the extractor: the default one sees the candidates (use plain ones)
      fs.writeFileSync(file, JSON.stringify({ ...config, content: { ...own, files: [{ raw: candidates.join(' '), extension: 'html' }] } }))
    } else {
      fs.writeFileSync(file, `module.exports = ${JSON.stringify(config)};\nmodule.exports.content = ${contentSource(own, candidates)};\n`)
    }
  }
  try {
    const result = await postcss([tailwindcss(full), autoprefixer({ remove: false, overrideBrowserslist: BROWSERS })]).process(css, { from })
    return { css: result.css }
  } catch (e) {
    return {
      error: {
        name: e.name,
        // as postcss reports a stylesheet without `from`
        message: typeof e.message === 'string' ? e.message.split(from).join('<css input>') : e.message,
        reason: e.reason ?? null,
        line: e.line ?? null,
        column: e.column ?? null,
        endLine: e.endLine ?? null,
        endColumn: e.endColumn ?? null,
      },
    }
  } finally {
    // every case starts from a fresh context (v3 keeps rules per config across builds)
    sharedState.contextMap.clear()
    sharedState.configContextMap.clear()
    sharedState.contextSourcesMap.clear()
    sharedState.sourceHashMap.clear()
    fs.rmSync(dir, { recursive: true, force: true })
  }
}

function contentSource(own, candidates) {
  return `{ ...${JSON.stringify(own)}, files: [{ raw: ${JSON.stringify(candidates.join(' '))}, extension: 'html' }], extract: { html: s => s.split(' ') } }`
}
