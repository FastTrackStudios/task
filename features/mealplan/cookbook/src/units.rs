//! Unit conversion, delegated to cooklang.
//!
//! We used to compare unit strings and merge only exact matches, which
//! meant 500 g and 0.5 kg stayed two rows on a shopping list and a
//! recipe asking for 1 kg looked unmet against 1200 g of stock.
//!
//! Cooklang ships the unit database its own parser uses, so the answer
//! is to ask it rather than keep a second table in step with it.

use std::sync::OnceLock;

use cooklang::Converter;
use cooklang::quantity::{Quantity, Value};

/// The bundled unit database. Built once — construction parses
/// cooklang's whole units file, which is not something to do per row of
/// a shopping list.
fn converter() -> &'static Converter {
    static CONVERTER: OnceLock<Converter> = OnceLock::new();
    CONVERTER.get_or_init(Converter::bundled)
}

/// `value` expressed in `to`, or `None` when the two units aren't the
/// same kind of thing — grams into cloves has no answer, and inventing
/// one is worse than admitting it.
///
/// Unitless values pass through unchanged: "2" of something and "3" of
/// it are 5 of it.
#[must_use]
pub fn convert(value: f64, from: &str, to: &str) -> Option<f64> {
    if from.eq_ignore_ascii_case(to) {
        return Some(value);
    }
    if from.is_empty() || to.is_empty() {
        return None;
    }
    let mut q = Quantity::new(Value::Number(value.into()), Some(from.to_string()));
    q.convert(to, converter()).ok()?;
    match q.value() {
        Value::Number(n) => Some(n.value()),
        // A range can't collapse to one number; the caller wanted a
        // scalar and should keep the rows apart instead.
        _ => None,
    }
}

/// Whether two units describe the same physical quantity, so their
/// amounts can be added at all.
#[must_use]
pub fn compatible(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b) || convert(1.0, a, b).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_within_a_system() {
        assert_eq!(convert(500.0, "g", "kg"), Some(0.5));
        assert_eq!(convert(2.0, "kg", "g"), Some(2000.0));
    }

    #[test]
    fn crosses_systems() {
        let oz = convert(1000.0, "g", "oz").expect("g converts to oz");
        assert!((oz - 35.27).abs() < 0.1, "1 kg is about 35.27 oz, got {oz}");
    }

    #[test]
    fn refuses_incompatible_units() {
        assert_eq!(convert(3.0, "g", "clove"), None);
        assert_eq!(
            convert(3.0, "ml", "g"),
            None,
            "volume to mass needs density"
        );
    }

    #[test]
    fn identical_units_pass_through() {
        assert_eq!(convert(7.0, "tbsp", "tbsp"), Some(7.0));
        assert_eq!(convert(7.0, "TBSP", "tbsp"), Some(7.0));
    }

    #[test]
    fn unitless_has_no_conversion() {
        assert_eq!(convert(2.0, "", "g"), None);
    }

    #[test]
    fn compatibility_is_what_merging_asks() {
        assert!(compatible("g", "kg"));
        assert!(compatible("", ""));
        assert!(!compatible("g", "clove"));
    }
}
