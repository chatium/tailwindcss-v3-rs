//! `util/parseObjectStyles.js`: `postcss-js` parsing of CSS-in-JS objects followed by
//! `postcss-nested` (`bubble: ['screen']`).

use crate::js::{self, Value};
use crate::postcss::{Add, Css, Flow, Kind, NodeId};
use crate::selector::{self, SFlow, SId, SKind, Sel};

/// `parseObjectStyles(styles)`: the returned nodes keep the temporary root as parent.
pub fn parse_object_styles(css: &mut Css, styles: &Value) -> Result<Vec<NodeId>, String> {
    let list: Vec<Value> = match styles {
        Value::Arr(a) => a.to_vec(),
        v => vec![v.clone()],
    };
    let mut out = Vec::new();
    for style in list {
        let root = css.root();
        js_parse(css, &style, root)?;
        nested(css, root)?;
        out.extend(css.children(root).iter().copied());
    }
    Ok(out)
}

// ---- postcss-js/parser.js

const UNITLESS: &[&str] = &[
    "box-flex",
    "box-flex-group",
    "column-count",
    "flex",
    "flex-grow",
    "flex-positive",
    "flex-shrink",
    "flex-negative",
    "font-weight",
    "line-clamp",
    "line-height",
    "opacity",
    "order",
    "orphans",
    "tab-size",
    "widows",
    "z-index",
    "zoom",
    "fill-opacity",
    "stroke-dashoffset",
    "stroke-opacity",
    "stroke-width",
];

fn dashify(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
        }
        out.push(c);
    }
    if let Some(rest) = out.strip_prefix("ms-") {
        out = format!("-ms-{rest}");
    }
    // toLowerCase (full Unicode lowercasing like JS)
    out.to_lowercase()
}

/// `/\s*!important\s*$/i`
fn strip_important(v: &str) -> Option<String> {
    let t = v.trim_end_matches(js::is_js_whitespace);
    let lower = t.to_ascii_lowercase();
    if lower.ends_with("!important") {
        let base = &t[..t.len() - "!important".len()];
        let base = base.trim_end_matches(js::is_js_whitespace);
        Some(base.to_string())
    } else {
        None
    }
}

fn js_decl(css: &mut Css, parent: NodeId, name: &str, value: &Value) {
    if matches!(value, Value::Bool(false) | Value::Null) {
        return;
    }
    let mut name = if name.starts_with("--") { name.to_string() } else { dashify(name) };
    let value: String = match value {
        Value::Num(n) => {
            if *n == 0.0 || UNITLESS.contains(&name.as_str()) {
                js::number_to_string(*n)
            } else {
                format!("{}px", js::number_to_string(*n))
            }
        }
        v => v.to_js_string(),
    };
    if name == "css-float" {
        name = "float".into();
    }
    let d = if let Some(v) = strip_important(&value) {
        let d = css.decl(&name, &v);
        css[d].important = true;
        d
    } else {
        css.decl(&name, &value)
    };
    css.push(parent, d);
}

fn js_at_rule(css: &mut Css, parent: NodeId, name: &str, params: &str, value: &Value) -> Result<(), String> {
    let node = css.at_rule(name, params);
    if value.is_object_like() || matches!(value, Value::Null) {
        css[node].nodes = Some(Vec::new());
        js_parse(css, value, node)?;
    }
    css.push(parent, node);
    Ok(())
}

/// `/@(\S+)(\s+([\W\w]*)\s*)?/` → (name, params)
fn at_parts(key: &str) -> Option<(String, String)> {
    let at = key.find('@')?;
    let rest = &key[at + 1..];
    let name_end = rest.find(js::is_js_whitespace).unwrap_or(rest.len());
    if name_end == 0 {
        return None;
    }
    let name = rest[..name_end].to_string();
    let after = &rest[name_end..];
    let params_start = after.len() - after.trim_start_matches(js::is_js_whitespace).len();
    let params = if params_start > 0 { after[params_start..].to_string() } else { String::new() };
    Some((name, params))
}

