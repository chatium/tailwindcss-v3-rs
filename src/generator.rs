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
//!
//! `Generator::derive` makes the generator of another config of the account from a base one
//! (see `try_derive`), and every generator keeps the processed output of its generated rules
//! (see `Prefab`), so a call mostly processes its blocks.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

use crate::js::{FxHashSet, FxHasher};
use crate::tw::context::{ConfigRead, Context, PluginRule};
use crate::tw::offsets::Offsets;
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
    /// what `derive` needs (generators made by `new` for a standard config)
    base: Option<Box<BaseInfo>>,
    /// the generated part of the output, processed once (built by the first `generate`)
    prefab: OnceLock<Option<Prefab<'a>>>,
}

/// The full run for empty blocks, kept: its output, and the generated rules before the blocks'
/// place (base, components, utilities) and after it (the variants, appended to the root)
/// processed and printed. A call's blocks then go through the passes on their own and are
/// printed between the two parts.
///
/// That is the full run's output when nothing reaches across the boundaries; `generate_spliced`
/// checks it (otherwise the call runs the whole pipeline on the ready context):
/// - the blocks have no `@tailwind`/`@layer` (the generated rules are the same), and the
///   generated rules before them exist (during the passes a stand-in keeps the blocks from being
///   the root's first child, which `Root.normalize`/`removeChild` treat specially);
/// - `@apply`, `theme()`, `@screen`, duplicate declarations work per rule; the local `@apply`
///   cache skips generated rules anyway;
/// - `@defaults`: the blocks' are empty (they are only removed); no `optimizeUniversalDefaults`;
/// - adjacent rules: neither boundary merges (checked on the rules as they were at that pass);
/// - autoprefixer (with the bundle's data) only changes declarations inside rules and
///   `@supports` params: no declarations at the blocks' top level, no control comments;
/// - the variants get the `raws.before` of the root's last node when appended: the blocks'
///   last one in a call, so the parts are kept per that value;
/// - inferred raws (`rawCache`): autoprefixer asks for some mid-run, from the tree as it is
///   then; those must be settled by the rules before the blocks (`settled_raws`), and a call's
///   root starts with them. The rest are asked for while printing, from the final tree: the
///   parts are kept per the values they were printed with and printed again on a miss.
struct Prefab<'a> {
    ctx: Context<'a>,
    root: crate::postcss::NodeId,
    pre: Vec<crate::postcss::NodeId>,
    post: Vec<crate::postcss::NodeId>,
    /// printed parts (before, after the blocks), each with the inferred raws it was printed with
    /// and the `raws.before` of the rules after the blocks
    parts: std::sync::Mutex<Vec<(Vec<(&'static str, String)>, Option<String>, std::sync::Arc<(String, String)>)>>,
    /// `raws.before` of the rules after the blocks: `append` gives them the root's last node's
    post_before: Option<String>,
    /// the inferred raws the generated rules before the blocks settle on their own (whatever
    /// follows them, before and after autoprefixer): a call's root starts with them, so the raws
    /// autoprefixer asks for mid-run are the full run's
    seeds: Vec<(&'static str, String)>,
    /// the output for blocks without nodes (the root has no trailing raws)
    empty: String,
    /// whether calls with blocks can use the parts
    splittable: bool,
    /// the root's `raws.semicolon` in the empty run
    semicolon: Option<bool>,
    /// shallow copies of the boundary rules as `collapseAdjacentRules` saw them
    pre_last: Option<crate::postcss::NodeId>,
    post_first: Option<crate::postcss::NodeId>,
}


/// The registration of a base generator's config and the candidates it matched.
struct BaseInfo {
    reads: Vec<ConfigRead>,
    rule_plugins: crate::js::FxHashMap<String, Vec<u32>>,
    variant_plugins: Vec<u32>,
    plugins: u32,
    /// offsets right after `createContext`
    offsets: Offsets,
    pool: Vec<u32>,
    /// the candidate list `generateRules` ran over (pool, safelist, `*`)
    list: Vec<String>,
    /// rule map keys and lookup modifiers, interned
    keys: Vec<String>,
    mods: Vec<String>,
    /// per `list` entry: the (key, modifier) lookups of `resolveMatchedPlugins` hitting a key
    plans: Vec<Vec<(u32, u32)>>,
}

impl BaseInfo {
    fn plans(list: &[String], map: &crate::js::FxHashMap<String, Vec<(crate::tw::context::Meta, PluginRule)>>) -> (Vec<String>, Vec<String>, Vec<Vec<(u32, u32)>>) {
        let mut key_ids: crate::js::FxHashMap<&str, u32> = Default::default();
        let mut keys: Vec<String> = Vec::new();
        for k in map.keys() {
            key_ids.insert(k.as_str(), keys.len() as u32);
            keys.push(k.clone());
        }
        let mut mod_ids: crate::js::FxHashMap<String, u32> = Default::default();
        let mut mods: Vec<String> = Vec::new();
        let plans = list
            .iter()
            .map(|cand| {
                let class = if cand == crate::tw::context::NOT_ON_DEMAND { cand.as_str() } else { class_part(cand, ":") };
                crate::tw::generate::lookup_plan(class, "")
                    .into_iter()
                    .filter_map(|(key, m)| {
                        let k = *key_ids.get(key.as_str())?;
                        let n = mods.len() as u32;
                        let id = *mod_ids.entry(m.clone()).or_insert(n);
                        if id == n {
                            mods.push(m);
                        }
                        Some((k, id))
                    })
                    .collect()
            })
            .collect();
        (keys, mods, plans)
    }
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

/// Candidates that produce rules (and the `min-*`/`max-*` ones).
fn working_of(pool: &[u32], candidates: &Candidates, ctx: &Context) -> Vec<u32> {
    pool.iter()
        .copied()
        .filter(|&i| {
            let c = &*candidates.list[i as usize];
            ctx.class_cache.contains_key(c) || c.contains("min-") || c.contains("max-")
        })
        .collect()
}

/// The inferred raws (cache entries) that the nodes `pre` decide on their own: the same whatever
/// follows them (checked with two different tails).
fn settled_raws(ctx: &mut Context, pre: &[crate::postcss::NodeId]) -> Vec<(&'static str, String)> {
    use crate::postcss::{Kind, Node};
    // whitespace nothing generated contains, and both `semicolon` values
    const TAIL_A: &str = "\x0c/*\x0ca\x0c*/\x0c.p\x0c{\x0ca\x0c:\x0cb\x0c;\x0c}\x0c.q\x0c{\x0c}\x0c@m\x0c{\x0c.r\x0c{\x0cc\x0c:\x0cd;\x0c}\x0c}\x0c";
    const TAIL_B: &str = "\x0c\x0c/*a*/\x0c\x0c.p{\x0c\x0ca:b\x0c\x0c}\x0c\x0c.q{}\x0c\x0c@m{\x0c\x0c.r{\x0c\x0cc:d\x0c\x0c}\x0c\x0c}\x0c\x0c";
    let tree = |ctx: &mut Context, tail: Option<&str>| {
        let extra = match tail {
            Some(t) => {
                let r = postcss::parse(&mut ctx.css, t).expect("tail parses");
                ctx.css.children(r).to_vec()
            }
            None => Vec::new(),
        };
        let x = ctx.css.root();
        let ask = ctx.css.add(Node::new(Kind::Rule));
        ctx.css[ask].parent = Some(x);
        let mut list = pre.to_vec();
        list.extend(extra);
        list.push(ask);
        ctx.css[x].nodes = Some(list);
        (x, ask)
    };
    let (x, ask) = tree(ctx, None);
    let (_, ask_a) = tree(ctx, Some(TAIL_A));
    let (_, ask_b) = tree(ctx, Some(TAIL_B));
    let mut settled = Vec::new();
    for k in postcss::INFERRED_RAWS {
        let v = postcss::infer_kind(&mut ctx.css, ask, k);
        if v == postcss::infer_kind(&mut ctx.css, ask_a, k) && v == postcss::infer_kind(&mut ctx.css, ask_b, k) {
            settled.push(k);
        }
    }
    postcss::raw_cache(&ctx.css, x).into_iter().filter(|(k, _)| settled.contains(k)).collect()
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
        let mut generator = Generator { config, candidates, working: Vec::new(), own_index: None, ready: None, base: None, prefab: OnceLock::new() };
        if config.content_candidates.is_some() || process::prepare(&mut ctx, root).is_err() {
            // `{ config }` ignores the candidates; a broken config fails the same way every time
            return generator;
        }
        let keys = default_keys();
        let standard = cfg.separator == ":" && ctx.candidate_rule_map.keys().all(|k| keys.contains(k));
        let registration = (
            std::mem::take(&mut ctx.reads),
            std::mem::take(&mut ctx.rule_plugins),
            std::mem::take(&mut ctx.variant_plugins),
            ctx.current_plugin,
            ctx.offsets.clone(),
        );
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
        generator.working = working_of(&pool, candidates, &ctx);
        if standard {
            let (reads, rule_plugins, variant_plugins, plugins, offsets) = registration;
            let (keys, mods, plans) = BaseInfo::plans(&list, &ctx.candidate_rule_map);
            generator.base = Some(Box::new(BaseInfo { reads, rule_plugins, variant_plugins, plugins, offsets, pool, list, keys, mods, plans }));
        }
        ctx.sorted_rules = Some((ctx.rule_cache.len(), process::sorted_rules(&ctx)));
        ctx.compact_nodes();
        ctx.css.freeze();
        generator.ready = Some(ctx);
        generator
    }

    /// A generator for another config of the same account, reusing this one's matching: only the
    /// candidates whose lookups differ between the two configs are matched again. Same output as
    /// `Generator::new(config, candidates)`; falls back to it when the configs differ in more than
    /// theme values (prefix, separator, important, corePlugins, variants, darkMode, safelist, …).
    pub fn derive<'b>(&self, config: &'b Config) -> Generator<'b>
    where
        'a: 'b,
    {
        self.try_derive(config).unwrap_or_else(|| Generator::new(config, self.candidates))
    }

    fn try_derive<'b>(&self, config: &'b Config) -> Option<Generator<'b>>
    where
        'a: 'b,
    {
        let (base, info) = (self.ready.as_ref()?, self.base.as_deref()?);
        if config.content_candidates.is_some() {
            return None;
        }
        let (b, c) = (&*self.config.inner, &*config.inner);
        let theme = |cfg: &crate::tw::TwConfig, k: &str| cfg.value.get("theme").get(k);
        let same = |x: &crate::js::Value, y: &crate::js::Value| crate::js::deep_eq(x, y);
        if b.prefix != c.prefix
            || b.separator != c.separator
            || b.core_plugins != c.core_plugins
            || b.blocklist != c.blocklist
            || b.generalized_modifiers != c.generalized_modifiers
            || !same(&b.important, &c.important)
            || !same(&theme(b, "opacity"), &theme(c, "opacity"))
            || !same(&b.value.get("darkMode"), &c.value.get("darkMode"))
            || !same(&b.value.get("safelist"), &c.value.get("safelist"))
        {
            return None;
        }
        // the other config's registration
        let mut cx = Context::new(c);
        let root = postcss::parse(&mut cx.css, INPUT_PREFIX).ok()?;
        process::prepare(&mut cx, root).ok()?;
        if cx.offsets != info.offsets || cx.current_plugin != info.plugins || cx.variant_plugins != info.variant_plugins {
            return None;
        }
        // per plugin: same reads, only values/modifiers reads differ, or other reads differ
        #[derive(Clone, Copy, PartialEq)]
        enum Diff {
            Same,
            Values,
            Other,
        }
        let mut diff = vec![Diff::Same; info.plugins as usize];
        {
            fn group(reads: &[ConfigRead], n: u32) -> Vec<Vec<&ConfigRead>> {
                let mut g: Vec<Vec<&ConfigRead>> = vec![Vec::new(); n as usize];
                for r in reads {
                    g[r.plugin as usize].push(r);
                }
                g
            }
            let (gb, gc) = (group(&info.reads, info.plugins), group(&cx.reads, info.plugins));
            for p in 0..info.plugins as usize {
                let (rb, rc) = (&gb[p], &gc[p]);
                if rb.len() != rc.len() || rb.iter().zip(rc).any(|(x, y)| x.path != y.path || x.values_only != y.values_only) {
                    diff[p] = Diff::Other;
                    continue;
                }
                for (x, y) in rb.iter().zip(rc) {
                    if !same(&x.value, &y.value) {
                        let d = if x.values_only { Diff::Values } else { Diff::Other };
                        if d == Diff::Other || diff[p] == Diff::Same {
                            diff[p] = d;
                        }
                    }
                }
            }
        }
        if info.variant_plugins.iter().any(|&p| diff[p as usize] != Diff::Same) {
            return None;
        }

        // same rule map layout; entries whose behaviour may differ
        if cx.candidate_rule_map.len() != base.candidate_rule_map.len() {
            return None;
        }
        let mut changed: crate::js::FxHashMap<&str, Vec<Diff>> = Default::default();
        for (key, eb) in base.candidate_rule_map.iter() {
            let ec = cx.candidate_rule_map.get(key)?;
            let plugins = info.rule_plugins.get(key)?;
            if ec.len() != eb.len() || plugins.len() != eb.len() {
                return None;
            }
            let mut per = Vec::with_capacity(eb.len());
            for (j, ((mb, rb), (mc, rc))) in eb.iter().zip(ec.iter()).enumerate() {
                if format!("{:?}", mb.sort) != format!("{:?}", mc.sort) || mb.layer != mc.layer || mb.options.is_some() != mc.options.is_some() {
                    return None;
                }
                if let (Some(ob), Some(oc)) = (&mb.options, &mc.options) {
                    let tb: Vec<_> = ob.types.iter().map(|t| (&t.ty, t.prefer_on_conflict)).collect();
                    let tc: Vec<_> = oc.types.iter().map(|t| (&t.ty, t.prefer_on_conflict)).collect();
                    if tb != tc
                        || ob.respect_prefix != oc.respect_prefix
                        || ob.respect_important != oc.respect_important
                        || ob.preserve_source != oc.preserve_source
                        || ob.has_types != oc.has_types
                        || ob.supports_negative_values != oc.supports_negative_values
                    {
                        return None;
                    }
                }
                let d = match (rb, rc, diff[plugins[j] as usize]) {
                    (_, _, Diff::Same) => Diff::Same,
                    (PluginRule::Static(x), PluginRule::Static(y), _) => {
                        if base.css.same_tree(*x, &cx.css, *y) { Diff::Same } else { Diff::Other }
                    }
                    (PluginRule::Dynamic(_), PluginRule::Dynamic(_), Diff::Values) => {
                        let (ob, oc) = (mb.options.as_ref()?, mc.options.as_ref()?);
                        if same(&ob.values, &oc.values) && same(&ob.modifiers, &oc.modifiers) { Diff::Same } else { Diff::Values }
                    }
                    (PluginRule::Dynamic(_), PluginRule::Dynamic(_), _) => Diff::Other,
                    _ => return None,
                };
                per.push(d);
            }
            if per.iter().any(|d| *d != Diff::Same) {
                changed.insert(key.as_str(), per);
            }
        }
        // candidates whose lookups differ
        let changed_by_id: Vec<Option<&Vec<Diff>>> = info.keys.iter().map(|k| changed.get(k.as_str())).collect();
        // per changed values entry: the top-level `values` keys that differ (None: compare all)
        let mut delta: crate::js::FxHashMap<(u32, u32), Option<crate::js::FxHashSet<String>>> = Default::default();
        let mut memo: crate::js::FxHashMap<(u32, u32, u32), bool> = Default::default();
        let mut entry_differs = |k: u32, j: usize, mi: u32, d: Diff| -> bool {
            match d {
                Diff::Same => false,
                Diff::Other => true,
                Diff::Values => *memo.entry((k, j as u32, mi)).or_insert_with(|| {
                    let (key, m) = (info.keys[k as usize].as_str(), info.mods[mi as usize].as_str());
                    let (ob, oc) = (base.candidate_rule_map[key][j].0.options.as_ref().unwrap(), cx.candidate_rule_map[key][j].0.options.as_ref().unwrap());
                    let dv = delta.entry((k, j as u32)).or_insert_with(|| {
                        if !same(&ob.modifiers, &oc.modifiers) {
                            return None;
                        }
                        let (crate::js::Value::Obj(vb), crate::js::Value::Obj(vc)) = (&ob.values, &oc.values) else { return None };
                        let mut d = crate::js::FxHashSet::default();
                        for (k, v) in vb.iter() {
                            if !vc.get(k).is_some_and(|w| same(v, w)) {
                                d.insert(k.to_string());
                            }
                        }
                        for (k, _) in vc.iter() {
                            if vb.get(k).is_none() {
                                d.insert(k.to_string());
                            }
                        }
                        Some(d)
                    });
                    // the keys `coerceValue`/`getMatchingTypes` can look up for `m`: `m` and its
                    // prefixes before a `/`, each also without a leading `-`; `DEFAULT`; `[…]`
                    if let Some(dv) = dv {
                        let touches = |p: &str| dv.contains(p) || p.strip_prefix('-').is_some_and(|q| dv.contains(q));
                        let mut hit = dv.contains("DEFAULT") || dv.iter().any(|x| x.starts_with('[')) || touches(m);
                        for (i, ch) in m.char_indices() {
                            if ch == '/' && touches(&m[..i]) {
                                hit = true;
                            }
                        }
                        if !hit {
                            return false;
                        }
                    }
                    let cb = crate::tw::plugin_utils::coerce_value(&ob.types, m, ob, b);
                    let cc = crate::tw::plugin_utils::coerce_value(&oc.types, m, oc, c);
                    let eq3 = |x: &(crate::js::Value, String, Option<String>), y: &(crate::js::Value, String, Option<String>)| {
                        same(&x.0, &y.0) && x.1 == y.1 && x.2 == y.2
                    };
                    let coerce_same = match (&cb, &cc) {
                        (None, None) => true,
                        (Some(x), Some(y)) => eq3(x, y),
                        _ => false,
                    };
                    let tb = crate::tw::plugin_utils::get_matching_types(&ob.types, m, ob, b);
                    let tc = crate::tw::plugin_utils::get_matching_types(&oc.types, m, oc, c);
                    !(coerce_same && tb.len() == tc.len() && tb.iter().zip(&tc).all(|(x, y)| eq3(x, y)))
                }),
            }
        };
        let mut affected: Vec<&str> = Vec::new();
        for (cand, plan) in info.list.iter().zip(&info.plans) {
            let hit = plan.iter().any(|&(k, mi)| {
                changed_by_id[k as usize].is_some_and(|per| per.iter().enumerate().any(|(j, d)| entry_differs(k, j, mi, *d)))
            });
            if hit {
                let class = if cand == crate::tw::context::NOT_ON_DEMAND { cand.as_str() } else { class_part(cand, ":") };
                // their effects are not local to the candidate: unit cache, arbitrary property order
                if cand.contains("min-[") || cand.contains("max-[") || class.starts_with('[') {
                    return None;
                }
                affected.push(cand);
            }
        }
        // this context with the other config's differing entries, the affected candidates rematched
        let mut ctx = base.clone();
        ctx.cfg = c;
        {
            let map = std::sync::Arc::make_mut(&mut ctx.candidate_rule_map);
            for (key, per) in &changed {
                let entries = map.get_mut(*key)?;
                for (j, d) in per.iter().enumerate() {
                    if *d == Diff::Same {
                        continue;
                    }
                    let (meta, rule) = cx.candidate_rule_map[*key][j].clone();
                    let rule = match rule {
                        PluginRule::Static(n) => PluginRule::Static(ctx.css.copy_from(&cx.css, n)),
                        r => r,
                    };
                    entries[j] = (meta, rule);
                }
            }
        }
        if !affected.is_empty() {
            {
                let cc = std::sync::Arc::make_mut(&mut ctx.class_cache);
                let nc = std::sync::Arc::make_mut(&mut ctx.not_class_cache);
                let rc = std::sync::Arc::make_mut(&mut ctx.candidate_rule_cache);
                for a in &affected {
                    cc.remove(*a);
                    nc.remove(*a);
                    rc.remove(*a);
                }
            }
            let list: Vec<String> = affected.iter().map(|s| s.to_string()).collect();
            ctx.generate_rules(&list).ok()?;
            // the rule cache in candidate order, as a full run builds it
            let old = std::mem::take(std::sync::Arc::make_mut(&mut ctx.rule_cache));
            let crc = std::sync::Arc::make_mut(&mut ctx.candidate_rule_cache);
            let mut rule_cache = Vec::with_capacity(old.len());
            for cand in &info.list {
                if let Some(idxs) = crc.get_mut(cand.as_str()) {
                    for i in idxs.iter_mut() {
                        let e = old[*i].clone();
                        *i = rule_cache.len();
                        rule_cache.push(e);
                    }
                }
            }
            ctx.rule_cache = std::sync::Arc::new(rule_cache);
            ctx.sorted_rules = Some((ctx.rule_cache.len(), process::sorted_rules(&ctx)));
        }
        let working = working_of(&info.pool, self.candidates, &ctx);
        Some(Generator { config, candidates: self.candidates, working, own_index: None, ready: Some(ctx), base: None, prefab: OnceLock::new() })
    }

    fn build_prefab(&self) -> Option<Prefab<'a>> {
        use crate::postcss::{Kind, Node};
        let ready = self.ready.as_ref()?;
        if crate::tw::config::flag_enabled(&self.config.inner.value, "optimizeUniversalDefaults") {
            return None;
        }
        let mut ctx = ready.clone();
        let root = postcss::parse(&mut ctx.css, INPUT_PREFIX).ok()?;
        if !process::prepare_without_layers(&mut ctx, root).ok()? {
            return None;
        }
        process::expand_tailwind_at_rules(&mut ctx, root, &[]).ok()?;
        process::partition_apply_at_rules(&mut ctx, root);
        ctx.expand_apply_at_rules(root).ok()?;
        ctx.evaluate_tailwind_functions(root).ok()?;
        process::substitute_screen_at_rules(&mut ctx, root).ok()?;
        process::resolve_defaults_at_rules(&mut ctx, root).ok()?;
        // the blocks go between the generated rules and the variants appended after them
        let kids = ctx.css.children(root).to_vec();
        let at = kids.iter().position(|&n| ctx.css[n].tw().and_then(|t| t.layer.as_deref()) == Some("variants")).unwrap_or(kids.len());
        let boundary = |ctx: &mut Context, n: Option<&crate::postcss::NodeId>| {
            let n = *n?;
            let node = &ctx.css[n];
            if !matches!(node.kind, Kind::Rule | Kind::AtRule) {
                return None;
            }
            let mut copy = Node::new(node.kind);
            copy.name = node.name.clone();
            copy.params = node.params.clone();
            copy.selector = node.selector.clone();
            Some(ctx.css.add(copy))
        };
        let pre_last = if at > 0 { boundary(&mut ctx, kids.get(at - 1)) } else { None };
        let post_first = boundary(&mut ctx, kids.get(at));
        let first_variant = kids.get(at).copied();
        process::collapse_adjacent_rules(&mut ctx, root);
        process::collapse_duplicate_declarations(&mut ctx, root);
        let mut control = false;
        ctx.css.walk(root, &mut |c, n| {
            if c[n].kind == Kind::Comment && c[n].text.to_ascii_lowercase().contains("autoprefixer:") {
                control = true;
            }
            crate::postcss::Flow::Continue
        });
        let split = |ctx: &Context| {
            let kids = ctx.css.children(root).to_vec();
            let at = match first_variant {
                Some(v) => kids.iter().position(|&n| n == v),
                None => Some(kids.len()),
            };
            (kids, at)
        };
        let seeds_before = match split(&ctx) {
            (kids, Some(at)) => settled_raws(&mut ctx, &kids[..at]),
            _ => Vec::new(),
        };
        autoprefixer::autoprefix(&mut ctx.css, root).ok()?;
        // raws asked for while the tree was still changing must not depend on the blocks
        let asked_mid_run = postcss::raw_cache(&ctx.css, root);
        let (kids, at) = split(&ctx);
        let seeds: Vec<(&'static str, String)> = match at {
            Some(at) => settled_raws(&mut ctx, &kids[..at]).into_iter().filter(|e| seeds_before.contains(e)).collect(),
            None => Vec::new(),
        };
        let semicolon = ctx.css[root].raws.semicolon;
        // splitting needs: the variants did not merge into the rule before them, no control
        // comments, parts that do not depend on their position (`semicolon` only affects these)
        let splittable = at.is_some()
            && !control
            && asked_mid_run.iter().all(|e| seeds.contains(e))
            && !kids.iter().any(|&n| ctx.css[n].kind == Kind::Decl || (ctx.css[n].kind == Kind::AtRule && ctx.css[n].nodes.is_none()));
        let at = at.unwrap_or(0);
        // the variants all got the `before` of the root's last node when appended
        let post_before = kids.get(at).and_then(|&n| ctx.css[n].raws.before.clone());
        let splittable = splittable && kids[at..].iter().all(|&n| ctx.css[n].raws.before == post_before);
        let mut known = Vec::new();
        let empty = if splittable {
            // printed on a root whose raws start from the seeds (as a call's does): the seeds are
            // what this root infers anyway, so this is also the output for empty blocks
            postcss::set_raw_cache(&mut ctx.css, root, seeds.clone());
            let (all, after) = postcss::to_string_parts(&mut ctx.css, root);
            known.push((postcss::raw_cache(&ctx.css, root), post_before.clone(), std::sync::Arc::new((all[..at].concat(), all[at..].concat()))));
            all.concat() + &after
        } else {
            postcss::to_string(&mut ctx.css, root)
        };
        ctx.css.freeze();
        Some(Prefab {
            ctx,
            root,
            pre: kids[..at].to_vec(),
            post: kids[at..].to_vec(),
            splittable,
            semicolon,
            parts: std::sync::Mutex::new(known),
            post_before,
            seeds,
            empty,
            pre_last,
            post_first,
        })
    }

    /// The fast path without the prefab: the ready context's clone through the whole pipeline;
    /// `None` when the input has `@layer` rules.
    fn generate_ready(&self, ready: &Context<'a>, input: &str) -> Result<Option<String>, Error> {
        let mut ctx = ready.clone();
        let root = postcss::parse(&mut ctx.css, input)?;
        if !process::prepare_without_layers(&mut ctx, root)? {
            return Ok(None);
        }
        // the rule cache already holds every rule the candidates would produce
        process::finish(&mut ctx, root, &[])?;
        autoprefixer::autoprefix(&mut ctx.css, root)?;
        Ok(Some(postcss::to_string(&mut ctx.css, root)))
    }

    /// `generate` with the prefab's parts; `Err` when a condition does not hold.
    fn generate_spliced(&self, pf: &Prefab<'a>, input: &str) -> Result<String, &'static str> {
        use crate::postcss::{Flow, Kind};
        let mut ctx = pf.ctx.clone();
        let root = postcss::parse(&mut ctx.css, input).map_err(|_| "parse")?;
        postcss::set_raw_cache(&mut ctx.css, root, pf.seeds.clone());
        if !process::prepare_without_layers(&mut ctx, root).map_err(|_| "prepare")? {
            return Err("layers");
        }
        // the three `@tailwind` rules of the prefix, and no others
        let kids = ctx.css.children(root).to_vec();
        let mut tailwind = 0;
        ctx.css.walk_at_rules_named(root, "tailwind", &mut |_, _| {
            tailwind += 1;
            Flow::Continue
        });
        if tailwind != 3 || kids.len() < 3 || !kids[..3].iter().all(|&n| ctx.css[n].kind == Kind::AtRule && ctx.css[n].name == "tailwind") {
            return Err("tailwind directives");
        }
        // removed the way the full run removes them, after the generated rules: those come first
        // (with none before the blocks, the root would hand the first rule's `before` over)
        if pf.pre.is_empty() {
            return Err("nothing before the blocks");
        }
        for &n in &kids[..3] {
            ctx.css.detach(n);
        }
        // what the variants get appended after: the blocks' last rule as it is at that point
        let post_before = ctx.css.children(root).last().and_then(|&n| ctx.css[n].raws.before.clone());
        // the generated rules stand in front of the blocks during the passes (`Root.normalize`
        // and `removeChild` treat the root's first child specially): a raw-less stand-in
        let stand_in = ctx.css.add(crate::postcss::Node::new(Kind::Comment));
        ctx.css[stand_in].parent = Some(root);
        ctx.css[root].nodes.as_mut().unwrap().insert(0, stand_in);
        process::partition_apply_at_rules(&mut ctx, root);
        ctx.expand_apply_at_rules(root).map_err(|_| "apply")?;
        ctx.evaluate_tailwind_functions(root).map_err(|_| "functions")?;
        process::substitute_screen_at_rules(&mut ctx, root).map_err(|_| "screen")?;
        // `@defaults`: the blocks' empty ones are just removed
        let mut defaults = Vec::new();
        ctx.css.walk_at_rules_named(root, "defaults", &mut |_, r| {
            defaults.push(r);
            Flow::Continue
        });
        for r in defaults {
            if ctx.css[r].nodes.as_ref().is_some_and(|n| !n.is_empty()) {
                return Err("defaults");
            }
            ctx.css.remove(r);
        }
        let Some(&first) = ctx.css.children(root).get(1) else {
            // only the generated rules: the output for empty blocks with this root's trailing raws
            if ctx.css[root].raws.semicolon != pf.semicolon || (kids.len() > 3 && post_before != pf.post_before) {
                return Err("root raws");
            }
            let after = ctx.css[root].raws.after.clone().unwrap_or_default();
            return Ok(format!("{}{after}", pf.empty));
        };
        if !pf.splittable {
            return Err("not splittable");
        }
        let mergeable = |ctx: &Context, n: crate::postcss::NodeId| {
            matches!(ctx.css[n].kind, Kind::Rule | Kind::AtRule) && !(ctx.css[n].kind == Kind::AtRule && ctx.css[n].name == "font-face")
        };
        if let Some(cur) = pf.pre_last {
            if mergeable(&ctx, first) && process::collapses_into(&ctx.css, cur, first) {
                return Err("merge before");
            }
        }
        process::collapse_adjacent_rules(&mut ctx, root);
        process::collapse_duplicate_declarations(&mut ctx, root);
        let last = *ctx.css.children(root).last().ok_or("empty")?;
        if let Some(next) = pf.post_first {
            if matches!(ctx.css[last].kind, Kind::Rule | Kind::AtRule) && mergeable(&ctx, next) && process::collapses_into(&ctx.css, last, next) {
                return Err("merge after");
            }
        }
        let mut local = true;
        for &n in ctx.css.children(root) {
            if ctx.css[n].kind == Kind::Decl {
                local = false;
            }
        }
        ctx.css.walk(root, &mut |c, n| {
            if c[n].kind == Kind::Comment && c[n].text.to_ascii_lowercase().contains("autoprefixer:") {
                local = false;
            }
            Flow::Continue
        });
        if !local {
            return Err("autoprefixer");
        }
        autoprefixer::autoprefix(&mut ctx.css, root).map_err(|_| "autoprefix")?;
        // a raw first asked for mid-run would have been inferred from another tree
        if postcss::raw_cache(&ctx.css, root).len() != pf.seeds.len() {
            return Err("raws mid-run");
        }
        if ctx.css.children(root).first() != Some(&stand_in) {
            return Err("stand-in moved");
        }
        ctx.css.detach(stand_in);
        let blocks = ctx.css.children(root).to_vec();
        blocks.first().ok_or("empty")?;
        let mut spliced = pf.pre.clone();
        spliced.extend_from_slice(&blocks);
        spliced.extend_from_slice(&pf.post);
        ctx.css[root].nodes = Some(spliced);
        // parts printed with this root's raws (the ones they ask for, inferred on the whole tree
        // for a raw-less node of this root that is not in it)
        let ask = ctx.css.add(crate::postcss::Node::new(Kind::Rule));
        ctx.css[ask].parent = Some(root);
        let value_of = |ctx: &mut Context, k: &'static str| {
            postcss::infer_kind(&mut ctx.css, ask, k);
            postcss::raw_cache(&ctx.css, root).into_iter().find(|(k2, _)| *k2 == k).map(|x| x.1)
        };
        let known: Vec<_> = pf.parts.lock().unwrap().clone();
        let found = known
            .into_iter()
            .find(|(deps, before, _)| *before == post_before && deps.iter().all(|(k, v)| value_of(&mut ctx, k).as_deref() == Some(v.as_str())));
        let parts = match found {
            Some((_, _, p)) => p,
            None => {
                for &n in &pf.post {
                    ctx.css[n].raws.before = post_before.clone();
                }
                let mut rounds = 0;
                let all = loop {
                    rounds += 1;
                    if rounds > postcss::INFERRED_RAWS.len() + 1 {
                        return Err("raws loop");
                    }
                    let cache = postcss::raw_cache(&ctx.css, root);
                    postcss::set_raw_cache(&mut ctx.css, pf.root, cache.clone());
                    let (all, _) = postcss::to_string_parts(&mut ctx.css, pf.root);
                    let asked: Vec<&'static str> =
                        postcss::raw_cache(&ctx.css, pf.root).into_iter().map(|(k, _)| k).filter(|k| !cache.iter().any(|(k2, _)| k2 == k)).collect();
                    if asked.is_empty() {
                        break all;
                    }
                    for k in asked {
                        postcss::infer_kind(&mut ctx.css, ask, k);
                    }
                };
                let deps = postcss::raw_cache(&ctx.css, pf.root);
                let at = pf.pre.len();
                let p = std::sync::Arc::new((all[..at].concat(), all[at..].concat()));
                pf.parts.lock().unwrap().push((deps, post_before.clone(), p.clone()));
                p
            }
        };
        Ok(postcss::to_string_spliced(&mut ctx.css, root, (pf.pre.len(), &parts.0), (pf.post.len(), &parts.1)))
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
            if let Some(pf) = self.prefab.get_or_init(|| self.build_prefab()) {
                if let Ok(out) = self.generate_spliced(pf, &input) {
                    return Ok(out);
                }
            }
            if let Some(out) = self.generate_ready(ready, &input)? {
                return Ok(out);
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
