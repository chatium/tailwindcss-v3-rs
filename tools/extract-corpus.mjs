// Builds the production corpus from Chatium accounts (~/github/chatium/chatium.ru/*, chatium.ai/*):
//
//   - sets: every distinct (tailwind.config, <style type="text/tailwindcss"> blocks) pair. A pair is
//     collected per enclosing function (a page/head component), falling back to the file. Configs
//     and blocks are string contents (template literals) evaluated statically; interpolations are
//     resolved from file-level constants. Configs with functions/plugins and anything depending on
//     request data are skipped and counted.
//   - candidates: `extract_candidates` (the Rust port) over every non-media file of the account.
//
// Output (local only — account data is not committed): tests/corpus-local/accounts.json
//   { candidates: { [account]: string[] }, cases: [{ name, account, config, blocks }], stats }
//
//   node tools/extract-corpus.mjs [account-dir…]
import fs from 'node:fs'
import path from 'node:path'
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
const require = createRequire(import.meta.url)
const { parse } = require('@babel/parser')
const traverse = require('@babel/traverse').default

const ROOT = path.join(process.env.HOME, 'github/chatium')
const MEDIA =
  /\.(png|jpe?g|gif|webp|avif|ico|bmp|tiff?|svgz|mp4|webm|mov|avi|mkv|mp3|wav|ogg|m4a|flac|aac|woff2?|ttf|otf|eot|zip|gz|tgz|rar|7z|pdf|docx?|xlsx?|pptx?|wasm|bin|exe|dll|so|dylib|db|sqlite|lock)$/i

function listFiles(dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    if (e.name === 'node_modules' || e.name.startsWith('.')) continue
    const p = path.join(dir, e.name)
    if (e.isDirectory()) listFiles(p, out)
    else if (e.isFile() && !MEDIA.test(e.name)) out.push(p)
  }
  return out
}

class Dynamic extends Error {}
class HasFunction extends Error {}

// ---- static evaluation of a small JS subset

function evaluate(node, scope, depth = 0) {
  if (depth > 50) throw new Dynamic('depth')
  switch (node.type) {
    case 'StringLiteral':
    case 'NumericLiteral':
    case 'BooleanLiteral':
      return node.value
    case 'NullLiteral':
      return null
    case 'TemplateLiteral': {
      let s = ''
      node.quasis.forEach((q, i) => {
        s += q.value.cooked
        if (i < node.expressions.length) s += String(evaluate(node.expressions[i], scope, depth + 1))
      })
      return s
    }
    case 'TSAsExpression':
    case 'TSSatisfiesExpression':
    case 'TSNonNullExpression':
    case 'ParenthesizedExpression':
      return evaluate(node.expression, scope, depth + 1)
    case 'UnaryExpression':
      if (node.operator === '-') return -evaluate(node.argument, scope, depth + 1)
      if (node.operator === '+') return +evaluate(node.argument, scope, depth + 1)
      if (node.operator === '!') return !evaluate(node.argument, scope, depth + 1)
      throw new Dynamic(node.operator)
    case 'BinaryExpression':
      if (node.operator === '+') return evaluate(node.left, scope, depth + 1) + evaluate(node.right, scope, depth + 1)
      throw new Dynamic(node.operator)
    case 'ArrayExpression':
      return node.elements.flatMap(e => {
        if (!e) return [undefined]
        if (e.type === 'SpreadElement') return evaluate(e.argument, scope, depth + 1)
        return [evaluate(e, scope, depth + 1)]
      })
    case 'ObjectExpression': {
      const o = {}
      for (const p of node.properties) {
        if (p.type === 'SpreadElement') {
          Object.assign(o, evaluate(p.argument, scope, depth + 1))
          continue
        }
        if (p.type === 'ObjectMethod') throw new HasFunction('method')
        let key
        if (p.computed) key = evaluate(p.key, scope, depth + 1)
        else key = p.key.type === 'Identifier' ? p.key.name : p.key.value
        if (p.value.type === 'ArrowFunctionExpression' || p.value.type === 'FunctionExpression') throw new HasFunction(String(key))
        o[key] = evaluate(p.value, scope, depth + 1)
      }
      return o
    }
    case 'Identifier': {
      if (node.name === 'undefined') return undefined
      const b = scope?.getBinding(node.name)
      if (!b || b.kind === 'param' || b.kind === 'module') throw new Dynamic(node.name)
      if (!b.constant && b.kind !== 'const') throw new Dynamic(node.name)
      const decl = b.path.node
      if (decl.type !== 'VariableDeclarator' || !decl.init) throw new Dynamic(node.name)
      return evaluate(decl.init, b.path.scope, depth + 1)
    }
    case 'MemberExpression': {
      const obj = evaluate(node.object, scope, depth + 1)
      const key = node.computed ? evaluate(node.property, scope, depth + 1) : node.property.name
      if (obj == null) throw new Dynamic('member of null')
      return obj[key]
    }
    case 'CallExpression': {
      const c = node.callee
      if (c.type === 'MemberExpression' && c.object.name === 'JSON' && c.property.name === 'stringify') {
        const args = node.arguments.map(a => evaluate(a, scope, depth + 1))
        return JSON.stringify(...args)
      }
      if (c.type === 'MemberExpression' && !c.computed && ['join', 'trim', 'replace', 'map'].includes(c.property.name)) {
        const obj = evaluate(c.object, scope, depth + 1)
        if (c.property.name === 'join') return obj.join(...node.arguments.map(a => evaluate(a, scope, depth + 1)))
        if (c.property.name === 'trim') return obj.trim()
      }
      throw new Dynamic('call')
    }
    case 'ConditionalExpression':
    case 'LogicalExpression':
    default:
      throw new Dynamic(node.type)
  }
}

