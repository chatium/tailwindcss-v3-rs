//! `corePlugins.js`: the core utility plugins and the variant plugins.

use std::collections::HashSet;
use std::sync::Arc;

use crate::js::{self, Object, Value};
use crate::postcss::{Add, Flow, NodeId};

use super::context::{
    types, MatchOptions, MatchVariantFn, PluginApi, RuleFn, Style, TypeSpec, URet, UserVFn, UserVariantCtx, VDef, VarValue, VariantOptions,
};
use super::offsets::{SortFn, VariantId};
use super::util::{
    self, build_media_query, escape_class_name, flatten_color_palette, format_box_shadow_value, normalize, normalize_attribute_selectors,
    normalize_screens, parse_animation_value, parse_box_shadow_value, to_color_value, transform_theme_value, with_alpha_value,
    with_alpha_variable, Screen,
};

pub type CorePlugin = fn(&mut PluginApi) -> Result<(), String>;

// ---- small builders

fn s(v: &str) -> Value {
    Value::str(v)
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut o = Object::new();
    for (k, v) in pairs {
        o.insert(k, v);
    }
    o.into_value()
}

fn objs(pairs: &[(&str, &str)]) -> Value {
    obj(pairs.iter().map(|(k, v)| (*k, s(v))).collect())
}

/// `{ '.a': { prop: value, … }, … }`
fn statics(rules: &[(&str, &[(&str, &str)])]) -> Value {
    obj(rules.iter().map(|(sel, decls)| (*sel, objs(decls))).collect())
}

fn add_utils(api: &mut PluginApi, v: Value) -> Result<(), String> {
    api.add_utilities(vec![Style::Obj(v)], false, true)
}

fn rf(f: impl Fn(&Value, &Option<String>) -> Value + Send + Sync + 'static) -> RuleFn {
    Arc::new(f)
}

fn str_of(v: &Value) -> String {
    v.to_js_string()
}

fn opts(values: Value, ty: &[&str]) -> MatchOptions {
    MatchOptions { values, types: types(ty), ..Default::default() }
}

fn values_only(values: Value) -> MatchOptions {
    MatchOptions { values, ..Default::default() }
}

fn without_default(o: Object) -> Value {
    let mut o = o;
    o.remove("DEFAULT");
    o.into_value()
}

const CSS_TRANSFORM_VALUE: &str =
    "translate(var(--tw-translate-x), var(--tw-translate-y)) rotate(var(--tw-rotate)) skewX(var(--tw-skew-x)) skewY(var(--tw-skew-y)) scaleX(var(--tw-scale-x)) scaleY(var(--tw-scale-y))";
const CSS_FILTER_VALUE: &str = "var(--tw-blur) var(--tw-brightness) var(--tw-contrast) var(--tw-grayscale) var(--tw-hue-rotate) var(--tw-invert) var(--tw-saturate) var(--tw-sepia) var(--tw-drop-shadow)";
const CSS_BACKDROP_FILTER_VALUE: &str = "var(--tw-backdrop-blur) var(--tw-backdrop-brightness) var(--tw-backdrop-contrast) var(--tw-backdrop-grayscale) var(--tw-backdrop-hue-rotate) var(--tw-backdrop-invert) var(--tw-backdrop-opacity) var(--tw-backdrop-saturate) var(--tw-backdrop-sepia)";

// ---- createUtilityPlugin

