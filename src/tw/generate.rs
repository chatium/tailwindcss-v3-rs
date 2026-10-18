//! `lib/generateRules.js`

use std::sync::Arc;

use crate::js::{Object, Value};
use crate::postcss::{self, Add, Css, Flow, Kind, NodeId};
use crate::selector::{self, SFlow, SKind, Sel};

use super::context::{Context, Format, Match, Meta, PluginOptions, PluginRule, VRet, VarValue, VariantApi, VariantArgs, VariantEntry, VariantInfo, VariantType, VarValueSort};
use super::format_variant_selector::{apply_important_selector, eliminate_irrelevant_selectors, finalize_selector, format_variant_selector, FormatsArg};
use super::object_styles::parse_object_styles;
use super::offsets::{Layer, RuleOffset, SortValue, VariantOption};
use super::plugin_utils::get_matching_types;
use super::util::{as_class, is_syntactically_valid_property_value, normalize, split_at_top_level_only};

/// `sharedState.NOT_ON_DEMAND` is `new String('*')`: a Map key distinct from the string `'*'`
pub const NOT_ON_DEMAND: &str = "\u{0}*";

fn is_arbitrary_value(s: &str) -> bool {
    s.starts_with('[') && s.ends_with(']')
}

/// The class part of a candidate (`resolveMatches`: last top-level part, without `!`).
pub fn class_part<'a>(candidate: &'a str, separator: &str) -> &'a str {
    let parts = split_at_top_level_only(candidate, separator);
    let last = parts.last().copied().unwrap_or(candidate);
    last.strip_prefix('!').unwrap_or(last)
}

/// Whether `resolveMatchedPlugins(classCandidate)` yields anything given the candidate rule map
/// keys (`has_key`); arbitrary properties are assumed to match. A candidate for which this is
/// false resolves to nothing and leaves no trace in the context.
pub fn has_plugins(class_candidate: &str, prefix: &str, has_key: impl Fn(&str) -> bool) -> bool {
    if has_key(class_candidate) || is_arbitrary_value(class_candidate) {
        return true;
    }
    let mut candidate_prefix = class_candidate.to_string();
    let has_matching_prefix = candidate_prefix.starts_with(prefix) || candidate_prefix.starts_with(&format!("-{prefix}"));
    if candidate_prefix.as_bytes().get(prefix.len()) == Some(&b'-') && has_matching_prefix {
        candidate_prefix = format!("{prefix}{}", &candidate_prefix[prefix.len() + 1..]);
        if has_key(&candidate_prefix) {
            return true;
        }
    }
    candidate_permutations(&candidate_prefix).iter().any(|(p, _)| has_key(p))
}

/// `candidatePermutations(candidate)`
fn candidate_permutations(candidate: &str) -> Vec<(String, String)> {
    let b = candidate.as_bytes();
    let mut out = Vec::new();
    let mut last_index: Option<isize> = None; // None = Infinity
    loop {
        if let Some(li) = last_index {
            if li < 0 {
                break;
            }
        }
        let mut was_slash = false;
        let dash_idx: isize;
        if last_index.is_none() && candidate.ends_with(']') {
            let bracket = candidate.find('[').map(|i| i as isize).unwrap_or(-1);
            let before = if bracket >= 1 { Some(b[bracket as usize - 1]) } else { None };
            if before == Some(b'-') {
                dash_idx = bracket - 1;
            } else if before == Some(b'/') {
                dash_idx = bracket - 1;
                was_slash = true;
            } else {
                dash_idx = -1;
            }
        } else if last_index.is_none() && candidate.contains('/') {
            dash_idx = candidate.rfind('/').unwrap() as isize;
            was_slash = true;
        } else {
            // lastIndexOf('-', lastIndex)
            let from = match last_index {
                None => b.len() as isize - 1,
                Some(li) => li.min(b.len() as isize - 1),
            };
            dash_idx = if from < 0 { -1 } else { (0..=from as usize).rev().find(|&i| b[i] == b'-').map(|i| i as isize).unwrap_or(-1) };
        }
        if dash_idx < 0 {
            break;
        }
        let d = dash_idx as usize;
        let prefix = &candidate[..d];
        let modifier = if was_slash { &candidate[d..] } else { &candidate[d + 1..] };
        last_index = Some(dash_idx - 1);
        if prefix.is_empty() || modifier == "/" {
            continue;
        }
        out.push((prefix.to_string(), modifier.to_string()));
    }
    out
}

