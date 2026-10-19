//! `processTailwindFeatures.js` and the passes it runs.

use std::sync::Arc;

use crate::js::Value;
use crate::postcss::{Add, CssError, Flow, Kind, NodeId, Source};
use crate::selector::{self, SFlow, SKind, Sel};

use super::context::{Context, PluginApi, Style};
use super::core_plugins::{self as cp};
use super::offsets::Layer;
use super::util::{build_media_query, normalize_screens};
use super::TwConfig;

pub type Result<T> = std::result::Result<T, CssError>;

fn plain(e: String) -> CssError {
    CssError::plain(e)
}

/// `normalizeTailwindDirectives(root)` — returns whether there are `@tailwind` directives
fn normalize_tailwind_directives(ctx: &mut Context, root: NodeId) -> Result<()> {
    let mut tailwind_directives: Vec<String> = Vec::new();
    let mut layer_directives: Vec<NodeId> = Vec::new();
    let css = &mut ctx.css;
    css.walk_at_rules(root, &mut |css, at| {
        let name = css[at].name.clone();
        if name == "import" {
            let p = css[at].params.clone();
            let map = |x: &str| -> Option<&'static str> {
                match x {
                    "\"tailwindcss/base\"" | "'tailwindcss/base'" => Some("base"),
                    "\"tailwindcss/components\"" | "'tailwindcss/components'" => Some("components"),
                    "\"tailwindcss/utilities\"" | "'tailwindcss/utilities'" => Some("utilities"),
                    "\"tailwindcss/screens\"" | "'tailwindcss/screens'" | "\"tailwindcss/variants\"" | "'tailwindcss/variants'" => Some("variants"),
                    _ => None,
                }
            };
            if let Some(n) = map(&p) {
                css[at].name = "tailwind".into();
                css[at].params = n.into();
            }
        }
        if css[at].name == "tailwind" {
            if css[at].params == "screens" {
                css[at].params = "variants".into();
            }
            tailwind_directives.push(css[at].params.clone());
        }
        if matches!(css[at].name.as_str(), "layer" | "responsive" | "variants") {
            layer_directives.push(at);
        }
        Flow::Continue
    });
    let has = |d: &str| tailwind_directives.iter().any(|x| x == d);
    if !has("base") || !has("components") || !has("utilities") {
        for rule in layer_directives {
            let name = css[rule].name.clone();
            let params = css[rule].params.clone();
            if name == "layer" && matches!(params.as_str(), "base" | "components" | "utilities") {
                if !has(&params) {
                    return Err(css.error(rule, format!("`@layer {params}` is used but no matching `@tailwind {params}` directive is present.")));
                }
            } else if name == "responsive" {
                if !has("utilities") {
                    return Err(css.error(rule, "`@responsive` is used but `@tailwind utilities` is missing."));
                }
            } else if name == "variants" && !has("utilities") {
                return Err(css.error(rule, "`@variants` is used but `@tailwind utilities` is missing."));
            }
        }
    }
    Ok(())
}

/// `partitionApplyAtRules()(root)`
pub fn partition_apply_at_rules(ctx: &mut Context, root: NodeId) {
    let css = &mut ctx.css;
    let mut parents: Vec<NodeId> = Vec::new();
    css.walk_at_rules_named(root, "apply", &mut |css, r| {
        if let Some(p) = css[r].parent {
            if !parents.contains(&p) {
                parents.push(p);
            }
        }
        Flow::Continue
    });
    for rule in parents {
        let mut groups: Vec<Vec<NodeId>> = Vec::new();
        let mut last: Vec<NodeId> = Vec::new();
        for &n in css.children(rule).to_vec().iter() {
            if css[n].kind == Kind::AtRule && css[n].name == "apply" {
                if !last.is_empty() {
                    groups.push(std::mem::take(&mut last));
                }
                groups.push(vec![n]);
            } else {
                last.push(n);
            }
        }
        if !last.is_empty() {
            groups.push(last);
        }
        if groups.len() == 1 {
            continue;
        }
        for group in groups.into_iter().rev() {
            let clone = css.clone_empty(rule);
            css.append(clone, Add::Nodes(group));
            css.after(rule, Add::Node(clone));
        }
        css.remove(rule);
    }
}

