//! `lib/setupContextUtils.js`: the context, the plugin API and plugin registration.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::js::{Object, Value};
use crate::postcss::{Css, Flow, Kind, NodeId};
use crate::selector::{self, SFlow, SKind, Sel};

use super::config::{dlv, flag_enabled};
use super::object_styles::parse_object_styles;
use super::offsets::{Layer, Offsets, RuleOffset, SortFn, SortValue, VariantId};
use super::plugin_utils::coerce_value;
use super::util::{self, name_class, to_path, transform_theme_value, Screen};
use super::TwConfig;

/// `{ type, preferOnConflict }`
#[derive(Clone, Debug)]
pub struct TypeSpec {
    pub ty: String,
    pub prefer_on_conflict: bool,
}

/// Plugin options as they end up in `meta.options`
#[derive(Clone, Debug, Default)]
pub struct PluginOptions {
    pub respect_prefix: Option<bool>,
    pub respect_important: Option<bool>,
    pub preserve_source: Option<bool>,
    pub types: Vec<TypeSpec>,
    pub has_types: bool,
    pub values: Value,
    pub supports_negative_values: bool,
    /// `false` / `'any'` / object
    pub modifiers: Value,
}

#[derive(Clone, Debug)]
pub struct Format {
    pub format: String,
    pub respect_prefix: bool,
}

/// `[meta, rule]` meta
#[derive(Clone, Debug)]
pub struct Meta {
    pub sort: RuleOffset,
    pub layer: Layer,
    pub options: Option<Arc<PluginOptions>>,
    pub important: bool,
    pub collected_formats: Option<Vec<Format>>,
}

pub type DynRule = Arc<dyn Fn(&TwConfig, &str, bool) -> Vec<Value> + Send + Sync>;

#[derive(Clone)]
pub enum PluginRule {
    Static(NodeId),
    Dynamic(DynRule),
}

pub type Match = (Meta, NodeId);

// ---- variants

/// Arguments a variant receives (`{ modifier, value }`)
#[derive(Clone, Debug)]
pub struct VariantArgs {
    pub modifier: Option<String>,
    pub value: VarValue,
}

/// `args.value` / `options.values[key]`
#[derive(Clone)]
pub enum VarValue {
    /// `sharedState.NONE`
    None,
    Str(String),
    Screen(Screen),
    /// a function value (`pseudoVariants` entries like `visited`)
    Fn(Arc<dyn Fn(&mut UserVariantCtx) -> String + Send + Sync>),
    Other(Value),
}

impl std::fmt::Debug for VarValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VarValue::None => write!(f, "NONE"),
            VarValue::Str(s) => write!(f, "{s:?}"),
            VarValue::Screen(s) => write!(f, "{s:?}"),
            VarValue::Fn(_) => write!(f, "[fn]"),
            VarValue::Other(v) => write!(f, "{v:?}"),
        }
    }
}

/// What a variant function returns (after `addVariant`'s wrapping)
pub enum VRet {
    Undefined,
    Null,
    Str(String),
    Fns(Vec<VFn>),
}

pub type VFn = Arc<dyn Fn(&mut VariantApi) -> Result<VRet, String> + Send + Sync>;

/// The api object `applyVariant` passes to variant functions.
pub struct VariantApi<'a> {
    pub css: &'a mut Css,
    pub clone: NodeId,
    pub formats: Vec<Format>,
    pub args: VariantArgs,
    pub respect_prefix: bool,
    pub separator: String,
}

