// Collects the inputs of tailwindcss v3.4.19's own test-suite (tools/upstream/tests/*.test.js,
// the v3.4.19 tag) by running every test with a stubbed `run(input, config)` that records its
// arguments. Each recorded call becomes a corpus case for the Play bundle:
//   css        = the test's CSS input
//   config     = the test's config without `content` (the candidates replace it)
//   candidates = tailwind's default extractor over the `content` raw strings
// Calls whose config contains functions, plugins or file-based content are skipped and counted.
//
//   node tools/extract-tw-tests.mjs  → tests/corpus/tailwind-tests.json
import fs from 'node:fs'
import path from 'node:path'
import Module from 'node:module'
import { createRequire } from 'node:module'
const require = createRequire(import.meta.url)
// tailwind's src/ and tests/ resolve their dependencies from tools/node_modules
const nm = new URL('./node_modules/', import.meta.url).pathname
process.env.NODE_PATH = [nm, nm + 'tailwindcss/node_modules'].join(path.delimiter)
Module._initPaths()
if (!fs.existsSync(new URL('./upstream/src/corePluginList.js', import.meta.url))) {
  fs.copyFileSync(new URL('./node_modules/tailwindcss/src/corePluginList.js', import.meta.url), new URL('./upstream/src/corePluginList.js', import.meta.url))
}
require('sucrase/register')
const { defaultExtractor } = require('tailwindcss/lib/lib/defaultExtractor')

const testsDir = new URL('./upstream/tests/', import.meta.url).pathname
const runPath = path.join(testsDir, 'util/run.js')

const calls = []
let currentFile = ''
let currentTest = ''
const stats = { files: 0, calls: 0, kept: 0, skipped: {} }
const skip = reason => (stats.skipped[reason] = (stats.skipped[reason] ?? 0) + 1)

// ---- the stubbed harness

const fakeResult = { css: '', root: { toString: () => '' }, messages: [], warnings: () => [], map: { toJSON: () => ({}) } }
function record(input, config) {
  calls.push({ file: currentFile, test: currentTest, input, config })
  return Promise.resolve(fakeResult)
}
const strings = require(path.join(testsDir, 'util/strings.js'))
const defaults = require(path.join(testsDir, 'util/defaults.js'))
const stubRun = { ...strings, ...defaults, run: record, runWithSourceMaps: record, map: '{}' }

