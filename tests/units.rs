//! Differential tests of the ported building blocks (see `tools/gen-unit-fixtures.mjs`).

use serde_json::Value;
use tailwindcss_v3::{postcss, selector};

fn load(name: &str) -> Vec<Value> {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn report(name: &str, total: usize, failed: Vec<String>) {
    if !failed.is_empty() {
        for f in failed.iter().take(std::env::var("SHOW").ok().and_then(|s| s.parse().ok()).unwrap_or(15)) {
            eprintln!("--- {f}");
        }
        panic!("{name}: {} of {total} failed", failed.len());
    }
}

#[test]
fn selector_parser() {
    let cases = load("selector");
    let mut failed = Vec::new();
    for c in &cases {
        let input = c["input"].as_str().unwrap();
        let mut sel = selector::Sel::new();
        match selector::parse(&mut sel, input) {
            Ok(root) => {
                let out = sel.to_string(root);
                let classes = sel.class_values(root);
                let mut types = Vec::new();
                sel.walk(root, &mut |s, n| {
                    let k = match s[n].kind {
                        selector::SKind::Root => "root",
                        selector::SKind::Selector => "selector",
                        selector::SKind::Class => "class",
                        selector::SKind::Tag => "tag",
                        selector::SKind::Id => "id",
                        selector::SKind::Attribute => "attribute",
                        selector::SKind::Pseudo => "pseudo",
                        selector::SKind::Combinator => "combinator",
                        selector::SKind::Comment => "comment",
                        selector::SKind::Str => "string",
                        selector::SKind::Nesting => "nesting",
                        selector::SKind::Universal => "universal",
                    };
                    types.push(format!("{k}:{}", s[n].value.clone().unwrap_or_default()));
                    selector::SFlow::Continue
                });
                if c.get("error").is_some() {
                    failed.push(format!("{input:?}: expected error {}", c["error"]));
                    continue;
                }
                if out != c["out"].as_str().unwrap() {
                    failed.push(format!("{input:?}: out {out:?} != {}", c["out"]));
                }
                let want: Vec<String> = c["classes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
                if classes != want {
                    failed.push(format!("{input:?}: classes {classes:?} != {want:?}"));
                }
                let want: Vec<String> = c["types"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
                if types != want {
                    failed.push(format!("{input:?}: types {types:?} != {want:?}"));
                }
            }
            Err(e) => {
                if c["error"].as_str() != Some(&e.0) {
                    failed.push(format!("{input:?}: error {:?} vs {}", e.0, c["error"]));
                }
            }
        }
    }
    report("selector", cases.len(), failed);
}

#[test]
fn postcss_roundtrip() {
    let cases = load("postcss");
    let mut failed = Vec::new();
    for c in &cases {
        let input = c["input"].as_str().unwrap();
        let mut css = postcss::Css::new();
        match postcss::parse(&mut css, input) {
            Ok(root) => {
                let rule = css.rule(".n");
                let d = css.decl("p", "v");
                css.append(rule, d);
                css.append(root, rule);
                let at = css.at_rule("media", "x");
                let m = css.rule(".m");
                css.append(at, m);
                css.prepend(root, at);
                let out = postcss::to_string(&mut css, root);
                if Some(out.as_str()) != c["out"].as_str() {
                    failed.push(format!("{input:?}\n got {out:?}\nwant {}", c["out"]));
                }
            }
            Err(e) => {
                if Some(e.message().as_str()) != c["error"].as_str() {
                    failed.push(format!("{input:?}: error {:?} vs {}", e.message(), c["error"]));
                }
            }
        }
    }
    report("postcss", cases.len(), failed);
}

fn to_json(v: &tailwindcss_v3::js::Value) -> Value {
    use tailwindcss_v3::js::Value as V;
    match v {
        V::Undefined | V::Func(_) => Value::Null,
        V::Null => Value::Null,
        V::Bool(b) => Value::Bool(*b),
        V::Num(n) => serde_json::Number::from_f64(*n).map(Value::Number).unwrap_or(Value::Null),
        V::Str(s) => Value::String(s.to_string()),
        V::Arr(a) => Value::Array(a.iter().map(to_json).collect()),
        V::Obj(o) => Value::Object(o.iter().filter(|(_, v)| !v.is_undefined() && !v.is_function()).map(|(k, v)| (k.to_string(), to_json(v))).collect()),
    }
}

#[test]
fn resolve_config() {
    let cases = load("resolve-config");
    let mut failed = Vec::new();
    for c in &cases {
        let cfg = tailwindcss_v3::js::Value::from_json(&c["config"]);
        let r = tailwindcss_v3::tw::config::resolve_config(&cfg);
        let mut got = serde_json::Map::new();
        for k in ["theme", "corePlugins", "prefix", "important", "separator", "darkMode"] {
            let v = r.get(k);
            if !v.is_undefined() {
                got.insert(k.into(), to_json(&v));
            }
        }
        let got = Value::Object(got);
        if got != c["resolved"] {
            let g = serde_json::to_string(&got).unwrap();
            let w = serde_json::to_string(&c["resolved"]).unwrap();
            let i = g.bytes().zip(w.bytes()).position(|(a, b)| a != b).unwrap_or(0);
            failed.push(format!("{}: differs at {i}: got ...{}... want ...{}...", c["config"], &g[i.saturating_sub(80)..(i + 80).min(g.len())], &w[i.saturating_sub(80)..(i + 80).min(w.len())]));
        }
    }
    report("resolve-config", cases.len(), failed);
}

/// The extractor's hand-written scanners against the regex engine on random lines.
#[test]
fn extractor_scan_fuzz() {
    let alphabet: Vec<&str> = vec![
        "a", "b", "z", "Q", "0", "9", "_", "-", "-", ":", ":", "[", "]", "!", "@", "/", ".", "\"", "'", "`", "\\", "$", "=", "{", "}", "(", ")", "<", ">",
        "#", "%", " ", " ", "\t", ",", "&", "*", "+", "é", "😀", "md:", "hover:", "bg-", "-[", "]:", "p-4", "w-1/2", "\n",
    ];
    let extractor = tailwindcss_v3::extractor::Extractor::new(":", "");
    let mut seed: u64 = 0x9e3779b97f4a7c15;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..200_000 {
        let len = (next() % 24) as usize;
        let s: String = (0..len).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
        assert_eq!(extractor.extract(&s), extractor.extract_reference(&s), "{s:?}");
    }
}

#[test]
fn candidates_with_class() {
    let c = tailwindcss_v3::Candidates::new(["card", "md:card", "hover:!-card", "[&_p]:card", "card-lg", "md:flex", "x-card"]);
    assert_eq!(c.with_class("card"), ["[&_p]:card", "card", "hover:!-card", "md:card"]);
}

/// `Candidates::new` against `candidates_from_tokens` on token lists with whitespace.
#[test]
fn candidates_tokenization_fuzz() {
    let alphabet = ["a", "b", "md:x", "!*", "", " ", "\n", "\t", "a\tb", " a", "b ", "x\ny", "\u{a0}", "c", "\r\n", "  "];
    let mut seed: u64 = 0x2545f4914f6cdd1d;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..20_000 {
        let len = (next() % 12) as usize;
        let tokens: Vec<&str> = (0..len).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
        let want: Vec<String> =
            tailwindcss_v3::tw::process::candidates_from_tokens(tokens.iter().copied(), &[]).into_iter().filter(|c| c != "\u{0}*").collect();
        let got: Vec<String> = tailwindcss_v3::Candidates::new(tokens.iter().copied()).iter().map(|s| s.to_string()).collect();
        assert_eq!(got, want, "{tokens:?}");
    }
}

/// Generations of one `Generator` do not share state (the `min-*` unit cache once did).
#[test]
fn generator_calls_are_independent() {
    let config = tailwindcss_v3::Config::default_config();
    let list = ["min-[640px]:flex", "p-4"];
    let cands = tailwindcss_v3::Candidates::new(list);
    let g = tailwindcss_v3::Generator::new(&config, &cands);
    let a = ".a { @apply min-[40em]:flex }";
    let b = ".b { @apply min-[700px]:block }";
    let _ = g.generate(a);
    assert_eq!(g.generate(b), tailwindcss_v3::generate(&config, b, list));
    assert_eq!(g.generate(a), tailwindcss_v3::generate(&config, a, list));
}

/// `Generator` (prefab splicing, derived configs) against `generate` on random blocks made of
/// pieces that reach across the generated rules: whitespace styles, comments, `@apply` (with
/// variants and defaults), media queries that match the variants', `@font-face`, `@import`,
/// top-level declarations, `theme()`, `@screen`, autoprefixer input, directives. `FUZZ_N` sets
/// the number of runs per config.
#[test]
fn generator_fuzz() {
    let pieces = [
        "",
        "\n  .a { color: red; }\n",
        ".a{color:red}",
        "/* c */",
        "/*! keep */\n",
        "\n\n/*x*/\n",
        ".b { @apply p-4 text-red-500; }",
        "\n    .c {\n      @apply hover:underline sm:flex;\n    }\n",
        "@media (min-width: 640px) { .d { @apply font-bold; } }",
        "@media (min-width: 640px) {\n  .e { color: blue }\n}",
        "\n@media (min-width: 768px) {\n    .z { color: red }\n}\n",
        "@media (prefers-color-scheme: dark) { .pc { color: red } }",
        ".hover\\:underline:hover { x: y }",
        "@supports (display: grid) { .g { display: grid } }",
        "@font-face { font-family: X; src: url(x.woff) }",
        "@import url('x.css');",
        "color: red;",
        ".t { color: theme('colors.red.500') }",
        "@screen md { .s { color: red } }",
        "::placeholder { color: gray }",
        ".tr { transition: transform 1s; user-select: none; backdrop-filter: blur(2px); }",
        "  .w {\n      display: flex;\n  }\n",
        "\t.x\t{\tmargin:0\t}",
        ".y { @apply shadow ring; }",
        "@tailwind utilities;",
        "@layer components { .btn { @apply px-4 } }",
        "*, ::before, ::after { box-sizing: border-box }",
        ".container { max-width: 1px }",
        "/* autoprefixer: off */",
        ".ab { @apply dark:bg-black; }",
        ".p-4 { color: red }",
        ".sm\\:flex { color: red }",
        ".bad { @apply nonexistent; }",
        ".mm { @apply min-[40em]:flex; }",
        "@media (min-width: 640px) { .sm\\:flex { display: flex } }",
        ".e2 {}",
        "\n",
        ";",
        "\n    .gh {\n      @apply font-normal group-hover:underline;\n    }\n  ",
        "\n  .pe { @apply peer-focus:flex !p-4; }\n",
        "\n  .dk { @apply dark:sm:flex; }",
    ];
    let configs = [
        serde_json::json!({}),
        serde_json::json!({ "corePlugins": { "preflight": false } }),
        serde_json::json!({ "important": true, "darkMode": "class" }),
        serde_json::json!({ "theme": { "extend": { "colors": { "brand": "#123456" } } } }),
        serde_json::json!({ "theme": { "extend": { "colors": { "brand": "#654321", "red": { "500": "#f00" } } } } }),
        serde_json::json!({ "important": "#app", "corePlugins": { "preflight": false } }),
    ];
    let list = [
        "p-4", "sm:flex", "hover:underline", "font-normal", "group-hover:underline", "peer-focus:flex", "md:p-2", "lg:grid", "text-red-500", "shadow", "transition", "dark:bg-black", "placeholder:text-gray-500",
        "select-none", "bg-brand", "md:bg-brand/50", "text-brand", "backdrop-blur", "min-[640px]:flex", "[&_p]:mt-1", "container",
    ];
    let n: usize = std::env::var("FUZZ_N").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    let mut seed: u64 = 0x5851f42d4c957f2d;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let parsed: Vec<tailwindcss_v3::Config> = configs.iter().map(|c| tailwindcss_v3::Config::from_json(c.clone()).unwrap()).collect();
    for subset in 0..3 {
        let cands: Vec<&str> = list.iter().copied().filter(|_| subset == 0 || next() % 2 == 0).collect();
        let candidates = tailwindcss_v3::Candidates::new(cands.iter().copied());
        let base = tailwindcss_v3::Generator::new(&parsed[0], &candidates);
        for (ci, config) in parsed.iter().enumerate() {
            let generator = if ci == 0 { None } else { Some(base.derive(config)) };
            let g = generator.as_ref().unwrap_or(&base);
            for _ in 0..n {
                let k = (next() % 5) as usize;
                let seps = ["", "\n", "\n  ", " "];
                let mut blocks = String::new();
                for _ in 0..k {
                    blocks.push_str(seps[(next() % 4) as usize]);
                    blocks.push_str(pieces[(next() % pieces.len() as u64) as usize]);
                }
                let want = tailwindcss_v3::generate(config, &blocks, cands.iter().copied());
                let got = g.generate(&blocks);
                assert_eq!(got, want, "config {ci}, candidates {cands:?}, blocks {blocks:?}");
            }
        }
    }
}
