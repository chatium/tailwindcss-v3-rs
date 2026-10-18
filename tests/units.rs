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
