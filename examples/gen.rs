//! `cargo run --example gen -- '<config json>' '<blocks>' cand1 cand2 …`
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = tailwindcss_v3::Config::from_json(serde_json::from_str(&args[0]).unwrap()).unwrap();
    match tailwindcss_v3::generate(&config, &args[1], args[2..].iter().map(|s| s.as_str())) {
        Ok(css) => println!("{css}"),
        Err(e) => println!("ERROR {e:?}"),
    }
}
