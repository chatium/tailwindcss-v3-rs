//! `cargo run --release --example extract -- <file>…` (or file paths on stdin) → JSON
//! `{ file: [candidates] }`
use std::io::BufRead;

fn main() {
    let mut files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        files = std::io::stdin().lock().lines().map(|l| l.unwrap()).filter(|l| !l.is_empty()).collect();
    }
    let mut out = serde_json::Map::new();
    let t = std::time::Instant::now();
    let mut bytes = 0;
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        bytes += text.len();
        let c: Vec<serde_json::Value> = tailwindcss_v3::extract_candidates(&text).into_iter().map(|s| s.into()).collect();
        out.insert(f, c.into());
    }
    eprintln!("extracted {} bytes in {:?}", bytes, t.elapsed());
    println!("{}", serde_json::Value::Object(out));
}
