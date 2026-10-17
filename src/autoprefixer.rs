//! Port of `autoprefixer@10.4` as the Play bundle runs it: `autoprefixer({ remove: false })` with
//! the bundle's baked browser list (`and_chr 114`, …, `samsung 21`) and its pre-selected prefix
//! table (`data/autoprefixer.json`, extracted from the bundle). Only the code paths that table
//! can reach are ported: no selector prefixes, no `@keyframes`/`@viewport`/resolution, no grid.

use std::sync::OnceLock;

use crate::js::{self, Value};
use crate::postcss::{self, Add, Css, CssError, Flow, Kind, NodeId};
use crate::value_parser::{self, VNode};

/// `Browsers.prefixes()`: every agent prefix, longest first
const ALL_PREFIXES: [&str; 4] = ["-webkit-", "-moz-", "-ms-", "-o-"];

fn with_prefix(value: &str) -> bool {
    ALL_PREFIXES.iter().any(|p| value.contains(p))
}

/// `vendor.prefix(prop)`: /^(-\w+-)/
fn vendor_prefix(prop: &str) -> &str {
    let b = prop.as_bytes();
    if b.first() != Some(&b'-') {
        return "";
    }
    let mut i = 1;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
        i += 1;
    }
    // `\w+` is greedy but may backtrack to find the closing `-`
    let mut j = i;
    while j > 1 {
        if b.get(j) == Some(&b'-') && j > 1 {
            return &prop[..=j];
        }
        j -= 1;
    }
    ""
}

/// `vendor.unprefixed(prop)`
fn vendor_unprefixed(prop: &str) -> &str {
    &prop[vendor_prefix(prop).len()..]
}

/// `utils.removeNote`
fn remove_note(s: &str) -> &str {
    s.split(' ').next().unwrap_or(s)
}

// ---- data

struct Feature {
    name: String,
    add: Vec<String>,
    props: Option<Vec<String>>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum DeclKind {
    Generic,
    UserSelect,
    BackgroundClip,
    TextDecoration,
    TextDecorationSkipInk,
    MaskBorder,
    MaskComposite,
    PrintColorAdjust,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ValueKind {
    Generic,
    Intrinsic,
    ImageSet,
    CrossFade,
}

struct DeclPrefixer {
    name: String,
    prefixes: Vec<String>,
    kind: DeclKind,
}

struct ValuePrefixer {
    name: String,
    prefixes: Vec<String>,
    kind: ValueKind,
}

struct OldValue {
    string: String,
    /// word regexp of `utils.regexp(prefixed)` or the intrinsic variant
    intrinsic: bool,
}

struct Data {
    /// `prefixes.add[prop]` declaration prefixers
    decls: Vec<(String, DeclPrefixer)>,
    /// `prefixes.add[prop].values`
    values: Vec<(String, Vec<usize>)>,
    value_prefixers: Vec<ValuePrefixer>,
    /// cleaner `remove[prop].remove`
    remove_props: Vec<String>,
    /// cleaner `remove[prop].values`
    remove_values: Vec<(String, Vec<OldValue>)>,
}

static DATA_JSON: &str = include_str!("autoprefixer_data.json");

const BROWSERS: [&str; 14] = [
    "and_chr 114",
    "and_uc 15.5",
    "chrome 114",
    "chrome 113",
    "chrome 109",
    "edge 114",
    "firefox 114",
    "ios_saf 16.5",
    "ios_saf 16.4",
    "ios_saf 16.3",
    "ios_saf 16.1",
    "opera 99",
    "safari 16.5",
    "samsung 21",
];

/// `Browsers#prefix(browser)` for the bundle's agents table
fn browser_prefix(browser: &str) -> String {
    let (name, version) = browser.split_once(' ').unwrap_or((browser, ""));
    let p = match name {
        "ie" | "ie_mob" => "ms",
        "edge" => {
            if matches!(version, "12" | "13" | "14" | "15" | "16" | "17" | "18") {
                "ms"
            } else {
                "webkit"
            }
        }
        "firefox" | "and_ff" | "kaios" => "moz",
        "opera" => {
            if matches!(version, "9" | "11" | "12" | "9.5-9.6" | "10.0-10.1" | "10.5" | "10.6" | "11.1" | "11.5" | "11.6" | "12.1") {
                "o"
            } else {
                "webkit"
            }
        }
        "op_mini" => "o",
        "op_mob" => {
            if version == "73" {
                "webkit"
            } else {
                "o"
            }
        }
        _ => "webkit",
    };
    format!("-{p}-")
}

/// `Prefixes#sort`
fn sort_prefixes(list: &mut Vec<String>) {
    js::sort_by(list, |a, b| {
        let al = remove_note(a).len() as i32;
        let bl = remove_note(b).len() as i32;
        if al == bl { b.len() as i32 - a.len() as i32 } else { bl - al }
    });
}

fn uniq(list: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in list {
        if !out.contains(&x) {
            out.push(x);
        }
    }
    out
}

fn decl_kind(name: &str) -> DeclKind {
    match name {
        "user-select" => DeclKind::UserSelect,
        "background-clip" => DeclKind::BackgroundClip,
        "text-decoration" => DeclKind::TextDecoration,
        "text-decoration-skip-ink" | "text-decoration-skip" => DeclKind::TextDecorationSkipInk,
        n if n.starts_with("mask-border") || n.starts_with("mask-box-image") => DeclKind::MaskBorder,
        "mask" | "mask-composite" => DeclKind::MaskComposite,
        "print-color-adjust" | "color-adjust" => DeclKind::PrintColorAdjust,
        _ => DeclKind::Generic,
    }
}

fn value_kind(name: &str) -> ValueKind {
    match name {
        "max-content" | "min-content" | "fit-content" | "fill" | "fill-available" | "stretch" => ValueKind::Intrinsic,
        "image-set" => ValueKind::ImageSet,
        "cross-fade" => ValueKind::CrossFade,
        _ => ValueKind::Generic,
    }
}

fn is_stretch(name: &str) -> bool {
    matches!(name, "stretch" | "fill" | "fill-available")
}

fn data() -> &'static Data {
    static CELL: OnceLock<Data> = OnceLock::new();
    CELL.get_or_init(|| {
        let json: serde_json::Value = serde_json::from_str(DATA_JSON).unwrap();
        let mut features = Vec::new();
        let mut all_prefixes: Vec<(String, Vec<String>)> = Vec::new();
        for (name, f) in json.as_object().unwrap() {
            let browsers: Vec<String> = f["browsers"].as_array().unwrap().iter().map(|b| b.as_str().unwrap().to_string()).collect();
            let selected: Vec<String> = browsers.iter().filter(|b| BROWSERS.contains(&b.as_str())).cloned().collect();
            let mut add = uniq(selected.iter().map(|b| browser_prefix(b)).collect());
            sort_prefixes(&mut add);
            let mut all: Vec<String> = browsers.iter().map(|b| browser_prefix(b)).collect();
            if let Some(m) = f.get("mistakes").and_then(|m| m.as_array()) {
                all.extend(m.iter().map(|x| x.as_str().unwrap().to_string()));
            }
            all_prefixes.push((name.clone(), uniq(all)));
            let props = f.get("props").and_then(|p| p.as_array()).map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect());
            features.push(Feature { name: name.clone(), add, props });
        }
        let mut decls: Vec<(String, DeclPrefixer)> = Vec::new();
        let mut values: Vec<(String, Vec<usize>)> = Vec::new();
        let mut value_prefixers = Vec::new();
        for f in &features {
            if f.add.is_empty() {
                continue;
            }
            match &f.props {
                Some(props) => {
                    let idx = value_prefixers.len();
                    value_prefixers.push(ValuePrefixer { name: f.name.clone(), prefixes: f.add.clone(), kind: value_kind(&f.name) });
                    for p in props {
                        match values.iter_mut().find(|(k, _)| k == p) {
                            Some(e) => e.1.push(idx),
                            None => values.push((p.clone(), vec![idx])),
                        }
                    }
                }
                None => {
                    let prefixes: Vec<String> = f.add.clone();
                    let d = DeclPrefixer { name: f.name.clone(), prefixes, kind: decl_kind(&f.name) };
                    match decls.iter_mut().find(|(k, _)| *k == f.name) {
                        Some(e) => e.1 = d,
                        None => decls.push((f.name.clone(), d)),
                    }
                }
            }
        }
        // the cleaner (empty browser selection): everything goes to `remove`
        let mut remove_props = Vec::new();
        let mut remove_values: Vec<(String, Vec<OldValue>)> = Vec::new();
        for (name, prefixes) in &all_prefixes {
            let f = features.iter().find(|f| &f.name == name).unwrap();
            match &f.props {
                Some(props) => {
                    for p in prefixes {
                        let old = old_value(name, p);
                        for prop in props {
                            match remove_values.iter_mut().find(|(k, _)| k == prop) {
                                Some(e) => e.1.push(old_clone(&old)),
                                None => remove_values.push((prop.clone(), vec![old_clone(&old)])),
                            }
                        }
                    }
                }
                None => {
                    for p in prefixes {
                        let prefixed = decl_prefixed(decl_kind(name), name, p);
                        if !remove_props.contains(&prefixed) {
                            remove_props.push(prefixed);
                        }
                    }
                }
            }
        }
        Data { decls, values, value_prefixers, remove_props, remove_values }
    })
}

