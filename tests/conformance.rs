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

type GenFn<'f> = &'f dyn Fn(&tailwindcss_v3::Config, &str, &[&str]) -> Result<String, tailwindcss_v3::Error>;

pub fn run_case(c: &Value) -> Result<(), String> {
    run_case_with(c, &|config, blocks, candidates| tailwindcss_v3::generate(config, blocks, candidates.iter().copied()))
}

/// Through `Candidates` + `Generator` (fresh per case).
fn run_case_generator(c: &Value) -> Result<(), String> {
    run_case_with(c, &|config, blocks, candidates| {
        let cands = tailwindcss_v3::Candidates::new(candidates.iter().copied());
        tailwindcss_v3::Generator::new(config, &cands).generate(blocks)
    })
}

fn run_case_with(c: &Value, generate: GenFn) -> Result<(), String> {
    let config = match tailwindcss_v3::Config::from_json(c["config"].clone()) {
        Ok(c) => c,
        Err(e) => {
            // tailwind reports config problems when it runs
            return match c["expected"].get("error") {
                Some(w) if w["message"].as_str() == Some(e.message.as_str()) => Ok(()),
                _ => Err(format!("config error: {e}")),
            };
        }
    };
    let blocks = c["blocks"].as_str().unwrap_or("");
    let candidates: Vec<&str> = c["candidates"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
    let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| generate(&config, blocks, &candidates)))
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
        let generator = std::env::var("GENERATOR").is_ok();
        for (i, c) in cases.iter().enumerate() {
            if let Err(e) = if generator { run_case_generator(c) } else { run_case(c) } {
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

/// The local production corpus (account data is not committed; see tools/extract-corpus.mjs
/// and tools/gen-fixtures-local.mjs). Skipped when absent. `ONLY=substring` filters cases.
#[test]
fn conformance_local() {
    let root = env!("CARGO_MANIFEST_DIR");
    let corpus_path = format!("{root}/tests/corpus-local/accounts.json");
    let Ok(text) = std::fs::read_to_string(&corpus_path) else {
        eprintln!("no local corpus");
        return;
    };
    let corpus: Value = serde_json::from_str(&text).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    let show: usize = std::env::var("SHOW").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let only = std::env::var("ONLY").ok();
    let mut checked = 0;
    let mut failed = 0;
    let mut slowest: Vec<(std::time::Duration, String)> = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let name = c["name"].as_str().unwrap();
        if only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
            continue;
        }
        let Ok(fx) = std::fs::read_to_string(format!("{root}/tests/fixtures-local/accounts/{i}.json")) else { continue };
        let fx: Value = serde_json::from_str(&fx).unwrap();
        let mut case = c.clone();
        case["candidates"] = corpus["candidates"][c["account"].as_str().unwrap()].clone();
        case["expected"] = fx["expected"].clone();
        let t = std::time::Instant::now();
        let r = run_case(&case);
        slowest.push((t.elapsed(), name.to_string()));
        checked += 1;
        if let Err(e) = r {
            if failed < show {
                eprintln!("--- {name}: {e}");
            }
            failed += 1;
        }
    }
    slowest.sort_by(|a, b| b.0.cmp(&a.0));
    for (t, n) in slowest.iter().take(3) {
        eprintln!("slow: {t:?} {n}");
    }
    eprintln!("local: {}/{} passed", checked - failed, checked);
    assert_eq!(failed, 0);
}

/// The local corpus through `Candidates` (once per account) and `Generator` (once per config),
/// compared with the same fixtures, plus timings against plain `generate`.
#[test]
fn generator_local() {
    let root = env!("CARGO_MANIFEST_DIR");
    let Ok(text) = std::fs::read_to_string(format!("{root}/tests/corpus-local/accounts.json")) else {
        eprintln!("no local corpus");
        return;
    };
    let corpus: Value = serde_json::from_str(&text).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    let only = std::env::var("ONLY").ok();
    let mut by_account: std::collections::BTreeMap<&str, Vec<usize>> = Default::default();
    for (i, c) in cases.iter().enumerate() {
        if only.as_ref().is_some_and(|o| !c["name"].as_str().unwrap().contains(o.as_str())) {
            continue;
        }
        by_account.entry(c["account"].as_str().unwrap()).or_default().push(i);
    }
    let (mut checked, mut failed) = (0, 0);
    for (account, idxs) in by_account {
        let list: Vec<&str> = corpus["candidates"][account].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let t = std::time::Instant::now();
        let cands = tailwindcss_v3::Candidates::new(list.iter().copied());
        let t_cands = t.elapsed();
        let mut generators: Vec<(String, tailwindcss_v3::Generator)> = Vec::new();
        let mut t_new = std::time::Duration::ZERO;
        let mut t_gen = std::time::Duration::ZERO;
        for &i in &idxs {
            let c = &cases[i];
            let Ok(fx) = std::fs::read_to_string(format!("{root}/tests/fixtures-local/accounts/{i}.json")) else { continue };
            let fx: Value = serde_json::from_str(&fx).unwrap();
            let mut case = c.clone();
            case["expected"] = fx["expected"].clone();
            let key = c["config"].to_string();
            if let Ok(config) = tailwindcss_v3::Config::from_json(c["config"].clone()) {
                if !generators.iter().any(|(k, _)| *k == key) {
                    let t = std::time::Instant::now();
                    let config: &'static tailwindcss_v3::Config = Box::leak(Box::new(config));
                    generators.push((key.clone(), tailwindcss_v3::Generator::new(config, &cands)));
                    t_new += t.elapsed();
                }
            }
            let generator = generators.iter().find(|(k, _)| *k == key).map(|(_, g)| g);
            let t = std::time::Instant::now();
            let r = run_case_with(&case, &|_, blocks, _| generator.unwrap().generate(blocks));
            t_gen += t.elapsed();
            checked += 1;
            if let Err(e) = r {
                if failed < 5 {
                    eprintln!("--- {}: {e}", c["name"]);
                }
                failed += 1;
            }
        }
        let working: Vec<usize> = generators.iter().map(|(_, g)| g.working_len()).collect();
        eprintln!(
            "{account}: {} candidates, {} viable; Candidates::new {t_cands:?}, {} Generator::new {t_new:?} (working {:?}), {} generate {t_gen:?}",
            cands.len(),
            cands.viable_len(),
            generators.len(),
            working.iter().max(),
            idxs.len()
        );
    }
    eprintln!("generator local: {}/{} passed", checked - failed, checked);
    assert_eq!(failed, 0);
}