impl VariantApi<'_> {
    fn prepare_backup(&mut self) {
        if self.css[self.clone].raws.needed_backup {
            return;
        }
        self.css[self.clone].raws.needed_backup = true;
        self.css.walk_rules(self.clone, &mut |css, r| {
            css[r].raws.original_selector = Some(css[r].selector.clone());
            Flow::Continue
        });
    }

    /// `api.container`
    pub fn container(&mut self) -> NodeId {
        self.prepare_backup();
        self.clone
    }

    pub fn format(&mut self, s: &str) {
        self.formats.push(Format { format: s.to_string(), respect_prefix: self.respect_prefix });
    }

    /// `api.wrap(postcss.atRule({ name, params }))`
    pub fn wrap(&mut self, name: &str, params: &str) {
        let wrapper = self.css.at_rule(name, params);
        let nodes = self.css.children(self.clone).to_vec();
        self.css.remove_all(self.clone);
        self.css.append(wrapper, crate::postcss::Add::Nodes(nodes));
        self.css.append(self.clone, crate::postcss::Add::Node(wrapper));
    }
}

/// The view user-level variant functions get (`{ container, separator, args }`)
pub struct UserVariantCtx<'a, 'b> {
    pub api: &'a mut VariantApi<'b>,
}

impl UserVariantCtx<'_, '_> {
    pub fn container(&mut self) -> NodeId {
        self.api.container()
    }
    pub fn css(&mut self) -> &mut Css {
        self.api.css
    }
}

/// What a user-level variant function returns
pub enum URet {
    Undefined,
    Null,
    Str(String),
    Arr(Vec<String>),
}

pub type UserVFn = Arc<dyn Fn(&mut UserVariantCtx) -> Result<URet, String> + Send + Sync>;

pub enum VDef {
    Str(String),
    Fn(UserVFn),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VariantType {
    Add,
    Match,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VariantInfo {
    Base,
    Dynamic,
}

/// `context.variantOptions.get(variant)`
#[derive(Clone, Debug, Default)]
pub struct VariantOptions {
    pub id: Option<VariantId>,
    pub sort: Option<SortFn>,
    /// `options.value` (only set for addVariant screens and matchVariant value variants)
    pub value: Option<VarValueSort>,
    pub vtype: Option<VariantType>,
    pub variant_info: Option<VariantInfo>,
    pub respect_prefix_false: bool,
    pub has_values: bool,
}

#[derive(Clone, Debug)]
pub enum VarValueSort {
    Screen(Screen),
    Str(String),
    Other,
}

#[derive(Clone)]
pub struct VariantEntry {
    pub sort: RuleOffset,
    pub f: VFn,
    pub container: Option<NodeId>,
}

pub struct Context<'c> {
    pub css: Css,
    pub cfg: &'c TwConfig,
    pub candidate_rule_map: HashMap<String, Vec<(Meta, PluginRule)>>,
    pub variant_map: HashMap<String, Vec<VariantEntry>>,
    pub variant_options: HashMap<String, VariantOptions>,
    pub offsets: Offsets,
    pub class_cache: HashMap<String, Vec<Match>>,
    pub not_class_cache: HashSet<String>,
    pub candidate_rule_cache: HashMap<String, Vec<usize>>,
    /// `context.ruleCache` in insertion order (`None` = deleted)
    pub rule_cache: Vec<Option<(RuleOffset, NodeId)>>,
    pub apply_class_cache: HashMap<String, Vec<Match>>,
    pub changed_content: Vec<String>,
    variant_identifier: u32,
    /// `getClasses` cache (parse results only depend on the selector)
    pub errors: Vec<String>,
}

pub type Plugin = Arc<dyn Fn(&mut PluginApi) -> Result<(), String> + Send + Sync>;

pub enum Style {
    Node(NodeId),
    Obj(Value),
}

/// Rule function a `matchUtilities` entry gets: `(value, { modifier }) => styles`
pub type RuleFn = Arc<dyn Fn(&Value, &Option<String>) -> Value + Send + Sync>;

#[derive(Clone, Default)]
pub struct MatchOptions {
    pub values: Value,
    pub types: Vec<TypeSpec>,
    pub supports_negative_values: bool,
    pub modifiers: Value,
    pub respect_prefix: Option<bool>,
    pub respect_important: Option<bool>,
}

pub fn types(list: &[&str]) -> Vec<TypeSpec> {
    list.iter().map(|t| TypeSpec { ty: t.to_string(), prefer_on_conflict: false }).collect()
}

pub struct PluginApi<'a, 'c> {
    pub ctx: &'a mut Context<'c>,
    pub variant_list: &'a mut Vec<String>,
}

impl<'c> Context<'c> {
    pub fn new(cfg: &'c TwConfig) -> Self {
        Context {
            css: Css::new(),
            cfg,
            candidate_rule_map: HashMap::new(),
            variant_map: HashMap::new(),
            variant_options: HashMap::new(),
            offsets: Offsets::new(),
            class_cache: HashMap::new(),
            not_class_cache: cfg.blocklist.iter().cloned().collect(),
            candidate_rule_cache: HashMap::new(),
            rule_cache: Vec::new(),
            apply_class_cache: HashMap::new(),
            changed_content: Vec::new(),
            variant_identifier: 0,
            errors: Vec::new(),
        }
    }

