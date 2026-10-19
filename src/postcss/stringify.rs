//! `postcss/lib/stringifier.js`, including raw-style detection and the per-root `rawCache`.

use super::*;

fn default_raw(detect: &str) -> RawV {
    RawV::S(
        match detect {
            "after" => "\n",
            "beforeClose" => "\n",
            "beforeComment" => "\n",
            "beforeDecl" => "\n",
            "beforeOpen" => " ",
            "beforeRule" => "\n",
            "colon" => ": ",
            "commentLeft" => " ",
            "commentRight" => " ",
            "emptyBody" => "",
            "indent" => "    ",
            "semicolon" => return RawV::B(false),
            _ => return RawV::Undef,
        }
        .to_string(),
    )
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RawV {
    Undef,
    S(String),
    B(bool),
}

impl RawV {
    fn into_string(self) -> String {
        match self {
            RawV::S(s) => s,
            RawV::B(b) => b.to_string(),
            RawV::Undef => "undefined".to_string(),
        }
    }
}

fn own_raw(n: &Node, own: &str) -> RawV {
    let r = &n.raws;
    let m = r.more();
    let s = |v: &Option<String>| v.clone().map(RawV::S).unwrap_or(RawV::Undef);
    match own {
        "before" => s(&r.before),
        "after" => s(&r.after),
        "between" => s(&r.between),
        "left" => s(&m.left),
        "right" => s(&m.right),
        "afterName" => s(&m.after_name),
        "important" => s(&m.important),
        "ownSemicolon" => s(&m.own_semicolon),
        "indent" => s(&m.indent),
        "semicolon" => r.semicolon.map(RawV::B).unwrap_or(RawV::Undef),
        _ => RawV::Undef,
    }
}

fn detect_key(detect: &str) -> &'static str {
    match detect {
        "before" => "before",
        "after" => "after",
        "beforeClose" => "beforeClose",
        "beforeComment" => "beforeComment",
        "beforeDecl" => "beforeDecl",
        "beforeOpen" => "beforeOpen",
        "beforeRule" => "beforeRule",
        "colon" => "colon",
        "commentLeft" => "commentLeft",
        "commentRight" => "commentRight",
        "emptyBody" => "emptyBody",
        "indent" => "indent",
        "semicolon" => "semicolon",
        "between" => "between",
        "left" => "left",
        "right" => "right",
        _ => "other",
    }
}

/// `node.raw(own, detect)` — returns the raw as a string (`"true"`/`"false"` for semicolon).
pub fn raw(css: &mut Css, node: NodeId, own: Option<&str>, detect: &str) -> String {
    raw_v(css, node, own, Some(detect)).into_string()
}

pub(crate) fn raw_v(css: &mut Css, node: NodeId, own: Option<&str>, detect: Option<&str>) -> RawV {
    let detect = detect.or(own).unwrap();
    if let Some(own) = own {
        let v = own_raw(&css[node], own);
        if v != RawV::Undef {
            return v;
        }
    }
    let parent = css[node].parent;
    if detect == "before" {
        match parent {
            None => return RawV::S(String::new()),
            Some(p) if css[p].kind == Kind::Root && css.first(p) == Some(node) => return RawV::S(String::new()),
            _ => {}
        }
    }
    if parent.is_none() {
        return default_raw(detect);
    }
    let root = css.top(node);
    let key = detect_key(detect);
    if let Some(cache) = &css[root].raw_cache {
        if let Some((_, v)) = cache.iter().find(|(k, _)| *k == key) {
            return parse_cached(key, v);
        }
    }

    if detect == "before" || detect == "after" {
        return RawV::S(before_after(css, node, detect));
    }
    let mut value = match detect {
        "beforeClose" => raw_before_close(css, root),
        "beforeComment" => raw_before_comment(css, root, node),
        "beforeDecl" => raw_before_decl(css, root, node),
        "beforeOpen" => raw_before_open(css, root),
        "beforeRule" => raw_before_rule(css, root),
        "colon" => raw_colon(css, root),
        "emptyBody" => raw_empty_body(css, root),
        "indent" => raw_indent(css, root),
        "semicolon" => raw_semicolon(css, root),
        _ => {
            let own = own.unwrap_or("");
            let mut found = RawV::Undef;
            for (i, _, _) in css.walk_down(root) {
                let v = own_raw(&css[i], own);
                if v != RawV::Undef {
                    found = v;
                    break;
                }
            }
            found
        }
    };
    if value == RawV::Undef {
        value = default_raw(detect);
    }
    let stored = match &value {
        RawV::S(s) => s.clone(),
        RawV::B(b) => if *b { "\u{1}true".into() } else { "\u{1}false".into() },
        RawV::Undef => "\u{1}undef".into(),
    };
    css[root].raw_cache.get_or_insert_with(Default::default).push((key, stored));
    value
}

fn parse_cached(_key: &str, v: &str) -> RawV {
    match v {
        "\u{1}true" => RawV::B(true),
        "\u{1}false" => RawV::B(false),
        "\u{1}undef" => RawV::Undef,
        s => RawV::S(s.to_string()),
    }
}