fn old_clone(o: &OldValue) -> OldValue {
    OldValue { string: o.string.clone(), intrinsic: o.intrinsic }
}

fn old_value(name: &str, prefix: &str) -> OldValue {
    if value_kind(name) == ValueKind::Intrinsic {
        let mut prefixed = format!("{prefix}{name}");
        if is_stretch(name) {
            if prefix == "-moz-" {
                prefixed = "-moz-available".into();
            } else if prefix == "-webkit-" {
                prefixed = "-webkit-fill-available".into();
            }
        }
        return OldValue { string: prefixed, intrinsic: true };
    }
    OldValue { string: format!("{prefix}{name}"), intrinsic: false }
}

impl OldValue {
    fn check(&self, value: &str) -> bool {
        if value.contains(&self.string) {
            let trailing: &[char] = if self.intrinsic { &[')', ','] } else { &['(', ','] };
            return find_word(value, &self.string, trailing, 0).is_some();
        }
        false
    }
}

/// `prefixed(prop, prefix)` for a declaration prefixer kind
fn decl_prefixed(kind: DeclKind, prop: &str, prefix: &str) -> String {
    match kind {
        DeclKind::MaskBorder => {
            let r = format!("{prefix}{prop}");
            if prefix == "-webkit-" { r.replacen("border", "box-image", 1) } else { r }
        }
        DeclKind::PrintColorAdjust => {
            if prefix == "-moz-" {
                "color-adjust".into()
            } else {
                format!("{prefix}print-color-adjust")
            }
        }
        _ => format!("{prefix}{prop}"),
    }
}

/// `Declaration.load(prop).normalize(prop)` across all of autoprefixer's declaration hacks
fn normalize_prop(prop: &str) -> String {
    match prop {
        "align-content" | "flex-line-pack" => "align-content".into(),
        "align-self" | "flex-item-align" => "align-self".into(),
        "align-items" | "flex-align" | "box-align" => "align-items".into(),
        "flex-direction" | "box-direction" | "box-orient" => "flex-direction".into(),
        "flex-grow" | "flex-positive" => "flex".into(),
        "flex-basis" | "flex-preferred-size" => "flex-basis".into(),
        "flex" | "box-flex" => "flex".into(),
        "grid-column-align" => "justify-self".into(),
        "flex-shrink" | "flex-negative" => "flex-shrink".into(),
        "grid-row-align" => "align-self".into(),
        "justify-content" | "flex-pack" | "box-pack" => "justify-content".into(),
        "order" | "flex-order" | "box-ordinal-group" => "order".into(),
        "print-color-adjust" | "color-adjust" => "print-color-adjust".into(),
        "image-rendering" | "interpolation-mode" => "image-rendering".into(),
        "overscroll-behavior" | "scroll-chaining" => "overscroll-behavior".into(),
        "border-radius-topleft" => "border-top-left-radius".into(),
        "border-radius-topright" => "border-top-right-radius".into(),
        "border-radius-bottomleft" => "border-bottom-left-radius".into(),
        "border-radius-bottomright" => "border-bottom-right-radius".into(),
        "break-inside" | "page-break-inside" | "column-break-inside" => "break-inside".into(),
        "break-before" | "page-break-before" | "column-break-before" => "break-before".into(),
        "break-after" | "page-break-after" | "column-break-after" => "break-after".into(),
        "border-block-start" | "border-block-end" | "margin-block-start" | "margin-block-end" | "padding-block-start"
        | "padding-block-end" | "border-before" | "border-after" | "margin-before" | "margin-after" | "padding-before"
        | "padding-after" => {
            if prop.contains("-before") {
                prop.replacen("-before", "-block-start", 1)
            } else {
                prop.replacen("-after", "-block-end", 1)
            }
        }
        "border-inline-start" | "border-inline-end" | "margin-inline-start" | "margin-inline-end" | "padding-inline-start"
        | "padding-inline-end" | "border-start" | "border-end" | "margin-start" | "margin-end" | "padding-start" | "padding-end" => {
            // prop.replace(/(margin|padding|border)-(start|end)/, '$1-inline-$2')
            for a in ["margin", "padding", "border"] {
                for b in ["start", "end"] {
                    let pat = format!("{a}-{b}");
                    if let Some(i) = prop.find(&pat) {
                        return format!("{}{a}-inline-{b}{}", &prop[..i], &prop[i + pat.len()..]);
                    }
                }
            }
            prop.to_string()
        }
        "grid-template-rows" | "grid-template-columns" | "grid-rows" | "grid-columns" => {
            if let Some(r) = prop.strip_prefix("grid-rows") {
                format!("grid-template-rows{r}")
            } else if let Some(r) = prop.strip_prefix("grid-columns") {
                format!("grid-template-columns{r}")
            } else {
                prop.to_string()
            }
        }
        "grid-row-start" | "grid-column-start" => prop.replacen("-start", "", 1),
        p if p.starts_with("mask-border") || p.starts_with("mask-box-image") => {
            // MaskBorder#normalize uses the hack's own name, which is the prop itself
            p.replacen("box-image", "border", 1)
        }
        p => p.to_string(),
    }
}

/// `Prefixes#unprefixed(prop)`
fn unprefixed(prop: &str) -> String {
    let v = normalize_prop(vendor_unprefixed(prop));
    if v == "flex-direction" { "flex-flow".into() } else { v }
}

/// `Prefixes#prefixed(prop, prefix)`
fn prefixes_prefixed(prop: &str, prefix: &str) -> String {
    let p = vendor_unprefixed(prop);
    decl_prefixed(decl_kind(p), p, prefix)
}

fn add_decl(prop: &str) -> Option<&'static DeclPrefixer> {
    data().decls.iter().find(|(k, _)| k == prop).map(|(_, d)| d)
}

