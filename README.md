# tailwindcss-v3-rs

Rust-порт компилятора Tailwind CSS **v3.4.19**: `postcss([tailwindcss(config), autoprefixer({ remove: false })])`
над таблицей стилей с директивами `@tailwind`. Цель — побайтно тот же CSS, что дают npm-пакеты, без
Node и JS-движка в рантайме. autoprefixer работает на данных Play-бандла (`cdn.tailwindcss.com`):
его таблица префиксов и 14 целевых браузеров.

Потребитель — сборщик Chatium (`chatium-ugc-compiler`): автор пишет входной `.css` с
`@config` и `@tailwind`, сборщик резолвит конфиг, собирает кандидатов из графа импортов и
генерирует CSS при сборке.

## API

```toml
[dependencies]
tailwindcss-v3 = { git = "https://github.com/chatium/tailwindcss-v3-rs", rev = "…" }
```

```rust
// конфиг, на который указывает `@config`, как JSON (serde_json с preserve_order; целочисленные
// ключи переставляются по правилам JS внутри from_json)
let config = tailwindcss_v3::Config::from_json(serde_json::json!({
    "theme": { "extend": { "colors": { "brand": "#123456" } } }
}))?;

// кандидаты из любых исходников (tsx, ts, vue, html, …) — порт default extractor v3
let candidates = tailwindcss_v3::extract_candidates(source);

// таблица стилей → tailwind → autoprefixer
let css = tailwindcss_v3::generate(&config, r#"
@import url(https://fonts.googleapis.com/css2?family=Inter);
@config "./tailwind.config.json";
@tailwind base;
@tailwind components;
@tailwind utilities;
@layer components { .btn { @apply px-4 py-2 bg-brand; } }
"#, candidates)?;
```

- `generate(&config, css, candidates)` обрабатывает `css` как обычный вход v3: директивы
  `@tailwind` раскрываются там, где написаны, допустим любой их поднабор (только
  `@tailwind utilities` — тоже); `@import url()` и прочие at-rule (`@source`, `@theme`, `@plugin`)
  остаются как написаны.
- `@config` удаляется, как в v3 (конфиг передаётся аргументом); ошибки, не требующие файловой
  системы, те же: второй `@config`, `@config` без пути в кавычках, абсолютный путь.
- `candidates` — классы, найденные в контенте: порядок и дубли не важны, лишние строки молча
  игнорируются. Кандидаты, на которых сам JS-tailwind падает с исключением (например,
  `[&{]:flex`), отбрасываются: в JS такой класс в контенте ломал бы генерацию целиком.
- `Config: Send + Sync + Clone`; `Config::extract_candidates` — тот же extractor с
  `prefix`/`separator` конфига.
- Ошибки (`Error`) повторяют tailwind/postcss: `name` (`CssSyntaxError`/`Error`), `reason`,
  `message` ровно как в JS для таблицы без `from` (`<css input>:3:6: The \`foo\` class does not
  exist…`), позиция `line`/`column` (1-based, колонки в UTF-16) в `css`.
- Модуль `postcss` публичный: `postcss::parse(&mut css, input)` и обход узлов — чтобы читать
  `@tailwind`/`@config`/`@import` и их позиции; токенизатор `postcss::tokenize::Tokenizer`, в том
  числе `Tokenizer::ignoring_errors` (`tokenizer(input, { ignoreErrors: true })`: незакрытые строка,
  комментарий и `url(` — токены, а не ошибка) — чтобы читать таблицу, которую парсер не принял.

## Устройство

Порт — транслитерация, а не редизайн: порядок обходов, мутации деревьев во время обхода и
квирки JS воспроизводятся, потому что вывод должен совпасть побайтно.

| Модуль | Что портировано |
| --- | --- |
| `js` | значения JS: порядок ключей объекта, `Number#toString`, `Number()`, `parseFloat`, `Array#sort` V8 |
| `postcss` | `postcss@8.4.49`: узлы в арене, итераторы `each`/`walk` с индексами, токенизатор, парсер, стрингифаер с выводом raws и `rawCache` |
| `selector` | `postcss-selector-parser@6.1.2` с алиасингом узлов как в JS, `cssesc` |
| `value_parser` | форк postcss-value-parser из tailwind и оригинальный 4.2.0 (для autoprefixer) |
| `tw::config` | `config.full.js`, `resolveConfig`, `normalizeConfig`, `featureFlags`; функции темы — записанные шаблоны |
| `tw::context`, `tw::core_plugins` | plugin API (`addUtilities`, `matchUtilities`, `addVariant`, `matchVariant`, …) и все core-плагины и варианты |
| `tw::generate`, `tw::offsets`, `tw::format_variant_selector` | `generateRules`, сортировка `Offsets` (bigint-битовые маски), форматирование селекторов вариантов |
| `tw::apply`, `tw::functions`, `tw::process` | `@config`, `@apply`, `theme()`/`screen()`, `@layer`, `@screen`, defaults, collapse-проходы |
| `autoprefixer` | `autoprefixer@10.4` на таблицах Play-бандла (37 признаков, 14 браузеров), `transition`, `@supports`, управляющие комментарии |
| `extractor`, `jsregex` | default extractor v3: сканеры для типичных строк и собственный backtracking-движок регулярок с семантикой JS (UTF-16, lookahead, обратные ссылки) для остальных |

Данные `src/tw/data/*` берутся из npm-пакета скриптом `tools/gen-data.mjs` (preflight —
`lib/css/preflight.css` как есть); `src/autoprefixer_data.json` извлечён из Play-бандла 3.4.16.

## Проверка

