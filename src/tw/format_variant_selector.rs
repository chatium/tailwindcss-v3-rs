//! `util/formatVariantSelector.js`, `util/pseudoElements.js`, `util/applyImportantSelector.js`

use crate::selector::{self, unesc, SFlow, SId, SKind, SNode, Sel, SelError};

use super::context::Format;
use super::util::{escape_class_name, prefix_selector_ast, split_at_top_level_only};

const MERGE: &str = ":merge";

/// `formatVariantSelector(formats, { context, candidate })` → root in `sel`
pub fn format_variant_selector(sel: &mut Sel, formats: &[Format], prefix: &str, candidate: &str) -> Result<SId, SelError> {
    let mut parsed = Vec::new();
    for f in formats {
        let ast = selector::parse(sel, &f.format)?;
        if f.respect_prefix {
            prefix_selector_ast(sel, ast, prefix, false);
        }
        parsed.push(ast);
    }
    let class = sel.class_name(&escape_class_name(candidate));
    let s = sel.selector_with(vec![class]);
    sel[class].parent = None;
    let mut format_ast = sel.root();
    sel[format_ast].nodes = Some(vec![s]);
    for ast in parsed {
        let (fa, a) = handle_merge_pseudo(sel, format_ast, ast);
        format_ast = fa;
        let ast = a;
        let replacement: Vec<SId> = sel.children(sel.children(format_ast)[0]).to_vec();
        sel.walk_kind(ast, SKind::Nesting, &mut |sel, nesting| {
            sel.replace_with(nesting, &replacement);
            SFlow::Continue
        });
        format_ast = ast;
    }
    Ok(format_ast)
}

fn simple_selector_for_node(sel: &Sel, node: SId) -> Vec<SId> {
    let mut node = node;
    while let Some(p) = sel.prev(node) {
        if sel[p].kind == SKind::Combinator {
            break;
        }
        node = p;
    }
    let mut nodes = Vec::new();
    let mut cur = Some(node);
    while let Some(n) = cur {
        if sel[n].kind == SKind::Combinator {
            break;
        }
        nodes.push(n);
        cur = sel.next(n);
    }
    nodes
}

fn resort_selector(sel: &mut Sel, nodes: &mut Vec<SId>) {
    // `sel.sort(...)` with the fallback `sel.index(a) - sel.index(b)` comparing positions in the
    // array being sorted; during V8's binary insertion the pivot keeps its slot, which this
    // reproduces by comparing original indices.
    let original = nodes.clone();
    crate::js::sort_by(nodes, |a, b| {
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
            let ia = original.iter().position(|x| x == a).unwrap() as i32;
            let ib = original.iter().position(|x| x == b).unwrap() as i32;
            ia - ib
        }
    });
}

/// `eliminateIrrelevantSelectors(sel, base)`
pub fn eliminate_irrelevant_selectors(sel: &mut Sel, s: SId, base: &str) {
    let mut has = false;
    sel.walk(s, &mut |sel, child| {
        if sel[child].kind == SKind::Class && sel[child].val() == base {
            has = true;
            return SFlow::Stop;
        }
        SFlow::Continue
    });
    if !has {
        sel.remove(s);
    }
}