fn values_for(prop: &str) -> Vec<&'static ValuePrefixer> {
    let d = data();
    match d.values.iter().find(|(k, _)| k == prop) {
        Some((_, idxs)) => idxs.iter().map(|i| &d.value_prefixers[*i]).collect(),
        None => Vec::new(),
    }
}

/// Matches `(^|[\s,(])(word($|[trailing]))` (case-insensitive) starting the search at `from`;
/// returns (match start, group-1 end, group-2 end incl. trailing char, trailing char len).
fn find_word(s: &str, word: &str, trailing: &[char], from: usize) -> Option<(usize, usize, usize, usize)> {
    let lower_word = word.to_ascii_lowercase();
    let wl = word.len();
    let mut i = from;
    while i <= s.len() {
        // group 1 at i: either `^` (empty, only at 0) or one delimiter char
        let mut starts: Vec<usize> = Vec::new();
        if let Some(c) = s[i..].chars().next() {
            if js::is_js_whitespace(c) || c == ',' || c == '(' {
                starts.push(i + c.len_utf8());
            }
        }
        if i == 0 {
            starts.push(0);
        }
        for g1 in starts {
            if s.len() >= g1 + wl && s.is_char_boundary(g1 + wl) && s[g1..g1 + wl].to_ascii_lowercase() == lower_word {
                let after = g1 + wl;
                match s[after..].chars().next() {
                    None => return Some((i, g1, after, 0)),
                    Some(c) if js::is_js_whitespace(c) || trailing.contains(&c) => return Some((i, g1, after + c.len_utf8(), c.len_utf8())),
                    _ => {}
                }
            }
        }
        match s[i..].chars().next() {
            Some(c) => i += c.len_utf8(),
            None => break,
        }
    }
    None
}

/// `string.replace(regexp, '$1' + prefix + '$2')` (global)
fn replace_word(s: &str, word: &str, trailing: &[char], f: &dyn Fn(&str, &str) -> String) -> String {
    let mut out = String::new();
    let mut pos = 0;
    let mut search = 0;
    while let Some((start, g1, end, tl)) = find_word(s, word, trailing, search) {
        out.push_str(&s[pos..start]);
        let delim = &s[start..g1];
        let trail = &s[end - tl..end];
        out.push_str(&f(delim, trail));
        pos = end;
        search = end;
        if start == end {
            // zero-length match cannot happen (word is non-empty)
            break;
        }
    }
    out.push_str(&s[pos..]);
    out
}

// ---- processor

pub struct Ap<'a> {
    css: &'a mut Css,
}

/// `IGNORE_NEXT = /(!\s*)?autoprefixer:\s*ignore\s+next/i`
fn ignore_next(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    let mut i = 0;
    while let Some(p) = l[i..].find("autoprefixer:") {
        let rest = &l[i + p + "autoprefixer:".len()..];
        let rest = rest.trim_start_matches(js::is_js_whitespace);
        if let Some(r) = rest.strip_prefix("ignore") {
            let ws = r.len() - r.trim_start_matches(js::is_js_whitespace).len();
            if ws > 0 && r[ws..].starts_with("next") {
                return true;
            }
        }
        i += p + 1;
    }
    false
}

/// `/(!\s*)?autoprefixer:\s*(off|on)/i` → Some(on)
fn control_comment(text: &str) -> Option<bool> {
    let l = text.to_ascii_lowercase();
    let mut i = 0;
    while let Some(p) = l[i..].find("autoprefixer:") {
        let rest = &l[i + p + "autoprefixer:".len()..];
        let rest = rest.trim_start_matches(js::is_js_whitespace);
        if rest.starts_with("off") || rest.starts_with("on") {
            // status = /on/i.test(text)
            return Some(l.contains("on"));
        }
        i += p + 1;
    }
    None
}

impl<'a> Ap<'a> {
    /// `Processor#disabled(node)`
    fn disabled(&mut self, node: NodeId) -> bool {
        if let Some(v) = self.css[node].ap_disabled {
            return v;
        }
        if self.css[node].parent.is_some() {
            if let Some(p) = self.css.prev(node) {
                if self.css[p].kind == Kind::Comment && ignore_next(&self.css[p].text) {
                    self.css[node].ap_disabled = Some(true);
                    self.css[node].ap_self_disabled = true;
                    return true;
                }
            }
        }
        let mut value: Option<bool> = None;
        if self.css[node].nodes.is_some() {
            let mut status: Option<bool> = None;
            for c in self.css.children(node).to_vec() {
                if self.css[c].kind != Kind::Comment {
                    continue;
                }
                if let Some(on) = control_comment(&self.css[c].text) {
                    if status.is_none() {
                        status = Some(on);
                    }
                }
            }
            if let Some(s) = status {
                value = Some(!s);
            }
        }
        let value = if self.css[node].nodes.is_none() || value.is_none() {
            match self.css[node].parent {
                Some(p) => {
                    let parent_disabled = self.disabled(p);
                    if self.css[p].ap_self_disabled { false } else { parent_disabled }
                }
                None => false,
            }
        } else {
            value.unwrap()
        };
        self.css[node].ap_disabled = Some(value);
        value
    }

    /// `Prefixer#parentPrefix(node)`
    fn parent_prefix(&mut self, node: NodeId) -> Option<String> {
        if let Some(p) = &self.css[node].ap_prefix {
            return p.clone();
        }
        let n = &self.css[node];
        let mut prefix: Option<String> = if n.kind == Kind::Decl && n.prop.starts_with('-') {
            Some(vendor_prefix(&n.prop).to_string())
        } else if n.kind == Kind::Root {
            None
        } else if n.kind == Kind::Rule && n.selector.contains(":-") && rule_vendor_prefix(&n.selector).is_some() {
            rule_vendor_prefix(&n.selector)
        } else if n.kind == Kind::AtRule && n.name.starts_with('-') {
            Some(vendor_prefix(&n.name).to_string())
        } else {
            match n.parent {
                Some(p) => self.parent_prefix(p),
                // `this.parentPrefix(undefined)` would throw; detached nodes never get here
                None => None,
            }
        };
        if let Some(p) = &prefix {
            if !ALL_PREFIXES.contains(&p.as_str()) {
                prefix = None;
            }
        }
        self.css[node].ap_prefix = Some(prefix.clone());
        prefix
    }

    /// `decl.raw('before')`
    fn raw_before(&mut self, decl: NodeId) -> String {
        postcss::raw(self.css, decl, Some("before"), "before")
    }

    fn need_cascade(&mut self, decl: NodeId) -> bool {
        if !self.css[decl].ap_cascade {
            let v = self.raw_before(decl).contains('\n');
            self.css[decl].ap_cascade = v;
        }
        self.css[decl].ap_cascade
    }

    fn max_prefixed(&mut self, prefixes: &[String], decl: NodeId) -> usize {
        if let Some(m) = self.css[decl].ap_max {
            if m != 0 {
                return m;
            }
        }
        let max = prefixes.iter().map(|p| remove_note(p).len()).max().unwrap_or(0);
        self.css[decl].ap_max = Some(max);
        max
    }

    fn calc_before(&mut self, prefixes: &[String], decl: NodeId, prefix: &str) -> String {
        let max = self.max_prefixed(prefixes, decl) as isize;
        let diff = max - remove_note(prefix).len() as isize;
        let mut before = self.raw_before(decl);
        if diff > 0 {
            before.push_str(&" ".repeat(diff as usize));
        }
        before
    }

    /// `Prefixer.clone(node, overrides)`: postcss-like deep copy without the prefix/value caches
    fn ap_clone(&mut self, node: NodeId) -> NodeId {
        let c = self.css.clone_node(node);
        self.css[c].ap_prefix = None;
        self.css[c].ap_values = None;
        c
    }

