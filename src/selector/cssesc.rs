//! `cssesc@3.0.0`

#[derive(Clone, Copy)]
pub struct Cssesc {
    pub is_identifier: bool,
    pub double_quotes: bool,
    pub wrap: bool,
}

impl Cssesc {
    pub fn identifier() -> Self {
        Cssesc { is_identifier: true, double_quotes: false, wrap: false }
    }
    pub fn quoted(double: bool) -> Self {
        Cssesc { is_identifier: false, double_quotes: double, wrap: true }
    }
}

fn single_escape(c: char) -> bool {
    // /[ -,\.\/:-@\[\]\^`\{-~]/
    matches!(c, ' '..=',' | '.' | '/' | ':'..='@' | '[' | ']' | '^' | '`' | '{'..='~')
}

fn hex_escape(cp: u32) -> String {
    format!("\\{:X} ", cp)
}

pub fn cssesc(s: &str, opts: Cssesc) -> String {
    let quote = if opts.double_quotes { '"' } else { '\'' };
    let mut output = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        let cp = c as u32;
        if cp < 0x20 || cp > 0x7e {
            output.push_str(&hex_escape(cp));
        } else if matches!(c, '\t' | '\n' | '\x0c' | '\r' | '\x0b') {
            output.push_str(&hex_escape(cp));
        } else if c == '\\'
            || (!opts.is_identifier && ((c == '"' && quote == c) || (c == '\'' && quote == c)))
            || (opts.is_identifier && single_escape(c))
        {
            output.push('\\');
            output.push(c);
        } else {
            output.push(c);
        }
    }

    if opts.is_identifier {
        let b = output.as_bytes();
        if b.len() >= 2 && b[0] == b'-' && (b[1] == b'-' || b[1].is_ascii_digit()) {
            output = format!("\\-{}", &output[1..]);
        } else if let Some(first) = s.chars().next() {
            if first.is_ascii_digit() {
                output = format!("\\3{} {}", first, &output[1..]);
            }
        }
    }

    output = remove_excessive_spaces(&output);

    if !opts.is_identifier && opts.wrap {
        return format!("{quote}{output}{quote}");
    }
    output
}

/// `output.replace(/(^|\\+)?(\\[A-F0-9]{1,6})\x20(?![a-fA-F0-9\x20])/g, …)`
fn remove_excessive_spaces(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        // try to match at position i
        if let Some((end, keep_space)) = match_at(b, i) {
            if !keep_space {
                out.push_str(&s[last..end - 1]);
                last = end;
                i = end;
                continue;
            } else {
                // whole match kept verbatim; continue after it
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    out
}

/// Matches the regex at `start`; returns (end index after the space, whether to keep the space).
fn match_at(b: &[u8], start: usize) -> Option<(usize, bool)> {
    // group 1: (^|\\+)? — greedy backslashes; we try the longest run of backslashes first
    // and then need a `\\[A-F0-9]{1,6} ` after it.
    let mut j = start;
    while j < b.len() && b[j] == b'\\' {
        j += 1;
    }
    let run = j - start;
    // Try with group1 = run-1 backslashes (the last backslash starts group 2), down to 0.
    // The regex engine backtracks from the greedy choice.
    if run == 0 {
        return None;
    }
    let mut g1 = run - 1;
    loop {
        let p = start + g1;
        if let Some(end) = match_escape(b, p) {
            let keep = g1 > 0 && g1 % 2 == 1;
            return Some((end, keep));
        }
        if g1 == 0 {
            return None;
        }
        g1 -= 1;
    }
}

/// `\\[A-F0-9]{1,6}\x20(?![a-fA-F0-9\x20])` at `p`; returns the index after the space.
fn match_escape(b: &[u8], p: usize) -> Option<usize> {
    if b.get(p) != Some(&b'\\') {
        return None;
    }
    let mut q = p + 1;
    let mut n = 0;
    while q < b.len() && n < 6 && matches!(b[q], b'A'..=b'F' | b'0'..=b'9') {
        q += 1;
        n += 1;
    }
    // backtrack over the hex count until a space follows that is not followed by hex/space
    while n >= 1 {
        let sp = p + 1 + n;
        if b.get(sp) == Some(&b' ') {
            let next = b.get(sp + 1);
            if !matches!(next, Some(c) if c.is_ascii_hexdigit() || *c == b' ') {
                return Some(sp + 1);
            }
        }
        n -= 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basic() {
        let id = Cssesc::identifier();
        assert_eq!(cssesc("w-[10px]", id), "w-\\[10px\\]");
        assert_eq!(cssesc("md:px-2", id), "md\\:px-2");
        assert_eq!(cssesc("2xl:p-1", id), "\\32xl\\:p-1");
        assert_eq!(cssesc("-mt-1", id), "-mt-1");
        assert_eq!(cssesc("--x", id), "\\--x");
        assert_eq!(cssesc("a b", id), "a\\ b");
        assert_eq!(cssesc("é", id), "\\E9");
        assert_eq!(cssesc("éa", id), "\\E9 a");
        assert_eq!(cssesc("1/2", id), "\\31\\/2");
    }
}