fn js_parse(css: &mut Css, obj: &Value, parent: NodeId) -> Result<(), String> {
    let entries: Vec<(js::Str, Value)> = match obj {
        Value::Obj(o) => o.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Value::Arr(a) => a.iter().enumerate().map(|(i, v)| (i.to_string().into(), v.clone())).collect(),
        Value::Str(_) => {
            let mut o = js::Object::new();
            super::config::spread(&mut o, obj);
            o.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        }
        _ => Vec::new(),
    };
    for (name, value) in entries {
        if value.is_nullish() {
            continue;
        }
        if name.starts_with('@') {
            let Some((at_name, params)) = at_parts(&name) else {
                return Err("Cannot read properties of null (reading '1')".into());
            };
            if let Value::Arr(items) = &value {
                for i in items.iter() {
                    js_at_rule(css, parent, &at_name, &params, i)?;
                }
            } else {
                js_at_rule(css, parent, &at_name, &params, &value)?;
            }
        } else if let Value::Arr(items) = &value {
            for i in items.iter() {
                js_decl(css, parent, &name, i);
            }
        } else if value.is_object() {
            let node = css.rule(&name);
            js_parse(css, &value, node)?;
            css.push(parent, node);
        } else {
            js_decl(css, parent, &name, &value);
        }
    }
    Ok(())
}

// ---- postcss-nested

fn is_bubble(name: &str) -> bool {
    matches!(name, "media" | "supports" | "layer" | "container" | "starting-style" | "screen")
}

fn is_unwrap(name: &str) -> bool {
    matches!(name, "document" | "font-face" | "keyframes" | "-webkit-keyframes" | "-moz-keyframes")
}

/// Runs the `Rule` visitor the way postcss's LazyResult walks: a rule's listener runs
/// before its children are visited, and walks repeat until nothing changes.
fn nested(css: &mut Css, root: NodeId) -> Result<(), String> {
    let mut err: Option<String> = None;
    loop {
        let mut changed = false;
        walk_visit(css, root, &mut changed, &mut err);
        if let Some(e) = err {
            return Err(e);
        }
        if !changed {
            break;
        }
    }
    Ok(())
}

fn walk_visit(css: &mut Css, node: NodeId, changed: &mut bool, err: &mut Option<String>) {
    css.each(node, &mut |css, child| {
        if err.is_some() {
            return Flow::Stop;
        }
        if css[child].kind == Kind::Rule {
            match rule_visitor(css, child) {
                Ok(true) => *changed = true,
                Ok(false) => {}
                Err(e) => {
                    *err = Some(e);
                    return Flow::Stop;
                }
            }
            // a removed rule is not walked further
            if css[child].parent.is_none() {
                return Flow::Continue;
            }
        }
        if css[child].nodes.is_some() {
            walk_visit(css, child, changed, err);
        }
        Flow::Continue
    });
}

/// `parse(rawSelector)` → first selector of the parsed root
fn parse_selector(sel: &mut Sel, raw: &str) -> Result<SId, String> {
    match selector::parse(sel, raw) {
        Ok(root) => Ok(sel.first(root).unwrap()),
        Err(e) => {
            if raw.contains(':') {
                Err("Missed semicolon".into())
            } else {
                Err(e.0)
            }
        }
    }
}

fn interpolate_amp_in_selector(sel: &mut Sel, nodes: SId, parent: SId) -> Result<bool, String> {
    let mut replaced = false;
    let mut err = None;
    sel.each(nodes, &mut |sel, node| {
        if sel[node].kind == SKind::Nesting {
            let cloned_parent = sel.clone_node(parent);
            if sel[node].val() != "&" {
                let text = sel[node].val().replacen('&', &sel.to_string(cloned_parent), 1);
                match parse_selector(sel, &text) {
                    Ok(n) => sel.replace_with(node, &[n]),
                    Err(e) => {
                        err = Some(e);
                        return SFlow::Stop;
                    }
                }
            } else {
                sel.replace_with(node, &[cloned_parent]);
            }
            replaced = true;
        } else if sel[node].nodes.is_some() {
            match interpolate_amp_in_selector(sel, node, parent) {
                Ok(true) => replaced = true,
                Ok(false) => {}
                Err(e) => {
                    err = Some(e);
                    return SFlow::Stop;
                }
            }
        }
        SFlow::Continue
    });
    if let Some(e) = err {
        return Err(e);
    }
    Ok(replaced)
}

fn merge_selectors(css: &Css, parent: NodeId, child: NodeId) -> Result<Vec<String>, String> {
    let mut merged = Vec::new();
    for sel_str in css.selectors(parent) {
        let mut sel = Sel::new();
        let parent_node = parse_selector(&mut sel, &sel_str)?;
        for selector_str in css.selectors(child) {
            if selector_str.is_empty() {
                continue;
            }
            let node = parse_selector(&mut sel, &selector_str)?;
            let replaced = interpolate_amp_in_selector(&mut sel, node, parent_node)?;
            if !replaced {
                let mut comb = selector::SNode::new(SKind::Combinator);
                comb.value = Some(" ".into());
                let comb = sel.add(comb);
                sel.prepend(node, comb);
                let pc = sel.clone_node(parent_node);
                sel.prepend(node, pc);
            }
            merged.push(sel.to_string(node));
        }
    }
    Ok(merged)
}