    /// `Prefixes#group(decl)`: walk up (-1) or down (+1)
    fn group(&mut self, decl: NodeId, step: isize, f: &mut dyn FnMut(&mut Self, NodeId) -> bool) -> bool {
        let rule = self.css[decl].parent.unwrap();
        let nodes = self.css.children(rule).to_vec();
        let length = nodes.len() as isize;
        let mut index = self.css.index(rule, decl).map(|i| i as isize).unwrap_or(-1);
        let unpref = unprefixed(&self.css[decl].prop);
        index += step;
        while index >= 0 && index < length {
            let other = self.css.children(rule).get(index as usize).copied();
            let Some(other) = other else { break };
            if self.css[other].kind == Kind::Decl {
                let oprop = self.css[other].prop.clone();
                if step == -1 && oprop == unpref && !with_prefix(&self.css[other].value) {
                    break;
                }
                if unprefixed(&oprop) != unpref {
                    break;
                } else if f(self, other) {
                    return true;
                }
                if step == 1 && oprop == unpref && !with_prefix(&self.css[other].value) {
                    break;
                }
            }
            index += step;
        }
        false
    }

    // ---- Declaration

    fn decl_process(&mut self, p: &DeclPrefixer, decl: NodeId) {
        if !self.decl_check(p, decl) {
            return;
        }
        if !self.need_cascade(decl) {
            self.prefixer_process(p, decl);
            return;
        }
        let prefixes = self.prefixer_process(p, decl);
        if prefixes.is_empty() {
            return;
        }
        self.restore_before(decl);
        let b = self.calc_before(&prefixes, decl, "");
        self.css[decl].raws.before = Some(b);
    }

    fn decl_check(&self, p: &DeclPrefixer, decl: NodeId) -> bool {
        let v = &self.css[decl].value;
        match p.kind {
            DeclKind::BackgroundClip => v.to_lowercase() == "text",
            DeclKind::TextDecoration => {
                const BASIC: [&str; 8] = ["none", "underline", "overline", "line-through", "blink", "inherit", "initial", "unset"];
                split_ws(v).iter().any(|i| !BASIC.contains(&i.as_str()))
            }
            _ => true,
        }
    }

    /// `Prefixer#process` (shared part): returns the prefixes that were added
    fn prefixer_process(&mut self, p: &DeclPrefixer, decl: NodeId) -> Vec<String> {
        let parent = self.parent_prefix(decl);
        let prefixes: Vec<String> = p.prefixes.iter().filter(|pr| parent.as_ref().is_none_or(|pp| pp == remove_note(pr))).cloned().collect();
        let mut added: Vec<String> = Vec::new();
        for prefix in prefixes {
            let mut list = added.clone();
            list.push(prefix.clone());
            if self.decl_add(p, decl, &prefix, &list) {
                added.push(prefix);
            }
        }
        added
    }

    fn decl_add(&mut self, p: &DeclPrefixer, decl: NodeId, prefix: &str, prefixes: &[String]) -> bool {
        let prefixed = decl_prefixed(p.kind, &self.css[decl].prop.clone(), prefix);
        if self.is_already(decl, &prefixed) || other_prefixes(&self.css[decl].value, prefix) {
            return false;
        }
        self.decl_insert(p, decl, prefix, prefixes)
    }

    fn is_already(&mut self, decl: NodeId, prefixed: &str) -> bool {
        let pf = prefixed.to_string();
        let mut already = self.group(decl, -1, &mut |s, i| s.css[i].prop == pf);
        if !already {
            already = self.group(decl, 1, &mut |s, i| s.css[i].prop == pf);
        }
        already
    }

    fn decl_insert(&mut self, p: &DeclPrefixer, decl: NodeId, prefix: &str, prefixes: &[String]) -> bool {
        let value = self.css[decl].value.clone();
        match p.kind {
            DeclKind::UserSelect => {
                if (value == "all" && prefix == "-ms-") || (value == "contain" && (prefix == "-moz-" || prefix == "-webkit-")) {
                    return false;
                }
            }
            DeclKind::MaskComposite => return self.mask_composite_insert(p, decl, prefix, prefixes),
            _ => {}
        }
        let cloned = self.ap_clone(decl);
        self.decl_set(p, cloned, prefix);
        let parent = self.css[decl].parent.unwrap();
        let (cp, cv) = (self.css[cloned].prop.clone(), self.css[cloned].value.clone());
        let already = self.css.children(parent).iter().any(|i| self.css[*i].prop == cp && self.css[*i].value == cv);
        if already {
            return false;
        }
        if self.need_cascade(decl) {
            let b = self.calc_before(prefixes, decl, prefix);
            self.css[cloned].raws.before = Some(b);
        }
        self.css.insert_before(parent, decl, Add::Node(cloned));
        true
    }

    fn decl_set(&mut self, p: &DeclPrefixer, decl: NodeId, prefix: &str) {
        match p.kind {
            DeclKind::UserSelect if prefix == "-ms-" && self.css[decl].value == "contain" => {
                self.css[decl].value = "element".into();
                let prop = self.css[decl].prop.clone();
                self.css[decl].prop = format!("{prefix}{prop}");
            }
            DeclKind::TextDecorationSkipInk if self.css[decl].prop == "text-decoration-skip-ink" && self.css[decl].value == "auto" => {
                self.css[decl].prop = format!("{prefix}text-decoration-skip");
                self.css[decl].value = "ink".into();
            }
            kind => {
                let prop = self.css[decl].prop.clone();
                self.css[decl].prop = decl_prefixed(kind, &prop, prefix);
            }
        }
    }

    fn restore_before(&mut self, decl: NodeId) {
        let before = self.raw_before(decl);
        let mut lines: Vec<String> = before.split('\n').map(|s| s.to_string()).collect();
        let mut min = lines.last().unwrap().clone();
        let mut mins: Vec<String> = Vec::new();
        self.group(decl, -1, &mut |s, prefixed| {
            let b = s.raw_before(prefixed);
            let last = b.split('\n').last().unwrap_or("").to_string();
            mins.push(last);
            false
        });
        for last in mins {
            if last.len() < min.len() {
                min = last;
            }
        }
        *lines.last_mut().unwrap() = min;
        self.css[decl].raws.before = Some(lines.join("\n"));
    }

    fn mask_composite_insert(&mut self, _p: &DeclPrefixer, decl: NodeId, prefix: &str, prefixes: &[String]) -> bool {
        let is_composite = self.css[decl].prop == "mask-composite";
        let value = self.css[decl].value.clone();
        let composite_values: Vec<String> = if is_composite {
            value.split(',').map(|s| js::trim(s).to_string()).filter(|s| !s.is_empty()).collect()
        } else {
            mask_composite_matches(&value).into_iter().map(|s| js::trim(&s).to_string()).filter(|s| !s.is_empty()).collect()
        };
        let has = !composite_values.is_empty();
        let mut composite_decl = None;
        if has {
            let c = self.ap_clone(decl);
            let old = |v: &str| match v {
                "add" => "source-over".to_string(),
                "exclude" => "xor".to_string(),
                "intersect" => "source-in".to_string(),
                "subtract" => "source-out".to_string(),
                o => o.to_string(),
            };
            let mut v = composite_values.iter().map(|x| old(x)).collect::<Vec<_>>().join(", ");
            if composite_values.iter().any(|x| x == "intersect") {
                v.push_str(", xor");
            }
            self.css[c].value = v;
            self.css[c].prop = format!("{prefix}mask-composite");
            composite_decl = Some(c);
        }
        let parent = self.css[decl].parent.unwrap();
        if is_composite {
            let Some(cd) = composite_decl else { return false };
            if self.need_cascade(decl) {
                let b = self.calc_before(prefixes, decl, prefix);
                self.css[cd].raws.before = Some(b);
            }
            self.css.insert_before(parent, decl, Add::Node(cd));
            return true;
        }
        let cloned = self.ap_clone(decl);
        let prop = self.css[cloned].prop.clone();
        self.css[cloned].prop = format!("{prefix}{prop}");
        if has {
            let v = self.css[cloned].value.clone();
            self.css[cloned].value = mask_composite_strip(&v);
        }
        if self.need_cascade(decl) {
            let b = self.calc_before(prefixes, decl, prefix);
            self.css[cloned].raws.before = Some(b);
        }
        self.css.insert_before(parent, decl, Add::Node(cloned));
        let Some(cd) = composite_decl else { return true };
        if self.need_cascade(decl) {
            let b = self.calc_before(prefixes, decl, prefix);
            self.css[cd].raws.before = Some(b);
        }
        self.css.insert_before(parent, decl, Add::Node(cd));
        true
    }

