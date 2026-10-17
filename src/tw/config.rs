//! Config resolution: `stubs/config.full.js`, `util/resolveConfig.js`, `util/getAllConfigs.js`,
//! `util/normalizeConfig.js`, `featureFlags.js`.

use std::sync::{Arc, OnceLock};

use crate::js::{Object, Str, Value};

use super::util::{negate_value, parse_color_format, to_color_value, to_path, with_alpha_value};

static DEFAULT_CONFIG_JSON: &str = include_str!("data/default_config.json");
static CORE_PLUGIN_LIST_JSON: &str = include_str!("data/core_plugin_list.json");

/// `stubs/config.full.js` with its theme functions.
pub fn default_config() -> &'static Value {
    static CELL: OnceLock<Value> = OnceLock::new();
    CELL.get_or_init(|| {
        let json: serde_json::Value = serde_json::from_str(DEFAULT_CONFIG_JSON).unwrap();
        let v = Value::from_json(&json);
        let mut cfg = v.as_obj().unwrap().clone();
        let theme = cfg.get("theme").unwrap().as_obj().unwrap().clone();
        let mut new_theme = Object::new();
        for (k, v) in theme.iter() {
            if let Some(f) = v.as_obj().and_then(|o| o.get("$fn")) {
                let template = f.clone();
                new_theme.insert(
                    k.clone(),
                    Value::func(move |args: &[Value]| eval_template(&template, &args[0], args.get(1).unwrap_or(&Value::Undefined))),
                );
            } else {
                new_theme.insert(k.clone(), v.clone());
            }
        }
        cfg.insert("theme", new_theme.into_value());
        cfg.into_value()
    })
}

pub fn core_plugin_list() -> &'static [String] {
    static CELL: OnceLock<Vec<String>> = OnceLock::new();
    CELL.get_or_init(|| serde_json::from_str(CORE_PLUGIN_LIST_JSON).unwrap())
}