fn extract_variant_at_rules(ctx: &mut Context, node: NodeId) {
    let mut found: Vec<NodeId> = Vec::new();
    ctx.css.walk_at_rules(node, &mut |css, at| {
        if matches!(css[at].name.as_str(), "responsive" | "variants") {
            found.push(at);
        }
        Flow::Continue
    });
    // the JS mutates during the walk; nested variant at-rules are handled recursively first
    for at in found {
        if ctx.css[at].parent.is_none() {
            continue;
        }
        extract_variant_at_rules(ctx, at);
        let nodes = ctx.css.children(at).to_vec();
        ctx.css.before(at, Add::Nodes(nodes));
        ctx.css.remove(at);
    }
}

enum LayerPlugin {
    Base(NodeId),
    Components(NodeId),
    Utilities(NodeId),
}

fn collect_layer_plugins(ctx: &mut Context, root: NodeId) -> Vec<LayerPlugin> {
    let mut plugins = Vec::new();
    for n in ctx.css.children(root).to_vec() {
        if ctx.css[n].kind == Kind::AtRule && matches!(ctx.css[n].name.as_str(), "responsive" | "variants") {
            ctx.css[n].name = "layer".into();
            ctx.css[n].params = "utilities".into();
        }
    }
    let mut layers: Vec<NodeId> = Vec::new();
    ctx.css.walk_at_rules_named(root, "layer", &mut |_, l| {
        layers.push(l);
        Flow::Continue
    });
    // the walk still descends into a layer after the callback removed it, so nested layers
    // are processed too (in document order)
    let mut removed: Vec<NodeId> = Vec::new();
    for l in layers {
        extract_variant_at_rules(ctx, l);
        let params = ctx.css[l].params.clone();
        let nodes = ctx.css.children(l).to_vec();
        match params.as_str() {
            "base" => {
                plugins.extend(nodes.into_iter().map(LayerPlugin::Base));
                ctx.css.remove(l);
                removed.push(l);
            }
            "components" => {
                plugins.extend(nodes.into_iter().map(LayerPlugin::Components));
                ctx.css.remove(l);
                removed.push(l);
            }
            "utilities" => {
                plugins.extend(nodes.into_iter().map(LayerPlugin::Utilities));
                ctx.css.remove(l);
                removed.push(l);
            }
            _ => {}
        }
    }
    plugins
}

/// `createContext(tailwindConfig, [], root)`
pub fn create_context<'c>(cfg: &'c TwConfig, ctx: &mut Context<'c>, root: NodeId) -> Result<()> {
    let layer_plugins = collect_layer_plugins(ctx, root);
    let mut variant_list: Vec<String> = Vec::new();
    {
        let mut api = PluginApi { ctx, variant_list: &mut variant_list };
        fn run(api: &mut PluginApi, plugin: impl FnOnce(&mut PluginApi) -> std::result::Result<(), String>) -> Result<()> {
            plugin(api).map_err(plain)
        }
        for (name, plugin) in cp::core_plugins() {
            if cfg.core_plugins.iter().any(|p| p == name) {
                run(&mut api, plugin)?;
            }
        }
        let before: [cp::CorePlugin; 6] =
            [cp::child_variant, cp::pseudo_element_variants, cp::pseudo_class_variants, cp::has_variants, cp::aria_variants, cp::data_variants];
        for p in before {
            run(&mut api, p)?;
        }
        let dm = cfg.value.get("darkMode");
        let legacy = dm == Value::str("class") || dm.as_arr().is_some_and(|a| a.first() == Some(&Value::str("class")));
        let after: Vec<cp::CorePlugin> = if legacy {
            vec![
                cp::supports_variants,
                cp::reduced_motion_variants,
                cp::prefers_contrast_variants,
                cp::dark_variants,
                cp::screen_variants,
                cp::orientation_variants,
                cp::direction_variants,
                cp::forced_colors_variants,
                cp::print_variant,
            ]
        } else {
            vec![
                cp::supports_variants,
                cp::reduced_motion_variants,
                cp::prefers_contrast_variants,
                cp::screen_variants,
                cp::orientation_variants,
                cp::direction_variants,
                cp::dark_variants,
                cp::forced_colors_variants,
                cp::print_variant,
            ]
        };
        for p in after {
            run(&mut api, p)?;
        }
        for lp in layer_plugins {
            match lp {
                LayerPlugin::Base(n) => api.add_base(vec![Style::Node(n)]).map_err(plain)?,
                LayerPlugin::Components(n) => api.add_components(vec![Style::Node(n)], true, false).map_err(plain)?,
                LayerPlugin::Utilities(n) => api.add_utilities(vec![Style::Node(n)], true, false).map_err(plain)?,
            }
        }
    }
    ctx.finish_variants(&variant_list);

    // safelist
    if let Value::Arr(list) = cfg.value.get("safelist") {
        let mut has_checks = false;
        for v in list.iter().filter(|v| v.truthy()) {
            match v {
                Value::Str(s) => ctx.changed_content.push(s.to_string()),
                _ => has_checks = true,
            }
        }
        if has_checks {
            // `check.pattern.source.includes('!')` throws for patterns that are not RegExps
            return Err(plain("Cannot read properties of undefined (reading 'includes')".into()));
        }
    }
    Ok(())
}

