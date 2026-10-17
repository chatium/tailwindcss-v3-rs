//! `postcss/lib/tokenize.js`

use super::{CssError, Input};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum T {
    Space,
    Word,
    String,
    AtWord,
    Brackets,
    Comment,
    OpenParen,
    CloseParen,
    OpenSquare,
    CloseSquare,
    OpenCurly,
    CloseCurly,
    Colon,
    Semicolon,
}

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub kind: T,
    /// byte range of the token text
    pub start: usize,
    pub end: usize,
    /// token[2] / token[3]
    pub pos: Option<usize>,
    pub end_pos: Option<usize>,
}

impl Token {
    pub fn text<'a>(&self, css: &'a str) -> &'a str {
        &css[self.start..self.end]
    }
}

pub struct Tokenizer<'a> {
    css: &'a str,
    bytes: &'a [u8],
    pos: usize,
    buffer: Vec<Token>,
    returned: Vec<Token>,
}

fn is_at_end(c: u8) -> bool {
    matches!(c, b'\t' | b'\n' | 0x0c | b'\r' | b' ' | b'"' | b'#' | b'\'' | b'(' | b')' | b'/' | b';' | b'[' | b'\\' | b']' | b'{' | b'}')
}

fn is_word_end(c: u8) -> bool {
    matches!(c, b'\t' | b'\n' | 0x0c | b'\r' | b' ' | b'!' | b'"' | b'#' | b'\'' | b'(' | b')' | b':' | b';' | b'@' | b'[' | b'\\' | b']' | b'{' | b'}')
}

