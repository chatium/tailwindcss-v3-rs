//! `public/colors.js`

use std::sync::OnceLock;

use crate::js::Value;

pub fn colors() -> &'static Value {
    static CELL: OnceLock<Value> = OnceLock::new();
    CELL.get_or_init(|| Value::from_json(&serde_json::from_str(include_str!("data/colors.json")).unwrap()))
}