/// `finalizeSelector(current, formats, { context, candidate, base })`
pub fn finalize_selector(
    current: &str,
    formats: FormatsArg,
    prefix: &str,
    separator: &str,
    candidate: &str,
    base: Option<&str>,
) -> Result<Option<String>, SelError> {
    let base = match base {
        Some(b) => b.to_string(),
        None => split_at_top_level_only(candidate, separator).pop().unwrap_or("").to_string(),
    };
    let mut sel = Sel::new();
    let selector = selector::parse(&mut sel, current)?;
    sel.walk_classes(selector, &mut |s, node| {
        if s[node].raws.is_some() && s[node].val().contains(&base) {
            let raw = s[node].raws.as_ref().unwrap().value.clone();
            let raw = raw.unwrap_or_else(|| "undefined".into());
            let escaped = escape_class_name(&unesc(&raw));
            s[node].raws.as_mut().unwrap().value = Some(escaped);
        }
        SFlow::Continue
    });
    let sels: Vec<SId> = sel.children(selector).to_vec();
    sel.each(selector, &mut |s, child| {
        eliminate_irrelevant_selectors(s, child, &base);
        SFlow::Continue
    });
    let _ = sels;
    if sel.len(selector) == 0 {
        return Ok(None);
    }
    let format_ast = match formats {
        FormatsArg::Raw(f) => format_variant_selector(&mut sel, f, prefix, candidate)?,
        FormatsArg::Parsed(s) => s.import_into(&mut sel),
    };

    let simple_start = sel.comment("/*__simple__*/");
    let simple_end = sel.comment("/*__simple__*/");
    let format_nodes: Vec<SId> = {
        let first = sel.children(format_ast)[0];
        sel.children(first).to_vec()
    };
    sel.walk_classes(selector, &mut |sel, node| {
        if sel[node].val() != base {
            return SFlow::Continue;
        }
        let parent = sel[node].parent.unwrap();
        if sel.len(parent) == 1 {
            sel.replace_with(node, &format_nodes);
            return SFlow::Continue;
        }
        let simple = simple_selector_for_node(sel, node);
        sel.insert_before(parent, simple[0], simple_start);
        sel.insert_after(parent, *simple.last().unwrap(), simple_end);
        for &child in &format_nodes {
            let c = sel.clone_node(child);
            sel.insert_before(parent, simple[0], c);
        }
        sel.remove(node);
        let mut simple = simple_selector_for_node(sel, simple_start);
        let first_node = sel.index(parent, simple_start);
        resort_selector(sel, &mut simple);
        let list = sel[parent].nodes.as_mut().unwrap();
        let start = (first_node.max(0) as usize).min(list.len());
        let end = (start + simple.len()).min(list.len());
        list.splice(start..end, simple);
        sel.remove(simple_start);
        sel.remove(simple_end);
        SFlow::Continue
    });

    sel.walk_pseudos(selector, &mut |sel, p| {
        if sel[p].val() == MERGE {
            let nodes = sel.children(p).to_vec();
            sel.replace_with(p, &nodes);
        }
        SFlow::Continue
    });

    let list = sel.children(selector).to_vec();
    sel.each(selector, &mut |sel, s| {
        move_pseudos(sel, s);
        SFlow::Continue
    });
    let _ = list;
    Ok(Some(sel.to_string(selector)))
}

/// A pre-parsed format AST (`formats` that is not an array)
pub struct ParsedFormat {
    sel: Sel,
    root: SId,
}

impl ParsedFormat {
    pub fn new(sel: Sel, root: SId) -> Self {
        ParsedFormat { sel, root }
    }

    fn import_into(&self, target: &mut Sel) -> SId {
        // copy the whole arena so ids shift by a constant offset
        let base = target.n.len() as u32;
        for n in &self.sel.n {
            let mut n = n.clone();
            n.parent = n.parent.map(|p| SId(p.0 + base));
            if let Some(list) = n.nodes.as_mut() {
                for c in list.iter_mut() {
                    *c = SId(c.0 + base);
                }
            }
            target.n.push(n);
        }
        SId(self.root.0 + base)
    }
}

pub enum FormatsArg<'a> {
    Raw(&'a [Format]),
    Parsed(&'a ParsedFormat),
}

/// `handleMergePseudo(selector, format)`
fn handle_merge_pseudo(sel: &mut Sel, selector: SId, format: SId) -> (SId, SId) {
    let mut merges: Vec<(SId, String)> = Vec::new();
    sel.walk_pseudos(selector, &mut |s, p| {
        if s[p].val() == MERGE {
            let first = s.children(p)[0];
            let v = s.to_string(first);
            merges.push((p, v));
        }
        SFlow::Continue
    });
    sel.walk_pseudos(format, &mut |s, pseudo| {
        if s[pseudo].val() != MERGE {
            return SFlow::Continue;
        }
        let first = s.children(pseudo)[0];
        let value = s.to_string(first);
        let Some((existing, _)) = merges.iter().find(|(_, v)| *v == value).cloned() else {
            return SFlow::Continue;
        };
        let mut attachments = Vec::new();
        let mut next = s.next(pseudo);
        while let Some(n) = next {
            if s[n].kind == SKind::Combinator {
                break;
            }
            attachments.push(n);
            next = s.next(n);
        }
        let combinator = next;
        let clones: Vec<SId> = attachments.iter().map(|n| s.clone_node(*n)).collect();
        let new_sel = s.selector_with(clones);
        let ep = s[existing].parent.unwrap();
        s.insert_after(ep, existing, new_sel);
        s.remove(pseudo);
        for a in attachments {
            s.remove(a);
        }
        if let Some(c) = combinator {
            if s[c].kind == SKind::Combinator {
                s.remove(c);
            }
        }
        SFlow::Continue
    });
    (selector, format)
}

// ---- pseudoElements.js

fn element_properties(value: &str) -> Option<&'static [&'static str]> {
    Some(match value {
        "::after" | "::backdrop" | "::before" | "::first-letter" | "::first-line" | "::marker" | "::placeholder" | "::selection" => {
            &["terminal", "jumpable"]
        }
        "::cue" | "::cue-region" | "::grammar-error" | "::slotted" | "::spelling-error" | "::target-text" => &["terminal"],
        "::part" | "::file-selector-button" => &["terminal", "actionable"],
        "::deep" | "::v-deep" | "::ng-deep" => &["actionable"],
        ":after" | ":before" | ":first-letter" | ":first-line" => &["terminal", "jumpable"],
        ":where" | ":is" | ":has" => &[],
        _ => return None,
    })
}

