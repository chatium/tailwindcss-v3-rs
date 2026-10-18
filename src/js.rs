//! A small model of JavaScript values, as tailwind's config and plugin code sees them.
//!
//! Objects keep JS property order: integer-like keys ("0", "50", "100") come first in
//! ascending numeric order, then string keys in insertion order. Theme sections, screens
//! and keyframes are iterated in that order by tailwind, so it shows up in the output.

use std::fmt;
use std::sync::Arc;

pub type Str = Arc<str>;

pub type FnPtr = Arc<dyn Fn(&[Value]) -> Value + Send + Sync>;

#[derive(Clone)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(Str),
    Arr(Arc<Vec<Value>>),
    Obj(Arc<Object>),
    Func(FnPtr),
}

impl Default for Value {
    fn default() -> Self {
        Value::Undefined
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Undefined => write!(f, "undefined"),
            Value::Null => write!(f, "null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Num(n) => write!(f, "{}", number_to_string(*n)),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Arr(a) => f.debug_list().entries(a.iter()).finish(),
            Value::Obj(o) => f.debug_map().entries(o.iter().map(|(k, v)| (k, v))).finish(),
            Value::Func(_) => write!(f, "[Function]"),
        }
    }
}

impl PartialEq for Value {
    /// `===` semantics (objects/functions by identity).
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Num(a), Value::Num(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Arr(a), Value::Arr(b)) => Arc::ptr_eq(a, b),
            (Value::Obj(a), Value::Obj(b)) => Arc::ptr_eq(a, b),
            (Value::Func(a), Value::Func(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// Structural equality: same JSON-like shape, keys in the same order (functions by identity).
pub fn deep_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Arr(x), Value::Arr(y)) => Arc::ptr_eq(x, y) || (x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| deep_eq(p, q))),
        (Value::Obj(x), Value::Obj(y)) => {
            Arc::ptr_eq(x, y) || (x.len() == y.len() && x.iter().zip(y.iter()).all(|((k1, v1), (k2, v2))| k1 == k2 && deep_eq(v1, v2)))
        }
        (Value::Num(x), Value::Num(y)) => x.to_bits() == y.to_bits() || x == y,
        _ => a == b,
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.into())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s.into())
    }
}
impl From<Str> for Value {
    fn from(s: Str) -> Self {
        Value::Str(s)
    }
}
impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Num(n)
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<Object> for Value {
    fn from(o: Object) -> Self {
        Value::Obj(Arc::new(o))
    }
}
impl From<Vec<Value>> for Value {
    fn from(a: Vec<Value>) -> Self {
        Value::Arr(Arc::new(a))
    }
}

