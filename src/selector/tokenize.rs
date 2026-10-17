//! `postcss-selector-parser/dist/tokenize.js`

use super::SelError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tk {
    Space,
    Combinator,
    Word,
    Str,
    Comment,
    Asterisk,
    Ampersand,
    Bang,
    Comma,
    Equals,
    Dollar,
    Caret,
    OpenSquare,
    CloseSquare,
    Colon,
    Semicolon,
    OpenParen,
    CloseParen,
    Slash,
}

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub kind: Tk,
    /// byte offsets of the token text
    pub start: usize,
    pub end: usize,
}

fn is_word_delimiter(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'\t' | b'\n' | b'\r' | 0x0c | b'&' | b'*' | b'!' | b',' | b':' | b';' | b'(' | b')' | b'[' | b']' | b'\''
            | b'"' | b'+' | b'|' | b'~' | b'>' | b'=' | b'$' | b'^' | b'/'
    )
}

fn char_len(css: &str, i: usize) -> usize {
    css[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)
}

fn consume_word(css: &str, start: usize) -> usize {
    let b = css.as_bytes();
    let mut next = start;
    loop {
        let code = b[next];
        if is_word_delimiter(code) {
            return next.wrapping_sub(1);
        } else if code == b'\\' {
            next = consume_escape(css, next) + 1;
        } else {
            next += char_len(css, next);
        }
        if next >= b.len() {
            break;
        }
    }
    next - 1
}

fn consume_escape(css: &str, start: usize) -> usize {
    let b = css.as_bytes();
    let mut next = start;
    match b.get(next + 1) {
        Some(b'\t' | b'\n' | b'\r' | 0x0c) => {}
        Some(c) if c.is_ascii_hexdigit() => {
            let mut digits = 0;
            loop {
                next += 1;
                digits += 1;
                match b.get(next + 1) {
                    Some(c) if c.is_ascii_hexdigit() && digits < 6 => continue,
                    _ => break,
                }
            }
            if digits < 6 && b.get(next + 1) == Some(&b' ') {
                next += 1;
            }
        }
        Some(_) => {
            next += char_len(css, next + 1);
        }
        None => {
            next += 1;
        }
    }
    next
}

pub fn tokenize(css: &str) -> Result<Vec<Token>, SelError> {
    let b = css.as_bytes();
    let length = b.len();
    let mut tokens = Vec::new();
    let mut start = 0;
    while start < length {
        let code = b[start];
        let (kind, end);
        match code {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0c => {
                let mut next = start;
                loop {
                    next += 1;
                    match b.get(next) {
                        Some(b' ' | b'\n' | b'\t' | b'\r' | 0x0c) => {}
                        _ => break,
                    }
                }
                kind = Tk::Space;
                end = next;
            }
            b'+' | b'>' | b'~' | b'|' => {
                let mut next = start;
                loop {
                    next += 1;
                    match b.get(next) {
                        Some(b'+' | b'>' | b'~' | b'|') => {}
                        _ => break,
                    }
                }
                kind = Tk::Combinator;
                end = next;
            }
            b'*' | b'&' | b'!' | b',' | b'=' | b'$' | b'^' | b'[' | b']' | b':' | b';' | b'(' | b')' => {
                kind = match code {
                    b'*' => Tk::Asterisk,
                    b'&' => Tk::Ampersand,
                    b'!' => Tk::Bang,
                    b',' => Tk::Comma,
                    b'=' => Tk::Equals,
                    b'$' => Tk::Dollar,
                    b'^' => Tk::Caret,
                    b'[' => Tk::OpenSquare,
                    b']' => Tk::CloseSquare,
                    b':' => Tk::Colon,
                    b';' => Tk::Semicolon,
                    b'(' => Tk::OpenParen,
                    _ => Tk::CloseParen,
                };
                end = start + 1;
            }
            b'\'' | b'"' => {
                let quote = code as char;
                let mut next = start;
                loop {
                    let mut escaped = false;
                    match css[next + 1..].find(quote) {
                        Some(i) => next = next + 1 + i,
                        None => return Err(SelError("Unclosed quote".into())),
                    }
                    let mut p = next;
                    while p > 0 && b[p - 1] == b'\\' {
                        p -= 1;
                        escaped = !escaped;
                    }
                    if !escaped {
                        break;
                    }
                }
                kind = Tk::Str;
                end = next + 1;
            }
            _ => {
                if code == b'/' && b.get(start + 1) == Some(&b'*') {
                    let next = match css[start + 2..].find("*/") {
                        Some(i) => start + 2 + i + 1,
                        None => return Err(SelError("Unclosed comment".into())),
                    };
                    kind = Tk::Comment;
                    end = next + 1;
                } else if code == b'/' {
                    kind = Tk::Slash;
                    end = start + 1;
                } else {
                    let next = consume_word(css, start);
                    kind = Tk::Word;
                    end = (next.wrapping_add(1)).min(length);
                }
            }
        }
        tokens.push(Token { kind, start, end });
        start = end;
    }
    Ok(tokens)
}
