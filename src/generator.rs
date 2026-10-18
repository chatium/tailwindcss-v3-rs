//! Many `generate` calls over one candidate set: the candidates are prepared once per account,
//! matched once per config, and every `generate(blocks)` runs only over the candidates that can
//! produce rules.
//!
//! Why the output stays byte-identical to `generate(config, blocks, all candidates)`: a candidate
//! that resolves to no rules has no effect on the output. Its side effects on the context are
//! either nothing at all (no plugin for its class part) or order-only: arbitrary variant bits are
//! remapped by name before sorting, and offsets are only compared. The exception is the unit
//! cache of `min-*`/`max-*`, so those candidates are always kept. Blocks change which candidates
//! produce rules only through their `@layer` classes (static rules matched by the class part), so
//! candidates whose class part is such an identifier are added per call.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

use crate::js::{FxHashSet, FxHasher};
use crate::tw::context::Context;
use crate::tw::generate::{class_part, has_plugins};
use crate::tw::process;
use crate::{autoprefixer, postcss, Config, Error, INPUT_PREFIX};

/// The class names of one account, deduplicated and ordered the way tailwind sees them.
pub struct Candidates {
    list: Vec<Box<str>>,
    /// indices of candidates whose class part (separator `:`) has a plugin in some config
    /// without plugins and prefix
    viable: Vec<u32>,
    /// (hash of the class part with separator `:`, index), sorted
    by_class: Vec<(u64, u32)>,
}

/// One config over one `Candidates`: `generate(blocks)` is `&self` and can run in parallel.
pub struct Generator<'a> {
    config: &'a Config,
    candidates: &'a Candidates,
    /// candidates that produce rules without blocks (plus `min-*`/`max-*` ones), sorted
    working: Vec<u32>,
    /// the class index for a separator other than `:`
    own_index: Option<Vec<(u64, u32)>>,
    /// The context every call without `@layer` rules reaches before `expandTailwindAtRules`:
    /// `createContext` and `generateRules` over the working candidates (+ safelist) done.
    ready: Option<Context<'a>>,
}

fn hash(s: &str) -> u64 {
    let mut h = FxHasher::default();
    s.hash(&mut h);
    h.finish()
}

/// A token that `candidates_from_tokens` passes through unchanged.
fn is_simple(t: &str) -> bool {
    !t.is_empty() && !t.chars().any(crate::js::is_js_whitespace)
}

const RUN_MARKER: &str = "\u{1}";

/// What `candidates_from_tokens` makes of the non-simple tokens: they depend on their neighbours
/// only through whether a non-whitespace token is next to them, so every run of simple tokens is
/// replaced by a marker. `None` when the marker could be ambiguous.
fn pieces_of_complex(tokens: &[&str]) -> Option<Vec<String>> {
    if tokens.is_empty() {
        // `''.split('\n')` is one empty line
        return Some(vec![String::new()]);
    }
    if tokens.iter().all(|t| is_simple(t)) {
        return Some(Vec::new());
    }
    let mut reduced: Vec<&str> = Vec::new();
    for t in tokens {
        if is_simple(t) {
            if reduced.last() != Some(&RUN_MARKER) {
                reduced.push(RUN_MARKER);
            }
        } else if t.contains(RUN_MARKER) {
            return None;
        } else {
            reduced.push(t);
        }
    }
    let not_on_demand = crate::tw::context::NOT_ON_DEMAND;
    Some(process::candidates_from_tokens(reduced, &[]).into_iter().filter(|c| c != not_on_demand && c != RUN_MARKER).collect())
}

/// `f` over contiguous chunks of `items` on up to `n` threads, results in chunk order.
fn par_chunks<'s, T: Sync, R: Send>(items: &'s [T], n: usize, f: impl Fn(&'s [T]) -> R + Sync) -> Vec<R> {
    let chunk = items.len().div_ceil(n).max(1);
    std::thread::scope(|s| {
        let f = &f;
        let hs: Vec<_> = items.chunks(chunk).map(|part| s.spawn(move || f(part))).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(16)
}

/// Merges sorted, deduplicated runs into one sorted, deduplicated list.
fn merge_sorted(mut runs: Vec<Vec<&str>>) -> Vec<&str> {
    while runs.len() > 1 {
        let mut next = Vec::with_capacity(runs.len().div_ceil(2));
        let mut it = runs.into_iter();
        while let Some(a) = it.next() {
            let Some(b) = it.next() else {
                next.push(a);
                break;
            };
            let mut out = Vec::with_capacity(a.len() + b.len());
            let (mut i, mut j) = (0, 0);
            while i < a.len() && j < b.len() {
                match crate::js::cmp_utf16(a[i], b[j]) {
                    std::cmp::Ordering::Less => {
                        out.push(a[i]);
                        i += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        out.push(b[j]);
                        j += 1;
                    }
                    std::cmp::Ordering::Equal => {
                        out.push(a[i]);
                        i += 1;
                        j += 1;
                    }
                }
            }
            out.extend_from_slice(&a[i..]);
            out.extend_from_slice(&b[j..]);
            next.push(out);
        }
        runs = next;
    }
    runs.pop().unwrap_or_default()
}

fn class_index(list: &[Box<str>], separator: &str) -> Vec<(u64, u32)> {
    let mut v: Vec<(u64, u32)> = list.iter().enumerate().map(|(i, c)| (hash(class_part(c, separator)), i as u32)).collect();
    v.sort_unstable();
    v
}

/// The candidate rule map keys of the default config (no prefix, all core plugins). A config
/// without plugins only adds values to these roots.
fn default_keys() -> &'static FxHashSet<String> {
    static KEYS: OnceLock<FxHashSet<String>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let config = Config::default_config();
        let mut ctx = Context::new(&config.inner);
        let root = postcss::parse(&mut ctx.css, INPUT_PREFIX).unwrap();
        process::prepare(&mut ctx, root).unwrap();
        ctx.candidate_rule_map.keys().cloned().collect()
    })
}

