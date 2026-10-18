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

/// `!?{prefix}{utility}`: the first two patterns at a position from which no separator follows
/// before whitespace (the variant group only matches non-whitespace ending in the separator).
fn bare_source(separator: &str, prefix: &str) -> String {
    let full = &build_sources(separator, prefix)[0];
    let i = full.find(")?!?").expect("variant group") + 2;
    full[i..].to_string()
}

/// `separator` is interpolated into the pattern source as is (`regex.pattern` joins strings)
fn escape_for_pattern(s: &str) -> String {
    s.to_string()
}

pub struct Extractor {
    patterns: Vec<Regex>,
    /// `bare_source` and the separator's first unit, when the separator has no whitespace
    bare: Option<(Regex, u16)>,
    /// default separator and prefix: `Scan` can stand in for the regexes
    scan: bool,
}

impl Extractor {
    pub fn new(separator: &str, prefix: &str) -> Extractor {
        let bare = (!separator.is_empty() && !separator.chars().any(crate::js::is_js_whitespace))
            .then(|| (Regex::new(&bare_source(separator, prefix), false), separator.encode_utf16().next().unwrap()));
        let scan = separator == ":" && prefix.is_empty();
        Extractor { patterns: build_sources(separator, prefix).iter().map(|s| Regex::new(s, false)).collect(), bare, scan }
    }

    /// `line.match(full)` for patterns 0/1, running `bare` wherever `has_sep[pos]` is false.
    fn find_all_hybrid(full: &Regex, bare: &Regex, line: &[u16], has_sep: &[bool], scan: Option<(&Scan, usize)>) -> Vec<(usize, usize)> {
        let (mut tf, mut tb) = (None, None);
        let mut out = Vec::new();
        let n = line.len();
        let mut p = 0;
        while p <= n {
            if let Some((sc, k)) = scan {
                if sc.simple(p) {
                    match sc.full(k, p) {
                        Some(e) => {
                            out.push((p, e));
                            p = e;
                        }
                        None => p += 1,
                    }
                    continue;
                }
            }
            let (re, t) = if has_sep[p] { (full, &mut tf) } else { (bare, &mut tb) };
            let t = t.get_or_insert_with(|| Tables::new(line));
            if re.may_start(t, p) {
                if let Some(e) = re.match_at(t, p) {
                    out.push((p, e));
                    p = if e == p { e + 1 } else { e };
                    continue;
                }
            }
            p += 1;
        }
        out
    }

    pub fn default_extractor() -> &'static Extractor {
        static CELL: OnceLock<Extractor> = OnceLock::new();
        CELL.get_or_init(|| Extractor::new(":", ""))
    }

    /// The extractor function itself: results for one string (with duplicates), as UTF-16
    /// ranges.
    fn extract_line(&self, line: &[u16], reference: bool) -> Vec<(usize, usize)> {
        let mut results: Vec<(usize, usize)> = Vec::new();
        let scan = (self.scan && !reference).then(|| Scan::new(line));
        // has_sep[p]: the separator's first unit occurs in line[p..] before whitespace
        let has_sep: Option<Vec<bool>> = self.bare.as_ref().map(|&(_, c)| {
            let mut v = vec![false; line.len() + 1];
            for i in (0..line.len()).rev() {
                let u = line[i];
                v[i] = !is_ws(u) && (u == c || v[i + 1]);
            }
            v
        });
        for (k, p) in self.patterns.iter().enumerate() {
            let found = match (&self.bare, &has_sep) {
                _ if reference => p.find_all_units(&mut Tables::new(line)),
                // without a separator anywhere, pattern 1 only repeats pattern 0's results
                (Some(_), Some(h)) if k == 1 && !h.iter().any(|&b| b) => continue,
                (Some((bare, _)), Some(h)) if k < 2 => Self::find_all_hybrid(p, bare, line, h, scan.as_ref().map(|s| (s, k))),
                _ if k == 2 && scan.is_some() => scan.as_ref().unwrap().fallback(),
                // run-end tables are per regex (repeat ids are local to a program)
                _ => p.find_all_units(&mut Tables::new(line)),
            };
            for (s, e) in found {
                let clipped = clip_at_balanced_parens(&line[s..e]);
                results.push((s, s + clipped));
            }
        }
        let snapshot = results.clone();
        for (s, e) in snapshot {
            if !line[s..e].contains(&(b'.' as u16)) {
                results.push((s, e));
                continue;
            }
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
    fn line_ranges(&self, line: &str, units: &mut Vec<u16>, offsets: &mut Vec<usize>, reference: bool) -> Vec<(u32, u32)> {
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
        for (s, e) in self.extract_line(units, reference) {
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

    /// `extract` through the regex engine only (the reference for the scanners).
    #[doc(hidden)]
    pub fn extract_reference<'a>(&self, source: &'a str) -> Vec<&'a str> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let (mut units, mut offsets) = (Vec::new(), Vec::new());
        let mut seen_lines = std::collections::HashSet::new();
        for line in source.split('\n') {
            let line = crate::js::trim(line);
            if !seen_lines.insert(line) {
                continue;
            }
            for (bs, be) in self.line_ranges(line, &mut units, &mut offsets, true) {
                let cand = &line[bs as usize..be as usize];
                if seen.insert(cand) {
                    out.push(cand);
                }
            }
        }
        out
    }

    fn extract_with_cache<'a>(&self, source: &'a str, mut cache: Option<&mut LineCache>) -> Vec<&'a str> {
        let mut seen_lines: crate::js::FxHashSet<&str> = Default::default();
        let mut seen: crate::js::FxHashSet<&str> = Default::default();
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
                        let r: std::rc::Rc<[(u32, u32)]> = self.line_ranges(line, &mut units, &mut offsets, false).into();
                        c.insert(line, r.clone());
                        r
                    }
                },
                None => self.line_ranges(line, &mut units, &mut offsets, false).into(),
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