    pub fn tailwind_config(&self) -> &Value {
        &self.cfg.value
    }

    pub fn push_rule(&mut self, identifier: String, meta: Meta, rule: PluginRule) {
        self.candidate_rule_map.entry(identifier).or_default().push((meta, rule));
    }
}

const NOT_ON_DEMAND: &str = "*";

/// `getClasses(selector, ignoreNot)`
fn get_classes(selector: &str) -> Result<Vec<String>, String> {
    let mut sel = Sel::new();
    let root = selector::parse(&mut sel, selector).map_err(|e| e.0)?;
    sel.walk_pseudos(root, &mut |s, p| {
        if s[p].val() == ":not" {
            s.remove(p);
        }
        SFlow::Continue
    });
    Ok(sel.class_values(root))
}

/// `extractCandidates(node)` at depth 0: (containsNonOnDemandable, classes)
fn extract_candidates(css: &mut Css, node: NodeId) -> Result<(bool, Vec<String>), String> {
    let mut selectors: Vec<String> = Vec::new();
    match css[node].kind {
        Kind::Rule => selectors.extend(css.selectors(node)),
        Kind::AtRule => {
            css.walk_rules(node, &mut |css, r| {
                selectors.extend(css.selectors(r));
                Flow::Continue
            });
        }
        _ => {}
    }
    let mut contains_non_on_demandable = false;
    let mut classes = Vec::new();
    for s in selectors {
        let c = get_classes(&s)?;
        if c.is_empty() {
            contains_non_on_demandable = true;
        }
        classes.extend(c);
    }
    let non = contains_non_on_demandable || classes.is_empty();
    Ok((non, classes))
}

/// `withIdentifiers(styles)`
fn with_identifiers(css: &mut Css, styles: Vec<Style>) -> Result<Vec<(String, NodeId)>, String> {
    let mut nodes = Vec::new();
    for s in styles {
        match s {
            Style::Node(n) => nodes.push(n),
            Style::Obj(v) => nodes.extend(parse_object_styles(css, &v)?),
        }
    }
    let mut out = Vec::new();
    for node in nodes {
        let (non, mut candidates) = extract_candidates(css, node)?;
        if non {
            candidates.insert(0, NOT_ON_DEMAND.to_string());
        }
        for c in candidates {
            out.push((c, node));
        }
    }
    Ok(out)
}

impl<'a, 'c> PluginApi<'a, 'c> {
    pub fn config_value(&self) -> &Value {
        &self.ctx.cfg.value
    }

    /// `config(path, default)` with a dotted path
    pub fn config(&self, path: &str) -> Value {
        let parts: Vec<String> = path.split('.').map(|s| s.to_string()).collect();
        dlv(&self.ctx.cfg.value, &parts, Value::Undefined)
    }

