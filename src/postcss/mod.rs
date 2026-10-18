//! Port of the parts of `postcss@8.4.49` tailwind and autoprefixer use: the node model
//! (`node.js`, `container.js`, `root.js`, …), the tokenizer, the parser, the stringifier
//! (with its raw-style inference and per-root cache) and `list`.
//!
//! Nodes live in an arena ([`Css`]) and are addressed by [`NodeId`]. Containers keep the
//! JS iteration bookkeeping (`indexes`) so walks that mutate the tree behave exactly like
//! postcss's `each`.

mod list;
mod parser;
mod stringify;
mod tokenize;

pub use list::{comma, space, split};
pub use parser::parse;
pub use stringify::{INFERRED_RAWS, infer_kind, infer_raw, raw, raw_cache, set_raw_cache, to_string, to_string_parts, to_string_spliced};

use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Root,
    Rule,
    AtRule,
    Decl,
    Comment,
}

/// `raws.selector` / `raws.value` / `raws.params`
#[derive(Debug, Clone, PartialEq)]
pub struct RawValue {
    pub raw: String,
    pub value: String,
}

#[derive(Debug, Clone, Default)]
pub struct Raws {
    pub before: Option<String>,
    pub after: Option<String>,
    pub between: Option<String>,
    pub semicolon: Option<bool>,
    /// `raws.neededBackup` (tailwind variants)
    pub needed_backup: bool,
    /// `raws.tailwind`
    pub tailwind: Option<Box<TwRaws>>,
    /// the raws most nodes do not have
    pub more: Option<Box<RawsMore>>,
}

#[derive(Debug, Clone, Default)]
pub struct RawsMore {
    pub after_name: Option<String>,
    pub important: Option<String>,
    pub left: Option<String>,
    pub right: Option<String>,
    pub own_semicolon: Option<String>,
    pub indent: Option<String>,
    pub selector: Option<RawValue>,
    pub value: Option<RawValue>,
    pub params: Option<RawValue>,
    /// `raws.originalSelector` (tailwind variants)
    pub original_selector: Option<String>,
}

static NO_MORE: RawsMore = RawsMore {
    after_name: None,
    important: None,
    left: None,
    right: None,
    own_semicolon: None,
    indent: None,
    selector: None,
    value: None,
    params: None,
    original_selector: None,
};

impl Raws {
    pub fn more(&self) -> &RawsMore {
        self.more.as_deref().unwrap_or(&NO_MORE)
    }
    pub fn more_mut(&mut self) -> &mut RawsMore {
        self.more.get_or_insert_with(Default::default)
    }
}

/// `raws.tailwind`
#[derive(Debug, Clone, Default)]
pub struct TwRaws {
    pub candidate: Option<String>,
    pub class_candidate: Option<String>,
    pub preserve_source: Option<bool>,
    pub parent_layer: Option<String>,
    pub layer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
    /// byte offset in the input
    pub offset: usize,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub input: Arc<Input>,
    pub start: Option<Position>,
    pub end: Option<Position>,
}

#[derive(Debug)]
pub struct Input {
    pub css: String,
    line_starts: Vec<usize>,
}

impl Input {
    pub fn new(css: &str) -> Arc<Input> {
        let css = css.strip_prefix('\u{feff}').or_else(|| css.strip_prefix('\u{fffe}')).unwrap_or(css);
        let mut line_starts = vec![0];
        for (i, b) in css.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Arc::new(Input { css: css.to_string(), line_starts })
    }

    /// line/column (1-based, UTF-16 columns) for a byte offset
    pub fn from_offset(&self, offset: usize) -> (usize, usize) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.line_starts[line];
        let end = offset.min(self.css.len());
        let col = if start <= end {
            utf16_units(&self.css.as_bytes()[start..end]) + 1
        } else {
            1
        };
        (line + 1, col)
    }

    pub fn position(&self, offset: usize) -> Position {
        let (line, column) = self.from_offset(offset);
        Position { line, column, offset }
    }
}

fn utf16_units(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.encode_utf16().count(),
        Err(_) => String::from_utf8_lossy(bytes).encode_utf16().count(),
    }
}