impl<'a> Tokenizer<'a> {
    pub fn new(css: &'a str) -> Self {
        Tokenizer { css, bytes: css.as_bytes(), pos: 0, buffer: Vec::new(), returned: Vec::new() }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn end_of_file(&self) -> bool {
        self.returned.is_empty() && self.pos >= self.bytes.len()
    }

    pub fn back(&mut self, t: Token) {
        self.returned.push(t);
    }

    fn code(&self, i: usize) -> Option<u8> {
        self.bytes.get(i).copied()
    }

    fn unclosed(&self, input: &Input, what: &str) -> CssError {
        let (line, col) = input.from_offset(self.pos);
        CssError::syntax(format!("Unclosed {what}"), Some((line, col)), None)
    }

    pub fn next_token(&mut self, input: &Input) -> Result<Option<Token>, CssError> {
        if let Some(t) = self.returned.pop() {
            return Ok(Some(t));
        }
        let len = self.bytes.len();
        if self.pos >= len {
            return Ok(None);
        }
        let pos = self.pos;
        let code = self.bytes[pos];
        let tok;
        match code {
            b'\n' | b' ' | b'\t' | b'\r' | 0x0c => {
                let mut next = pos;
                loop {
                    next += 1;
                    match self.code(next) {
                        Some(b' ' | b'\n' | b'\t' | b'\r' | 0x0c) => {}
                        _ => break,
                    }
                }
                tok = Token { kind: T::Space, start: pos, end: next, pos: None, end_pos: None };
                self.pos = next - 1;
            }
            b'[' | b']' | b'{' | b'}' | b':' | b';' | b')' => {
                let kind = match code {
                    b'[' => T::OpenSquare,
                    b']' => T::CloseSquare,
                    b'{' => T::OpenCurly,
                    b'}' => T::CloseCurly,
                    b':' => T::Colon,
                    b';' => T::Semicolon,
                    _ => T::CloseParen,
                };
                tok = Token { kind, start: pos, end: pos + 1, pos: Some(pos), end_pos: None };
            }
            b'(' => {
                let prev = match self.buffer.pop() {
                    Some(t) => t.text(self.css),
                    None => "",
                };
                let n = self.code(pos + 1);
                if prev == "url" && !matches!(n, Some(b'\'' | b'"' | b' ' | b'\n' | b'\t' | 0x0c | b'\r')) {
                    let mut next = pos;
                    loop {
                        let mut escaped = false;
                        match self.css[next + 1..].find(')') {
                            Some(i) => next = next + 1 + i,
                            None => return Err(self.unclosed(input, "bracket")),
                        }
                        let mut escape_pos = next;
                        while escape_pos > 0 && self.bytes[escape_pos - 1] == b'\\' {
                            escape_pos -= 1;
                            escaped = !escaped;
                        }
                        if !escaped {
                            break;
                        }
                    }
                    tok = Token { kind: T::Brackets, start: pos, end: next + 1, pos: Some(pos), end_pos: Some(next) };
                    self.pos = next;
                } else {
                    let next = self.css[pos + 1..].find(')').map(|i| pos + 1 + i);
                    let bad = match next {
                        None => true,
                        Some(next) => {
                            let content = &self.bytes[pos..next + 1];
                            content.len() > 1
                                && content[1..].iter().any(|c| matches!(c, b'\r' | b'\n' | b'"' | b'\'' | b'(' | b'/' | b'\\'))
                        }
                    };
                    if bad {
                        tok = Token { kind: T::OpenParen, start: pos, end: pos + 1, pos: Some(pos), end_pos: None };
                    } else {
                        let next = next.unwrap();
                        tok = Token { kind: T::Brackets, start: pos, end: next + 1, pos: Some(pos), end_pos: Some(next) };
                        self.pos = next;
                    }
                }
            }
            b'\'' | b'"' => {
                let quote = code as char;
                let mut next = pos;
                loop {
                    let mut escaped = false;
                    match self.css[next + 1..].find(quote) {
                        Some(i) => next = next + 1 + i,
                        None => return Err(self.unclosed(input, "string")),
                    }
                    let mut escape_pos = next;
                    while escape_pos > 0 && self.bytes[escape_pos - 1] == b'\\' {
                        escape_pos -= 1;
                        escaped = !escaped;
                    }
                    if !escaped {
                        break;
                    }
                }
                tok = Token { kind: T::String, start: pos, end: next + 1, pos: Some(pos), end_pos: Some(next) };
                self.pos = next;
            }
            b'@' => {
                let next = match self.bytes[pos + 1..].iter().position(|c| is_at_end(*c)) {
                    Some(i) => pos + 1 + i - 1,
                    None => len - 1,
                };
                tok = Token { kind: T::AtWord, start: pos, end: next + 1, pos: Some(pos), end_pos: Some(next) };
                self.pos = next;
            }
            b'\\' => {
                let mut next = pos;
                let mut escape = true;
                while self.code(next + 1) == Some(b'\\') {
                    next += 1;
                    escape = !escape;
                }
                let c = self.code(next + 1);
                if escape && !matches!(c, Some(b'/' | b' ' | b'\n' | b'\t' | b'\r' | 0x0c)) && c.is_some() {
                    next += 1;
                    if self.bytes[next].is_ascii_hexdigit() {
                        while self.code(next + 1).is_some_and(|c| c.is_ascii_hexdigit()) {
                            next += 1;
                        }
                        if self.code(next + 1) == Some(b' ') {
                            next += 1;
                        }
                    } else {
                        // include the whole (possibly multi-byte) escaped character
                        while next + 1 < len && !self.css.is_char_boundary(next + 1) {
                            next += 1;
                        }
                    }
                } else if escape && c.is_none() {
                    // `css.charCodeAt(next + 1)` is NaN: still advances one position
                    next += 1;
                    next = next.min(len - 1).max(pos);
                }
                tok = Token { kind: T::Word, start: pos, end: next + 1, pos: Some(pos), end_pos: Some(next) };
                self.pos = next;
            }
            _ => {
                if code == b'/' && self.code(pos + 1) == Some(b'*') {
                    let next = match self.css[pos + 2..].find("*/") {
                        Some(i) => pos + 2 + i + 1,
                        None => return Err(self.unclosed(input, "comment")),
                    };
                    tok = Token { kind: T::Comment, start: pos, end: next + 1, pos: Some(pos), end_pos: Some(next) };
                    self.pos = next;
                } else {
                    let mut next = len - 1;
                    let mut i = pos + 1;
                    while i < len {
                        let c = self.bytes[i];
                        if is_word_end(c) || (c == b'/' && self.code(i + 1) == Some(b'*')) {
                            next = i - 1;
                            break;
                        }
                        i += 1;
                    }
                    // never split a multi-byte character
                    tok = Token { kind: T::Word, start: pos, end: next + 1, pos: Some(pos), end_pos: Some(next) };
                    self.buffer.push(tok);
                    self.pos = next;
                }
            }
        }
        self.pos += 1;
        Ok(Some(tok))
    }
}