fn in_keyframes(css: &Css, rule: NodeId) -> bool {
    match css[rule].parent {
        Some(p) => css[p].kind == Kind::AtRule && css[p].name == "keyframes",
        None => false,
    }
}

impl<'c> Context<'c> {
    fn apply_prefix(&mut self, matches: Vec<Match>) -> Result<Vec<Match>, String> {
        if matches.is_empty() || self.cfg.prefix.is_empty() {
            return Ok(matches);
        }
        let prefix = self.cfg.prefix.clone();
        let mut out = Vec::new();
        for (meta, rule) in matches {
            if meta.options.as_ref().and_then(|o| o.respect_prefix).unwrap_or(false) {
                let container = self.css.root_with(&[rule]);
                let class_candidate = self.css[rule].tw().and_then(|t| t.class_candidate.clone()).unwrap_or_default();
                let mut err = None;
                self.css.walk_rules(container, &mut |css, r| {
                    let negative = class_candidate.starts_with('-');
                    match super::util::prefix_selector(&prefix, &css[r].selector.clone(), negative) {
                        Ok(s) => css[r].selector = s,
                        Err(e) => {
                            err = Some(e.0);
                            return Flow::Stop;
                        }
                    }
                    Flow::Continue
                });
                if let Some(e) = err {
                    return Err(e);
                }
                let first = self.css.first(container).unwrap();
                out.push((meta, first));
            } else {
                out.push((meta, rule));
            }
        }
        Ok(out)
    }

    fn apply_important(&mut self, matches: Vec<Match>, class_candidate: &str) -> Result<Vec<Match>, String> {
        if matches.is_empty() {
            return Ok(matches);
        }
        let mut result = Vec::new();
        for (meta, rule) in matches {
            let container = self.css.root_with(&[rule]);
            let mut err = None;
            self.css.walk_rules(container, &mut |css, r| {
                if in_keyframes(css, r) {
                    return Flow::Continue;
                }
                let mut sel = Sel::new();
                let ast = match selector::parse(&mut sel, &css[r].selector) {
                    Ok(a) => a,
                    Err(e) => {
                        err = Some(e.0);
                        return Flow::Stop;
                    }
                };
                sel.each(ast, &mut |s, child| {
                    eliminate_irrelevant_selectors(s, child, class_candidate);
                    SFlow::Continue
                });
                update_all_classes(&mut sel, ast, &|c| if c == class_candidate { format!("!{c}") } else { c.to_string() });
                css[r].selector = sel.to_string(ast);
                css.walk_decls(r, &mut |css, d| {
                    css[d].important = true;
                    Flow::Continue
                });
                Flow::Continue
            });
            if let Some(e) = err {
                return Err(e);
            }
            let first = self.css.first(container).unwrap();
            result.push((Meta { important: true, ..meta }, first));
        }
        Ok(result)
    }