impl Value {
    pub fn func(f: impl Fn(&[Value]) -> Value + Send + Sync + 'static) -> Value {
        Value::Func(Arc::new(f))
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }
    pub fn is_nullish(&self) -> bool {
        matches!(self, Value::Undefined | Value::Null)
    }
    pub fn is_function(&self) -> bool {
        matches!(self, Value::Func(_))
    }
    pub fn is_array(&self) -> bool {
        matches!(self, Value::Arr(_))
    }
    /// `isPlainObject`
    pub fn is_object(&self) -> bool {
        matches!(self, Value::Obj(_))
    }
    pub fn is_string(&self) -> bool {
        matches!(self, Value::Str(_))
    }
    /// `typeof v === 'object' && v !== null`
    pub fn is_object_like(&self) -> bool {
        matches!(self, Value::Obj(_) | Value::Arr(_))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&Object> {
        match self {
            Value::Obj(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn call(&self, args: &[Value]) -> Value {
        match self {
            Value::Func(f) => f(args),
            _ => panic!("not a function: {self:?}"),
        }
    }

    /// JS truthiness
    pub fn truthy(&self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0 && !n.is_nan(),
            Value::Str(s) => !s.is_empty(),
            _ => true,
        }
    }

    /// `String(v)` / template literal interpolation
    pub fn to_js_string(&self) -> String {
        match self {
            Value::Undefined => "undefined".into(),
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Num(n) => number_to_string(*n),
            Value::Str(s) => s.to_string(),
            Value::Arr(a) => a
                .iter()
                .map(|v| if v.is_nullish() { String::new() } else { v.to_js_string() })
                .collect::<Vec<_>>()
                .join(","),
            Value::Obj(_) => "[object Object]".into(),
            Value::Func(_) => "function () { [native code] }".into(),
        }
    }

    /// Property access `v[key]` for objects and arrays (no prototype chain).
    pub fn get(&self, key: &str) -> Value {
        match self {
            Value::Obj(o) => o.get(key).cloned().unwrap_or(Value::Undefined),
            Value::Arr(a) => {
                if key == "length" {
                    return Value::Num(a.len() as f64);
                }
                match array_index(key) {
                    Some(i) => a.get(i as usize).cloned().unwrap_or(Value::Undefined),
                    None => Value::Undefined,
                }
            }
            Value::Str(s) => {
                if key == "length" {
                    return Value::Num(s.encode_utf16().count() as f64);
                }
                match array_index(key) {
                    Some(i) => s
                        .encode_utf16()
                        .nth(i as usize)
                        .map(|c| Value::Str(String::from_utf16_lossy(&[c]).into()))
                        .unwrap_or(Value::Undefined),
                    None => Value::Undefined,
                }
            }
            _ => Value::Undefined,
        }
    }

    pub fn str(s: impl Into<Str>) -> Value {
        Value::Str(s.into())
    }

    pub fn from_json(v: &serde_json::Value) -> Value {
        match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => Value::Num(n.as_f64().unwrap_or(f64::NAN)),
            serde_json::Value::String(s) => Value::Str(s.as_str().into()),
            serde_json::Value::Array(a) => Value::Arr(Arc::new(a.iter().map(Value::from_json).collect())),
            serde_json::Value::Object(o) => {
                let mut obj = Object::new();
                for (k, v) in o {
                    obj.insert(k.as_str(), Value::from_json(v));
                }
                Value::Obj(Arc::new(obj))
            }
        }
    }

    /// Deep structural copy (functions shared), like `cloneDeep`.
    pub fn clone_deep(&self) -> Value {
        match self {
            Value::Arr(a) => Value::Arr(Arc::new(a.iter().map(|v| v.clone_deep()).collect())),
            Value::Obj(o) => {
                let mut n = Object::new();
                for (k, v) in o.iter() {
                    n.insert(k.clone(), v.clone_deep());
                }
                Value::Obj(Arc::new(n))
            }
            v => v.clone(),
        }
    }
}

/// A canonical array index ("0".."4294967294") — those keys are ordered first in JS objects.
pub fn array_index(key: &str) -> Option<u32> {
    let b = key.as_bytes();
    if b.is_empty() || b.len() > 10 {
        return None;
    }
    if b[0] == b'0' && b.len() > 1 {
        return None;
    }
    if !b.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u64 = key.parse().ok()?;
    if n < 4294967295 { Some(n as u32) } else { None }
}

/// A JS plain object with JS property ordering.
#[derive(Clone, Default)]
pub struct Object {
    /// integer-like keys first (ascending), then the rest in insertion order
    entries: Vec<(Str, Value)>,
    n_int: usize,
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter().map(|(k, v)| (k, v))).finish()
    }
}

