//! Rust port of the Tailwind CSS v3.4.16 compiler, byte-identical to the Play CDN bundle
//! (`tailwindcss` + `autoprefixer` with the bundle's baked browser data).
//!
//! ```no_run
//! let config = tailwindcss_v3::Config::from_json(serde_json::json!({})).unwrap();
//! let css = tailwindcss_v3::generate(&config, "", ["flex", "md:px-2"]).unwrap();
//! ```

pub mod js;
pub mod postcss;
pub mod selector;
pub mod tw;
pub mod value_parser;

use std::sync::Arc;

pub use postcss::CssError;

/// The prefix the Play bundle puts in front of the `<style type="text/tailwindcss">` blocks.
pub const INPUT_PREFIX: &str = "@tailwind base;@tailwind components;@tailwind utilities;";

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
    /// Position in the whole input (`@tailwind base;@tailwind components;@tailwind utilities;` +
    /// `blocks`), 1-based, as postcss reports it.
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub end_line: Option<usize>,
    pub end_column: Option<usize>,
}

impl Error {
    /// The position translated into `blocks` coordinates (line 1 loses the 53-column prefix).
    pub fn position_in_blocks(&self) -> Option<(usize, usize)> {
        let (l, c) = (self.line?, self.column?);
        if l == 1 {
            let p = INPUT_PREFIX.len();
            if c > p { Some((1, c - p)) } else { None }
        } else {
            Some((l, c))
        }
    }
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
    /// The `tailwind.config` object as JSON (keys in source order — `serde_json` with
    /// `preserve_order`; integer-like keys are reordered the way JS orders them).
    pub fn from_json(value: serde_json::Value) -> Result<Config, Error> {
        let user = js::Value::from_json(&value);
        if !user.is_object() && !user.is_nullish() {
            return Err(CssError::plain("tailwind.config must be an object").into());
        }
        // `{ ...window.tailwind.config, _hash, content: {...}, plugins: [...] }`
        let mut cfg = match &user {
            js::Value::Obj(o) => (**o).clone(),
            _ => js::Object::new(),
        };
        cfg.insert("_hash", js::Value::Num(0.0));
        let mut content = js::Object::new();
        content.insert("files", js::Value::from(vec![js::Value::str("/template.html")]));
        cfg.insert("content", content.into_value());
        let plugins = match cfg.get("plugins") {
            Some(js::Value::Arr(a)) => js::Value::Arr(a.clone()),
            _ => js::Value::from(Vec::<js::Value>::new()),
        };
        cfg.insert("plugins", plugins);
        let resolved = tw::config::resolve_config(&cfg.into_value());
        let inner = tw::TwConfig::new(resolved);
        if inner.separator == "-" {
            return Err(CssError::plain(
                "The '-' character cannot be used as a custom separator in JIT mode due to parsing ambiguity. Please use another character like '_' instead.",
            )
            .into());
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
}

/// CSS as the Play bundle produces it: `@tailwind base;@tailwind components;@tailwind
/// utilities;` + `blocks`, tailwind over `candidates`, then autoprefixer.
///
/// Candidates are treated like the class names the bundle reads from the DOM: order and
/// duplicates do not matter, and anything that is not a tailwind class is ignored.
pub fn generate<'a>(config: &Config, blocks: &str, candidates: impl IntoIterator<Item = &'a str>) -> Result<String, Error> {
    let input = format!("{INPUT_PREFIX}{blocks}");
    let cfg = &*config.inner;
    let mut ctx = tw::context::Context::new(cfg);
    let root = postcss::parse(&mut ctx.css, &input)?;
    tw::process::process_tailwind_features(&mut ctx, root, candidates)?;
    Ok(postcss::to_string(&mut ctx.css, root))
}
