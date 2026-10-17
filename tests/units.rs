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
