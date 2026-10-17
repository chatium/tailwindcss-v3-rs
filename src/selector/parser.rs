//! `postcss-selector-parser/dist/parser.js` (non-lossy mode, string input)

use super::tokenize::{tokenize, Tk, Token};
use super::*;

/// `parser().astSync(selector)`: parses into `sel` and returns the root.
pub fn parse(sel: &mut Sel, css: &str) -> Result<SId, SelError> {
    let tokens = tokenize(css)?;
    let root = sel.root();
    let selector = sel.selector();
    sel.append(root, selector);
    let mut p = Parser { sel, css, tokens, position: 0, root, current: selector, spaces: String::new() };
    p.run()?;
    Ok(root)
}

struct Parser<'a> {
    sel: &'a mut Sel,
    css: &'a str,
    tokens: Vec<Token>,
    position: usize,
    root: SId,
    current: SId,
    spaces: String,
}

fn is_ws(t: Tk) -> bool {
    t == Tk::Space
}

fn err(msg: impl Into<String>) -> SelError {
    SelError(msg.into())
}

/// Wrapper over the attribute-in-progress (`node` object in `attribute()`).
#[derive(Default)]
struct AttrBuild {
    attribute: Option<String>,
    namespace: Option<Ns>,
    operator: Option<String>,
    value: Option<String>,
    quote_mark: Option<Option<char>>,
    insensitive: Option<bool>,
    raws: SRaws,
    has_raws: bool,
    spaces: Vec<(&'static str, PartSpaces)>,
}

fn part_entry<'a>(list: &'a mut Vec<(&'static str, PartSpaces)>, name: &'static str) -> &'a mut PartSpaces {
    if let Some(i) = list.iter().position(|(k, _)| *k == name) {
        &mut list[i].1
    } else {
        list.push((name, PartSpaces::default()));
        &mut list.last_mut().unwrap().1
    }
}