impl Object {
    pub fn new() -> Self {
        Object { entries: Vec::new(), n_int: 0 }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn position(&self, key: &str) -> Option<usize> {
        if let Some(idx) = array_index(key) {
            self.entries[..self.n_int]
                .binary_search_by(|(k, _)| array_index(k).unwrap().cmp(&idx))
                .ok()
        } else {
            self.entries[self.n_int..].iter().position(|(k, _)| &**k == key).map(|p| p + self.n_int)
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.position(key).map(|i| &self.entries[i].1)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.position(key).map(move |i| &mut self.entries[i].1)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.position(key).is_some()
    }

    /// `obj[key] = value`: existing keys keep their position.
    pub fn insert(&mut self, key: impl Into<Str>, value: Value) {
        let key: Str = key.into();
        if let Some(i) = self.position(&key) {
            self.entries[i].1 = value;
            return;
        }
        if let Some(idx) = array_index(&key) {
            let at = self.entries[..self.n_int]
                .binary_search_by(|(k, _)| array_index(k).unwrap().cmp(&idx))
                .unwrap_err();
            self.entries.insert(at, (key, value));
            self.n_int += 1;
        } else {
            self.entries.push((key, value));
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let i = self.position(key)?;
        if i < self.n_int {
            self.n_int -= 1;
        }
        Some(self.entries.remove(i).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Str, &Value)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &Str> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|(_, v)| v)
    }

    /// `Object.assign(this, other)`
    pub fn assign(&mut self, other: &Object) {
        for (k, v) in other.iter() {
            self.insert(k.clone(), v.clone());
        }
    }

    pub fn into_value(self) -> Value {
        Value::Obj(Arc::new(self))
    }
}

impl FromIterator<(Str, Value)> for Object {
    fn from_iter<T: IntoIterator<Item = (Str, Value)>>(iter: T) -> Self {
        let mut o = Object::new();
        for (k, v) in iter {
            o.insert(k, v);
        }
        o
    }
}

/// `Number.prototype.toString()` for finite and special values.
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let neg = n < 0.0;
    let a = n.abs();
    // shortest round-trip digits via Rust's `{:e}`: "d.ddde±x"
    let sci = format!("{a:e}");
    let (mant, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let nexp = exp + 1; // position of decimal point relative to digits
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if k <= nexp && nexp <= 21 {
        out.push_str(&digits);
        for _ in 0..(nexp - k) {
            out.push('0');
        }
    } else if 0 < nexp && nexp <= 21 {
        out.push_str(&digits[..nexp as usize]);
        out.push('.');
        out.push_str(&digits[nexp as usize..]);
    } else if -6 < nexp && nexp <= 0 {
        out.push_str("0.");
        for _ in 0..(-nexp) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let e = nexp - 1;
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if e >= 0 { '+' } else { '-' });
        out.push_str(&e.abs().to_string());
    }
    out
}

/// `Number(str)` (string to number conversion).
pub fn string_to_number(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| is_js_whitespace(c));
    if t.is_empty() {
        return 0.0;
    }
    let (neg, body) = if let Some(r) = t.strip_prefix('-') {
        (true, r)
    } else if let Some(r) = t.strip_prefix('+') {
        (false, r)
    } else {
        (false, t)
    };
    let sign = if neg { -1.0 } else { 1.0 };
    if body == "Infinity" {
        return sign * f64::INFINITY;
    }
    if t.len() > 2 && t.is_char_boundary(2) && !t.starts_with(['-', '+']) {
        let lower = &t[..2];
        let radix = match lower {
            "0x" | "0X" => 16,
            "0o" | "0O" => 8,
            "0b" | "0B" => 2,
            _ => 0,
        };
        if radix != 0 {
            let digits = &t[2..];
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            let mut v = 0f64;
            for c in digits.chars() {
                v = v * radix as f64 + c.to_digit(radix).unwrap() as f64;
            }
            return v;
        }
    }
    // StrDecimalLiteral
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut int_digits = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        int_digits += 1;
    }
    let mut frac_digits = 0;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            frac_digits += 1;
        }
    }
    if int_digits == 0 && frac_digits == 0 {
        return f64::NAN;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return f64::NAN;
        }
    }
    if i != bytes.len() {
        return f64::NAN;
    }
    let normalized = if body.starts_with('.') { format!("0{body}") } else { body.to_string() };
    let normalized = normalized.replace(".e", ".0e").replace(".E", ".0E");
    let normalized = if normalized.ends_with('.') { format!("{normalized}0") } else { normalized };
    sign * normalized.parse::<f64>().unwrap_or(f64::NAN)
}

/// `parseFloat(str)`
pub fn parse_float(s: &str) -> f64 {
    let t = s.trim_start_matches(|c: char| is_js_whitespace(c));
    let bytes = t.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    if t[i..].starts_with("Infinity") {
        return if t.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let start_digits = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let mut any = i > start_digits;
    if i < bytes.len() && bytes[i] == b'.' {
        let j = i + 1;
        let mut k = j;
        while k < bytes.len() && bytes[k].is_ascii_digit() {
            k += 1;
        }
        if k > j || any {
            any = any || k > j;
            i = k;
        }
    }
    if !any {
        return f64::NAN;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut k = i + 1;
        if k < bytes.len() && (bytes[k] == b'+' || bytes[k] == b'-') {
            k += 1;
        }
        let ds = k;
        while k < bytes.len() && bytes[k].is_ascii_digit() {
            k += 1;
        }
        if k > ds {
            i = k;
        }
    }
    let lit = &t[..i];
    let lit = if lit.ends_with('.') { &lit[..lit.len() - 1] } else { lit };
    let lit = lit.replace("+.", "+0.").replace("-.", "-0.");
    let lit = if lit.starts_with('.') { format!("0{lit}") } else { lit };
    lit.parse::<f64>().unwrap_or(f64::NAN)
}

/// `parseInt(str)` (radix 10)
pub fn parse_int(s: &str) -> f64 {
    let t = s.trim_start_matches(|c: char| is_js_whitespace(c));
    let (neg, body) = if let Some(r) = t.strip_prefix('-') {
        (true, r)
    } else if let Some(r) = t.strip_prefix('+') {
        (false, r)
    } else {
        (false, t)
    };
    let digits: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return f64::NAN;
    }
    let v: f64 = digits.parse().unwrap_or(f64::NAN);
    if neg { -v } else { v }
}

