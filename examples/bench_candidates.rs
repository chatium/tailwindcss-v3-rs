//! `cargo run --release --example bench_candidates -- <account substring>`: Candidates::new timing
fn main() {
    let filter = std::env::args().nth(1).unwrap();
    let corpus: serde_json::Value = serde_json::from_str(&std::fs::read_to_string("tests/corpus-local/accounts.json").unwrap()).unwrap();
    let (_, list) = corpus["candidates"].as_object().unwrap().iter().find(|(k, _)| k.contains(&filter)).unwrap();
    let list: Vec<&str> = list.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    for _ in 0..std::env::var("N").ok().and_then(|s| s.parse().ok()).unwrap_or(5) {
        let t = std::time::Instant::now();
        let c = tailwindcss_v3::Candidates::new(list.iter().copied());
        eprintln!("{} candidates, {} viable: {:?}", c.len(), c.viable_len(), t.elapsed());
    }
}
