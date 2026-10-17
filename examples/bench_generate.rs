//! `cargo run --release --example bench_generate -- <name substring>`: times `generate` on local
//! corpus cases (tests/corpus-local/accounts.json).
fn main() {
    let filter = std::env::args().nth(1).unwrap_or_default();
    let corpus: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string("tests/corpus-local/accounts.json").unwrap()).unwrap();
    for c in corpus["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        if !name.contains(&filter) {
            continue;
        }
        let cands: Vec<&str> = corpus["candidates"][c["account"].as_str().unwrap()].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let t = std::time::Instant::now();
        let config = tailwindcss_v3::Config::from_json(c["config"].clone()).unwrap();
        let t1 = t.elapsed();
        let r = tailwindcss_v3::generate(&config, c["blocks"].as_str().unwrap(), cands.iter().copied());
        eprintln!("{:>10?} (config {:?}) {} candidates → {} bytes  {name}", t.elapsed(), t1, cands.len(), r.map(|s| s.len()).unwrap_or(0));
        break;
    }
}
