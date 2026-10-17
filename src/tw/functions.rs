//! `lib/evaluateTailwindFunctions.js` (+ `didyoumean`)

use crate::js::{Object, Value};
use crate::postcss::{CssError, Flow, Kind, NodeId};
use crate::value_parser::{self, VNode};

use super::config::dlv;
use super::context::Context;
use super::util::{build_media_query, normalize_screens, parse_color_format, to_path, transform_theme_value, with_alpha_value};

fn strip_quotes(s: &str) -> String {
    s.trim_start_matches(['\'', '"']).trim_end_matches(['\'', '"']).to_string()
}

fn path_to_string(path: &[String]) -> String {
    let mut acc = String::new();
    for (i, cur) in path.iter().enumerate() {
        if cur.contains('.') {
            acc = format!("{acc}[{cur}]");
        } else if i == 0 {
            acc = cur.clone();
        } else {
            acc = format!("{acc}.{cur}");
        }
    }
    acc
}

fn list(items: &[String]) -> String {
    items.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(", ")
}

fn keys_of(v: &Value) -> Vec<String> {
    match v {
        Value::Obj(o) => o.keys().map(|k| k.to_string()).collect(),
        Value::Arr(a) => (0..a.len()).map(|i| i.to_string()).collect(),
        _ => Vec::new(),
    }
}

fn is_object(v: &Value) -> bool {
    v.is_object_like()
}

/// `didYouMean(str, list)` with the library defaults (threshold 0.4, absolute 20, case-insensitive)
fn did_you_mean(s: &str, list: &[String]) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    let s: Vec<u16> = s.to_lowercase().encode_utf16().collect();
    let rel = 0.4 * s.len() as f64;
    let mut winning = rel.min(20.0);
    let mut winner: Option<String> = None;
    for cand in list {
        if cand.is_empty() {
            continue;
        }
        let test: Vec<u16> = cand.to_lowercase().encode_utf16().collect();
        let val = edit_distance(&s, &test, winning);
        if val < winning {
            winning = val;
            winner = Some(cand.clone());
        }
    }
    winner
}

fn edit_distance(a: &[u16], b: &[u16], max: f64) -> f64 {
    let lena = a.len();
    let lenb = b.len();
    if lena == 0 {
        return (max + 1.0).min(lenb as f64);
    }
    if lenb == 0 {
        return (max + 1.0).min(lena as f64);
    }
    if (lena as f64 - lenb as f64).abs() > max {
        return max + 1.0;
    }
    let mut m = vec![vec![0f64; lena + 1]; lenb + 1];
    for (i, row) in m.iter_mut().enumerate() {
        row[0] = i as f64;
    }
    for j in 0..=lena {
        m[0][j] = j as f64;
    }
    for i in 1..=lenb {
        let mut col_min = f64::MAX;
        let mut min_j = 1f64;
        if i as f64 > max {
            min_j = i as f64 - max;
        }
        let mut max_j = (lenb + 1) as f64;
        if max_j > max + i as f64 {
            max_j = max + i as f64;
        }
        for j in 1..=lena {
            if (j as f64) < min_j || (j as f64) > max_j {
                m[i][j] = max + 1.0;
            } else if b[i - 1] == a[j - 1] {
                m[i][j] = m[i - 1][j - 1];
            } else {
                m[i][j] = (m[i - 1][j - 1] + 1.0).min((m[i][j - 1] + 1.0).min(m[i - 1][j] + 1.0));
            }
            if m[i][j] < col_min {
                col_min = m[i][j];
            }
        }
        if col_min > max {
            return max + 1.0;
        }
    }
    m[lenb][lena]
}

enum PathArg {
    Str(String),
    Arr(Vec<String>),
}

struct Validation {
    valid: bool,
    value: Value,
    error: String,
}

fn find_closest_existing_path(theme: &Value, path: &str) -> Result<Option<Vec<String>>, String> {
    let mut parts = to_path(path)?;
    loop {
        parts.pop();
        if !dlv(theme, &parts, Value::Undefined).is_undefined() {
            break;
        }
        if parts.is_empty() {
            break;
        }
    }
    Ok(if parts.is_empty() { None } else { Some(parts) })
}