    // ---- Value

    fn value_check(&self, v: &ValuePrefixer, decl: NodeId) -> bool {
        let value = &self.css[decl].value;
        if !value.contains(&v.name) {
            return false;
        }
        find_word(value, &v.name, trailing_for(v.kind), 0).is_some()
    }

    fn value_process(&mut self, v: &ValuePrefixer, decl: NodeId) {
        if !self.value_check(v, decl) {
            return;
        }
        let parent = self.parent_prefix(decl);
        let prefixes: Vec<String> = v.prefixes.iter().filter(|pr| parent.as_ref().is_none_or(|pp| pp == remove_note(pr))).cloned().collect();
        for prefix in prefixes {
            self.value_add(v, decl, &prefix);
        }
    }

    fn value_add(&mut self, v: &ValuePrefixer, decl: NodeId, prefix: &str) {
        if v.kind == ValueKind::Intrinsic && self.css[decl].prop.contains("grid") && prefix != "-webkit-" {
            return;
        }
        let existing = self.css[decl].ap_values.as_ref().and_then(|l| l.iter().find(|(k, _)| k == prefix).map(|(_, x)| x.clone()));
        let mut value = existing.filter(|x| !x.is_empty()).unwrap_or_else(|| {
            let n = &self.css[decl];
            match &n.raws.value {
                Some(r) if r.value == n.value => r.raw.clone(),
                _ => n.value.clone(),
            }
        });
        loop {
            let before = value.clone();
            value = value_replace(v, &value, prefix);
            if value == before {
                break;
            }
        }
        let list = self.css[decl].ap_values.get_or_insert_with(Vec::new);
        match list.iter_mut().find(|(k, _)| k == prefix) {
            Some(e) => e.1 = value,
            None => list.push((prefix.to_string(), value)),
        }
    }

    /// `Value.save(prefixes, decl)`
    fn value_save(&mut self, decl: NodeId) {
        let Some(values) = self.css[decl].ap_values.clone() else { return };
        for (prefix, value) in values {
            if value == self.css[decl].value {
                continue;
            }
            let prop = self.css[decl].prop.clone();
            let prop_prefix = vendor_prefix(&prop).to_string();
            if prop_prefix == "-pie-" {
                continue;
            }
            if prop_prefix == prefix {
                self.css[decl].value = value;
                continue;
            }
            let prefixed = prefixes_prefixed(&prop, &prefix);
            let rule = self.css[decl].parent.unwrap();
            if self.css.children(rule).iter().any(|i| self.css[*i].prop == prefixed) {
                continue;
            }
            let trimmed = replace_first_ws(&value);
            let already = self.css.children(rule).iter().any(|i| self.css[*i].prop == prop && replace_first_ws(&self.css[*i].value) == trimmed);
            if already {
                continue;
            }
            let cloned = self.ap_clone(decl);
            self.css[cloned].value = value;
            self.css.insert_before(rule, decl, Add::Node(cloned));
        }
    }

    // ---- transition

    fn rule_vendor_prefixes(&self, decl: NodeId) -> Option<Vec<String>> {
        let parent = self.css[decl].parent?;
        if self.css[parent].kind != Kind::Rule {
            return None;
        }
        let sel = &self.css[parent].selector;
        if !sel.contains(":-") {
            return None;
        }
        let list: Vec<String> = ALL_PREFIXES.iter().filter(|p| sel.contains(&format!(":{p}"))).map(|s| s.to_string()).collect();
        if list.is_empty() { None } else { Some(list) }
    }

    fn transition_add(&mut self, decl: NodeId) {
        let prop = self.css[decl].prop.clone();
        let add = add_decl(&prop);
        let vendor_prefixes = self.rule_vendor_prefixes(decl);
        let decl_prefixes: Vec<String> = vendor_prefixes.clone().or_else(|| add.map(|a| a.prefixes.clone())).unwrap_or_default();
        let value = self.css[decl].value.clone();
        let mut params = transition_parse(&value);
        let names: Vec<String> = params.iter().map(|p| find_prop(p)).collect();
        if names.iter().any(|n| n.starts_with('-')) {
            return;
        }
        let mut added: Vec<Vec<VNode>> = Vec::new();
        for param in &params {
            let p = find_prop(param);
            if p.starts_with('-') {
                continue;
            }
            let Some(prefixer) = add_decl(&p) else { continue };
            for prefix in &prefixer.prefixes {
                if let Some(vp) = &vendor_prefixes {
                    if !vp.iter().any(|x| prefix.contains(x.as_str())) {
                        continue;
                    }
                }
                let prefixed = prefixes_prefixed(&p, prefix);
                if prefixed != "-ms-transform" && !names.contains(&prefixed) {
                    added.push(clone_param(&p, &prefixed, param));
                }
            }
        }
        params.extend(added);
        let value_str = transition_stringify(&mut params.clone());
        let webkit_clean = transition_stringify(&mut clean_from_unprefixed(&params, "-webkit-"));
        if decl_prefixes.iter().any(|p| p == "-webkit-") {
            self.clone_before(decl, &format!("-webkit-{prop}"), &webkit_clean);
        }
        self.clone_before(decl, &prop, &webkit_clean);
        if decl_prefixes.iter().any(|p| p == "-o-") {
            let opera_clean = transition_stringify(&mut clean_from_unprefixed(&params, "-o-"));
            self.clone_before(decl, &format!("-o-{prop}"), &opera_clean);
        }
        for prefix in &decl_prefixes {
            if prefix != "-webkit-" && prefix != "-o-" {
                let pv = transition_stringify(&mut clean_other_prefixes(&params, prefix));
                self.clone_before(decl, &format!("{prefix}{prop}"), &pv);
            }
        }
        if value_str != self.css[decl].value && !self.already(decl, &prop, &value_str) {
            let c = self.css.clone_node(decl);
            let parent = self.css[decl].parent.unwrap();
            self.css.insert_before(parent, decl, Add::Node(c));
            self.css[decl].value = value_str;
        }
    }

    fn already(&self, decl: NodeId, prop: &str, value: &str) -> bool {
        let parent = self.css[decl].parent.unwrap();
        self.css.children(parent).iter().any(|i| self.css[*i].prop == prop && self.css[*i].value == value)
    }

    /// `decl.cloneBefore({ prop, value })` unless it already exists
    fn clone_before(&mut self, decl: NodeId, prop: &str, value: &str) {
        if !self.already(decl, prop, value) {
            let c = self.css.clone_node(decl);
            self.css[c].prop = prop.to_string();
            self.css[c].value = value.to_string();
            let parent = self.css[decl].parent.unwrap();
            self.css.insert_before(parent, decl, Add::Node(c));
        }
    }

