//! `lib/expandApplyAtRules.js`

use crate::postcss::{Add, CssError, Flow, Kind, NodeId};
use crate::selector::{self, SFlow, SId, SKind, Sel};

use super::context::{Context, Match, Meta};
use super::format_variant_selector::{apply_important_selector, move_pseudos};
use super::offsets::Layer;
use super::util::escape_class_name;

/// `extractClasses(node)` → (flat classes, groups)
fn extract_classes(ctx: &mut Context, node: NodeId) -> Result<(Vec<String>, Vec<Vec<String>>), CssError> {
    let container = ctx.css.root_with(&[node]);
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut err = None;
    ctx.css.walk_rules(container, &mut |css, rule| {
        let mut sel = Sel::new();
        let root = match selector::parse(&mut sel, &css[rule].selector) {
            Ok(r) => r,
            Err(e) => {
                err = Some(CssError::plain(e.0));
                return Flow::Stop;
            }
        };
        sel.walk_classes(root, &mut |s, c| {
            let parent = s[c].parent.unwrap();
            let ps = s.to_string(parent);
            let v = s[c].val().to_string();
            match groups.iter_mut().find(|(k, _)| *k == ps) {
                Some(e) => {
                    if !e.1.contains(&v) {
                        e.1.push(v);
                    }
                }
                None => groups.push((ps, vec![v])),
            }
            SFlow::Continue
        });
        Flow::Continue
    });
    if let Some(e) = err {
        return Err(e);
    }
    let groups: Vec<Vec<String>> = groups.into_iter().map(|(_, v)| v).collect();
    let flat = groups.iter().flatten().cloned().collect();
    Ok((flat, groups))
}

fn extract_base_candidates(candidates: &[String], separator: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in candidates {
        let b = c.split(separator).last().unwrap_or("").to_string();
        if !out.contains(&b) {
            out.push(b);
        }
    }
    out
}

fn path_to_root(ctx: &Context, node: NodeId) -> Vec<NodeId> {
    let mut out = vec![node];
    let mut n = node;
    while let Some(p) = ctx.css[n].parent {
        out.push(p);
        n = p;
    }
    out
}

/// `nestedClone(node)`: top-level rules are returned as is; nested ones are wrapped in
/// shallow clones of their ancestors whose `nodes` arrays reference the original node.
fn nested_clone(ctx: &mut Context, node: NodeId) -> NodeId {
    let path = path_to_root(ctx, node);
    let mut cur = node;
    for parent in path {
        if parent == node {
            continue;
        }
        if ctx.css[parent].kind == Kind::Root {
            break;
        }
        let c = ctx.css.clone_shallow(parent);
        ctx.css[c].nodes = Some(vec![cur]);
        cur = c;
    }
    cur
}

struct LocalEntry {
    class: String,
    sort: super::offsets::RuleOffset,
    container: NodeId,
}

fn build_local_apply_cache(ctx: &mut Context, root: NodeId) -> Result<Vec<LocalEntry>, CssError> {
    let mut rules = Vec::new();
    ctx.css.walk_rules(root, &mut |_, r| {
        rules.push(r);
        Flow::Continue
    });
    let mut cache = Vec::new();
    for rule in rules {
        let generated = path_to_root(ctx, rule).into_iter().any(|n| ctx.css[n].tw().is_some_and(|t| t.layer.is_some()));
        if generated {
            continue;
        }
        let container = nested_clone(ctx, rule);
        let sort = ctx.offsets.create(Layer::User);
        let (classes, _) = extract_classes(ctx, rule)?;
        for c in classes {
            cache.push(LocalEntry { class: c, sort: sort.clone(), container });
        }
    }
    Ok(cache)
}

fn extract_apply_candidates(params: &str) -> (Vec<String>, bool) {
    // params.split(/[\s\t\n]+/g)
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_ws = false;
    for c in params.chars() {
        if crate::js::is_js_whitespace(c) {
            if !in_ws {
                parts.push(std::mem::take(&mut cur));
            }
            in_ws = true;
        } else {
            in_ws = false;
            cur.push(c);
        }
    }
    parts.push(cur);
    if parts.last().map(|s| s.as_str()) == Some("!important") {
        parts.pop();
        return (parts, true);
    }
    (parts, false)
}