impl Candidates {
    /// Class names as `generate` takes them (order and duplicates do not matter).
    pub fn new<'a>(candidates: impl IntoIterator<Item = &'a str>) -> Candidates {
        let tokens: Vec<&str> = candidates.into_iter().collect();
        let n = threads();
        let list: Vec<Box<str>> = match pieces_of_complex(&tokens) {
            Some(extra) => {
                let runs = par_chunks(&tokens, n, |part| {
                    let mut v: Vec<&str> = part.iter().copied().filter(|t| is_simple(t) && *t != "!*").collect();
                    v.sort_unstable_by(|a, b| crate::js::cmp_utf16(a, b));
                    v.dedup();
                    v
                });
                let mut extra: Vec<&str> = extra.iter().map(|s| s.as_str()).collect();
                extra.sort_unstable_by(|a, b| crate::js::cmp_utf16(a, b));
                extra.dedup();
                let mut runs = runs;
                runs.push(extra);
                let merged = merge_sorted(runs);
                par_chunks(&merged, n, |part| part.iter().map(|&s| Box::from(s)).collect::<Vec<Box<str>>>()).into_iter().flatten().collect()
            }
            None => {
                let not_on_demand = crate::tw::context::NOT_ON_DEMAND;
                process::candidates_from_tokens(tokens, &[]).into_iter().filter(|c| c != not_on_demand).map(String::into_boxed_str).collect()
            }
        };
        let keys = default_keys();
        let (viable, mut by_class): (Vec<u32>, Vec<(u64, u32)>) = {
            let chunk = list.len().div_ceil(n).max(1);
            let parts: Vec<(Vec<u32>, Vec<(u64, u32)>)> = std::thread::scope(|s| {
                let hs: Vec<_> = (0..list.len())
                    .step_by(chunk)
                    .map(|start| {
                        let list = &list;
                        s.spawn(move || {
                            let mut viable = Vec::new();
                            let mut index = Vec::new();
                            for i in start..(start + chunk).min(list.len()) {
                                let class = class_part(&list[i], ":");
                                if has_plugins(class, "", |k| keys.contains(k)) {
                                    viable.push(i as u32);
                                }
                                index.push((hash(class), i as u32));
                            }
                            (viable, index)
                        })
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap()).collect()
            });
            let mut viable = Vec::new();
            let mut index = Vec::new();
            for (v, ix) in parts {
                viable.extend(v);
                index.extend(ix);
            }
            (viable, index)
        };
        by_class.sort_unstable();
        Candidates { list, viable, by_class }
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// The candidates in tailwind's order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.list.iter().map(|s| &**s)
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Candidates whose class part (separator `:`, without variants, `!` and a leading `-`) is
    /// `class`: `textarea-field`, `md:textarea-field`, `hover:!-textarea-field`, …
    pub fn with_class(&self, class: &str) -> Vec<&str> {
        let mut out: Vec<u32> = Vec::new();
        for key in [class.to_string(), format!("-{class}")] {
            let h = hash(&key);
            let start = self.by_class.partition_point(|&(x, _)| x < h);
            out.extend(
                self.by_class[start..]
                    .iter()
                    .take_while(|&&(x, _)| x == h)
                    .map(|&(_, i)| i)
                    .filter(|&i| class_part(&self.list[i as usize], ":") == key),
            );
        }
        out.sort_unstable();
        out.into_iter().map(|i| &*self.list[i as usize]).collect()
    }

    /// Candidates that can produce a rule in some config (without blocks).
    pub fn viable_len(&self) -> usize {
        self.viable.len()
    }
}

impl<'a> Generator<'a> {
    pub fn new(config: &'a Config, candidates: &'a Candidates) -> Generator<'a> {
        let cfg = &*config.inner;
        let mut ctx = Context::new(cfg);
        let root = postcss::parse(&mut ctx.css, INPUT_PREFIX).expect("the prefix parses");
        let mut generator = Generator { config, candidates, working: Vec::new(), own_index: None, ready: None };
        if config.content_candidates.is_some() || process::prepare(&mut ctx, root).is_err() {
            // `{ config }` ignores the candidates; a broken config fails the same way every time
            return generator;
        }
        let keys = default_keys();
        let standard = cfg.separator == ":" && ctx.candidate_rule_map.keys().all(|k| keys.contains(k));
        let pool: Vec<u32> = if standard {
            candidates.viable.clone()
        } else {
            generator.own_index = (cfg.separator != ":").then(|| class_index(&candidates.list, &cfg.separator));
            (0..candidates.list.len() as u32)
                .filter(|&i| has_plugins(class_part(&candidates.list[i as usize], &cfg.separator), &cfg.prefix, |k| ctx.candidate_rule_map.contains_key(k)))
                .collect()
        };
        // The pool in the order of a full run (with the safelist and `*`): this context is then
        // the state of a full run without `@layer` rules — non-pool candidates leave no trace,
        // and the extra ones only order-only traces.
        let safelist = std::mem::take(&mut ctx.changed_content);
        let list = process::merge_candidates(pool.iter().map(|&i| &*candidates.list[i as usize]).collect(), &safelist);
        if ctx.generate_rules(&list).is_err() {
            generator.working = pool;
            return generator;
        }
        generator.working = pool
            .into_iter()
            .filter(|&i| {
                let c = &*candidates.list[i as usize];
                ctx.class_cache.contains_key(c) || c.contains("min-") || c.contains("max-")
            })
            .collect();
        ctx.sorted_rules = Some((ctx.rule_cache.len(), process::sorted_rules(&ctx)));
        ctx.compact_nodes();
        ctx.css.freeze();
        generator.ready = Some(ctx);
        generator
    }

    /// Candidates every `generate` call starts from.
    pub fn working_len(&self) -> usize {
        self.working.len()
    }

    /// Same as `generate(config, blocks, all candidates)`.
    pub fn generate(&self, blocks: &str) -> Result<String, Error> {
        self.generate_excluding(blocks, &HashSet::new())
    }

    /// Same as `generate(config, blocks, all candidates except excluded)`.
    pub fn generate_excluding(&self, blocks: &str, excluded: &HashSet<&str>) -> Result<String, Error> {
        if self.config.content_candidates.is_some() {
            return crate::generate(&self.config, blocks, []);
        }
        let input = format!("{INPUT_PREFIX}{blocks}");
        if let (Some(ready), true) = (&self.ready, excluded.is_empty()) {
            let mut ctx = ready.clone();
            let root = postcss::parse(&mut ctx.css, &input)?;
            if process::prepare_without_layers(&mut ctx, root)? {
                // the rule cache already holds every rule the candidates would produce
                process::finish(&mut ctx, root, &[])?;
                autoprefixer::autoprefix(&mut ctx.css, root)?;
                return Ok(postcss::to_string(&mut ctx.css, root));
            }
        }
        let mut ctx = Context::new(&self.config.inner);
        let root = postcss::parse(&mut ctx.css, &input)?;
        process::prepare(&mut ctx, root)?;
        let list = &self.candidates.list;
        let index = self.own_index.as_ref().unwrap_or(&self.candidates.by_class);
        let mut picked = self.working.clone();
        let prefix = &self.config.inner.prefix;
        for id in ctx.layer_identifiers.take().unwrap_or_default() {
            // class parts that resolve to `id` (`-id` and the prefixed negative included)
            let mut keys = vec![id.clone(), format!("-{id}")];
            if let Some(rest) = id.strip_prefix(prefix.as_str()) {
                keys.push(format!("{prefix}-{rest}"));
            }
            for k in keys {
                let h = hash(&k);
                let start = index.partition_point(|&(x, _)| x < h);
                picked.extend(index[start..].iter().take_while(|&&(x, _)| x == h).map(|&(_, i)| i));
            }
        }
        picked.sort_unstable();
        picked.dedup();
        let tokens: Vec<&str> = picked.iter().map(|&i| &*list[i as usize]).filter(|c| !excluded.contains(c)).collect();
        let safelist = std::mem::take(&mut ctx.changed_content);
        let candidates = process::merge_candidates(tokens, &safelist);
        process::finish(&mut ctx, root, &candidates)?;
        autoprefixer::autoprefix(&mut ctx.css, root)?;
        Ok(postcss::to_string(&mut ctx.css, root))
    }
}