    /// `theme(path, defaultValue)`
    pub fn theme_or(&self, path: &str, default: Value) -> Value {
        let parts = to_path(path).unwrap_or_default();
        let mut full = vec!["theme".to_string()];
        full.extend(parts.iter().cloned());
        let value = dlv(&self.ctx.cfg.value, &full, default);
        transform_theme_value(parts.first().map(|s| s.as_str()).unwrap_or(""), value, &Object::new().into_value())
    }

    pub fn theme(&self, path: &str) -> Value {
        self.theme_or(path, Value::Undefined)
    }

    /// `corePlugins(name)`
    pub fn core_plugins(&self, name: &str) -> bool {
        self.ctx.cfg.core_plugins.iter().any(|p| p == name)
    }

    /// `prefix(selector)`
    pub fn prefix(&self, selector: &str) -> String {
        util::prefix_selector(&self.ctx.cfg.prefix, selector, false).unwrap_or_else(|_| selector.to_string())
    }

    fn prefix_identifier(&self, identifier: &str, respect_prefix: bool) -> String {
        if identifier == NOT_ON_DEMAND {
            return identifier.to_string();
        }
        if !respect_prefix {
            return identifier.to_string();
        }
        format!("{}{}", self.ctx.cfg.prefix, identifier)
    }

    pub fn add_base(&mut self, styles: Vec<Style>) -> Result<(), String> {
        for (identifier, rule) in with_identifiers(&mut self.ctx.css, styles)? {
            // prefixIdentifier(identifier, {}) — respectPrefix undefined → falsy
            let id = self.prefix_identifier(&identifier, false);
            let sort = self.ctx.offsets.create(Layer::Base);
            self.ctx.push_rule(id, Meta { sort, layer: Layer::Base, options: None, important: false, collected_formats: None }, PluginRule::Static(rule));
        }
        Ok(())
    }

    pub fn add_defaults(&mut self, group: &str, declarations: Value) -> Result<(), String> {
        let mut o = Object::new();
        o.insert(format!("@defaults {group}"), declarations);
        for (identifier, rule) in with_identifiers(&mut self.ctx.css, vec![Style::Obj(o.into_value())])? {
            let id = self.prefix_identifier(&identifier, false);
            let sort = self.ctx.offsets.create(Layer::Defaults);
            self.ctx.push_rule(id, Meta { sort, layer: Layer::Defaults, options: None, important: false, collected_formats: None }, PluginRule::Static(rule));
        }
        Ok(())
    }

    fn add_static(&mut self, styles: Vec<Style>, layer: Layer, options: PluginOptions) -> Result<(), String> {
        let options = Arc::new(options);
        for (identifier, rule) in with_identifiers(&mut self.ctx.css, styles)? {
            let id = self.prefix_identifier(&identifier, options.respect_prefix.unwrap_or(false));
            let sort = self.ctx.offsets.create(layer);
            self.ctx.push_rule(id, Meta { sort, layer, options: Some(options.clone()), important: false, collected_formats: None }, PluginRule::Static(rule));
        }
        Ok(())
    }

    pub fn add_components(&mut self, styles: Vec<Style>, preserve_source: bool, respect_prefix: bool) -> Result<(), String> {
        let o = PluginOptions {
            preserve_source: Some(preserve_source),
            respect_prefix: Some(respect_prefix),
            respect_important: Some(false),
            ..Default::default()
        };
        self.add_static(styles, Layer::Components, o)
    }

    pub fn add_utilities(&mut self, styles: Vec<Style>, preserve_source: bool, respect_prefix: bool) -> Result<(), String> {
        let o = PluginOptions {
            preserve_source: Some(preserve_source),
            respect_prefix: Some(respect_prefix),
            respect_important: Some(true),
            ..Default::default()
        };
        self.add_static(styles, Layer::Utilities, o)
    }

    pub fn match_utilities(&mut self, utilities: Vec<(&str, RuleFn)>, options: MatchOptions) {
        self.match_impl(utilities, options, Layer::Utilities, true)
    }