fn break_out(css: &mut Css, child: NodeId, after: NodeId) -> NodeId {
    let mut prev = css.prev(child);
    css.after(after, Add::Node(child));
    while let Some(p) = prev {
        if css[p].kind != Kind::Comment {
            break;
        }
        let next_prev = css.prev(p);
        css.after(after, Add::Node(p));
        prev = next_prev;
    }
    child
}

fn atrule_childs(css: &mut Css, rule: NodeId, atrule: NodeId, bubbling: bool, merge_sels: bool) -> Result<(), String> {
    let mut children = Vec::new();
    let mut err = None;
    css.each(atrule, &mut |css, child| {
        let kind = css[child].kind;
        if kind == Kind::Rule && bubbling {
            if merge_sels {
                match merge_selectors(css, rule, child) {
                    Ok(m) => css.set_selectors(child, &m),
                    Err(e) => {
                        err = Some(e);
                        return Flow::Stop;
                    }
                }
            }
        } else if kind == Kind::AtRule && css[child].nodes.is_some() {
            if is_bubble(&css[child].name.clone()) {
                if let Err(e) = atrule_childs(css, rule, child, merge_sels, merge_sels) {
                    err = Some(e);
                    return Flow::Stop;
                }
            } else {
                // atrule[rootRuleMergeSel] is undefined here (no @at-root support needed)
                children.push(child);
            }
        } else {
            children.push(child);
        }
        Flow::Continue
    });
    if let Some(e) = err {
        return Err(e);
    }
    if bubbling && !children.is_empty() {
        let clone = css.clone_empty(rule);
        for c in children {
            css.append(clone, Add::Node(c));
        }
        css.prepend(atrule, Add::Node(clone));
    }
    Ok(())
}

fn pick_declarations(css: &mut Css, selector: &str, declarations: Vec<NodeId>, after: NodeId) -> NodeId {
    let parent = css.rule(selector);
    css.append(parent, Add::Nodes(declarations));
    css.after(after, Add::Node(parent));
    parent
}

/// The `Rule(rule)` listener; returns whether it changed the tree.
fn rule_visitor(css: &mut Css, rule: NodeId) -> Result<bool, String> {
    let mut unwrapped = false;
    let mut after = rule;
    let mut copy_declarations = false;
    let mut declarations: Vec<NodeId> = Vec::new();
    let mut err = None;
    css.each(rule, &mut |css, child| {
        let kind = css[child].kind;
        if kind == Kind::Rule {
            if !declarations.is_empty() {
                let sel = css[rule].selector.clone();
                after = pick_declarations(css, &sel, std::mem::take(&mut declarations), after);
            }
            copy_declarations = true;
            unwrapped = true;
            match merge_selectors(css, rule, child) {
                Ok(m) => css.set_selectors(child, &m),
                Err(e) => {
                    err = Some(e);
                    return Flow::Stop;
                }
            }
            after = break_out(css, child, after);
        } else if kind == Kind::AtRule {
            if !declarations.is_empty() {
                let sel = css[rule].selector.clone();
                after = pick_declarations(css, &sel, std::mem::take(&mut declarations), after);
            }
            let name = css[child].name.clone();
            if is_bubble(&name) {
                copy_declarations = true;
                unwrapped = true;
                if let Err(e) = atrule_childs(css, rule, child, true, true) {
                    err = Some(e);
                    return Flow::Stop;
                }
                after = break_out(css, child, after);
            } else if is_unwrap(&name) {
                copy_declarations = true;
                unwrapped = true;
                if let Err(e) = atrule_childs(css, rule, child, false, false) {
                    err = Some(e);
                    return Flow::Stop;
                }
                after = break_out(css, child, after);
            } else if copy_declarations {
                declarations.push(child);
            }
        } else if kind == Kind::Decl && copy_declarations {
            declarations.push(child);
        }
        Flow::Continue
    });
    if let Some(e) = err {
        return Err(e);
    }
    if !declarations.is_empty() {
        let sel = css[rule].selector.clone();
        pick_declarations(css, &sel, declarations, after);
    }
    if unwrapped {
        css[rule].raws.semicolon = Some(true);
        if css.children(rule).is_empty() {
            css.remove(rule);
        }
    }
    Ok(unwrapped)
}