/// `cloneNodes(nodes, source, { layer })`
fn clone_nodes(ctx: &mut Context, nodes: &[NodeId], source: Option<Source>, layer: &str) -> Vec<NodeId> {
    let mut out = Vec::new();
    for &n in nodes {
        let c = ctx.css.clone_node(n);
        ctx.css[c].tw_mut().layer = Some(layer.to_string());
        if let Some(src) = &source {
            fn traverse(css: &mut crate::postcss::Css, n: NodeId, src: &Source) {
                let preserve = css[n].tw().and_then(|t| t.preserve_source) == Some(true) && css[n].source.is_some();
                if preserve {
                    return;
                }
                css[n].source = Some(src.clone());
                for c in css.children(n).to_vec() {
                    traverse(css, c, src);
                }
            }
            traverse(&mut ctx.css, c, src);
        }
        out.push(c);
    }
    out
}

/// `getClassCandidates` over the candidates as one raw content string (joined with spaces,
/// split on spaces per trimmed line), deduplicated, plus the non-on-demand key, sorted like JS
/// strings. Candidates without whitespace come out as they are.
pub fn candidates_from_tokens<'a>(tokens: impl IntoIterator<Item = &'a str>, safelist: &[String]) -> Vec<String> {
    let not_on_demand = super::context::NOT_ON_DEMAND;
    let mut set: Vec<String> = vec![not_on_demand.to_string()];
    let mut in_set: crate::js::FxHashSet<String> = Default::default();
    let mut add = |tok: &str, set: &mut Vec<String>| {
        if tok != "!*" && !in_set.contains(tok) {
            in_set.insert(tok.to_string());
            set.push(tok.to_string());
        }
    };
    let tokens: Vec<&str> = tokens.into_iter().collect();
    let simple = tokens.iter().all(|t| !t.contains('\n') && !t.contains(|c: char| c != ' ' && crate::js::is_js_whitespace(c)));
    let mut seen_lines: std::collections::HashSet<String> = std::collections::HashSet::new();
    if simple {
        // one line: `join(' ')`, trim, `split(' ')` — only the outer spaces disappear
        let n = tokens.len();
        let first_nonempty = tokens.iter().position(|t| !t.trim_matches(' ').is_empty());
        let last_nonempty = tokens.iter().rposition(|t| !t.trim_matches(' ').is_empty());
        match (first_nonempty, last_nonempty) {
            (Some(f), Some(l)) => {
                let mut line_parts: Vec<&str> = Vec::new();
                for (i, t) in tokens.iter().enumerate().take(l + 1).skip(f) {
                    let mut t: &str = t;
                    if i == f {
                        t = t.trim_start_matches(' ');
                    }
                    if i == l {
                        t = t.trim_end_matches(' ');
                    }
                    line_parts.push(t);
                }
                let _ = n;
                for part in line_parts.join(" ").split(' ') {
                    add(part, &mut set);
                }
            }
            _ => add("", &mut set),
        }
    } else {
        for line in tokens.join(" ").split('\n') {
            let line = crate::js::trim(line).to_string();
            if !seen_lines.insert(line.clone()) {
                continue;
            }
            for tok in line.split(' ') {
                add(tok, &mut set);
            }
        }
    }
    for c in safelist {
        for line in c.split('\n') {
            let line = crate::js::trim(line).to_string();
            if !seen_lines.insert(line.clone()) {
                continue;
            }
            for tok in line.split(' ') {
                add(tok, &mut set);
            }
        }
    }
    set.sort_by(|a, b| crate::js::cmp_utf16(sort_key(a), sort_key(b)));
    set
}

