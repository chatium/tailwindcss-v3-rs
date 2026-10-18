//! `cargo run --release --example debug_local -- <name substring>`: Generator vs generate on a
//! local corpus case
fn main() {
    let name = std::env::args().nth(1).unwrap();
    let corpus: serde_json::Value = serde_json::from_str(&std::fs::read_to_string("tests/corpus-local/accounts.json").unwrap()).unwrap();
    let c = corpus["cases"].as_array().unwrap().iter().find(|c| c["name"].as_str().unwrap().contains(&name)).unwrap();
    let list: Vec<&str> = corpus["candidates"][c["account"].as_str().unwrap()].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let config = tailwindcss_v3::Config::from_json(c["config"].clone()).unwrap();
    let blocks = c["blocks"].as_str().unwrap();
    let want = tailwindcss_v3::generate(&config, blocks, list.iter().copied()).unwrap();
    let cands = tailwindcss_v3::Candidates::new(list.iter().copied());
    let got = tailwindcss_v3::Generator::new(&config, &cands).generate(blocks).unwrap();
    let i = want.bytes().zip(got.bytes()).position(|(x, y)| x != y).unwrap_or(want.len().min(got.len()));
    let cut = |s: &str, a: usize, b: usize| { let mut a = a.min(s.len()); while !s.is_char_boundary(a) { a -= 1 } let mut b = b.min(s.len()); while !s.is_char_boundary(b) { b += 1 } s[a..b].to_string() };
    println!("at {i} of {} / {}\nwant: {:?}\n got: {:?}", want.len(), got.len(), cut(&want, i.saturating_sub(150), i + 150), cut(&got, i.saturating_sub(150), i + 150));
    println!("blocks: {:?}", &blocks[..blocks.len().min(400)]);
}
