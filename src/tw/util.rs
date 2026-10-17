//! Ports of tailwind's `src/util/*` helpers.

use std::sync::{LazyLock, OnceLock};

use regex::Regex;

use crate::js::{self, Object, Value};
use crate::selector::{cssesc, Cssesc};

/// JS `\s` as a regex class body
pub const WS: &str = r"\t\n\x0B\x0C\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}";

pub fn re(src: &str) -> Regex {
    Regex::new(&src.replace("\\s", &format!("[{WS}]")).replace("\\S", &format!("[^{WS}]"))).unwrap()
}

// ---- toPath.js

/// `toPath(path)`
pub fn to_path(path: &str) -> Result<Vec<String>, String> {
    let open = path.matches('[').count();
    let close = path.matches(']').count();
    if open != close {
        return Err(format!("Path is invalid. Has unbalanced brackets: {path}"));
    }
    // path.split(/\.(?![^\[]*\])|[\[\]]/g).filter(Boolean)
    let b = path.as_bytes();
    let mut parts = Vec::new();
    let mut last = 0;
    for i in 0..b.len() {
        let c = b[i];
        let split = if c == b'[' || c == b']' {
            true
        } else if c == b'.' {
            // lookahead (?![^\[]*\]): fails when a `]` comes before any `[`
            let mut j = i + 1;
            let mut inside = false;
            while j < b.len() {
                if b[j] == b'[' {
                    break;
                }
                if b[j] == b']' {
                    inside = true;
                    break;
                }
                j += 1;
            }
            !inside
        } else {
            false
        };
        if split {
            parts.push(&path[last..i]);
            last = i + 1;
        }
    }
    parts.push(&path[last..]);
    Ok(parts.into_iter().filter(|p| !p.is_empty()).map(|s| s.to_string()).collect())
}

// ---- negateValue.js

static NEGATABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+-]?([0-9]+|[0-9]*\.[0-9]+)(e[+-]?[0-9]+)?(%|[0-9A-Za-z_]+)?$").unwrap());

/// `negateValue(value)`; `None` is `undefined`
pub fn negate_value(value: &Value) -> Option<String> {
    let value = value.to_js_string();
    if value == "0" {
        return Some("0".into());
    }
    if NEGATABLE.is_match(&value) {
        return Some(if let Some(r) = value.strip_prefix('-') {
            r.to_string()
        } else if let Some(r) = value.strip_prefix('+') {
            format!("-{r}")
        } else {
            format!("-{value}")
        });
    }
    for f in ["var", "calc", "min", "max", "clamp"] {
        if value.contains(&format!("{f}(")) {
            return Some(format!("calc({value} * -1)"));
        }
    }
    None
}

// ---- escapeClassName.js / escapeCommas.js / nameClass.js

pub fn escape_commas(s: &str) -> String {
    s.replace("\\,", "\\2c ")
}

pub fn escape_class_name(class_name: &str) -> String {
    escape_commas(&cssesc(class_name, Cssesc::identifier()))
}

pub fn as_class(name: &str) -> String {
    escape_commas(&format!(".{}", escape_class_name(name)))
}

pub fn format_class(class_prefix: &str, key: &str) -> String {
    if key == "DEFAULT" {
        return class_prefix.to_string();
    }
    if key == "-" || key == "-DEFAULT" {
        return format!("-{class_prefix}");
    }
    if key.starts_with('-') {
        return format!("-{class_prefix}{key}");
    }
    if key.starts_with('/') {
        return format!("{class_prefix}{key}");
    }
    format!("{class_prefix}-{key}")
}

pub fn name_class(class_prefix: &str, key: &str) -> String {
    as_class(&format_class(class_prefix, key))
}

// ---- splitAtTopLevelOnly.js

pub fn split_at_top_level_only<'a>(input: &'a str, separator: &str) -> Vec<&'a str> {
    let mut stack: Vec<u8> = Vec::new();
    let mut parts = Vec::new();
    let mut last_pos = 0;
    let mut is_escaped = false;
    let b = input.as_bytes();
    let sep = separator.as_bytes();
    let first = sep.first().copied();
    let mut idx = 0;
    while idx < b.len() {
        let c = b[idx];
        if stack.is_empty() && Some(c) == first && !is_escaped && (sep.len() == 1 || b[idx..].starts_with(sep)) {
            parts.push(&input[last_pos..idx]);
            last_pos = idx + sep.len();
        }
        is_escaped = if is_escaped { false } else { c == b'\\' };
        if c == b'(' || c == b'[' || c == b'{' {
            stack.push(c);
        } else if (c == b')' && stack.last() == Some(&b'('))
            || (c == b']' && stack.last() == Some(&b'['))
            || (c == b'}' && stack.last() == Some(&b'{'))
        {
            stack.pop();
        }
        idx += 1;
    }
    parts.push(&input[last_pos.min(input.len())..]);
    parts
}

// ---- isSyntacticallyValidPropertyValue.js