    pub fn match_components(&mut self, utilities: Vec<(&str, RuleFn)>, options: MatchOptions) {
        self.match_impl(utilities, options, Layer::Components, false)
    }

    fn match_impl(&mut self, utilities: Vec<(&str, RuleFn)>, options: MatchOptions, layer: Layer, respect_important: bool) {
        let types = if options.types.is_empty() { types(&["any"]) } else { options.types.clone() };
        let opts = Arc::new(PluginOptions {
            respect_prefix: Some(options.respect_prefix.unwrap_or(true)),
            respect_important: Some(options.respect_important.unwrap_or(respect_important)),
            preserve_source: None,
            types,
            has_types: true,
            values: options.values.clone(),
            supports_negative_values: options.supports_negative_values,
            modifiers: if options.modifiers.is_undefined() { Value::Bool(false) } else { options.modifiers.clone() },
        });
        let offset = self.ctx.offsets.create(layer);
        for (identifier, rule) in utilities {
            let prefixed = self.prefix_identifier(identifier, true);
            let identifier = identifier.to_string();
            let o = opts.clone();
            let wrapped: DynRule = Arc::new(move |cfg: &TwConfig, modifier: &str, is_only_plugin: bool| {
                let (value, coerced_type, utility_modifier) = match coerce_value(&o.types, modifier, &o, cfg) {
                    Some(r) => r,
                    None => return Vec::new(),
                };
                if value.is_undefined() {
                    return Vec::new();
                }
                if !o.types.iter().any(|t| t.ty == coerced_type) && !is_only_plugin {
                    return Vec::new();
                }
                if !is_valid_value(&value) {
                    return Vec::new();
                }
                let result = if cfg.generalized_modifiers { rule(&value, &utility_modifier) } else { rule(&value, &None) };
                let list: Vec<Value> = match result {
                    Value::Arr(a) => a.to_vec(),
                    v => vec![v],
                };
                let class = name_class(&identifier, modifier);
                list.into_iter()
                    .filter(|d| d.truthy())
                    .map(|d| {
                        let mut obj = Object::new();
                        obj.insert(class.clone(), d);
                        obj.into_value()
                    })
                    .collect()
            });
            self.ctx.push_rule(
                prefixed,
                Meta { sort: offset.clone(), layer, options: Some(opts.clone()), important: false, collected_formats: None },
                PluginRule::Dynamic(wrapped),
            );
        }
    }

    /// `addVariant(variantName, variantFunctions, options)`
    pub fn add_variant(&mut self, name: &str, defs: Vec<VDef>, options: VariantOptions) -> Result<(), String> {
        let is_match = options.vtype == Some(VariantType::Match);
        let mut fns: Vec<VFn> = Vec::new();
        for d in defs {
            match d {
                VDef::Str(s) => {
                    if !is_valid_variant_format_string(&s) {
                        return Err(format!(
                            "Your custom variant `{name}` has an invalid format string. Make sure it's an at-rule or contains a `&` placeholder."
                        ));
                    }
                    fns.push(parse_variant(&s)?);
                }
                VDef::Fn(f) => {
                    let vname = name.to_string();
                    fns.push(Arc::new(move |api: &mut VariantApi| {
                        let _ = is_match;
                        let result = {
                            let mut ctx = UserVariantCtx { api };
                            f(&mut ctx)?
                        };
                        match result {
                            URet::Str(s) => {
                                if !is_valid_variant_format_string(&s) {
                                    return Err(format!(
                                        "Your custom variant `{vname}` has an invalid format string. Make sure it's an at-rule or contains a `&` placeholder."
                                    ));
                                }
                                let pv = parse_variant(&s)?;
                                pv(api)?;
                                Ok(VRet::Undefined)
                            }
                            URet::Arr(list) => {
                                let mut out = Vec::new();
                                for s in list {
                                    out.push(parse_variant(&s)?);
                                }
                                Ok(VRet::Fns(out))
                            }
                            URet::Null => Ok(VRet::Null),
                            URet::Undefined => Ok(VRet::Undefined),
                        }
                    }));
                }
            }
        }
        // insertInto(variantList, variantName, options) — no `before` in core
        self.variant_list.push(name.to_string());
        let entries: Vec<VariantEntry> = fns
            .into_iter()
            .map(|f| VariantEntry { sort: self.ctx.offsets.create(Layer::Variants), f, container: None })
            .collect();
        self.ctx.variant_map.insert(name.to_string(), entries);
        self.ctx.variant_options.insert(name.to_string(), options);
        Ok(())
    }

