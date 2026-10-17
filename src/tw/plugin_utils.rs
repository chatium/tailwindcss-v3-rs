//! `util/pluginUtils.js`

use crate::js::Value;

use super::context::{PluginOptions, TypeSpec};
use super::util::{self, negate_value, normalize, parse_color_format, with_alpha_value};
use super::TwConfig;

fn is_arbitrary_value(s: &str) -> bool {
    s.starts_with('[') && s.ends_with(']')
}

/// `options.values?.[key]` — `None` for undefined
fn lookup(options: &PluginOptions, key: &str) -> Value {
    options.values.get(key)
}

/// `key in options.values`
fn has_key(options: &PluginOptions, key: &str) -> bool {
    match &options.values {
        Value::Obj(o) => o.contains_key(key),
        Value::Arr(a) => key == "length" || crate::js::array_index(key).is_some_and(|i| (i as usize) < a.len()),
        _ => false,
    }
}

fn resolve_arbitrary_value(modifier: &str, validate: &dyn Fn(&str) -> bool) -> Value {
    if !is_arbitrary_value(modifier) {
        return Value::Undefined;
    }
    let value = &modifier[1..modifier.len() - 1];
    if !validate(value) {
        return Value::Undefined;
    }
    Value::str(normalize(value, None))
}

fn as_negative_value(modifier: &str, options: &PluginOptions, validate: &dyn Fn(&str) -> bool) -> Value {
    let positive = lookup(options, modifier);
    if !positive.is_undefined() {
        return negate_value(&positive).map(Value::str).unwrap_or(Value::Undefined);
    }
    if is_arbitrary_value(modifier) {
        let resolved = resolve_arbitrary_value(modifier, validate);
        if resolved.is_undefined() {
            return Value::Undefined;
        }
        return negate_value(&resolved).map(Value::str).unwrap_or(Value::Undefined);
    }
    Value::Undefined
}

/// `asValue(modifier, options, { validate })`
pub fn as_value(modifier: &str, options: &PluginOptions, validate: &dyn Fn(&str) -> bool) -> Value {
    let value = lookup(options, modifier);
    if !value.is_undefined() {
        return value;
    }
    if options.supports_negative_values && modifier.starts_with('-') {
        return as_negative_value(&modifier[1..], options, validate);
    }
    resolve_arbitrary_value(modifier, validate)
}

/// `splitUtilityModifier(modifier)` → (modifier, utilityModifier)
fn split_utility_modifier(modifier: &str) -> (String, Option<String>) {
    let b = modifier.as_bytes();
    let mut slash_idx: isize = modifier.rfind('/').map(|i| i as isize).unwrap_or(-1);
    let last_index_of = |c: u8, from: isize| -> isize {
        if from < 0 {
            return -1;
        }
        let from = (from as usize).min(b.len().saturating_sub(1));
        (0..=from).rev().find(|&i| b[i] == c).map(|i| i as isize).unwrap_or(-1)
    };
    let index_of = |c: u8, from: isize| -> isize {
        let from = from.max(0) as usize;
        (from..b.len()).find(|&i| b[i] == c).map(|i| i as isize).unwrap_or(-1)
    };
    // `lastIndexOf('[', -1)` searches from 0
    let arbitrary_start = if slash_idx < 0 { if b.first() == Some(&b'[') { 0 } else { -1 } } else { last_index_of(b'[', slash_idx) };
    let arbitrary_end = index_of(b']', slash_idx);
    let at = |i: isize| if i >= 0 { b.get(i as usize).copied() } else { None };
    let is_next_to_arbitrary = at(slash_idx - 1) == Some(b']') || at(slash_idx + 1) == Some(b'[');
    if !is_next_to_arbitrary && arbitrary_start != -1 && arbitrary_end != -1 && arbitrary_start < slash_idx && slash_idx < arbitrary_end {
        slash_idx = last_index_of(b'/', arbitrary_start);
    }
    if slash_idx == -1 || slash_idx as usize == b.len() - 1 {
        return (modifier.to_string(), None);
    }
    let arbitrary = is_arbitrary_value(modifier);
    if arbitrary && !modifier.contains("]/[") {
        return (modifier.to_string(), None);
    }
    let s = slash_idx as usize;
    (modifier[..s].to_string(), Some(modifier[s + 1..].to_string()))
}

fn unwrap_arbitrary_modifier(m: &str) -> String {
    normalize(&m[1..m.len() - 1], None)
}

/// `asColor(modifier, options, { tailwindConfig })`
pub fn as_color(modifier: &str, options: &PluginOptions, cfg: &TwConfig) -> Value {
    let v = lookup(options, modifier);
    if !v.is_undefined() {
        return parse_color_format(&v);
    }
    let (color, alpha) = split_utility_modifier(modifier);
    if let Some(alpha) = alpha {
        let mut normalized = lookup(options, &color);
        if normalized.is_nullish() {
            normalized = if is_arbitrary_value(&color) { Value::str(&color[1..color.len() - 1]) } else { Value::Undefined };
        }
        if normalized.is_undefined() {
            return Value::Undefined;
        }
        let normalized = parse_color_format(&normalized);
        if is_arbitrary_value(&alpha) {
            return with_alpha_value(&normalized, &Value::str(unwrap_arbitrary_modifier(&alpha)), &Value::Undefined);
        }
        let opacity = cfg.value.get("theme").get("opacity").get(&alpha);
        if opacity.is_undefined() {
            return Value::Undefined;
        }
        return with_alpha_value(&normalized, &opacity, &Value::Undefined);
    }
    as_value(modifier, options, &util::dt_color)
}