/// The rule cache sorted by offsets: (layer, rule)
pub fn sorted_rules(ctx: &Context) -> Arc<Vec<(Layer, NodeId)>> {
    let entries: Vec<(super::offsets::RuleOffset, NodeId)> = ctx.rule_cache.iter().flatten().cloned().collect();
    Arc::new(ctx.offsets.sort(entries).into_iter().map(|(o, r)| (o.layer, r)).collect())
}

/// `expandTailwindAtRules(context)(root)`
pub fn expand_tailwind_at_rules(ctx: &mut Context, root: NodeId, candidates: &[String]) -> Result<()> {
    let mut layer_nodes: [Option<NodeId>; 4] = [None; 4]; // base, components, utilities, variants
    ctx.css.walk_at_rules(root, &mut |css, r| {
        if css[r].name == "tailwind" {
            let i = match css[r].params.as_str() {
                "base" => 0,
                "components" => 1,
                "utilities" => 2,
                "variants" => 3,
                _ => return Flow::Continue,
            };
            layer_nodes[i] = Some(r);
        }
        Flow::Continue
    });
    if layer_nodes.iter().all(|n| n.is_none()) {
        return Ok(());
    }

    ctx.generate_rules(candidates).map_err(plain)?;

    // buildStylesheet
    let sorted = sorted_rules(ctx);
    let mut layers: [Vec<NodeId>; 5] = Default::default(); // base, defaults, components, utilities, variants
    let mut seen: [crate::js::FxHashSet<NodeId>; 5] = Default::default();
    for &(layer, rule) in sorted.iter() {
        let i = match layer {
            Layer::Base => 0,
            Layer::Defaults => 1,
            Layer::Components => 2,
            Layer::Utilities => 3,
            Layer::Variants => 4,
            Layer::User => continue,
        };
        if seen[i].insert(rule) {
            layers[i].push(rule);
        }
    }
    let [base_nodes, default_nodes, component_nodes, utility_nodes, screen_nodes] = layers;

    if let Some(b) = layer_nodes[0] {
        let src = ctx.css[b].source.clone();
        let mut list = default_nodes.clone();
        list.extend(base_nodes.iter().copied());
        let clones = clone_nodes(ctx, &list, src, "base");
        ctx.css.before(b, Add::Nodes(clones));
        ctx.css.remove(b);
    }
    if let Some(c) = layer_nodes[1] {
        let src = ctx.css[c].source.clone();
        let clones = clone_nodes(ctx, &component_nodes, src, "components");
        ctx.css.before(c, Add::Nodes(clones));
        ctx.css.remove(c);
    }
    if let Some(u) = layer_nodes[2] {
        let src = ctx.css[u].source.clone();
        let clones = clone_nodes(ctx, &utility_nodes, src, "utilities");
        ctx.css.before(u, Add::Nodes(clones));
        ctx.css.remove(u);
    }
    let variant_nodes: Vec<NodeId> = screen_nodes
        .into_iter()
        .filter(|n| match ctx.css[*n].tw().and_then(|t| t.parent_layer.as_deref()) {
            Some("components") => layer_nodes[1].is_some(),
            Some("utilities") => layer_nodes[2].is_some(),
            _ => true,
        })
        .collect();
    if let Some(v) = layer_nodes[3] {
        let src = ctx.css[v].source.clone();
        let clones = clone_nodes(ctx, &variant_nodes, src, "variants");
        ctx.css.before(v, Add::Nodes(clones));
        ctx.css.remove(v);
    } else if !variant_nodes.is_empty() {
        let src = ctx.css[root].source.clone();
        let clones = clone_nodes(ctx, &variant_nodes, src, "variants");
        ctx.css.append(root, Add::Nodes(clones));
    }
    // root.source.end = root.source.end ?? root.source.start
    if let Some(s) = ctx.css[root].source.as_mut() {
        if s.end.is_none() {
            s.end = s.start;
        }
    }
    ctx.css.walk_at_rules_named(root, "layer", &mut |css, r| {
        if matches!(css[r].params.as_str(), "base" | "components" | "utilities" | "variants") {
            css.remove(r);
        }
        Flow::Continue
    });
    Ok(())
}