/// Evaluates a recorded default-theme function (see `tools/gen-data.mjs`).
fn eval_template(t: &Value, resolve_path: &Value, utils: &Value) -> Value {
    let Some(obj) = t.as_obj() else { return t.clone() };
    // a lone sentinel is `theme(path)` returned as the value
    if obj.len() == 1 {
        let (k, v) = obj.iter().next().unwrap();
        if k.starts_with("$theme#") {
            return call_theme(resolve_path, v);
        }
    }
    let mut out = Object::new();
    for (k, v) in obj.iter() {
        if k.starts_with("$theme#") {
            spread(&mut out, &call_theme(resolve_path, v));
        } else if k.starts_with("$breakpoints#") {
            let screens = resolve_path.call(&[v.clone()]);
            let bp = utils.get("breakpoints").call(&[screens]);
            spread(&mut out, &bp);
        } else if k.starts_with("$negative#") {
            let scale = resolve_path.call(&[v.clone()]);
            let neg = utils.get("negative").call(&[scale]);
            spread(&mut out, &neg);
        } else if let Some(o) = v.as_obj() {
            if o.len() == 1 && o.keys().next().unwrap().starts_with("$theme#") {
                out.insert(k.clone(), call_theme(resolve_path, o.values().next().unwrap()));
            } else {
                out.insert(k.clone(), v.clone());
            }
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    out.into_value()
}

fn call_theme(resolve_path: &Value, args: &Value) -> Value {
    let args: Vec<Value> = args.as_arr().unwrap().to_vec();
    resolve_path.call(&args)
}

/// `{...value}`
pub fn spread(out: &mut Object, v: &Value) {
    match v {
        Value::Obj(o) => out.assign(o),
        Value::Arr(a) => {
            for (i, x) in a.iter().enumerate() {
                out.insert(i.to_string(), x.clone());
            }
        }
        Value::Str(s) => {
            for (i, c) in s.encode_utf16().enumerate() {
                out.insert(i.to_string(), Value::str(String::from_utf16_lossy(&[c])));
            }
        }
        _ => {}
    }
}

// ---- resolveConfig.js

type Customizer<'a> = &'a dyn Fn(&Value, &Value) -> Option<Value>;

/// `for (let k in source)` keys (own enumerable string keys)
fn for_in_keys(v: &Value) -> Vec<(Str, Value)> {
    match v {
        Value::Obj(o) => o.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Value::Arr(a) => a.iter().enumerate().map(|(i, v)| (Str::from(i.to_string()), v.clone())).collect(),
        Value::Str(s) => s
            .encode_utf16()
            .enumerate()
            .map(|(i, c)| (Str::from(i.to_string()), Value::str(String::from_utf16_lossy(&[c]))))
            .collect(),
        _ => Vec::new(),
    }
}

fn merge_with(target: &mut Object, sources: &[Value], customizer: Customizer) {
    for source in sources {
        for (k, sv) in for_in_keys(source) {
            let tv = target.get(&k).cloned().unwrap_or(Value::Undefined);
            match customizer(&tv, &sv) {
                None => {
                    if tv.is_object() && sv.is_object() {
                        let mut o = Object::new();
                        merge_with(&mut o, &[tv, sv], customizer);
                        target.insert(k, o.into_value());
                    } else {
                        target.insert(k, sv);
                    }
                }
                Some(m) => target.insert(k, m),
            }
        }
    }
}

fn collect_extends(themes: &[Value]) -> Object {
    let mut merged = Object::new();
    for theme in themes {
        let extend = theme.get("extend");
        merge_with(&mut merged, &[extend], &|merged_value: &Value, extend_value: &Value| {
            Some(match merged_value {
                Value::Undefined => vec![extend_value.clone()].into(),
                Value::Arr(a) => {
                    let mut v = vec![extend_value.clone()];
                    v.extend(a.iter().cloned());
                    v.into()
                }
                other => vec![extend_value.clone(), other.clone()].into(),
            })
        });
    }
    merged
}

/// `defaults(target, ...sources)`
fn defaults(target: &mut Object, sources: &[Value]) {
    for source in sources {
        for (k, v) in for_in_keys(source) {
            if !target.contains_key(&k) {
                target.insert(k, v);
            }
        }
    }
}

fn merge_themes(themes: &[Value]) -> Object {
    let mut merged = Object::new();
    for t in themes {
        defaults(&mut merged, std::slice::from_ref(t));
    }
    merged.insert("extend", collect_extends(themes).into_value());
    merged
}

fn merge_extension_customizer(merged: &Value, value: &Value) -> Option<Value> {
    if let Value::Arr(m) = merged {
        if m.first().is_some_and(|f| f.is_object()) {
            let mut out: Vec<Value> = m.to_vec();
            match value {
                Value::Arr(a) => out.extend(a.iter().cloned()),
                v => out.push(v.clone()),
            }
            return Some(out.into());
        }
    }
    if let Value::Arr(v) = value {
        if v.first().is_some_and(|f| f.is_object()) && merged.is_object() {
            let mut out = vec![merged.clone()];
            out.extend(v.iter().cloned());
            return Some(out.into());
        }
        return Some(value.clone());
    }
    None
}

fn merge_extensions(mut theme: Object) -> Object {
    let extend = theme.remove("extend").unwrap_or(Value::Undefined);
    merge_with(&mut theme, &[extend], &|theme_value: &Value, extensions: &Value| {
        let exts: Vec<Value> = extensions.as_arr().map(|a| a.to_vec()).unwrap_or_default();
        if !theme_value.is_function() && !exts.iter().any(|e| e.is_function()) {
            let mut o = Object::new();
            let mut sources = vec![theme_value.clone()];
            sources.extend(exts);
            merge_with(&mut o, &sources, &merge_extension_customizer);
            return Some(o.into_value());
        }
        let theme_value = theme_value.clone();
        Some(Value::func(move |args: &[Value]| {
            let mut sources = Vec::new();
            for e in std::iter::once(&theme_value).chain(exts.iter()) {
                sources.push(if e.is_function() { e.call(args) } else { e.clone() });
            }
            let mut o = Object::new();
            merge_with(&mut o, &sources, &merge_extension_customizer);
            o.into_value()
        }))
    });
    theme
}

/// `toPaths(key)` from resolveConfig: yields (path, alpha)
fn to_paths(key: &Value) -> Result<Vec<(Vec<String>, Option<String>)>, String> {
    let mut out = Vec::new();
    let path = match key {
        Value::Arr(a) => a.iter().map(|v| v.to_js_string()).collect::<Vec<_>>(),
        k => to_path(&k.to_js_string())?,
    };
    if path.is_empty() {
        return Ok(out);
    }
    out.push((path, None));
    if key.is_array() {
        return Ok(out);
    }
    let key = key.to_js_string();
    // /^(.*?)\s*\/\s*([^/]+)$/
    if let Some((prefix, alpha)) = split_alpha_suffix(&key) {
        out.push((to_path(&prefix)?, Some(alpha)));
    }
    Ok(out)
}

/// `/^(.*?)\s*\/\s*([^/]+)$/`
fn split_alpha_suffix(key: &str) -> Option<(String, String)> {
    let slash = key.rfind('/')?;
    let alpha = &key[slash + 1..];
    if alpha.is_empty() {
        return None;
    }
    // `\s*` after the slash is consumed by the lazy/greedy split: `[^/]+` may start with spaces,
    // but `\s*` before it is greedy, so leading whitespace goes to the separator.
    let alpha_trimmed = alpha.trim_start_matches(|c: char| crate::js::is_js_whitespace(c));
    if alpha_trimmed.is_empty() {
        // `\s*` backtracks to give `[^/]+` at least one char
        let last = alpha.chars().last().unwrap();
        let prefix = key[..slash].trim_end_matches(|c: char| crate::js::is_js_whitespace(c)).to_string();
        return Some((prefix, last.to_string()));
    }
    let prefix = key[..slash].trim_end_matches(|c: char| crate::js::is_js_whitespace(c)).to_string();
    Some((prefix, alpha_trimmed.to_string()))
}

fn config_utils() -> Value {
    static CELL: OnceLock<Value> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut o = Object::new();
        o.insert("colors", super::colors::colors().clone());
        o.insert(
            "negative",
            Value::func(|args: &[Value]| {
                let scale = args.first().cloned().unwrap_or(Value::Undefined);
                let mut out = Object::new();
                for (k, v) in for_in_keys(&scale) {
                    if v == Value::str("0") {
                        continue;
                    }
                    if let Some(n) = negate_value(&v) {
                        out.insert(format!("-{k}"), Value::str(n));
                    }
                }
                out.into_value()
            }),
        );
        o.insert(
            "breakpoints",
            Value::func(|args: &[Value]| {
                let screens = args.first().cloned().unwrap_or(Value::Undefined);
                let mut out = Object::new();
                for (k, v) in for_in_keys(&screens) {
                    if v.is_string() {
                        out.insert(format!("screen-{k}"), v);
                    }
                }
                out.into_value()
            }),
        );
        o.into_value()
    })
    .clone()
}

