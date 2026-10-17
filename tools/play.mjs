// Runs the Tailwind Play CDN bundle (3.4.16) in node:vm with a minimal DOM stub.
// This is the reference: whatever CSS the bundle puts into <head> is the expected output.
import fs from 'node:fs'
import vm from 'node:vm'

const script = new vm.Script(
  fs.readFileSync(new URL('./play.3.4.16.min.js', import.meta.url), 'utf8'),
  { filename: 'play.js' },
)

/**
 * @param {{config?: object, blocks?: string, candidates?: string[]}} input
 * @returns {Promise<{css: string} | {error: object}>}
 */
export async function runPlay({ config = {}, blocks = '', candidates = [] }) {
  let observer
  let style = null
  const styleBlocks = blocks === '' ? [] : [{ textContent: blocks }]
  const classEl = { classList: candidates }
  const document = {
    documentElement: {},
    body: null,
    head: { append: el => (style = el) },
    createElement: () => ({ isConnected: true, textContent: '' }),
    querySelectorAll: sel =>
      sel.startsWith('style') ? styleBlocks : sel === '[class]' ? [classEl] : [],
  }
  class MutationObserver {
    constructor(cb) {
      if (!observer) observer = cb
    }
    observe() {}
  }
  const ctx = {
    document,
    MutationObserver,
    console: { ...console, warn() {} },
    setTimeout,
    clearTimeout,
    queueMicrotask,
    URL,
    TextEncoder,
    TextDecoder,
  }
  ctx.window = ctx.self = ctx.globalThis = ctx
  vm.createContext(ctx)
  script.runInContext(ctx)
  ctx.tailwind.config = config
  document.body = {}
  try {
    await observer([])
  } catch (e) {
    return {
      error: {
        name: e.name,
        message: e.message,
        reason: e.reason ?? null,
        line: e.line ?? null,
        column: e.column ?? null,
        endLine: e.endLine ?? null,
        endColumn: e.endColumn ?? null,
      },
    }
  }
  return { css: style.textContent }
}

if (process.argv[1] === new URL(import.meta.url).pathname) {
  const r = await runPlay({
    config: { theme: { extend: { colors: { brand: '#123456' } } } },
    blocks: '.btn{@apply px-4 bg-brand}',
    candidates: ['flex', 'md:px-2', 'text-brand/50', 'w-[10px]', 'foo', 'backdrop-blur', 'select-none'],
  })
  console.log(r.css ?? r.error)
}
