//! `postcss/lib/parser.js`

use std::sync::Arc;

use super::tokenize::{T, Token, Tokenizer};
use super::*;

/// `postcss.parse(css)`: returns the root node in `css`.
pub fn parse(arena: &mut Css, css: &str) -> Result<NodeId, CssError> {
    let input = Input::new(css);
    let root = arena.root();
    arena[root].source = Some(Source { input: input.clone(), start: Some(Position { line: 1, column: 1, offset: 0 }), end: None });
    let text = input.css.clone();
    let mut p = Parser {
        arena,
        input,
        css: &text,
        tokenizer: Tokenizer::new(&text),
        root,
        current: root,
        spaces: String::new(),
        semicolon: false,
    };
    p.parse()?;
    Ok(root)
}

struct Parser<'a, 'b> {
    arena: &'a mut Css,
    input: Arc<Input>,
    css: &'b str,
    tokenizer: Tokenizer<'b>,
    root: NodeId,
    current: NodeId,
    spaces: String,
    semicolon: bool,
}

fn is_space_or_comment(t: T) -> bool {
    t == T::Space || t == T::Comment
}

impl<'a, 'b> Parser<'a, 'b> {
    fn text(&self, t: &Token) -> &'b str {
        t.text(self.css)
    }

    fn next(&mut self) -> Result<Option<Token>, CssError> {
        self.tokenizer.next_token(&self.input)
    }

    fn get_position(&self, offset: usize) -> Position {
        self.input.position(offset)
    }

    fn err_range(&self, msg: &str, start: usize, end: usize) -> CssError {
        let s = self.input.from_offset(start);
        let e = self.input.from_offset(end);
        CssError::syntax(msg, Some(s), Some(e))
    }

    fn parse(&mut self) -> Result<(), CssError> {
        while !self.tokenizer.end_of_file() {
            let Some(token) = self.next()? else { break };
            match token.kind {
                T::Space => self.spaces.push_str(self.text(&token)),
                T::Semicolon => self.free_semicolon(&token),
                T::CloseCurly => self.end(&token)?,
                T::Comment => self.comment(&token),
                T::AtWord => self.atrule(token)?,
                T::OpenCurly => self.empty_rule(&token),
                _ => self.other(token)?,
            }
        }
        self.end_file()
    }

    fn init(&mut self, node: NodeId, offset: usize) {
        self.arena.push(self.current, node);
        let start = self.get_position(offset);
        self.arena[node].source = Some(Source { input: self.input.clone(), start: Some(start), end: None });
        self.arena[node].raws.before = Some(std::mem::take(&mut self.spaces));
        if self.arena[node].kind != Kind::Comment {
            self.semicolon = false;
        }
    }

    fn set_end(&mut self, node: NodeId, offset: usize) {
        let mut p = self.get_position(offset);
        p.offset += 1;
        if let Some(s) = self.arena[node].source.as_mut() {
            s.end = Some(p);
        }
    }

    fn comment(&mut self, token: &Token) {
        let node = self.arena.add(Node::new(Kind::Comment));
        self.init(node, token.pos.unwrap());
        self.set_end(node, token.end_pos.or(token.pos).unwrap());
        let raw = self.text(token);
        let text = &raw[2..raw.len() - 2];
        let n = &mut self.arena[node];
        if text.chars().all(|c| c.is_whitespace_js()) {
            n.text = String::new();
            n.raws.left = Some(text.to_string());
            n.raws.right = Some(String::new());
        } else {
            let start = text.len() - text.trim_start_matches(|c: char| c.is_whitespace_js()).len();
            let trimmed_end = text.trim_end_matches(|c: char| c.is_whitespace_js()).len();
            n.text = text[start..trimmed_end].to_string();
            n.raws.left = Some(text[..start].to_string());
            n.raws.right = Some(text[trimmed_end..].to_string());
        }
    }

    fn empty_rule(&mut self, token: &Token) {
        let node = self.arena.add(Node::new(Kind::Rule));
        self.init(node, token.pos.unwrap());
        self.arena[node].selector = String::new();
        self.arena[node].raws.between = Some(String::new());
        self.current = node;
    }

    fn other(&mut self, start: Token) -> Result<(), CssError> {
        let mut end = false;
        let mut colon = false;
        let mut bracket: Option<Token> = None;
        let mut brackets: Vec<T> = Vec::new();
        let custom_property = self.text(&start).starts_with("--");

        let mut tokens: Vec<Token> = Vec::new();
        let mut token = Some(start);
        while let Some(t) = token {
            let kind = t.kind;
            tokens.push(t);

            if kind == T::OpenParen || kind == T::OpenSquare {
                if bracket.is_none() {
                    bracket = Some(t);
                }
                brackets.push(if kind == T::OpenParen { T::CloseParen } else { T::CloseSquare });
            } else if custom_property && colon && kind == T::OpenCurly {
                if bracket.is_none() {
                    bracket = Some(t);
                }
                brackets.push(T::CloseCurly);
            } else if brackets.is_empty() {
                if kind == T::Semicolon {
                    if colon {
                        return self.decl(tokens, custom_property);
                    } else {
                        break;
                    }
                } else if kind == T::OpenCurly {
                    return self.rule(tokens);
                } else if kind == T::CloseCurly {
                    let t = tokens.pop().unwrap();
                    self.tokenizer.back(t);
                    end = true;
                    break;
                } else if kind == T::Colon {
                    colon = true;
                }
            } else if Some(&kind) == brackets.last() {
                brackets.pop();
                if brackets.is_empty() {
                    bracket = None;
                }
            }

            token = self.next()?;
        }

        if self.tokenizer.end_of_file() {
            end = true;
        }
        if !brackets.is_empty() {
            let b = bracket.unwrap();
            let p = b.pos.unwrap();
            return Err(self.err_range("Unclosed bracket", p, p + 1));
        }

        if end && colon {
            if !custom_property {
                while let Some(last) = tokens.last() {
                    if !is_space_or_comment(last.kind) {
                        break;
                    }
                    let t = tokens.pop().unwrap();
                    self.tokenizer.back(t);
                }
            }
            self.decl(tokens, custom_property)
        } else {
            Err(self.unknown_word(&tokens))
        }
    }

    fn unknown_word(&self, tokens: &[Token]) -> CssError {
        let t = tokens[0];
        let p = t.pos.unwrap_or(t.start);
        self.err_range("Unknown word", p, p + self.text(&t).len())
    }

    fn rule(&mut self, mut tokens: Vec<Token>) -> Result<(), CssError> {
        tokens.pop();
        let node = self.arena.add(Node::new(Kind::Rule));
        self.init(node, tokens[0].pos.unwrap_or(tokens[0].start));
        let between = self.spaces_and_comments_from_end(&mut tokens);
        self.arena[node].raws.between = Some(between);
        self.raw(node, Prop::Selector, &tokens, false);
        self.current = node;
        Ok(())
    }

    fn decl(&mut self, mut tokens: Vec<Token>, custom_property: bool) -> Result<(), CssError> {
        let node = self.arena.add(Node::new(Kind::Decl));
        self.init(node, tokens[0].pos.unwrap_or(tokens[0].start));

        let last = *tokens.last().unwrap();
        if last.kind == T::Semicolon {
            self.semicolon = true;
            tokens.pop();
        }
        let end_off = last.end_pos.or(last.pos).or_else(|| find_last_with_position(&tokens)).unwrap_or(0);
        self.set_end(node, end_off);

        while tokens[0].kind != T::Word {
            if tokens.len() == 1 {
                return Err(self.unknown_word(&tokens));
            }
            let t = tokens.remove(0);
            let s = self.text(&t);
            self.arena[node].raws.before.as_mut().unwrap().push_str(s);
        }
        let start = self.get_position(tokens[0].pos.unwrap_or(tokens[0].start));
        self.arena[node].source.as_mut().unwrap().start = Some(start);

        let mut prop = String::new();
        while let Some(t) = tokens.first() {
            if matches!(t.kind, T::Colon | T::Space | T::Comment) {
                break;
            }
            let t = tokens.remove(0);
            prop.push_str(self.text(&t));
        }

        let mut between = String::new();
        while !tokens.is_empty() {
            let t = tokens.remove(0);
            if t.kind == T::Colon {
                between.push_str(self.text(&t));
                break;
            } else {
                if t.kind == T::Word && self.text(&t).chars().any(crate::js::is_word_char) {
                    return Err(self.unknown_word(&[t]));
                }
                between.push_str(self.text(&t));
            }
        }

        if prop.starts_with('_') || prop.starts_with('*') {
            let c = prop.remove(0);
            self.arena[node].raws.before.as_mut().unwrap().push(c);
        }

        let mut first_spaces: Vec<Token> = Vec::new();
        while let Some(t) = tokens.first() {
            if !is_space_or_comment(t.kind) {
                break;
            }
            first_spaces.push(tokens.remove(0));
        }

        let mut important = false;
        let mut raws_important: Option<String> = None;
        let mut i = tokens.len() as isize - 1;
        while i >= 0 {
            let t = tokens[i as usize];
            let lower = self.text(&t).to_lowercase();
            if lower == "!important" {
                important = true;
                let mut string = self.string_from(&mut tokens, i as usize);
                string = self.spaces_from_end(&mut tokens) + &string;
                if string != " !important" {
                    raws_important = Some(string);
                }
                break;
            } else if lower == "important" {
                let mut cache = tokens.clone();
                let mut s = String::new();
                let mut j = i;
                while j > 0 {
                    let kind = cache[j as usize].kind;
                    if crate::js::trim(&s).starts_with('!') && kind != T::Space {
                        break;
                    }
                    let t = cache.pop().unwrap();
                    s = self.text(&t).to_string() + &s;
                    j -= 1;
                }
                if crate::js::trim(&s).starts_with('!') {
                    important = true;
                    raws_important = Some(s);
                    tokens = cache;
                }
            }
            if t.kind != T::Space && t.kind != T::Comment {
                break;
            }
            i -= 1;
        }

        let has_word = tokens.iter().any(|t| !is_space_or_comment(t.kind));
        if has_word {
            for t in &first_spaces {
                between.push_str(self.text(t));
            }
            first_spaces.clear();
        }
        {
            let n = &mut self.arena[node];
            n.prop = prop;
            n.raws.between = Some(between);
            n.important = important;
            n.raws.important = raws_important;
        }
        let mut all = first_spaces;
        all.extend(tokens.iter().copied());
        self.raw(node, Prop::Value, &all, custom_property);

        if self.arena[node].value.contains(':') && !custom_property {
            self.check_missed_semicolon(&tokens)?;
        }
        Ok(())
    }

    fn check_missed_semicolon(&self, tokens: &[Token]) -> Result<(), CssError> {
        let colon = self.colon(tokens)?;
        let Some(colon) = colon else { return Ok(()) };
        let mut founded = 0;
        let mut token = None;
        let mut j = colon as isize - 1;
        while j >= 0 {
            let t = tokens[j as usize];
            token = Some(t);
            if t.kind != T::Space {
                founded += 1;
                if founded == 2 {
                    break;
                }
            }
            j -= 1;
        }
        let token = token.unwrap();
        let offset = if token.kind == T::Word { token.end_pos.unwrap() + 1 } else { token.pos.unwrap() };
        let (l, c) = self.input.from_offset(offset);
        Err(CssError::syntax("Missed semicolon", Some((l, c)), None))
    }

    fn colon(&self, tokens: &[Token]) -> Result<Option<usize>, CssError> {
        let mut brackets = 0;
        let mut prev: Option<Token> = None;
        for (i, t) in tokens.iter().enumerate() {
            if t.kind == T::OpenParen {
                brackets += 1;
            }
            if t.kind == T::CloseParen {
                brackets -= 1;
            }
            if brackets == 0 && t.kind == T::Colon {
                match prev {
                    None => {
                        let p = t.pos.unwrap();
                        return Err(self.err_range("Double colon", p, p + 1));
                    }
                    Some(p) if p.kind == T::Word && self.text(&p) == "progid" => continue,
                    Some(_) => return Ok(Some(i)),
                }
            }
            prev = Some(*t);
        }
        Ok(None)
    }

    fn atrule(&mut self, token: Token) -> Result<(), CssError> {
        let node = self.arena.add(Node::new(Kind::AtRule));
        let name = self.text(&token)[1..].to_string();
        if name.is_empty() {
            let p = token.pos.unwrap();
            return Err(self.err_range("At-rule without name", p, p + self.text(&token).len()));
        }
        self.arena[node].name = name;
        self.init(node, token.pos.unwrap());

        let mut last = false;
        let mut open = false;
        let mut params: Vec<Token> = Vec::new();
        let mut brackets: Vec<T> = Vec::new();

        while !self.tokenizer.end_of_file() {
            let Some(token) = self.next()? else { break };
            let kind = token.kind;
            if kind == T::OpenParen || kind == T::OpenSquare {
                brackets.push(if kind == T::OpenParen { T::CloseParen } else { T::CloseSquare });
            } else if kind == T::OpenCurly && !brackets.is_empty() {
                brackets.push(T::CloseCurly);
            } else if Some(&kind) == brackets.last() {
                brackets.pop();
            }

            if brackets.is_empty() {
                if kind == T::Semicolon {
                    self.set_end(node, token.pos.unwrap());
                    self.semicolon = true;
                    break;
                } else if kind == T::OpenCurly {
                    open = true;
                    break;
                } else if kind == T::CloseCurly {
                    if !params.is_empty() {
                        let mut shift = params.len() as isize - 1;
                        let mut prev = params.get(shift as usize).copied();
                        while let Some(p) = prev {
                            if p.kind != T::Space {
                                break;
                            }
                            shift -= 1;
                            prev = if shift >= 0 { params.get(shift as usize).copied() } else { None };
                        }
                        if let Some(p) = prev {
                            self.set_end(node, p.end_pos.or(p.pos).unwrap());
                        }
                    }
                    self.end(&token)?;
                    break;
                } else {
                    params.push(token);
                }
            } else {
                params.push(token);
            }

            if self.tokenizer.end_of_file() {
                last = true;
                break;
            }
        }

        let between = self.spaces_and_comments_from_end(&mut params);
        self.arena[node].raws.between = Some(between);
        if !params.is_empty() {
            let after_name = self.spaces_and_comments_from_start(&mut params);
            self.arena[node].raws.after_name = Some(after_name);
            self.raw(node, Prop::Params, &params, false);
            if last {
                let t = *params.last().unwrap();
                self.set_end(node, t.end_pos.or(t.pos).unwrap());
                self.spaces = self.arena[node].raws.between.take().unwrap_or_default();
                self.arena[node].raws.between = Some(String::new());
            }
        } else {
            self.arena[node].raws.after_name = Some(String::new());
            self.arena[node].params = String::new();
        }

        if open {
            self.arena[node].nodes = Some(Vec::new());
            self.current = node;
        }
        Ok(())
    }

    fn end(&mut self, token: &Token) -> Result<(), CssError> {
        let cur = self.current;
        if self.arena[cur].nodes.as_ref().is_some_and(|n| !n.is_empty()) {
            self.arena[cur].raws.semicolon = Some(self.semicolon);
        }
        self.semicolon = false;
        let after = self.arena[cur].raws.after.take().unwrap_or_default() + &self.spaces;
        self.arena[cur].raws.after = Some(after);
        self.spaces.clear();

        if let Some(parent) = self.arena[cur].parent {
            self.set_end(cur, token.pos.unwrap());
            self.current = parent;
            Ok(())
        } else {
            let p = token.pos.unwrap();
            Err(self.err_range("Unexpected }", p, p + 1))
        }
    }

    fn end_file(&mut self) -> Result<(), CssError> {
        let cur = self.current;
        if self.arena[cur].parent.is_some() {
            let pos = self.arena[cur].source.as_ref().unwrap().start.unwrap();
            return Err(CssError::syntax("Unclosed block", Some((pos.line, pos.column)), None));
        }
        if self.arena[cur].nodes.as_ref().is_some_and(|n| !n.is_empty()) {
            self.arena[cur].raws.semicolon = Some(self.semicolon);
        }
        let after = self.arena[cur].raws.after.take().unwrap_or_default() + &self.spaces;
        self.arena[cur].raws.after = Some(after);
        let end = self.get_position(self.tokenizer.position());
        let root = self.root;
        self.arena[root].source.as_mut().unwrap().end = Some(end);
        Ok(())
    }

    fn free_semicolon(&mut self, token: &Token) {
        self.spaces.push_str(self.text(token));
        let cur = self.current;
        if let Some(list) = &self.arena[cur].nodes {
            if let Some(&prev) = list.last() {
                if self.arena[prev].kind == Kind::Rule && self.arena[prev].raws.own_semicolon.as_deref().is_none_or(|s| s.is_empty()) {
                    self.arena[prev].raws.own_semicolon = Some(std::mem::take(&mut self.spaces));
                }
            }
        }
    }

    fn raw(&mut self, node: NodeId, prop: Prop, tokens: &[Token], custom_property: bool) {
        let length = tokens.len();
        let mut value = String::new();
        let mut clean = true;
        for i in 0..length {
            let t = tokens[i];
            if t.kind == T::Space && i == length - 1 && !custom_property {
                clean = false;
            } else if t.kind == T::Comment {
                let prev = if i > 0 { Some(tokens[i - 1].kind) } else { None };
                let next = tokens.get(i + 1).map(|t| t.kind);
                let safe = |k: Option<T>| matches!(k, None | Some(T::Space));
                if !safe(prev) && !safe(next) {
                    if value.ends_with(',') {
                        clean = false;
                    } else {
                        value.push_str(self.text(&t));
                    }
                } else {
                    clean = false;
                }
            } else {
                value.push_str(self.text(&t));
            }
        }
        let n = &mut self.arena[node];
        if !clean {
            let raw: String = tokens.iter().map(|t| t.text(self.css)).collect();
            let rv = Some(RawValue { raw, value: value.clone() });
            match prop {
                Prop::Selector => n.raws.selector = rv,
                Prop::Value => n.raws.value = rv,
                Prop::Params => n.raws.params = rv,
            }
        }
        match prop {
            Prop::Selector => n.selector = value,
            Prop::Value => n.value = value,
            Prop::Params => n.params = value,
        }
    }

    fn spaces_and_comments_from_end(&self, tokens: &mut Vec<Token>) -> String {
        let mut spaces = String::new();
        while let Some(t) = tokens.last() {
            if !is_space_or_comment(t.kind) {
                break;
            }
            let t = tokens.pop().unwrap();
            spaces = self.text(&t).to_string() + &spaces;
        }
        spaces
    }

    fn spaces_and_comments_from_start(&self, tokens: &mut Vec<Token>) -> String {
        let mut spaces = String::new();
        while let Some(t) = tokens.first() {
            if !is_space_or_comment(t.kind) {
                break;
            }
            let t = tokens.remove(0);
            spaces.push_str(self.text(&t));
        }
        spaces
    }

    fn spaces_from_end(&self, tokens: &mut Vec<Token>) -> String {
        let mut spaces = String::new();
        while let Some(t) = tokens.last() {
            if t.kind != T::Space {
                break;
            }
            let t = tokens.pop().unwrap();
            spaces = self.text(&t).to_string() + &spaces;
        }
        spaces
    }

    fn string_from(&self, tokens: &mut Vec<Token>, from: usize) -> String {
        let s: String = tokens[from..].iter().map(|t| t.text(self.css)).collect();
        tokens.truncate(from);
        s
    }
}

#[derive(Clone, Copy)]
enum Prop {
    Selector,
    Value,
    Params,
}

fn find_last_with_position(tokens: &[Token]) -> Option<usize> {
    for t in tokens.iter().rev() {
        let p = t.end_pos.or(t.pos);
        if let Some(p) = p {
            if p != 0 {
                return Some(p);
            }
        }
    }
    None
}