    /// `applyVariant(variant, matches, context)`
    fn apply_variant(&mut self, variant: &str, matches: Vec<Match>) -> Result<Vec<Match>, String> {
        if matches.is_empty() {
            return Ok(matches);
        }
        let mut variant = variant.to_string();
        let mut args = VariantArgs { modifier: None, value: VarValue::None };
        {
            let parts = split_at_top_level_only(&variant, "/");
            let mut base_variant = parts[0].to_string();
            let mut modifiers: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
            if modifiers.len() > 1 {
                base_variant = format!("{base_variant}/{}", modifiers[..modifiers.len() - 1].join("/"));
                modifiers = vec![modifiers.last().unwrap().clone()];
            }
            if !modifiers.is_empty() && !self.variant_map.contains_key(&variant) {
                variant = base_variant;
                args.modifier = Some(modifiers[0].clone());
                if !self.cfg.generalized_modifiers {
                    return Ok(Vec::new());
                }
            }
        }
        if variant.ends_with(']') && !variant.starts_with('[') {
            // /(.)(-?)\[(.*)\]/g.exec(variant)
            if let Some((ch, sep, value, start)) = match_arbitrary_variant(&variant) {
                if ch == '@' && sep == "-" {
                    return Ok(Vec::new());
                }
                if ch != '@' && sep.is_empty() {
                    return Ok(Vec::new());
                }
                let _ = start;
                let needle = format!("{sep}[{value}]");
                variant = variant.replacen(&needle, "", 1);
                args.value = VarValue::Str(value);
            }
        }

        if is_arbitrary_value(&variant) && !self.variant_map.contains_key(&variant) {
            let sort = self.offsets.record_variant(&variant, 1);
            let selector = normalize(&variant[1..variant.len() - 1], None);
            let selectors = split_at_top_level_only(&selector, ",");
            if selectors.len() > 1 {
                return Ok(Vec::new());
            }
            if !selectors.iter().all(|s| super::context::is_valid_variant_format_string(s)) {
                return Ok(Vec::new());
            }
            let mut records = Vec::new();
            for (idx, s) in selectors.iter().enumerate() {
                let f = super::context::parse_variant(crate::js::trim(s))?;
                records.push(VariantEntry { sort: self.offsets.apply_parallel_offset(&sort, idx as u64), f, container: None });
            }
            Arc::make_mut(&mut self.variant_map).insert(variant.clone(), records);
        }

        let Some(tuples) = self.variant_map.get(&variant).cloned() else {
            return Ok(Vec::new());
        };
        let is_arbitrary_variant = is_arbitrary_value(&variant);
        let vopts = self.variant_options.get(&variant).cloned();
        let respect_prefix = !(is_arbitrary_variant || vopts.as_ref().is_some_and(|o| o.respect_prefix_false));

        // `Object.assign(args, context.variantOptions.get(variant))`: the sort-relevant fields
        let mut opt = VariantOption { id: None, sort: None, value: SortValue::Other, variant: Default::default() };
        let mut args_value_sort = match &args.value {
            VarValue::Str(s) => SortValue::Str(s.clone()),
            VarValue::Screen(s) => SortValue::Screen(s.clone()),
            _ => SortValue::Other,
        };
        if let Some(o) = &vopts {
            opt.id = o.id.clone();
            opt.sort = o.sort;
            if let Some(v) = &o.value {
                args_value_sort = match v {
                    VarValueSort::Screen(s) => SortValue::Screen(s.clone()),
                    VarValueSort::Str(s) => SortValue::Str(s.clone()),
                    VarValueSort::Other => SortValue::Other,
                };
            }
        }
        opt.value = args_value_sort;
        let _ = (VariantType::Add, VariantInfo::Base);

        let mut result = Vec::new();
        // `variantFunctionTuples` is shared by all matches: entries queued while handling one
        // match are replayed (with that match's container) for the following ones.
        let mut queue: Vec<VariantEntry> = tuples.clone();
        for (meta, rule) in matches {
            if meta.layer == Layer::User {
                continue;
            }
            let container = self.css.root_with(&[rule]);
            let mut qi = 0;
            while qi < queue.len() {
                let entry = queue[qi].clone();
                qi += 1;
                let src = entry.container.unwrap_or(container);
                let clone = self.css.clone_node(src);
                let mut api = VariantApi {
                    css: &mut self.css,
                    clone,
                    formats: Vec::new(),
                    args: args.clone(),
                    respect_prefix,
                    separator: self.cfg.separator.clone(),
                };
                let ret = (entry.f)(&mut api)?;
                let mut collected = std::mem::take(&mut api.formats);
                drop(api);
                match ret {
                    VRet::Fns(fns) => {
                        for (idx, f) in fns.into_iter().enumerate() {
                            let c = self.css.clone_node(clone);
                            queue.push(VariantEntry { sort: self.offsets.apply_parallel_offset(&entry.sort, idx as u64), f, container: Some(c) });
                        }
                        continue;
                    }
                    VRet::Str(s) => collected.push(Format { format: s, respect_prefix }),
                    VRet::Null => continue,
                    VRet::Undefined => {}
                }
                if self.css[clone].raws.needed_backup {
                    self.css[clone].raws.needed_backup = false;
                    let sep = self.cfg.separator.clone();
                    let mut err = None;
                    self.css.walk_rules(clone, &mut |css, r| {
                        let Some(before) = css[r].raws.more.as_mut().and_then(|m| m.original_selector.take()) else { return Flow::Continue };
                        if before.is_empty() {
                            return Flow::Continue;
                        }
                        if before == css[r].selector {
                            return Flow::Continue;
                        }
                        let modified = css[r].selector.clone();
                        let mut sel = Sel::new();
                        let rebuilt = match selector::parse(&mut sel, &before) {
                            Ok(root) => {
                                sel.walk_classes(root, &mut |s, c| {
                                    let v = format!("{variant}{sep}{}", s[c].val());
                                    s.set_class_value(c, &v);
                                    SFlow::Continue
                                });
                                sel.to_string(root)
                            }
                            Err(e) => {
                                err = Some(e.0);
                                return Flow::Stop;
                            }
                        };
                        collected.push(Format { format: modified.replacen(&rebuilt, "&", 1), respect_prefix });
                        css[r].selector = before;
                        Flow::Continue
                    });
                    if let Some(e) = err {
                        return Err(e);
                    }
                }
                let first = self.css.first(clone).unwrap();
                self.css[first].tw_mut().parent_layer = Some(meta.layer.name().to_string());
                let sort = self.offsets.apply_variant_offset(&meta.sort, &entry.sort, opt.clone());
                let mut formats = meta.collected_formats.clone().unwrap_or_default();
                formats.extend(collected);
                result.push((Meta { sort, collected_formats: Some(formats), ..meta.clone() }, first));
            }
        }
        Ok(result)
    }

