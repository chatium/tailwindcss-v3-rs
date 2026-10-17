// Generates the static data the Rust port embeds:
//   src/tw/data/default_config.json — stubs/config.full.js; theme functions are recorded with
//     sentinels (see `config.rs`): `{"$theme#N": [path, default?]}` for theme() calls (spread or
//     used as a value), `{"$breakpoints#N": path}` for breakpoints(theme(path)).
//   src/tw/data/preflight.css — the (minified) preflight string inlined in the Play bundle.
//   src/tw/data/core_plugin_list.json, color_names.json
import fs from 'node:fs'
import { createRequire } from 'node:module'
const require = createRequire(import.meta.url)

const out = new URL('../src/tw/data/', import.meta.url)
fs.mkdirSync(out, { recursive: true })

const config = require('tailwindcss/stubs/config.full.js')
const colors = require('tailwindcss/colors')

let counter = 0
function themeSentinel(path, def) {
  const args = def === undefined ? [path] : [path, def]
  return { [`$theme#${counter++}`]: args }
}
function recordFn(fn) {
  const utils = {
    theme: themeSentinel,
    colors,
    breakpoints(sentinel) {
      const [key] = Object.keys(sentinel)
      return { [`$breakpoints#${counter++}`]: sentinel[key][0] }
    },
    negative(sentinel) {
      const [key] = Object.keys(sentinel)
      return { [`$negative#${counter++}`]: sentinel[key][0] }
    },
  }
  const resolvePath = Object.assign((p, d) => themeSentinel(p, d), utils)
  return fn(resolvePath, utils)
}

const theme = {}
for (const [k, v] of Object.entries(config.theme)) {
  theme[k] = typeof v === 'function' ? { $fn: recordFn(v) } : v
}
const data = { ...config, theme }
fs.writeFileSync(new URL('default_config.json', out), JSON.stringify(data, null, 1))

// preflight as inlined in the bundle
const bundle = fs.readFileSync(new URL('./play.3.4.16.min.js', import.meta.url), 'utf8')
const start = bundle.indexOf('preflight:({addBase:r})=>{let e=ee.parse(`')
if (start < 0) throw new Error('preflight not found in bundle')
const from = bundle.indexOf('`', start) + 1
const to = bundle.indexOf('`', from)
const preflight = bundle.slice(from, to)
if (preflight.includes('${')) throw new Error('unexpected interpolation in preflight')
fs.writeFileSync(new URL('preflight.css', out), preflight)

const corePluginList = require('tailwindcss/lib/corePluginList').default
fs.writeFileSync(new URL('core_plugin_list.json', out), JSON.stringify(corePluginList))
const colorNames = require('tailwindcss/lib/util/colorNames').default
fs.writeFileSync(new URL('color_names.json', out), JSON.stringify(colorNames))

// sanity: the bundle's core plugin order must match the package's
const m = bundle.match(/vf=\[("preflight"[^\]]*)\]/)
if (JSON.stringify(JSON.parse('[' + m[1] + ']')) !== JSON.stringify(corePluginList)) {
  throw new Error('core plugin list differs from the bundle')
}
console.log('ok', preflight.length, corePluginList.length)
