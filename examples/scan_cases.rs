//! `cargo run --release --example scan_cases -- <fixture>`: every case through Generator, name first
use std::io::Write;
fn main() {
    let f = std::env::args().nth(1).unwrap();
    let cases: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("tests/fixtures/{f}.json")).unwrap()).unwrap();
    for c in cases.as_array().unwrap() {
        eprint!("{} ... ", c["name"]);
        std::io::stderr().flush().unwrap();
        let Ok(config) = tailwindcss_v3::Config::from_json(c["config"].clone()) else { eprintln!("config error"); continue };
        let list: Vec<&str> = c["candidates"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
        let cands = tailwindcss_v3::Candidates::new(list.iter().copied());
        let t = std::time::Instant::now();
        let r = tailwindcss_v3::Generator::new(&config, &cands).generate(c["blocks"].as_str().unwrap_or(""));
        let want = tailwindcss_v3::generate(&config, c["blocks"].as_str().unwrap_or(""), list.iter().copied());
        eprintln!("{:?} {}", t.elapsed(), if r == want { "ok" } else { "DIFF" });
    }
}