fn strip_last_line(v: &str) -> String {
    // value.replace(/[^\n]+$/, '')
    if v.contains('\n') {
        match v.rfind('\n') {
            Some(i) => v[..=i].to_string(),
            None => v.to_string(),
        }
    } else {
        v.to_string()
    }
}

fn only_ws(v: &str) -> String {
    v.chars().filter(|c| c.is_whitespace_js()).collect()
}

fn before_after(css: &mut Css, node: NodeId, detect: &str) -> String {
    let kind = css[node].kind;
    let mut value = if kind == Kind::Decl {
        raw(css, node, None, "beforeDecl")
    } else if kind == Kind::Comment {
        raw(css, node, None, "beforeComment")
    } else if detect == "before" {
        raw(css, node, None, "beforeRule")
    } else {
        raw(css, node, None, "beforeClose")
    };
    let mut depth = 0;
    let mut buf = css[node].parent;
    while let Some(b) = buf {
        if css[b].kind == Kind::Root {
            break;
        }
        depth += 1;
        buf = css[b].parent;
    }
    if value.contains('\n') {
        let indent = raw(css, node, None, "indent");
        if !indent.is_empty() {
            for _ in 0..depth {
                value.push_str(&indent);
            }
        }
    }
    value
}

fn raw_before_close(css: &Css, root: NodeId) -> RawV {
    let mut value = None;
    for (i, _, _) in css.walk_down(root) {
        let n = &css[i];
        if n.nodes.as_ref().is_some_and(|l| !l.is_empty()) {
            if let Some(a) = &n.raws.after {
                value = Some(strip_last_line(a));
                break;
            }
        }
    }
    match value {
        Some(v) if !v.is_empty() => RawV::S(only_ws(&v)),
        Some(v) => RawV::S(v),
        None => RawV::Undef,
    }
}

fn raw_before_comment(css: &mut Css, root: NodeId, node: NodeId) -> RawV {
    let mut value = None;
    for (i, _, _) in css.walk_down(root) {
        let n = &css[i];
        if n.kind == Kind::Comment {
            if let Some(b) = &n.raws.before {
                value = Some(strip_last_line(b));
                break;
            }
        }
    }
    match value {
        None => RawV::S(raw(css, node, None, "beforeDecl")),
        Some(v) if !v.is_empty() => RawV::S(only_ws(&v)),
        Some(v) => RawV::S(v),
    }
}

fn raw_before_decl(css: &mut Css, root: NodeId, node: NodeId) -> RawV {
    let mut value = None;
    for (i, _, _) in css.walk_down(root) {
        let n = &css[i];
        if n.kind == Kind::Decl {
            if let Some(b) = &n.raws.before {
                value = Some(strip_last_line(b));
                break;
            }
        }
    }
    match value {
        None => RawV::S(raw(css, node, None, "beforeRule")),
        Some(v) if !v.is_empty() => RawV::S(only_ws(&v)),
        Some(v) => RawV::S(v),
    }
}

fn raw_before_open(css: &Css, root: NodeId) -> RawV {
    for (i, _, _) in css.walk_down(root) {
        let n = &css[i];
        if n.kind != Kind::Decl {
            if let Some(b) = &n.raws.between {
                return RawV::S(b.clone());
            }
        }
    }
    RawV::Undef
}

fn raw_before_rule(css: &Css, root: NodeId) -> RawV {
    let mut value = None;
    for (i, p, _) in css.walk_down(root) {
        let n = &css[i];
        if n.nodes.is_some() && (p != root || css.first(root) != Some(i)) {
            if let Some(b) = &n.raws.before {
                value = Some(strip_last_line(b));
                break;
            }
        }
    }
    match value {
        Some(v) if !v.is_empty() => RawV::S(only_ws(&v)),
        Some(v) => RawV::S(v),
        None => RawV::Undef,
    }
}

fn raw_colon(css: &Css, root: NodeId) -> RawV {
    for (i, _, _) in css.walk_down(root) {
        let n = &css[i];
        if n.kind == Kind::Decl {
            if let Some(b) = &n.raws.between {
                return RawV::S(b.chars().filter(|c| c.is_whitespace_js() || *c == ':').collect());
            }
        }
    }
    RawV::Undef
}

fn raw_empty_body(css: &Css, root: NodeId) -> RawV {
    for (i, _, _) in css.walk_down(root) {
        let n = &css[i];
        if n.nodes.as_ref().is_some_and(|l| l.is_empty()) {
            if let Some(a) = &n.raws.after {
                return RawV::S(a.clone());
            }
        }
    }
    RawV::Undef
}

fn raw_indent(css: &Css, root: NodeId) -> RawV {
    if let Some(i) = &css[root].raws.more().indent {
        if !i.is_empty() {
            return RawV::S(i.clone());
        }
    }
    // parents as the walk reaches them (a spliced root lends nodes whose pointers differ)
    for (i, p, pp) in css.walk_down(root) {
        let n = &css[i];
        if p != root && pp == Some(root) {
            if let Some(b) = &n.raws.before {
                let last = b.split('\n').last().unwrap_or("");
                return RawV::S(only_ws(last));
            }
        }
    }
    RawV::Undef
}