/// A `CssSyntaxError` (or a plain `Error` thrown by a plugin when `line` is `None`).
#[derive(Debug, Clone, PartialEq)]
pub struct CssError {
    pub name: &'static str,
    pub reason: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub end_line: Option<usize>,
    pub end_column: Option<usize>,
}

impl CssError {
    pub fn syntax(reason: impl Into<String>, start: Option<(usize, usize)>, end: Option<(usize, usize)>) -> Self {
        CssError {
            name: "CssSyntaxError",
            reason: reason.into(),
            line: start.map(|s| s.0),
            column: start.map(|s| s.1),
            end_line: end.map(|e| e.0),
            end_column: end.map(|e| e.1),
        }
    }

    pub fn plain(reason: impl Into<String>) -> Self {
        CssError { name: "Error", reason: reason.into(), line: None, column: None, end_line: None, end_column: None }
    }

    /// `error.message` as JS formats it
    pub fn message(&self) -> String {
        if self.name != "CssSyntaxError" {
            return self.reason.clone();
        }
        let mut m = "<css input>".to_string();
        if let (Some(l), Some(c)) = (self.line, self.column) {
            m.push_str(&format!(":{l}:{c}"));
        }
        m.push_str(": ");
        m.push_str(&self.reason);
        m
    }
}

impl std::fmt::Display for CssError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for CssError {}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: Kind,
    pub parent: Option<NodeId>,
    pub nodes: Option<Vec<NodeId>>,
    pub raws: Raws,
    pub selector: String,
    pub name: String,
    pub params: String,
    pub prop: String,
    pub value: String,
    pub important: bool,
    pub text: String,
    pub source: Option<Source>,
    /// container iteration state: (iterator id, index)
    indexes: Vec<(u32, usize)>,
    last_each: u32,
    /// `root.rawCache` (on whatever node is the top of the tree)
    pub(crate) raw_cache: Option<Box<Vec<(&'static str, String)>>>,
    /// autoprefixer's `_autoprefixerDisabled` / `_autoprefixerSelfDisabled`
    pub ap_disabled: Option<bool>,
    pub ap_self_disabled: bool,
    /// `_autoprefixerCascade` (only truthy values stick)
    pub ap_cascade: bool,
    /// `_autoprefixerPrefix`, `_autoprefixerMax`, `_autoprefixerValues`
    pub ap: Option<Box<ApCache>>,
}

#[derive(Debug, Clone, Default)]
pub struct ApCache {
    /// None = not computed, Some(None) = false
    pub prefix: Option<Option<String>>,
    pub max: Option<usize>,
    pub values: Option<Vec<(String, String)>>,
}

impl Node {
    pub fn new(kind: Kind) -> Node {
        Node {
            kind,
            parent: None,
            nodes: if matches!(kind, Kind::Root | Kind::Rule) { Some(Vec::new()) } else { None },
            raws: Raws::default(),
            selector: String::new(),
            name: String::new(),
            params: String::new(),
            prop: String::new(),
            value: String::new(),
            important: false,
            text: String::new(),
            source: None,
            indexes: Vec::new(),
            last_each: 0,
            raw_cache: None,
            ap_disabled: None,
            ap_self_disabled: false,
            ap_cascade: false,
            ap: None,
        }
    }

    pub fn tw(&self) -> Option<&TwRaws> {
        self.raws.tailwind.as_deref()
    }

    pub fn tw_mut(&mut self) -> &mut TwRaws {
        self.raws.tailwind.get_or_insert_with(Default::default)
    }

    pub fn is_container(&self) -> bool {
        self.nodes.is_some()
    }
}

/// How a value passed to `append`/`before`/… is interpreted (`Container.normalize`).
pub enum Add {
    /// a single node (`nodes.type`)
    Node(NodeId),
    /// an array of nodes (or a root's children): detached with `removeChild(i, 'ignore')`
    Nodes(Vec<NodeId>),
}