impl<'c> Context<'c> {
    fn build_apply_cache(&mut self, candidates: &[String]) -> Result<(), CssError> {
        for c in candidates {
            if self.not_class_cache.contains(c) || self.apply_class_cache.contains_key(c) {
                continue;
            }
            if let Some(m) = self.class_cache.get(c).cloned() {
                let cloned: Vec<Match> = m.into_iter().map(|(meta, rule)| (meta, self.css.clone_node(rule))).collect();
                self.apply_class_cache.insert(c.clone(), cloned);
                continue;
            }
            let matches = self.resolve_matches(c).map_err(CssError::plain)?;
            if matches.is_empty() {
                self.not_class_cache.insert(c.clone());
                continue;
            }
            self.apply_class_cache.insert(c.clone(), matches);
        }
        Ok(())
    }

    /// `replaceSelector(selector, utilitySelectors, candidate)`
    fn replace_selector(selector: &str, utility_selectors: &str, candidate: &str) -> Result<String, CssError> {
        let mut sel = Sel::new();
        let map = |e: selector::SelError| CssError::plain(e.0);
        let selector_list = selector::parse(&mut sel, selector).map_err(map)?;
        let utility_list = selector::parse(&mut sel, utility_selectors).map_err(map)?;
        let candidate_list = selector::parse(&mut sel, &format!(".{}", escape_class_name(candidate))).map_err(map)?;
        let cfirst = sel.children(candidate_list)[0];
        let candidate_class = sel.children(cfirst)[0];
        let candidate_value = sel[candidate_class].val().to_string();

        let sels: Vec<SId> = sel.children(selector_list).to_vec();
        sel.each(selector_list, &mut |sel, s| {
            let mut replaced: Vec<SId> = Vec::new();
            let utils: Vec<SId> = sel.children(utility_list).to_vec();
            sel.each(utility_list, &mut |sel, us| {
                let mut has_replaced = false;
                let us = sel.clone_node(us);
                let s_nodes: Vec<SId> = sel.children(s).to_vec();
                sel.walk_classes(us, &mut |sel, node| {
                    if sel[node].val() != candidate_value {
                        return SFlow::Continue;
                    }
                    if has_replaced {
                        return SFlow::Continue;
                    }
                    let clones: Vec<SId> = s_nodes.iter().map(|n| sel.clone_node(*n)).collect();
                    sel.replace_with(node, &clones);
                    if !replaced.contains(&us) {
                        replaced.push(us);
                    }
                    has_replaced = true;
                    SFlow::Continue
                });
                SFlow::Continue
            });
            let _ = utils;
            for &r in &replaced {
                let nodes: Vec<SId> = sel.children(r).to_vec();
                let mut groups: Vec<Result<Vec<SId>, SId>> = vec![Ok(Vec::new())];
                for n in nodes {
                    if sel[n].kind == SKind::Combinator {
                        groups.push(Err(n));
                        groups.push(Ok(Vec::new()));
                    } else if let Some(Ok(last)) = groups.last_mut() {
                        last.push(n);
                    }
                }
                let mut out: Vec<SId> = Vec::new();
                for g in groups {
                    match g {
                        Ok(mut list) => {
                            crate::js::sort_by(&mut list, |a, b| {
                                let (ka, kb) = (sel[*a].kind, sel[*b].kind);
                                let a_pe = ka == SKind::Pseudo && sel[*a].val().starts_with("::");
                                let b_pe = kb == SKind::Pseudo && sel[*b].val().starts_with("::");
                                if ka == SKind::Tag && kb == SKind::Class {
                                    -1
                                } else if ka == SKind::Class && kb == SKind::Tag {
                                    1
                                } else if ka == SKind::Class && b_pe {
                                    -1
                                } else if a_pe && kb == SKind::Class {
                                    1
                                } else {
                                    0
                                }
                            });
                            out.extend(list);
                        }
                        Err(c) => out.push(c),
                    }
                }
                sel[r].nodes = Some(out);
            }
            sel.replace_with(s, &replaced);
            SFlow::Continue
        });
        let _ = sels;
        Ok(sel.to_string(selector_list))
    }