    // ---- @supports

    fn supports_process(&mut self, rule: NodeId) {
        let params = self.css[rule].params.clone();
        let ast = brackets_parse(&params);
        let ast = supports_normalize(ast);
        let ast = supports_remove(ast, &params);
        let ast = self.supports_add(ast);
        let ast = clean_brackets(ast);
        self.css[rule].params = brackets_stringify(&ast);
    }

    fn supports_add(&mut self, nodes: Vec<B>) -> Vec<B> {
        nodes
            .into_iter()
            .map(|i| {
                if is_prop(&i) {
                    let B::List(l) = &i else { unreachable!() };
                    let B::Str(s) = &l[0] else { unreachable!() };
                    let prefixed = self.supports_prefixed(s);
                    if prefixed.len() > 1 {
                        return B::List(supports_convert(&prefixed));
                    }
                    return i;
                }
                match i {
                    B::List(l) => B::List(self.supports_add(l)),
                    s => s,
                }
            })
            .collect()
    }

    /// `Supports#prefixed(str)`: decls of a virtual rule after prefixing → (prop, value)
    fn supports_prefixed(&mut self, s: &str) -> Vec<(String, String)> {
        let (prop, value) = supports_parse(s);
        // virtual rule `a{}` from a separate parse
        let root = self.css.root();
        let rule = self.css.rule("a");
        self.css[rule].raws.between = Some(String::new());
        self.css[rule].raws.after = Some(String::new());
        self.css.push(root, rule);
        let d = self.css.decl(&prop, &value);
        self.css[d].raws.before = Some(String::new());
        self.css.append(rule, Add::Node(d));
        let first = d;
        let disabled = {
            let n = &self.css[first];
            (n.prop == "display" && n.value.contains("grid")) || n.prop.contains("grid") || n.prop == "justify-items"
        };
        if !disabled {
            if let Some(p) = add_decl(&prop) {
                self.decl_process(p, first);
            }
            for decl in self.css.children(rule).to_vec() {
                for v in values_for(&prop) {
                    self.value_process(v, decl);
                }
                self.value_save(decl);
            }
        }
        self.css.children(rule).iter().map(|n| (self.css[*n].prop.clone(), self.css[*n].value.clone())).collect()
    }

    // ---- the add() pass

    fn add(&mut self, root: NodeId) {
        // at-rules: only @supports can do something with this data
        let mut supports = Vec::new();
        self.css.walk_at_rules(root, &mut |css, r| {
            if css[r].name == "supports" {
                supports.push(r);
            }
            Flow::Continue
        });
        for r in supports {
            if !self.disabled(r) {
                self.supports_process(r);
            }
        }
        // selectors: no selector prefixes in the data, but `disabled()` caches flags
        let mut rules = Vec::new();
        self.css.walk_rules(root, &mut |_, r| {
            rules.push(r);
            Flow::Continue
        });
        for r in rules {
            self.disabled(r);
        }
        // declarations
        walk_decls_live(self.css, root, &mut |ap, decl| {
            if ap.disabled(decl) {
                return;
            }
            let prop = ap.css[decl].prop.clone();
            if prop == "grid-row-span" || prop == "grid-column-span" || (prop == "display" && ap.css[decl].value == "box") {
                return;
            }
            if prop == "transition" || prop == "transition-property" {
                ap.transition_add(decl);
            } else if prop == "align-self" || prop == "justify-self" || prop == "place-self" {
                // no flexbox/grid prefixes in the data
            } else if let Some(p) = add_decl(&prop) {
                ap.decl_process(p, decl);
            }
        });
        // values
        walk_decls_live(self.css, root, &mut |ap, decl| {
            if ap.css[decl].prop == "content" {
                return;
            }
            if ap.disabled(decl) {
                return;
            }
            let unpref = unprefixed(&ap.css[decl].prop);
            for v in values_for(&unpref) {
                ap.value_process(v, decl);
            }
            ap.value_save(decl);
        });
    }

}

/// `css.walkDecls(cb)` with postcss's live iteration semantics
fn walk_decls_live(css: &mut Css, root: NodeId, f: &mut dyn FnMut(&mut Ap, NodeId)) {
    css.walk(root, &mut |c, n| {
        if c[n].kind == Kind::Decl {
            let mut ap = Ap { css: c };
            f(&mut ap, n);
        }
        Flow::Continue
    });
}

fn rule_vendor_prefix(sel: &str) -> Option<String> {
    // selector.match(/:(-\w+-)/)[1]
    let b = sel.as_bytes();
    for i in 0..b.len() {
        if b[i] == b':' && b.get(i + 1) == Some(&b'-') {
            let mut j = i + 2;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            // backtrack to the last '-' within the \w+ run's end
            let mut k = j;
            while k > i + 2 {
                if b.get(k) == Some(&b'-') {
                    return Some(sel[i + 1..=k].to_string());
                }
                k -= 1;
            }
        }
    }
    None
}

fn trailing_for(kind: ValueKind) -> &'static [char] {
    match kind {
        ValueKind::Intrinsic => &[')', ','],
        _ => &['(', ','],
    }
}

fn value_replace(v: &ValuePrefixer, s: &str, prefix: &str) -> String {
    match v.kind {
        ValueKind::Intrinsic => {
            let t: &[char] = &[')', ','];
            if prefix == "-moz-" && is_stretch(&v.name) {
                return replace_word(s, &v.name, t, &|d, tr| format!("{d}-moz-available{tr}"));
            }
            if prefix == "-webkit-" && is_stretch(&v.name) {
                return replace_word(s, &v.name, t, &|d, tr| format!("{d}-webkit-fill-available{tr}"));
            }
            replace_keep(s, &v.name, t, prefix)
        }
        ValueKind::CrossFade => crate::postcss::space(s)
            .into_iter()
            .map(|value| {
                let name = &v.name;
                let head: String = value.chars().take(name.chars().count() + 1).collect();
                if head != format!("{name}(") {
                    return value;
                }
                let close = value.rfind(')');
                let after = match close {
                    Some(c) => value[c + 1..].to_string(),
                    None => value.clone(),
                };
                let start = name.len() + 1;
                let mut args = match close {
                    Some(c) if c >= start => value[start..c].to_string(),
                    Some(_) => String::new(),
                    None => {
                        let end = value.len().saturating_sub(value.chars().last().map(|c| c.len_utf8()).unwrap_or(0));
                        if end > start { value[start..end].to_string() } else { String::new() }
                    }
                };
                if prefix == "-webkit-" {
                    if let Some(m) = cross_fade_number(&args) {
                        args = js::trim(&args[m.len().min(args.len())..]).to_string();
                        args.push_str(&format!(", {m}"));
                    } else {
                        args.push_str(", 0.5");
                    }
                }
                format!("{prefix}{name}({args}){after}")
            })
            .collect::<Vec<_>>()
            .join(" "),
        ValueKind::ImageSet => {
            let fixed = replace_keep(s, &v.name, &['(', ','], prefix);
            if prefix == "-webkit-" { image_set_fix(&fixed) } else { fixed }
        }
        ValueKind::Generic => replace_keep(s, &v.name, &['(', ','], prefix),
    }
}

