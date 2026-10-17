# tailwindcss-v3-rs

Rust-порт компилятора Tailwind CSS **v3.4.16** в том виде, в каком его запускает Play-бандл
(`cdn.tailwindcss.com` / `tailwind.3.4.16.min.js`): `postcss([tailwindcss(config), autoprefixer({ remove: false })])`
с зашитым в бандл списком браузеров. Цель — побайтно тот же CSS, что кладёт в `<head>` Play-скрипт,
без Node и JS-движка в рантайме.

Потребитель — сборщик Chatium (`chatium-ugc-compiler`): он генерирует CSS страницы при сборке и
отдаёт его `<link>`-ом вместо Play-скрипта.

## API

```toml
[dependencies]
tailwindcss-v3 = { git = "https://github.com/chatium/tailwindcss-v3-rs", rev = "…" }
```

```rust
// объект `tailwind.config` как JSON (serde_json с preserve_order; целочисленные ключи
// переставляются по правилам JS внутри from_json)
let config = tailwindcss_v3::Config::from_json(serde_json::json!({
    "theme": { "extend": { "colors": { "brand": "#123456" } } }
}))?;

// кандидаты из любых исходников (tsx, ts, vue, html, …) — порт default extractor v3
let candidates = tailwindcss_v3::extract_candidates(source);

// `@tailwind base;@tailwind components;@tailwind utilities;` + blocks → tailwind → autoprefixer
let css = tailwindcss_v3::generate(&config, blocks, candidates)?;
```

- `Config: Send + Sync + Clone`; один конфиг можно использовать из многих потоков.
- `generate` принимает кандидатов как набор классов из DOM: порядок и дубли не важны, лишние
  строки молча игнорируются. Кандидаты, на которых сам JS-tailwind падает с исключением
  (например, `[&{]:flex`), отбрасываются: в бандле такой класс в DOM ломал бы генерацию целиком.
- `Config::extract_candidates` — тот же extractor с `prefix`/`separator` конфига.
- Ошибки (`Error`) повторяют tailwind/postcss: `name` (`CssSyntaxError`/`Error`), `reason`,
  `message` ровно как в JS (`<css input>:1:62: The \`foo\` class does not exist…`), позиция
  `line`/`column` во входе целиком; `position_in_blocks()` переводит её в координаты `blocks`.

## Устройство

Порт — транслитерация, а не редизайн: порядок обходов, мутации деревьев во время обхода и
квирки JS воспроизводятся, потому что вывод должен совпасть побайтно.

| Модуль | Что портировано |
| --- | --- |
| `js` | значения JS: порядок ключей объекта, `Number#toString`, `Number()`, `parseFloat`, `Array#sort` V8 |
| `postcss` | `postcss@8.4.49`: узлы в арене, итераторы `each`/`walk` с индексами, токенизатор, парсер, стрингифаер с выводом raws и `rawCache` |
| `selector` | `postcss-selector-parser@6.1.2` (версия из бандла) с алиасингом узлов как в JS, `cssesc` |
| `value_parser` | форк postcss-value-parser из tailwind и оригинальный 4.2.0 (для autoprefixer) |
| `tw::config` | `config.full.js`, `resolveConfig`, `normalizeConfig`, `featureFlags`; функции темы — записанные шаблоны |
| `tw::context`, `tw::core_plugins` | plugin API (`addUtilities`, `matchUtilities`, `addVariant`, `matchVariant`, …) и все core-плагины и варианты |
| `tw::generate`, `tw::offsets`, `tw::format_variant_selector` | `generateRules`, сортировка `Offsets` (bigint-битовые маски), форматирование селекторов вариантов |
| `tw::apply`, `tw::functions`, `tw::process` | `@apply`, `theme()`/`screen()`, `@layer`, `@screen`, defaults, collapse-проходы |
| `autoprefixer` | `autoprefixer@10.4` на зашитых в бандл таблицах (37 признаков, 14 браузеров), `transition`, `@supports`, управляющие комментарии |
| `extractor`, `jsregex` | default extractor v3 на собственном backtracking-движке регулярок с семантикой JS (UTF-16, lookahead, обратные ссылки) |

Данные (`src/tw/data/*`, `src/autoprefixer_data.json`) извлекаются из бандла и npm-пакета
скриптом `tools/gen-data.mjs`: preflight — ровно та минифицированная строка, что в бандле.

## Проверка

Дифференциальная: эталон — сам Play-бандл в `node:vm` с заглушкой DOM (`tools/play.mjs`).
`tools/gen-fixtures.mjs` прогоняет его по корпусу и пишет ожидаемый CSS в `tests/fixtures/`,
`cargo test` прогоняет тот же корпус через порт и сравнивает побайтно.

