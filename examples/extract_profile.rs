//! `cargo run --release --example extract_profile -- <dir>`: time per extractor pattern over the
//! distinct trimmed lines of a directory (no line cache).
use tailwindcss_v3::jsregex::{Regex, Tables};
fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(d).unwrap().flatten() {
        let p = e.path();
        let n = e.file_name().to_string_lossy().to_string();
        if n.starts_with('.') || n == "node_modules" {
            continue;
        }
        if p.is_dir() { walk(&p, out) } else { out.push(p) }
    }
}
fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let mut files = Vec::new();
    walk(std::path::Path::new(&dir), &mut files);
    let mut lines = std::collections::HashSet::new();
    let texts: Vec<String> = files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect();
    for t in &texts {
        for l in t.split('\n') {
            lines.insert(l.trim());
        }
    }
    let units: Vec<Vec<u16>> = lines.iter().map(|l| l.encode_utf16().collect()).collect();
    eprintln!("{} distinct lines, {} units", units.len(), units.iter().map(|u| u.len()).sum::<usize>());
    let sources = if std::env::var("EXTRACT_ONLY").is_ok() { Vec::new() } else { tailwindcss_v3::extractor::build_sources(":", "") };
    for (i, src) in sources.iter().enumerate() {
        let re = Regex::new(src, false);
        let t = std::time::Instant::now();
        let mut n = 0;
        for u in &units {
            let mut tables = Tables::new(u);
            n += re.find_all_units(&mut tables).len();
        }
        eprintln!("pattern {i}: {:?} ({n} matches)", t.elapsed());
    }
    let t = std::time::Instant::now();
    let mut n = 0;
    for t in &texts {
        n += tailwindcss_v3::extract_candidates(t).len();
    }
    eprintln!("extract_candidates over files: {:?} ({n})", t.elapsed());
}
