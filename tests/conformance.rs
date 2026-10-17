//! Differential conformance: every expected output was produced by the Play CDN bundle
//! (tailwindcss 3.4.16 + autoprefixer) — see `tools/gen-fixtures.mjs`.
//!
//! `SHOW=n` prints n failures per fixture; `ONLY=name` runs one fixture file.

use serde_json::Value;

fn fixture_names() -> Vec<String> {
    let dir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".json") && !matches!(n.as_str(), "selector.json" | "postcss.json" | "resolve-config.json"))
        .map(|n| n.trim_end_matches(".json").to_string())
        .collect();
    names.sort();
    names
}

fn first_diff(a: &str, b: &str) -> String {
    let i = a.bytes().zip(b.bytes()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let from = i.saturating_sub(100);
    let cut = |s: &str| {
        let mut f = from.min(s.len());
        while !s.is_char_boundary(f) {
            f -= 1;
        }
        let mut t = (i + 150).min(s.len());
        while !s.is_char_boundary(t) {
            t += 1;
        }
        s[f..t].to_string()
    };
    format!("at byte {i}\n   got: …{}…\n  want: …{}…", cut(a), cut(b))
}

pub fn run_case(c: &Value) -> Result<(), String> {
    let config = match tailwindcss_v3::Config::from_json(c["config"].clone()) {
        Ok(c) => c,
        Err(e) => return Err(format!("config error: {e}")),
    };
    let blocks = c["blocks"].as_str().unwrap_or("");
    let candidates: Vec<&str> = c["candidates"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
    let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tailwindcss_v3::generate(&config, blocks, candidates.iter().copied())))
        .map_err(|p| format!("panic: {:?}", p.downcast_ref::<String>().cloned().or(p.downcast_ref::<&str>().map(|s| s.to_string()))))?;
    let want = &c["expected"];
    match (got, want.get("css"), want.get("error")) {
        (Ok(css), Some(w), _) => {
            let w = w.as_str().unwrap();
            if css == w { Ok(()) } else { Err(first_diff(&css, w)) }
        }
        (Ok(_), None, Some(e)) => Err(format!("expected error {}", e["message"])),
        (Err(e), Some(_), _) => Err(format!("unexpected error {}", e.message)),
        (Err(e), None, Some(w)) => {
            if Some(e.message.as_str()) == w["message"].as_str() {
                Ok(())
            } else {
                Err(format!("error {:?} != {}", e.message, w["message"]))
            }
        }
        _ => Err("bad fixture".into()),
    }
}

#[test]
fn conformance() {
    let show: usize = std::env::var("SHOW").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let only = std::env::var("ONLY").ok();
    let mut total_failed = 0;
    let mut summary = Vec::new();
    for name in fixture_names() {
        if only.as_ref().is_some_and(|o| o != &name) {
            continue;
        }
        let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
        let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut failed = 0;
        for (i, c) in cases.iter().enumerate() {
            if let Err(e) = run_case(c) {
                if failed < show {
                    let label = c["name"].as_str().map(|s| s.to_string()).unwrap_or_else(|| i.to_string());
                    eprintln!("--- {name}/{label}: {e}");
                }
                failed += 1;
            }
        }
        summary.push(format!("{name}: {}/{} passed", cases.len() - failed, cases.len()));
        total_failed += failed;
    }
    for s in &summary {
        eprintln!("{s}");
    }
    assert_eq!(total_failed, 0, "conformance failures");
}