/// Hand-written matchers for the default extractor's regexes (separator `:`, no prefix) at
/// positions whose whitespace-delimited run has no `[`: there the bracket alternatives cannot
/// match and the rest has no backtracking choices.
struct Scan<'a> {
    line: &'a [u16],
    flags: Vec<u8>,
    ws_end: Vec<u32>,
    next_bracket: Vec<u32>,
    /// end of `[^\s"'`\[\\]` / `[^\s`\[\\]` runs (the variant classes of patterns 0 / 1)
    vp_end: [Vec<u32>; 2],
    /// last `:` before each index (`u32::MAX` if none)
    prev_colon: Vec<u32>,
    word_end: Vec<u32>,
    /// end of `[^\s'"`\\$={><]` runs
    s3_end: Vec<u32>,
}

// per-unit classes
const WS: u8 = 1;
const WORD: u8 = 2;
/// not in `[^\s"'`\[\\]`
const VP0_STOP: u8 = 4;
/// not in `[^\s`\[\\]`
const VP1_STOP: u8 = 8;
/// not in `[^\s'"`\\$={><]`
const S3_STOP: u8 = 16;
/// not in `[^<>"'`\s(){}[\]#=%$]`
const MID_STOP: u8 = 32;

static ASCII_FLAGS: [u8; 128] = {
    let mut t = [0u8; 128];
    let mut i = 0;
    while i < 128 {
        let c = i as u8;
        let mut f = 0;
        if matches!(c, b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b' ') {
            f |= WS | VP0_STOP | VP1_STOP | S3_STOP | MID_STOP;
        }
        if c.is_ascii_alphanumeric() || c == b'_' {
            f |= WORD;
        }
        if matches!(c, b'"' | b'\'' | b'`' | b'[' | b'\\') {
            f |= VP0_STOP;
        }
        if matches!(c, b'`' | b'[' | b'\\') {
            f |= VP1_STOP;
        }
        if matches!(c, b'\'' | b'"' | b'`' | b'\\' | b'$' | b'=' | b'{' | b'>' | b'<') {
            f |= S3_STOP;
        }
        if matches!(c, b'<' | b'>' | b'"' | b'\'' | b'`' | b'(' | b')' | b'{' | b'}' | b'[' | b']' | b'#' | b'=' | b'%' | b'$') {
            f |= MID_STOP;
        }
        t[i] = f;
        i += 1;
    }
    t
};

fn unit_flags(u: u16) -> u8 {
    if u < 128 {
        ASCII_FLAGS[u as usize]
    } else if is_ws(u) {
        WS | VP0_STOP | VP1_STOP | S3_STOP | MID_STOP
    } else {
        0
    }
}