/// `substituteScreenAtRules`
pub fn substitute_screen_at_rules(ctx: &mut Context, root: NodeId) -> Result<()> {
    let screens = normalize_screens(&ctx.cfg.value.get("theme").get("screens")).map_err(plain)?;
    let mut err = None;
    ctx.css.walk_at_rules_named(root, "screen", &mut |css, at| {
        let screen = css[at].params.clone();
        match screens.iter().find(|s| s.name == screen) {
            None => {
                err = Some(css.error(at, format!("No `{screen}` screen found.")));
                Flow::Stop
            }
            Some(def) => {
                css[at].name = "media".into();
                css[at].params = build_media_query(std::slice::from_ref(def));
                Flow::Continue
            }
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

// ---- resolveDefaultsAtRules

pub fn resolve_defaults_at_rules(ctx: &mut Context, root: NodeId) -> Result<()> {
    let mut variable_node_map: Vec<(String, Vec<NodeId>)> = Vec::new();
    let mut universals: Vec<NodeId> = Vec::new();
    ctx.css.walk_at_rules_named(root, "defaults", &mut |css, rule| {
        if css[rule].nodes.as_ref().is_some_and(|n| !n.is_empty()) {
            if !universals.contains(&rule) {
                universals.push(rule);
            }
            return Flow::Continue;
        }
        let variable = css[rule].params.clone();
        let parent = css[rule].parent.unwrap();
        match variable_node_map.iter_mut().find(|(k, _)| *k == variable) {
            Some(e) => {
                if !e.1.contains(&parent) {
                    e.1.push(parent);
                }
            }
            None => variable_node_map.push((variable, vec![parent])),
        }
        css.remove(rule);
        Flow::Continue
    });

    if super::config::flag_enabled(&ctx.cfg.value, "optimizeUniversalDefaults") {
        for universal in universals {
            let params = ctx.css[universal].params.clone();
            let rules = variable_node_map.iter().find(|(k, _)| *k == params).map(|(_, v)| v.clone()).unwrap_or_default();
            let mut groups: Vec<(String, Vec<String>)> = Vec::new();
            for rule in rules {
                let sel = ctx.css[rule].selector.clone();
                for selector in extract_element_selector(&sel).map_err(|e| plain(e.0))? {
                    let group = if selector.contains(":-") || selector.contains("::-") || selector.contains(":has") {
                        selector.clone()
                    } else {
                        "__DEFAULT__".to_string()
                    };
                    match groups.iter_mut().find(|(k, _)| *k == group) {
                        Some(e) => {
                            if !e.1.contains(&selector) {
                                e.1.push(selector);
                            }
                        }
                        None => groups.push((group, vec![selector])),
                    }
                }
            }
            if groups.is_empty() {
                ctx.css.remove(universal);
                continue;
            }
            for (_, selectors) in groups {
                let r = ctx.css.add(crate::postcss::Node::new(Kind::Rule));
                ctx.css[r].source = ctx.css[universal].source.clone();
                ctx.css.set_selectors(r, &selectors);
                let clones: Vec<NodeId> = ctx.css.children(universal).to_vec().into_iter().map(|n| ctx.css.clone_node(n)).collect();
                ctx.css.append(r, Add::Nodes(clones));
                ctx.css.before(universal, Add::Node(r));
            }
            ctx.css.remove(universal);
        }
    } else if !universals.is_empty() {
        let universal_rule = ctx.css.rule_with_selectors(&["*".into(), "::before".into(), "::after".into()]);
        for universal in universals {
            let nodes = ctx.css.children(universal).to_vec();
            ctx.css.append(universal_rule, Add::Nodes(nodes));
            if ctx.css[universal_rule].parent.is_none() {
                ctx.css.before(universal, Add::Node(universal_rule));
            }
            if ctx.css[universal_rule].source.is_none() {
                ctx.css[universal_rule].source = ctx.css[universal].source.clone();
            }
            ctx.css.remove(universal);
        }
        let backdrop = ctx.css.clone_node(universal_rule);
        ctx.css.set_selectors(backdrop, &["::backdrop".into()]);
        ctx.css.after(universal_rule, Add::Node(backdrop));
    }
    Ok(())
}

fn extract_element_selector(selector: &str) -> std::result::Result<Vec<String>, selector::SelError> {
    let mut sel = Sel::new();
    let root = selector::parse(&mut sel, selector)?;
    let sels: Vec<_> = sel.children(root).to_vec();
    let mut out = Vec::new();
    for s in sels {
        // s.split(n => n.type === 'combinator' && n.value === ' ').pop()
        let nodes = sel.children(s).to_vec();
        let mut groups: Vec<Vec<selector::SId>> = Vec::new();
        let mut current = Vec::new();
        for (i, &n) in nodes.iter().enumerate() {
            let split = sel[n].kind == SKind::Combinator && sel[n].val() == " ";
            current.push(n);
            if split {
                groups.push(std::mem::take(&mut current));
            } else if i == nodes.len() - 1 {
                groups.push(std::mem::take(&mut current));
            }
        }
        let last = groups.pop().unwrap_or_default();
        out.push(minimum_impact_selector(&mut sel, last));
    }
    Ok(out)
}

fn minimum_impact_selector(sel: &mut Sel, nodes: Vec<selector::SId>) -> String {
    let mut rest: Vec<selector::SId> = nodes
        .into_iter()
        .filter(|&n| {
            if sel[n].kind != SKind::Pseudo {
                return true;
            }
            if sel.len(n) > 0 {
                return true;
            }
            let v = sel[n].val();
            v.starts_with("::") || matches!(v, ":before" | ":after" | ":first-line" | ":first-letter")
        })
        .collect();
    rest.reverse();
    let search = |k: SKind| matches!(k, SKind::Tag | SKind::Class | SKind::Id | SKind::Attribute);
    let Some(split) = rest.iter().position(|n| search(sel[*n].kind)) else {
        rest.reverse();
        let s: String = rest.iter().map(|n| sel.to_string(*n)).collect();
        return crate::js::trim(&s).to_string();
    };
    let node = rest[split];
    let best = if sel[node].kind == SKind::Id {
        let v = sel[node].val().to_string();
        sel.attribute("id", "=", &v, Some('"'))
    } else {
        node
    };
    let mut rest: Vec<selector::SId> = rest[..split].to_vec();
    if let Some(ci) = rest.iter().position(|n| sel[*n].kind == SKind::Combinator && sel[*n].val() == ">") {
        rest.drain(0..ci);
        let u = sel.universal();
        rest.insert(0, u);
    }
    rest.reverse();
    let mut all = vec![best];
    all.extend(rest);
    let s: String = all.iter().map(|n| sel.to_string(*n)).collect();
    crate::js::trim(&s).to_string()
}

// ---- collapseAdjacentRules / collapseDuplicateDeclarations

fn collapse_ws(s: &str) -> String {
    super::util::collapse_ws(s)
}

/// `collapseAdjacentRules`: whether `node` (a rule or at-rule other than `@font-face`) merges
/// into the current rule `cur`.
pub fn collapses_into(css: &crate::postcss::Css, cur: NodeId, node: NodeId) -> bool {
    match css[node].kind {
        Kind::AtRule => {
            // `node[property] ?? ''` — a rule compared with an at-rule has no `name`
            let cn = if css[cur].kind == Kind::AtRule { css[cur].name.clone() } else { String::new() };
            let cp = if css[cur].kind == Kind::AtRule { css[cur].params.clone() } else { String::new() };
            collapse_ws(&css[node].name) == collapse_ws(&cn) && collapse_ws(&css[node].params) == collapse_ws(&cp)
        }
        _ => {
            let cs = if css[cur].kind == Kind::Rule { css[cur].selector.clone() } else { String::new() };
            collapse_ws(&css[node].selector) == collapse_ws(&cs)
        }
    }
}

fn collapse_rules_in(ctx: &mut Context, root: NodeId) {
    let mut current: Option<NodeId> = None;
    ctx.css.each(root, &mut |css, node| {
        let kind = css[node].kind;
        if kind != Kind::Rule && kind != Kind::AtRule {
            current = None;
            return Flow::Continue;
        }
        let Some(cur) = current else {
            current = Some(node);
            return Flow::Continue;
        };
        if kind == Kind::AtRule && css[node].name == "font-face" {
            current = Some(node);
            return Flow::Continue;
        }
        if collapses_into(css, cur, node) {
            if css[node].nodes.is_some() {
                let nodes = css.children(node).to_vec();
                css.append(cur, Add::Nodes(nodes));
            }
            css.remove(node);
        } else {
            current = Some(node);
        }
        Flow::Continue
    });
    let children = ctx.css.children(root).to_vec();
    let mut list = Vec::new();
    ctx.css.each(root, &mut |css, node| {
        if css[node].kind == Kind::AtRule {
            list.push(node);
        }
        Flow::Continue
    });
    let _ = children;
    for n in list {
        collapse_rules_in(ctx, n);
    }
}

pub fn collapse_adjacent_rules(ctx: &mut Context, root: NodeId) {
    collapse_rules_in(ctx, root);
}

/// `resolveUnit(input)`: /^-?\d*.?\d+([\w%]+)?$/ → Some(unit) / Some("") for unitless / None
fn resolve_unit(input: &str) -> Option<String> {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut i = 0;
    if i < n && chars[i] == '-' {
        i += 1;
    }
    let is_d = |c: char| c.is_ascii_digit();
    let is_w = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '%';
    // try all splits: \d* (greedy) .? \d+ ([\w%]+)?
    let d0 = i;
    let mut d_end = i;
    while d_end < n && is_d(chars[d_end]) {
        d_end += 1;
    }
    // backtracking over \d* and .?
    for a in (d0..=d_end).rev() {
        for dot in [true, false] {
            let mut j = a;
            if dot {
                if j < n && !matches!(chars[j], '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                    j += 1;
                } else {
                    continue;
                }
            }
            let ds = j;
            let mut k = j;
            while k < n && is_d(chars[k]) {
                k += 1;
            }
            // \d+ greedy with backtracking
            for e in (ds + 1..=k).rev() {
                let rest = &chars[e..];
                if rest.is_empty() {
                    return Some(String::new());
                }
                if rest.iter().all(|c| is_w(*c)) {
                    return Some(rest.iter().collect());
                }
            }
        }
    }
    None
}

pub fn collapse_duplicate_declarations(ctx: &mut Context, root: NodeId) {
    let mut rules = Vec::new();
    ctx.css.walk_rules(root, &mut |_, r| {
        rules.push(r);
        Flow::Continue
    });
    for node in rules {
        let mut seen: Vec<(String, NodeId)> = Vec::new();
        let mut droppable: Vec<NodeId> = Vec::new();
        let mut by_property: Vec<(String, Vec<NodeId>)> = Vec::new();
        let decls: Vec<NodeId> = ctx.css.descendants(node).into_iter().filter(|d| ctx.css[*d].kind == Kind::Decl).collect();
        for decl in decls {
            if ctx.css[decl].parent != Some(node) {
                continue;
            }
            let prop = ctx.css[decl].prop.clone();
            if let Some(i) = seen.iter().position(|(p, _)| *p == prop) {
                let prev = seen[i].1;
                if ctx.css[prev].value == ctx.css[decl].value {
                    if !droppable.contains(&prev) {
                        droppable.push(prev);
                    }
                    seen[i].1 = decl;
                    continue;
                }
                let e = match by_property.iter_mut().position(|(p, _)| *p == prop) {
                    Some(j) => &mut by_property[j].1,
                    None => {
                        by_property.push((prop.clone(), Vec::new()));
                        &mut by_property.last_mut().unwrap().1
                    }
                };
                if !e.contains(&prev) {
                    e.push(prev);
                }
                if !e.contains(&decl) {
                    e.push(decl);
                }
                seen[i].1 = decl;
            } else {
                seen.push((prop, decl));
            }
        }
        for d in droppable {
            ctx.css.remove(d);
        }
        for (_, declarations) in by_property {
            let mut by_unit: Vec<(String, Vec<NodeId>)> = Vec::new();
            for decl in declarations {
                let Some(unit) = resolve_unit(&ctx.css[decl].value) else { continue };
                let key = if unit.is_empty() { "\u{0}unitless".to_string() } else { unit };
                match by_unit.iter_mut().find(|(k, _)| *k == key) {
                    Some(e) => {
                        if !e.1.contains(&decl) {
                            e.1.push(decl);
                        }
                    }
                    None => by_unit.push((key, vec![decl])),
                }
            }
            for (_, list) in by_unit {
                for d in &list[..list.len() - 1] {
                    ctx.css.remove(*d);
                }
            }
        }
    }
}

pub fn _unused(_: Arc<()>, _: SFlow) {}

/// `findAtConfigPath(root, result)`: the `@config` rules go (the caller resolves the config); the
/// checks that need no file system stay.
pub fn remove_config_at_rules(ctx: &mut Context, root: NodeId) -> Result<()> {
    let mut seen = false;
    let mut err = None;
    ctx.css.walk_at_rules_named(root, "config", &mut |css, rule| {
        if seen {
            err = Some(css.error(rule, "Only one `@config` directive is allowed per file."));
            return Flow::Stop;
        }
        // rule.params.match(/(['"])(.*?)\1/)
        let params = css[rule].params.clone();
        let path = params.char_indices().filter(|(_, c)| *c == '\'' || *c == '"').find_map(|(i, q)| {
            let rest = &params[i + 1..];
            rest.find(q).map(|j| rest[..j].to_string())
        });
        let Some(path) = path else {
            err = Some(css.error(rule, "A path is required when using the `@config` directive."));
            return Flow::Stop;
        };
        if path.starts_with('/') {
            err = Some(css.error(rule, "The `@config` directive cannot be used with an absolute path."));
            return Flow::Stop;
        }
        seen = true;
        css.remove(rule);
        Flow::Continue
    });
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// `processTailwindFeatures(setupContext)(root, result)`
pub fn process_tailwind_features<'a, 'c>(
    ctx: &mut Context<'c>,
    root: NodeId,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Result<()> {
    prepare(ctx, root)?;
    let safelist = std::mem::take(&mut ctx.changed_content);
    let candidates = candidates_from_tokens(candidates, &safelist);
    finish(ctx, root, &candidates)
}

/// The part of `processTailwindFeatures` before the candidates are needed: directives and the
/// context (core plugins, `@layer` rules of the input, safelist in `ctx.changed_content`).
pub fn prepare(ctx: &mut Context, root: NodeId) -> Result<()> {
    normalize_tailwind_directives(ctx, root)?;
    partition_apply_at_rules(ctx, root);
    let cfg = ctx.cfg;
    create_context(cfg, ctx, root)
}

/// The rest of `processTailwindFeatures` over the candidate list of `candidates_from_tokens`.
pub fn finish(ctx: &mut Context, root: NodeId, candidates: &[String]) -> Result<()> {
    expand_tailwind_at_rules(ctx, root, candidates)?;
    partition_apply_at_rules(ctx, root);
    ctx.expand_apply_at_rules(root)?;
    ctx.evaluate_tailwind_functions(root)?;
    substitute_screen_at_rules(ctx, root)?;
    resolve_defaults_at_rules(ctx, root)?;
    collapse_adjacent_rules(ctx, root);
    collapse_duplicate_declarations(ctx, root);
    Ok(())
}

/// The String object compares as '*' and is never equal to the primitive.
fn sort_key(s: &str) -> &str {
    if s == super::context::NOT_ON_DEMAND { "*" } else { s }
}