fn type_map(ty: &str, modifier: &str, options: &PluginOptions, cfg: &TwConfig) -> Option<Value> {
    Some(match ty {
        "any" => as_value(modifier, options, &|_| true),
        "color" => as_color(modifier, options, cfg),
        "url" => as_value(modifier, options, &util::dt_url),
        "image" => as_value(modifier, options, &util::dt_image),
        "length" => as_value(modifier, options, &util::dt_length),
        "percentage" => as_value(modifier, options, &util::dt_percentage),
        "position" => as_value(modifier, options, &util::dt_position),
        "lookup" => lookup(options, modifier),
        "generic-name" => as_value(modifier, options, &util::dt_generic_name),
        "family-name" => as_value(modifier, options, &util::dt_family_name),
        "number" => as_value(modifier, options, &util::dt_number),
        "line-width" => as_value(modifier, options, &util::dt_line_width),
        "absolute-size" => as_value(modifier, options, &util::dt_absolute_size),
        "relative-size" => as_value(modifier, options, &util::dt_relative_size),
        "shadow" => as_value(modifier, options, &util::dt_shadow),
        "size" => as_value(modifier, options, &util::dt_background_size),
        _ => return None,
    })
}

const SUPPORTED_TYPES: &[&str] = &[
    "any", "color", "url", "image", "length", "percentage", "position", "lookup", "generic-name", "family-name", "number", "line-width",
    "absolute-size", "relative-size", "shadow", "size",
];

/// `coerceValue(types, modifier, options, tailwindConfig)` → `[value, type, modifier]`
/// (`None` for the empty array)
pub fn coerce_value(types: &[TypeSpec], modifier: &str, options: &PluginOptions, cfg: &TwConfig) -> Option<(Value, String, Option<String>)> {
    if !options.values.is_nullish() && has_key(options, modifier) {
        for t in types {
            let Some(result) = type_map(&t.ty, modifier, options, cfg) else {
                // `typeMap[type]` is not a function → TypeError; types are always known in core
                continue;
            };
            if result.is_undefined() {
                continue;
            }
            return Some((result, t.ty.clone(), None));
        }
    }
    if is_arbitrary_value(modifier) {
        let arbitrary = &modifier[1..modifier.len() - 1];
        let (explicit_type, mut value) = match arbitrary.find(':') {
            Some(i) => (Some(&arbitrary[..i]), &arbitrary[i + 1..]),
            None => (None, arbitrary),
        };
        // /^[\w-_]+$/.test(explicitType) — `undefined` stringifies to a match
        let et = explicit_type.unwrap_or("undefined");
        let word = !et.is_empty() && et.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-');
        if !word {
            value = arbitrary;
        } else if let Some(t) = explicit_type {
            if !SUPPORTED_TYPES.contains(&t) {
                return None;
            }
        }
        if !value.is_empty() {
            if let Some(t) = explicit_type.filter(|t| SUPPORTED_TYPES.contains(t)) {
                return Some((as_value(&format!("[{value}]"), options, &|_| true), t.to_string(), None));
            }
        }
    }
    get_matching_types(types, modifier, options, cfg).into_iter().next()
}

/// `getMatchingTypes(types, rawModifier, options, tailwindConfig)`
pub fn get_matching_types(types: &[TypeSpec], raw_modifier: &str, options: &PluginOptions, cfg: &TwConfig) -> Vec<(Value, String, Option<String>)> {
    let modifiers_enabled = cfg.generalized_modifiers;
    let (mut modifier, mut utility_modifier) = split_utility_modifier(raw_modifier);
    let can_use = modifiers_enabled
        && !options.modifiers.is_nullish()
        && (options.modifiers == Value::str("any")
            || (options.modifiers.is_object_like()
                && (utility_modifier.as_deref().is_some_and(is_arbitrary_value)
                    || match &utility_modifier {
                        Some(u) => !options.modifiers.get(u).is_undefined() || options.modifiers.as_obj().is_some_and(|o| o.contains_key(u)),
                        None => options.modifiers.as_obj().is_some_and(|o| o.contains_key("undefined")),
                    })));
    if !can_use {
        modifier = raw_modifier.to_string();
        utility_modifier = None;
    }
    if utility_modifier.is_some() && modifier.is_empty() {
        modifier = "DEFAULT".into();
    }
    if let Some(u) = utility_modifier.clone() {
        if options.modifiers.is_object_like() {
            let config_value = options.modifiers.get(&u);
            if !config_value.is_nullish() {
                utility_modifier = Some(config_value.to_js_string());
            } else if is_arbitrary_value(&u) {
                utility_modifier = Some(unwrap_arbitrary_modifier(&u));
            }
        }
    }
    let mut out = Vec::new();
    for t in types {
        let Some(result) = type_map(&t.ty, &modifier, options, cfg) else { continue };
        if result.is_undefined() {
            continue;
        }
        out.push((result, t.ty.clone(), utility_modifier.clone()));
    }
    out
}