/// `string.replace(regexp, '$1' + prefix + '$2')` — keeps the matched text (and its case)
fn replace_keep(s: &str, name: &str, trailing: &[char], prefix: &str) -> String {
    let mut out = String::new();
    let mut pos = 0;
    let mut search = 0;
    while let Some((_, g1, end, _)) = find_word(s, name, trailing, search) {
        out.push_str(&s[pos..g1]);
        out.push_str(prefix);
        out.push_str(&s[g1..end]);
        pos = end;
        search = end;
    }
    out.push_str(&s[pos..]);
    out
}

/// `args.match(/\d*.?\d+%?/)`
fn cross_fade_number(args: &str) -> Option<String> {
    let c: Vec<char> = args.chars().collect();
    let is_line = |ch: char| matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    for start in 0..c.len() {
        // \d* greedy, .? greedy, \d+ greedy, %?
        let mut d_end = start;
        while d_end < c.len() && c[d_end].is_ascii_digit() {
            d_end += 1;
        }
        for a in (start..=d_end).rev() {
            for dot in [true, false] {
                let mut j = a;
                if dot {
                    if j < c.len() && !is_line(c[j]) {
                        j += 1;
                    } else {
                        continue;
                    }
                }
                let ds = j;
                let mut k = j;
                while k < c.len() && c[k].is_ascii_digit() {
                    k += 1;
                }
                if k > ds {
                    let mut e = k;
                    if e < c.len() && c[e] == '%' {
                        e += 1;
                    }
                    return Some(c[start..e].iter().collect());
                }
            }
        }
    }
    None
}