fn properties_for_pseudo(sel: &Sel, p: SId) -> &'static [&'static str] {
    element_properties(sel[p].val()).unwrap_or(&["terminal", "actionable"])
}

fn is_pseudo_element(sel: &Sel, n: SId) -> bool {
    let v = sel[n].val();
    v.starts_with("::") || element_properties(v).is_some()
}

fn is_movable_pseudo_element(sel: &Sel, n: SId) -> bool {
    is_pseudo_element(sel, n) && properties_for_pseudo(sel, n).contains(&"terminal")
}

fn is_attachable_pseudo_class(sel: &Sel, n: SId, pseudo: SId) -> bool {
    if sel[n].kind != SKind::Pseudo {
        return false;
    }
    if is_pseudo_element(sel, n) {
        return false;
    }
    properties_for_pseudo(sel, pseudo).contains(&"actionable")
}

/// (selector, pseudo, attachedTo)
type Movable = (SId, SId);

fn movable_pseudos(sel: &Sel, s: SId) -> (Vec<Movable>, Option<SId>) {
    let mut buffer: Vec<Movable> = Vec::new();
    let mut last_seen: Option<SId> = None;
    for &node in sel.children(s) {
        match sel[node].kind {
            SKind::Combinator => {
                buffer.retain(|(_, n)| properties_for_pseudo(sel, *n).contains(&"jumpable"));
                last_seen = None;
            }
            SKind::Pseudo => {
                if is_movable_pseudo_element(sel, node) {
                    last_seen = Some(node);
                    buffer.push((s, node));
                } else if last_seen.is_some_and(|l| is_attachable_pseudo_class(sel, node, l)) {
                    buffer.push((s, node));
                } else {
                    last_seen = None;
                }
                for &sub in sel.children(node) {
                    let (movable, last_in_sub) = movable_pseudos(sel, sub);
                    if last_in_sub.is_some() {
                        last_seen = last_in_sub;
                    }
                    buffer.extend(movable);
                }
            }
            _ => {}
        }
    }
    (buffer, last_seen)
}

/// `movePseudos(sel)`
pub fn move_pseudos(sel: &mut Sel, s: SId) {
    let (pseudos, _) = movable_pseudos(sel, s);
    for (owner, p) in &pseudos {
        sel.remove_child(*owner, *p);
    }
    let list = sel[s].nodes.as_mut().unwrap();
    for (_, p) in pseudos {
        list.push(p);
    }
}

/// `applyImportantSelector(selector, important)`
pub fn apply_important_selector(selector: &str, important: &str) -> Result<String, SelError> {
    let mut sel = Sel::new();
    let root = selector::parse(&mut sel, selector)?;
    let sels: Vec<SId> = sel.children(root).to_vec();
    sel.each(root, &mut |sel, s| {
        let should_wrap = sel.children(s).iter().any(|n| sel[*n].kind == SKind::Combinator);
        if should_wrap {
            let c = sel.clone_node(s);
            let p = sel.pseudo_with(":is", vec![c]);
            sel[s].nodes = Some(vec![p]);
        }
        move_pseudos(sel, s);
        SFlow::Continue
    });
    let _ = sels;
    Ok(format!("{important} {}", sel.to_string(root)))
}

pub fn _unused(_: SNode) {}