fn make_resolve_path(object: Arc<Object>) -> Value {
    Value::func(move |args: &[Value]| {
        let key = args.first().cloned().unwrap_or(Value::Undefined);
        let default = args.get(1).cloned().unwrap_or(Value::Undefined);
        resolve_path(&object, &key, default)
    })
}

fn resolve_path(object: &Arc<Object>, key: &Value, default: Value) -> Value {
    let paths = match to_paths(key) {
        Ok(p) => p,
        Err(e) => panic!("{e}"),
    };
    for (path, alpha) in paths {
        let mut index = 0;
        let mut val = Value::Obj(object.clone());
        while !val.is_nullish() && index < path.len() {
            val = val.get(&path[index]);
            index += 1;
            let should_call = val.is_function() && (alpha.is_none() || index <= path.len() - 1);
            if should_call {
                val = val.call(&[make_resolve_path(object.clone()), config_utils()]);
            }
        }
        if !val.is_undefined() {
            if let Some(alpha) = &alpha {
                let normalized = parse_color_format(&val);
                let fallback = to_color_value(&normalized);
                return with_alpha_value(&normalized, &Value::str(alpha.as_str()), &fallback);
            }
            if val.is_object() {
                return val.clone_deep();
            }
            return val;
        }
    }
    default
}