/// `fixed.replace(/("[^"]+"|'[^']+')(\s+\d+\w)/gi, 'url($1)$2')`
fn image_set_fix(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '"' || c[i] == '\'' {
            let q = c[i];
            if let Some(close) = c[i + 1..].iter().position(|x| *x == q).map(|p| p + i + 1) {
                if close > i + 1 {
                    let mut j = close + 1;
                    let ws_start = j;
                    while j < c.len() && js::is_js_whitespace(c[j]) {
                        j += 1;
                    }
                    if j > ws_start {
                        let ds = j;
                        while j < c.len() && c[j].is_ascii_digit() {
                            j += 1;
                        }
                        // \d+\w: at least one digit then a word char (backtracking allowed)
                        let mut ok_end = None;
                        if j > ds {
                            if j < c.len() && (c[j].is_ascii_alphanumeric() || c[j] == '_') {
                                ok_end = Some(j + 1);
                            } else if j - ds >= 2 {
                                ok_end = Some(j);
                            }
                        }
                        if let Some(e) = ok_end {
                            let quoted: String = c[i..=close].iter().collect();
                            let rest: String = c[close + 1..e].iter().collect();
                            out.push_str(&format!("url({quoted}){rest}"));
                            i = e;
                            continue;
                        }
                    }
                }
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn split_ws(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_ws = false;
    for ch in s.chars() {
        if js::is_js_whitespace(ch) {
            if !in_ws {
                out.push(std::mem::take(&mut cur));
            }
            in_ws = true;
        } else {
            in_ws = false;
            cur.push(ch);
        }
    }
    out.push(cur);
    out
}

/// `value.replace(/\s+/, ' ')` (first run only)
fn replace_first_ws(s: &str) -> String {
    let mut out = String::new();
    let mut done = false;
    let mut in_ws = false;
    for ch in s.chars() {
        if !done && js::is_js_whitespace(ch) {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
            continue;
        }
        if in_ws {
            done = true;
            in_ws = false;
        }
        out.push(ch);
    }
    out
}

/// `Declaration#otherPrefixes(value, prefix)`
fn other_prefixes(value: &str, prefix: &str) -> bool {
    for other in ALL_PREFIXES {
        if other == prefix {
            continue;
        }
        if value.contains(other) {
            // value.replace(/var\([^)]+\)/, '')
            let stripped = match value.find("var(") {
                Some(i) => {
                    let rest = &value[i + 4..];
                    match rest.find(')') {
                        Some(j) if j > 0 => format!("{}{}", &value[..i], &rest[j + 1..]),
                        _ => value.to_string(),
                    }
                }
                None => value.to_string(),
            };
            return stripped.contains(other);
        }
    }
    false
}

/// `MaskComposite.regexp` matches: /\s+(add|exclude|intersect|subtract)\b(?!\))\s*(?=[,])/ig
fn mask_composite_scan(value: &str) -> Vec<(usize, usize)> {
    let c: Vec<(usize, char)> = value.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if js::is_js_whitespace(c[i].1) {
            let mut j = i;
            while j < c.len() && js::is_js_whitespace(c[j].1) {
                j += 1;
            }
            let start_word = c.get(j).map(|x| x.0).unwrap_or(value.len());
            let rest = &value[start_word..];
            let lower = rest.to_ascii_lowercase();
            let mut matched = None;
            for w in ["add", "exclude", "intersect", "subtract"] {
                if lower.starts_with(w) {
                    let after = start_word + w.len();
                    let next = value[after..].chars().next();
                    let boundary = next.is_none_or(|ch| !(ch.is_ascii_alphanumeric() || ch == '_'));
                    if boundary && next != Some(')') {
                        let ws_end = after + value[after..].len() - value[after..].trim_start_matches(js::is_js_whitespace).len();
                        if value[ws_end..].starts_with(',') {
                            matched = Some((c[i].0, ws_end));
                        }
                    }
                }
            }
            if let Some(m) = matched {
                out.push(m);
                while i < c.len() && c[i].0 < m.1 {
                    i += 1;
                }
                continue;
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    out
}

fn mask_composite_matches(value: &str) -> Vec<String> {
    mask_composite_scan(value).into_iter().map(|(a, b)| value[a..b].to_string()).collect()
}

fn mask_composite_strip(value: &str) -> String {
    let mut out = String::new();
    let mut pos = 0;
    for (a, b) in mask_composite_scan(value) {
        out.push_str(&value[pos..a]);
        pos = b;
    }
    out.push_str(&value[pos..]);
    out
}

// ---- transition helpers (postcss-value-parser nodes)

fn transition_parse(value: &str) -> Vec<Vec<VNode>> {
    let ast = value_parser::parse_original(value);
    let mut result = Vec::new();
    let mut param = Vec::new();
    for node in ast {
        let is_comma = matches!(&node, VNode::Div { value, .. } if value == ",");
        param.push(node);
        if is_comma {
            result.push(std::mem::take(&mut param));
        }
    }
    result.push(param);
    result.into_iter().filter(|p| !p.is_empty()).collect()
}

fn node_value(n: &VNode) -> String {
    match n {
        VNode::Word(v) | VNode::Space(v) | VNode::UnicodeRange(v) => v.clone(),
        VNode::Str { value, .. } | VNode::Comment { value, .. } | VNode::Div { value, .. } | VNode::Function { value, .. } => value.clone(),
    }
}

fn find_prop(param: &[VNode]) -> String {
    let prop = node_value(&param[0]);
    if prop.starts_with(|c: char| c.is_ascii_digit()) {
        for (i, t) in param.iter().enumerate() {
            if i != 0 {
                if let VNode::Word(v) = t {
                    return v.clone();
                }
            }
        }
    }
    prop
}

fn clone_param(origin: &str, name: &str, param: &[VNode]) -> Vec<VNode> {
    let mut result = Vec::new();
    let mut changed = false;
    for i in param {
        if !changed && matches!(i, VNode::Word(v) if v == origin) {
            result.push(VNode::Word(name.to_string()));
            changed = true;
        } else {
            result.push(i.clone());
        }
    }
    result
}

fn clean_from_unprefixed(params: &[Vec<VNode>], prefix: &str) -> Vec<Vec<VNode>> {
    let remove: Vec<String> =
        params.iter().map(|i| find_prop(i)).filter(|i| i.starts_with(prefix)).map(|i| unprefixed(&i)).collect();
    let mut result = Vec::new();
    for p in params {
        let prop = find_prop(p);
        let pfx = vendor_prefix(&prop);
        if !remove.contains(&prop) && (pfx == prefix || pfx.is_empty()) {
            result.push(p.clone());
        }
    }
    result
}

fn clean_other_prefixes(params: &[Vec<VNode>], prefix: &str) -> Vec<Vec<VNode>> {
    params
        .iter()
        .filter(|p| {
            let prop = find_prop(p);
            let cur = vendor_prefix(&prop);
            cur.is_empty() || cur == prefix
        })
        .cloned()
        .collect()
}

/// `Transition#stringify(params)` (mutates params like the JS: appends separators)
fn transition_stringify(params: &mut Vec<Vec<VNode>>) -> String {
    if params.is_empty() {
        return String::new();
    }
    // div(params): the first ',' div found, or a default `{ after: ' ', value: ',' }`
    let div = params
        .iter()
        .flat_map(|p| p.iter())
        .find(|n| matches!(n, VNode::Div { value, .. } if value == ","))
        .cloned()
        .unwrap_or(VNode::Div { value: ",".into(), before: String::new(), after: " ".into() });
    let mut nodes: Vec<VNode> = Vec::new();
    for p in params.iter_mut() {
        if !matches!(p.last(), Some(VNode::Div { .. })) {
            p.push(div.clone());
        }
        nodes.extend(p.iter().cloned());
    }
    if matches!(nodes.first(), Some(VNode::Div { .. })) {
        nodes.remove(0);
    }
    if matches!(nodes.last(), Some(VNode::Div { .. })) {
        nodes.pop();
    }
    value_parser::stringify(&nodes)
}

// ---- @supports helpers (`brackets` + `Supports`)

#[derive(Clone, Debug, PartialEq)]
enum B {
    Str(String),
    List(Vec<B>),
}

fn brackets_parse(s: &str) -> Vec<B> {
    // a stack of lists; each list starts with an empty string
    let mut stack: Vec<Vec<B>> = vec![vec![B::Str(String::new())]];
    for ch in s.chars() {
        if ch == '(' {
            stack.push(vec![B::Str(String::new())]);
            continue;
        }
        if ch == ')' {
            if stack.len() > 1 {
                let done = stack.pop().unwrap();
                let parent = stack.last_mut().unwrap();
                parent.push(B::List(done));
                parent.push(B::Str(String::new()));
            } else {
                // `stack.pop()` empties the stack: `last(stack)` is undefined and the next
                // push throws in JS; such params never reach here in practice
                stack.last_mut().unwrap().push(B::Str(String::new()));
            }
            continue;
        }
        let cur = stack.last_mut().unwrap();
        if let Some(B::Str(last)) = cur.last_mut() {
            last.push(ch);
        }
    }
    // unclosed `(`: the list was pushed into its parent when it opened
    while stack.len() > 1 {
        let done = stack.pop().unwrap();
        stack.last_mut().unwrap().push(B::List(done));
    }
    stack.pop().unwrap()
}

fn brackets_stringify(ast: &[B]) -> String {
    let mut out = String::new();
    for i in ast {
        match i {
            B::List(l) => out.push_str(&format!("({})", brackets_stringify(l))),
            B::Str(s) => out.push_str(s),
        }
    }
    out
}

fn supports_normalize(nodes: Vec<B>) -> Vec<B> {
    let nodes: Vec<B> = nodes.into_iter().filter(|i| *i != B::Str(String::new())).collect();
    if let Some(B::Str(first)) = nodes.first() {
        let f = js::trim(first);
        if f.contains(':') || f == "selector" || f == "not selector" {
            return vec![B::Str(brackets_stringify(&nodes))];
        }
    }
    nodes
        .into_iter()
        .map(|i| match i {
            B::List(l) => B::List(supports_normalize(l)),
            s => s,
        })
        .collect()
}

fn is_prop(node: &B) -> bool {
    matches!(node, B::List(l) if l.len() == 1 && matches!(l[0], B::Str(_)))
}

fn is_not(node: Option<&B>) -> bool {
    // /not\s*/i
    matches!(node, Some(B::Str(s)) if s.to_ascii_lowercase().contains("not"))
}

fn is_or(node: Option<&B>) -> bool {
    matches!(node, Some(B::Str(s)) if s.to_ascii_lowercase().contains("or"))
}

fn supports_parse(s: &str) -> (String, String) {
    let parts: Vec<&str> = s.split(':').collect();
    let prop = parts[0];
    let value = parts.get(1).copied().unwrap_or("");
    (js::trim(prop).to_string(), js::trim(value).to_string())
}

fn is_hack(all: &str, unprefixed: &str) -> bool {
    // !new RegExp(`(\\(|\\s)${escapeRegexp(unprefixed)}:`).test(all)
    let needle = format!("{unprefixed}:");
    let mut i = 0;
    while let Some(p) = all[i..].find(&needle) {
        let at = i + p;
        if at > 0 {
            let prev = all[..at].chars().last().unwrap();
            if prev == '(' || js::is_js_whitespace(prev) {
                return false;
            }
        }
        i = at + 1;
    }
    true
}

fn to_remove(s: &str, all: &str) -> bool {
    let (prop, value) = supports_parse(s);
    let unpref = unprefixed(&prop);
    let d = data();
    if d.remove_props.contains(&prop) && !is_hack(all, &unpref) {
        return true;
    }
    if let Some((_, olds)) = d.remove_values.iter().find(|(k, _)| *k == unpref) {
        for o in olds {
            if o.check(&value) {
                return true;
            }
        }
    }
    false
}

fn supports_remove(mut nodes: Vec<B>, all: &str) -> Vec<B> {
    let mut i: isize = 0;
    while (i as usize) < nodes.len() {
        let iu = i as usize;
        let prev = if i > 0 { nodes.get(iu - 1) } else { None };
        if !is_not(prev) && is_prop(&nodes[iu]) && is_or(nodes.get(iu + 1)) {
            let B::List(l) = &nodes[iu] else { unreachable!() };
            let B::Str(s) = &l[0] else { unreachable!() };
            if to_remove(s, all) {
                nodes.drain(iu..(iu + 2).min(nodes.len()));
                continue;
            }
            i += 2;
            continue;
        }
        if let B::List(l) = &nodes[iu] {
            let l = l.clone();
            nodes[iu] = B::List(supports_remove(l, all));
        }
        i += 1;
    }
    nodes
}

fn supports_convert(progress: &[(String, String)]) -> Vec<B> {
    let mut result = vec![B::Str(String::new())];
    for (p, v) in progress {
        result.push(B::List(vec![B::Str(format!("{p}: {v}"))]));
        result.push(B::Str(" or ".into()));
    }
    let n = result.len();
    result[n - 1] = B::Str(String::new());
    result
}

fn clean_brackets(nodes: Vec<B>) -> Vec<B> {
    nodes
        .into_iter()
        .map(|i| match i {
            B::List(l) => {
                if l.len() == 1 && matches!(l[0], B::List(_)) {
                    let B::List(inner) = l.into_iter().next().unwrap() else { unreachable!() };
                    B::List(clean_brackets(inner))
                } else {
                    B::List(clean_brackets(l))
                }
            }
            s => s,
        })
        .collect()
}

/// Runs autoprefixer's `add` pass over `root`.
pub fn autoprefix(css: &mut Css, root: NodeId) -> Result<(), CssError> {
    let mut ap = Ap { css };
    ap.add(root);
    Ok(())
}

pub fn _unused(_: Value) {}
