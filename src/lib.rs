//! Rust port of the Tailwind CSS v3.4.19 compiler: `postcss([tailwindcss(config),
//! autoprefixer])` over a stylesheet, byte-identical to the npm packages (autoprefixer with the
//! Play CDN bundle's browser data).
//!
//! ```no_run
//! let config = tailwindcss_v3::Config::from_json(serde_json::json!({})).unwrap();
//! let css = tailwindcss_v3::generate(&config, "@tailwind utilities;", ["flex", "md:px-2"]).unwrap();
//! ```

pub mod autoprefixer;
pub mod extractor;
pub mod js;
pub mod jsregex;
pub mod postcss;
pub mod selector;
pub mod tw;
pub mod value_parser;

use std::sync::Arc;

pub use postcss::CssError;

/// A resolved tailwind v3 config (theme + extend, screens, darkMode, important, corePlugins,
/// safelist, blocklist, …).
#[derive(Clone)]
pub struct Config {
    inner: Arc<tw::TwConfig>,
}

/// An error as tailwind/postcss reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    /// `error.name`: `CssSyntaxError` or `Error`
    pub name: String,
    /// `error.reason` (the message without the position prefix)
    pub reason: String,
    /// `error.message` exactly as JS formats it (`<css input>:1:62: …`)
    pub message: String,
    /// Position in `css`, 1-based (UTF-16 columns), as postcss reports it.
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub end_line: Option<usize>,
    pub end_column: Option<usize>,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<CssError> for Error {
    fn from(e: CssError) -> Self {
        Error {
            name: e.name.to_string(),
            message: e.message(),
            reason: e.reason,
            line: e.line,
            column: e.column,
            end_line: e.end_line,
            end_column: e.end_column,
        }
    }
}

impl Config {
    /// The tailwind config object as JSON (what `@config` points to; keys in source order —
    /// `serde_json` with `preserve_order`; integer-like keys are reordered the way JS orders
    /// them). Plugins, functions and regular expressions cannot be expressed in JSON.
    pub fn from_json(value: serde_json::Value) -> Result<Config, Error> {
        let user = js::Value::from_json(&value);
        if !user.is_object() && !user.is_nullish() {
            return Err(CssError::plain("tailwind config must be an object").into());
        }
        let cfg = match &user {
            js::Value::Obj(o) => (**o).clone(),
            _ => js::Object::new(),
        };
        let resolved = tw::config::resolve_config(&cfg.into_value());
        let inner = tw::TwConfig::new(resolved);
        if inner.separator == "-" {
            return Err(separator_error());
        }
        Ok(Config { inner: Arc::new(inner) })
    }

    pub fn default_config() -> Config {
        Config::from_json(serde_json::json!({})).unwrap()
    }

    /// The resolved config as tailwind sees it.
    pub fn resolved(&self) -> &js::Value {
        &self.inner.value
    }

    /// The default extractor built for this config's `prefix` and `separator`.
    pub fn extract_candidates<'a>(&self, source: &'a str) -> Vec<&'a str> {
        if self.inner.prefix.is_empty() && self.inner.separator == ":" {
            return extractor::extract_candidates(source);
        }
        extractor::Extractor::new(&self.inner.separator, &self.inner.prefix).extract(source)
    }
}

/// `postcss([tailwindcss(config), autoprefixer])` over the stylesheet `css`: `@tailwind`
/// directives expand where they are written, `@config` rules are removed (the config is
/// `config`), everything else goes through as tailwind and postcss leave it.
///
/// `candidates` are the class names found in the content (order and duplicates do not matter;
/// strings that are not tailwind classes are ignored).
pub fn generate<'a>(config: &Config, css: &str, candidates: impl IntoIterator<Item = &'a str>) -> Result<String, Error> {
    let cfg = &*config.inner;
    let mut ctx = tw::context::Context::new(cfg);
    let root = postcss::parse(&mut ctx.css, css)?;
    tw::process::remove_config_at_rules(&mut ctx, root)?;
    tw::process::process_tailwind_features(&mut ctx, root, candidates)?;
    autoprefixer::autoprefix(&mut ctx.css, root)?;
    Ok(postcss::to_string(&mut ctx.css, root))
}

/// Port of tailwind v3's default extractor: class candidates in arbitrary source text (tsx, ts,
/// vue, html, …), deduplicated, as substrings of `source`.
pub fn extract_candidates(source: &str) -> Vec<&str> {
    extractor::extract_candidates(source)
}

fn separator_error() -> Error {
    CssError::plain(
        "The '-' character cannot be used as a custom separator in JIT mode due to parsing ambiguity. Please use another character like '_' instead.",
    )
    .into()
}