fn resolve_function_keys(object: Object) -> Object {
    let object = Arc::new(object);
    let rp = make_resolve_path(object.clone());
    let mut resolved = Object::new();
    for (k, v) in object.iter() {
        let v = if v.is_function() { v.call(&[rp.clone(), config_utils()]) } else { v.clone() };
        resolved.insert(k.clone(), v);
    }
    resolved
}

fn configure_plugins(plugin_config: &Value, plugins: Vec<String>) -> Vec<String> {
    match plugin_config {
        Value::Undefined => plugins,
        Value::Arr(a) => a.iter().map(|v| v.to_js_string()).collect(),
        other => {
            let not_false = |name: &str| !matches!(other.get(name), Value::Bool(false));
            let is_false_cfg = matches!(other, Value::Bool(false));
            let mut out: Vec<String> = Vec::new();
            let mut push = |s: String| {
                if !out.contains(&s) {
                    out.push(s);
                }
            };
            for p in plugins {
                if !is_false_cfg && not_false(&p) {
                    push(p);
                }
            }
            for (k, v) in for_in_keys(other) {
                if !matches!(v, Value::Bool(false)) {
                    push(k.to_string());
                }
            }
            out
        }
    }
}

/// `getAllConfigs(config)`
fn get_all_configs(config: &Value) -> Vec<Value> {
    let presets = match config.get("presets") {
        Value::Undefined | Value::Null => vec![default_config().clone()],
        Value::Arr(a) => a.to_vec(),
        _ => Vec::new(),
    };
    let mut configs = Vec::new();
    for preset in presets.iter().rev() {
        configs.extend(get_all_configs(preset));
    }
    let mut out = vec![config.clone()];
    if flag_enabled(config, "respectDefaultRingColorOpacity") {
        let mut theme = Object::new();
        theme.insert(
            "ringColor",
            Value::func(|args: &[Value]| {
                let mut o = Object::new();
                o.insert("DEFAULT", Value::str("#3b82f67f"));
                spread(&mut o, &args[0].call(&[Value::str("colors")]));
                o.into_value()
            }),
        );
        let mut c = Object::new();
        c.insert("theme", theme.into_value());
        out.push(c.into_value());
    }
    if flag_enabled(config, "disableColorOpacityUtilitiesByDefault") {
        let mut cp = Object::new();
        for k in ["backgroundOpacity", "borderOpacity", "divideOpacity", "placeholderOpacity", "ringOpacity", "textOpacity"] {
            cp.insert(k, Value::Bool(false));
        }
        let mut c = Object::new();
        c.insert("corePlugins", cp.into_value());
        out.push(c.into_value());
    }
    out.extend(configs);
    out
}

