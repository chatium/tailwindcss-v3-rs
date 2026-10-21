// Unit-level differential fixtures for the ported building blocks (postcss, selector parser, …).
import fs from 'node:fs'
import { createRequire } from 'node:module'
const require = createRequire(import.meta.url)
const tw = p => require.resolve(p, { paths: [require.resolve('tailwindcss/package.json')] })
const sp = require(tw('postcss-selector-parser'))
const postcss = require('postcss')

fs.mkdirSync('../tests/fixtures', { recursive: true })

// ---- selector corpus: selectors from tailwind's own test expectations
const selectors = new Set()
for (const f of fs.readdirSync('upstream/tests')) {
  if (!f.endsWith('.js')) continue
  const src = fs.readFileSync('upstream/tests/' + f, 'utf8')
  for (const m of src.matchAll(/([^{};`]{1,300})\{/g)) {
    const s = m[1].trim()
    if (s && !s.startsWith('@') && !s.includes('\n\n')) selectors.add(s.replace(/\s+/g, ' '))
  }
}
for (const s of [
  '.a', 'a b', 'a > b', 'a  >  b', '.a:hover::before', ':is(.a, .b) .c', '[data-x="y"]', "[data-x='y' i]",
  '[a=b]', '[ a = b ]', '[a|b]', '*|a', 'ns|a', '.a\\:b', '.\\31 0', '.w-\\[10px\\]', '#id.cls', 'a,b', 'a,',
  '.a /* c */ .b', '.a /*c*/', ':not(.a,.b)', '::-webkit-scrollbar', '&:hover', '& > *', '.a:merge(.b) &',
  '.group:hover .group-hover\\:x', ':where([dir="rtl"], [dir="rtl"] *)', 'a  ', '  a', 'a ~ b + c',
  '.a.b.c', 'a[b]c', '.a(b)', 'a:nth-child(2n+1)', '.a\\,b', '.x\\2c y', '@media', '[', ']', '(', 'a)', 'a;b',
  '!a', '.a!', ':', '::', '.', '#', 'a|', '|a', '*|*', '.a/*', "'x'", '"x', '[x="y]', '.a\\', 'a /x/ b',
  '50.5%', '.é', '.\\E9', '[data-a=b c]', '[a=\\"b]', '.a\\ b', 'a\tb', 'a\nb', '.a  .b', '.a > .b ~ .c',
])
  selectors.add(s)

const selOut = []
for (const input of selectors) {
  let r
  try {
    const root = sp().astSync(input)
    const classes = []
    root.walkClasses(c => { classes.push(c.value) })
    const types = []
    root.walk(n => { types.push(n.type + ':' + (n.value ?? '')) })
    r = { input, out: root.toString(), classes, types }
  } catch (e) {
    r = { input, error: e.message }
  }
  selOut.push(r)
}
fs.writeFileSync('../tests/fixtures/selector.json', JSON.stringify(selOut))
console.log('selector', selOut.length)

// ---- postcss round trip + raw inference
const cssInputs = []
for (const f of fs.readdirSync('upstream/tests')) {
  if (!f.endsWith('.js')) continue
  const src = fs.readFileSync('upstream/tests/' + f, 'utf8')
  for (const m of src.matchAll(/css`([^`]*)`/g)) cssInputs.push(m[1])
}
const pcOut = []
for (const input of cssInputs) {
  try {
    const root = postcss.parse(input)
    const rule = postcss.rule({ selector: '.n' })
    rule.append(postcss.decl({ prop: 'p', value: 'v' }))
    root.append(rule)
    const at = postcss.atRule({ name: 'media', params: 'x' })
    at.append(postcss.rule({ selector: '.m' }))
    root.prepend(at)
    pcOut.push({ input, out: root.toString() })
  } catch (e) {
    pcOut.push({ input, error: e.message })
  }
}
fs.writeFileSync('../tests/fixtures/postcss.json', JSON.stringify(pcOut))
console.log('postcss', pcOut.length)

// ---- resolveConfig
const resolveConfig = require('tailwindcss/resolveConfig')
const configs = [
  {},
  { theme: { extend: { colors: { brand: '#123456', primary: { 50: '#eef', DEFAULT: '#00f' } }, spacing: { 128: '32rem' } } } },
  { theme: { colors: { red: '#f00' }, extend: { fontFamily: { sans: ['Inter', 'sans-serif'] } } } },
  { theme: { screens: { tablet: '640px', laptop: '1024px' }, extend: { maxWidth: { x: '1px' } } } },
  { prefix: 'tw-', important: true, darkMode: 'class', corePlugins: { preflight: false } },
  { corePlugins: ['margin', 'padding'] },
  { presets: [], theme: { spacing: { 1: '1px' } } },
  { theme: { extend: { boxShadow: { x: ['0 0 1px red', '0 0 2px blue'] }, fontSize: { xl: ['2rem', { lineHeight: '1' }] } } } },
  { future: { respectDefaultRingColorOpacity: true }, theme: { extend: { borderColor: { DEFAULT: '#ccc' } } } },
]
const rcOut = configs.map(c => {
  const r = resolveConfig({ ...c })
  return { config: c, resolved: JSON.parse(JSON.stringify({ theme: r.theme, corePlugins: r.corePlugins, prefix: r.prefix, important: r.important, separator: r.separator, darkMode: r.darkMode })) }
})
fs.writeFileSync('../tests/fixtures/resolve-config.json', JSON.stringify(rcOut))
console.log('resolve-config', rcOut.length)

// ---- tokenizer with ignoreErrors: the postcss corpus, cut right after its first quote, comment and `url(`
// (unclosed), and hand-written broken inputs
const tokenize = require('postcss/lib/tokenize')
const Input = require('postcss/lib/input')
const tokInputs = new Set()
for (const input of cssInputs) {
  tokInputs.add(input)
  for (const open of ['"', "'", '/*', 'url(']) {
    const at = input.indexOf(open)
    if (at >= 0) tokInputs.add(input.slice(0, at + open.length) + input.slice(at + open.length).replace(/["'*)]/g, ''))
  }
}
for (const s of [
  '"', "'", '/*', 'url(', 'url(x', 'url( x', "a { b: url('x", 'a { content: "x', "a { content: 'x\\' }", '"\\', "'é",
  '"é', 'a /* x', '/* @tailwind', '/* docs @tailwind */\na {color: red;', '@tailwind base; a { b: c',
  "a::after { content: '@tailwind utilities; }", '/* x */ @tailwind utilities; a { b: "c', 'a { b: (c',
])
  tokInputs.add(s)
const tokOut = []
for (const input of tokInputs) {
  const t = tokenize(new Input(input), { ignoreErrors: true })
  const tokens = []
  while (!t.endOfFile()) tokens.push(t.nextToken().slice(0, 2))
  tokOut.push({ input, tokens })
}
fs.writeFileSync('../tests/fixtures/tokenize.json', JSON.stringify(tokOut))
console.log('tokenize', tokOut.length)