fn validate_path(config: &Value, path: PathArg, default: &Value, opacity: Option<&str>) -> Result<Validation, String> {
    let theme = config.get("theme");
    let (path_string, segments) = match path {
        PathArg::Arr(a) => (path_to_string(&a), a),
        PathArg::Str(s) => {
            let ps = strip_quotes(&s);
            let seg = to_path(&ps)?;
            (ps, seg)
        }
    };
    let value = dlv(&theme, &segments, default.clone());
    if value.is_undefined() {
        let mut error = format!("'{path_string}' does not exist in your theme config.");
        let parent_segments = segments[..segments.len().saturating_sub(1)].to_vec();
        let parent_value = dlv(&theme, &parent_segments, Value::Undefined);
        if is_object(&parent_value) {
            let mut valid_keys = Vec::new();
            for key in keys_of(&parent_value) {
                let mut p = parent_segments.clone();
                p.push(key.clone());
                if validate_path(config, PathArg::Arr(p), &Value::Undefined, None)?.valid {
                    valid_keys.push(key);
                }
            }
            let last = segments.last().cloned().unwrap_or_default();
            if let Some(sugg) = did_you_mean(&last, &valid_keys) {
                let mut p = parent_segments.clone();
                p.push(sugg);
                error.push_str(&format!(" Did you mean '{}'?", path_to_string(&p)));
            } else if !valid_keys.is_empty() {
                error.push_str(&format!(" '{}' has the following valid keys: {}", path_to_string(&parent_segments), list(&valid_keys)));
            }
        } else {
            match find_closest_existing_path(&theme, &path_string)? {
                Some(closest) => {
                    let cv = dlv(&theme, &closest, Value::Undefined);
                    if is_object(&cv) {
                        error.push_str(&format!(" '{}' has the following keys: {}", path_to_string(&closest), list(&keys_of(&cv))));
                    } else {
                        error.push_str(&format!(" '{}' is not an object.", path_to_string(&closest)));
                    }
                }
                None => {
                    error.push_str(&format!(" Your theme has the following top-level keys: {}", list(&keys_of(&theme))));
                }
            }
        }
        return Ok(Validation { valid: false, value: Value::Undefined, error });
    }
    if !matches!(value, Value::Str(_) | Value::Num(_) | Value::Func(_) | Value::Arr(_)) {
        let mut error = format!("'{path_string}' was found but does not resolve to a string.");
        if is_object(&value) {
            for key in keys_of(&value) {
                let mut p = segments.clone();
                p.push(key.clone());
                if validate_path(config, PathArg::Arr(p.clone()), &Value::Undefined, None)?.valid {
                    error.push_str(&format!(" Did you mean something like '{}'?", path_to_string(&p)));
                    break;
                }
            }
        }
        return Ok(Validation { valid: false, value: Value::Undefined, error });
    }
    let section = segments.first().cloned().unwrap_or_default();
    let mut opts = Object::new();
    opts.insert("opacityValue", opacity.map(Value::str).unwrap_or(Value::Undefined));
    Ok(Validation { valid: true, value: transform_theme_value(&section, value, &opts.into_value()), error: String::new() })
}

/// `toPaths(path)` from evaluateTailwindFunctions: [(path, alpha)]
fn to_paths(path: &str) -> Vec<(String, Option<String>)> {
    let path = strip_quotes(path);
    let mut out = vec![(path.clone(), None)];
    // /^([^\s]+)(?![^\[]*\])(?:\s*\/\s*([^\/\s]+))$/
    let is_ws = crate::js::is_js_whitespace;
    if let Some(k) = path.rfind('/') {
        let after = &path[k + 1..];
        let alpha = after.trim_start_matches(is_ws);
        let before = path[..k].trim_end_matches(is_ws);
        if !alpha.is_empty() && !alpha.contains(is_ws) && !before.is_empty() && !before.contains(is_ws) {
            // lookahead at the end of group 1: `[^\[]*\]` must not match
            let rest = &path[before.len()..];
            let mut lookahead = false;
            for c in rest.chars() {
                if c == '[' {
                    break;
                }
                if c == ']' {
                    lookahead = true;
                    break;
                }
            }
            if !lookahead {
                out.push((before.to_string(), Some(alpha.to_string())));
            }
        }
    }
    out
}

fn resolve_path(config: &Value, path: &str, default: &Value) -> Result<(Validation, Option<String>), String> {
    let mut results = Vec::new();
    for (p, alpha) in to_paths(path) {
        let v = validate_path(config, PathArg::Str(p), default, alpha.as_deref())?;
        results.push((v, alpha));
    }
    let idx = results.iter().position(|(v, _)| v.valid).unwrap_or(0);
    Ok(results.swap_remove(idx))
}