    fn process_apply(&mut self, root: NodeId, local: &mut Option<Vec<LocalEntry>>) -> Result<(), CssError> {
        let mut apply_candidates: Vec<String> = Vec::new();
        let mut applies: Vec<NodeId> = Vec::new();
        self.css.walk_at_rules_named(root, "apply", &mut |css, rule| {
            let (cands, _) = extract_apply_candidates(&css[rule].params);
            for c in cands {
                if !apply_candidates.contains(&c) {
                    apply_candidates.push(c);
                }
            }
            applies.push(rule);
            Flow::Continue
        });
        if applies.is_empty() {
            return Ok(());
        }
        self.build_apply_cache(&apply_candidates)?;

        // perParentApplies
        let mut per_parent: Vec<(NodeId, Vec<(String, bool, Vec<Match>)>, Option<crate::postcss::Source>)> = Vec::new();
        for &apply in &applies {
            let parent = self.css[apply].parent.unwrap();
            if !per_parent.iter().any(|(p, _, _)| *p == parent) {
                let src = self.css[apply].source.clone();
                per_parent.push((parent, Vec::new(), src));
            } else {
                // `perParentApplies.set(apply.parent, [candidates, apply.source])` — last source wins
                let src = self.css[apply].source.clone();
                per_parent.iter_mut().find(|(p, _, _)| *p == parent).unwrap().2 = src;
            }
            let (cands, important) = extract_apply_candidates(&self.css[apply].params);
            if self.css[parent].kind == Kind::AtRule {
                let pname = self.css[parent].name.clone();
                if pname == "screen" {
                    let st = self.css[parent].params.clone();
                    let sugg = cands.iter().map(|c| format!("{st}:{c}")).collect::<Vec<_>>().join(" ");
                    return Err(self.css.error(
                        apply,
                        format!("@apply is not supported within nested at-rules like @screen. We suggest you write this as @apply {sugg} instead."),
                    ));
                }
                return Err(self.css.error(
                    apply,
                    format!("@apply is not supported within nested at-rules like @{pname}. You can fix this by un-nesting @{pname}."),
                ));
            }
            for c in cands {
                let group = format!("{}group", self.cfg.prefix);
                let peer = format!("{}peer", self.cfg.prefix);
                if c == group || c == peer {
                    return Err(self.css.error(apply, format!("@apply should not be used with the '{c}' utility")));
                }
                let rules = self.apply_cache_get(root, local, &c)?;
                let Some(rules) = rules else {
                    return Err(self.css.error(
                        apply,
                        format!("The `{c}` class does not exist. If `{c}` is a custom class, make sure it is defined within a `@layer` directive."),
                    ));
                };
                for (_, rule) in &rules {
                    if self.css[*rule].kind == Kind::AtRule {
                        continue;
                    }
                    let mut nested = false;
                    self.css.walk_rules(*rule, &mut |_, _| {
                        nested = true;
                        Flow::Stop
                    });
                    if nested {
                        return Err(self.css.error(
                            apply,
                            [
                                format!("The `{c}` class cannot be used with `@apply` because `@apply` does not currently support nested CSS."),
                                "Rewrite the selector without nesting or configure the `tailwindcss/nesting` plugin:".to_string(),
                                "https://tailwindcss.com/docs/using-with-preprocessors#nesting".to_string(),
                            ]
                            .join("\n"),
                        ));
                    }
                }
                per_parent.iter_mut().find(|(p, _, _)| *p == parent).unwrap().1.push((c, important, rules));
            }
        }

        let separator = self.cfg.separator.clone();
        for (parent, candidates, at_apply_source) in per_parent {
            let mut siblings: Vec<(super::offsets::RuleOffset, NodeId)> = Vec::new();
            for (apply_candidate, important, rules) in candidates {
                let mut potential = vec![apply_candidate.clone()];
                potential.extend(extract_base_candidates(std::slice::from_ref(&apply_candidate), &separator));
                for (meta, node) in rules {
                    let (parent_classes, _) = extract_classes(self, parent)?;
                    let (_, node_groups) = extract_classes(self, node)?;
                    let mut node_classes: Vec<String> =
                        node_groups.into_iter().filter(|g| g.iter().any(|c| potential.contains(c))).flatten().collect();
                    let bases = extract_base_candidates(&node_classes, &separator);
                    node_classes.extend(bases);
                    if parent_classes.iter().any(|c| node_classes.contains(c)) {
                        return Err(self.css.error(
                            node,
                            format!("You cannot `@apply` the `{apply_candidate}` utility here because it creates a circular dependency."),
                        ));
                    }
                    let r = self.css.root_with(&[node]);
                    for d in self.css.descendants(r) {
                        self.css[d].source = at_apply_source.clone();
                    }
                    let can_rewrite = self.css[node].kind != Kind::AtRule || self.css[node].name != "keyframes";
                    if can_rewrite {
                        let mut rules_in: Vec<NodeId> = Vec::new();
                        self.css.walk_rules(r, &mut |_, x| {
                            rules_in.push(x);
                            Flow::Continue
                        });
                        for rule in rules_in {
                            if self.css[rule].parent.is_none() && rule != r {
                                // removed together with an ancestor
                            }
                            let (classes, _) = extract_classes(self, rule)?;
                            if !classes.iter().any(|c| *c == apply_candidate) {
                                self.css.remove(rule);
                                continue;
                            }
                            let important_selector = match &self.cfg.important {
                                crate::js::Value::Str(s) => Some(s.to_string()),
                                _ => None,
                            };
                            let is_generated = self.css[parent].raws.tailwind.is_some();
                            let psel = self.css[parent].selector.clone();
                            let mut parent_selector = match &important_selector {
                                Some(imp) if is_generated && psel.starts_with(imp.as_str()) => psel[imp.len()..].to_string(),
                                _ => psel.clone(),
                            };
                            if parent_selector.is_empty() {
                                parent_selector = psel.clone();
                            }
                            let rsel = self.css[rule].selector.clone();
                            let mut new_sel = Self::replace_selector(&parent_selector, &rsel, &apply_candidate)?;
                            if let Some(imp) = &important_selector {
                                if parent_selector != psel {
                                    new_sel = apply_important_selector(&new_sel, imp).map_err(|e| CssError::plain(e.0))?;
                                }
                            }
                            self.css[rule].selector = new_sel;
                            let imp_flag = meta.important || important;
                            self.css.walk_decls(rule, &mut |css, d| {
                                css[d].important = imp_flag;
                                Flow::Continue
                            });
                            let mut sel = Sel::new();
                            let ast = selector::parse(&mut sel, &self.css[rule].selector.clone()).map_err(|e| CssError::plain(e.0))?;
                            sel.each(ast, &mut |s, x| {
                                move_pseudos(s, x);
                                SFlow::Continue
                            });
                            self.css[rule].selector = sel.to_string(ast);
                        }
                    }
                    let Some(first) = self.css.first(r) else { continue };
                    siblings.push((meta.sort.clone(), first));
                }
            }
            let nodes: Vec<NodeId> = self.offsets.sort(siblings).into_iter().map(|(_, n)| n).collect();
            self.css.after(parent, Add::Nodes(nodes));
        }

        for apply in applies {
            let parent = self.css[apply].parent;
            match parent {
                Some(p) if self.css.children(p).len() > 1 => self.css.remove(apply),
                Some(p) => self.css.remove(p),
                None => {}
            }
        }
        self.process_apply(root, local)
    }

    /// `combineCaches([localCache, applyClassCache])` lookups
    fn apply_cache_get(&mut self, root: NodeId, local: &mut Option<Vec<LocalEntry>>, c: &str) -> Result<Option<Vec<Match>>, CssError> {
        if local.is_none() {
            *local = Some(build_local_apply_cache(self, root)?);
        }
        let mut out: Vec<Match> = Vec::new();
        let mut has = false;
        for e in local.as_ref().unwrap() {
            if e.class == c {
                has = true;
                out.push((
                    Meta { sort: e.sort.clone(), layer: Layer::User, options: None, important: false, collected_formats: None },
                    e.container,
                ));
            }
        }
        if let Some(m) = self.apply_class_cache.get(c) {
            has = true;
            out.extend(m.iter().cloned());
        }
        Ok(if has { Some(out) } else { None })
    }

    /// `expandApplyAtRules(context)(root)`
    pub fn expand_apply_at_rules(&mut self, root: NodeId) -> Result<(), CssError> {
        let mut local = None;
        self.process_apply(root, &mut local)
    }
}
