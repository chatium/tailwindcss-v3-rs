//! Port of tailwind's vendored `src/value-parser` (a postcss-value-parser fork).

#[derive(Debug, Clone, PartialEq)]
pub enum VNode {
    Word(String),
    Space(String),
    UnicodeRange(String),
    Str { quote: char, value: String, unclosed: bool },
    Comment { value: String, unclosed: bool },
    Div { value: String, before: String, after: String },
    Function { value: String, before: String, after: String, nodes: Vec<VNode>, unclosed: bool },
}

impl VNode {
    pub fn is_function(&self) -> bool {
        matches!(self, VNode::Function { .. })
    }
}

fn code(b: &[u8], i: usize) -> Option<u8> {
    b.get(i).copied()
}

fn le32(c: Option<u8>) -> bool {
    matches!(c, Some(c) if c <= 32)
}

/// `parse(input)`
pub fn parse(input: &str) -> Vec<VNode> {
    let mut value = input.to_string();
    let max = input.len();
    // stack of open function frames: (token without nodes, parent nodes)
    struct Frame {
        value: String,
        before: String,
        after: String,
        nodes: Vec<VNode>,
    }
    let mut stack: Vec<Frame> = Vec::new();
    let mut tokens: Vec<VNode> = Vec::new();
    let mut balanced = 0usize;
    let mut pos = 0usize;
    let mut name = String::new();
    let mut before = String::new();
    let mut after = String::new();
    let mut c = code(value.as_bytes(), pos);

    while pos < max {
        let b = value.as_bytes();
        let in_function = balanced > 0;
        if le32(c) {
            let mut next = pos;
            loop {
                next += 1;
                c = code(b, next);
                if !le32(c) {
                    break;
                }
            }
            let token = value[pos..next].to_string();
            let prev_is_div = matches!(tokens.last(), Some(VNode::Div { .. }));
            if c == Some(b')') && balanced > 0 {
                after = token;
            } else if prev_is_div {
                if let Some(VNode::Div { after: a, .. }) = tokens.last_mut() {
                    *a = token;
                }
            } else if c == Some(b',') || c == Some(b':') || (c == Some(b'/') && code(b, next + 1) != Some(b'*') && !in_function) {
                before = token;
            } else {
                tokens.push(VNode::Space(token));
            }
            pos = next;
        } else if c == Some(b'\'') || c == Some(b'"') {
            let quote = c.unwrap() as char;
            let mut next = pos;
            let mut unclosed = false;
            loop {
                let mut escape = false;
                match value[next + 1..].find(quote) {
                    Some(i) => {
                        next = next + 1 + i;
                        let vb = value.as_bytes();
                        let mut ep = next;
                        while ep > 0 && vb[ep - 1] == b'\\' {
                            ep -= 1;
                            escape = !escape;
                        }
                    }
                    None => {
                        value.push(quote);
                        next = value.len() - 1;
                        unclosed = true;
                    }
                }
                if !escape {
                    break;
                }
            }
            tokens.push(VNode::Str { quote, value: value[pos + 1..next].to_string(), unclosed });
            pos = next + 1;
            c = code(value.as_bytes(), pos);
        } else if c == Some(b'/') && code(b, pos + 1) == Some(b'*') {
            let found = value[pos..].find("*/").map(|i| pos + i);
            let (next, unclosed) = match found {
                Some(n) => (n, false),
                None => (value.len(), true),
            };
            tokens.push(VNode::Comment { value: value[pos + 2..next.max(pos + 2)].to_string(), unclosed });
            pos = next + 2;
            c = code(value.as_bytes(), pos);
        } else if (c == Some(b'/') || c == Some(b'*')) && in_function {
            tokens.push(VNode::Word((c.unwrap() as char).to_string()));
            pos += 1;
            c = code(b, pos);
        } else if c == Some(b'/') || c == Some(b',') || c == Some(b':') {
            tokens.push(VNode::Div { value: (c.unwrap() as char).to_string(), before: std::mem::take(&mut before), after: String::new() });
            pos += 1;
            c = code(b, pos);
        } else if c == Some(b'(') {
            let mut next = pos;
            loop {
                next += 1;
                c = code(b, next);
                if !le32(c) {
                    break;
                }
            }
            let open_pos = pos;
            let fn_before = value[open_pos + 1..next].to_string();
            pos = next;
            if name == "url" && c != Some(b'\'') && c != Some(b'"') {
                let mut next = next - 1;
                let mut unclosed = false;
                loop {
                    let mut escape = false;
                    match value[next + 1..].find(')') {
                        Some(i) => {
                            next = next + 1 + i;
                            let vb = value.as_bytes();
                            let mut ep = next;
                            while ep > 0 && vb[ep - 1] == b'\\' {
                                ep -= 1;
                                escape = !escape;
                            }
                        }
                        None => {
                            value.push(')');
                            next = value.len() - 1;
                            unclosed = true;
                        }
                    }
                    if !escape {
                        break;
                    }
                }
                let vb = value.as_bytes();
                let mut ws = next;
                loop {
                    if ws == 0 {
                        break;
                    }
                    ws -= 1;
                    if !le32(code(vb, ws)) {
                        break;
                    }
                }
                let mut nodes = Vec::new();
                let fn_after;
                if open_pos < ws {
                    if pos != ws + 1 {
                        nodes.push(VNode::Word(value[pos..ws + 1].to_string()));
                    }
                    if unclosed && ws + 1 != next {
                        fn_after = String::new();
                        nodes.push(VNode::Space(value[ws + 1..next].to_string()));
                    } else {
                        fn_after = value[ws + 1..next].to_string();
                    }
                } else {
                    fn_after = String::new();
                }
                pos = next + 1;
                c = code(value.as_bytes(), pos);
                tokens.push(VNode::Function {
                    value: std::mem::take(&mut name),
                    before: fn_before,
                    after: fn_after,
                    nodes,
                    unclosed,
                });
            } else {
                balanced += 1;
                stack.push(Frame {
                    value: std::mem::take(&mut name),
                    before: fn_before,
                    after: String::new(),
                    nodes: std::mem::take(&mut tokens),
                });
            }
            name.clear();
        } else if c == Some(b')') && balanced > 0 {
            pos += 1;
            c = code(b, pos);
            let frame = stack.pop().unwrap();
            let node = VNode::Function { value: frame.value, before: frame.before, after: std::mem::take(&mut after), nodes: std::mem::take(&mut tokens), unclosed: false };
            let _ = frame.after;
            tokens = frame.nodes;
            tokens.push(node);
            balanced -= 1;
        } else {
            let mut next = pos;
            loop {
                if c == Some(b'\\') {
                    next += 1;
                    // skip a whole character
                    while next < b.len() && !value.is_char_boundary(next) {
                        next += 1;
                    }
                    if next < b.len() {
                        let cl = value[next..].chars().next().map(|ch| ch.len_utf8()).unwrap_or(1);
                        next += cl - 1;
                    }
                }
                next += 1;
                while next < b.len() && !value.is_char_boundary(next) {
                    next += 1;
                }
                c = code(b, next);
                let stop = next >= max
                    || le32(c)
                    || matches!(c, Some(b'\'' | b'"' | b',' | b':' | b'/' | b'('))
                    || (c == Some(b'*') && in_function)
                    || (c == Some(b')') && balanced > 0);
                if stop {
                    break;
                }
            }
            let next = next.min(value.len());
            let token = value[pos..next].to_string();
            if c == Some(b'(') {
                name = token;
            } else if token.len() >= 2
                && matches!(token.as_bytes()[0], b'u' | b'U')
                && token.as_bytes()[1] == b'+'
                && !token[2..].is_empty()
                && token[2..].bytes().all(|x| x.is_ascii_hexdigit() || x == b'?' || x == b'-')
            {
                tokens.push(VNode::UnicodeRange(token));
            } else {
                tokens.push(VNode::Word(token));
            }
            pos = next;
        }
    }

    // close unclosed functions
    while let Some(frame) = stack.pop() {
        let node = VNode::Function { value: frame.value, before: frame.before, after: frame.after, nodes: std::mem::take(&mut tokens), unclosed: true };
        tokens = frame.nodes;
        tokens.push(node);
    }
    tokens
}

pub fn stringify_node(n: &VNode) -> String {
    match n {
        VNode::Word(v) | VNode::Space(v) | VNode::UnicodeRange(v) => v.clone(),
        VNode::Str { quote, value, unclosed } => {
            let q = quote.to_string();
            format!("{q}{value}{}", if *unclosed { "" } else { &q })
        }
        VNode::Comment { value, unclosed } => format!("/*{value}{}", if *unclosed { "" } else { "*/" }),
        VNode::Div { value, before, after } => format!("{before}{value}{after}"),
        VNode::Function { value, before, after, nodes, unclosed } => {
            format!("{value}({before}{}{after}{}", stringify(nodes), if *unclosed { "" } else { ")" })
        }
    }
}

pub fn stringify(nodes: &[VNode]) -> String {
    nodes.iter().map(stringify_node).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        for s in [
            "theme(colors.red.500)",
            "calc(theme('spacing.4') * 2)",
            "url(  a b  )",
            "a, b / c",
            "rgb(0 0 0 / theme(opacity.50))",
            "'unclosed",
            "f(a",
            "u+0025-00ff",
            "/* x */ a",
        ] {
            assert_eq!(stringify(&parse(s)), if s == "'unclosed" { "'unclosed" } else { s });
        }
    }
}