fn part_get<'a>(list: &'a [(&'static str, PartSpaces)], name: &str) -> Option<&'a PartSpaces> {
    list.iter().find(|(k, _)| *k == name).map(|(_, v)| v)
}

impl AttrBuild {
    fn prop_str(&self, name: &str) -> Option<String> {
        match name {
            "attribute" => self.attribute.clone(),
            "operator" => self.operator.clone(),
            "value" => self.value.clone(),
            "namespace" => match &self.namespace {
                Some(Ns::Str(s)) => Some(s.clone()),
                Some(Ns::True) => Some("true".into()),
                None => None,
            },
            _ => None,
        }
    }
    fn raw_prop(&self, name: &str) -> Option<String> {
        match name {
            "attribute" => self.raws.attribute.clone(),
            "operator" => self.raws.operator.clone(),
            "value" => self.raws.value.clone(),
            "namespace" => self.raws.namespace.clone(),
            "insensitive" => self.raws.insensitive_flag.clone(),
            _ => None,
        }
    }
    fn set_raw_prop(&mut self, name: &str, v: String) {
        self.has_raws = true;
        match name {
            "attribute" => self.raws.attribute = Some(v),
            "operator" => self.raws.operator = Some(v),
            "value" => self.raws.value = Some(v),
            "namespace" => self.raws.namespace = Some(v),
            _ => {}
        }
    }
}

fn tk_static(name: &str) -> &'static str {
    match name {
        "attribute" => "attribute",
        "operator" => "operator",
        "value" => "value",
        "namespace" => "namespace",
        "insensitive" => "insensitive",
        _ => "other",
    }
}

impl<'a> Parser<'a> {
    fn cur(&self) -> Option<Token> {
        self.tokens.get(self.position).copied()
    }
    fn next_tok(&self) -> Option<Token> {
        self.tokens.get(self.position + 1).copied()
    }
    fn prev_tok(&self) -> Option<Token> {
        if self.position == 0 { None } else { self.tokens.get(self.position - 1).copied() }
    }
    fn content_of(&self, t: Token) -> &'a str {
        &self.css[t.start..t.end.min(self.css.len())]
    }
    fn content(&self) -> &'a str {
        self.content_of(self.cur().unwrap())
    }

    fn run(&mut self) -> Result<(), SelError> {
        while self.position < self.tokens.len() {
            self.parse(true)?;
        }
        Ok(())
    }

    fn parse(&mut self, throw_on_parenthesis: bool) -> Result<(), SelError> {
        let t = self.cur().unwrap();
        match t.kind {
            Tk::Space => self.space(),
            Tk::Comment => self.comment(),
            Tk::OpenParen => self.parentheses(),
            Tk::CloseParen => {
                if throw_on_parenthesis {
                    return self.expected("opening parenthesis", None);
                }
                Ok(())
            }
            Tk::OpenSquare => self.attribute(),
            Tk::Dollar | Tk::Caret | Tk::Equals | Tk::Word => self.word(None),
            Tk::Colon => self.pseudo(),
            Tk::Comma => self.comma(),
            Tk::Asterisk => self.universal(None),
            Tk::Ampersand => self.nesting(),
            Tk::Slash | Tk::Combinator => self.combinator(),
            Tk::Str => self.string(),
            Tk::CloseSquare => self.expected("opening square bracket", None),
            Tk::Semicolon => Err(err("Expected a backslash preceding the semicolon.")),
            Tk::Bang => Err(self.unexpected()),
        }
    }

    fn unexpected(&self) -> SelError {
        err(format!("Unexpected '{}'. Escaping special characters with \\ may help.", self.content()))
    }

    fn expected(&self, description: &str, found: Option<&str>) -> Result<(), SelError> {
        let an = if description.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
        match found {
            None => Err(err(format!("Expected {an} {description}."))),
            Some(f) => Err(err(format!("Expected {an} {description}, found \"{f}\" instead."))),
        }
    }

    fn new_node(&mut self, node: SId, namespace: Option<Ns>) {
        if let Some(mut ns) = namespace {
            if let Ns::Str(s) = &ns {
                if !s.is_empty() && s.bytes().all(|c| c == b' ') {
                    self.spaces = std::mem::take(&mut self.spaces) + s;
                    ns = Ns::True;
                }
            }
            self.sel.set_namespace(node, ns.clone());
            // unescapeProp(node, "namespace")
            if let Ns::Str(s) = &ns {
                if s.contains('\\') {
                    self.sel.set_namespace(node, Ns::Str(unesc(s)));
                    let r = self.sel[node].raws.get_or_insert_with(Default::default);
                    if r.namespace.is_none() {
                        r.namespace = Some(s.clone());
                    }
                }
            }
        }
        if !self.spaces.is_empty() {
            self.sel[node].spaces_before = std::mem::take(&mut self.spaces);
        }
        self.sel.append(self.current, node);
    }

    fn attribute(&mut self) -> Result<(), SelError> {
        let mut attr: Vec<Token> = Vec::new();
        self.position += 1;
        while self.position < self.tokens.len() && self.cur().unwrap().kind != Tk::CloseSquare {
            attr.push(self.cur().unwrap());
            self.position += 1;
        }
        match self.cur() {
            None => return Err(err("Cannot read properties of undefined (reading '0')")),
            Some(t) if t.kind != Tk::CloseSquare => return self.expected("closing square bracket", None),
            _ => {}
        }
        let len = attr.len();
        let mut node = AttrBuild::default();
        if len == 1 && attr[0].kind != Tk::Word {
            return self.expected("attribute", None);
        }
        let mut pos = 0;
        let mut space_before = String::new();
        let mut comment_before = String::new();
        let mut last_added: Option<&'static str> = None;
        let mut space_after_meaningful = false;
        while pos < len {
            let token = attr[pos];
            let content = self.content_of(token).to_string();
            let next = attr.get(pos + 1).copied();
            match token.kind {
                Tk::Space => {
                    space_after_meaningful = true;
                    if let Some(la) = last_added {
                        let e = part_entry(&mut node.spaces, la);
                        let prev = e.after.clone().unwrap_or_default();
                        e.after = Some(prev + &content);
                        let existing = part_get(&node.raws.part_spaces, la).and_then(|p| p.after.clone()).filter(|s| !s.is_empty());
                        if let Some(existing) = existing {
                            part_entry(&mut node.raws.part_spaces, la).after = Some(existing + &content);
                        }
                    } else {
                        space_before.push_str(&content);
                        comment_before.push_str(&content);
                    }
                }
                Tk::Asterisk => {
                    if next.map(|t| t.kind) == Some(Tk::Equals) {
                        node.operator = Some(content.clone());
                        last_added = Some("operator");
                    } else if (node.namespace.is_none() || (last_added == Some("namespace") && !space_after_meaningful)) && next.is_some() {
                        if !space_before.is_empty() {
                            part_entry(&mut node.spaces, "attribute").before = Some(std::mem::take(&mut space_before));
                        }
                        if !comment_before.is_empty() {
                            node.has_raws = true;
                            part_entry(&mut node.raws.part_spaces, "attribute").before = Some(space_before.clone());
                            comment_before.clear();
                        }
                        let old = match &node.namespace {
                            Some(Ns::Str(s)) => s.clone(),
                            _ => String::new(),
                        };
                        node.namespace = Some(Ns::Str(old + &content));
                        if let Some(r) = node.raws.namespace.as_mut() {
                            if !r.is_empty() {
                                r.push_str(&content);
                            }
                        }
                        last_added = Some("namespace");
                    }
                    space_after_meaningful = false;
                }
                Tk::Dollar | Tk::Caret => {
                    let mut handled = false;
                    if token.kind == Tk::Dollar && last_added == Some("value") {
                        let old_raw = node.raws.value.clone();
                        node.value = Some(node.value.clone().unwrap_or_else(|| "undefined".into()) + "$");
                        if let Some(o) = old_raw.filter(|s| !s.is_empty()) {
                            node.raws.value = Some(o + "$");
                        }
                        handled = true;
                    }
                    if !handled {
                        if next.map(|t| t.kind) == Some(Tk::Equals) {
                            node.operator = Some(content.clone());
                            last_added = Some("operator");
                        }
                        space_after_meaningful = false;
                    }
                }
                Tk::Combinator => {
                    if content == "~" && next.map(|t| t.kind) == Some(Tk::Equals) {
                        node.operator = Some(content.clone());
                        last_added = Some("operator");
                    }
                    if content != "|" {
                        space_after_meaningful = false;
                    } else {
                        if next.map(|t| t.kind) == Some(Tk::Equals) {
                            node.operator = Some(content.clone());
                            last_added = Some("operator");
                        } else if node.namespace.is_none() && node.attribute.is_none() {
                            node.namespace = Some(Ns::True);
                        }
                        space_after_meaningful = false;
                    }
                }
                Tk::Word => {
                    let ns_empty = match &node.namespace {
                        None => true,
                        Some(Ns::Str(s)) => s.is_empty(),
                        Some(Ns::True) => false,
                    };
                    if next.is_some_and(|n| self.content_of(n) == "|")
                        && attr.get(pos + 2).is_some_and(|t| t.kind != Tk::Equals)
                        && node.operator.as_deref().is_none_or(|o| o.is_empty())
                        && ns_empty
                    {
                        node.namespace = Some(Ns::Str(content.clone()));
                        last_added = Some("namespace");
                    } else if node.attribute.as_deref().is_none_or(|a| a.is_empty())
                        || (last_added == Some("attribute") && !space_after_meaningful)
                    {
                        if !space_before.is_empty() {
                            part_entry(&mut node.spaces, "attribute").before = Some(std::mem::take(&mut space_before));
                        }
                        if !comment_before.is_empty() {
                            node.has_raws = true;
                            part_entry(&mut node.raws.part_spaces, "attribute").before = Some(std::mem::take(&mut comment_before));
                        }
                        node.attribute = Some(node.attribute.clone().unwrap_or_default() + &content);
                        if let Some(r) = node.raws.attribute.as_mut() {
                            if !r.is_empty() {
                                r.push_str(&content);
                            }
                        }
                        last_added = Some("attribute");
                    } else if (node.value.as_deref().is_none_or(|v| v.is_empty()) && node.value.as_deref() != Some(""))
                        || (last_added == Some("value") && !(space_after_meaningful || matches!(node.quote_mark, Some(Some(_)))))
                    {
                        let unescaped = unesc(&content);
                        let old_raw = node.raws.value.clone().unwrap_or_default();
                        let old_value = node.value.clone().unwrap_or_default();
                        node.value = Some(old_value.clone() + &unescaped);
                        node.quote_mark = Some(None);
                        if unescaped != content || !old_raw.is_empty() {
                            node.has_raws = true;
                            let base = if old_raw.is_empty() { old_value } else { old_raw };
                            node.raws.value = Some(base + &content);
                        }
                        last_added = Some("value");
                    } else {
                        let insensitive = content == "i" || content == "I";
                        let has_value = node.value.is_some();
                        if has_value && (matches!(node.quote_mark, Some(Some(_))) || space_after_meaningful) {
                            node.insensitive = Some(insensitive);
                            if !insensitive || content == "I" {
                                node.has_raws = true;
                                node.raws.insensitive_flag = Some(content.clone());
                            }
                            last_added = Some("insensitive");
                            if !space_before.is_empty() {
                                part_entry(&mut node.spaces, "insensitive").before = Some(std::mem::take(&mut space_before));
                            }
                            if !comment_before.is_empty() {
                                node.has_raws = true;
                                part_entry(&mut node.raws.part_spaces, "insensitive").before = Some(std::mem::take(&mut comment_before));
                            }
                        } else if has_value {
                            last_added = Some("value");
                            node.value = Some(node.value.clone().unwrap() + &content);
                            if let Some(r) = node.raws.value.as_mut() {
                                if !r.is_empty() {
                                    r.push_str(&content);
                                }
                            }
                        }
                    }
                    space_after_meaningful = false;
                }
                Tk::Str => {
                    if node.attribute.as_deref().is_none_or(|a| a.is_empty()) || node.operator.as_deref().is_none_or(|o| o.is_empty()) {
                        return Err(err("Expected an attribute followed by an operator preceding the string."));
                    }
                    let (unescaped, quote_mark) = unescape_value(&content);
                    node.value = Some(unescaped);
                    node.quote_mark = Some(quote_mark);
                    last_added = Some("value");
                    node.has_raws = true;
                    node.raws.value = Some(content.clone());
                    space_after_meaningful = false;
                }
                Tk::Equals => {
                    if node.attribute.as_deref().is_none_or(|a| a.is_empty()) {
                        return self.expected("attribute", Some(&content));
                    }
                    if node.value.as_deref().is_some_and(|v| !v.is_empty()) {
                        return Err(err("Unexpected \"=\" found; an operator was already defined."));
                    }
                    node.operator = Some(match &node.operator {
                        Some(o) if !o.is_empty() => format!("{o}{content}"),
                        _ => content.clone(),
                    });
                    last_added = Some("operator");
                    space_after_meaningful = false;
                }
                Tk::Comment => {
                    if let Some(la) = last_added {
                        if space_after_meaningful || next.map(|t| t.kind) == Some(Tk::Space) || la == "insensitive" {
                            let last_comment = part_get(&node.spaces, la).and_then(|p| p.after.clone()).filter(|s| !s.is_empty()).unwrap_or_default();
                            let raw_last = part_get(&node.raws.part_spaces, la)
                                .and_then(|p| p.after.clone())
                                .filter(|s| !s.is_empty())
                                .unwrap_or(last_comment);
                            node.has_raws = true;
                            part_entry(&mut node.raws.part_spaces, la).after = Some(raw_last + &content);
                        } else {
                            let last_value = node.prop_str(la).filter(|s| !s.is_empty()).unwrap_or_default();
                            let raw_last = node.raw_prop(la).filter(|s| !s.is_empty()).unwrap_or(last_value);
                            node.set_raw_prop(tk_static(la), raw_last + &content);
                        }
                    } else {
                        comment_before.push_str(&content);
                    }
                }
                _ => {
                    return Err(err(format!("Unexpected \"{content}\" found.")));
                }
            }
            pos += 1;
        }
        // unescapeProp(node, "attribute"); unescapeProp(node, "namespace")
        if let Some(a) = node.attribute.clone() {
            if a.contains('\\') {
                node.attribute = Some(unesc(&a));
                node.has_raws = true;
                if node.raws.attribute.is_none() {
                    node.raws.attribute = Some(a);
                }
            }
        }
        if let Some(Ns::Str(n)) = node.namespace.clone() {
            if n.contains('\\') {
                node.namespace = Some(Ns::Str(unesc(&n)));
                node.has_raws = true;
                if node.raws.namespace.is_none() {
                    node.raws.namespace = Some(n);
                }
            }
        }
        // new Attribute(node): handleDeprecatedContructorOpts — quoteMark always set when value is
        let mut n = SNode::new(SKind::Attribute);
        n.attribute = node.attribute;
        n.namespace = node.namespace;
        n.operator = node.operator;
        n.value = node.value.clone();
        n.insensitive = node.insensitive;
        n.part_spaces = node.spaces;
        if node.quote_mark.is_none() && node.value.is_some() {
            // deprecated constructor path: value is unescaped, raws.value keeps the original
            let v = node.value.clone().unwrap();
            let (unescaped, qm) = unescape_value(&v);
            if node.raws.value.is_none() {
                node.raws.value = Some(v);
            }
            node.has_raws = true;
            n.value = Some(unescaped);
            n.quote_mark = Some(qm);
        } else {
            n.quote_mark = node.quote_mark;
        }
        n.raws = Some(node.raws);
        n.constructed = true;
        let id = self.sel.add(n);
        self.new_node(id, None);
        self.position += 1;
        Ok(())
    }

    fn parse_whitespace_equivalent_tokens(&mut self, stop: isize) -> Vec<SId> {
        let stop = if stop < 0 { self.tokens.len() } else { stop as usize };
        let start_position = self.position;
        let mut nodes = Vec::new();
        let mut space = String::new();
        let mut last_comment: Option<SId> = None;
        loop {
            let t = self.cur().unwrap();
            if is_ws(t.kind) {
                space.push_str(self.content());
            } else if t.kind == Tk::Comment {
                let mut c = SNode::new(SKind::Comment);
                c.value = Some(self.content().to_string());
                if !space.is_empty() {
                    c.spaces_before = std::mem::take(&mut space);
                }
                let id = self.sel.add(c);
                last_comment = Some(id);
                nodes.push(id);
            }
            self.position += 1;
            if self.position >= stop {
                break;
            }
        }
        if !space.is_empty() {
            if let Some(c) = last_comment {
                self.sel[c].spaces_after = space;
            } else {
                let _ = start_position;
                let mut s = SNode::new(SKind::Str);
                s.value = Some(String::new());
                s.spaces_before = space;
                s.spaces_after = String::new();
                nodes.push(self.sel.add(s));
            }
        }
        nodes
    }

    fn raw_space_after_of(&self, id: SId) -> String {
        let n = &self.sel[id];
        n.raws.as_ref().and_then(|r| r.spaces_after.clone()).unwrap_or_else(|| n.spaces_after.clone())
    }

    /// returns (space, rawSpace)
    fn convert_whitespace_nodes_to_space(&self, nodes: &[SId]) -> (String, Option<String>) {
        let mut space = String::new();
        let mut raw_space = String::new();
        for &n in nodes {
            let node = &self.sel[n];
            let space_before = node.spaces_before.clone();
            space.push_str(&space_before);
            space.push_str(&node.spaces_after);
            raw_space.push_str(&space_before);
            raw_space.push_str(node.val());
            raw_space.push_str(&self.raw_space_after_of(n));
        }
        if raw_space == space { (space, None) } else { (space, Some(raw_space)) }
    }

    fn is_named_combinator(&self, position: usize) -> bool {
        let k = |i: usize| self.tokens.get(i).map(|t| t.kind);
        k(position) == Some(Tk::Slash) && k(position + 1) == Some(Tk::Word) && k(position + 2) == Some(Tk::Slash)
    }

    fn named_combinator(&mut self) -> Result<SId, SelError> {
        if self.is_named_combinator(self.position) {
            let name_raw = self.content_of(self.tokens[self.position + 1]).to_string();
            let name = unesc(&name_raw).to_lowercase();
            let mut n = SNode::new(SKind::Combinator);
            n.value = Some(format!("/{name}/"));
            if name != name_raw {
                n.raws_mut().value = Some(format!("/{name_raw}/"));
            } else {
                n.raws = Some(SRaws::default());
            }
            self.position += 3;
            Ok(self.sel.add(n))
        } else {
            Err(self.unexpected())
        }
    }

    fn locate_next_meaningful_token(&self, start: usize) -> isize {
        let mut s = start;
        while s < self.tokens.len() {
            let k = self.tokens[s].kind;
            if k == Tk::Space || k == Tk::Comment {
                s += 1;
                continue;
            }
            return s as isize;
        }
        -1
    }

    fn combinator(&mut self) -> Result<(), SelError> {
        if self.content() == "|" {
            return self.namespace();
        }
        let next_sig = self.locate_next_meaningful_token(self.position);
        if next_sig < 0 || matches!(self.tokens[next_sig as usize].kind, Tk::Comma | Tk::CloseParen) {
            let nodes = self.parse_whitespace_equivalent_tokens(next_sig);
            if !nodes.is_empty() {
                if let Some(last) = self.sel.last(self.current) {
                    let (space, raw_space) = self.convert_whitespace_nodes_to_space(&nodes);
                    if let Some(raw_space) = raw_space {
                        let cur = self.raw_space_after_of(last);
                        self.sel[last].raws_mut().spaces_after = Some(cur + &raw_space);
                    }
                    self.sel[last].spaces_after.push_str(&space);
                } else {
                    for n in nodes {
                        self.new_node(n, None);
                    }
                }
            }
            return Ok(());
        }
        let first_token_pos = self.position;
        let mut space_or_desc: Option<Vec<SId>> = None;
        if next_sig > self.position as isize {
            space_or_desc = Some(self.parse_whitespace_equivalent_tokens(next_sig));
        }
        let _ = first_token_pos;
        let node: SId;
        if self.is_named_combinator(self.position) {
            node = self.named_combinator()?;
        } else if self.cur().map(|t| t.kind) == Some(Tk::Combinator) {
            let mut n = SNode::new(SKind::Combinator);
            n.value = Some(self.content().to_string());
            node = self.sel.add(n);
            self.position += 1;
        } else if self.cur().is_some_and(|t| is_ws(t.kind)) {
            // pass
            node = self.descendant(space_or_desc.take().unwrap_or_default());
            return self.finish_combinator(node);
        } else if space_or_desc.is_none() {
            return Err(self.unexpected());
        } else {
            node = self.descendant(space_or_desc.take().unwrap());
            return self.finish_combinator(node);
        }
        if let Some(nodes) = space_or_desc {
            let (space, raw_space) = self.convert_whitespace_nodes_to_space(&nodes);
            self.sel[node].spaces_before = space;
            match raw_space {
                Some(r) => self.sel[node].raws_mut().spaces_before = Some(r),
                None => {
                    // `node.rawSpaceBefore = undefined` creates raws.spaces with before: undefined
                    self.sel[node].raws_mut().spaces_before = None;
                }
            }
        }
        self.finish_combinator(node)
    }

    fn descendant(&mut self, nodes: Vec<SId>) -> SId {
        // convertWhitespaceNodesToSpace(nodes, true) — requiredSpace only matters in lossy mode
        let (space, raw_space) = self.convert_whitespace_nodes_to_space(&nodes);
        let raw_space = match raw_space {
            Some(r) if !r.is_empty() => r,
            _ => space.clone(),
        };
        let mut n = SNode::new(SKind::Combinator);
        n.value = Some(" ".into());
        let mut raws = SRaws::default();
        if space.ends_with(' ') && raw_space.ends_with(' ') {
            n.spaces_before = space[..space.len() - 1].to_string();
            raws.spaces_before = Some(raw_space[..raw_space.len() - 1].to_string());
        } else if space.starts_with(' ') && raw_space.starts_with(' ') {
            n.spaces_after = space[1..].to_string();
            raws.spaces_after = Some(raw_space[1..].to_string());
        } else {
            raws.value = Some(raw_space);
        }
        n.raws = Some(raws);
        self.sel.add(n)
    }

    fn finish_combinator(&mut self, node: SId) -> Result<(), SelError> {
        if self.cur().is_some_and(|t| t.kind == Tk::Space) {
            self.sel[node].spaces_after = self.content().to_string();
            self.position += 1;
        }
        self.new_node(node, None);
        Ok(())
    }

    fn comma(&mut self) -> Result<(), SelError> {
        if self.position == self.tokens.len() - 1 {
            self.sel[self.root].trailing_comma = true;
            self.position += 1;
            return Ok(());
        }
        let selector = self.sel.selector();
        let parent = self.sel[self.current].parent.unwrap();
        self.sel.append(parent, selector);
        self.current = selector;
        self.position += 1;
        Ok(())
    }

    fn comment(&mut self) -> Result<(), SelError> {
        let mut n = SNode::new(SKind::Comment);
        n.value = Some(self.content().to_string());
        let id = self.sel.add(n);
        self.new_node(id, None);
        self.position += 1;
        Ok(())
    }

    fn namespace(&mut self) -> Result<(), SelError> {
        let before = match self.prev_tok() {
            Some(t) => {
                let c = self.content_of(t);
                if c.is_empty() { Ns::True } else { Ns::Str(c.to_string()) }
            }
            None => Ns::True,
        };
        match self.next_tok().map(|t| t.kind) {
            Some(Tk::Word) => {
                self.position += 1;
                self.word(Some(before))
            }
            Some(Tk::Asterisk) => {
                self.position += 1;
                self.universal(Some(before))
            }
            None => Err(err("Cannot read properties of undefined (reading '0')")),
            _ => Err(err("Unexpected '|'.")),
        }
    }

    fn nesting(&mut self) -> Result<(), SelError> {
        if let Some(n) = self.next_tok() {
            if self.content_of(n) == "|" {
                self.position += 1;
                return Ok(());
            }
        }
        let id = self.sel.add(SNode::new(SKind::Nesting));
        self.new_node(id, None);
        self.position += 1;
        Ok(())
    }

    fn parentheses(&mut self) -> Result<(), SelError> {
        let last = self.sel.last(self.current);
        let mut unbalanced = 1;
        self.position += 1;
        if let Some(last) = last.filter(|l| self.sel[*l].kind == SKind::Pseudo) {
            let selector = self.sel.selector();
            let cache = self.current;
            self.sel.append(last, selector);
            self.current = selector;
            while self.position < self.tokens.len() && unbalanced > 0 {
                let k = self.cur().unwrap().kind;
                if k == Tk::OpenParen {
                    unbalanced += 1;
                }
                if k == Tk::CloseParen {
                    unbalanced -= 1;
                }
                if unbalanced > 0 {
                    self.parse(false)?;
                } else {
                    self.position += 1;
                }
            }
            self.current = cache;
        } else {
            let mut paren_value = String::from("(");
            if last.is_none() && self.position >= self.tokens.len() {
                return Err(err("Cannot read properties of undefined (reading '1')"));
            }
            while self.position < self.tokens.len() && unbalanced > 0 {
                let t = self.cur().unwrap();
                if t.kind == Tk::OpenParen {
                    unbalanced += 1;
                }
                if t.kind == Tk::CloseParen {
                    unbalanced -= 1;
                }
                paren_value.push_str(self.content_of(t));
                self.position += 1;
            }
            if let Some(last) = last {
                // appendToPropertyAndEscape("value", parenValue, parenValue)
                let n = &mut self.sel[last];
                let original_value = n.value.clone().unwrap_or_else(|| "undefined".into());
                let original_escaped = n.raws.as_ref().and_then(|r| r.value.clone()).filter(|s| !s.is_empty());
                if n.kind == SKind::Class {
                    self.sel.set_class_value(last, &(original_value.clone() + &paren_value));
                } else {
                    self.sel[last].value = Some(original_value.clone() + &paren_value);
                }
                let n = &mut self.sel[last];
                if let Some(oe) = original_escaped {
                    n.raws_mut().value = Some(oe + &paren_value);
                } else if let Some(r) = n.raws.as_mut() {
                    r.value = None;
                }
            } else {
                let mut s = SNode::new(SKind::Str);
                s.value = Some(paren_value);
                let id = self.sel.add(s);
                self.new_node(id, None);
            }
        }
        if unbalanced > 0 {
            if self.cur().is_none() {
                return Err(err("Cannot read properties of undefined (reading '5')"));
            }
            return self.expected("closing parenthesis", None);
        }
        Ok(())
    }

    fn pseudo(&mut self) -> Result<(), SelError> {
        let mut pseudo_str = String::new();
        while self.cur().is_some_and(|t| t.kind == Tk::Colon) {
            pseudo_str.push_str(self.content());
            self.position += 1;
        }
        let Some(t) = self.cur() else {
            return self.expected("pseudo-class or pseudo-element", None);
        };
        if t.kind == Tk::Word {
            self.split_word(None, Some(pseudo_str))
        } else {
            self.expected("pseudo-class or pseudo-element", None)
        }
    }

    fn space(&mut self) -> Result<(), SelError> {
        let content = self.content().to_string();
        let prev_kind = self.prev_tok().map(|t| t.kind);
        let all_comments = self.sel.children(self.current).iter().all(|n| self.sel[*n].kind == SKind::Comment);
        if self.position == 0 || prev_kind == Some(Tk::Comma) || prev_kind == Some(Tk::OpenParen) || all_comments {
            self.spaces = content;
            self.position += 1;
        } else if self.position == self.tokens.len() - 1
            || matches!(self.next_tok().map(|t| t.kind), Some(Tk::Comma | Tk::CloseParen))
        {
            let last = self.sel.last(self.current).ok_or_else(|| err("Cannot read properties of undefined (reading 'spaces')"))?;
            self.sel[last].spaces_after = content;
            self.position += 1;
        } else {
            return self.combinator();
        }
        Ok(())
    }

    fn string(&mut self) -> Result<(), SelError> {
        let mut n = SNode::new(SKind::Str);
        n.value = Some(self.content().to_string());
        let id = self.sel.add(n);
        self.new_node(id, None);
        self.position += 1;
        Ok(())
    }

    fn universal(&mut self, namespace: Option<Ns>) -> Result<(), SelError> {
        if let Some(n) = self.next_tok() {
            if self.content_of(n) == "|" {
                self.position += 1;
                return self.namespace();
            }
        }
        let id = self.sel.add(SNode::new(SKind::Universal));
        self.new_node(id, namespace);
        self.position += 1;
        Ok(())
    }

    fn word(&mut self, namespace: Option<Ns>) -> Result<(), SelError> {
        if let Some(n) = self.next_tok() {
            if self.content_of(n) == "|" {
                self.position += 1;
                return self.namespace();
            }
        }
        self.split_word(namespace, None)
    }

    /// `splitWord(namespace, firstCallback)`; `pseudo` carries the pseudo prefix when
    /// called from `pseudo()` (the first callback).
    fn split_word(&mut self, mut namespace: Option<Ns>, pseudo: Option<String>) -> Result<(), SelError> {
        let mut next = self.next_tok();
        let mut word = self.content().to_string();
        while let Some(n) = next {
            if !matches!(n.kind, Tk::Dollar | Tk::Caret | Tk::Equals | Tk::Word) {
                break;
            }
            self.position += 1;
            let current = self.content().to_string();
            word.push_str(&current);
            if current.ends_with('\\') {
                if let Some(nt) = self.next_tok() {
                    if nt.kind == Tk::Space {
                        word.push_str(self.content_of(nt));
                        self.position += 1;
                    }
                }
            }
            next = self.next_tok();
        }
        let wb = word.as_bytes();
        let is_keyframes_percent = is_keyframes_percent(&word);
        let has_class: Vec<usize> = (0..wb.len())
            .filter(|&i| wb[i] == b'.' && !(i > 0 && wb[i - 1] == b'\\') && !is_keyframes_percent)
            .collect();
        let mut has_id: Vec<usize> = (0..wb.len()).filter(|&i| wb[i] == b'#' && !(i > 0 && wb[i - 1] == b'\\')).collect();
        let interpolations: Vec<usize> = (0..wb.len()).filter(|&i| wb[i] == b'#' && wb.get(i + 1) == Some(&b'{')).collect();
        if !interpolations.is_empty() {
            has_id.retain(|h| !interpolations.contains(h));
        }
        let mut indices: Vec<usize> = vec![0];
        for &i in has_class.iter().chain(has_id.iter()) {
            if !indices.contains(&i) {
                indices.push(i);
            }
        }
        indices.sort();
        let n_idx = indices.len();
        for (i, &ind) in indices.iter().enumerate() {
            let index = match indices.get(i + 1) {
                Some(&x) if x != 0 => x,
                _ => word.len(),
            };
            let value = &word[ind..index];
            if i == 0 {
                if let Some(pseudo_str) = &pseudo {
                    let mut n = SNode::new(SKind::Pseudo);
                    n.value = Some(format!("{pseudo_str}{value}"));
                    let id = self.sel.add(n);
                    self.new_node(id, None);
                    if n_idx > 1 && self.next_tok().is_some_and(|t| t.kind == Tk::OpenParen) {
                        return Err(err("Misplaced parenthesis."));
                    }
                    continue;
                }
            }
            let node = if has_class.contains(&ind) {
                let mut n = SNode::new(SKind::Class);
                n.constructed = false;
                set_unescaped(&mut n, &value[1..]);
                n.constructed = true;
                n
            } else if has_id.contains(&ind) {
                let mut n = SNode::new(SKind::Id);
                set_unescaped(&mut n, &value[1..]);
                n
            } else {
                let mut n = SNode::new(SKind::Tag);
                set_unescaped(&mut n, value);
                n
            };
            let id = self.sel.add(node);
            self.new_node(id, namespace.take());
        }
        self.position += 1;
        Ok(())
    }
}

/// `unescapeProp(opts, "value")` on a fresh node
fn set_unescaped(n: &mut SNode, value: &str) {
    if value.contains('\\') {
        n.value = Some(unesc(value));
        n.raws_mut().value = Some(value.to_string());
    } else {
        n.value = Some(value.to_string());
    }
}

/// `/^\d+\.\d+%$/`
fn is_keyframes_percent(w: &str) -> bool {
    let b = w.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i >= b.len() || b[i] != b'.' {
        return false;
    }
    i += 1;
    let s = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    i > s && i + 1 == b.len() && b[i] == b'%'
}

/// `unescapeValue` from attribute.js: returns (unescaped, quoteMark)
pub fn unescape_value(value: &str) -> (String, Option<char>) {
    let mut quote = None;
    let mut unescaped = value;
    let b = value.as_bytes();
    if b.len() >= 2 && (b[0] == b'\'' || b[0] == b'"') && b[b.len() - 1] == b[0] {
        quote = Some(b[0] as char);
        unescaped = &value[1..value.len() - 1];
    }
    (unesc(unescaped), quote)
}
