//! `cargo run --release --example extract_bench -- <dir>`: slowest files for extract_candidates
fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(d).unwrap().flatten() {
        let p = e.path();
        let n = e.file_name().to_string_lossy().to_string();
        if n == "node_modules" || n == ".git" {
            continue;
        }
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}
fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let mut files = Vec::new();
    walk(std::path::Path::new(&dir), &mut files);
    let mut times = Vec::new();
    let t0 = std::time::Instant::now();
    let mut total = 0;
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let t = std::time::Instant::now();
        let n = tailwindcss_v3::extract_candidates(&text).len();
        total += text.len();
        times.push((t.elapsed(), text.len(), n, f));
    }
    eprintln!("total {} bytes in {:?}", total, t0.elapsed());
    times.sort_by(|a, b| b.0.cmp(&a.0));
    for (t, len, n, f) in times.iter().take(12) {
        eprintln!("{t:>12?} {len:>9} {n:>6} {}", f.display());
    }
}