Дифференциальная: эталон — `tools/ref.mjs`, то есть `tailwindcss@3.4.19` + `autoprefixer@10.4.20`
под `postcss@8.4.49` в Node. Таблица префиксов и браузеры autoprefixer подменены данными бандла,
кандидаты передаются как raw-контент с экстрактором `split(' ')`, у каждого случая свежий контекст
tailwind. Зависимости зафиксированы на версиях, которые воспроизводит порт (selector-parser 6.1.2,
postcss-js 4.0.1, postcss-nested 6.2.0 — все в диапазонах 3.4.19). `tools/gen-fixtures.mjs`
пишет ожидаемый CSS в `tests/fixtures/`, `cargo test` прогоняет тот же корпус через порт и
сравнивает побайтно.

| Корпус | Откуда | Случаев | Совпало |
| --- | --- | ---: | ---: |
| `basics` | ручные случаи: `@apply`, ошибки, слои, префикс, important, dark, container, произвольные значения; подмножества директив, CSS до директив, `@import` первым, `@config`, неизвестные at-rule | 26 | 26 |
| `classlist` | все 11 344 класса из `getClassList()` и все 71 вариант × выборка утилит | 117 | 117 |
| `tailwind-tests` | входы из `tests/*.test.js` tailwindcss v3.4.19 как есть (`tools/extract-tw-tests.mjs`) | 362 | 362 |
| `regressions` | минимальные случаи найденных багов и изменений 3.4.17–3.4.19 | 3 | 3 |
| боевой (локально) | 22 аккаунта Chatium, см. ниже | 823 | 823 |

Плюс модульные дифференциальные тесты (`tests/units.rs`): селектор-парсер (1940 селекторов),
круговой разбор postcss (923 входа), токенизатор с `ignoreErrors` (769 входов, 115 из них с
незакрытыми строками, комментариями и `url(`), `resolveConfig`, фаззинг сканеров экстрактора против
движка регулярок.

### Боевой корпус

`tools/extract-corpus.mjs` собирает из `~/github/chatium/chatium.ru/*` и `chatium.ai/*` прежние
Play-настройки (`tailwind.config = {…}` и `<style type="text/tailwindcss">`); вход —
`@tailwind base;@tailwind components;@tailwind utilities;` + блоки настройки, кандидаты —
`extract_candidates` по всем немедийным файлам аккаунта. Данные аккаунтов в репозиторий не
попадают: корпус и эталоны лежат в `tests/corpus-local/` и `tests/fixtures-local/` (в
`.gitignore`), тест `conformance_local` пропускается, если их нет.

```sh
cd tools && npm ci
node extract-corpus.mjs            # → tests/corpus-local/accounts.json
node gen-fixtures-local.mjs        # эталоны параллельно → tests/fixtures-local/accounts/
cargo test --release --test conformance conformance_local
```

`tools/check-extractor.mjs <dir>…` сверяет `extract_candidates` с extractor из npm пофайлово:
на всех аккаунтах — 74 483 из 74 483 файлов (881 МБ) совпали.

## Совместимость

| Возможность | Статус |
| --- | --- |
| Все core-плагины и варианты v3.4.19, произвольные значения/свойства/варианты, модификаторы | ✅ |
| `@tailwind` (любой поднабор, в любом месте), `@layer`, `@apply` (вкл. `!important`, вложенные `@apply`, циклы), `theme()`, `screen()`, `@screen`, `@config` | ✅ |
| `prefix`, `important` (bool/селектор), `separator`, `darkMode` (`media`/`class`/`selector`/`variant`), `corePlugins`, `safelist` (строки), `blocklist`, `presets`, `future`/`experimental` | ✅ |
| autoprefixer с данными Play-бандла, включая каскадные отступы и `@supports` | ✅ |
| Ошибки: синтаксис CSS, `@apply` неизвестного класса, неверный `theme()`-путь, `@screen`, `@config` | ✅ (текст и позиция как в JS) |
| Функции и плагины в конфиге, `safelist` с регулярками, ключ `config` | ❌ конфиг — только JSON |
| `@config`: проверка, что файл существует | ❌ конфиг резолвит вызывающий |
| Классы-ключи прототипа `Object` (`text-constructor`, `bg-toString`…) | ❌ в JS дают мусорный CSS или падение; порт их игнорирует |
| autoprefixer: `/* autoprefixer grid: … */` | ❌ сетки в данных бандла нет, управление гридом не портировано |
| postcss 8.5 / selector-parser 6.1.4 (свежая установка npm) | ❌ порт следует postcss 8.4.49 и selector-parser 6.1.2 |

### Расхождения по построению

- Кандидат, на котором JS-tailwind бросает исключение (кроме `CssSyntaxError`), порт отбрасывает,
  а не падает. Генератор эталонов находит такие кандидаты бисекцией и исключает их из запуска.
- `extract_candidates` возвращает `&str`, поэтому кандидат, который в JS разрезал бы суррогатную
  пару UTF-16, представить нельзя — он пропускается.
- Незакрытая строка с `ignoreErrors` — кавычка и следующий символ; если он вне BMP, JS берёт половину
  суррогатной пары, порт — символ целиком.

## Производительность

Apple M1 Max, `--release`, один поток.

- `generate`: сотни–тысячи кандидатов — единицы–десятки миллисекунд; на аккаунте с ~1 млн
  кандидатов — ~1,4 с.
- `extract_candidates`: 83 МБ исходников dudinkonstantin — 5,6 с; результаты по строкам
  кэшируются в потоке.

## Лицензия

MIT (см. `LICENSE`); включает портированный код Tailwind CSS, PostCSS, Autoprefixer,
postcss-selector-parser и postcss-value-parser (все MIT).