const origResolve = Module._resolveFilename
Module._resolveFilename = function (request, parent, ...rest) {
  const resolved = origResolve.call(this, request, parent, ...rest)
  return resolved
}
const origLoad = Module._load
Module._load = function (request, parent, ...rest) {
  if (parent && /\/upstream\/tests\//.test(parent.filename) && /(^|\/)util\/run(\.js)?$/.test(request)) return stubRun
  return origLoad.call(this, request, parent, ...rest)
}

const anyProxy = () => {
  const f = function () {
    return p
  }
  const p = new Proxy(f, {
    get: (t, k) => (k === 'then' ? undefined : k === Symbol.toPrimitive ? () => '' : p),
    apply: () => p,
  })
  return p
}
const tests = []
function makeTest(prefix) {
  const t = (name, fn) => tests.push({ name: prefix + name, fn })
  t.each = table => (name, fn) => {
    for (const row of table) {
      const args = Array.isArray(row) ? row : [row]
      let n = name
      let i = 0
      n = n.replace(/%[sdpij#]/g, () => String(args[i++]))
      tests.push({ name: prefix + n, fn: () => fn(...args) })
    }
  }
  t.skip = () => {}
  t.only = t
  t.todo = () => {}
  t.concurrent = t
  t.failing = () => {}
  return t
}
let prefixStack = []
globalThis.test = makeTest('')
globalThis.it = globalThis.test
globalThis.describe = (name, fn) => {
  prefixStack.push(name)
  const saved = globalThis.test
  globalThis.test = globalThis.it = makeTest(prefixStack.join(' ') + ' ')
  try {
    fn()
  } finally {
    globalThis.test = globalThis.it = saved
    prefixStack.pop()
  }
}
globalThis.describe.each = table => (name, fn) => {
  for (const row of table) globalThis.describe(name, () => fn(...(Array.isArray(row) ? row : [row])))
}
globalThis.describe.skip = () => {}
globalThis.describe.only = globalThis.describe
const hooks = { beforeEach: [], afterEach: [], beforeAll: [], afterAll: [] }
for (const h of Object.keys(hooks)) globalThis[h] = fn => hooks[h].push(fn)
globalThis.expect = Object.assign(() => anyProxy(), {
  getState: () => ({ currentTestName: currentTest }),
  extend: () => {},
  any: () => anyProxy(),
  stringContaining: () => anyProxy(),
  objectContaining: () => anyProxy(),
  arrayContaining: () => anyProxy(),
  assertions: () => {},
  hasAssertions: () => {},
})
globalThis.jest = { fn: (impl = () => {}) => Object.assign((...a) => impl(...a), { mockImplementation: () => {}, mock: { calls: [] } }), spyOn: () => anyProxy(), mock: () => {}, setTimeout: () => {}, resetModules: () => {} }

// ---- run every test file

for (const file of fs.readdirSync(testsDir).sort()) {
  if (!file.endsWith('.test.js')) continue
  currentFile = file
  tests.length = 0
  for (const h of Object.keys(hooks)) hooks[h].length = 0
  try {
    require(path.join(testsDir, file))
  } catch (e) {
    skip('file-load-error')
    if (process.env.DEBUG) console.log('load error', file, String(e).slice(0, 200))
    continue
  }
  stats.files++
  for (const h of hooks.beforeAll) try { await h() } catch {}
  for (const t of tests) {
    currentTest = t.name
    for (const h of hooks.beforeEach) try { await h() } catch {}
    try {
      await Promise.race([Promise.resolve().then(() => t.fn(() => {})), new Promise(r => setTimeout(r, 2000))])
    } catch {}
    for (const h of hooks.afterEach) try { await h() } catch {}
  }
}

// ---- turn calls into cases

function hasFunction(v, seen = new Set()) {
  if (typeof v === 'function') return true
  if (v && typeof v === 'object') {
    if (seen.has(v)) return false
    seen.add(v)
    if (v instanceof RegExp) return true
    return Object.values(v).some(x => hasFunction(x, seen))
  }
  return false
}

function candidatesOf(config) {
  const extractor = defaultExtractor({ tailwindConfig: { separator: config.separator ?? ':', prefix: typeof config.prefix === 'string' ? config.prefix : '' } })
  const content = config.content
  const files = Array.isArray(content) ? content : Array.isArray(content?.files) ? content.files : []
  const out = new Set()
  for (const f of files) {
    if (typeof f === 'string') return null
    if (typeof f?.raw !== 'string') continue
    for (let line of f.raw.split('\n')) {
      line = line.trim()
      for (const m of extractor(line)) if (m !== '!*') out.add(m)
    }
  }
  return [...out]
}

const cases = []
const seen = new Set()
for (const c of calls) {
  stats.calls++
  const config = c.config ?? {}
  if (typeof c.input !== 'string') {
    skip('non-string-input')
    continue
  }
  if (config.config !== undefined) {
    // `tailwindcss({ config })`: the plugin's own form, not a config file
    skip('config-key')
    continue
  }
  if (config.plugins?.length) {
    skip('plugins')
    continue
  }
  const { content, ...rest } = config
  if (hasFunction(rest)) {
    skip('functions')
    continue
  }
  if (content && (content.extract || content.transform)) {
    skip('custom-extractor')
    continue
  }
  const candidates = candidatesOf(config)
  if (candidates === null) {
    skip('file-content')
    continue
  }
  const kase = { name: `${c.file} ${c.test}`, config: JSON.parse(JSON.stringify(rest)), css: c.input, candidates }
  const key = JSON.stringify([kase.config, kase.css, kase.candidates])
  if (seen.has(key)) {
    skip('duplicate')
    continue
  }
  seen.add(key)
  cases.push(kase)
}
stats.kept = cases.length
fs.writeFileSync(new URL('../tests/corpus/tailwind-tests.json', import.meta.url), JSON.stringify(cases))
console.log(JSON.stringify(stats))
process.exit(0)