/// `resolveConfig(config)` from `public/resolve-config.js`, followed by `normalizeConfig`.
pub fn resolve_config(config: &Value) -> Value {
    let mut configs = vec![config.clone()];
    let all = get_all_configs(config);
    configs.extend(all.into_iter().skip(1));

    // extractPluginConfigs: plugins are never functions here, so plugin configs are `{}`
    let mut all_configs: Vec<Value> = Vec::new();
    for c in &configs {
        all_configs.push(c.clone());
        let plugins = c.get("plugins");
        if let Value::Arr(p) = &plugins {
            for plugin in p.iter() {
                let pc = plugin.get("config");
                all_configs.push(if pc.is_nullish() { Object::new().into_value() } else { pc });
            }
        }
    }
    let mut base = Object::new();
    base.insert("prefix", Value::str(""));
    base.insert("important", Value::Bool(false));
    base.insert("separator", Value::str(":"));
    all_configs.push(base.into_value());

    let themes: Vec<Value> = all_configs
        .iter()
        .map(|t| {
            let th = t.get("theme");
            if th.is_nullish() { Object::new().into_value() } else { th }
        })
        .collect();
    let theme = resolve_function_keys(merge_extensions(merge_themes(&themes)));

    let mut core_plugins: Vec<String> = core_plugin_list().to_vec();
    for c in all_configs.iter().rev() {
        core_plugins = configure_plugins(&c.get("corePlugins"), core_plugins);
    }

    let mut plugins: Vec<Value> = Vec::new();
    for c in configs.iter().rev() {
        if let Value::Arr(p) = c.get("plugins") {
            plugins.extend(p.iter().cloned());
        }
    }

    let mut resolved = Object::new();
    resolved.insert("theme", theme.into_value());
    resolved.insert("corePlugins", Value::from(core_plugins.into_iter().map(Value::str).collect::<Vec<_>>()));
    resolved.insert("plugins", Value::from(plugins));
    defaults(&mut resolved, &all_configs);
    normalize_config(resolved)
}

fn normalize_config(mut config: Object) -> Value {
    let cv = config.clone().into_value();
    let content = cv.get("content");
    let purge = cv.get("purge");
    let safelist = {
        let s = cv.get("safelist");
        if s.is_array() {
            s
        } else if content.get("safelist").is_array() {
            content.get("safelist")
        } else if purge.get("safelist").is_array() {
            purge.get("safelist")
        } else if purge.get("options").get("safelist").is_array() {
            purge.get("options").get("safelist")
        } else {
            Value::from(Vec::<Value>::new())
        }
    };
    config.insert("safelist", safelist);
    let blocklist = match cv.get("blocklist") {
        Value::Arr(a) if a.iter().all(|v| v.is_string()) => Value::Arr(a),
        _ => Value::from(Vec::<Value>::new()),
    };
    config.insert("blocklist", blocklist);
    let prefix = match cv.get("prefix") {
        Value::Func(_) | Value::Undefined | Value::Null => Value::str(""),
        p => p,
    };
    config.insert("prefix", prefix);
    config.into_value()
}

/// `flagEnabled(config, flag)`
pub fn flag_enabled(config: &Value, flag: &str) -> bool {
    let default = |f: &str| match f {
        "generalizedModifiers" => Some(true),
        "optimizeUniversalDefaults" | "disableColorOpacityUtilitiesByDefault" | "relativeContentPathsByDefault" => Some(false),
        _ => None,
    };
    let future = ["hoverOnlyWhenSupported", "respectDefaultRingColorOpacity", "disableColorOpacityUtilitiesByDefault", "relativeContentPathsByDefault"];
    let experimental = ["optimizeUniversalDefaults", "generalizedModifiers"];
    let check = |group: &str| {
        let g = config.get(group);
        if g == Value::str("all") {
            return true;
        }
        let v = g.get(flag);
        if !v.is_nullish() {
            return v.truthy();
        }
        default(flag).unwrap_or(false)
    };
    if future.contains(&flag) {
        check("future")
    } else if experimental.contains(&flag) {
        check("experimental")
    } else {
        false
    }
}

/// `dlv(obj, path, default)`
pub fn dlv(obj: &Value, path: &[String], default: Value) -> Value {
    let mut o = obj.clone();
    for p in path {
        o = if o.truthy() { o.get(p) } else { Value::Undefined };
    }
    if o.is_undefined() { default } else { o }
}