fn raw_semicolon(css: &Css, root: NodeId) -> RawV {
    for (i, _, _) in css.walk_down(root) {
        let n = &css[i];
        if let Some(list) = &n.nodes {
            if let Some(&last) = list.last() {
                if css[last].kind == Kind::Decl {
                    if let Some(s) = n.raws.semicolon {
                        return RawV::B(s);
                    }
                }
            }
        }
    }
    RawV::Undef
}

fn raw_value<'a>(n: &'a Node, prop: &str) -> &'a str {
    let (value, raw) = match prop {
        "selector" => (&n.selector, &n.raws.more().selector),
        "params" => (&n.params, &n.raws.more().params),
        _ => (&n.value, &n.raws.more().value),
    };
    if let Some(r) = raw {
        if &r.value == value {
            return &r.raw;
        }
    }
    value
}

struct Stringifier<'a> {
    css: &'a mut Css,
    out: String,
}

/// `node.toString()`
pub fn to_string(css: &mut Css, node: NodeId) -> String {
    let mut s = Stringifier { css, out: String::new() };
    s.stringify(node, false);
    s.out
}

impl Stringifier<'_> {
    fn stringify(&mut self, node: NodeId, semicolon: bool) {
        match self.css[node].kind {
            Kind::Root => self.root(node),
            Kind::Rule => self.rule(node),
            Kind::AtRule => self.atrule(node, semicolon),
            Kind::Decl => self.decl(node, semicolon),
            Kind::Comment => self.comment(node),
        }
    }

    fn root(&mut self, node: NodeId) {
        self.body(node);
        if let Some(a) = &self.css[node].raws.after {
            if !a.is_empty() {
                let a = a.clone();
                self.out.push_str(&a);
            }
        }
    }

    fn rule(&mut self, node: NodeId) {
        let sel = raw_value(&self.css[node], "selector").to_string();
        self.block(node, &sel);
        if let Some(s) = &self.css[node].raws.more().own_semicolon {
            if !s.is_empty() {
                let s = s.clone();
                self.out.push_str(&s);
            }
        }
    }

    fn atrule(&mut self, node: NodeId, semicolon: bool) {
        let n = &self.css[node];
        let mut name = format!("@{}", n.name);
        let params = if n.params.is_empty() { String::new() } else { raw_value(n, "params").to_string() };
        if let Some(a) = &n.raws.more().after_name {
            name.push_str(a);
        } else if !params.is_empty() {
            name.push(' ');
        }
        if n.nodes.is_some() {
            self.block(node, &(name + &params));
        } else {
            let end = n.raws.between.clone().unwrap_or_default() + if semicolon { ";" } else { "" };
            self.out.push_str(&name);
            self.out.push_str(&params);
            self.out.push_str(&end);
        }
    }

    fn decl(&mut self, node: NodeId, semicolon: bool) {
        let between = raw(self.css, node, Some("between"), "colon");
        let n = &self.css[node];
        let mut string = format!("{}{}{}", n.prop, between, raw_value(n, "value"));
        if n.important {
            match &n.raws.more().important {
                Some(i) if !i.is_empty() => string.push_str(i),
                _ => string.push_str(" !important"),
            }
        }
        if semicolon {
            string.push(';');
        }
        self.out.push_str(&string);
    }

    fn comment(&mut self, node: NodeId) {
        let left = raw(self.css, node, Some("left"), "commentLeft");
        let right = raw(self.css, node, Some("right"), "commentRight");
        let text = &self.css[node].text;
        self.out.push_str("/*");
        self.out.push_str(&left);
        self.out.push_str(text);
        self.out.push_str(&right);
        self.out.push_str("*/");
    }

    fn block(&mut self, node: NodeId, start: &str) {
        let between = raw(self.css, node, Some("between"), "beforeOpen");
        self.out.push_str(start);
        self.out.push_str(&between);
        self.out.push('{');
        let after;
        if self.css[node].nodes.as_ref().is_some_and(|l| !l.is_empty()) {
            self.body(node);
            after = raw(self.css, node, Some("after"), "after");
        } else {
            after = raw(self.css, node, Some("after"), "emptyBody");
        }
        if !after.is_empty() {
            self.out.push_str(&after);
        }
        self.out.push('}');
    }

    fn body(&mut self, node: NodeId) {
        let list = self.css.children(node).to_vec();
        let mut last = list.len() as isize - 1;
        while last > 0 {
            if self.css[list[last as usize]].kind != Kind::Comment {
                break;
            }
            last -= 1;
        }
        let semicolon = matches!(raw_v(self.css, node, Some("semicolon"), Some("semicolon")), RawV::B(true));
        for (i, &child) in list.iter().enumerate() {
            let before = raw(self.css, child, Some("before"), "before");
            if !before.is_empty() {
                self.out.push_str(&before);
            }
            self.stringify(child, last != i as isize || semicolon);
        }
    }
}
