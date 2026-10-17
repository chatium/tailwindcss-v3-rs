//! Port of `postcss-selector-parser@6.1.2` (the version inside the Play bundle).
//!
//! Nodes live in an arena ([`Sel`]). As in JS, container mutation does not detach a node
//! from its previous parent and assigning `nodes` does not update `parent`, so the same
//! node can be referenced from several containers; tailwind relies on that aliasing.

mod cssesc;
mod parser;
mod tokenize;

pub use cssesc::{cssesc, Cssesc};
pub use parser::parse;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SKind {
    Root,
    Selector,
    Class,
    Tag,
    Id,
    Attribute,
    Pseudo,
    Combinator,
    Comment,
    Str,
    Nesting,
    Universal,
}

/// `node.namespace`: `true` or a string
#[derive(Debug, Clone, PartialEq)]
pub enum Ns {
    True,
    Str(String),
}

/// `spaces` / `raws.spaces` entry for attribute parts
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartSpaces {
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SRaws {
    pub value: Option<String>,
    pub spaces_before: Option<String>,
    pub spaces_after: Option<String>,
    pub namespace: Option<String>,
    pub attribute: Option<String>,
    pub operator: Option<String>,
    pub insensitive_flag: Option<String>,
    /// raws.spaces.{attribute,operator,value,insensitive,namespace}
    pub part_spaces: Vec<(&'static str, PartSpaces)>,
}

#[derive(Debug, Clone)]
pub struct SNode {
    pub kind: SKind,
    pub parent: Option<SId>,
    pub nodes: Option<Vec<SId>>,
    /// `value` (`_value` for classes and attributes); `None` = undefined
    pub value: Option<String>,
    pub spaces_before: String,
    pub spaces_after: String,
    pub raws: Option<SRaws>,
    pub namespace: Option<Ns>,
    // attribute
    pub attribute: Option<String>,
    pub operator: Option<String>,
    /// `_quoteMark`: None = undefined, Some(None) = null, Some(Some(q))
    pub quote_mark: Option<Option<char>>,
    pub insensitive: Option<bool>,
    pub part_spaces: Vec<(&'static str, PartSpaces)>,
    pub constructed: bool,
    pub trailing_comma: bool,
    indexes: Vec<(u32, isize)>,
    last_each: u32,
}

impl SNode {
    pub fn new(kind: SKind) -> SNode {
        SNode {
            kind,
            parent: None,
            nodes: if matches!(kind, SKind::Root | SKind::Selector | SKind::Pseudo) { Some(Vec::new()) } else { None },
            value: match kind {
                SKind::Nesting => Some("&".into()),
                SKind::Universal => Some("*".into()),
                _ => None,
            },
            spaces_before: String::new(),
            spaces_after: String::new(),
            raws: None,
            namespace: None,
            attribute: None,
            operator: None,
            quote_mark: None,
            insensitive: None,
            part_spaces: Vec::new(),
            constructed: matches!(kind, SKind::Class),
            trailing_comma: false,
            indexes: Vec::new(),
            last_each: 0,
        }
    }

    fn raws_mut(&mut self) -> &mut SRaws {
        self.raws.get_or_insert_with(Default::default)
    }

    pub fn val(&self) -> &str {
        self.value.as_deref().unwrap_or("")
    }

    pub fn is_container(&self) -> bool {
        self.nodes.is_some()
    }
}

/// Walk control
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SFlow {
    Continue,
    Stop,
}

#[derive(Default)]
pub struct Sel {
    pub n: Vec<SNode>,
}

impl std::ops::Index<SId> for Sel {
    type Output = SNode;
    fn index(&self, id: SId) -> &SNode {
        &self.n[id.0 as usize]
    }
}
impl std::ops::IndexMut<SId> for Sel {
    fn index_mut(&mut self, id: SId) -> &mut SNode {
        &mut self.n[id.0 as usize]
    }
}

/// A plain `Error` thrown by the selector parser (`new Error(message)`).
#[derive(Debug, Clone, PartialEq)]
pub struct SelError(pub String);

impl Sel {
    pub fn new() -> Sel {
        Sel { n: Vec::with_capacity(64) }
    }

    pub fn add(&mut self, node: SNode) -> SId {
        self.n.push(node);
        SId(self.n.len() as u32 - 1)
    }

    // ---- constructors (`parser.className({...})` …)

    pub fn root(&mut self) -> SId {
        self.add(SNode::new(SKind::Root))
    }

    pub fn selector(&mut self) -> SId {
        self.add(SNode::new(SKind::Selector))
    }

    /// `parser.selector({ nodes })` — assigns the array without touching `parent`
    pub fn selector_with(&mut self, nodes: Vec<SId>) -> SId {
        let mut n = SNode::new(SKind::Selector);
        n.nodes = Some(nodes);
        self.add(n)
    }

    /// `parser.pseudo({ value, nodes })`
    pub fn pseudo_with(&mut self, value: &str, nodes: Vec<SId>) -> SId {
        let mut n = SNode::new(SKind::Pseudo);
        n.value = Some(value.into());
        n.nodes = Some(nodes);
        self.add(n)
    }

    /// `parser.className({ value })` (value set during construction: not escaped)
    pub fn class_name(&mut self, value: &str) -> SId {
        let mut n = SNode::new(SKind::Class);
        n.value = Some(value.into());
        self.add(n)
    }

    /// `parser.comment({ value })`
    pub fn comment(&mut self, value: &str) -> SId {
        let mut n = SNode::new(SKind::Comment);
        n.value = Some(value.into());
        self.add(n)
    }

    pub fn universal(&mut self) -> SId {
        self.add(SNode::new(SKind::Universal))
    }

    /// `parser.attribute({ attribute, operator, value, quoteMark })`
    pub fn attribute(&mut self, attribute: &str, operator: &str, value: &str, quote_mark: Option<char>) -> SId {
        let mut n = SNode::new(SKind::Attribute);
        n.attribute = Some(attribute.into());
        n.operator = Some(operator.into());
        n.value = Some(value.into());
        n.quote_mark = Some(quote_mark);
        n.raws = Some(SRaws::default());
        n.constructed = true;
        self.add(n)
    }

    // ---- value setters

    /// `classNode.value = v` (escapes once constructed)
    pub fn set_class_value(&mut self, id: SId, v: &str) {
        let node = &mut self[id];
        if node.constructed {
            let escaped = cssesc(v, Cssesc::identifier());
            if escaped != v {
                node.raws_mut().value = Some(escaped);
            } else if let Some(r) = node.raws.as_mut() {
                r.value = None;
            }
        }
        node.value = Some(v.into());
    }

    /// `node.namespace = ns` (Namespace setter)
    pub fn set_namespace(&mut self, id: SId, ns: Ns) {
        let node = &mut self[id];
        match &ns {
            Ns::True => {
                if let Some(r) = node.raws.as_mut() {
                    r.namespace = None;
                }
            }
            Ns::Str(s) if s == "*" || s == "&" => {
                if let Some(r) = node.raws.as_mut() {
                    r.namespace = None;
                }
            }
            Ns::Str(s) => {
                let escaped = cssesc(s, Cssesc::identifier());
                if &escaped != s {
                    node.raws_mut().namespace = Some(escaped);
                } else if let Some(r) = node.raws.as_mut() {
                    r.namespace = None;
                }
            }
        }
        node.namespace = Some(ns);
    }

    /// `node.value = v` for nodes without a setter
    pub fn set_value(&mut self, id: SId, v: &str) {
        if self[id].kind == SKind::Class {
            self.set_class_value(id, v);
        } else {
            self[id].value = Some(v.into());
        }
    }

    // ---- navigation

    pub fn children(&self, id: SId) -> &[SId] {
        self[id].nodes.as_deref().unwrap_or(&[])
    }

    pub fn len(&self, id: SId) -> usize {
        self.children(id).len()
    }

    pub fn at(&self, id: SId, i: isize) -> Option<SId> {
        if i < 0 {
            return None;
        }
        self.children(id).get(i as usize).copied()
    }

    pub fn first(&self, id: SId) -> Option<SId> {
        self.at(id, 0)
    }

    pub fn last(&self, id: SId) -> Option<SId> {
        let l = self.len(id) as isize;
        self.at(id, l - 1)
    }

    pub fn index(&self, parent: SId, child: SId) -> isize {
        self.children(parent).iter().position(|c| *c == child).map(|i| i as isize).unwrap_or(-1)
    }

    pub fn next(&self, id: SId) -> Option<SId> {
        let p = self[id].parent?;
        let i = self.index(p, id);
        self.at(p, i + 1)
    }

    pub fn prev(&self, id: SId) -> Option<SId> {
        let p = self[id].parent?;
        let i = self.index(p, id);
        self.at(p, i - 1)
    }

    // ---- mutation (Container.prototype)

    pub fn append(&mut self, container: SId, child: SId) {
        self[child].parent = Some(container);
        self[container].nodes.get_or_insert_with(Vec::new).push(child);
    }

    pub fn prepend(&mut self, container: SId, child: SId) {
        self[child].parent = Some(container);
        self[container].nodes.get_or_insert_with(Vec::new).insert(0, child);
    }

    pub fn remove_child(&mut self, container: SId, child: SId) {
        let idx = self.index(container, child);
        if idx < 0 {
            // `this.at(-1).parent = undefined` throws in JS; keep going defensively
            return;
        }
        let c = self.children(container)[idx as usize];
        self[c].parent = None;
        self[container].nodes.as_mut().unwrap().remove(idx as usize);
        for e in self[container].indexes.iter_mut() {
            if e.1 >= idx {
                e.1 -= 1;
            }
        }
    }

    pub fn remove_all(&mut self, container: SId) {
        let list = std::mem::take(self[container].nodes.as_mut().unwrap());
        for n in list {
            self[n].parent = None;
        }
    }

    pub fn insert_after(&mut self, container: SId, old: SId, new: SId) {
        self[new].parent = Some(container);
        let old_index = self.index(container, old);
        let list = self[container].nodes.as_mut().unwrap();
        let at = splice_at(old_index + 1, list.len());
        list.insert(at, new);
        self[new].parent = Some(container);
        for e in self[container].indexes.iter_mut() {
            if old_index <= e.1 {
                e.1 += 1;
            }
        }
    }

    pub fn insert_before(&mut self, container: SId, old: SId, new: SId) {
        self[new].parent = Some(container);
        let old_index = self.index(container, old);
        let list = self[container].nodes.as_mut().unwrap();
        let at = splice_at(old_index, list.len());
        list.insert(at, new);
        self[new].parent = Some(container);
        for e in self[container].indexes.iter_mut() {
            if e.1 <= old_index {
                e.1 += 1;
            }
        }
    }

    /// `node.remove()`
    pub fn remove(&mut self, id: SId) {
        if let Some(p) = self[id].parent {
            self.remove_child(p, id);
        }
        self[id].parent = None;
    }

    /// `node.replaceWith(...nodes)`
    pub fn replace_with(&mut self, id: SId, nodes: &[SId]) {
        if let Some(p) = self[id].parent {
            for &n in nodes {
                self.insert_before(p, id, n);
            }
            self.remove(id);
        }
    }

    /// `node.clone()`
    pub fn clone_node(&mut self, id: SId) -> SId {
        self.clone_rec(id, None)
    }

    fn clone_rec(&mut self, id: SId, parent: Option<SId>) -> SId {
        let mut n = self[id].clone();
        n.parent = parent;
        let children = n.nodes.take();
        let new = self.add(n);
        if let Some(children) = children {
            let mut out = Vec::with_capacity(children.len());
            for c in children {
                out.push(self.clone_rec(c, Some(new)));
            }
            self[new].nodes = Some(out);
        }
        new
    }

    // ---- iteration

    /// `container.each(cb)`; `SFlow::Stop` when a callback returned false
    pub fn each(&mut self, container: SId, f: &mut dyn FnMut(&mut Sel, SId) -> SFlow) -> SFlow {
        self[container].last_each += 1;
        let id = self[container].last_each;
        self[container].indexes.push((id, 0));
        if self.len(container) == 0 {
            // JS leaves the index registered here
            return SFlow::Continue;
        }
        let mut result = SFlow::Continue;
        loop {
            let idx = self[container].indexes.iter().find(|e| e.0 == id).unwrap().1;
            if idx >= self.len(container) as isize {
                break;
            }
            let Some(child) = self.at(container, idx) else {
                // nodes[-1] is undefined in JS (would throw on property access)
                break;
            };
            result = f(self, child);
            if result == SFlow::Stop {
                break;
            }
            self[container].indexes.iter_mut().find(|e| e.0 == id).unwrap().1 += 1;
        }
        self[container].indexes.retain(|e| e.0 != id);
        result
    }

    /// `container.walk(cb)`
    pub fn walk(&mut self, container: SId, f: &mut dyn FnMut(&mut Sel, SId) -> SFlow) -> SFlow {
        self.each(container, &mut |sel, node| {
            let mut result = f(sel, node);
            if result != SFlow::Stop && sel.len(node) > 0 {
                result = sel.walk(node, f);
            }
            result
        })
    }

    pub fn walk_kind(&mut self, container: SId, kind: SKind, f: &mut dyn FnMut(&mut Sel, SId) -> SFlow) -> SFlow {
        self.walk(container, &mut |sel, n| if sel[n].kind == kind { f(sel, n) } else { SFlow::Continue })
    }

    pub fn walk_classes(&mut self, container: SId, f: &mut dyn FnMut(&mut Sel, SId) -> SFlow) -> SFlow {
        self.walk_kind(container, SKind::Class, f)
    }

    pub fn walk_pseudos(&mut self, container: SId, f: &mut dyn FnMut(&mut Sel, SId) -> SFlow) -> SFlow {
        self.walk_kind(container, SKind::Pseudo, f)
    }

    /// Classes in walk order (read-only snapshot)
    pub fn class_values(&mut self, container: SId) -> Vec<String> {
        let mut out = Vec::new();
        self.walk_classes(container, &mut |sel, n| {
            out.push(sel[n].val().to_string());
            SFlow::Continue
        });
        out
    }

    // ---- stringification

    fn raw_space_before(&self, n: &SNode) -> String {
        let raw = n.raws.as_ref().and_then(|r| r.spaces_before.clone());
        raw.unwrap_or_else(|| n.spaces_before.clone())
    }

    fn raw_space_after(&self, n: &SNode) -> String {
        let raw = n.raws.as_ref().and_then(|r| r.spaces_after.clone());
        raw.unwrap_or_else(|| n.spaces_after.clone())
    }

    fn stringify_value(&self, n: &SNode) -> String {
        // `this.raws && this.raws[name] || this[name]`
        match n.raws.as_ref().and_then(|r| r.value.as_ref()) {
            Some(v) if !v.is_empty() => v.clone(),
            _ => match &n.value {
                Some(v) => v.clone(),
                None => "undefined".into(),
            },
        }
    }

    fn namespace_string(&self, n: &SNode) -> String {
        match &n.namespace {
            None => String::new(),
            Some(Ns::Str(s)) if s.is_empty() => String::new(),
            Some(ns) => {
                // stringifyProperty("namespace")
                if let Some(r) = n.raws.as_ref().and_then(|r| r.namespace.as_ref()) {
                    if !r.is_empty() {
                        return r.clone();
                    }
                }
                match ns {
                    Ns::True => String::new(),
                    Ns::Str(s) => s.clone(),
                }
            }
        }
    }

    fn qualified_name(&self, n: &SNode, value: &str) -> String {
        let has_ns = match &n.namespace {
            None => false,
            Some(Ns::True) => true,
            Some(Ns::Str(s)) => !s.is_empty(),
        };
        if has_ns { format!("{}|{}", self.namespace_string(n), value) } else { value.to_string() }
    }

    fn value_to_string(&self, n: &SNode) -> String {
        let v = self.stringify_value(n);
        match n.kind {
            SKind::Class => format!(".{v}"),
            SKind::Id => format!("#{v}"),
            SKind::Tag | SKind::Universal => self.qualified_name(n, &v),
            _ => v,
        }
    }

    /// `String(node)`
    pub fn to_string(&self, id: SId) -> String {
        let n = &self[id];
        match n.kind {
            SKind::Root => {
                let s = self.children(id).iter().map(|c| self.to_string(*c)).collect::<Vec<_>>().join(",");
                if n.trailing_comma { s + "," } else { s }
            }
            SKind::Selector => self.children(id).iter().map(|c| self.to_string(*c)).collect(),
            SKind::Pseudo => {
                let params = if self.len(id) > 0 {
                    format!("({})", self.children(id).iter().map(|c| self.to_string(*c)).collect::<Vec<_>>().join(","))
                } else {
                    String::new()
                };
                format!("{}{}{}{}", self.raw_space_before(n), self.stringify_value(n), params, self.raw_space_after(n))
            }
            SKind::Attribute => self.attribute_to_string(n),
            _ => format!("{}{}{}", self.raw_space_before(n), self.value_to_string(n), self.raw_space_after(n)),
        }
    }

    fn spaces_for(&self, n: &SNode, name: &str) -> (String, String) {
        let mut before = String::new();
        let mut after = String::new();
        if let Some((_, s)) = n.part_spaces.iter().find(|(k, _)| *k == name) {
            if let Some(b) = &s.before {
                before = b.clone();
            }
            if let Some(a) = &s.after {
                after = a.clone();
            }
        }
        if let Some(r) = &n.raws {
            if let Some((_, s)) = r.part_spaces.iter().find(|(k, _)| *k == name) {
                if let Some(b) = &s.before {
                    before = b.clone();
                }
                if let Some(a) = &s.after {
                    after = a.clone();
                }
            }
        }
        (before, after)
    }

    fn attribute_to_string(&self, n: &SNode) -> String {
        let mut s = self.raw_space_before(n);
        s.push('[');
        // qualifiedAttribute
        let attr = match n.raws.as_ref().and_then(|r| r.attribute.as_ref()) {
            Some(a) if !a.is_empty() => a.clone(),
            _ => n.attribute.clone().unwrap_or_else(|| "undefined".into()),
        };
        let qa = self.qualified_name(n, &attr);
        let (b, a) = self.spaces_for(n, "attribute");
        s.push_str(&b);
        s.push_str(&qa);
        s.push_str(&a);
        let has_value = n.value.is_some();
        if n.operator.as_deref().is_some_and(|o| !o.is_empty()) && has_value {
            let op = match n.raws.as_ref().and_then(|r| r.operator.as_ref()) {
                Some(o) if !o.is_empty() => o.clone(),
                _ => n.operator.clone().unwrap(),
            };
            let (b, a) = self.spaces_for(n, "operator");
            s.push_str(&b);
            s.push_str(&op);
            s.push_str(&a);
            let v = self.stringify_value(n);
            let (b, a) = self.spaces_for(n, "value");
            s.push_str(&b);
            s.push_str(&v);
            s.push_str(&a);
            // insensitiveFlag
            let flag = match n.raws.as_ref().and_then(|r| r.insensitive_flag.as_ref()) {
                Some(f) if !f.is_empty() => f.clone(),
                _ => if n.insensitive == Some(true) { "i".to_string() } else { String::new() },
            };
            let (mut b, a) = self.spaces_for(n, "insensitive");
            let quoted = matches!(n.quote_mark, Some(Some(_)));
            let value_after = n.part_spaces.iter().find(|(k, _)| *k == "value").and_then(|(_, s)| s.after.clone());
            if !flag.is_empty() && !quoted && b.is_empty() && !value_after.is_some_and(|a| !a.is_empty()) {
                b = " ".into();
            }
            s.push_str(&b);
            s.push_str(&flag);
            s.push_str(&a);
        }
        s.push(']');
        s.push_str(&self.raw_space_after(n));
        s
    }

    /// Sort the container's nodes in place with a JS-style comparator.
    pub fn sort_nodes(&mut self, container: SId, cmp: &mut dyn FnMut(&Sel, SId, SId) -> i32) {
        let mut list = self[container].nodes.take().unwrap_or_default();
        crate::js::sort_by(&mut list, |a, b| cmp(self, *a, *b));
        self[container].nodes = Some(list);
    }
}

fn splice_at(i: isize, len: usize) -> usize {
    if i < 0 {
        let r = len as isize + i;
        if r < 0 { 0 } else { r as usize }
    } else {
        (i as usize).min(len)
    }
}

/// `unesc` from postcss-selector-parser
pub fn unesc(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut ret = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            // gobbleHex(str.slice(i + 1, i + 7))
            let mut hex = String::new();
            let mut space_terminated = false;
            let mut j = 0;
            while j < 6 && i + 1 + j < chars.len() {
                let c = chars[i + 1 + j].to_ascii_lowercase();
                space_terminated = c == ' ';
                if !(c.is_ascii_digit() || ('a'..='f').contains(&c)) {
                    break;
                }
                hex.push(c);
                j += 1;
            }
            if !hex.is_empty() {
                let cp = u32::from_str_radix(&hex, 16).unwrap();
                let is_surrogate = (0xD800..=0xDFFF).contains(&cp);
                let ch = if is_surrogate || cp == 0 || cp > 0x10FFFF { '\u{FFFD}' } else { char::from_u32(cp).unwrap() };
                ret.push(ch);
                i += hex.len() + if space_terminated { 1 } else { 0 } + 1;
                continue;
            }
            if chars.get(i + 1) == Some(&'\\') {
                ret.push('\\');
                i += 2;
                continue;
            }
            if chars.len() == i + 1 {
                ret.push('\\');
            }
            i += 1;
            continue;
        }
        ret.push(chars[i]);
        i += 1;
    }
    ret
}