| Корпус | Откуда | Случаев | Совпало |
| --- | --- | ---: | ---: |
| `basics` | ручные случаи: `@apply`, ошибки, слои, префикс, important, dark, container, произвольные значения | 14 | 14 |
| `classlist` | все 11 344 класса из `getClassList()` и все 71 вариант × выборка утилит | 117 | 117 |
| `tailwind-tests` | входы из `tests/*.test.js` tailwindcss v3.4.16 (`tools/extract-tw-tests.mjs`) | 364 | 364 |
| `regressions` | минимальные случаи найденных багов | 1 | 1 |
| боевой (локально) | 22 аккаунта Chatium, см. ниже | 823 | 823 |

Плюс модульные дифференциальные тесты (`tests/units.rs`): селектор-парсер (1940 селекторов),
круговой разбор postcss (923 входа), `resolveConfig`.

### Боевой корпус

`tools/extract-corpus.mjs` собирает из `~/github/chatium/chatium.ru/*` и `chatium.ai/*` каждый
различный набор «конфиг + блоки»: литералы `tailwind.config = {…}` и `<style type="text/tailwindcss">`
вычисляются статически (константы файла подставляются), пары берутся по объемлющей функции.
Кандидаты — `extract_candidates` по всем немедийным файлам аккаунта (без скрытых каталогов).
Данные аккаунтов в репозиторий не попадают: корпус и эталоны лежат в `tests/corpus-local/` и
`tests/fixtures-local/` (в `.gitignore`), тест `conformance_local` пропускается, если их нет.

```sh
cd tools && npm ci
node extract-corpus.mjs            # → tests/corpus-local/accounts.json
node gen-fixtures-local.mjs        # эталоны параллельно → tests/fixtures-local/accounts/
cargo test --release --test conformance conformance_local
```

Статистика текущего прогона: 57 085 файлов, 1 356 наборов, из них 823 различных; пропущено
29 конфигов и 60 блоков, зависящих от данных запроса (конфигов с функциями и плагинами в
аккаунтах нет). Кандидатов на аккаунт — от 12 тыс. до 1 млн. В 6 наборах ожидается ошибка
(неизвестный класс в `@apply`, синтаксис CSS) — порт выдаёт ту же. Кандидатов, на которых падал
бы JS, в корпусе нет.

`tools/check-extractor.mjs <dir>…` сверяет `extract_candidates` с extractor из npm
`tailwindcss@3.4.16` пофайлово: на всех аккаунтах — 74 483 из 74 483 файлов (881 МБ) совпали.

## Совместимость

| Возможность | Статус |
| --- | --- |
| Все core-плагины и варианты v3.4.16, произвольные значения/свойства/варианты, модификаторы | ✅ |
| `@tailwind`, `@layer`, `@apply` (вкл. `!important`, вложенные `@apply`, циклы), `theme()`, `screen()`, `@screen` | ✅ |
| `prefix`, `important` (bool/селектор), `separator`, `darkMode` (`media`/`class`/`selector`/`variant`), `corePlugins`, `safelist` (строки), `blocklist`, `presets`, `future`/`experimental` | ✅ |
| autoprefixer с данными бандла, включая каскадные отступы и `@supports` | ✅ |
| Ошибки: синтаксис CSS, `@apply` неизвестного класса, неверный `theme()`-путь, `@screen` | ✅ (текст и позиция как в JS) |
| Функции и плагины в конфиге, `safelist` с регулярками | ❌ конфиг — только JSON |
| Классы-ключи прототипа `Object` (`text-constructor`, `bg-toString`…) | ❌ в JS дают мусорный CSS или падение; порт их игнорирует |
| autoprefixer: `/* autoprefixer grid: … */` | ❌ сетки в данных бандла нет, управление гридом не портировано |

### Расхождения по построению

- Кандидат, на котором JS-tailwind бросает исключение (кроме `CssSyntaxError`), порт отбрасывает,
  а не падает. Генератор эталонов находит такие кандидаты бисекцией и исключает их из запуска бандла.
- `extract_candidates` возвращает `&str`, поэтому кандидат, который в JS разрезал бы суррогатную
  пару UTF-16, представить нельзя — он пропускается.

## Производительность

Один поток, Apple M-серии, `--release`:

- `generate` на аккаунте с ~1 млн кандидатов — ~1,4 с; ~300 тыс. — ~0,4 с; типичный набор
  (10–50 тыс.) — десятки миллисекунд.
- `extract_candidates`: 83 МБ исходников одного аккаунта — ~10,5 с (Node с npm-extractor — ~11 с);
  результаты по строкам кэшируются в потоке, повторяющиеся между файлами строки не пересчитываются.

## Лицензия

MIT (см. `LICENSE`); включает портированный код Tailwind CSS, PostCSS, Autoprefixer,
postcss-selector-parser и postcss-value-parser (все MIT).
