//! `lib/defaultExtractor.js` + `lib/regex.js`, applied the way `expandTailwindAtRules`
//! (`getClassCandidates`) applies it: per trimmed line, deduplicated, without `!*`.

use std::sync::OnceLock;

use crate::jsregex::{Regex, Tables};

/// `regex.escape(string)`
fn escape(s: &str) -> String {
    let special = |c: char| matches!(c, '\\' | '^' | '$' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|');
    if s.chars().any(special) {
        let mut out = String::new();
        for c in s.chars() {
            if special(c) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    } else {
        s.to_string()
    }
}

fn any(sources: &[&str]) -> String {
    format!("(?:{})", sources.join("|"))
}

fn optional(source: &str) -> String {
    format!("(?:{source})?")
}

/// `buildRegExps(context)` → regex sources
pub fn build_sources(separator: &str, prefix: &str) -> Vec<String> {
    let sep = escape_for_pattern(separator);
    let prefix = if prefix.is_empty() { String::new() } else { optional(&format!("-?{}", escape(prefix))) };
    let utility = any(&[
        r#"\[[^\s:'"`]+:[^\s\[\]]+\]"#,
        r#"\[[^\s:'"`\]]+:[^\s]+?\[[^\s]+\][^\s]+?\]"#,
        &format!(
            "{}{}",
            any(&[r"-?(?:\w+)", r"@(?:\w+)"]),
            optional(&any(&[
                &format!(
                    "{}{}{}",
                    any(&[
                        r#"-(?:\w+-)*\['[^\s]+'\]"#,
                        r#"-(?:\w+-)*\["[^\s]+"\]"#,
                        r#"-(?:\w+-)*\[`[^\s]+`\]"#,
                        r#"-(?:\w+-)*\[(?:[^\s\[\]]+\[[^\s\[\]]+\])*[^\s:\[\]]+\]"#,
                    ]),
                    r"(?![{([]])",
                    r#"(?:\/[^\s'"`\\><$]*)?"#
                ),
                &format!(
                    "{}{}{}",
                    any(&[
                        r#"-(?:\w+-)*\['[^\s]+'\]"#,
                        r#"-(?:\w+-)*\["[^\s]+"\]"#,
                        r#"-(?:\w+-)*\[`[^\s]+`\]"#,
                        r#"-(?:\w+-)*\[(?:[^\s\[\]]+\[[^\s\[\]]+\])*[^\s\[\]]+\]"#,
                    ]),
                    r"(?![{([]])",
                    r#"(?:\/[^\s'"`\\$]*)?"#
                ),
                r#"[-\/][^\s'"`\\$={><]*"#,
            ]))
        ),
    ]);
    let variant_patterns = [
        any(&[
            &format!(r#"@\[[^\s"'`]+\](\/[^\s"'`]+)?{sep}"#),
            &format!(r#"([^\s"'`\[\\]+-)?\[[^\s"'`]+\]\/[\w_-]+{sep}"#),
            &format!(r#"([^\s"'`\[\\]+-)?\[[^\s"'`]+\]{sep}"#),
            &format!(r#"[^\s"'`\[\\]+{sep}"#),
        ]),
        any(&[
            &format!(r#"([^\s"'`\[\\]+-)?\[[^\s`]+\]\/[\w_-]+{sep}"#),
            &format!(r#"([^\s"'`\[\\]+-)?\[[^\s`]+\]{sep}"#),
            &format!(r#"[^\s`\[\\]+{sep}"#),
        ]),
    ];
    let mut out = Vec::new();
    for vp in variant_patterns {
        out.push(format!("((?=(({vp})+))\\2)?!?{prefix}{utility}"));
    }
    out.push(r#"[^<>"'`\s.(){}[\]#=%$][^<>"'`\s(){}[\]#=%$]*[^<>"'`\s.(){}[\]#=%:$]"#.to_string());
    out
}

/// `separator` is interpolated into the pattern source as is (`regex.pattern` joins strings)
fn escape_for_pattern(s: &str) -> String {
    s.to_string()
}

pub struct Extractor {
    patterns: Vec<Regex>,
}

impl Extractor {
    pub fn new(separator: &str, prefix: &str) -> Extractor {
        Extractor { patterns: build_sources(separator, prefix).iter().map(|s| Regex::new(s, false)).collect() }
    }

    pub fn default_extractor() -> &'static Extractor {
        static CELL: OnceLock<Extractor> = OnceLock::new();
        CELL.get_or_init(|| Extractor::new(":", ""))
    }

    /// The extractor function itself: results for one string (with duplicates), as UTF-16
    /// ranges.
    fn extract_line(&self, line: &[u16]) -> Vec<(usize, usize)> {
        let mut results: Vec<(usize, usize)> = Vec::new();
        for p in &self.patterns {
            // run-end tables are per regex (repeat ids are local to a program)
            let mut tables = Tables::new(line);
            for (s, e) in p.find_all_units(&mut tables) {
                let clipped = clip_at_balanced_parens(&line[s..e]);
                results.push((s, s + clipped));
            }
        }
        let snapshot = results.clone();
        for (s, e) in snapshot {
            // splitAtTopLevelOnly(result, '.') on code units
            let seg = split_units_top_level(&line[s..e], b'.' as u16);
            let mut idx = 0;
            while idx < seg.len() {
                let (a, b) = seg[idx];
                if idx + 1 >= seg.len() {
                    results.push((s + a, s + b));
                    idx += 1;
                    continue;
                }
                let (na, nb) = seg[idx + 1];
                let next = crate::js::string_to_number(&String::from_utf16_lossy(&line[s + na..s + nb]));
                if next.is_nan() {
                    results.push((s + a, s + b));
                } else {
                    idx += 1;
                }
                idx += 1;
            }
        }
        results
    }

    /// Byte ranges (relative to the trimmed line) of the candidates in one line, in extractor
    /// order with duplicates, `!*` and unrepresentable ranges removed.
    fn line_ranges(&self, line: &str, units: &mut Vec<u16>, offsets: &mut Vec<usize>) -> Vec<(u32, u32)> {
        units.clear();
        offsets.clear();
        // unit index → byte offset (usize::MAX inside a surrogate pair)
        for (b, c) in line.char_indices() {
            let mut buf = [0u16; 2];
            let enc = c.encode_utf16(&mut buf);
            units.extend_from_slice(enc);
            offsets.push(b);
            if enc.len() == 2 {
                offsets.push(usize::MAX);
            }
        }
        offsets.push(line.len());
        let mut out = Vec::new();
        for (s, e) in self.extract_line(units) {
            let (bs, be) = (offsets[s], offsets[e]);
            if bs == usize::MAX || be == usize::MAX {
                // splits a surrogate pair: not representable as &str
                continue;
            }
            if &line[bs..be] == "!*" {
                continue;
            }
            out.push((bs as u32, be as u32));
        }
        out
    }

    /// Candidates in `source`, deduplicated in first-seen order.
    pub fn extract<'a>(&self, source: &'a str) -> Vec<&'a str> {
        self.extract_with_cache(source, None)
    }

    fn extract_with_cache<'a>(&self, source: &'a str, mut cache: Option<&mut LineCache>) -> Vec<&'a str> {
        let mut seen_lines: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut out = Vec::new();
        let mut units: Vec<u16> = Vec::new();
        let mut offsets: Vec<usize> = Vec::new();
        for line in source.split('\n') {
            let line = crate::js::trim(line);
            if !seen_lines.insert(line) {
                continue;
            }
            let ranges: std::rc::Rc<[(u32, u32)]> = match cache.as_deref_mut() {
                Some(c) => match c.get(line) {
                    Some(r) => r,
                    None => {
                        let r: std::rc::Rc<[(u32, u32)]> = self.line_ranges(line, &mut units, &mut offsets).into();
                        c.insert(line, r.clone());
                        r
                    }
                },
                None => self.line_ranges(line, &mut units, &mut offsets).into(),
            };
            for &(bs, be) in ranges.iter() {
                let cand = &line[bs as usize..be as usize];
                if seen.insert(cand) {
                    out.push(cand);
                }
            }
        }
        out
    }
}

/// Per-thread cache of extractor results by line (tailwind keeps a similar `extractorCache`):
/// accounts repeat a lot of lines across files.
pub struct LineCache {
    map: std::collections::HashMap<Box<str>, std::rc::Rc<[(u32, u32)]>>,
    bytes: usize,
}

const LINE_CACHE_BYTES: usize = 64 << 20;

impl LineCache {
    fn new() -> LineCache {
        LineCache { map: std::collections::HashMap::new(), bytes: 0 }
    }
    fn get(&self, line: &str) -> Option<std::rc::Rc<[(u32, u32)]>> {
        self.map.get(line).cloned()
    }
    fn insert(&mut self, line: &str, r: std::rc::Rc<[(u32, u32)]>) {
        if self.bytes + line.len() > LINE_CACHE_BYTES {
            self.map.clear();
            self.bytes = 0;
        }
        self.bytes += line.len() + r.len() * 8 + 32;
        self.map.insert(line.into(), r);
    }
}

thread_local! {
    static CACHE: std::cell::RefCell<LineCache> = std::cell::RefCell::new(LineCache::new());
}

fn split_units_top_level(input: &[u16], sep: u16) -> Vec<(usize, usize)> {
    let mut stack: Vec<u16> = Vec::new();
    let mut parts = Vec::new();
    let mut last = 0;
    let mut escaped = false;
    for (i, &c) in input.iter().enumerate() {
        if stack.is_empty() && c == sep && !escaped {
            parts.push((last, i));
            last = i + 1;
        }
        escaped = if escaped { false } else { c == b'\\' as u16 };
        let open = |c: u16| c == b'(' as u16 || c == b'[' as u16 || c == b'{' as u16;
        if open(c) {
            stack.push(c);
        } else if (c == b')' as u16 && stack.last() == Some(&(b'(' as u16)))
            || (c == b']' as u16 && stack.last() == Some(&(b'[' as u16)))
            || (c == b'}' as u16 && stack.last() == Some(&(b'{' as u16)))
        {
            stack.pop();
        }
    }
    parts.push((last, input.len()));
    parts
}

/// `clipAtBalancedParens(input)` → clipped length in chars
fn clip_at_balanced_parens(input: &[u16]) -> usize {
    let input: Vec<char> = input.iter().map(|u| char::from_u32(*u as u32).unwrap_or('\u{fffd}')).collect();
    let input = &input[..];
    let has = input.windows(2).any(|w| w[0] == '-' && w[1] == '[');
    if !has {
        return input.len();
    }
    let special = |c: char| matches!(c, '[' | ']' | '\'' | '"' | '`');
    // matchAll(/([\[\]'"`])([^\[\]'"`])?/g) flattened into (char?, index)
    let mut matches: Vec<(Option<char>, usize)> = Vec::new();
    let mut i = 0;
    while i < input.len() {
        if special(input[i]) {
            matches.push((Some(input[i]), i));
            if i + 1 < input.len() && !special(input[i + 1]) {
                matches.push((Some(input[i + 1]), i + 1));
                i += 2;
            } else {
                matches.push((None, i + 1));
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    let mut depth: i32 = 0;
    let mut open: Vec<char> = Vec::new();
    for (ch, index) in matches {
        let in_string = open.last().copied();
        if ch.is_some() && ch == in_string {
            open.pop();
        } else if matches!(ch, Some('\'' | '"' | '`')) {
            open.push(ch.unwrap());
        }
        if in_string.is_some() {
            continue;
        } else if ch == Some('[') {
            depth += 1;
            continue;
        } else if ch == Some(']') {
            depth -= 1;
            continue;
        }
        if depth < 0 {
            return index.saturating_sub(1);
        }
        // ALLOWED_CLASS_CHARACTERS.test(char): `undefined` stringifies to an allowed word
        let allowed = match ch {
            None => true,
            Some(c) => !matches!(c, '"' | '\'' | '`' | '<' | '>' | ']') && !crate::js::is_js_whitespace(c),
        };
        if depth == 0 && !allowed {
            return index;
        }
    }
    input.len()
}

/// Port of tailwind's default extractor (default `prefix` and `separator`): candidates in
/// arbitrary source text (tsx, ts, vue, html, …).
pub fn extract_candidates(source: &str) -> Vec<&str> {
    CACHE.with(|c| Extractor::default_extractor().extract_with_cache(source, Some(&mut c.borrow_mut())))
}