impl Context<'_> {
    fn fn_theme(&mut self, node: NodeId, args: &[String]) -> Result<Value, CssError> {
        let path = args.first().cloned().unwrap_or_else(|| "undefined".into());
        let default = if args.len() > 1 {
            Value::from(args[1..].iter().map(|s| Value::str(s.as_str())).collect::<Vec<_>>())
        } else {
            Value::Undefined
        };
        let (v, alpha) = resolve_path(&self.cfg.value, &path, &default).map_err(CssError::plain)?;
        if !v.valid {
            let parent = self.css[node].parent;
            let candidate = parent.and_then(|p| self.css[p].tw().and_then(|t| t.candidate.clone()));
            if let (Some(p), Some(_)) = (parent, candidate) {
                self.css.remove(p);
                return Ok(Value::Undefined);
            }
            return Err(self.css.error(node, v.error));
        }
        let mut value = v.value;
        let maybe_color = parse_color_format(&value);
        let is_color_function = maybe_color.is_function();
        if alpha.is_some() || is_color_function {
            let a = match &alpha {
                Some(a) => Value::str(a.as_str()),
                None => Value::Num(1.0),
            };
            value = with_alpha_value(&maybe_color, &a, &maybe_color);
        }
        Ok(value)
    }

    fn fn_screen(&mut self, node: NodeId, args: &[String]) -> Result<Value, CssError> {
        let screen = strip_quotes(args.first().map(|s| s.as_str()).unwrap_or("undefined"));
        let screens = normalize_screens(&self.cfg.value.get("theme").get("screens")).map_err(CssError::plain)?;
        match screens.iter().find(|s| s.name == screen) {
            Some(def) => Ok(Value::str(build_media_query(std::slice::from_ref(def)))),
            None => Err(self.css.error(node, format!("The '{screen}' screen does not exist in your theme."))),
        }
    }

    fn resolve_vnode(&mut self, node: NodeId, v: &mut VNode) -> Result<(), CssError> {
        if let VNode::Function { value, nodes, .. } = v {
            if value == "theme" || value == "screen" {
                let name = value.clone();
                let args = self.extract_args(node, nodes)?;
                let result = if name == "theme" { self.fn_theme(node, &args)? } else { self.fn_screen(node, &args)? };
                *v = VNode::Word(result.to_js_string());
            }
        }
        Ok(())
    }

    fn extract_args(&mut self, node: NodeId, vnodes: &mut [VNode]) -> Result<Vec<String>, CssError> {
        for v in vnodes.iter_mut() {
            self.resolve_vnode(node, v)?;
        }
        let mut args = vec![String::new()];
        for v in vnodes.iter() {
            if matches!(v, VNode::Div { value, .. } if value == ",") {
                args.push(String::new());
            } else {
                args.last_mut().unwrap().push_str(&value_parser::stringify_node(v));
            }
        }
        Ok(args)
    }

    fn walk_vnodes(&mut self, node: NodeId, nodes: &mut [VNode]) -> Result<(), CssError> {
        for v in nodes.iter_mut() {
            self.resolve_vnode(node, v)?;
            if let VNode::Function { nodes: children, .. } = v {
                self.walk_vnodes(node, children)?;
            }
        }
        Ok(())
    }

    fn resolve_functions(&mut self, node: NodeId, input: &str) -> Result<String, CssError> {
        if !input.contains("theme(") && !input.contains("screen(") {
            return Ok(input.to_string());
        }
        let mut nodes = value_parser::parse(input);
        self.walk_vnodes(node, &mut nodes)?;
        Ok(value_parser::stringify(&nodes))
    }

    /// `evaluateTailwindFunctions(context)(root)`
    pub fn evaluate_tailwind_functions(&mut self, root: NodeId) -> Result<(), CssError> {
        let mut err = None;
        // `theme()` only rewrites values and removes parents, and the JS walk keeps visiting the
        // children of a removed rule, so a snapshot of the walk order is equivalent
        let mut list = Vec::new();
        self.css.walk(root, &mut |_, n| {
            list.push(n);
            Flow::Continue
        });
        for n in list {
            let kind = self.css[n].kind;
            let input = match kind {
                Kind::AtRule => self.css[n].params.clone(),
                Kind::Decl => self.css[n].value.clone(),
                _ => continue,
            };
            match self.resolve_functions(n, &input) {
                Ok(v) => match kind {
                    Kind::AtRule => self.css[n].params = v,
                    _ => self.css[n].value = v,
                },
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