pub fn is_syntactically_valid_property_value(value: &str) -> bool {
    let mut stack: Vec<char> = Vec::new();
    let mut in_quotes = false;
    let chars: Vec<char> = value.chars().collect();
    for i in 0..chars.len() {
        let c = chars[i];
        let prev = if i > 0 { Some(chars[i - 1]) } else { None };
        if c == ':' && !in_quotes && stack.is_empty() {
            return false;
        }
        if matches!(c, '"' | '\'' | '`') && prev != Some('\\') {
            in_quotes = !in_quotes;
        }
        if in_quotes {
            continue;
        }
        if prev == Some('\\') {
            continue;
        }
        match c {
            '{' | '[' | '(' => stack.push(c),
            '}' | ']' | ')' => {
                let inverse = match c {
                    '}' => '{',
                    ']' => '[',
                    _ => '(',
                };
                match stack.pop() {
                    None => return false,
                    Some(p) if p != inverse => return false,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    stack.is_empty()
}

// ---- dataTypes.js

const AUTO_VAR_INJECTION_EXCEPTIONS: &[&str] = &[
    "scroll-timeline-name",
    "timeline-scope",
    "view-timeline-name",
    "font-palette",
    "anchor-name",
    "anchor-scope",
    "position-anchor",
    "position-try-options",
    "scroll-timeline",
    "animation-timeline",
    "view-timeline",
    "position-try",
];

static URL_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"url\([^\n\r\x{2028}\x{2029}]*?\)").unwrap());
static UNDERSCORES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([^\\])_+").unwrap());
static MATH_FN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(calc|min|max|clamp)\([^\n\r\x{2028}\x{2029}]+\)").unwrap());

/// `normalize(value, context, isRoot)`
pub fn normalize(value: &str, property: Option<&str>) -> String {
    normalize_inner(value, property, true)
}

fn normalize_inner(value: &str, property: Option<&str>, is_root: bool) -> String {
    let is_var_exception = property.is_some_and(|p| AUTO_VAR_INJECTION_EXCEPTIONS.contains(&p));
    if value.starts_with("--") && !is_var_exception {
        return format!("var({value})");
    }
    if value.contains("url(") {
        // value.split(/(url\(.*?\))/g).filter(Boolean).map(...)
        let mut out = String::new();
        let mut last = 0;
        let mut parts: Vec<(&str, bool)> = Vec::new();
        for m in URL_SPLIT.find_iter(value) {
            parts.push((&value[last..m.start()], false));
            parts.push((m.as_str(), true));
            last = m.end();
        }
        parts.push((&value[last..], false));
        for (part, _) in parts {
            if part.is_empty() {
                continue;
            }
            if part.starts_with("url(") && part.ends_with(')') && URL_SPLIT.find(part).is_some_and(|m| m.start() == 0 && m.end() == part.len()) {
                out.push_str(part);
            } else {
                out.push_str(&normalize_inner(part, property, false));
            }
        }
        return out;
    }
    let mut v = UNDERSCORES
        .replace_all(value, |caps: &regex::Captures| {
            let full = caps.get(0).unwrap().as_str();
            let before = caps.get(1).unwrap().as_str();
            format!("{before}{}", " ".repeat(full.len() - before.len()))
        })
        .into_owned();
    if v.starts_with('_') {
        v.replace_range(0..1, " ");
    }
    v = v.replace("\\_", "_");
    if is_root {
        v = js::trim(&v).to_string();
    }
    normalize_math_operator_spacing(&v)
}

/// `normalizeAttributeSelectors(value)`
pub fn normalize_attribute_selectors(value: &str) -> String {
    if !value.contains('=') {
        return value.to_string();
    }
    // value.replace(/(=.*)/g, …): `.` stops at line terminators
    let mut out = String::new();
    let mut rest = value;
    while let Some(i) = rest.find('=') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find(['\n', '\r', '\u{2028}', '\u{2029}']).unwrap_or(tail.len());
        let m = &tail[..end];
        out.push_str(&normalize_attr_match(m));
        rest = &tail[end..];
        if rest.is_empty() {
            break;
        }
        // the line terminator itself
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out.push_str(rest);
    out
}

fn normalize_attr_match(m: &str) -> String {
    let chars: Vec<char> = m.chars().collect();
    if chars.get(1) == Some(&'\'') || chars.get(1) == Some(&'"') {
        return m.to_string();
    }
    if chars.len() > 2 {
        let trailing = chars[chars.len() - 1];
        if chars[chars.len() - 2] == ' ' && matches!(trailing, 'i' | 'I' | 's' | 'S') {
            let inner: String = chars[1..chars.len() - 2].iter().collect();
            return format!("=\"{inner}\" {trailing}");
        }
    }
    let inner: String = chars[1..].iter().collect();
    format!("=\"{inner}\"")
}

const PREVENT_FORMATTING_KEYWORDS: &[&str] = &[
    "min-content",
    "max-content",
    "fit-content",
    "safe-area-inset-top",
    "safe-area-inset-right",
    "safe-area-inset-bottom",
    "safe-area-inset-left",
    "titlebar-area-x",
    "titlebar-area-y",
    "titlebar-area-width",
    "titlebar-area-height",
    "keyboard-inset-top",
    "keyboard-inset-right",
    "keyboard-inset-bottom",
    "keyboard-inset-left",
    "keyboard-inset-width",
    "keyboard-inset-height",
    "radial-gradient",
    "linear-gradient",
    "conic-gradient",
    "repeating-radial-gradient",
    "repeating-linear-gradient",
    "repeating-conic-gradient",
    "anchor-size",
];

fn normalize_math_operator_spacing(value: &str) -> String {
    MATH_FN
        .replace_all(value, |caps: &regex::Captures| {
            let m: Vec<u16> = caps.get(0).unwrap().as_str().encode_utf16().collect();
            let mut result: Vec<u16> = Vec::new();
            let peek = |i: usize, word: &str| word.encode_utf16().enumerate().all(|(j, c)| m.get(i + j) == Some(&c));
            let consume_until = |i: &mut usize, chars: &[u16]| -> Vec<u16> {
                let mut min_index = usize::MAX;
                for &c in chars {
                    if let Some(idx) = m[*i..].iter().position(|x| *x == c) {
                        let idx = idx + *i;
                        if idx < min_index {
                            min_index = idx;
                        }
                    }
                }
                let end = if min_index == usize::MAX { m.len() } else { min_index };
                let r = m[*i..end].to_vec();
                *i += r.len();
                *i = i.wrapping_sub(1);
                r
            };
            let last_char = |result: &[u16]| -> Option<u16> {
                let s = String::from_utf16_lossy(result);
                let t = s.trim_end_matches(js::is_js_whitespace);
                t.encode_utf16().last()
            };
            let mut i = 0usize;
            while i < m.len() {
                let ch = m[i];
                if peek(i, "var") {
                    let r = consume_until(&mut i, &[b')' as u16, b',' as u16]);
                    result.extend(r);
                } else if let Some(k) = PREVENT_FORMATTING_KEYWORDS.iter().find(|k| peek(i, k)) {
                    result.extend(k.encode_utf16());
                    i += k.len() - 1;
                } else if peek(i, "theme") {
                    let r = consume_until(&mut i, &[b')' as u16]);
                    result.extend(r);
                } else if peek(i, "[") {
                    let r = consume_until(&mut i, &[b']' as u16]);
                    result.extend(r);
                } else if [b'+', b'-', b'*', b'/'].iter().any(|c| *c as u16 == ch)
                    && !last_char(&result).is_some_and(|l| [b'(', b'+', b'-', b'*', b'/', b','].iter().any(|c| *c as u16 == l))
                {
                    result.push(b' ' as u16);
                    result.push(ch);
                    result.push(b' ' as u16);
                } else {
                    result.push(ch);
                }
                i = i.wrapping_add(1);
            }
            let s = String::from_utf16_lossy(&result);
            collapse_ws(&s)
        })
        .into_owned()
}

/// `.replace(/\s+/g, ' ')`
pub fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if js::is_js_whitespace(c) {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

fn is_css_function(value: &str) -> bool {
    for f in ["min", "max", "clamp", "calc"] {
        if let Some(rest) = value.strip_prefix(f) {
            if let Some(rest) = rest.strip_prefix('(') {
                let line_end = rest.find(['\n', '\r', '\u{2028}', '\u{2029}']).unwrap_or(rest.len());
                if rest[..line_end].contains(')') {
                    return true;
                }
            }
        }
    }
    false
}

pub fn dt_url(value: &str) -> bool {
    value.starts_with("url(")
}

pub fn dt_number(value: &str) -> bool {
    !js::string_to_number(value).is_nan() || is_css_function(value)
}

pub fn dt_percentage(value: &str) -> bool {
    (value.ends_with('%') && dt_number(&value[..value.len() - 1])) || is_css_function(value)
}

static LENGTH_RE: LazyLock<Regex> = LazyLock::new(|| {
    let units = [
        "cm", "mm", "Q", "in", "pc", "pt", "px", "em", "ex", "ch", "rem", "lh", "rlh", "vw", "vh", "vmin", "vmax", "vb", "vi", "svw",
        "svh", "lvw", "lvh", "dvw", "dvh", "cqw", "cqh", "cqi", "cqb", "cqmin", "cqmax",
    ];
    // note: the `\.` in the template literal is a plain `.` (any character)
    Regex::new(&format!(r"^[+-]?[0-9]*[^\n\r\x{{2028}}\x{{2029}}]?[0-9]+(?:[eE][+-]?[0-9]+)?(?:{})$", units.join("|"))).unwrap()
});

pub fn dt_length(value: &str) -> bool {
    value == "0" || LENGTH_RE.is_match(value) || is_css_function(value)
}

pub fn dt_line_width(value: &str) -> bool {
    matches!(value, "thin" | "medium" | "thick")
}

pub fn dt_shadow(value: &str) -> bool {
    parse_box_shadow_value(&normalize(value, None)).iter().all(|s| s.valid)
}

pub fn dt_color(value: &str) -> bool {
    let mut colors = 0;
    let ok = split_at_top_level_only(value, "_").into_iter().all(|part| {
        let part = normalize(part, None);
        if part.starts_with("var(") {
            return true;
        }
        if parse_color(&part, true).is_some() {
            colors += 1;
            return true;
        }
        false
    });
    ok && colors > 0
}

pub fn dt_image(value: &str) -> bool {
    let mut images = 0;
    let ok = split_at_top_level_only(value, ",").into_iter().all(|part| {
        let part = normalize(part, None);
        if part.starts_with("var(") {
            return true;
        }
        if dt_url(&part) || dt_gradient(&part) || ["element(", "image(", "cross-fade(", "image-set("].iter().any(|f| part.starts_with(f)) {
            images += 1;
            return true;
        }
        false
    });
    ok && images > 0
}

pub fn dt_gradient(value: &str) -> bool {
    let value = normalize(value, None);
    ["conic-gradient", "linear-gradient", "radial-gradient", "repeating-conic-gradient", "repeating-linear-gradient", "repeating-radial-gradient"]
        .iter()
        .any(|t| value.starts_with(&format!("{t}(")))
}

pub fn dt_position(value: &str) -> bool {
    let mut positions = 0;
    let ok = split_at_top_level_only(value, "_").into_iter().all(|part| {
        let part = normalize(part, None);
        if part.starts_with("var(") {
            return true;
        }
        if matches!(part.as_str(), "center" | "top" | "right" | "bottom" | "left") || dt_length(&part) || dt_percentage(&part) {
            positions += 1;
            return true;
        }
        false
    });
    ok && positions > 0
}

/// `/(['"])([^"']+)\1/` somewhere in the string
fn has_quoted_part(s: &str) -> bool {
    let c: Vec<char> = s.chars().collect();
    for i in 0..c.len() {
        if c[i] == '\'' || c[i] == '"' {
            let mut j = i + 1;
            while j < c.len() && c[j] != '"' && c[j] != '\'' {
                j += 1;
            }
            if j > i + 1 && j < c.len() && c[j] == c[i] {
                return true;
            }
        }
    }
    false
}

pub fn dt_family_name(value: &str) -> bool {
    let mut fonts = 0;
    let ok = split_at_top_level_only(value, ",").into_iter().all(|part| {
        let part = normalize(part, None);
        if part.starts_with("var(") {
            return true;
        }
        if part.contains(' ') && !has_quoted_part(&part) {
            return false;
        }
        if part.starts_with(|c: char| c.is_ascii_digit()) {
            return false;
        }
        fonts += 1;
        true
    });
    ok && fonts > 0
}

pub fn dt_generic_name(value: &str) -> bool {
    matches!(
        value,
        "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy" | "system-ui" | "ui-serif" | "ui-sans-serif" | "ui-monospace" | "ui-rounded" | "math" | "emoji" | "fangsong"
    )
}

pub fn dt_absolute_size(value: &str) -> bool {
    matches!(value, "xx-small" | "x-small" | "small" | "medium" | "large" | "x-large" | "xx-large" | "xxx-large")
}

pub fn dt_relative_size(value: &str) -> bool {
    matches!(value, "larger" | "smaller")
}

/// `validateFormalSyntax.backgroundSize`
pub fn dt_background_size(value: &str) -> bool {
    split_at_top_level_only(value, ",").into_iter().all(|part| {
        let sizes: Vec<&str> = split_at_top_level_only(part, "_").into_iter().filter(|s| !s.is_empty()).collect();
        if sizes.len() == 1 && (sizes[0] == "cover" || sizes[0] == "contain") {
            return true;
        }
        if sizes.len() != 1 && sizes.len() != 2 {
            return false;
        }
        sizes.iter().all(|s| dt_length(s) || dt_percentage(s) || *s == "auto")
    })
}

// ---- color.js

#[derive(Debug, Clone)]
pub struct Color {
    pub mode: String,
    pub color: Vec<String>,
    pub alpha: Option<String>,
}

fn color_names() -> &'static Object {
    static CELL: OnceLock<Value> = OnceLock::new();
    CELL.get_or_init(|| Value::from_json(&serde_json::from_str(include_str!("data/color_names.json")).unwrap()))
        .as_obj()
        .unwrap()
}

static RGB_RE: LazyLock<Regex> = LazyLock::new(|| color_re("(rgba?)", ""));
static HSL_RE: LazyLock<Regex> = LazyLock::new(|| color_re("(hsla?)", "(?:deg|rad|grad|turn)?"));

fn color_re(head: &str, first_unit: &str) -> Regex {
    let value = r"(?:[0-9]+|[0-9]*\.[0-9]+)%?";
    let custom = r"var\(--(?:[^ )]*?)(?:,(?:[^ )]*?|var\(--[^ )]*?\)))?\)";
    let s = format!(r"[{WS}]");
    let sep = format!(r"(?:{s}*,{s}*|{s}+)");
    let alpha_sep = format!(r"{s}*[,/]{s}*");
    let first = format!("((?:{value}){first_unit}|{custom})");
    let part = format!("({value}|{custom})");
    Regex::new(&format!(r"^{head}\({s}*{first}(?:{sep}{part})?(?:{sep}{part})?(?:{alpha_sep}{part})?{s}*\)$")).unwrap()
}

fn hex_pair(s: &str) -> u32 {
    u32::from_str_radix(s, 16).unwrap()
}

/// `parseColor(value, { loose })`
pub fn parse_color(value: &str, loose: bool) -> Option<Color> {
    let value = js::trim(value);
    if value == "transparent" {
        return Some(Color { mode: "rgb".into(), color: vec!["0".into(), "0".into(), "0".into()], alpha: Some("0".into()) });
    }
    if let Some(c) = color_names().get(value) {
        let color = c.as_arr().unwrap().iter().map(|v| v.to_js_string()).collect();
        return Some(Color { mode: "rgb".into(), color, alpha: None });
    }
    // SHORT_HEX expansion, then HEX
    let b = value.as_bytes();
    let is_hex = |s: &[u8]| s.iter().all(|c| c.is_ascii_hexdigit());
    let hex: Option<String> = if b.first() == Some(&b'#') && (b.len() == 4 || b.len() == 5) && is_hex(&b[1..]) {
        let mut s = String::from("#");
        for &c in &b[1..] {
            s.push(c as char);
            s.push(c as char);
        }
        Some(s)
    } else if b.first() == Some(&b'#') && (b.len() == 7 || b.len() == 9) && is_hex(&b[1..]) {
        Some(value.to_string())
    } else {
        None
    };
    if let Some(h) = hex {
        let color = vec![hex_pair(&h[1..3]).to_string(), hex_pair(&h[3..5]).to_string(), hex_pair(&h[5..7]).to_string()];
        let alpha = if h.len() == 9 { Some(js::number_to_string(hex_pair(&h[7..9]) as f64 / 255.0)) } else { None };
        return Some(Color { mode: "rgb".into(), color, alpha });
    }
    let caps = RGB_RE.captures(value).or_else(|| HSL_RE.captures(value))?;
    let get = |i: usize| caps.get(i).map(|m| m.as_str().to_string());
    let color: Vec<String> = [get(2), get(3), get(4)].into_iter().flatten().filter(|s| !s.is_empty()).collect();
    if color.len() == 2 && color[0].starts_with("var(") {
        return Some(Color { mode: get(1).unwrap(), color: vec![color[0].clone()], alpha: Some(color[1].clone()) });
    }
    if !loose && color.len() != 3 {
        return None;
    }
    if color.len() < 3 && !color.iter().any(|p| p.starts_with("var(") && p.ends_with(')')) {
        return None;
    }
    Some(Color { mode: get(1).unwrap(), color, alpha: get(5) })
}

pub fn format_color(c: &Color) -> String {
    if c.mode == "rgba" || c.mode == "hsla" {
        return format!("{}({}{})", c.mode, c.color.join(", "), c.alpha.as_ref().map(|a| format!(", {a}")).unwrap_or_default());
    }
    format!("{}({}{})", c.mode, c.color.join(" "), c.alpha.as_ref().map(|a| format!(" / {a}")).unwrap_or_default())
}

// ---- withAlphaVariable.js / pluginUtils.parseColorFormat / toColorValue

/// `parseColorFormat(value)`: strings with `<alpha-value>` become functions
pub fn parse_color_format(value: &Value) -> Value {
    if let Value::Str(s) = value {
        if s.contains("<alpha-value>") {
            let old = s.clone();
            return Value::func(move |args: &[Value]| {
                let opts = args.first().cloned().unwrap_or(Value::Undefined);
                let ov = opts.get("opacityValue");
                let ov = if ov.is_undefined() { Value::Num(1.0) } else { ov };
                Value::str(old.replace("<alpha-value>", &ov.to_js_string()))
            });
        }
    }
    value.clone()
}

/// `toColorValue(maybeFunction)`
pub fn to_color_value(v: &Value) -> Value {
    if v.is_function() { v.call(&[Object::new().into_value()]) } else { v.clone() }
}

fn opacity_arg(key: &str, value: Value) -> Value {
    let mut o = Object::new();
    o.insert(key, value);
    o.into_value()
}

/// `withAlphaValue(color, alphaValue, defaultValue)`
pub fn with_alpha_value(color: &Value, alpha: &Value, default: &Value) -> Value {
    if color.is_function() {
        return color.call(&[opacity_arg("opacityValue", alpha.clone())]);
    }
    let parsed = match color {
        Value::Str(s) => parse_color(s, true),
        _ => None,
    };
    match parsed {
        None => default.clone(),
        Some(mut p) => {
            p.alpha = if alpha.is_undefined() { None } else { Some(alpha.to_js_string()) };
            Value::str(format_color(&p))
        }
    }
}

/// `withAlphaVariable({ color, property, variable })`
pub fn with_alpha_variable(color: &Value, properties: &[&str], variable: &str) -> Value {
    let mut out = Object::new();
    if color.is_function() {
        out.insert(variable, Value::str("1"));
        for p in properties {
            let mut arg = Object::new();
            arg.insert("opacityVariable", Value::str(variable));
            arg.insert("opacityValue", Value::str(format!("var({variable}, 1)")));
            out.insert(*p, color.call(&[arg.into_value()]));
        }
        return out.into_value();
    }
    let parsed = match color {
        Value::Str(s) => parse_color(s, false),
        _ => None,
    };
    match parsed {
        None => {
            for p in properties {
                out.insert(*p, color.clone());
            }
        }
        Some(p) if p.alpha.is_some() => {
            for pr in properties {
                out.insert(*pr, color.clone());
            }
        }
        Some(mut p) => {
            out.insert(variable, Value::str("1"));
            p.alpha = Some(format!("var({variable}, 1)"));
            let f = format_color(&p);
            for pr in properties {
                out.insert(*pr, Value::str(f.clone()));
            }
        }
    }
    out.into_value()
}

// ---- flattenColorPalette.js

pub fn flatten_color_palette(colors: &Value) -> Object {
    let mut out = Object::new();
    let entries: Vec<(js::Str, Value)> = match colors {
        Value::Obj(o) => o.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Value::Arr(a) => a.iter().enumerate().map(|(i, v)| (i.to_string().into(), v.clone())).collect(),
        Value::Str(_) => {
            let mut o = Object::new();
            super::config::spread(&mut o, colors);
            o.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        }
        _ => Vec::new(),
    };
    for (color, values) in entries {
        if values.is_object_like() || matches!(values, Value::Null) {
            if matches!(values, Value::Null) {
                // typeof null == 'object': Object.entries(null) throws in JS; flattenColorPalette(null) → {}
                continue;
            }
            for (number, hex) in flatten_color_palette(&values).iter() {
                let key = if &**number == "DEFAULT" { color.to_string() } else { format!("{color}-{number}") };
                out.insert(key, hex.clone());
            }
        } else {
            out.insert(color.clone(), values.clone());
        }
    }
    out
}

// ---- transformThemeValue.js

pub fn transform_theme_value(section: &str, value: Value, opts: &Value) -> Value {
    let call = |v: Value| if v.is_function() { v.call(&[Object::new().into_value()]) } else { v };
    match section {
        "fontSize" | "outline" => {
            let v = call(value);
            if let Value::Arr(a) = &v { a.first().cloned().unwrap_or(Value::Undefined) } else { v }
        }
        "fontFamily" => {
            let v = call(value);
            let families = match &v {
                Value::Arr(a) if a.get(1).is_some_and(|x| x.is_object()) => a[0].clone(),
                _ => v.clone(),
            };
            match &families {
                Value::Arr(a) => Value::str(a.iter().map(|x| if x.is_nullish() { String::new() } else { x.to_js_string() }).collect::<Vec<_>>().join(", ")),
                _ => families,
            }
        }
        "boxShadow" | "transitionProperty" | "transitionDuration" | "transitionDelay" | "transitionTimingFunction" | "backgroundImage"
        | "backgroundSize" | "backgroundColor" | "cursor" | "animation" => {
            let v = call(value);
            match &v {
                Value::Arr(a) => Value::str(a.iter().map(|x| if x.is_nullish() { String::new() } else { x.to_js_string() }).collect::<Vec<_>>().join(", ")),
                _ => v,
            }
        }
        "gridTemplateColumns" | "gridTemplateRows" | "objectPosition" => {
            let v = call(value);
            match &v {
                Value::Str(s) => Value::str(crate::postcss::comma(s).join(" ")),
                _ => v,
            }
        }
        _ => {
            if value.is_function() {
                value.call(&[opts.clone()])
            } else {
                value
            }
        }
    }
}

// ---- normalizeScreens.js / buildMediaQuery.js

#[derive(Debug, Clone, PartialEq)]
pub struct ScreenValue {
    pub min: Value,
    pub max: Value,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub name: String,
    pub not: bool,
    pub values: Vec<ScreenValue>,
}

fn resolve_screen_value(o: &Value) -> ScreenValue {
    let min_width = o.get("min-width");
    let min = o.get("min");
    let min = if min.is_undefined() { min_width } else { min };
    ScreenValue { min, max: o.get("max"), raw: o.get("raw") }
}

/// `normalizeScreens(screens)` — `Err` for the tuple syntax
pub fn normalize_screens(screens: &Value) -> Result<Vec<Screen>, String> {
    normalize_screens_inner(screens, true)
}

fn normalize_screens_inner(screens: &Value, root: bool) -> Result<Vec<Screen>, String> {
    match screens {
        Value::Arr(a) => a
            .iter()
            .map(|screen| {
                if root && screen.is_array() {
                    return Err("The tuple syntax is not supported for `screens`.".to_string());
                }
                if let Value::Str(s) = screen {
                    return Ok(Screen {
                        name: s.to_string(),
                        not: false,
                        values: vec![ScreenValue { min: screen.clone(), max: Value::Undefined, raw: Value::Undefined }],
                    });
                }
                let name = screen.get("0").to_js_string();
                let options = screen.get("1");
                Ok(match &options {
                    Value::Str(_) => Screen {
                        name,
                        not: false,
                        values: vec![ScreenValue { min: options.clone(), max: Value::Undefined, raw: Value::Undefined }],
                    },
                    Value::Arr(opts) => Screen { name, not: false, values: opts.iter().map(resolve_screen_value).collect() },
                    _ => Screen { name, not: false, values: vec![resolve_screen_value(&options)] },
                })
            })
            .collect(),
        Value::Obj(o) => {
            let entries: Vec<Value> = o.iter().map(|(k, v)| Value::from(vec![Value::str(k.clone()), v.clone()])).collect();
            normalize_screens_inner(&Value::from(entries), false)
        }
        _ => Ok(Vec::new()),
    }
}

pub fn build_media_query(screens: &[Screen]) -> String {
    screens
        .iter()
        .map(|screen| {
            let values: Vec<String> = screen
                .values
                .iter()
                .map(|v| {
                    if !v.raw.is_undefined() {
                        return v.raw.to_js_string();
                    }
                    let mut parts = Vec::new();
                    if v.min.truthy() {
                        parts.push(format!("(min-width: {})", v.min.to_js_string()));
                    }
                    if v.max.truthy() {
                        parts.push(format!("(max-width: {})", v.max.to_js_string()));
                    }
                    parts.join(" and ")
                })
                .collect();
            // `${values}` joins an array with ","
            if screen.not { format!("not all and {}", values.join(",")) } else { values.join(",") }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---- parseBoxShadowValue.js

#[derive(Debug, Clone, Default)]
pub struct Shadow {
    pub raw: String,
    pub keyword: Option<String>,
    pub x: Option<String>,
    pub y: Option<String>,
    pub blur: Option<String>,
    pub spread: Option<String>,
    pub color: Option<String>,
    pub valid: bool,
}

/// `str.split(/\ +(?![^(]*\))/g)` (and the comma variant): split on runs of `sep` that are not
/// followed by `[^(]*\)`.
pub fn split_outside_parens(s: &str, sep: char, run: bool) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let followed_by_close = |from: usize| {
        for &c in &chars[from..] {
            if c == '(' {
                return false;
            }
            if c == ')' {
                return true;
            }
        }
        false
    };
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == sep {
            let mut j = i + 1;
            if run {
                while j < chars.len() && chars[j] == sep {
                    j += 1;
                }
            }
            if !followed_by_close(j) {
                parts.push(std::mem::take(&mut cur));
                i = j;
                continue;
            }
            // the regex may still match a shorter run starting later in this run; all
            // positions see the same lookahead, so none match
            for k in i..j {
                cur.push(chars[k]);
            }
            i = j;
            continue;
        }
        cur.push(chars[i]);
        i += 1;
    }
    parts.push(cur);
    parts
}

fn is_shadow_length(part: &str) -> bool {
    // /^-?(\d+|\.\d+)(.*?)$/
    let p = part.strip_prefix('-').unwrap_or(part);
    let b = p.as_bytes();
    if b.first().is_some_and(|c| c.is_ascii_digit()) {
        return !part.contains(['\n', '\r', '\u{2028}', '\u{2029}']);
    }
    if b.first() == Some(&b'.') && b.get(1).is_some_and(|c| c.is_ascii_digit()) {
        return !part.contains(['\n', '\r', '\u{2028}', '\u{2029}']);
    }
    false
}

pub fn parse_box_shadow_value(input: &str) -> Vec<Shadow> {
    split_at_top_level_only(input, ",")
        .into_iter()
        .map(|shadow| {
            let value = js::trim(shadow).to_string();
            let mut r = Shadow { raw: value.clone(), ..Default::default() };
            let mut seen_keyword = false;
            let mut unknown_color_seen = false;
            for part in split_outside_parens(&value, ' ', true) {
                if !seen_keyword && matches!(part.as_str(), "inset" | "inherit" | "initial" | "revert" | "unset") {
                    r.keyword = Some(part);
                    seen_keyword = true;
                } else if is_shadow_length(&part) {
                    if r.x.is_none() {
                        r.x = Some(part);
                    } else if r.y.is_none() {
                        r.y = Some(part);
                    } else if r.blur.is_none() {
                        r.blur = Some(part);
                    } else if r.spread.is_none() {
                        r.spread = Some(part);
                    }
                } else if r.color.as_deref().is_none_or(|c| c.is_empty()) {
                    r.color = Some(part);
                } else {
                    unknown_color_seen = true;
                }
            }
            let _ = unknown_color_seen;
            r.valid = r.x.is_some() && r.y.is_some();
            r
        })
        .collect()
}

pub fn format_box_shadow_value(shadows: &[Shadow]) -> String {
    shadows
        .iter()
        .map(|s| {
            if !s.valid {
                return s.raw.clone();
            }
            [&s.keyword, &s.x, &s.y, &s.blur, &s.spread, &s.color]
                .into_iter()
                .filter_map(|p| p.as_ref().filter(|p| !p.is_empty()).cloned())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---- parseAnimationValue.js

#[derive(Debug, Clone, Default)]
pub struct Animation {
    pub value: String,
    pub name: Option<String>,
}

fn is_time(p: &str) -> bool {
    // /^(-?[\d.]+m?s)$/
    let p = p.strip_prefix('-').unwrap_or(p);
    let p = if let Some(r) = p.strip_suffix("ms") {
        r
    } else if let Some(r) = p.strip_suffix('s') {
        r
    } else {
        return false;
    };
    !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit() || c == b'.')
}

pub fn parse_animation_value(input: &str) -> Vec<Animation> {
    split_outside_parens(input, ',', false)
        .into_iter()
        .map(|animation| {
            let value = js::trim(&animation).to_string();
            let mut result = Animation { value: value.clone(), name: None };
            let mut seen: Vec<&str> = Vec::new();
            for part in split_outside_parens(&value, ' ', true) {
                let p = part.as_str();
                if !seen.contains(&"DIRECTIONS") && matches!(p, "normal" | "reverse" | "alternate" | "alternate-reverse") {
                    seen.push("DIRECTIONS");
                } else if !seen.contains(&"PLAY_STATES") && matches!(p, "running" | "paused") {
                    seen.push("PLAY_STATES");
                } else if !seen.contains(&"FILL_MODES") && matches!(p, "none" | "forwards" | "backwards" | "both") {
                    seen.push("FILL_MODES");
                } else if !seen.contains(&"ITERATION_COUNTS") && (p == "infinite" || (!p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))) {
                    seen.push("ITERATION_COUNTS");
                } else if !seen.contains(&"TIMING_FUNCTION")
                    && matches!(p, "linear" | "ease" | "ease-in" | "ease-out" | "ease-in-out" | "step-start" | "step-end")
                {
                    seen.push("TIMING_FUNCTION");
                } else if !seen.contains(&"TIMING_FUNCTION") && (p.starts_with("cubic-bezier(") || p.starts_with("steps(")) {
                    seen.push("TIMING_FUNCTION");
                } else if !seen.contains(&"DURATION") && is_time(p) {
                    seen.push("DURATION");
                } else if !seen.contains(&"DELAY") && is_time(p) {
                    seen.push("DELAY");
                } else if !seen.contains(&"NAME") {
                    result.name = Some(part.clone());
                    seen.push("NAME");
                }
            }
            result
        })
        .collect()
}

// ---- prefixSelector.js (string form)

pub fn prefix_selector(prefix: &str, selector: &str, prepend_negative: bool) -> Result<String, crate::selector::SelError> {
    if prefix.is_empty() {
        return Ok(selector.to_string());
    }
    let mut sel = crate::selector::Sel::new();
    let root = crate::selector::parse(&mut sel, selector)?;
    prefix_selector_ast(&mut sel, root, prefix, prepend_negative);
    Ok(sel.to_string(root))
}

pub fn prefix_selector_ast(sel: &mut crate::selector::Sel, root: crate::selector::SId, prefix: &str, prepend_negative: bool) {
    if prefix.is_empty() {
        return;
    }
    sel.walk_classes(root, &mut |s, n| {
        let base = s[n].val().to_string();
        let v = if prepend_negative && base.starts_with('-') { format!("-{prefix}{}", &base[1..]) } else { format!("{prefix}{base}") };
        s.set_class_value(n, &v);
        crate::selector::SFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(to_path("colors.red.500").unwrap(), ["colors", "red", "500"]);
        assert_eq!(to_path("spacing[2.5]").unwrap(), ["spacing", "2.5"]);
        assert_eq!(to_path("a[b.c].e").unwrap(), ["a", "b.c", "e"]);
        assert_eq!(normalize("calc(100%-10px)", None), "calc(100% - 10px)");
        assert_eq!(normalize("--my-var", None), "var(--my-var)");
        assert_eq!(normalize("a_b\\_c", None), "a b_c");
        assert_eq!(normalize("url(a_b)_c", None), "url(a_b) c");
        assert!(dt_length("10px"));
        assert!(dt_length("1x5px"));
        assert!(!dt_length("10"));
        assert!(dt_color("#fff"));
        assert!(dt_color("rgb(0_0_0)"));
        assert!(!dt_color("10px"));
        assert_eq!(format_color(&parse_color("#0088cc", false).unwrap()), "rgb(0 136 204)");
        assert_eq!(negate_value(&Value::str("1rem")).unwrap(), "-1rem");
        assert_eq!(negate_value(&Value::str("var(--x)")).unwrap(), "calc(var(--x) * -1)");
        assert_eq!(name_class("w", "1/2"), ".w-1\\/2");
        assert_eq!(split_outside_parens("0 0 0 rgb(0, 0, 0)", ' ', true), ["0", "0", "0", "rgb(0, 0, 0)"]);
    }
}