    /// `matchVariant(variant, variantFn, options)`
    pub fn match_variant(
        &mut self,
        variant: &str,
        variant_fn: MatchVariantFn,
        values: Vec<(String, VarValue)>,
        mut options: VariantOptions,
    ) -> Result<(), String> {
        let id = match options.id.clone() {
            Some(id) => id,
            None => {
                self.ctx.variant_identifier += 1;
                VariantId::Num(self.ctx.variant_identifier)
            }
        };
        options.id = Some(id.clone());
        let is_special = variant == "@";
        let modifiers_enabled = self.ctx.cfg.generalized_modifiers;
        let has_default = values.iter().any(|(k, _)| k == "DEFAULT");
        options.has_values = true;
        for (key, value) in values.iter() {
            if key == "DEFAULT" {
                continue;
            }
            let name = if is_special { format!("{variant}{key}") } else { format!("{variant}-{key}") };
            let vf = variant_fn.clone();
            let captured = value.clone();
            let f: UserVFn = Arc::new(move |ctx: &mut UserVariantCtx| {
                let modifier = if modifiers_enabled { ctx.api.args.modifier.clone() } else { None };
                vf(&captured, &modifier, ctx)
            });
            let mut o = options.clone();
            o.value = Some(match value {
                VarValue::Screen(s) => VarValueSort::Screen(s.clone()),
                VarValue::Str(s) => VarValueSort::Str(s.clone()),
                _ => VarValueSort::Other,
            });
            o.vtype = Some(VariantType::Match);
            o.variant_info = Some(VariantInfo::Base);
            self.add_variant(&name, vec![VDef::Fn(f)], o)?;
        }
        let default = values.iter().find(|(k, _)| k == "DEFAULT").map(|(_, v)| v.clone());
        let vf = variant_fn.clone();
        let f: UserVFn = Arc::new(move |ctx: &mut UserVariantCtx| {
            let value = ctx.api.args.value.clone();
            if matches!(value, VarValue::None) && !has_default {
                return Ok(URet::Null);
            }
            let value = if matches!(value, VarValue::None) { default.clone().unwrap() } else { value };
            let modifier = if modifiers_enabled { ctx.api.args.modifier.clone() } else { None };
            vf(&value, &modifier, ctx)
        });
        let mut o = options;
        o.vtype = Some(VariantType::Match);
        o.variant_info = Some(VariantInfo::Dynamic);
        self.add_variant(variant, vec![VDef::Fn(f)], o)
    }
}

pub type MatchVariantFn = Arc<dyn Fn(&VarValue, &Option<String>, &mut UserVariantCtx) -> Result<URet, String> + Send + Sync>;

/// `isSyntacticallyValidPropertyValue` applied to whatever value type a plugin got
fn is_valid_value(v: &Value) -> bool {
    match v {
        Value::Str(s) => util::is_syntactically_valid_property_value(s),
        Value::Arr(a) => {
            // indexing an array yields its elements; only single-character strings matter
            let chars: String = a
                .iter()
                .map(|x| match x {
                    Value::Str(s) if s.chars().count() == 1 => s.to_string(),
                    _ => "\u{0}".to_string(),
                })
                .collect();
            util::is_syntactically_valid_property_value(&chars)
        }
        _ => true,
    }
}

pub fn is_valid_variant_format_string(format: &str) -> bool {
    format.starts_with('@') || format.contains('&')
}

/// `parseVariantFormatString(input)`
fn parse_variant_format_string(input: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth: i32 = 0;
    let chars: Vec<char> = input.chars().collect();
    let mut idx = 0;
    while idx < chars.len() {
        let c = chars[idx];
        if c == '\\' {
            current.push('\\');
            idx += 1;
            match chars.get(idx) {
                Some(n) => current.push(*n),
                None => current.push_str("undefined"),
            }
        } else if c == '{' {
            depth += 1;
            parts.push(crate::js::trim(&current).to_string());
            current.clear();
        } else if c == '}' {
            depth -= 1;
            if depth < 0 {
                return Err("Your { and } are unbalanced.".into());
            }
            parts.push(crate::js::trim(&current).to_string());
            current.clear();
        } else {
            current.push(c);
        }
        idx += 1;
    }
    if !current.is_empty() {
        parts.push(crate::js::trim(&current).to_string());
    }
    Ok(parts.into_iter().filter(|p| !p.is_empty()).collect())
}

enum VOp {
    Format(String),
    Wrap(String, String),
}

/// `parseVariant(variant)`
pub fn parse_variant(variant: &str) -> Result<VFn, String> {
    let v: String = variant.chars().filter(|c| *c != '\n').collect();
    let v = util::collapse_ws(&v);
    let v = crate::js::trim(&v).to_string();
    let mut ops: Vec<VOp> = Vec::new();
    for s in parse_variant_format_string(&v)? {
        if !s.starts_with('@') {
            ops.push(VOp::Format(s));
        } else {
            // /@(\S*)( .+|[({].*)?/
            let rest = &s[1..];
            let name_end = rest.find(|c: char| crate::js::is_js_whitespace(c)).unwrap_or(rest.len());
            let name = &rest[..name_end];
            let after = &rest[name_end..];
            // `( .+|[({].*)?` right after the name: either a space followed by at least one
            // character, or `(`/`{` — but `\S*` already consumed `(`/`{`
            let params = if after.starts_with(' ') && after.chars().count() >= 2 {
                let line_end = after.find(['\n', '\r', '\u{2028}', '\u{2029}']).unwrap_or(after.len());
                crate::js::trim(&after[..line_end]).to_string()
            } else {
                String::new()
            };
            ops.push(VOp::Wrap(name.to_string(), params));
        }
    }
    ops.reverse();
    let ops = Arc::new(ops);
    Ok(Arc::new(move |api: &mut VariantApi| {
        for op in ops.iter() {
            match op {
                VOp::Format(s) => api.format(s),
                VOp::Wrap(n, p) => api.wrap(n, p),
            }
        }
        Ok(VRet::Undefined)
    }))
}

impl<'c> Context<'c> {
    /// `context.offsets.recordVariants` + rebuilding `variantMap` with offsets
    pub fn finish_variants(&mut self, variant_list: &[String]) {
        for v in variant_list {
            let n = self.variant_map.get(v).map(|l| l.len()).unwrap_or(0) as u64;
            self.offsets.record_variant(v, n);
        }
        let names: Vec<String> = self.variant_map.keys().cloned().collect();
        for name in names {
            let mut list = self.variant_map.remove(&name).unwrap();
            for (idx, e) in list.iter_mut().enumerate() {
                e.sort = self.offsets.for_variant(&name, idx as u64);
            }
            self.variant_map.insert(name, list);
        }
    }
}

/// Selector of a rule without a parent (used by debug helpers)
pub fn _unused(_: SKind, _: SortValue) {}

pub fn flag(cfg: &TwConfig, f: &str) -> bool {
    flag_enabled(&cfg.value, f)
}
