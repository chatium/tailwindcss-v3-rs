// Synthetic coverage corpus: every class tailwind's own getClassList() knows (default config),
// chunked, plus every variant applied to a sample of utilities, plus arbitrary values.
import fs from 'node:fs'
import { createRequire } from 'node:module'
const require = createRequire(import.meta.url)
const resolveConfig = require('tailwindcss/resolveConfig')
const { createContext } = require('tailwindcss/lib/lib/setupContextUtils')

const ctx = createContext(resolveConfig({ content: ['x'] }))
const classes = ctx.getClassList()
const variants = ctx.getVariants()
const cases = []
const chunk = 250
for (let i = 0; i < classes.length; i += chunk) {
  cases.push({ name: `classes-${i}`, config: {}, blocks: '', candidates: classes.slice(i, i + chunk) })
}
const sample = ['flex', 'p-4', 'text-red-500', 'bg-blue-500/50', 'underline', 'shadow-lg', 'ring-2', 'translate-x-1', 'w-[10px]', 'border', 'opacity-50', 'blur', 'space-x-2', 'divide-y']
for (const v of variants) {
  const names = v.isArbitrary ? [v.name + (v.hasDash ? '-' : '') + '[.foo]', ...v.values.slice(0, 4).map(x => v.name + (v.hasDash ? '-' : '') + x)] : [v.name]
  const cands = []
  for (const n of names) for (const s of sample) cands.push(`${n}:${s}`)
  cases.push({ name: `variant-${v.name}`, config: {}, blocks: '', candidates: cands })
}
fs.writeFileSync('../tests/corpus/classlist.json', JSON.stringify(cases))
console.log(classes.length, 'classes,', variants.length, 'variants,', cases.length, 'cases')
