//! The tailwindcss plugin: config resolution, context setup, rule generation and the
//! post-processing passes of `processTailwindFeatures`.

pub mod colors;
pub mod config;
pub mod util;
pub mod context;
pub mod format_variant_selector;
pub mod generate;
pub mod object_styles;
pub mod offsets;
pub mod plugin_utils;
pub mod screens;
pub mod url;

use crate::js::Value;

/// The resolved config plus the fields the pipeline reads all the time.
pub struct TwConfig {
    pub value: Value,
    pub prefix: String,
    pub separator: String,
    pub important: Value,
    pub core_plugins: Vec<String>,
    pub blocklist: Vec<String>,
    pub generalized_modifiers: bool,
}

impl TwConfig {
    pub fn new(value: Value) -> TwConfig {
        let prefix = value.get("prefix").to_js_string();
        let separator = value.get("separator").to_js_string();
        let important = value.get("important");
        let core_plugins = value.get("corePlugins").as_arr().map(|a| a.iter().map(|v| v.to_js_string()).collect()).unwrap_or_default();
        let blocklist = value.get("blocklist").as_arr().map(|a| a.iter().map(|v| v.to_js_string()).collect()).unwrap_or_default();
        let generalized_modifiers = config::flag_enabled(&value, "generalizedModifiers");
        TwConfig { value, prefix, separator, important, core_plugins, blocklist, generalized_modifiers }
    }
}
pub mod apply;
pub mod core_plugins;
pub mod functions;
pub mod process;