impl From<NodeId> for Add {
    fn from(n: NodeId) -> Self {
        Add::Node(n)
    }
}
impl From<Vec<NodeId>> for Add {
    fn from(n: Vec<NodeId>) -> Self {
        Add::Nodes(n)
    }
}

/// Walk control: `return false` from a postcss callback stops the walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

impl Flow {
    pub fn stop_if(b: bool) -> Flow {
        if b { Flow::Stop } else { Flow::Continue }
    }
}

/// The node arena. `freeze` moves the nodes into a shared, immutable layer, so a clone after it
/// is cheap: frozen nodes are copied into the clone only when it mutates them.
/// See `Css::walk_down`.
pub struct WalkDown<'a> {
    css: &'a Css,
    /// (container, its parent, next child index)
    stack: Vec<(NodeId, Option<NodeId>, usize)>,
}

impl Iterator for WalkDown<'_> {
    type Item = (NodeId, NodeId, Option<NodeId>);
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (c, up, i) = *self.stack.last()?;
            match self.css.children(c).get(i) {
                Some(&n) => {
                    self.stack.last_mut().unwrap().2 += 1;
                    self.stack.push((n, Some(c), 0));
                    return Some((n, c, up));
                }
                None => {
                    self.stack.pop();
                }
            }
        }
    }
}

#[derive(Default, Clone)]
pub struct Css {
    frozen: Option<std::sync::Arc<Frozen>>,
    /// per frozen node: index of its mutable copy in `copies`, or `u32::MAX`
    copied: Vec<u32>,
    copies: Vec<Node>,
    /// nodes with ids from the frozen length on
    nodes: Vec<Node>,
}

/// One frozen layer: the nodes with ids `start..`, and replacements for lower ids that were
/// mutated before this layer was frozen.
struct Frozen {
    below: Option<std::sync::Arc<Frozen>>,
    start: usize,
    nodes: Vec<Node>,
    replaced: Vec<u32>,
    replacements: Vec<Node>,
}

impl Frozen {
    fn len(&self) -> usize {
        self.start + self.nodes.len()
    }

    fn get(&self, i: usize) -> &Node {
        let mut layer = self;
        loop {
            if i >= layer.start {
                return &layer.nodes[i - layer.start];
            }
            match layer.replaced[i] {
                u32::MAX => layer = layer.below.as_deref().expect("frozen id"),
                r => return &layer.replacements[r as usize],
            }
        }
    }
}

impl Css {
    #[inline]
    fn frozen_len(&self) -> usize {
        self.frozen.as_ref().map_or(0, |f| f.len())
    }
}

impl std::ops::Index<NodeId> for Css {
    type Output = Node;
    #[inline]
    fn index(&self, id: NodeId) -> &Node {
        let i = id.0 as usize;
        let fl = self.frozen_len();
        if i < fl {
            match self.copied[i] {
                u32::MAX => self.frozen.as_ref().unwrap().get(i),
                c => &self.copies[c as usize],
            }
        } else {
            &self.nodes[i - fl]
        }
    }
}
impl std::ops::IndexMut<NodeId> for Css {
    #[inline]
    fn index_mut(&mut self, id: NodeId) -> &mut Node {
        let i = id.0 as usize;
        let fl = self.frozen_len();
        if i < fl {
            if self.copied[i] == u32::MAX {
                self.copied[i] = self.copies.len() as u32;
                let n = self.frozen.as_ref().unwrap().get(i).clone();
                self.copies.push(n);
            }
            &mut self.copies[self.copied[i] as usize]
        } else {
            &mut self.nodes[i - fl]
        }
    }
}

impl Css {
    pub fn new() -> Css {
        Css { nodes: Vec::with_capacity(1024), ..Default::default() }
    }

    pub fn add(&mut self, node: Node) -> NodeId {
        self.nodes.push(node);
        NodeId((self.frozen_len() + self.nodes.len()) as u32 - 1)
    }