    /// `parseRules` for a dynamic rule set (always plain objects here)
    fn parse_value_rules(&mut self, v: &Value) -> Result<Vec<NodeId>, String> {
        parse_object_styles(&mut self.css, v)
    }

    fn extract_arbitrary_property(&mut self, class_candidate: &str) -> Option<Vec<(Meta, PluginRule)>> {
        // /^\[([a-zA-Z0-9-_]+):(\S+)\]$/
        let inner = class_candidate.strip_prefix('[')?.strip_suffix(']')?;
        let colon = inner.find(':')?;
        let property = &inner[..colon];
        let value = &inner[colon + 1..];
        if property.is_empty() || !property.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
            return None;
        }
        if value.is_empty() || value.chars().any(crate::js::is_js_whitespace) {
            return None;
        }
        // IS_VALID_PROPERTY_NAME = /^[a-z_-]/
        if !property.starts_with(|c: char| c.is_ascii_lowercase() || c == '_' || c == '-') {
            return None;
        }
        if !is_syntactically_valid_property_value(value) {
            return None;
        }
        let normalized = normalize(value, Some(property));
        if !is_parsable_css_value(property, &normalized) {
            return None;
        }
        let sort = self.offsets.arbitrary_property(class_candidate);
        let class = as_class(class_candidate);
        let property = property.to_string();
        let f: super::context::DynRule = Arc::new(move |_, _, _| {
            let mut inner = Object::new();
            inner.insert(property.clone(), Value::str(normalized.as_str()));
            let mut o = Object::new();
            o.insert(class.clone(), inner.into_value());
            vec![o.into_value()]
        });
        let options = Arc::new(PluginOptions { respect_important: Some(true), ..Default::default() });
        Some(vec![(Meta { sort, layer: Layer::Utilities, options: Some(options), important: false, collected_formats: None }, PluginRule::Dynamic(f))])
    }

    /// `resolveMatchedPlugins(classCandidate, context)` → (plugins, modifier) in order
    fn resolve_matched_plugins(&mut self, class_candidate: &str) -> Vec<(Vec<(Meta, PluginRule)>, String)> {
        let mut out = Vec::new();
        if let Some(p) = self.candidate_rule_map.get(class_candidate) {
            out.push((p.clone(), "DEFAULT".to_string()));
        }
        if let Some(p) = self.extract_arbitrary_property(class_candidate) {
            out.push((p, "DEFAULT".to_string()));
        }
        let prefix = self.cfg.prefix.clone();
        let mut candidate_prefix = class_candidate.to_string();
        let mut negative = false;
        let has_matching_prefix = candidate_prefix.starts_with(&prefix) || candidate_prefix.starts_with(&format!("-{prefix}"));
        if candidate_prefix.as_bytes().get(prefix.len()) == Some(&b'-') && has_matching_prefix {
            negative = true;
            candidate_prefix = format!("{prefix}{}", &candidate_prefix[prefix.len() + 1..]);
        }
        if negative {
            if let Some(p) = self.candidate_rule_map.get(&candidate_prefix) {
                out.push((p.clone(), "-DEFAULT".to_string()));
            }
        }
        for (prefix, modifier) in candidate_permutations(&candidate_prefix) {
            if let Some(p) = self.candidate_rule_map.get(&prefix) {
                out.push((p.clone(), if negative { format!("-{modifier}") } else { modifier }));
            }
        }
        out
    }

    /// `resolveMatches(candidate, context)`
    pub fn resolve_matches(&mut self, candidate: &str) -> Result<Vec<Match>, String> {
        let separator = self.cfg.separator.clone();
        let mut parts: Vec<String> = if candidate == NOT_ON_DEMAND {
            vec![NOT_ON_DEMAND.to_string()]
        } else {
            split_at_top_level_only(candidate, &separator).into_iter().map(|s| s.to_string()).collect()
        };
        parts.reverse();
        let mut class_candidate = parts[0].clone();
        let variants: Vec<String> = parts[1..].to_vec();
        let mut important = false;
        if let Some(rest) = class_candidate.strip_prefix('!') {
            important = true;
            class_candidate = rest.to_string();
        }

        let mut out = Vec::new();
        for (plugins, modifier) in self.resolve_matched_plugins(&class_candidate) {
            let mut matches: Vec<Vec<Match>> = Vec::new();
            let mut types_by_matches: Vec<Option<Vec<String>>> = Vec::new();
            let is_only_plugin = plugins.len() == 1;
            for (meta, plugin) in plugins {
                let mut per_plugin: Vec<Match> = Vec::new();
                match &plugin {
                    PluginRule::Dynamic(f) => {
                        for rule_set in f(self.cfg, &modifier, is_only_plugin) {
                            for rule in self.parse_value_rules(&rule_set)? {
                                per_plugin.push((meta.clone(), rule));
                            }
                        }
                    }
                    PluginRule::Static(node) => {
                        if modifier == "DEFAULT" || modifier == "-DEFAULT" {
                            per_plugin.push((meta.clone(), *node));
                        }
                    }
                }
                if !per_plugin.is_empty() {
                    let empty = PluginOptions::default();
                    let opts = meta.options.as_deref().unwrap_or(&empty);
                    let matching: Vec<String> = get_matching_types(&opts.types, &modifier, opts, self.cfg).into_iter().map(|(_, t, _)| t).collect();
                    types_by_matches.push(if matching.is_empty() { None } else { Some(matching) });
                    matches.push(per_plugin);
                }
            }

            if is_arbitrary_value(&modifier) {
                if matches.len() > 1 {
                    let has_any = |m: &Vec<Match>| m.iter().any(|(meta, _)| meta.options.as_ref().is_some_and(|o| o.types.iter().any(|t| t.ty == "any")));
                    let mut with_any = Vec::new();
                    let mut without_any = Vec::new();
                    for (i, m) in matches.iter().enumerate() {
                        if has_any(m) {
                            with_any.push(i);
                        } else {
                            without_any.push(i);
                        }
                    }
                    let fallback = self.find_fallback(&without_any, &matches, &types_by_matches)?
                        .or(self.find_fallback(&with_any, &matches, &types_by_matches)?);
                    match fallback {
                        Some(i) => {
                            let m = matches.swap_remove(i);
                            matches = vec![m];
                        }
                        None => {
                            // ambiguous: log.warn and skip this plugin set
                            continue;
                        }
                    }
                }
                let mut filtered = Vec::new();
                for list in matches {
                    let mut l = Vec::new();
                    for m in list {
                        if self.is_parsable_node(m.1) {
                            l.push(m);
                        }
                    }
                    filtered.push(l);
                }
                matches = filtered;
            }

            let mut flat: Vec<Match> = matches.into_iter().flatten().collect();
            // recordCandidates
            for (meta, rule) in flat.iter() {
                let n = self.css[*rule].tw_mut();
                n.class_candidate = Some(class_candidate.clone());
                n.preserve_source = Some(meta.options.as_ref().and_then(|o| o.preserve_source).unwrap_or(false));
            }
            flat = self.apply_prefix(flat)?;
            if important {
                flat = self.apply_important(flat, &class_candidate)?;
            }
            for v in &variants {
                flat = self.apply_variant(v, flat)?;
            }
            for (meta, rule) in flat {
                self.css[rule].tw_mut().candidate = Some(candidate.to_string());
                if let Some(m) = self.apply_final_format((meta, rule), candidate)? {
                    out.push(m);
                }
            }
        }
        Ok(out)
    }

    fn find_fallback(&mut self, idxs: &[usize], matches: &[Vec<Match>], types_by: &[Option<Vec<String>>]) -> Result<Option<usize>, String> {
        if idxs.len() == 1 {
            return Ok(Some(idxs[0]));
        }
        for &i in idxs {
            let matching = types_by[i].clone().unwrap_or_default();
            let found = matches[i].iter().any(|(meta, rule)| {
                if !self.is_parsable_node(*rule) {
                    return false;
                }
                meta.options.as_ref().is_some_and(|o| o.types.iter().any(|t| matching.contains(&t.ty) && t.prefer_on_conflict))
            });
            if found {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    fn is_parsable_node(&mut self, node: NodeId) -> bool {
        let mut ok = true;
        let decls: Vec<NodeId> = if self.css[node].kind == Kind::Decl {
            vec![]
        } else {
            self.css.descendants(node).into_iter().filter(|n| self.css[*n].kind == Kind::Decl).collect()
        };
        for d in decls {
            if !is_parsable_css_value(&self.css[d].prop.clone(), &self.css[d].value.clone()) {
                ok = false;
                break;
            }
        }
        ok
    }

    fn apply_final_format(&mut self, m: Match, candidate: &str) -> Result<Option<Match>, String> {
        let (meta, rule) = m;
        let Some(formats) = meta.collected_formats.clone() else {
            return Ok(Some((meta, rule)));
        };
        let prefix = self.cfg.prefix.clone();
        let separator = self.cfg.separator.clone();
        let mut fsel = Sel::new();
        let final_format = match format_variant_selector(&mut fsel, &formats, &prefix, candidate) {
            Ok(r) => super::format_variant_selector::ParsedFormat::new(fsel, r),
            Err(_) => return Ok(None),
        };
        let container = self.css.root_with(&[rule]);
        let mut valid = true;
        self.css.walk_rules(container, &mut |css, r| {
            if in_keyframes(css, r) {
                return Flow::Continue;
            }
            let current = css[r].selector.clone();
            match finalize_selector(&current, FormatsArg::Parsed(&final_format), &prefix, &separator, candidate, None) {
                Ok(None) => {
                    css.remove(r);
                    Flow::Continue
                }
                Ok(Some(s)) => {
                    css[r].selector = s;
                    Flow::Continue
                }
                Err(_) => {
                    valid = false;
                    Flow::Stop
                }
            }
        });
        if !valid {
            return Ok(None);
        }
        if self.css.children(container).is_empty() {
            return Ok(None);
        }
        let first = self.css.first(container).unwrap();
        Ok(Some((meta, first)))
    }

    /// `generateRules(candidates, context, isSorting)`
    pub fn generate_rules(&mut self, candidates: &[String]) -> Result<Vec<(RuleOffset, NodeId)>, String> {
        let mut all = Vec::new();
        for candidate in candidates {
            if self.not_class_cache.contains(candidate) {
                continue;
            }
            if let Some(idxs) = self.candidate_rule_cache.get(candidate) {
                for &i in idxs {
                    if let Some(e) = &self.rule_cache[i] {
                        all.push(e.clone());
                    }
                }
                continue;
            }
            let matches = match self.resolve_matches(candidate) {
                Ok(m) => m,
                Err(e) => {
                    // the JS build would throw here; a junk candidate is dropped instead
                    self.errors.push(format!("{candidate}: {e}"));
                    Vec::new()
                }
            };
            if matches.is_empty() {
                Arc::make_mut(&mut self.not_class_cache).insert(candidate.clone());
                continue;
            }
            Arc::make_mut(&mut self.class_cache).insert(candidate.clone(), matches.clone());
            let mut idxs = Vec::new();
            for (meta, rule) in matches {
                let respect_important = meta.options.as_ref().and_then(|o| o.respect_important).unwrap_or(false);
                let mut rule = rule;
                if respect_important {
                    rule = self.apply_important_strategy(rule)?;
                }
                let entry = (meta.sort.clone(), rule);
                idxs.push(self.rule_cache.len());
                Arc::make_mut(&mut self.rule_cache).push(Some(entry.clone()));
                all.push(entry);
            }
            Arc::make_mut(&mut self.candidate_rule_cache).insert(candidate.clone(), idxs);
        }
        Ok(all)
    }

    fn apply_important_strategy(&mut self, rule: NodeId) -> Result<NodeId, String> {
        let important = self.cfg.important.clone();
        match &important {
            Value::Bool(true) => {
                let container = self.css.root_with(&[rule]);
                self.css.walk_rules(container, &mut |css, r| {
                    if in_keyframes(css, r) {
                        return Flow::Continue;
                    }
                    css.walk_decls(r, &mut |css, d| {
                        let p = css[d].parent.unwrap();
                        if css[p].kind == Kind::Rule && !in_keyframes(css, p) {
                            css[d].important = true;
                        }
                        Flow::Continue
                    });
                    Flow::Continue
                });
                Ok(self.css.first(container).unwrap())
            }
            Value::Str(imp) => {
                let imp = imp.to_string();
                let container = self.css.root_with(&[rule]);
                let mut err = None;
                self.css.walk_rules(container, &mut |css, r| {
                    if in_keyframes(css, r) {
                        return Flow::Continue;
                    }
                    let sels = css.selectors(r);
                    let mut out = Vec::new();
                    for s in sels {
                        match apply_important_selector(&s, &imp) {
                            Ok(x) => out.push(x),
                            Err(e) => {
                                err = Some(e.0);
                                return Flow::Stop;
                            }
                        }
                    }
                    css.set_selectors(r, &out);
                    Flow::Continue
                });
                if let Some(e) = err {
                    return Err(e);
                }
                Ok(self.css.first(container).unwrap())
            }
            _ => Ok(rule),
        }
    }
}

/// `updateAllClasses(selectors, updateClass)`
pub fn update_all_classes(sel: &mut Sel, root: selector::SId, f: &dyn Fn(&str) -> String) {
    sel.walk_classes(root, &mut |s, c| {
        let v = f(s[c].val());
        s.set_class_value(c, &v);
        if let Some(r) = s[c].raws.as_mut() {
            if let Some(rv) = r.value.as_mut() {
                if !rv.is_empty() {
                    *rv = super::util::escape_commas(rv);
                }
            }
        }
        SFlow::Continue
    });
}

/// `/(.)(-?)\[(.*)\]/g.exec(variant)` → (char, separator, value, index)
fn match_arbitrary_variant(v: &str) -> Option<(char, String, String, usize)> {
    let chars: Vec<(usize, char)> = v.char_indices().collect();
    // leftmost match: first position i with chars[i] any (not a line terminator), then optional
    // '-', then '[', then greedy `.*` and the last `]` (on the same line)
    for (k, &(i, c)) in chars.iter().enumerate() {
        if matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            continue;
        }
        let j = k + 1;
        let try_at = |j: usize, sep: &str| -> Option<(char, String, String, usize)> {
            if chars.get(j).map(|x| x.1) != Some('[') {
                return None;
            }
            let start = chars[j].0 + 1;
            let rest = &v[start..];
            let line_end = rest.find(['\n', '\r', '\u{2028}', '\u{2029}']).unwrap_or(rest.len());
            let seg = &rest[..line_end];
            let close = seg.rfind(']')?;
            Some((c, sep.to_string(), seg[..close].to_string(), i))
        };
        if chars.get(j).map(|x| x.1) == Some('-') {
            if let Some(r) = try_at(j + 1, "-") {
                return Some(r);
            }
        }
        if let Some(r) = try_at(j, "") {
            return Some(r);
        }
    }
    None
}

/// `isParsableCssValue(property, value)`
pub fn is_parsable_css_value(property: &str, value: &str) -> bool {
    let decl = format!("{property}:{value}");
    if looks_like_uri(&decl) {
        return false;
    }
    let mut css = Css::new();
    postcss::parse(&mut css, &format!("a{{{property}:{value}}}")).is_ok()
}

fn looks_like_uri(declaration: &str) -> bool {
    if !declaration.contains("://") {
        return false;
    }
    super::url::is_valid_url_with_host(declaration)
}

pub fn _unused(_: Add, _: SKind) {}