/// A property in a utility variation: a theme-value property or a fixed `[name, value]`.
#[derive(Clone)]
enum P {
    V(&'static str),
    Fixed(&'static str, Value),
}

fn defaults_p(group: &'static str) -> P {
    P::Fixed(group, Object::new().into_value())
}

struct Cup {
    key: &'static str,
    variations: Vec<Vec<(&'static str, Vec<P>)>>,
    filter_default: bool,
    supports_negative_values: bool,
    types: Option<Vec<TypeSpec>>,
}

fn cup(key: &'static str, variations: Vec<Vec<(&'static str, Vec<P>)>>) -> Cup {
    Cup { key, variations, filter_default: false, supports_negative_values: false, types: None }
}

fn single(prefix: &'static str, props: Vec<P>) -> Vec<Vec<(&'static str, Vec<P>)>> {
    vec![vec![(prefix, props)]]
}

impl Cup {
    fn neg(mut self) -> Self {
        self.supports_negative_values = true;
        self
    }
    fn ty(mut self, t: &[&str]) -> Self {
        self.types = Some(types(t));
        self
    }
    fn types(mut self, t: Vec<TypeSpec>) -> Self {
        self.types = Some(t);
        self
    }
    fn filter_default(mut self, f: bool) -> Self {
        self.filter_default = f;
        self
    }

    fn run(&self, api: &mut PluginApi) -> Result<(), String> {
        let key = self.key;
        for group in &self.variations {
            let mut utilities: Vec<(&str, RuleFn)> = Vec::new();
            for (class_prefix, properties) in group {
                let props = properties.clone();
                utilities.push((
                    class_prefix,
                    rf(move |value, _| {
                        let mut o = Object::new();
                        for p in &props {
                            match p {
                                P::Fixed(n, v) => o.insert(*n, v.clone()),
                                P::V(n) => o.insert(*n, transform_theme_value(key, value.clone(), &Object::new().into_value())),
                            }
                        }
                        o.into_value()
                    }),
                ));
            }
            let theme_values = api.theme(key);
            let values = if self.filter_default {
                let mut o = Object::new();
                if let Value::Obj(src) = if theme_values.is_nullish() { Object::new().into_value() } else { theme_values.clone() } {
                    for (k, v) in src.iter() {
                        if &**k != "DEFAULT" {
                            o.insert(k.clone(), v.clone());
                        }
                    }
                }
                o.into_value()
            } else {
                theme_values
            };
            let mut mo = MatchOptions { values, supports_negative_values: self.supports_negative_values, ..Default::default() };
            if let Some(t) = &self.types {
                mo.types = t.clone();
            }
            api.match_utilities(utilities, mo);
        }
        Ok(())
    }
}

fn v(p: &'static str) -> P {
    P::V(p)
}

// ---- color helpers

fn with_alpha(value: &Value, property: &[&str], variable: &str) -> Value {
    with_alpha_variable(value, property, variable)
}

fn color_values(api: &PluginApi, key: &str) -> Value {
    flatten_color_palette(&api.theme(key)).into_value()
}

fn color_values_no_default(api: &PluginApi, key: &str) -> Value {
    without_default(flatten_color_palette(&api.theme(key)))
}

/// `matchUtilities({ [name]: value => ({ [prop]: toColorValue(value) }) }, { values, type: ['color', 'any'] })`
fn simple_color(api: &mut PluginApi, name: &str, key: &str, prop: &'static str, ty: &[&str]) {
    let values = color_values(api, key);
    api.match_utilities(vec![(name, rf(move |v, _| obj(vec![(prop, to_color_value(v))])))], opts(values, ty));
}

/// `borderColor`-style: opacity variable when the opacity core plugin is enabled
fn alpha_color_rule(enabled: bool, props: &'static [&'static str], variable: &'static str) -> RuleFn {
    rf(move |value, _| {
        if !enabled {
            let mut o = Object::new();
            for p in props {
                o.insert(*p, to_color_value(value));
            }
            return o.into_value();
        }
        with_alpha(value, props, variable)
    })
}

// ---- the plugin table

pub fn core_plugins() -> Vec<(&'static str, CorePlugin)> {
    vec![
        ("preflight", preflight),
        ("container", container),
        ("accessibility", accessibility),
        ("pointerEvents", |api| add_utils(api, statics(&[(".pointer-events-none", &[("pointer-events", "none")]), (".pointer-events-auto", &[("pointer-events", "auto")])]))),
        ("visibility", |api| {
            add_utils(api, statics(&[(".visible", &[("visibility", "visible")]), (".invisible", &[("visibility", "hidden")]), (".collapse", &[("visibility", "collapse")])]))
        }),
        ("position", |api| {
            add_utils(
                api,
                statics(&[
                    (".static", &[("position", "static")]),
                    (".fixed", &[("position", "fixed")]),
                    (".absolute", &[("position", "absolute")]),
                    (".relative", &[("position", "relative")]),
                    (".sticky", &[("position", "sticky")]),
                ]),
            )
        }),
        ("inset", |api| {
            cup(
                "inset",
                vec![
                    vec![("inset", vec![v("inset")])],
                    vec![("inset-x", vec![v("left"), v("right")]), ("inset-y", vec![v("top"), v("bottom")])],
                    vec![
                        ("start", vec![v("inset-inline-start")]),
                        ("end", vec![v("inset-inline-end")]),
                        ("top", vec![v("top")]),
                        ("right", vec![v("right")]),
                        ("bottom", vec![v("bottom")]),
                        ("left", vec![v("left")]),
                    ],
                ],
            )
            .neg()
            .run(api)
        }),
        ("isolation", |api| add_utils(api, statics(&[(".isolate", &[("isolation", "isolate")]), (".isolation-auto", &[("isolation", "auto")])]))),
        ("zIndex", |api| cup("zIndex", single("z", vec![v("zIndex")])).neg().run(api)),
        ("order", |api| cup("order", single("order", vec![v("order")])).neg().run(api)),
        ("gridColumn", |api| cup("gridColumn", single("col", vec![v("gridColumn")])).run(api)),
        ("gridColumnStart", |api| cup("gridColumnStart", single("col-start", vec![v("gridColumnStart")])).neg().run(api)),
        ("gridColumnEnd", |api| cup("gridColumnEnd", single("col-end", vec![v("gridColumnEnd")])).neg().run(api)),
        ("gridRow", |api| cup("gridRow", single("row", vec![v("gridRow")])).run(api)),
        ("gridRowStart", |api| cup("gridRowStart", single("row-start", vec![v("gridRowStart")])).neg().run(api)),
        ("gridRowEnd", |api| cup("gridRowEnd", single("row-end", vec![v("gridRowEnd")])).neg().run(api)),
        ("float", |api| {
            add_utils(
                api,
                statics(&[
                    (".float-start", &[("float", "inline-start")]),
                    (".float-end", &[("float", "inline-end")]),
                    (".float-right", &[("float", "right")]),
                    (".float-left", &[("float", "left")]),
                    (".float-none", &[("float", "none")]),
                ]),
            )
        }),
        ("clear", |api| {
            add_utils(
                api,
                statics(&[
                    (".clear-start", &[("clear", "inline-start")]),
                    (".clear-end", &[("clear", "inline-end")]),
                    (".clear-left", &[("clear", "left")]),
                    (".clear-right", &[("clear", "right")]),
                    (".clear-both", &[("clear", "both")]),
                    (".clear-none", &[("clear", "none")]),
                ]),
            )
        }),
        ("margin", |api| {
            cup(
                "margin",
                vec![
                    vec![("m", vec![v("margin")])],
                    vec![("mx", vec![v("margin-left"), v("margin-right")]), ("my", vec![v("margin-top"), v("margin-bottom")])],
                    vec![
                        ("ms", vec![v("margin-inline-start")]),
                        ("me", vec![v("margin-inline-end")]),
                        ("mt", vec![v("margin-top")]),
                        ("mr", vec![v("margin-right")]),
                        ("mb", vec![v("margin-bottom")]),
                        ("ml", vec![v("margin-left")]),
                    ],
                ],
            )
            .neg()
            .run(api)
        }),
        ("boxSizing", |api| add_utils(api, statics(&[(".box-border", &[("box-sizing", "border-box")]), (".box-content", &[("box-sizing", "content-box")])]))),
        ("lineClamp", |api| {
            let values = api.theme("lineClamp");
            api.match_utilities(
                vec![(
                    "line-clamp",
                    rf(|value, _| {
                        obj(vec![
                            ("overflow", s("hidden")),
                            ("display", s("-webkit-box")),
                            ("-webkit-box-orient", s("vertical")),
                            ("-webkit-line-clamp", s(&str_of(value))),
                        ])
                    }),
                )],
                values_only(values),
            );
            add_utils(
                api,
                statics(&[(
                    ".line-clamp-none",
                    &[("overflow", "visible"), ("display", "block"), ("-webkit-box-orient", "horizontal"), ("-webkit-line-clamp", "none")],
                )]),
            )
        }),
        ("display", |api| {
            add_utils(
                api,
                statics(&[
                    (".block", &[("display", "block")]),
                    (".inline-block", &[("display", "inline-block")]),
                    (".inline", &[("display", "inline")]),
                    (".flex", &[("display", "flex")]),
                    (".inline-flex", &[("display", "inline-flex")]),
                    (".table", &[("display", "table")]),
                    (".inline-table", &[("display", "inline-table")]),
                    (".table-caption", &[("display", "table-caption")]),
                    (".table-cell", &[("display", "table-cell")]),
                    (".table-column", &[("display", "table-column")]),
                    (".table-column-group", &[("display", "table-column-group")]),
                    (".table-footer-group", &[("display", "table-footer-group")]),
                    (".table-header-group", &[("display", "table-header-group")]),
                    (".table-row-group", &[("display", "table-row-group")]),
                    (".table-row", &[("display", "table-row")]),
                    (".flow-root", &[("display", "flow-root")]),
                    (".grid", &[("display", "grid")]),
                    (".inline-grid", &[("display", "inline-grid")]),
                    (".contents", &[("display", "contents")]),
                    (".list-item", &[("display", "list-item")]),
                    (".hidden", &[("display", "none")]),
                ]),
            )
        }),
        ("aspectRatio", |api| cup("aspectRatio", single("aspect", vec![v("aspect-ratio")])).run(api)),
        ("size", |api| cup("size", single("size", vec![v("width"), v("height")])).run(api)),
        ("height", |api| cup("height", single("h", vec![v("height")])).run(api)),
        ("maxHeight", |api| cup("maxHeight", single("max-h", vec![v("maxHeight")])).run(api)),
        ("minHeight", |api| cup("minHeight", single("min-h", vec![v("minHeight")])).run(api)),
        ("width", |api| cup("width", single("w", vec![v("width")])).run(api)),
        ("minWidth", |api| cup("minWidth", single("min-w", vec![v("minWidth")])).run(api)),
        ("maxWidth", |api| cup("maxWidth", single("max-w", vec![v("maxWidth")])).run(api)),
        ("flex", |api| cup("flex", single("flex", vec![v("flex")])).run(api)),
        ("flexShrink", |api| {
            cup("flexShrink", vec![vec![("flex-shrink", vec![v("flex-shrink")])], vec![("shrink", vec![v("flex-shrink")])]]).run(api)
        }),
        ("flexGrow", |api| cup("flexGrow", vec![vec![("flex-grow", vec![v("flex-grow")])], vec![("grow", vec![v("flex-grow")])]]).run(api)),
        ("flexBasis", |api| cup("flexBasis", single("basis", vec![v("flex-basis")])).run(api)),
        ("tableLayout", |api| add_utils(api, statics(&[(".table-auto", &[("table-layout", "auto")]), (".table-fixed", &[("table-layout", "fixed")])]))),
        ("captionSide", |api| add_utils(api, statics(&[(".caption-top", &[("caption-side", "top")]), (".caption-bottom", &[("caption-side", "bottom")])]))),
        ("borderCollapse", |api| {
            add_utils(api, statics(&[(".border-collapse", &[("border-collapse", "collapse")]), (".border-separate", &[("border-collapse", "separate")])]))
        }),
        ("borderSpacing", border_spacing),
        ("transformOrigin", |api| cup("transformOrigin", single("origin", vec![v("transformOrigin")])).run(api)),
        ("translate", |api| {
            cup(
                "translate",
                vec![vec![
                    ("translate-x", vec![defaults_p("@defaults transform"), v("--tw-translate-x"), P::Fixed("transform", s(CSS_TRANSFORM_VALUE))]),
                    ("translate-y", vec![defaults_p("@defaults transform"), v("--tw-translate-y"), P::Fixed("transform", s(CSS_TRANSFORM_VALUE))]),
                ]],
            )
            .neg()
            .run(api)
        }),
        ("rotate", |api| {
            cup("rotate", single("rotate", vec![defaults_p("@defaults transform"), v("--tw-rotate"), P::Fixed("transform", s(CSS_TRANSFORM_VALUE))]))
                .neg()
                .run(api)
        }),
        ("skew", |api| {
            cup(
                "skew",
                vec![vec![
                    ("skew-x", vec![defaults_p("@defaults transform"), v("--tw-skew-x"), P::Fixed("transform", s(CSS_TRANSFORM_VALUE))]),
                    ("skew-y", vec![defaults_p("@defaults transform"), v("--tw-skew-y"), P::Fixed("transform", s(CSS_TRANSFORM_VALUE))]),
                ]],
            )
            .neg()
            .run(api)
        }),
        ("scale", |api| {
            cup(
                "scale",
                vec![
                    vec![(
                        "scale",
                        vec![defaults_p("@defaults transform"), v("--tw-scale-x"), v("--tw-scale-y"), P::Fixed("transform", s(CSS_TRANSFORM_VALUE))],
                    )],
                    vec![
                        ("scale-x", vec![defaults_p("@defaults transform"), v("--tw-scale-x"), P::Fixed("transform", s(CSS_TRANSFORM_VALUE))]),
                        ("scale-y", vec![defaults_p("@defaults transform"), v("--tw-scale-y"), P::Fixed("transform", s(CSS_TRANSFORM_VALUE))]),
                    ],
                ],
            )
            .neg()
            .run(api)
        }),
        ("transform", |api| {
            api.add_defaults(
                "transform",
                objs(&[
                    ("--tw-translate-x", "0"),
                    ("--tw-translate-y", "0"),
                    ("--tw-rotate", "0"),
                    ("--tw-skew-x", "0"),
                    ("--tw-skew-y", "0"),
                    ("--tw-scale-x", "1"),
                    ("--tw-scale-y", "1"),
                ]),
            )?;
            let gpu = CSS_TRANSFORM_VALUE.replacen(
                "translate(var(--tw-translate-x), var(--tw-translate-y))",
                "translate3d(var(--tw-translate-x), var(--tw-translate-y), 0)",
                1,
            );
            add_utils(
                api,
                obj(vec![
                    (".transform", obj(vec![("@defaults transform", Object::new().into_value()), ("transform", s(CSS_TRANSFORM_VALUE))])),
                    (".transform-cpu", objs(&[("transform", CSS_TRANSFORM_VALUE)])),
                    (".transform-gpu", obj(vec![("transform", s(&gpu))])),
                    (".transform-none", objs(&[("transform", "none")])),
                ]),
            )
        }),
        ("animation", animation),
        ("cursor", |api| cup("cursor", single("cursor", vec![v("cursor")])).run(api)),
        ("touchAction", touch_action),
        ("userSelect", |api| {
            add_utils(
                api,
                statics(&[
                    (".select-none", &[("user-select", "none")]),
                    (".select-text", &[("user-select", "text")]),
                    (".select-all", &[("user-select", "all")]),
                    (".select-auto", &[("user-select", "auto")]),
                ]),
            )
        }),
        ("resize", |api| {
            add_utils(
                api,
                statics(&[
                    (".resize-none", &[("resize", "none")]),
                    (".resize-y", &[("resize", "vertical")]),
                    (".resize-x", &[("resize", "horizontal")]),
                    (".resize", &[("resize", "both")]),
                ]),
            )
        }),
        ("scrollSnapType", |api| {
            api.add_defaults("scroll-snap-type", objs(&[("--tw-scroll-snap-strictness", "proximity")]))?;
            let d = || ("@defaults scroll-snap-type", Object::new().into_value());
            add_utils(
                api,
                obj(vec![
                    (".snap-none", objs(&[("scroll-snap-type", "none")])),
                    (".snap-x", obj(vec![d(), ("scroll-snap-type", s("x var(--tw-scroll-snap-strictness)"))])),
                    (".snap-y", obj(vec![d(), ("scroll-snap-type", s("y var(--tw-scroll-snap-strictness)"))])),
                    (".snap-both", obj(vec![d(), ("scroll-snap-type", s("both var(--tw-scroll-snap-strictness)"))])),
                    (".snap-mandatory", objs(&[("--tw-scroll-snap-strictness", "mandatory")])),
                    (".snap-proximity", objs(&[("--tw-scroll-snap-strictness", "proximity")])),
                ]),
            )
        }),
        ("scrollSnapAlign", |api| {
            add_utils(
                api,
                statics(&[
                    (".snap-start", &[("scroll-snap-align", "start")]),
                    (".snap-end", &[("scroll-snap-align", "end")]),
                    (".snap-center", &[("scroll-snap-align", "center")]),
                    (".snap-align-none", &[("scroll-snap-align", "none")]),
                ]),
            )
        }),
        ("scrollSnapStop", |api| {
            add_utils(api, statics(&[(".snap-normal", &[("scroll-snap-stop", "normal")]), (".snap-always", &[("scroll-snap-stop", "always")])]))
        }),
        ("scrollMargin", |api| {
            cup(
                "scrollMargin",
                vec![
                    vec![("scroll-m", vec![v("scroll-margin")])],
                    vec![
                        ("scroll-mx", vec![v("scroll-margin-left"), v("scroll-margin-right")]),
                        ("scroll-my", vec![v("scroll-margin-top"), v("scroll-margin-bottom")]),
                    ],
                    vec![
                        ("scroll-ms", vec![v("scroll-margin-inline-start")]),
                        ("scroll-me", vec![v("scroll-margin-inline-end")]),
                        ("scroll-mt", vec![v("scroll-margin-top")]),
                        ("scroll-mr", vec![v("scroll-margin-right")]),
                        ("scroll-mb", vec![v("scroll-margin-bottom")]),
                        ("scroll-ml", vec![v("scroll-margin-left")]),
                    ],
                ],
            )
            .neg()
            .run(api)
        }),
        ("scrollPadding", |api| {
            cup(
                "scrollPadding",
                vec![
                    vec![("scroll-p", vec![v("scroll-padding")])],
                    vec![
                        ("scroll-px", vec![v("scroll-padding-left"), v("scroll-padding-right")]),
                        ("scroll-py", vec![v("scroll-padding-top"), v("scroll-padding-bottom")]),
                    ],
                    vec![
                        ("scroll-ps", vec![v("scroll-padding-inline-start")]),
                        ("scroll-pe", vec![v("scroll-padding-inline-end")]),
                        ("scroll-pt", vec![v("scroll-padding-top")]),
                        ("scroll-pr", vec![v("scroll-padding-right")]),
                        ("scroll-pb", vec![v("scroll-padding-bottom")]),
                        ("scroll-pl", vec![v("scroll-padding-left")]),
                    ],
                ],
            )
            .run(api)
        }),
        ("listStylePosition", |api| {
            add_utils(api, statics(&[(".list-inside", &[("list-style-position", "inside")]), (".list-outside", &[("list-style-position", "outside")])]))
        }),
        ("listStyleType", |api| cup("listStyleType", single("list", vec![v("listStyleType")])).run(api)),
        ("listStyleImage", |api| cup("listStyleImage", single("list-image", vec![v("listStyleImage")])).run(api)),
        ("appearance", |api| add_utils(api, statics(&[(".appearance-none", &[("appearance", "none")]), (".appearance-auto", &[("appearance", "auto")])]))),
        ("columns", |api| cup("columns", single("columns", vec![v("columns")])).run(api)),
        ("breakBefore", |api| {
            add_utils(
                api,
                statics(&[
                    (".break-before-auto", &[("break-before", "auto")]),
                    (".break-before-avoid", &[("break-before", "avoid")]),
                    (".break-before-all", &[("break-before", "all")]),
                    (".break-before-avoid-page", &[("break-before", "avoid-page")]),
                    (".break-before-page", &[("break-before", "page")]),
                    (".break-before-left", &[("break-before", "left")]),
                    (".break-before-right", &[("break-before", "right")]),
                    (".break-before-column", &[("break-before", "column")]),
                ]),
            )
        }),
        ("breakInside", |api| {
            add_utils(
                api,
                statics(&[
                    (".break-inside-auto", &[("break-inside", "auto")]),
                    (".break-inside-avoid", &[("break-inside", "avoid")]),
                    (".break-inside-avoid-page", &[("break-inside", "avoid-page")]),
                    (".break-inside-avoid-column", &[("break-inside", "avoid-column")]),
                ]),
            )
        }),
        ("breakAfter", |api| {
            add_utils(
                api,
                statics(&[
                    (".break-after-auto", &[("break-after", "auto")]),
                    (".break-after-avoid", &[("break-after", "avoid")]),
                    (".break-after-all", &[("break-after", "all")]),
                    (".break-after-avoid-page", &[("break-after", "avoid-page")]),
                    (".break-after-page", &[("break-after", "page")]),
                    (".break-after-left", &[("break-after", "left")]),
                    (".break-after-right", &[("break-after", "right")]),
                    (".break-after-column", &[("break-after", "column")]),
                ]),
            )
        }),
        ("gridAutoColumns", |api| cup("gridAutoColumns", single("auto-cols", vec![v("gridAutoColumns")])).run(api)),
        ("gridAutoFlow", |api| {
            add_utils(
                api,
                statics(&[
                    (".grid-flow-row", &[("gridAutoFlow", "row")]),
                    (".grid-flow-col", &[("gridAutoFlow", "column")]),
                    (".grid-flow-dense", &[("gridAutoFlow", "dense")]),
                    (".grid-flow-row-dense", &[("gridAutoFlow", "row dense")]),
                    (".grid-flow-col-dense", &[("gridAutoFlow", "column dense")]),
                ]),
            )
        }),
        ("gridAutoRows", |api| cup("gridAutoRows", single("auto-rows", vec![v("gridAutoRows")])).run(api)),
        ("gridTemplateColumns", |api| cup("gridTemplateColumns", single("grid-cols", vec![v("gridTemplateColumns")])).run(api)),
        ("gridTemplateRows", |api| cup("gridTemplateRows", single("grid-rows", vec![v("gridTemplateRows")])).run(api)),
        ("flexDirection", |api| {
            add_utils(
                api,
                statics(&[
                    (".flex-row", &[("flex-direction", "row")]),
                    (".flex-row-reverse", &[("flex-direction", "row-reverse")]),
                    (".flex-col", &[("flex-direction", "column")]),
                    (".flex-col-reverse", &[("flex-direction", "column-reverse")]),
                ]),
            )
        }),
        ("flexWrap", |api| {
            add_utils(
                api,
                statics(&[
                    (".flex-wrap", &[("flex-wrap", "wrap")]),
                    (".flex-wrap-reverse", &[("flex-wrap", "wrap-reverse")]),
                    (".flex-nowrap", &[("flex-wrap", "nowrap")]),
                ]),
            )
        }),
        ("placeContent", |api| {
            add_utils(
                api,
                statics(&[
                    (".place-content-center", &[("place-content", "center")]),
                    (".place-content-start", &[("place-content", "start")]),
                    (".place-content-end", &[("place-content", "end")]),
                    (".place-content-between", &[("place-content", "space-between")]),
                    (".place-content-around", &[("place-content", "space-around")]),
                    (".place-content-evenly", &[("place-content", "space-evenly")]),
                    (".place-content-baseline", &[("place-content", "baseline")]),
                    (".place-content-stretch", &[("place-content", "stretch")]),
                ]),
            )
        }),
        ("placeItems", |api| {
            add_utils(
                api,
                statics(&[
                    (".place-items-start", &[("place-items", "start")]),
                    (".place-items-end", &[("place-items", "end")]),
                    (".place-items-center", &[("place-items", "center")]),
                    (".place-items-baseline", &[("place-items", "baseline")]),
                    (".place-items-stretch", &[("place-items", "stretch")]),
                ]),
            )
        }),
        ("alignContent", |api| {
            add_utils(
                api,
                statics(&[
                    (".content-normal", &[("align-content", "normal")]),
                    (".content-center", &[("align-content", "center")]),
                    (".content-start", &[("align-content", "flex-start")]),
                    (".content-end", &[("align-content", "flex-end")]),
                    (".content-between", &[("align-content", "space-between")]),
                    (".content-around", &[("align-content", "space-around")]),
                    (".content-evenly", &[("align-content", "space-evenly")]),
                    (".content-baseline", &[("align-content", "baseline")]),
                    (".content-stretch", &[("align-content", "stretch")]),
                ]),
            )
        }),
        ("alignItems", |api| {
            add_utils(
                api,
                statics(&[
                    (".items-start", &[("align-items", "flex-start")]),
                    (".items-end", &[("align-items", "flex-end")]),
                    (".items-center", &[("align-items", "center")]),
                    (".items-baseline", &[("align-items", "baseline")]),
                    (".items-stretch", &[("align-items", "stretch")]),
                ]),
            )
        }),
        ("justifyContent", |api| {
            add_utils(
                api,
                statics(&[
                    (".justify-normal", &[("justify-content", "normal")]),
                    (".justify-start", &[("justify-content", "flex-start")]),
                    (".justify-end", &[("justify-content", "flex-end")]),
                    (".justify-center", &[("justify-content", "center")]),
                    (".justify-between", &[("justify-content", "space-between")]),
                    (".justify-around", &[("justify-content", "space-around")]),
                    (".justify-evenly", &[("justify-content", "space-evenly")]),
                    (".justify-stretch", &[("justify-content", "stretch")]),
                ]),
            )
        }),
        ("justifyItems", |api| {
            add_utils(
                api,
                statics(&[
                    (".justify-items-start", &[("justify-items", "start")]),
                    (".justify-items-end", &[("justify-items", "end")]),
                    (".justify-items-center", &[("justify-items", "center")]),
                    (".justify-items-stretch", &[("justify-items", "stretch")]),
                ]),
            )
        }),
        ("gap", |api| cup("gap", vec![vec![("gap", vec![v("gap")])], vec![("gap-x", vec![v("columnGap")]), ("gap-y", vec![v("rowGap")])]]).run(api)),
        ("space", space),
        ("divideWidth", divide_width),
        ("divideStyle", |api| {
            add_utils(
                api,
                statics(&[
                    (".divide-solid > :not([hidden]) ~ :not([hidden])", &[("border-style", "solid")]),
                    (".divide-dashed > :not([hidden]) ~ :not([hidden])", &[("border-style", "dashed")]),
                    (".divide-dotted > :not([hidden]) ~ :not([hidden])", &[("border-style", "dotted")]),
                    (".divide-double > :not([hidden]) ~ :not([hidden])", &[("border-style", "double")]),
                    (".divide-none > :not([hidden]) ~ :not([hidden])", &[("border-style", "none")]),
                ]),
            )
        }),
        ("divideColor", |api| {
            let enabled = api.core_plugins("divideOpacity");
            let values = color_values_no_default(api, "divideColor");
            api.match_utilities(
                vec![(
                    "divide",
                    rf(move |value, _| {
                        let inner = if !enabled {
                            obj(vec![("border-color", to_color_value(value))])
                        } else {
                            with_alpha(value, &["border-color"], "--tw-divide-opacity")
                        };
                        obj(vec![("& > :not([hidden]) ~ :not([hidden])", inner)])
                    }),
                )],
                opts(values, &["color", "any"]),
            );
            Ok(())
        }),
        ("divideOpacity", |api| {
            let values = api.theme("divideOpacity");
            api.match_utilities(
                vec![("divide-opacity", rf(|value, _| obj(vec![("& > :not([hidden]) ~ :not([hidden])", obj(vec![("--tw-divide-opacity", value.clone())]))])))],
                values_only(values),
            );
            Ok(())
        }),
        ("placeSelf", |api| {
            add_utils(
                api,
                statics(&[
                    (".place-self-auto", &[("place-self", "auto")]),
                    (".place-self-start", &[("place-self", "start")]),
                    (".place-self-end", &[("place-self", "end")]),
                    (".place-self-center", &[("place-self", "center")]),
                    (".place-self-stretch", &[("place-self", "stretch")]),
                ]),
            )
        }),
        ("alignSelf", |api| {
            add_utils(
                api,
                statics(&[
                    (".self-auto", &[("align-self", "auto")]),
                    (".self-start", &[("align-self", "flex-start")]),
                    (".self-end", &[("align-self", "flex-end")]),
                    (".self-center", &[("align-self", "center")]),
                    (".self-stretch", &[("align-self", "stretch")]),
                    (".self-baseline", &[("align-self", "baseline")]),
                ]),
            )
        }),
        ("justifySelf", |api| {
            add_utils(
                api,
                statics(&[
                    (".justify-self-auto", &[("justify-self", "auto")]),
                    (".justify-self-start", &[("justify-self", "start")]),
                    (".justify-self-end", &[("justify-self", "end")]),
                    (".justify-self-center", &[("justify-self", "center")]),
                    (".justify-self-stretch", &[("justify-self", "stretch")]),
                ]),
            )
        }),
        ("overflow", |api| {
            add_utils(
                api,
                statics(&[
                    (".overflow-auto", &[("overflow", "auto")]),
                    (".overflow-hidden", &[("overflow", "hidden")]),
                    (".overflow-clip", &[("overflow", "clip")]),
                    (".overflow-visible", &[("overflow", "visible")]),
                    (".overflow-scroll", &[("overflow", "scroll")]),
                    (".overflow-x-auto", &[("overflow-x", "auto")]),
                    (".overflow-y-auto", &[("overflow-y", "auto")]),
                    (".overflow-x-hidden", &[("overflow-x", "hidden")]),
                    (".overflow-y-hidden", &[("overflow-y", "hidden")]),
                    (".overflow-x-clip", &[("overflow-x", "clip")]),
                    (".overflow-y-clip", &[("overflow-y", "clip")]),
                    (".overflow-x-visible", &[("overflow-x", "visible")]),
                    (".overflow-y-visible", &[("overflow-y", "visible")]),
                    (".overflow-x-scroll", &[("overflow-x", "scroll")]),
                    (".overflow-y-scroll", &[("overflow-y", "scroll")]),
                ]),
            )
        }),
        ("overscrollBehavior", |api| {
            add_utils(
                api,
                statics(&[
                    (".overscroll-auto", &[("overscroll-behavior", "auto")]),
                    (".overscroll-contain", &[("overscroll-behavior", "contain")]),
                    (".overscroll-none", &[("overscroll-behavior", "none")]),
                    (".overscroll-y-auto", &[("overscroll-behavior-y", "auto")]),
                    (".overscroll-y-contain", &[("overscroll-behavior-y", "contain")]),
                    (".overscroll-y-none", &[("overscroll-behavior-y", "none")]),
                    (".overscroll-x-auto", &[("overscroll-behavior-x", "auto")]),
                    (".overscroll-x-contain", &[("overscroll-behavior-x", "contain")]),
                    (".overscroll-x-none", &[("overscroll-behavior-x", "none")]),
                ]),
            )
        }),
        ("scrollBehavior", |api| add_utils(api, statics(&[(".scroll-auto", &[("scroll-behavior", "auto")]), (".scroll-smooth", &[("scroll-behavior", "smooth")])]))),
        ("textOverflow", |api| {
            add_utils(
                api,
                statics(&[
                    (".truncate", &[("overflow", "hidden"), ("text-overflow", "ellipsis"), ("white-space", "nowrap")]),
                    (".overflow-ellipsis", &[("text-overflow", "ellipsis")]),
                    (".text-ellipsis", &[("text-overflow", "ellipsis")]),
                    (".text-clip", &[("text-overflow", "clip")]),
                ]),
            )
        }),
        ("hyphens", |api| {
            add_utils(
                api,
                statics(&[(".hyphens-none", &[("hyphens", "none")]), (".hyphens-manual", &[("hyphens", "manual")]), (".hyphens-auto", &[("hyphens", "auto")])]),
            )
        }),
        ("whitespace", |api| {
            add_utils(
                api,
                statics(&[
                    (".whitespace-normal", &[("white-space", "normal")]),
                    (".whitespace-nowrap", &[("white-space", "nowrap")]),
                    (".whitespace-pre", &[("white-space", "pre")]),
                    (".whitespace-pre-line", &[("white-space", "pre-line")]),
                    (".whitespace-pre-wrap", &[("white-space", "pre-wrap")]),
                    (".whitespace-break-spaces", &[("white-space", "break-spaces")]),
                ]),
            )
        }),
        ("textWrap", |api| {
            add_utils(
                api,
                statics(&[
                    (".text-wrap", &[("text-wrap", "wrap")]),
                    (".text-nowrap", &[("text-wrap", "nowrap")]),
                    (".text-balance", &[("text-wrap", "balance")]),
                    (".text-pretty", &[("text-wrap", "pretty")]),
                ]),
            )
        }),
        ("wordBreak", |api| {
            add_utils(
                api,
                statics(&[
                    (".break-normal", &[("overflow-wrap", "normal"), ("word-break", "normal")]),
                    (".break-words", &[("overflow-wrap", "break-word")]),
                    (".break-all", &[("word-break", "break-all")]),
                    (".break-keep", &[("word-break", "keep-all")]),
                ]),
            )
        }),
        ("borderRadius", |api| {
            cup(
                "borderRadius",
                vec![
                    vec![("rounded", vec![v("border-radius")])],
                    vec![
                        ("rounded-s", vec![v("border-start-start-radius"), v("border-end-start-radius")]),
                        ("rounded-e", vec![v("border-start-end-radius"), v("border-end-end-radius")]),
                        ("rounded-t", vec![v("border-top-left-radius"), v("border-top-right-radius")]),
                        ("rounded-r", vec![v("border-top-right-radius"), v("border-bottom-right-radius")]),
                        ("rounded-b", vec![v("border-bottom-right-radius"), v("border-bottom-left-radius")]),
                        ("rounded-l", vec![v("border-top-left-radius"), v("border-bottom-left-radius")]),
                    ],
                    vec![
                        ("rounded-ss", vec![v("border-start-start-radius")]),
                        ("rounded-se", vec![v("border-start-end-radius")]),
                        ("rounded-ee", vec![v("border-end-end-radius")]),
                        ("rounded-es", vec![v("border-end-start-radius")]),
                        ("rounded-tl", vec![v("border-top-left-radius")]),
                        ("rounded-tr", vec![v("border-top-right-radius")]),
                        ("rounded-br", vec![v("border-bottom-right-radius")]),
                        ("rounded-bl", vec![v("border-bottom-left-radius")]),
                    ],
                ],
            )
            .run(api)
        }),
        ("borderWidth", |api| {
            let d = || defaults_p("@defaults border-width");
            cup(
                "borderWidth",
                vec![
                    vec![("border", vec![d(), v("border-width")])],
                    vec![
                        ("border-x", vec![d(), v("border-left-width"), v("border-right-width")]),
                        ("border-y", vec![d(), v("border-top-width"), v("border-bottom-width")]),
                    ],
                    vec![
                        ("border-s", vec![d(), v("border-inline-start-width")]),
                        ("border-e", vec![d(), v("border-inline-end-width")]),
                        ("border-t", vec![d(), v("border-top-width")]),
                        ("border-r", vec![d(), v("border-right-width")]),
                        ("border-b", vec![d(), v("border-bottom-width")]),
                        ("border-l", vec![d(), v("border-left-width")]),
                    ],
                ],
            )
            .ty(&["line-width", "length"])
            .run(api)
        }),
        ("borderStyle", |api| {
            add_utils(
                api,
                statics(&[
                    (".border-solid", &[("border-style", "solid")]),
                    (".border-dashed", &[("border-style", "dashed")]),
                    (".border-dotted", &[("border-style", "dotted")]),
                    (".border-double", &[("border-style", "double")]),
                    (".border-hidden", &[("border-style", "hidden")]),
                    (".border-none", &[("border-style", "none")]),
                ]),
            )
        }),
        ("borderColor", border_color),
        ("borderOpacity", |api| cup("borderOpacity", single("border-opacity", vec![v("--tw-border-opacity")])).run(api)),
        ("backgroundColor", |api| {
            let enabled = api.core_plugins("backgroundOpacity");
            let values = color_values(api, "backgroundColor");
            api.match_utilities(vec![("bg", alpha_color_rule(enabled, &["background-color"], "--tw-bg-opacity"))], opts(values, &["color", "any"]));
            Ok(())
        }),
        ("backgroundOpacity", |api| cup("backgroundOpacity", single("bg-opacity", vec![v("--tw-bg-opacity")])).run(api)),
        ("backgroundImage", |api| cup("backgroundImage", single("bg", vec![v("background-image")])).ty(&["lookup", "image", "url"]).run(api)),
        ("gradientColorStops", gradient_color_stops),
        ("boxDecorationBreak", |api| {
            add_utils(
                api,
                statics(&[
                    (".decoration-slice", &[("box-decoration-break", "slice")]),
                    (".decoration-clone", &[("box-decoration-break", "clone")]),
                    (".box-decoration-slice", &[("box-decoration-break", "slice")]),
                    (".box-decoration-clone", &[("box-decoration-break", "clone")]),
                ]),
            )
        }),
        ("backgroundSize", |api| {
            cup("backgroundSize", single("bg", vec![v("background-size")])).ty(&["lookup", "length", "percentage", "size"]).run(api)
        }),
        ("backgroundAttachment", |api| {
            add_utils(
                api,
                statics(&[
                    (".bg-fixed", &[("background-attachment", "fixed")]),
                    (".bg-local", &[("background-attachment", "local")]),
                    (".bg-scroll", &[("background-attachment", "scroll")]),
                ]),
            )
        }),
        ("backgroundClip", |api| {
            add_utils(
                api,
                statics(&[
                    (".bg-clip-border", &[("background-clip", "border-box")]),
                    (".bg-clip-padding", &[("background-clip", "padding-box")]),
                    (".bg-clip-content", &[("background-clip", "content-box")]),
                    (".bg-clip-text", &[("background-clip", "text")]),
                ]),
            )
        }),
        ("backgroundPosition", |api| {
            cup("backgroundPosition", single("bg", vec![v("background-position")]))
                .types(vec![
                    TypeSpec { ty: "lookup".into(), prefer_on_conflict: false },
                    TypeSpec { ty: "position".into(), prefer_on_conflict: true },
                ])
                .run(api)
        }),
        ("backgroundRepeat", |api| {
            add_utils(
                api,
                statics(&[
                    (".bg-repeat", &[("background-repeat", "repeat")]),
                    (".bg-no-repeat", &[("background-repeat", "no-repeat")]),
                    (".bg-repeat-x", &[("background-repeat", "repeat-x")]),
                    (".bg-repeat-y", &[("background-repeat", "repeat-y")]),
                    (".bg-repeat-round", &[("background-repeat", "round")]),
                    (".bg-repeat-space", &[("background-repeat", "space")]),
                ]),
            )
        }),
        ("backgroundOrigin", |api| {
            add_utils(
                api,
                statics(&[
                    (".bg-origin-border", &[("background-origin", "border-box")]),
                    (".bg-origin-padding", &[("background-origin", "padding-box")]),
                    (".bg-origin-content", &[("background-origin", "content-box")]),
                ]),
            )
        }),
        ("fill", |api| {
            simple_color(api, "fill", "fill", "fill", &["color", "any"]);
            Ok(())
        }),
        ("stroke", |api| {
            simple_color(api, "stroke", "stroke", "stroke", &["color", "url", "any"]);
            Ok(())
        }),
        ("strokeWidth", |api| cup("strokeWidth", single("stroke", vec![v("stroke-width")])).ty(&["length", "number", "percentage"]).run(api)),
        ("objectFit", |api| {
            add_utils(
                api,
                statics(&[
                    (".object-contain", &[("object-fit", "contain")]),
                    (".object-cover", &[("object-fit", "cover")]),
                    (".object-fill", &[("object-fit", "fill")]),
                    (".object-none", &[("object-fit", "none")]),
                    (".object-scale-down", &[("object-fit", "scale-down")]),
                ]),
            )
        }),
        ("objectPosition", |api| cup("objectPosition", single("object", vec![v("object-position")])).run(api)),
        ("padding", |api| {
            cup(
                "padding",
                vec![
                    vec![("p", vec![v("padding")])],
                    vec![("px", vec![v("padding-left"), v("padding-right")]), ("py", vec![v("padding-top"), v("padding-bottom")])],
                    vec![
                        ("ps", vec![v("padding-inline-start")]),
                        ("pe", vec![v("padding-inline-end")]),
                        ("pt", vec![v("padding-top")]),
                        ("pr", vec![v("padding-right")]),
                        ("pb", vec![v("padding-bottom")]),
                        ("pl", vec![v("padding-left")]),
                    ],
                ],
            )
            .run(api)
        }),
        ("textAlign", |api| {
            add_utils(
                api,
                statics(&[
                    (".text-left", &[("text-align", "left")]),
                    (".text-center", &[("text-align", "center")]),
                    (".text-right", &[("text-align", "right")]),
                    (".text-justify", &[("text-align", "justify")]),
                    (".text-start", &[("text-align", "start")]),
                    (".text-end", &[("text-align", "end")]),
                ]),
            )
        }),
        ("textIndent", |api| cup("textIndent", single("indent", vec![v("text-indent")])).neg().run(api)),
        ("verticalAlign", |api| {
            add_utils(
                api,
                statics(&[
                    (".align-baseline", &[("vertical-align", "baseline")]),
                    (".align-top", &[("vertical-align", "top")]),
                    (".align-middle", &[("vertical-align", "middle")]),
                    (".align-bottom", &[("vertical-align", "bottom")]),
                    (".align-text-top", &[("vertical-align", "text-top")]),
                    (".align-text-bottom", &[("vertical-align", "text-bottom")]),
                    (".align-sub", &[("vertical-align", "sub")]),
                    (".align-super", &[("vertical-align", "super")]),
                ]),
            )?;
            api.match_utilities(vec![("align", rf(|value, _| obj(vec![("vertical-align", value.clone())])))], MatchOptions::default());
            Ok(())
        }),
        ("fontFamily", font_family),
        ("fontSize", font_size),
        ("fontWeight", |api| cup("fontWeight", single("font", vec![v("fontWeight")])).ty(&["lookup", "number", "any"]).run(api)),
        ("textTransform", |api| {
            add_utils(
                api,
                statics(&[
                    (".uppercase", &[("text-transform", "uppercase")]),
                    (".lowercase", &[("text-transform", "lowercase")]),
                    (".capitalize", &[("text-transform", "capitalize")]),
                    (".normal-case", &[("text-transform", "none")]),
                ]),
            )
        }),
        ("fontStyle", |api| add_utils(api, statics(&[(".italic", &[("font-style", "italic")]), (".not-italic", &[("font-style", "normal")])]))),
        ("fontVariantNumeric", font_variant_numeric),
        ("lineHeight", |api| cup("lineHeight", single("leading", vec![v("lineHeight")])).run(api)),
        ("letterSpacing", |api| cup("letterSpacing", single("tracking", vec![v("letterSpacing")])).neg().run(api)),
        ("textColor", |api| {
            let enabled = api.core_plugins("textOpacity");
            let values = color_values(api, "textColor");
            api.match_utilities(vec![("text", alpha_color_rule(enabled, &["color"], "--tw-text-opacity"))], opts(values, &["color", "any"]));
            Ok(())
        }),
        ("textOpacity", |api| cup("textOpacity", single("text-opacity", vec![v("--tw-text-opacity")])).run(api)),
        ("textDecoration", |api| {
            add_utils(
                api,
                statics(&[
                    (".underline", &[("text-decoration-line", "underline")]),
                    (".overline", &[("text-decoration-line", "overline")]),
                    (".line-through", &[("text-decoration-line", "line-through")]),
                    (".no-underline", &[("text-decoration-line", "none")]),
                ]),
            )
        }),
        ("textDecorationColor", |api| {
            simple_color(api, "decoration", "textDecorationColor", "text-decoration-color", &["color", "any"]);
            Ok(())
        }),
        ("textDecorationStyle", |api| {
            add_utils(
                api,
                statics(&[
                    (".decoration-solid", &[("text-decoration-style", "solid")]),
                    (".decoration-double", &[("text-decoration-style", "double")]),
                    (".decoration-dotted", &[("text-decoration-style", "dotted")]),
                    (".decoration-dashed", &[("text-decoration-style", "dashed")]),
                    (".decoration-wavy", &[("text-decoration-style", "wavy")]),
                ]),
            )
        }),
        ("textDecorationThickness", |api| {
            cup("textDecorationThickness", single("decoration", vec![v("text-decoration-thickness")])).ty(&["length", "percentage"]).run(api)
        }),
        ("textUnderlineOffset", |api| {
            cup("textUnderlineOffset", single("underline-offset", vec![v("text-underline-offset")])).ty(&["length", "percentage", "any"]).run(api)
        }),
        ("fontSmoothing", |api| {
            add_utils(
                api,
                statics(&[
                    (".antialiased", &[("-webkit-font-smoothing", "antialiased"), ("-moz-osx-font-smoothing", "grayscale")]),
                    (".subpixel-antialiased", &[("-webkit-font-smoothing", "auto"), ("-moz-osx-font-smoothing", "auto")]),
                ]),
            )
        }),
        ("placeholderColor", |api| {
            let enabled = api.core_plugins("placeholderOpacity");
            let values = color_values(api, "placeholderColor");
            api.match_utilities(
                vec![(
                    "placeholder",
                    rf(move |value, _| {
                        let inner = if !enabled {
                            obj(vec![("color", to_color_value(value))])
                        } else {
                            with_alpha(value, &["color"], "--tw-placeholder-opacity")
                        };
                        obj(vec![("&::placeholder", inner)])
                    }),
                )],
                opts(values, &["color", "any"]),
            );
            Ok(())
        }),
        ("placeholderOpacity", |api| {
            let values = api.theme("placeholderOpacity");
            api.match_utilities(
                vec![("placeholder-opacity", rf(|value, _| obj(vec![("&::placeholder", obj(vec![("--tw-placeholder-opacity", value.clone())]))])))],
                values_only(values),
            );
            Ok(())
        }),
        ("caretColor", |api| {
            simple_color(api, "caret", "caretColor", "caret-color", &["color", "any"]);
            Ok(())
        }),
        ("accentColor", |api| {
            simple_color(api, "accent", "accentColor", "accent-color", &["color", "any"]);
            Ok(())
        }),
        ("opacity", |api| cup("opacity", single("opacity", vec![v("opacity")])).run(api)),
        ("backgroundBlendMode", |api| {
            let modes = [
                "normal", "multiply", "screen", "overlay", "darken", "lighten", "color-dodge", "color-burn", "hard-light", "soft-light", "difference",
                "exclusion", "hue", "saturation", "color", "luminosity",
            ];
            let mut o = Object::new();
            for m in modes {
                o.insert(format!(".bg-blend-{m}"), objs(&[("background-blend-mode", m)]));
            }
            add_utils(api, o.into_value())
        }),
        ("mixBlendMode", |api| {
            let modes = [
                "normal", "multiply", "screen", "overlay", "darken", "lighten", "color-dodge", "color-burn", "hard-light", "soft-light", "difference",
                "exclusion", "hue", "saturation", "color", "luminosity", "plus-darker", "plus-lighter",
            ];
            let mut o = Object::new();
            for m in modes {
                o.insert(format!(".mix-blend-{m}"), objs(&[("mix-blend-mode", m)]));
            }
            add_utils(api, o.into_value())
        }),
        ("boxShadow", box_shadow),
        ("boxShadowColor", |api| {
            let values = color_values(api, "boxShadowColor");
            api.match_utilities(
                vec![("shadow", rf(|value, _| obj(vec![("--tw-shadow-color", to_color_value(value)), ("--tw-shadow", s("var(--tw-shadow-colored)"))])))],
                opts(values, &["color", "any"]),
            );
            Ok(())
        }),
        ("outlineStyle", |api| {
            add_utils(
                api,
                statics(&[
                    (".outline-none", &[("outline", "2px solid transparent"), ("outline-offset", "2px")]),
                    (".outline", &[("outline-style", "solid")]),
                    (".outline-dashed", &[("outline-style", "dashed")]),
                    (".outline-dotted", &[("outline-style", "dotted")]),
                    (".outline-double", &[("outline-style", "double")]),
                ]),
            )
        }),
        ("outlineWidth", |api| cup("outlineWidth", single("outline", vec![v("outline-width")])).ty(&["length", "number", "percentage"]).run(api)),
        ("outlineOffset", |api| {
            cup("outlineOffset", single("outline-offset", vec![v("outline-offset")])).ty(&["length", "number", "percentage", "any"]).neg().run(api)
        }),
        ("outlineColor", |api| {
            simple_color(api, "outline", "outlineColor", "outline-color", &["color", "any"]);
            Ok(())
        }),
        ("ringWidth", ring_width),
        ("ringColor", |api| {
            let enabled = api.core_plugins("ringOpacity");
            let values = color_values_no_default(api, "ringColor");
            api.match_utilities(vec![("ring", alpha_color_rule(enabled, &["--tw-ring-color"], "--tw-ring-opacity"))], opts(values, &["color", "any"]));
            Ok(())
        }),
        ("ringOpacity", |api| {
            let filter = !super::config::flag_enabled(api.config_value(), "respectDefaultRingColorOpacity");
            cup("ringOpacity", single("ring-opacity", vec![v("--tw-ring-opacity")])).filter_default(filter).run(api)
        }),
        ("ringOffsetWidth", |api| cup("ringOffsetWidth", single("ring-offset", vec![v("--tw-ring-offset-width")])).ty(&["length"]).run(api)),
        ("ringOffsetColor", |api| {
            simple_color(api, "ring-offset", "ringOffsetColor", "--tw-ring-offset-color", &["color", "any"]);
            Ok(())
        }),
        ("blur", |api| filter_fn(api, "blur", "blur", "--tw-blur", FilterKind::Blur, false)),
        ("brightness", |api| filter_fn(api, "brightness", "brightness", "--tw-brightness", FilterKind::Wrap("brightness"), false)),
        ("contrast", |api| filter_fn(api, "contrast", "contrast", "--tw-contrast", FilterKind::Wrap("contrast"), false)),
        ("dropShadow", |api| filter_fn(api, "drop-shadow", "dropShadow", "--tw-drop-shadow", FilterKind::DropShadow, false)),
        ("grayscale", |api| filter_fn(api, "grayscale", "grayscale", "--tw-grayscale", FilterKind::Wrap("grayscale"), false)),
        ("hueRotate", |api| filter_fn(api, "hue-rotate", "hueRotate", "--tw-hue-rotate", FilterKind::Wrap("hue-rotate"), false)),
        ("invert", |api| filter_fn(api, "invert", "invert", "--tw-invert", FilterKind::Wrap("invert"), false)),
        ("saturate", |api| filter_fn(api, "saturate", "saturate", "--tw-saturate", FilterKind::Wrap("saturate"), false)),
        ("sepia", |api| filter_fn(api, "sepia", "sepia", "--tw-sepia", FilterKind::Wrap("sepia"), false)),
        ("filter", |api| {
            api.add_defaults(
                "filter",
                objs(&[
                    ("--tw-blur", " "),
                    ("--tw-brightness", " "),
                    ("--tw-contrast", " "),
                    ("--tw-grayscale", " "),
                    ("--tw-hue-rotate", " "),
                    ("--tw-invert", " "),
                    ("--tw-saturate", " "),
                    ("--tw-sepia", " "),
                    ("--tw-drop-shadow", " "),
                ]),
            )?;
            add_utils(
                api,
                obj(vec![
                    (".filter", obj(vec![("@defaults filter", Object::new().into_value()), ("filter", s(CSS_FILTER_VALUE))])),
                    (".filter-none", objs(&[("filter", "none")])),
                ]),
            )
        }),
        ("backdropBlur", |api| filter_fn(api, "backdrop-blur", "backdropBlur", "--tw-backdrop-blur", FilterKind::Blur, true)),
        ("backdropBrightness", |api| filter_fn(api, "backdrop-brightness", "backdropBrightness", "--tw-backdrop-brightness", FilterKind::Wrap("brightness"), true)),
        ("backdropContrast", |api| filter_fn(api, "backdrop-contrast", "backdropContrast", "--tw-backdrop-contrast", FilterKind::Wrap("contrast"), true)),
        ("backdropGrayscale", |api| filter_fn(api, "backdrop-grayscale", "backdropGrayscale", "--tw-backdrop-grayscale", FilterKind::Wrap("grayscale"), true)),
        ("backdropHueRotate", |api| filter_fn(api, "backdrop-hue-rotate", "backdropHueRotate", "--tw-backdrop-hue-rotate", FilterKind::Wrap("hue-rotate"), true)),
        ("backdropInvert", |api| filter_fn(api, "backdrop-invert", "backdropInvert", "--tw-backdrop-invert", FilterKind::Wrap("invert"), true)),
        ("backdropOpacity", |api| filter_fn(api, "backdrop-opacity", "backdropOpacity", "--tw-backdrop-opacity", FilterKind::Wrap("opacity"), true)),
        ("backdropSaturate", |api| filter_fn(api, "backdrop-saturate", "backdropSaturate", "--tw-backdrop-saturate", FilterKind::Wrap("saturate"), true)),
        ("backdropSepia", |api| filter_fn(api, "backdrop-sepia", "backdropSepia", "--tw-backdrop-sepia", FilterKind::Wrap("sepia"), true)),
        ("backdropFilter", |api| {
            api.add_defaults(
                "backdrop-filter",
                objs(&[
                    ("--tw-backdrop-blur", " "),
                    ("--tw-backdrop-brightness", " "),
                    ("--tw-backdrop-contrast", " "),
                    ("--tw-backdrop-grayscale", " "),
                    ("--tw-backdrop-hue-rotate", " "),
                    ("--tw-backdrop-invert", " "),
                    ("--tw-backdrop-opacity", " "),
                    ("--tw-backdrop-saturate", " "),
                    ("--tw-backdrop-sepia", " "),
                ]),
            )?;
            add_utils(
                api,
                obj(vec![
                    (
                        ".backdrop-filter",
                        obj(vec![
                            ("@defaults backdrop-filter", Object::new().into_value()),
                            ("-webkit-backdrop-filter", s(CSS_BACKDROP_FILTER_VALUE)),
                            ("backdrop-filter", s(CSS_BACKDROP_FILTER_VALUE)),
                        ]),
                    ),
                    (".backdrop-filter-none", objs(&[("-webkit-backdrop-filter", "none"), ("backdrop-filter", "none")])),
                ]),
            )
        }),
        ("transitionProperty", |api| {
            let tf = api.theme("transitionTimingFunction.DEFAULT");
            let dur = api.theme("transitionDuration.DEFAULT");
            let values = api.theme("transitionProperty");
            api.match_utilities(
                vec![(
                    "transition",
                    rf(move |value, _| {
                        let mut o = Object::new();
                        o.insert("transition-property", value.clone());
                        if *value != s("none") {
                            o.insert("transition-timing-function", tf.clone());
                            o.insert("transition-duration", dur.clone());
                        }
                        o.into_value()
                    }),
                )],
                values_only(values),
            );
            Ok(())
        }),
        ("transitionDelay", |api| cup("transitionDelay", single("delay", vec![v("transitionDelay")])).run(api)),
        ("transitionDuration", |api| cup("transitionDuration", single("duration", vec![v("transitionDuration")])).filter_default(true).run(api)),
        ("transitionTimingFunction", |api| cup("transitionTimingFunction", single("ease", vec![v("transitionTimingFunction")])).filter_default(true).run(api)),
        ("willChange", |api| cup("willChange", single("will-change", vec![v("will-change")])).run(api)),
        ("contain", contain),
        ("content", |api| cup("content", single("content", vec![v("--tw-content"), P::Fixed("content", s("var(--tw-content)"))])).run(api)),
        ("forcedColorAdjust", |api| {
            add_utils(
                api,
                statics(&[(".forced-color-adjust-auto", &[("forced-color-adjust", "auto")]), (".forced-color-adjust-none", &[("forced-color-adjust", "none")])]),
            )
        }),
    ]
}

// ---- plugins with more logic

fn preflight(api: &mut PluginApi) -> Result<(), String> {
    let css = &mut api.ctx.css;
    let root = crate::postcss::parse(css, include_str!("data/preflight.css")).map_err(|e| e.message())?;
    let comment = css.comment("! tailwindcss v3.4.16 | MIT License | https://tailwindcss.com");
    let mut styles = vec![Style::Node(comment)];
    for &n in css.children(root) {
        styles.push(Style::Node(n));
    }
    api.add_base(styles)
}

fn container(api: &mut PluginApi) -> Result<(), String> {
    let screens_v = api.theme_or("container.screens", api.theme("screens"));
    let screens = normalize_screens(&screens_v)?;
    let min_widths: Vec<Value> = screens.iter().flat_map(|s| s.values.iter().map(|v| v.min.clone())).filter(|v| !v.is_undefined()).collect();
    let paddings = api.theme("container.padding");
    // mapMinWidthsToPadding
    let mut mapping: Vec<(Value, Value)> = Vec::new(); // (minWidth, padding)
    if !paddings.is_undefined() {
        if !paddings.is_object_like() {
            mapping.push((Value::Num(0.0), paddings.clone()));
        } else {
            let d = paddings.get("DEFAULT");
            if d.truthy() {
                mapping.push((Value::Num(0.0), d));
            }
            for mw in &min_widths {
                for screen in &screens {
                    for v in &screen.values {
                        if v.min == *mw {
                            mapping.push((mw.clone(), paddings.get(&screen.name)));
                        }
                    }
                }
            }
        }
    }
    let padding_for = |mw: &Value| -> Vec<(&'static str, Value)> {
        match mapping.iter().find(|(m, _)| m == mw) {
            None => vec![],
            Some((_, p)) => vec![("paddingRight", p.clone()), ("paddingLeft", p.clone())],
        }
    };
    let mut sorted = min_widths.clone();
    crate::js::sort_by(&mut sorted, |a, z| {
        let d = js::parse_int(&a.to_js_string()) - js::parse_int(&z.to_js_string());
        if d.is_nan() || d == 0.0 { 0 } else if d < 0.0 { -1 } else { 1 }
    });
    let mut unique: Vec<Value> = Vec::new();
    for v in sorted {
        if !unique.iter().any(|u| *u == v) {
            unique.push(v);
        }
    }
    let mut container = Object::new();
    container.insert("width", s("100%"));
    if api.theme_or("container.center", Value::Bool(false)).truthy() {
        container.insert("marginRight", s("auto"));
        container.insert("marginLeft", s("auto"));
    }
    for (k, v) in padding_for(&Value::Num(0.0)) {
        container.insert(k, v);
    }
    let mut styles = vec![Style::Obj(obj(vec![(".container", container.into_value())]))];
    for mw in unique {
        let mut inner = Object::new();
        inner.insert("max-width", mw.clone());
        for (k, v) in padding_for(&mw) {
            inner.insert(k, v);
        }
        let media = format!("@media (min-width: {})", mw.to_js_string());
        styles.push(Style::Obj(obj(vec![(&media, obj(vec![(".container", inner.into_value())]))])));
    }
    let _ = &screens;
    api.add_components(styles, false, true)
}

fn accessibility(api: &mut PluginApi) -> Result<(), String> {
    add_utils(
        api,
        statics(&[
            (
                ".sr-only",
                &[
                    ("position", "absolute"),
                    ("width", "1px"),
                    ("height", "1px"),
                    ("padding", "0"),
                    ("margin", "-1px"),
                    ("overflow", "hidden"),
                    ("clip", "rect(0, 0, 0, 0)"),
                    ("whiteSpace", "nowrap"),
                    ("borderWidth", "0"),
                ],
            ),
            (
                ".not-sr-only",
                &[
                    ("position", "static"),
                    ("width", "auto"),
                    ("height", "auto"),
                    ("padding", "0"),
                    ("margin", "0"),
                    ("overflow", "visible"),
                    ("clip", "auto"),
                    ("whiteSpace", "normal"),
                ],
            ),
        ]),
    )
}

fn border_spacing(api: &mut PluginApi) -> Result<(), String> {
    api.add_defaults("border-spacing", obj(vec![("--tw-border-spacing-x", Value::Num(0.0)), ("--tw-border-spacing-y", Value::Num(0.0))]))?;
    let values = api.theme("borderSpacing");
    let d = || ("@defaults border-spacing", Object::new().into_value());
    let bs = || ("border-spacing", s("var(--tw-border-spacing-x) var(--tw-border-spacing-y)"));
    api.match_utilities(
        vec![
            ("border-spacing", rf(move |v, _| obj(vec![("--tw-border-spacing-x", v.clone()), ("--tw-border-spacing-y", v.clone()), d(), bs()]))),
            ("border-spacing-x", rf(move |v, _| obj(vec![("--tw-border-spacing-x", v.clone()), d(), bs()]))),
            ("border-spacing-y", rf(move |v, _| obj(vec![("--tw-border-spacing-y", v.clone()), d(), bs()]))),
        ],
        values_only(values),
    );
    Ok(())
}

fn animation(api: &mut PluginApi) -> Result<(), String> {
    let prefix = api.config("prefix").to_js_string();
    let prefix_name = move |name: &str| escape_class_name(&format!("{prefix}{name}"));
    let mut keyframes = Object::new();
    let kf = api.theme("keyframes");
    if let Value::Obj(o) = &kf {
        for (k, v) in o.iter() {
            let mut inner = Object::new();
            inner.insert(format!("@keyframes {}", prefix_name(k)), v.clone());
            keyframes.insert(k.clone(), inner.into_value());
        }
    }
    let values = api.theme("animation");
    let keyframes = Arc::new(keyframes);
    api.match_utilities(
        vec![(
            "animate",
            rf(move |value, _| {
                let animations = parse_animation_value(&value.to_js_string());
                let mut out: Vec<Value> = Vec::new();
                for a in &animations {
                    match &a.name {
                        Some(n) => out.push(keyframes.get(n).cloned().unwrap_or(Value::Undefined)),
                        None => out.push(Value::Undefined),
                    }
                }
                let anim = animations
                    .iter()
                    .map(|a| match &a.name {
                        Some(n) if keyframes.get(n).is_some() => a.value.replacen(n.as_str(), &prefix_name(n), 1),
                        _ => a.value.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push(obj(vec![("animation", s(&anim))]));
                Value::from(out)
            }),
        )],
        values_only(values),
    );
    Ok(())
}

fn touch_action(api: &mut PluginApi) -> Result<(), String> {
    api.add_defaults("touch-action", objs(&[("--tw-pan-x", " "), ("--tw-pan-y", " "), ("--tw-pinch-zoom", " ")]))?;
    let v = "var(--tw-pan-x) var(--tw-pan-y) var(--tw-pinch-zoom)";
    let d = || ("@defaults touch-action", Object::new().into_value());
    let pan = |var: &str, val: &str| obj(vec![d(), (var, s(val)), ("touch-action", s(v))]);
    // keys are inserted in order; `var` borrows are fine because obj copies them
    let mut o = Object::new();
    o.insert(".touch-auto", objs(&[("touch-action", "auto")]));
    o.insert(".touch-none", objs(&[("touch-action", "none")]));
    o.insert(".touch-pan-x", pan("--tw-pan-x", "pan-x"));
    o.insert(".touch-pan-left", pan("--tw-pan-x", "pan-left"));
    o.insert(".touch-pan-right", pan("--tw-pan-x", "pan-right"));
    o.insert(".touch-pan-y", pan("--tw-pan-y", "pan-y"));
    o.insert(".touch-pan-up", pan("--tw-pan-y", "pan-up"));
    o.insert(".touch-pan-down", pan("--tw-pan-y", "pan-down"));
    o.insert(".touch-pinch-zoom", pan("--tw-pinch-zoom", "pinch-zoom"));
    o.insert(".touch-manipulation", objs(&[("touch-action", "manipulation")]));
    add_utils(api, o.into_value())
}

fn space(api: &mut PluginApi) -> Result<(), String> {
    let values = api.theme("space");
    let zero = |v: &Value| if *v == s("0") { "0px".to_string() } else { v.to_js_string() };
    api.match_utilities(
        vec![
            (
                "space-x",
                rf(move |v, _| {
                    let v = zero(v);
                    obj(vec![(
                        "& > :not([hidden]) ~ :not([hidden])",
                        obj(vec![
                            ("--tw-space-x-reverse", s("0")),
                            ("margin-right", s(&format!("calc({v} * var(--tw-space-x-reverse))"))),
                            ("margin-left", s(&format!("calc({v} * calc(1 - var(--tw-space-x-reverse)))"))),
                        ]),
                    )])
                }),
            ),
            (
                "space-y",
                rf(move |v, _| {
                    let v = zero(v);
                    obj(vec![(
                        "& > :not([hidden]) ~ :not([hidden])",
                        obj(vec![
                            ("--tw-space-y-reverse", s("0")),
                            ("margin-top", s(&format!("calc({v} * calc(1 - var(--tw-space-y-reverse)))"))),
                            ("margin-bottom", s(&format!("calc({v} * var(--tw-space-y-reverse))"))),
                        ]),
                    )])
                }),
            ),
        ],
        MatchOptions { values, supports_negative_values: true, ..Default::default() },
    );
    add_utils(
        api,
        obj(vec![
            (".space-y-reverse > :not([hidden]) ~ :not([hidden])", objs(&[("--tw-space-y-reverse", "1")])),
            (".space-x-reverse > :not([hidden]) ~ :not([hidden])", objs(&[("--tw-space-x-reverse", "1")])),
        ]),
    )
}

fn divide_width(api: &mut PluginApi) -> Result<(), String> {
    let values = api.theme("divideWidth");
    let zero = |v: &Value| if *v == s("0") { "0px".to_string() } else { v.to_js_string() };
    let d = || ("@defaults border-width", Object::new().into_value());
    api.match_utilities(
        vec![
            (
                "divide-x",
                rf(move |v, _| {
                    let v = zero(v);
                    obj(vec![(
                        "& > :not([hidden]) ~ :not([hidden])",
                        obj(vec![
                            d(),
                            ("--tw-divide-x-reverse", s("0")),
                            ("border-right-width", s(&format!("calc({v} * var(--tw-divide-x-reverse))"))),
                            ("border-left-width", s(&format!("calc({v} * calc(1 - var(--tw-divide-x-reverse)))"))),
                        ]),
                    )])
                }),
            ),
            (
                "divide-y",
                rf(move |v, _| {
                    let v = zero(v);
                    obj(vec![(
                        "& > :not([hidden]) ~ :not([hidden])",
                        obj(vec![
                            d(),
                            ("--tw-divide-y-reverse", s("0")),
                            ("border-top-width", s(&format!("calc({v} * calc(1 - var(--tw-divide-y-reverse)))"))),
                            ("border-bottom-width", s(&format!("calc({v} * var(--tw-divide-y-reverse))"))),
                        ]),
                    )])
                }),
            ),
        ],
        opts(values, &["line-width", "length", "any"]),
    );
    add_utils(
        api,
        obj(vec![
            (".divide-y-reverse > :not([hidden]) ~ :not([hidden])", obj(vec![d(), ("--tw-divide-y-reverse", s("1"))])),
            (".divide-x-reverse > :not([hidden]) ~ :not([hidden])", obj(vec![d(), ("--tw-divide-x-reverse", s("1"))])),
        ]),
    )
}

fn border_color(api: &mut PluginApi) -> Result<(), String> {
    let enabled = api.core_plugins("borderOpacity");
    let values = color_values_no_default(api, "borderColor");
    let var = "--tw-border-opacity";
    api.match_utilities(vec![("border", alpha_color_rule(enabled, &["border-color"], var))], opts(values.clone(), &["color", "any"]));
    api.match_utilities(
        vec![
            ("border-x", alpha_color_rule(enabled, &["border-left-color", "border-right-color"], var)),
            ("border-y", alpha_color_rule(enabled, &["border-top-color", "border-bottom-color"], var)),
        ],
        opts(values.clone(), &["color", "any"]),
    );
    api.match_utilities(
        vec![
            ("border-s", alpha_color_rule(enabled, &["border-inline-start-color"], var)),
            ("border-e", alpha_color_rule(enabled, &["border-inline-end-color"], var)),
            ("border-t", alpha_color_rule(enabled, &["border-top-color"], var)),
            ("border-r", alpha_color_rule(enabled, &["border-right-color"], var)),
            ("border-b", alpha_color_rule(enabled, &["border-bottom-color"], var)),
            ("border-l", alpha_color_rule(enabled, &["border-left-color"], var)),
        ],
        opts(values, &["color", "any"]),
    );
    Ok(())
}

fn gradient_color_stops(api: &mut PluginApi) -> Result<(), String> {
    api.add_defaults(
        "gradient-color-stops",
        objs(&[("--tw-gradient-from-position", " "), ("--tw-gradient-via-position", " "), ("--tw-gradient-to-position", " ")]),
    )?;
    let transparent_to = |v: &Value| with_alpha_value(v, &Value::Num(0.0), &s("rgb(255 255 255 / 0)")).to_js_string();
    let values = color_values(api, "gradientColorStops");
    let positions = api.theme("gradientColorStopPositions");
    let d = || ("@defaults gradient-color-stops", Object::new().into_value());
    let color_opts = || opts(values.clone(), &["color", "any"]);
    let pos_opts = || opts(positions.clone(), &["length", "percentage"]);
    api.match_utilities(
        vec![(
            "from",
            rf(move |v, _| {
                let t = transparent_to(v);
                obj(vec![
                    d(),
                    ("--tw-gradient-from", s(&format!("{} var(--tw-gradient-from-position)", to_color_value(v).to_js_string()))),
                    ("--tw-gradient-to", s(&format!("{t} var(--tw-gradient-to-position)"))),
                    ("--tw-gradient-stops", s("var(--tw-gradient-from), var(--tw-gradient-to)")),
                ])
            }),
        )],
        color_opts(),
    );
    api.match_utilities(vec![("from", rf(|v, _| obj(vec![("--tw-gradient-from-position", v.clone())])))], pos_opts());
    api.match_utilities(
        vec![(
            "via",
            rf(move |v, _| {
                let t = transparent_to(v);
                obj(vec![
                    d(),
                    ("--tw-gradient-to", s(&format!("{t}  var(--tw-gradient-to-position)"))),
                    (
                        "--tw-gradient-stops",
                        s(&format!("var(--tw-gradient-from), {} var(--tw-gradient-via-position), var(--tw-gradient-to)", to_color_value(v).to_js_string())),
                    ),
                ])
            }),
        )],
        color_opts(),
    );
    api.match_utilities(vec![("via", rf(|v, _| obj(vec![("--tw-gradient-via-position", v.clone())])))], pos_opts());
    api.match_utilities(
        vec![(
            "to",
            rf(move |v, _| obj(vec![d(), ("--tw-gradient-to", s(&format!("{} var(--tw-gradient-to-position)", to_color_value(v).to_js_string())))])),
        )],
        color_opts(),
    );
    api.match_utilities(vec![("to", rf(|v, _| obj(vec![("--tw-gradient-to-position", v.clone())])))], pos_opts());
    Ok(())
}

fn font_family(api: &mut PluginApi) -> Result<(), String> {
    let values = api.theme("fontFamily");
    api.match_utilities(
        vec![(
            "font",
            rf(|value, _| {
                let (families, options) = match value {
                    Value::Arr(a) if a.get(1).is_some_and(|x| x.is_object()) => (a[0].clone(), a[1].clone()),
                    v => (v.clone(), Object::new().into_value()),
                };
                let ff = match &families {
                    Value::Arr(a) => s(&a.iter().map(|x| if x.is_nullish() { String::new() } else { x.to_js_string() }).collect::<Vec<_>>().join(", ")),
                    v => v.clone(),
                };
                let mut o = Object::new();
                o.insert("font-family", ff);
                let ffs = options.get("fontFeatureSettings");
                if !ffs.is_undefined() {
                    o.insert("font-feature-settings", ffs);
                }
                let fvs = options.get("fontVariationSettings");
                if !fvs.is_undefined() {
                    o.insert("font-variation-settings", fvs);
                }
                o.into_value()
            }),
        )],
        opts(values, &["lookup", "generic-name", "family-name"]),
    );
    Ok(())
}

fn font_size(api: &mut PluginApi) -> Result<(), String> {
    let values = api.theme("fontSize");
    let modifiers = api.theme("lineHeight");
    api.match_utilities(
        vec![(
            "text",
            rf(|value, modifier| {
                let (font_size, options) = match value {
                    Value::Arr(a) => (a.first().cloned().unwrap_or(Value::Undefined), a.get(1).cloned().unwrap_or(Value::Undefined)),
                    v => (v.clone(), Value::Undefined),
                };
                if let Some(m) = modifier.as_ref().filter(|m| !m.is_empty()) {
                    return obj(vec![("font-size", font_size), ("line-height", s(m))]);
                }
                let (lh, ls, fw) = if options.is_object() {
                    (options.get("lineHeight"), options.get("letterSpacing"), options.get("fontWeight"))
                } else {
                    (options.clone(), Value::Undefined, Value::Undefined)
                };
                let mut o = Object::new();
                o.insert("font-size", font_size);
                if !lh.is_undefined() {
                    o.insert("line-height", lh);
                }
                if !ls.is_undefined() {
                    o.insert("letter-spacing", ls);
                }
                if !fw.is_undefined() {
                    o.insert("font-weight", fw);
                }
                o.into_value()
            }),
        )],
        MatchOptions { values, modifiers, types: types(&["absolute-size", "relative-size", "length", "percentage"]), ..Default::default() },
    );
    Ok(())
}

fn font_variant_numeric(api: &mut PluginApi) -> Result<(), String> {
    let v = "var(--tw-ordinal) var(--tw-slashed-zero) var(--tw-numeric-figure) var(--tw-numeric-spacing) var(--tw-numeric-fraction)";
    api.add_defaults(
        "font-variant-numeric",
        objs(&[("--tw-ordinal", " "), ("--tw-slashed-zero", " "), ("--tw-numeric-figure", " "), ("--tw-numeric-spacing", " "), ("--tw-numeric-fraction", " ")]),
    )?;
    let d = || ("@defaults font-variant-numeric", Object::new().into_value());
    let item = |var: &str, val: &str| obj(vec![d(), (var, s(val)), ("font-variant-numeric", s(v))]);
    let mut o = Object::new();
    o.insert(".normal-nums", objs(&[("font-variant-numeric", "normal")]));
    o.insert(".ordinal", item("--tw-ordinal", "ordinal"));
    o.insert(".slashed-zero", item("--tw-slashed-zero", "slashed-zero"));
    o.insert(".lining-nums", item("--tw-numeric-figure", "lining-nums"));
    o.insert(".oldstyle-nums", item("--tw-numeric-figure", "oldstyle-nums"));
    o.insert(".proportional-nums", item("--tw-numeric-spacing", "proportional-nums"));
    o.insert(".tabular-nums", item("--tw-numeric-spacing", "tabular-nums"));
    o.insert(".diagonal-fractions", item("--tw-numeric-fraction", "diagonal-fractions"));
    o.insert(".stacked-fractions", item("--tw-numeric-fraction", "stacked-fractions"));
    add_utils(api, o.into_value())
}

fn box_shadow(api: &mut PluginApi) -> Result<(), String> {
    api.add_defaults(
        "box-shadow",
        objs(&[("--tw-ring-offset-shadow", "0 0 #0000"), ("--tw-ring-shadow", "0 0 #0000"), ("--tw-shadow", "0 0 #0000"), ("--tw-shadow-colored", "0 0 #0000")]),
    )?;
    let values = api.theme("boxShadow");
    let default_box_shadow = "var(--tw-ring-offset-shadow, 0 0 #0000), var(--tw-ring-shadow, 0 0 #0000), var(--tw-shadow)";
    api.match_utilities(
        vec![(
            "shadow",
            rf(move |value, _| {
                let value = transform_theme_value("boxShadow", value.clone(), &Object::new().into_value());
                let vs = value.to_js_string();
                let mut ast = parse_box_shadow_value(&vs);
                for sh in ast.iter_mut() {
                    if !sh.valid {
                        continue;
                    }
                    sh.color = Some("var(--tw-shadow-color)".into());
                }
                let none = value == s("none");
                obj(vec![
                    ("@defaults box-shadow", Object::new().into_value()),
                    ("--tw-shadow", if none { s("0 0 #0000") } else { value.clone() }),
                    ("--tw-shadow-colored", if none { s("0 0 #0000") } else { s(&format_box_shadow_value(&ast)) }),
                    ("box-shadow", s(default_box_shadow)),
                ])
            }),
        )],
        opts(values, &["shadow"]),
    );
    Ok(())
}

fn ring_width(api: &mut PluginApi) -> Result<(), String> {
    let ring_color_default = if super::config::flag_enabled(api.config_value(), "respectDefaultRingColorOpacity") {
        api.theme("ringColor.DEFAULT")
    } else {
        let ring_opacity_default = api.theme_or("ringOpacity.DEFAULT", s("0.5"));
        let rc = api.theme("ringColor");
        let fallback = s(&format!("rgb(147 197 253 / {})", ring_opacity_default.to_js_string()));
        if !rc.get("DEFAULT").truthy() {
            fallback
        } else {
            with_alpha_value(&rc.get("DEFAULT"), &ring_opacity_default, &fallback)
        }
    };
    api.add_defaults(
        "ring-width",
        obj(vec![
            ("--tw-ring-inset", s(" ")),
            ("--tw-ring-offset-width", api.theme_or("ringOffsetWidth.DEFAULT", s("0px"))),
            ("--tw-ring-offset-color", api.theme_or("ringOffsetColor.DEFAULT", s("#fff"))),
            ("--tw-ring-color", ring_color_default),
            ("--tw-ring-offset-shadow", s("0 0 #0000")),
            ("--tw-ring-shadow", s("0 0 #0000")),
            ("--tw-shadow", s("0 0 #0000")),
            ("--tw-shadow-colored", s("0 0 #0000")),
        ]),
    )?;
    let values = api.theme("ringWidth");
    api.match_utilities(
        vec![(
            "ring",
            rf(|value, _| {
                obj(vec![
                    ("@defaults ring-width", Object::new().into_value()),
                    ("--tw-ring-offset-shadow", s("var(--tw-ring-inset) 0 0 0 var(--tw-ring-offset-width) var(--tw-ring-offset-color)")),
                    (
                        "--tw-ring-shadow",
                        s(&format!("var(--tw-ring-inset) 0 0 0 calc({} + var(--tw-ring-offset-width)) var(--tw-ring-color)", value.to_js_string())),
                    ),
                    ("box-shadow", s("var(--tw-ring-offset-shadow), var(--tw-ring-shadow), var(--tw-shadow, 0 0 #0000)")),
                ])
            }),
        )],
        opts(values, &["length"]),
    );
    add_utils(api, obj(vec![(".ring-inset", obj(vec![("@defaults ring-width", Object::new().into_value()), ("--tw-ring-inset", s("inset"))]))]))
}

enum FilterKind {
    Blur,
    DropShadow,
    Wrap(&'static str),
}

fn filter_fn(api: &mut PluginApi, name: &'static str, key: &str, var: &'static str, kind: FilterKind, backdrop: bool) -> Result<(), String> {
    let values = api.theme(key);
    let kind = Arc::new(kind);
    let rule = rf(move |value, _| {
        let v = match &*kind {
            FilterKind::Blur => {
                let sv = value.to_js_string();
                if crate::js::trim(&sv).is_empty() { s(" ") } else { s(&format!("blur({sv})")) }
            }
            FilterKind::DropShadow => match value {
                Value::Arr(a) => s(&a.iter().map(|x| format!("drop-shadow({})", x.to_js_string())).collect::<Vec<_>>().join(" ")),
                v => s(&format!("drop-shadow({})", v.to_js_string())),
            },
            FilterKind::Wrap(f) => s(&format!("{f}({})", value.to_js_string())),
        };
        if backdrop {
            obj(vec![
                (var, v),
                ("@defaults backdrop-filter", Object::new().into_value()),
                ("-webkit-backdrop-filter", s(CSS_BACKDROP_FILTER_VALUE)),
                ("backdrop-filter", s(CSS_BACKDROP_FILTER_VALUE)),
            ])
        } else {
            obj(vec![(var, v), ("@defaults filter", Object::new().into_value()), ("filter", s(CSS_FILTER_VALUE))])
        }
    });
    let neg = matches!(name, "hue-rotate" | "backdrop-hue-rotate");
    api.match_utilities(vec![(name, rule)], MatchOptions { values, supports_negative_values: neg, ..Default::default() });
    Ok(())
}

fn contain(api: &mut PluginApi) -> Result<(), String> {
    let v = "var(--tw-contain-size) var(--tw-contain-layout) var(--tw-contain-paint) var(--tw-contain-style)";
    api.add_defaults("contain", objs(&[("--tw-contain-size", " "), ("--tw-contain-layout", " "), ("--tw-contain-paint", " "), ("--tw-contain-style", " ")]))?;
    let d = || ("@defaults contain", Object::new().into_value());
    let item = |var: &str, val: &str| obj(vec![d(), (var, s(val)), ("contain", s(v))]);
    let mut o = Object::new();
    o.insert(".contain-none", objs(&[("contain", "none")]));
    o.insert(".contain-content", objs(&[("contain", "content")]));
    o.insert(".contain-strict", objs(&[("contain", "strict")]));
    o.insert(".contain-size", item("--tw-contain-size", "size"));
    o.insert(".contain-inline-size", item("--tw-contain-size", "inline-size"));
    o.insert(".contain-layout", item("--tw-contain-layout", "layout"));
    o.insert(".contain-paint", item("--tw-contain-paint", "paint"));
    o.insert(".contain-style", item("--tw-contain-style", "style"));
    add_utils(api, o.into_value())
}

// ---- variant plugins

fn str_variant(api: &mut PluginApi, name: &str, format: &str) -> Result<(), String> {
    api.add_variant(name, vec![VDef::Str(format.to_string())], VariantOptions::default())
}

fn ufn(f: impl Fn(&mut UserVariantCtx) -> Result<URet, String> + Send + Sync + 'static) -> UserVFn {
    Arc::new(f)
}

/// `removeAlphaVariables(container, toRemove)`
fn remove_alpha_variables(ctx: &mut UserVariantCtx, to_remove: &'static [&'static str]) {
    let container = ctx.container();
    let css = ctx.css();
    css.walk_decls(container, &mut |css, d| {
        if to_remove.contains(&css[d].prop.as_str()) {
            css.remove(d);
            return Flow::Continue;
        }
        for var in to_remove {
            let a = format!("/ var({var})");
            let b = format!("/ var({var}, 1)");
            if css[d].value.contains(&a) {
                css[d].value = css[d].value.replacen(&a, "", 1);
            } else if css[d].value.contains(&b) {
                css[d].value = css[d].value.replacen(&b, "", 1);
            }
        }
        Flow::Continue
    });
}

fn content_variant(ctx: &mut UserVariantCtx, ret: &'static str) -> Result<URet, String> {
    let container = ctx.container();
    let css = ctx.css();
    css.walk_rules(container, &mut |css, rule| {
        let mut found = false;
        css.walk_decls(rule, &mut |css, d| {
            if css[d].prop == "content" {
                found = true;
            }
            Flow::Continue
        });
        if !found {
            let d = css.decl("content", "var(--tw-content)");
            css.prepend(rule, Add::Node(d));
        }
        Flow::Continue
    });
    Ok(URet::Str(ret.into()))
}

pub fn child_variant(api: &mut PluginApi) -> Result<(), String> {
    str_variant(api, "*", "& > *")
}

pub fn pseudo_element_variants(api: &mut PluginApi) -> Result<(), String> {
    str_variant(api, "first-letter", "&::first-letter")?;
    str_variant(api, "first-line", "&::first-line")?;
    api.add_variant(
        "marker",
        vec![
            VDef::Fn(ufn(|ctx| {
                remove_alpha_variables(ctx, &["--tw-text-opacity"]);
                Ok(URet::Str("& *::marker".into()))
            })),
            VDef::Fn(ufn(|ctx| {
                remove_alpha_variables(ctx, &["--tw-text-opacity"]);
                Ok(URet::Str("&::marker".into()))
            })),
        ],
        VariantOptions::default(),
    )?;
    api.add_variant("selection", vec![VDef::Str("& *::selection".into()), VDef::Str("&::selection".into())], VariantOptions::default())?;
    str_variant(api, "file", "&::file-selector-button")?;
    str_variant(api, "placeholder", "&::placeholder")?;
    str_variant(api, "backdrop", "&::backdrop")?;
    api.add_variant("before", vec![VDef::Fn(ufn(|ctx| content_variant(ctx, "&::before")))], VariantOptions::default())?;
    api.add_variant("after", vec![VDef::Fn(ufn(|ctx| content_variant(ctx, "&::after")))], VariantOptions::default())
}

/// A pseudo-class variant state: a format string or the `visited` function
#[derive(Clone)]
enum PState {
    S(String),
    Visited,
}

fn pseudo_variants(api: &PluginApi) -> Vec<(String, PState)> {
    let hover = if super::config::flag_enabled(api.config_value(), "hoverOnlyWhenSupported") {
        "@media (hover: hover) and (pointer: fine) { &:hover }".to_string()
    } else {
        "&:hover".to_string()
    };
    let mut list: Vec<(String, PState)> = vec![
        ("first".into(), PState::S("&:first-child".into())),
        ("last".into(), PState::S("&:last-child".into())),
        ("only".into(), PState::S("&:only-child".into())),
        ("odd".into(), PState::S("&:nth-child(odd)".into())),
        ("even".into(), PState::S("&:nth-child(even)".into())),
    ];
    let plain = |n: &str| (n.to_string(), PState::S(format!("&:{n}")));
    list.push(plain("first-of-type"));
    list.push(plain("last-of-type"));
    list.push(plain("only-of-type"));
    list.push(("visited".into(), PState::Visited));
    list.push(plain("target"));
    list.push(("open".into(), PState::S("&[open]".into())));
    for n in [
        "default",
        "checked",
        "indeterminate",
        "placeholder-shown",
        "autofill",
        "optional",
        "required",
        "valid",
        "invalid",
        "in-range",
        "out-of-range",
        "read-only",
        "empty",
        "focus-within",
    ] {
        list.push(plain(n));
    }
    list.push(("hover".into(), PState::S(hover)));
    for n in ["focus", "focus-visible", "active", "enabled", "disabled"] {
        list.push(plain(n));
    }
    list
}

fn visited_fn(ctx: &mut UserVariantCtx) -> String {
    remove_alpha_variables(ctx, &["--tw-text-opacity", "--tw-border-opacity", "--tw-bg-opacity"]);
    "&:visited".into()
}

pub fn pseudo_class_variants(api: &mut PluginApi) -> Result<(), String> {
    let pv = pseudo_variants(api);
    for (name, state) in pv.iter().cloned() {
        let f = ufn(move |ctx| {
            Ok(match &state {
                PState::S(s) => URet::Str(s.clone()),
                PState::Visited => URet::Str(visited_fn(ctx)),
            })
        });
        api.add_variant(&name, vec![VDef::Fn(f)], VariantOptions::default())?;
    }
    let values: Vec<(String, VarValue)> = pv
        .iter()
        .map(|(n, st)| {
            (
                n.clone(),
                match st {
                    PState::S(s) => VarValue::Str(s.clone()),
                    PState::Visited => VarValue::Fn(Arc::new(|ctx: &mut UserVariantCtx| visited_fn(ctx))),
                },
            )
        })
        .collect();
    for (name, combinator) in [("group", " &"), ("peer", " ~ &")] {
        let pref = api.prefix(&format!(".{name}"));
        let f: MatchVariantFn = Arc::new(move |value, modifier, ctx| {
            let raw = match value {
                VarValue::Fn(f) => f(ctx),
                VarValue::Str(s) => s.clone(),
                VarValue::None => String::new(),
                VarValue::Screen(_) => "[object Object]".into(),
                VarValue::Other(v) => if v.is_undefined() { String::new() } else { v.to_js_string() },
            };
            let mut result = normalize(&raw, None);
            if !result.contains('&') {
                result = format!("&{result}");
            }
            let a = match modifier {
                Some(m) if !m.is_empty() => format!(":merge({pref}\\/{})", escape_class_name(m)),
                _ => format!(":merge({pref})"),
            };
            let b = combinator;
            let chars: Vec<char> = result.chars().collect();
            let mut start: Option<usize> = None;
            let mut end: Option<usize> = None;
            let mut quotes = 0;
            for (i, c) in chars.iter().enumerate() {
                if *c == '&' {
                    start = Some(i);
                } else if *c == '\'' || *c == '"' {
                    quotes += 1;
                } else if start.is_some() && *c == ' ' && quotes == 0 {
                    end = Some(i);
                }
            }
            if start.is_some() && end.is_none() {
                end = Some(chars.len());
            }
            let start = start.unwrap();
            let end = end.unwrap();
            let slice = |a: usize, b: usize| -> String { if a < b { chars[a..b].iter().collect() } else { String::new() } };
            Ok(URet::Str(format!("{}{a}{}{b}{}", slice(0, start), slice(start + 1, end), slice(end, chars.len()))))
        });
        let o = VariantOptions { respect_prefix_false: true, ..Default::default() };
        api.match_variant(name, f, values.clone(), o)?;
    }
    Ok(())
}

pub fn direction_variants(api: &mut PluginApi) -> Result<(), String> {
    str_variant(api, "ltr", "&:where([dir=\"ltr\"], [dir=\"ltr\"] *)")?;
    str_variant(api, "rtl", "&:where([dir=\"rtl\"], [dir=\"rtl\"] *)")
}

pub fn reduced_motion_variants(api: &mut PluginApi) -> Result<(), String> {
    str_variant(api, "motion-safe", "@media (prefers-reduced-motion: no-preference)")?;
    str_variant(api, "motion-reduce", "@media (prefers-reduced-motion: reduce)")
}

pub fn dark_variants(api: &mut PluginApi) -> Result<(), String> {
    let dm = {
        let v = api.config("darkMode");
        if v.is_undefined() { s("media") } else { v }
    };
    let (mut mode, selector): (Value, Value) = match &dm {
        Value::Arr(a) => (a.first().cloned().unwrap_or(Value::Undefined), a.get(1).cloned().unwrap_or(Value::Undefined)),
        v => (v.clone(), Value::Undefined),
    };
    let selector = if selector.is_undefined() { s(".dark") } else { selector };
    if mode == Value::Bool(false) {
        mode = s("media");
    }
    let mut formats: Vec<String> = Vec::new();
    if mode == s("variant") {
        match &selector {
            Value::Arr(a) => formats = a.iter().map(|x| x.to_js_string()).collect(),
            Value::Str(st) => formats = vec![st.to_string()],
            _ => {}
        }
        for f in &formats {
            if f == ".dark" || !f.contains('&') {
                mode = Value::Bool(false);
            }
        }
    }
    let sel = selector.to_js_string();
    if mode == s("selector") {
        str_variant(api, "dark", &format!("&:where({sel}, {sel} *)"))
    } else if mode == s("media") {
        str_variant(api, "dark", "@media (prefers-color-scheme: dark)")
    } else if mode == s("variant") {
        api.add_variant("dark", formats.into_iter().map(VDef::Str).collect(), VariantOptions::default())
    } else if mode == s("class") {
        str_variant(api, "dark", &format!("&:is({sel} *)"))
    } else {
        Ok(())
    }
}

pub fn print_variant(api: &mut PluginApi) -> Result<(), String> {
    str_variant(api, "print", "@media print")
}

fn screen_value_sort(v: &VarValue) -> super::offsets::SortValue {
    match v {
        VarValue::Screen(s) => super::offsets::SortValue::Screen(s.clone()),
        VarValue::Str(s) => super::offsets::SortValue::Str(s.clone()),
        _ => super::offsets::SortValue::Other,
    }
}

pub fn screen_variants(api: &mut PluginApi) -> Result<(), String> {
    let raw_screens = {
        let v = api.theme("screens");
        if v.is_nullish() { Object::new().into_value() } else { v }
    };
    let are_simple = match &raw_screens {
        Value::Obj(o) => o.values().all(|v| v.is_string()),
        Value::Arr(a) => a.iter().all(|v| v.is_string()),
        Value::Str(_) => true,
        _ => true,
    };
    let screens = normalize_screens(&api.theme("screens"))?;
    let mut unit_cache: HashSet<String> = HashSet::new();
    fn units(value: &str) -> String {
        // value.match(/(\D+)$/)?.[1] ?? '(none)'
        let t = value.trim_end_matches(|c: char| !c.is_ascii_digit());
        let tail = &value[t.len()..];
        if tail.is_empty() { "(none)".into() } else { tail.to_string() }
    }
    for sc in &screens {
        for v in &sc.values {
            if !v.min.is_undefined() {
                unit_cache.insert(units(&v.min.to_js_string()));
            }
            if !v.max.is_undefined() {
                unit_cache.insert(units(&v.max.to_js_string()));
            }
        }
    }
    let consistent = unit_cache.len() <= 1;
    api.ctx.screen_units = unit_cache.into_iter().collect();
    let build_values = |ty: &str| -> Vec<(String, VarValue)> {
        let mut out: Vec<(String, VarValue)> = Vec::new();
        for screen in screens.iter().filter(|s| super::screens::is_screen_sortable(s)) {
            let v0 = &screen.values[0];
            let sc = if ty == "min" && !v0.min.is_undefined() {
                screen.clone()
            } else if ty == "min" && !v0.max.is_undefined() {
                Screen { not: !screen.not, ..screen.clone() }
            } else if ty == "max" && !v0.max.is_undefined() {
                screen.clone()
            } else if ty == "max" && !v0.min.is_undefined() {
                Screen { not: !screen.not, ..screen.clone() }
            } else {
                // `undefined` entries make `[screen.name, screen]` throw; unreachable for sortable screens
                continue;
            };
            if let Some(e) = out.iter_mut().find(|(k, _)| *k == screen.name) {
                e.1 = VarValue::Screen(sc);
            } else {
                out.push((screen.name.clone(), VarValue::Screen(sc)));
            }
        }
        out
    };
    let build_variant = |ty: &'static str| -> MatchVariantFn {
        Arc::new(move |value, _modifier, ctx| {
            if !are_simple || !consistent {
                return Ok(URet::Arr(vec![]));
            }
            if let VarValue::Str(st) = value {
                let c = &mut *ctx.api.screen_units;
                c.insert(units(st));
                if c.len() != 1 {
                    return Ok(URet::Arr(vec![]));
                }
            }
            let screen = match value {
                VarValue::Screen(s) => s.clone(),
                VarValue::Str(st) => super::screens::to_screen(&super::offsets::SortValue::Str(st.clone()), ty),
                _ => super::screens::to_screen(&super::offsets::SortValue::Other, ty),
            };
            Ok(URet::Arr(vec![format!("@media {}", build_media_query(&[screen]))]))
        })
    };
    let max_values = if are_simple { build_values("max") } else { vec![] };
    let o = VariantOptions { sort: Some(SortFn::MaxScreens), ..Default::default() };
    api.match_variant("max", build_variant("max"), max_values, o)?;
    let id = VariantId::Str("min-screens".into());
    for screen in &screens {
        let o = VariantOptions {
            id: Some(id.clone()),
            sort: if are_simple && consistent { Some(SortFn::MinScreens) } else { None },
            value: Some(super::context::VarValueSort::Screen(screen.clone())),
            ..Default::default()
        };
        api.add_variant(&screen.name, vec![VDef::Str(format!("@media {}", build_media_query(std::slice::from_ref(screen))))], o)?;
    }
    let o = VariantOptions { id: Some(id), sort: Some(SortFn::MinScreens), ..Default::default() };
    api.match_variant("min", build_variant("min"), vec![], o)?;
    let _ = screen_value_sort;
    Ok(())
}

fn theme_values(api: &PluginApi, key: &str) -> Vec<(String, VarValue)> {
    match api.theme(key) {
        Value::Obj(o) => o.iter().map(|(k, v)| (k.to_string(), match v {
            Value::Str(s) => VarValue::Str(s.to_string()),
            other => VarValue::Other(other.clone()),
        })).collect(),
        _ => vec![],
    }
}

fn var_value_string(v: &VarValue) -> String {
    match v {
        VarValue::Str(s) => s.clone(),
        VarValue::None => String::new(),
        VarValue::Other(v) => v.to_js_string(),
        VarValue::Screen(_) => "[object Object]".into(),
        VarValue::Fn(_) => "function () { [native code] }".into(),
    }
}

pub fn supports_variants(api: &mut PluginApi) -> Result<(), String> {
    let values = theme_values(api, "supports");
    let f: MatchVariantFn = Arc::new(|value, _, _| {
        let raw = match value {
            VarValue::Other(v) if v.is_undefined() => String::new(),
            v => var_value_string(v),
        };
        let mut check = normalize(&raw, None);
        // /^\w*\s*\(/
        let w = check.bytes().take_while(|c| c.is_ascii_alphanumeric() || *c == b'_').count();
        let rest = check[w..].trim_start_matches(crate::js::is_js_whitespace);
        let is_raw = rest.starts_with('(');
        if is_raw {
            check = replace_logical_words(&check);
            return Ok(URet::Str(format!("@supports {check}")));
        }
        if !check.contains(':') {
            check = format!("{check}: var(--tw)");
        }
        if !(check.starts_with('(') && check.ends_with(')')) {
            check = format!("({check})");
        }
        Ok(URet::Str(format!("@supports {check}")))
    });
    api.match_variant("supports", f, values, VariantOptions::default())
}

/// `check.replace(/\b(and|or|not)\b/g, ' $1 ')`
fn replace_logical_words(s: &str) -> String {
    let b = s.as_bytes();
    let is_w = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let mut matched = None;
        for w in ["and", "not", "or"] {
            if b[i..].starts_with(w.as_bytes()) {
                let before_ok = i == 0 || !is_w(b[i - 1]);
                let after = i + w.len();
                let after_ok = after >= b.len() || !is_w(b[after]);
                if before_ok && after_ok {
                    matched = Some(w);
                    break;
                }
            }
        }
        if let Some(w) = matched {
            out.push(' ');
            out.push_str(w);
            out.push(' ');
            i += w.len();
        } else {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

pub fn has_variants(api: &mut PluginApi) -> Result<(), String> {
    let no_prefix = || VariantOptions { respect_prefix_false: true, ..Default::default() };
    api.match_variant("has", Arc::new(|v, _, _| Ok(URet::Str(format!("&:has({})", normalize(&var_value_string(v), None))))), vec![], no_prefix())?;
    let group = api.prefix(".group");
    api.match_variant(
        "group-has",
        Arc::new(move |v, m, _| {
            let val = normalize(&var_value_string(v), None);
            Ok(URet::Str(match m {
                Some(m) if !m.is_empty() => format!(":merge({group}\\/{m}):has({val}) &"),
                _ => format!(":merge({group}):has({val}) &"),
            }))
        }),
        vec![],
        no_prefix(),
    )?;
    let peer = api.prefix(".peer");
    api.match_variant(
        "peer-has",
        Arc::new(move |v, m, _| {
            let val = normalize(&var_value_string(v), None);
            Ok(URet::Str(match m {
                Some(m) if !m.is_empty() => format!(":merge({peer}\\/{m}):has({val}) ~ &"),
                _ => format!(":merge({peer}):has({val}) ~ &"),
            }))
        }),
        vec![],
        no_prefix(),
    )
}

fn attr_variants(api: &mut PluginApi, kind: &'static str) -> Result<(), String> {
    let values = theme_values(api, kind);
    api.match_variant(
        kind,
        Arc::new(move |v, _, _| Ok(URet::Str(format!("&[{kind}-{}]", normalize_attribute_selectors(&normalize(&var_value_string(v), None)))))),
        values.clone(),
        VariantOptions::default(),
    )?;
    api.match_variant(
        &format!("group-{kind}"),
        Arc::new(move |v, m, _| {
            let a = normalize_attribute_selectors(&normalize(&var_value_string(v), None));
            Ok(URet::Str(match m {
                Some(m) if !m.is_empty() => format!(":merge(.group\\/{m})[{kind}-{a}] &"),
                _ => format!(":merge(.group)[{kind}-{a}] &"),
            }))
        }),
        values.clone(),
        VariantOptions::default(),
    )?;
    api.match_variant(
        &format!("peer-{kind}"),
        Arc::new(move |v, m, _| {
            let a = normalize_attribute_selectors(&normalize(&var_value_string(v), None));
            Ok(URet::Str(match m {
                Some(m) if !m.is_empty() => format!(":merge(.peer\\/{m})[{kind}-{a}] ~ &"),
                _ => format!(":merge(.peer)[{kind}-{a}] ~ &"),
            }))
        }),
        values,
        VariantOptions::default(),
    )
}

pub fn aria_variants(api: &mut PluginApi) -> Result<(), String> {
    attr_variants(api, "aria")
}

pub fn data_variants(api: &mut PluginApi) -> Result<(), String> {
    attr_variants(api, "data")
}

pub fn orientation_variants(api: &mut PluginApi) -> Result<(), String> {
    str_variant(api, "portrait", "@media (orientation: portrait)")?;
    str_variant(api, "landscape", "@media (orientation: landscape)")
}

pub fn prefers_contrast_variants(api: &mut PluginApi) -> Result<(), String> {
    str_variant(api, "contrast-more", "@media (prefers-contrast: more)")?;
    str_variant(api, "contrast-less", "@media (prefers-contrast: less)")
}

pub fn forced_colors_variants(api: &mut PluginApi) -> Result<(), String> {
    str_variant(api, "forced-colors", "@media (forced-colors: active)")
}

pub fn _unused(_: NodeId, _: &dyn Fn(&str) -> String) {
    let _ = util::dt_url;
}