// `tailwind.config = {...}` inside script text
function configsInScript(text) {
  const out = []
  if (!/tailwind\s*\.\s*config\s*=/.test(text)) return out
  let ast
  try {
    ast = parse(text, { sourceType: 'script', plugins: ['typescript'], errorRecovery: true })
  } catch {
    return [{ error: 'parse' }]
  }
  traverse(ast, {
    AssignmentExpression(p) {
      const l = p.node.left
      if (
        l.type === 'MemberExpression' &&
        !l.computed &&
        l.property.name === 'config' &&
        l.object.type === 'Identifier' &&
        l.object.name === 'tailwind'
      ) {
        try {
          out.push({ config: evaluate(p.node.right, p.scope) })
        } catch (e) {
          out.push({ error: e instanceof HasFunction ? 'function' : 'dynamic' })
        }
      }
    },
  })
  return out
}

function jsxAttr(el, name) {
  for (const a of el.openingElement.attributes) {
    if (a.type === 'JSXAttribute' && a.name.name === name) return a.value
  }
  return undefined
}

function jsxText(el, scope) {
  // children: {`...`} or text; or dangerouslySetInnerHTML={{ __html: ... }}
  const html = jsxAttr(el, 'dangerouslySetInnerHTML')
  if (html?.type === 'JSXExpressionContainer' && html.expression.type === 'ObjectExpression') {
    const prop = html.expression.properties.find(p => p.key?.name === '__html')
    if (prop) return evaluate(prop.value, scope)
  }
  let s = ''
  for (const c of el.children) {
    if (c.type === 'JSXText') s += c.value
    else if (c.type === 'JSXExpressionContainer') {
      if (c.expression.type === 'JSXEmptyExpression') continue
      s += String(evaluate(c.expression, scope))
    } else throw new Dynamic('jsx child')
  }
  return s
}

function scopeKey(p) {
  const f = p.getFunctionParent()
  return f ? `${f.node.start}` : 'file'
}