    pub fn len(&self) -> usize {
        self.frozen_len() + self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Moves every node into a new frozen layer (ids stay the same), without copying.
    pub fn freeze(&mut self) {
        let below = self.frozen.take();
        let start = below.as_ref().map_or(0, |f| f.len());
        let layer = Frozen {
            below,
            start,
            nodes: std::mem::take(&mut self.nodes),
            replaced: std::mem::take(&mut self.copied),
            replacements: std::mem::take(&mut self.copies),
        };
        let len = layer.len();
        // a layer without replacements needs no lookup table below it
        let layer = if layer.replacements.is_empty() { Frozen { replaced: vec![u32::MAX; start], ..layer } } else { layer };
        self.frozen = Some(std::sync::Arc::new(layer));
        self.copied = vec![u32::MAX; len];
    }

    /// Drops every tree none of whose nodes is in `refs`, renumbering the rest (relative order
    /// kept). Returns old id → new id (`u32::MAX` for dropped nodes).
    pub fn compact(&mut self, refs: &[NodeId]) -> Vec<u32> {
        let len = self.len();
        let mut keep = vec![false; len];
        let mut stack: Vec<NodeId> = Vec::new();
        for &r in refs {
            let top = self.top(r);
            if !keep[top.0 as usize] {
                stack.push(top);
                while let Some(n) = stack.pop() {
                    if !keep[n.0 as usize] {
                        keep[n.0 as usize] = true;
                        stack.extend(self.children(n).iter().copied());
                    }
                }
            }
        }
        let mut map = vec![u32::MAX; len];
        let mut next = 0;
        for (i, k) in keep.iter().enumerate() {
            if *k {
                map[i] = next;
                next += 1;
            }
        }
        let m = |id: NodeId| NodeId(map[id.0 as usize]);
        let mut nodes = Vec::with_capacity(next as usize);
        let old: Vec<Node> = if self.frozen.is_none() {
            std::mem::take(&mut self.nodes)
        } else {
            (0..len).map(|i| self[NodeId(i as u32)].clone()).collect()
        };
        for (n, k) in old.into_iter().zip(keep) {
            if k {
                let mut n = n;
                n.parent = n.parent.map(m);
                if let Some(list) = &mut n.nodes {
                    for c in list.iter_mut() {
                        *c = m(*c);
                    }
                }
                nodes.push(n);
            }
        }
        *self = Css { nodes, ..Default::default() };
        map
    }

    // ---- factories (`postcss.rule({...})` etc.)

    pub fn root(&mut self) -> NodeId {
        self.add(Node::new(Kind::Root))
    }

    /// `postcss.root({ nodes })`: every node is cloned and appended.
    pub fn root_with(&mut self, nodes: &[NodeId]) -> NodeId {
        let r = self.root();
        for &n in nodes {
            let c = self.clone_node(n);
            self.append(r, Add::Node(c));
        }
        r
    }

    pub fn rule(&mut self, selector: &str) -> NodeId {
        let mut n = Node::new(Kind::Rule);
        n.selector = selector.to_string();
        self.add(n)
    }

    /// `postcss.rule({ selectors })`
    pub fn rule_with_selectors(&mut self, selectors: &[String]) -> NodeId {
        let id = self.add(Node::new(Kind::Rule));
        self.set_selectors(id, selectors);
        id
    }

    pub fn at_rule(&mut self, name: &str, params: &str) -> NodeId {
        let mut n = Node::new(Kind::AtRule);
        n.name = name.to_string();
        n.params = params.to_string();
        self.add(n)
    }

    pub fn decl(&mut self, prop: &str, value: &str) -> NodeId {
        let mut n = Node::new(Kind::Decl);
        n.prop = prop.to_string();
        n.value = value.to_string();
        self.add(n)
    }

    pub fn comment(&mut self, text: &str) -> NodeId {
        let mut n = Node::new(Kind::Comment);
        n.text = text.to_string();
        self.add(n)
    }

    // ---- tree navigation

    pub fn kind(&self, id: NodeId) -> Kind {
        self[id].kind
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self[id].parent
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self[id].nodes.as_deref().unwrap_or(&[])
    }

    pub fn first(&self, id: NodeId) -> Option<NodeId> {
        self.children(id).first().copied()
    }

    pub fn last(&self, id: NodeId) -> Option<NodeId> {
        self.children(id).last().copied()
    }

    pub fn index(&self, parent: NodeId, child: NodeId) -> Option<usize> {
        self.children(parent).iter().position(|c| *c == child)
    }

    pub fn next(&self, id: NodeId) -> Option<NodeId> {
        let p = self[id].parent?;
        let i = self.index(p, id)?;
        self.children(p).get(i + 1).copied()
    }

    pub fn prev(&self, id: NodeId) -> Option<NodeId> {
        let p = self[id].parent?;
        let i = self.index(p, id)?;
        if i == 0 { None } else { self.children(p).get(i - 1).copied() }
    }

    /// `node.root()`
    pub fn top(&self, id: NodeId) -> NodeId {
        let mut r = id;
        while let Some(p) = self[r].parent {
            r = p;
        }
        r
    }

    // ---- cloning

    /// `node.clone()`: a deep copy without a parent.
    pub fn clone_node(&mut self, id: NodeId) -> NodeId {
        let new = self.clone_rec(id, None);
        new
    }

    fn clone_rec(&mut self, id: NodeId, parent: Option<NodeId>) -> NodeId {
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

    /// A deep copy of `other`'s node `id` (no parent) in this arena.
    pub fn copy_from(&mut self, other: &Css, id: NodeId) -> NodeId {
        let mut n = other[id].clone();
        n.parent = None;
        let children = n.nodes.take();
        let new = self.add(n);
        if let Some(children) = children {
            let mut out = Vec::with_capacity(children.len());
            for c in children {
                let k = self.copy_from(other, c);
                self[k].parent = Some(new);
                out.push(k);
            }
            self[new].nodes = Some(out);
        }
        new
    }

    /// Whether two subtrees (possibly in different arenas) have the same content.
    pub fn same_tree(&self, a: NodeId, other: &Css, b: NodeId) -> bool {
        let (x, y) = (&self[a], &other[b]);
        let same = x.kind == y.kind
            && x.selector == y.selector
            && x.name == y.name
            && x.params == y.params
            && x.prop == y.prop
            && x.value == y.value
            && x.important == y.important
            && x.text == y.text
            && x.source.is_some() == y.source.is_some()
            && format!("{:?}", x.raws) == format!("{:?}", y.raws)
            && x.nodes.as_ref().map(|v| v.len()) == y.nodes.as_ref().map(|v| v.len());
        same && self.children(a).iter().zip(other.children(b)).all(|(&p, &q)| self.same_tree(p, other, q))
    }

    /// `node.clone({ nodes: [] })`
    pub fn clone_empty(&mut self, id: NodeId) -> NodeId {
        let mut n = self[id].clone();
        n.parent = None;
        n.nodes = Some(Vec::new());
        self.add(n)
    }

    /// `shallowClone`: the node itself, children replaced by the given (already detached) nodes
    pub fn clone_shallow(&mut self, id: NodeId) -> NodeId {
        let mut n = self[id].clone();
        n.parent = None;
        if n.nodes.is_some() {
            n.nodes = Some(Vec::new());
        }
        self.add(n)
    }

    // ---- container mutation (Container.prototype)

    fn normalize(&mut self, container: NodeId, add: Add, sample: Option<NodeId>, prepend: bool) -> Vec<NodeId> {
        let is_root = self[container].kind == Kind::Root;
        let nodes: Vec<NodeId> = match add {
            Add::Nodes(list) => {
                for &i in &list {
                    if let Some(p) = self[i].parent {
                        self.remove_child_ignore(p, i, true);
                    }
                }
                list
            }
            Add::Node(n) => vec![n],
        };
        // Root#normalize calls super.normalize(child) without a sample
        let base_sample = if is_root { None } else { sample };
        for &i in &nodes {
            if let Some(p) = self[i].parent {
                self.remove_child_ignore(p, i, false);
            }
            if self[i].raws.before.is_none() {
                if let Some(s) = base_sample {
                    if let Some(b) = &self[s].raws.before {
                        let b = b.chars().filter(|c| c.is_whitespace_js()).collect::<String>();
                        self[i].raws.before = Some(b);
                    }
                }
            }
            self[i].parent = Some(container);
        }
        if is_root {
            if let Some(sample) = sample {
                if prepend {
                    let list = self.children(container);
                    if list.len() > 1 {
                        let b = self[list[1]].raws.before.clone();
                        self[sample].raws.before = b;
                    } else {
                        self[sample].raws.before = None;
                    }
                } else if self.first(container) != Some(sample) {
                    let b = self[sample].raws.before.clone();
                    for &n in &nodes {
                        self[n].raws.before = b.clone();
                    }
                }
            }
        }
        nodes
    }

    pub fn append(&mut self, container: NodeId, add: impl Into<Add>) {
        if self[container].nodes.is_none() {
            self[container].nodes = Some(Vec::new());
        }
        let last = self.last(container);
        let nodes = self.normalize(container, add.into(), last, false);
        self[container].nodes.as_mut().unwrap().extend(nodes);
    }

    /// `container.push(child)`: raw push without normalization (used by parsers)
    pub fn push(&mut self, container: NodeId, child: NodeId) {
        self[child].parent = Some(container);
        self[container].nodes.get_or_insert_with(Vec::new).push(child);
    }

    pub fn prepend(&mut self, container: NodeId, add: impl Into<Add>) {
        if self[container].nodes.is_none() {
            self[container].nodes = Some(Vec::new());
        }
        let first = self.first(container);
        let mut nodes = self.normalize(container, add.into(), first, true);
        nodes.reverse();
        let n = nodes.len();
        for node in nodes {
            self[container].nodes.as_mut().unwrap().insert(0, node);
        }
        for e in self[container].indexes.iter_mut() {
            e.1 += n;
        }
    }

    pub fn insert_before(&mut self, container: NodeId, exist: NodeId, add: impl Into<Add>) {
        let exist_index = self.index(container, exist).unwrap_or(usize::MAX);
        let prepend = exist_index == 0;
        let sample = self.children(container).get(exist_index).copied();
        let mut nodes = self.normalize(container, add.into(), sample, prepend);
        nodes.reverse();
        let exist_index = self.index(container, exist).map(|i| i as isize).unwrap_or(-1);
        let n = nodes.len();
        for node in nodes {
            let list = self[container].nodes.as_mut().unwrap();
            let at = splice_index(exist_index, list.len());
            list.insert(at, node);
        }
        for e in self[container].indexes.iter_mut() {
            if exist_index <= e.1 as isize {
                e.1 += n;
            }
        }
    }

    pub fn insert_after(&mut self, container: NodeId, exist: NodeId, add: impl Into<Add>) {
        let exist_index = self.index(container, exist).unwrap_or(usize::MAX);
        let sample = self.children(container).get(exist_index).copied();
        let mut nodes = self.normalize(container, add.into(), sample, false);
        nodes.reverse();
        let exist_index = self.index(container, exist).map(|i| i as isize).unwrap_or(-1);
        let n = nodes.len();
        for node in nodes {
            let list = self[container].nodes.as_mut().unwrap();
            let at = splice_index(exist_index + 1, list.len());
            list.insert(at, node);
        }
        for e in self[container].indexes.iter_mut() {
            if exist_index < e.1 as isize {
                e.1 += n;
            }
        }
    }

    /// `node.before(add)`
    pub fn before(&mut self, node: NodeId, add: impl Into<Add>) {
        let p = self[node].parent.expect("before() on a detached node");
        self.insert_before(p, node, add);
    }

    /// `node.after(add)`
    pub fn after(&mut self, node: NodeId, add: impl Into<Add>) {
        let p = self[node].parent.expect("after() on a detached node");
        self.insert_after(p, node, add);
    }

    pub fn remove_child(&mut self, container: NodeId, child: NodeId) {
        self.remove_child_ignore(container, child, false)
    }

    /// `container.removeChild(child, 'ignore')`: no `raws.before` hand-over in a root.
    pub fn detach(&mut self, child: NodeId) {
        if let Some(p) = self[child].parent {
            self.remove_child_ignore(p, child, true);
        }
    }

    fn remove_child_ignore(&mut self, container: NodeId, child: NodeId, ignore: bool) {
        let Some(index) = self.index(container, child) else { return };
        if self[container].kind == Kind::Root && !ignore && index == 0 && self.children(container).len() > 1 {
            let b = self[child].raws.before.clone();
            let second = self.children(container)[1];
            self[second].raws.before = b;
        }
        self[child].parent = None;
        self[container].nodes.as_mut().unwrap().remove(index);
        for e in self[container].indexes.iter_mut() {
            if e.1 >= index {
                e.1 = e.1.wrapping_sub(1);
            }
        }
    }

    /// `node.remove()`
    pub fn remove(&mut self, node: NodeId) {
        if let Some(p) = self[node].parent {
            self.remove_child(p, node);
        }
        self[node].parent = None;
    }

    pub fn remove_all(&mut self, container: NodeId) {
        let list = std::mem::take(self[container].nodes.as_mut().unwrap());
        for n in list {
            self[n].parent = None;
        }
    }

    /// `node.replaceWith(...nodes)`
    pub fn replace_with(&mut self, node: NodeId, nodes: &[NodeId]) {
        let Some(p) = self[node].parent else { return };
        let mut bookmark = node;
        let mut found_self = false;
        for &n in nodes {
            if n == node {
                found_self = true;
            } else if found_self {
                self.insert_after(p, bookmark, Add::Node(n));
                bookmark = n;
            } else {
                self.insert_before(p, bookmark, Add::Node(n));
            }
        }
        if !found_self {
            self.remove(node);
        }
    }

    // ---- iteration (`each` / `walk*`)

    /// `container.each(cb)`; returns `Flow::Stop` when a callback returned false.
    pub fn each(&mut self, container: NodeId, f: &mut dyn FnMut(&mut Css, NodeId) -> Flow) -> Flow {
        if self[container].nodes.is_none() {
            return Flow::Continue;
        }
        self[container].last_each += 1;
        let iter = self[container].last_each;
        self[container].indexes.push((iter, 0));
        let mut result = Flow::Continue;
        loop {
            let idx = self[container].indexes.iter().find(|e| e.0 == iter).unwrap().1;
            let list = self[container].nodes.as_ref().unwrap();
            if idx >= list.len() || (idx as isize) < 0 {
                break;
            }
            let child = list[idx];
            result = f(self, child);
            if result == Flow::Stop {
                break;
            }
            let e = self[container].indexes.iter_mut().find(|e| e.0 == iter).unwrap();
            e.1 = e.1.wrapping_add(1);
        }
        self[container].indexes.retain(|e| e.0 != iter);
        result
    }

    /// `container.walk(cb)`
    pub fn walk(&mut self, container: NodeId, f: &mut dyn FnMut(&mut Css, NodeId) -> Flow) -> Flow {
        self.each(container, &mut |css, child| {
            let mut result = f(css, child);
            if result != Flow::Stop && css.has_walk(child) {
                result = css.walk(child, f);
            }
            result
        })
    }

    fn has_walk(&self, id: NodeId) -> bool {
        matches!(self[id].kind, Kind::Root | Kind::Rule | Kind::AtRule)
    }

    pub fn walk_rules(&mut self, container: NodeId, f: &mut dyn FnMut(&mut Css, NodeId) -> Flow) -> Flow {
        self.walk(container, &mut |css, n| if css[n].kind == Kind::Rule { f(css, n) } else { Flow::Continue })
    }

    pub fn walk_decls(&mut self, container: NodeId, f: &mut dyn FnMut(&mut Css, NodeId) -> Flow) -> Flow {
        self.walk(container, &mut |css, n| if css[n].kind == Kind::Decl { f(css, n) } else { Flow::Continue })
    }

    pub fn walk_at_rules(&mut self, container: NodeId, f: &mut dyn FnMut(&mut Css, NodeId) -> Flow) -> Flow {
        self.walk(container, &mut |css, n| if css[n].kind == Kind::AtRule { f(css, n) } else { Flow::Continue })
    }

    pub fn walk_at_rules_named(
        &mut self,
        container: NodeId,
        name: &str,
        f: &mut dyn FnMut(&mut Css, NodeId) -> Flow,
    ) -> Flow {
        self.walk(container, &mut |css, n| {
            if css[n].kind == Kind::AtRule && css[n].name == name { f(css, n) } else { Flow::Continue }
        })
    }

    pub fn walk_comments(&mut self, container: NodeId, f: &mut dyn FnMut(&mut Css, NodeId) -> Flow) -> Flow {
        self.walk(container, &mut |css, n| if css[n].kind == Kind::Comment { f(css, n) } else { Flow::Continue })
    }

    /// Read-only pre-order list of descendants (snapshot; no mutation semantics).
    pub fn descendants(&self, container: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.collect(container, &mut out);
        out
    }

    /// `descendants` as a lazy walk: (node, parent, grandparent) as reached through the child
    /// lists, in document order.
    pub fn walk_down(&self, container: NodeId) -> WalkDown<'_> {
        WalkDown { css: self, stack: vec![(container, None, 0)] }
    }

    fn collect(&self, id: NodeId, out: &mut Vec<NodeId>) {
        for &c in self.children(id) {
            out.push(c);
            self.collect(c, out);
        }
    }

    // ---- rule selectors

    /// `rule.selectors`
    pub fn selectors(&self, rule: NodeId) -> Vec<String> {
        list::comma(&self[rule].selector)
    }

    /// `rule.selectors = values`
    pub fn set_selectors(&mut self, rule: NodeId, values: &[String]) {
        let sel = &self[rule].selector;
        let sep = match find_comma_sep(sel) {
            Some(s) => s,
            None => format!(",{}", stringify::raw(self, rule, Some("between"), "beforeOpen")),
        };
        self[rule].selector = values.join(&sep);
    }

    // ---- errors

    /// `node.error(message)`
    pub fn error(&self, node: NodeId, message: impl Into<String>) -> CssError {
        self.error_opts(node, message, None)
    }

    /// `node.error(message, { word })`
    pub fn error_opts(&self, node: NodeId, message: impl Into<String>, word: Option<&str>) -> CssError {
        let message = message.into();
        let Some(source) = &self[node].source else {
            return CssError::syntax(message, None, None);
        };
        let Some(start_pos) = source.start else {
            return CssError::syntax(message, None, None);
        };
        let input = &source.input;
        let mut start = (start_pos.line, start_pos.column);
        let mut end = match source.end {
            Some(e) => (e.line, e.column + 1),
            None => (start.0, start.1 + 1),
        };
        if let Some(word) = word {
            let s = start_pos.offset;
            let e = source.end.map(|e| e.offset).unwrap_or(s);
            let repr = input.css.get(s..e.max(s)).unwrap_or("");
            if let Some(idx) = repr.find(word) {
                start = input.from_offset(s + idx);
                end = input.from_offset(s + idx + word.len());
            }
        }
        if end.0 < start.0 || (end.0 == start.0 && end.1 <= start.1) {
            end = (start.0, start.1 + 1);
        }
        CssError::syntax(message, Some(start), Some(end))
    }
}

fn splice_index(i: isize, len: usize) -> usize {
    // Array.prototype.splice start semantics
    if i < 0 {
        let r = len as isize + i;
        if r < 0 { 0 } else { r as usize }
    } else {
        (i as usize).min(len)
    }
}

/// `selector.match(/,\s*/)`
fn find_comma_sep(s: &str) -> Option<String> {
    let i = s.find(',')?;
    let rest = &s[i + 1..];
    let ws: String = rest.chars().take_while(|c| c.is_whitespace_js()).collect();
    Some(format!(",{ws}"))
}

pub(crate) trait CharExt {
    fn is_whitespace_js(&self) -> bool;
}
impl CharExt for char {
    fn is_whitespace_js(&self) -> bool {
        crate::js::is_js_whitespace(*self)
    }
}
