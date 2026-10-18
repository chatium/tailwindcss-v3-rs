//! `cargo run --release --example bench_generator -- <account substring> [threads]`: all local
//! corpus cases of an account through `Candidates` + `Generator` vs plain `generate`, in parallel.
use std::sync::atomic::{AtomicUsize, Ordering};

// the consumer's allocator (SYSTEM_ALLOC=1 for the system one)
struct Alloc;
unsafe impl std::alloc::GlobalAlloc for Alloc {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        if *SYSTEM { unsafe { std::alloc::System.alloc(l) } } else { unsafe { mimalloc::MiMalloc.alloc(l) } }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        if *SYSTEM { unsafe { std::alloc::System.dealloc(p, l) } } else { unsafe { mimalloc::MiMalloc.dealloc(p, l) } }
    }
}
static SYSTEM: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("SYSTEM_ALLOC").is_some());
#[global_allocator]
static GLOBAL: Alloc = Alloc;

fn par<'s, T: Sync, R: Send>(items: &'s [T], threads: usize, f: impl Fn(&'s T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let mut out: Vec<(usize, R)> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut v = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= items.len() {
                            return v;
                        }
                        v.push((i, f(&items[i])));
                    }
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

fn main() {
    let filter = std::env::args().nth(1).unwrap();
    let threads: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    let plain = std::env::var("PLAIN").is_ok();
    let corpus: serde_json::Value = serde_json::from_str(&std::fs::read_to_string("tests/corpus-local/accounts.json").unwrap()).unwrap();
    let cases: Vec<&serde_json::Value> = corpus["cases"].as_array().unwrap().iter().filter(|c| c["account"].as_str().unwrap().contains(&filter)).collect();
    let account = cases[0]["account"].as_str().unwrap();
    let list: Vec<&str> = corpus["candidates"][account].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let keys: Vec<String> = cases.iter().map(|c| c["config"].to_string()).collect();
    let mut configs = keys.clone();
    configs.sort();
    configs.dedup();
    eprintln!("{account}: {} candidates, {} cases, {} configs, {threads} threads", list.len(), cases.len(), configs.len());

    if std::env::var("LOAD_ONLY").is_ok() {
        return;
    }
    let t = std::time::Instant::now();
    let cands = tailwindcss_v3::Candidates::new(list.iter().copied());
    let t_cands = t.elapsed();
    let t = std::time::Instant::now();
    let outs: Vec<Option<usize>> = if std::env::var("GROUPED").is_ok() {
        // one task per config: build its Generator (derived from the most frequent config's with
        // DERIVE=1), generate its cases, drop it
        let base_idx = (0..configs.len()).max_by_key(|&k| keys.iter().filter(|x| **x == configs[k]).count()).unwrap();
        let base_config = tailwindcss_v3::Config::from_json(serde_json::from_str(&configs[base_idx]).unwrap()).ok();
        let base = base_config.as_ref().filter(|_| std::env::var("DERIVE").is_ok()).map(|c| tailwindcss_v3::Generator::new(c, &cands));
        let outs: Vec<Vec<(usize, Option<usize>)>> = par(&configs, threads, |key| {
            let Ok(config) = tailwindcss_v3::Config::from_json(serde_json::from_str(key).unwrap()) else {
                return (0..cases.len()).filter(|&i| keys[i] == *key).map(|i| (i, None)).collect();
            };
            let g = match &base {
                Some(b) => b.derive(&config),
                None => tailwindcss_v3::Generator::new(&config, &cands),
            };
            (0..cases.len())
                .filter(|&i| keys[i] == *key)
                .map(|i| (i, Some(g.generate(cases[i]["blocks"].as_str().unwrap()).map(|s| s.len()).unwrap_or(0))))
                .collect()
        });
        let mut outs: Vec<(usize, Option<usize>)> = outs.into_iter().flatten().collect();
        outs.sort();
        outs.into_iter().map(|(_, r)| r).collect()
    } else {
        // every Generator first, then every case
        let parsed: Vec<Option<tailwindcss_v3::Config>> = configs.iter().map(|c| tailwindcss_v3::Config::from_json(serde_json::from_str(c).unwrap()).ok()).collect();
        // DERIVE=1: the most frequent config is the base, the others are derived from it
        let base_idx = (0..configs.len()).max_by_key(|&k| keys.iter().filter(|x| **x == configs[k]).count()).unwrap();
        let base = if std::env::var("DERIVE").is_ok() { parsed[base_idx].as_ref().map(|c| tailwindcss_v3::Generator::new(c, &cands)) } else { None };
        let gens = par(&parsed, threads, |c| {
            c.as_ref().map(|c| match &base {
                Some(b) => b.derive(c),
                None => tailwindcss_v3::Generator::new(c, &cands),
            })
        });
        eprintln!("Generator::new ×{}: {:?}", gens.len(), t.elapsed());
        par(&(0..cases.len()).collect::<Vec<_>>(), threads, |&i| {
            let k = configs.binary_search(&keys[i]).unwrap();
            gens[k].as_ref().map(|g| g.generate(cases[i]["blocks"].as_str().unwrap()).map(|s| s.len()).unwrap_or(0))
        })
    };
    let t_all = t.elapsed();
    eprintln!("Candidates::new {t_cands:?}, Generator::new ×{} + generate ×{}: {t_all:?}; total {:?}", configs.len(), cases.len(), t_cands + t_all);
    if plain {
        let t = std::time::Instant::now();
        let base = par(&cases, threads, |c| {
            let config = tailwindcss_v3::Config::from_json(c["config"].clone()).ok()?;
            Some(tailwindcss_v3::generate(&config, c["blocks"].as_str().unwrap(), list.iter().copied()).map(|s| s.len()).unwrap_or(0))
        });
        eprintln!("plain generate ×{} {:?}; same sizes: {}", cases.len(), t.elapsed(), base == outs);
    }
}
