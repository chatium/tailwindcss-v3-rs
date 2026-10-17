//! Screen sorting from `util/normalizeScreens.js` (`compareScreens`, `toScreen`, `isScreenSortable`).

use crate::js::{self, Value};

use super::offsets::SortValue;
use super::util::{Screen, ScreenValue};

/// `toScreen(value, type)`
pub fn to_screen(value: &SortValue, ty: &str) -> Screen {
    match value {
        SortValue::Screen(s) => s.clone(),
        SortValue::Str(s) => {
            let v = Value::str(s.as_str());
            let sv = if ty == "min" {
                ScreenValue { min: v, max: Value::Undefined, raw: Value::Undefined }
            } else {
                ScreenValue { min: Value::Undefined, max: v, raw: Value::Undefined }
            };
            Screen { name: "arbitrary-screen".into(), not: false, values: vec![sv] }
        }
        SortValue::Other => Screen { name: "arbitrary-screen".into(), not: false, values: vec![] },
    }
}

/// `isScreenSortable(screen).result`
pub fn is_screen_sortable(screen: &Screen) -> bool {
    screen.values.len() == 1 && screen.values[0].raw.is_undefined() && !(!screen.values[0].min.is_undefined() && !screen.values[0].max.is_undefined())
}

fn pf(v: &Value) -> Option<f64> {
    if v.is_undefined() { None } else { Some(js::parse_float(&v.to_js_string())) }
}

/// `compareScreens(type, a, z)`; NaN stands for both NaN results and the "should never
/// happen" throws.
pub fn compare_screens(ty: &str, a: &SortValue, z: &SortValue) -> f64 {
    let a_screen = to_screen(a, ty);
    let z_screen = to_screen(z, ty);
    if !is_screen_sortable(&a_screen) || !is_screen_sortable(&z_screen) {
        return f64::NAN;
    }
    let (mut a_min, mut a_max) = (a_screen.values[0].min.clone(), a_screen.values[0].max.clone());
    let (mut z_min, mut z_max) = (z_screen.values[0].min.clone(), z_screen.values[0].max.clone());
    if matches!(a, SortValue::Screen(s) if s.not) {
        std::mem::swap(&mut a_min, &mut a_max);
    }
    if matches!(z, SortValue::Screen(s) if s.not) {
        std::mem::swap(&mut z_min, &mut z_max);
    }
    let (a_v, z_v) = if ty == "min" { (pf(&a_min), pf(&z_min)) } else { (pf(&z_max), pf(&a_max)) };
    match (a_v, z_v) {
        (Some(a), Some(z)) => a - z,
        _ => f64::NAN,
    }
}