/// `\s` in JS regexes and `String.prototype.trim`.
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
            | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// `String.prototype.trim`
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// JS `\w`
pub fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// JS string comparison `a < b` (UTF-16 code unit order).
pub fn cmp_utf16(a: &str, b: &str) -> std::cmp::Ordering {
    // UTF-8 byte order is code point order; UTF-16 order differs only between a supplementary
    // character and a BMP character at or above U+E000, so compare the first differing chars
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let mut i = 0;
    let n = ab.len().min(bb.len());
    while i < n && ab[i] == bb[i] {
        i += 1;
    }
    if i == n {
        return ab.len().cmp(&bb.len());
    }
    while !a.is_char_boundary(i) {
        i -= 1;
    }
    let ca = a[i..].chars().next().unwrap();
    let cb = b[i..].chars().next().unwrap();
    let unit = |c: char| -> u32 {
        let c = c as u32;
        if c >= 0x10000 { 0xD800 + ((c - 0x10000) >> 10) } else { c }
    };
    match unit(ca).cmp(&unit(cb)) {
        std::cmp::Ordering::Equal => (ca as u32).cmp(&(cb as u32)),
        o => o,
    }
}

/// A fast non-cryptographic hasher (FxHash) for the string-keyed maps on hot paths.
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

impl std::hash::Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        const K: u64 = 0x517cc1b727220a95;
        let mut h = self.0;
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            h = (h.rotate_left(5) ^ u64::from_le_bytes(c.try_into().unwrap())).wrapping_mul(K);
        }
        for &b in chunks.remainder() {
            h = (h.rotate_left(5) ^ b as u64).wrapping_mul(K);
        }
        self.0 = h;
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxBuild = std::hash::BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuild>;
pub type FxHashSet<K> = std::collections::HashSet<K, FxBuild>;

/// UTF-16 length of a string
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        for (n, s) in [
            (0.5, "0.5"),
            (1.0, "1"),
            (100.0, "100"),
            (1e21, "1e+21"),
            (1e-7, "1e-7"),
            (0.000001, "0.000001"),
            (123456789012345680000.0, "123456789012345680000"),
            (-1.5, "-1.5"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1.0 / 255.0, "0.00392156862745098"),
        ] {
            assert_eq!(number_to_string(n), s);
        }
        assert_eq!(string_to_number(" 12 "), 12.0);
        assert!(string_to_number("12px").is_nan());
        assert_eq!(string_to_number(".5"), 0.5);
        assert_eq!(string_to_number("5."), 5.0);
        assert_eq!(string_to_number(""), 0.0);
        assert_eq!(parse_float("12.5px"), 12.5);
        assert_eq!(parse_float(".5em"), 0.5);
        assert!(parse_float("px").is_nan());
    }

    #[test]
    fn key_order() {
        let mut o = Object::new();
        o.insert("px", Value::Null);
        o.insert("10", Value::Null);
        o.insert("0.5", Value::Null);
        o.insert("2", Value::Null);
        o.insert("010", Value::Null);
        let keys: Vec<_> = o.keys().map(|k| k.to_string()).collect();
        assert_eq!(keys, ["2", "10", "px", "0.5", "010"]);
    }
}

/// `Array.prototype.sort(cmp)` as V8 runs it. Short arrays (< 64) go through V8's
/// `CountAndMakeRun` + binary insertion sort exactly, so even inconsistent comparators give
/// the same order; longer arrays use a stable merge sort (same result for consistent
/// comparators).
pub fn sort_by<T: Clone>(list: &mut [T], mut cmp: impl FnMut(&T, &T) -> i32) {
    let n = list.len();
    if n < 2 {
        return;
    }
    if n >= 64 {
        list.sort_by(|a, b| cmp(a, b).cmp(&0));
        return;
    }
    // CountAndMakeRun(0, n)
    let mut run = 2;
    let order = cmp(&list[1], &list[0]);
    let descending = order < 0;
    let mut prev = 1;
    for idx in 2..n {
        let order = cmp(&list[idx], &list[prev]);
        if descending {
            if order >= 0 {
                break;
            }
        } else if order < 0 {
            break;
        }
        prev = idx;
        run += 1;
    }
    if descending {
        list[..run].reverse();
    }
    // BinaryInsertionSort(0, run, n)
    for start in run..n {
        let pivot = list[start].clone();
        let mut left = 0;
        let mut right = start;
        while left < right {
            let mid = left + ((right - left) >> 1);
            if cmp(&pivot, &list[mid]) < 0 {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        let mut p = start;
        while p > left {
            list[p] = list[p - 1].clone();
            p -= 1;
        }
        list[left] = pivot;
    }
}