export function analyzeFile(file, stats) {
  const text = fs.readFileSync(file, 'utf8')
  if (!text.includes('tailwind') ) return []
  let ast
  try {
    ast = parse(text, { sourceType: 'module', plugins: ['typescript', 'jsx'], errorRecovery: true })
  } catch {
    stats.parseErrors++
    return []
  }
  const groups = new Map() // scope → { configs: [], blocks: [], skipped: [] }
  const group = p => {
    const k = scopeKey(p)
    if (!groups.has(k)) groups.set(k, { configs: [], blocks: [], skipped: [] })
    return groups.get(k)
  }
  traverse(ast, {
    JSXElement(p) {
      const name = p.node.openingElement.name
      if (name.type !== 'JSXIdentifier') return
      if (name.name === 'style') {
        const t = jsxAttr(p.node, 'type')
        if (t?.type === 'StringLiteral' && t.value === 'text/tailwindcss') {
          try {
            group(p).blocks.push(jsxText(p.node, p.scope))
          } catch {
            group(p).skipped.push('dynamic-block')
          }
        }
      } else if (name.name === 'script') {
        let s
        try {
          s = jsxText(p.node, p.scope)
        } catch {
          if (/tailwind\s*\.\s*config/.test(text.slice(p.node.start, p.node.end))) group(p).skipped.push('dynamic-config')
          return
        }
        for (const c of configsInScript(s)) {
          if (c.error) group(p).skipped.push(c.error === 'function' ? 'function-config' : 'dynamic-config')
          else group(p).configs.push(c.config)
        }
      }
    },
    // configs/blocks written into HTML strings (`<script>tailwind.config = …</script>` in a template)
    TemplateLiteral(p) {
      if (p.parent.type === 'JSXExpressionContainer') return
      let s
      try {
        s = evaluate(p.node, p.scope)
      } catch {
        const raw = text.slice(p.node.start, p.node.end)
        if (/tailwind\s*\.\s*config\s*=/.test(raw)) group(p).skipped.push('dynamic-config')
        if (raw.includes('text/tailwindcss')) group(p).skipped.push('dynamic-block')
        return
      }
      for (const m of s.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/g)) {
        for (const c of configsInScript(m[1])) {
          if (c.error) group(p).skipped.push(c.error === 'function' ? 'function-config' : 'dynamic-config')
          else group(p).configs.push(c.config)
        }
      }
      for (const m of s.matchAll(/<style[^>]*type=["']text\/tailwindcss["'][^>]*>([\s\S]*?)<\/style>/g)) {
        group(p).blocks.push(m[1])
      }
    },
  })
  const sets = []
  for (const g of groups.values()) {
    for (const s of g.skipped) stats.skipped[s] = (stats.skipped[s] ?? 0) + 1
    if (!g.configs.length && !g.blocks.length) continue
    const configs = g.configs.length ? g.configs : [{}]
    for (const config of configs) {
      if (Array.isArray(config.plugins) && config.plugins.length) {
        stats.skipped['plugins'] = (stats.skipped['plugins'] ?? 0) + 1
        continue
      }
      sets.push({ config, blocks: g.blocks.join('') })
    }
  }
  return sets
}

if (process.argv[1] === new URL(import.meta.url).pathname) {
  let accounts = process.argv.slice(2)
  if (!accounts.length) {
    for (const top of ['chatium.ru', 'chatium.ai']) {
      for (const d of fs.readdirSync(path.join(ROOT, top))) {
        const p = path.join(ROOT, top, d)
        if (fs.statSync(p).isDirectory()) accounts.push(p)
      }
    }
  }
  const out = { candidates: {}, cases: [], stats: { accounts: 0, files: 0, parseErrors: 0, skipped: {}, sets: 0, distinct: 0 } }
  const cargoRoot = new URL('..', import.meta.url).pathname
  for (const acc of accounts) {
    const name = path.relative(ROOT, acc)
    const files = listFiles(acc)
    out.stats.files += files.length
    const seen = new Set()
    const sets = []
    for (const f of files) {
      if (!/\.(tsx?|jsx?|mjs|vue|html)$/.test(f)) continue
      for (const s of analyzeFile(f, out.stats)) {
        out.stats.sets++
        const key = JSON.stringify(s)
        if (seen.has(key)) continue
        seen.add(key)
        sets.push({ ...s, file: path.relative(acc, f) })
      }
    }
    if (!sets.length) continue
    out.stats.accounts++
    out.stats.distinct += sets.length
    const extracted = JSON.parse(
      execFileSync('cargo', ['run', '-q', '--release', '--example', 'extract'], { cwd: cargoRoot, input: files.join('\n'), maxBuffer: 1 << 30, stdio: ['pipe', 'pipe', 'ignore'] }).toString()
    )
    const cands = new Set()
    for (const list of Object.values(extracted)) for (const c of list) cands.add(c)
    out.candidates[name] = [...cands]
    sets.forEach((s, i) => out.cases.push({ name: `${name}#${i} ${s.file}`, account: name, config: s.config, blocks: s.blocks }))
    console.log(`${name}: ${sets.length} sets, ${cands.size} candidates`)
  }
  fs.mkdirSync(new URL('../tests/corpus-local/', import.meta.url), { recursive: true })
  fs.writeFileSync(new URL('../tests/corpus-local/accounts.json', import.meta.url), JSON.stringify(out))
  console.log(JSON.stringify(out.stats))
}