fn is_ws(u: u16) -> bool {
    matches!(u, 0x09..=0x0d | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
}

fn is_word(u: u16) -> bool {
    u < 128 && ASCII_FLAGS[u as usize] & WORD != 0
}

/// For each index, the last index before it where `hit` holds (`u32::MAX` if none).
fn prev_hits(line: &[u16], hit: impl Fn(u16) -> bool) -> Vec<u32> {
    let mut v = vec![u32::MAX; line.len() + 1];
    for i in 0..line.len() {
        v[i + 1] = if hit(line[i]) { i as u32 } else { v[i] };
    }
    v
}

impl<'a> Scan<'a> {
    fn new(line: &'a [u16]) -> Scan<'a> {
        let n = line.len();
        let flags: Vec<u8> = line.iter().map(|&u| unit_flags(u)).collect();
        let end = vec![n as u32; n + 2];
        let (mut ws_end, mut next_bracket, mut vp0, mut vp1, mut word_end, mut s3_end) =
            (end.clone(), end.clone(), end.clone(), end.clone(), end.clone(), end);
        for i in (0..n).rev() {
            let (f, x) = (flags[i], i as u32);
            ws_end[i] = if f & WS != 0 { x } else { ws_end[i + 1] };
            next_bracket[i] = if line[i] == b'[' as u16 { x } else { next_bracket[i + 1] };
            vp0[i] = if f & VP0_STOP != 0 { x } else { vp0[i + 1] };
            vp1[i] = if f & VP1_STOP != 0 { x } else { vp1[i + 1] };
            word_end[i] = if f & WORD == 0 { x } else { word_end[i + 1] };
            s3_end[i] = if f & S3_STOP != 0 { x } else { s3_end[i + 1] };
        }
        Scan {
            line,
            flags,
            ws_end,
            next_bracket,
            vp_end: [vp0, vp1],
            prev_colon: prev_hits(line, |u| u == b':' as u16),
            word_end,
            s3_end,
        }
    }

    fn at(&self, i: usize) -> u16 {
        self.line.get(i).copied().unwrap_or(0xffff)
    }

    /// No `[` before the next whitespace.
    fn simple(&self, p: usize) -> bool {
        self.next_bracket[p] >= self.ws_end[p]
    }

    /// `(?:-?\w+|@\w+)(?:[-\/][^\s'"`\\$={><]*)?` (the bracket alternatives cannot match)
    fn utility_body(&self, i: usize) -> Option<usize> {
        let w = |j: usize| self.word_end[j] as usize;
        let start = match self.at(i) {
            0x2d | 0x40 if w(i + 1) > i + 1 => i + 1, // `-` / `@`
            u if is_word(u) => i,
            _ => return None,
        };
        let e = w(start);
        Some(if matches!(self.at(e), 0x2d | 0x2f) { self.s3_end[e + 1] as usize } else { e })
    }

    /// `!?{utility}`
    fn utility(&self, i: usize) -> Option<usize> {
        if self.at(i) == b'!' as u16 {
            if let Some(e) = self.utility_body(i + 1) {
                return Some(e);
            }
        }
        self.utility_body(i)
    }

    /// Pattern 0 or 1 at `p` (`simple(p)` holds): the only variant alternative that can match is
    /// `[^…]+:`, which takes the run up to its last `:` once.
    fn full(&self, k: usize, p: usize) -> Option<usize> {
        let r = self.vp_end[k][p] as usize;
        let j = self.prev_colon[r];
        if j != u32::MAX && j as usize > p {
            if let Some(e) = self.utility(j as usize + 1) {
                return Some(e);
            }
        }
        self.utility(p)
    }

    /// Pattern 2: `[^<>"'`\s.(){}[\]#=%$][^<>"'`\s(){}[\]#=%$]*[^<>"'`\s.(){}[\]#=%:$]`, global.
    fn fallback(&self) -> Vec<(usize, usize)> {
        let (line, n) = (self.line, self.line.len());
        let mid = |i: usize| self.flags[i] & MID_STOP == 0;
        let mut mid_end = vec![n as u32; n + 2];
        for i in (0..n).rev() {
            mid_end[i] = if mid(i) { mid_end[i + 1] } else { i as u32 };
        }
        let mut prev_last = vec![u32::MAX; n + 1];
        for i in 0..n {
            prev_last[i + 1] = if mid(i) && line[i] != b'.' as u16 && line[i] != b':' as u16 { i as u32 } else { prev_last[i] };
        }
        let mut out = Vec::new();
        let mut p = 0;
        while p < n {
            if mid(p) && line[p] != b'.' as u16 {
                let q = prev_last[mid_end[p + 1] as usize];
                if q != u32::MAX && q as usize > p {
                    out.push((p, q as usize + 1));
                    p = q as usize + 1;
                    continue;
                }
            }
            p += 1;
        }
        out
    }
}

/// Per-thread cache of extractor results by line (tailwind keeps a similar `extractorCache`):
/// accounts repeat a lot of lines across files.
pub struct LineCache {
    map: crate::js::FxHashMap<Box<str>, std::rc::Rc<[(u32, u32)]>>,
    bytes: usize,
}

const LINE_CACHE_BYTES: usize = 64 << 20;

impl LineCache {
    fn new() -> LineCache {
        LineCache { map: Default::default(), bytes: 0 }
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
    if !input.windows(2).any(|w| w[0] == b'-' as u16 && w[1] == b'[' as u16) {
        return input.len();
    }
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
