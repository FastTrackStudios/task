//! Quantity units + conversions.
//!
//! Modeled on grocy's `quantity_units` +
//! `quantity_unit_conversions` tables, but lighter: same-base
//! conversions (mass↔mass, volume↔volume) are hard-coded with
//! SI + US-customary factors. Cross-base conversions
//! (mass↔volume) require density per item and stay out of
//! scope — see `plans/mealplan-grocy-parity.md` risk register.
//!
//! Units stay free-form `String` on the wire so unknown /
//! custom units round-trip; this module layers typed math
//! on top. The page's `unit` field is the canonical **stock
//! unit**; `purchase_unit` + `purchase_to_stock_factor` let
//! a buy-by-the-box-store-by-the-can workflow round-trip.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    Mass(MassUnit),
    Volume(VolumeUnit),
    Count(CountUnit),
    /// Free-form package shape that can't be cross-converted
    /// (boxes, bags, cans, bottles). Counted, not measured.
    Package(PackageUnit),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MassUnit {
    Microgram,
    Milligram,
    Gram,
    Kilogram,
    Ounce,
    Pound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VolumeUnit {
    Milliliter,
    Liter,
    Teaspoon,
    Tablespoon,
    FluidOunce,
    Cup,
    Pint,
    Quart,
    Gallon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CountUnit {
    /// Individual count: `1 egg`, `1 clove`, `1 banana`.
    Each,
    /// Bunch (e.g. of herbs).
    Bunch,
    /// Single clove (garlic) — distinct from Each because
    /// recipes often spell it out separately.
    Clove,
    /// Slice (bread, cheese).
    Slice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PackageUnit {
    Can,
    Bag,
    Box,
    Bottle,
    Jar,
    Carton,
    Pack,
}

impl Unit {
    /// Parse a free-form unit string into a canonical
    /// [`Unit`]. Returns `None` for anything unrecognized so
    /// callers can keep the raw string for round-trip.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.trim().to_ascii_lowercase();
        Some(match s.as_str() {
            "ug" | "mcg" | "microgram" | "micrograms" => Self::Mass(MassUnit::Microgram),
            "mg" | "milligram" | "milligrams" => Self::Mass(MassUnit::Milligram),
            "g" | "gram" | "grams" => Self::Mass(MassUnit::Gram),
            "kg" | "kilo" | "kilos" | "kilogram" | "kilograms" => Self::Mass(MassUnit::Kilogram),
            "oz" | "ounce" | "ounces" => Self::Mass(MassUnit::Ounce),
            "lb" | "lbs" | "pound" | "pounds" => Self::Mass(MassUnit::Pound),

            "ml" | "milliliter" | "milliliters" | "millilitre" | "millilitres" => {
                Self::Volume(VolumeUnit::Milliliter)
            }
            "l" | "liter" | "liters" | "litre" | "litres" => Self::Volume(VolumeUnit::Liter),
            "tsp" | "teaspoon" | "teaspoons" => Self::Volume(VolumeUnit::Teaspoon),
            "tbsp" | "tbs" | "tablespoon" | "tablespoons" => Self::Volume(VolumeUnit::Tablespoon),
            "fl oz" | "floz" | "fl-oz" | "fluid ounce" | "fluid ounces" => {
                Self::Volume(VolumeUnit::FluidOunce)
            }
            "cup" | "cups" | "c" => Self::Volume(VolumeUnit::Cup),
            "pt" | "pint" | "pints" => Self::Volume(VolumeUnit::Pint),
            "qt" | "quart" | "quarts" => Self::Volume(VolumeUnit::Quart),
            "gal" | "gallon" | "gallons" => Self::Volume(VolumeUnit::Gallon),

            "each" | "ea" | "" => Self::Count(CountUnit::Each),
            "bunch" | "bunches" => Self::Count(CountUnit::Bunch),
            "clove" | "cloves" => Self::Count(CountUnit::Clove),
            "slice" | "slices" => Self::Count(CountUnit::Slice),

            "can" | "cans" => Self::Package(PackageUnit::Can),
            "bag" | "bags" => Self::Package(PackageUnit::Bag),
            "box" | "boxes" => Self::Package(PackageUnit::Box),
            "bottle" | "bottles" => Self::Package(PackageUnit::Bottle),
            "jar" | "jars" => Self::Package(PackageUnit::Jar),
            "carton" | "cartons" => Self::Package(PackageUnit::Carton),
            "pack" | "packs" | "package" | "packages" => Self::Package(PackageUnit::Pack),

            _ => return None,
        })
    }

    /// Convert `qty` from `self` to `target`. Returns `None`
    /// when the units live in different bases (mass↔volume
    /// without density, count↔mass, etc.) or one of the
    /// `Package` variants is involved (no canonical
    /// conversion between bag and box).
    #[must_use]
    pub fn convert(self, qty: f64, target: Unit) -> Option<f64> {
        if self == target {
            return Some(qty);
        }
        match (self, target) {
            (Unit::Mass(a), Unit::Mass(b)) => Some(qty * a.to_grams() / b.to_grams()),
            (Unit::Volume(a), Unit::Volume(b)) => {
                Some(qty * a.to_milliliters() / b.to_milliliters())
            }
            (Unit::Count(a), Unit::Count(b)) if a == b => Some(qty),
            _ => None,
        }
    }
}

impl MassUnit {
    /// Conversion factor to grams.
    #[must_use]
    pub fn to_grams(self) -> f64 {
        match self {
            Self::Microgram => 1e-6,
            Self::Milligram => 1e-3,
            Self::Gram => 1.0,
            Self::Kilogram => 1000.0,
            Self::Ounce => 28.3495,
            Self::Pound => 453.592,
        }
    }
}

impl VolumeUnit {
    /// Conversion factor to milliliters (US-customary
    /// for cup/tbsp/tsp/floz).
    #[must_use]
    pub fn to_milliliters(self) -> f64 {
        match self {
            Self::Milliliter => 1.0,
            Self::Liter => 1000.0,
            Self::Teaspoon => 4.92892,
            Self::Tablespoon => 14.7868,
            Self::FluidOunce => 29.5735,
            Self::Cup => 236.588,
            Self::Pint => 473.176,
            Self::Quart => 946.353,
            Self::Gallon => 3785.41,
        }
    }
}

/// Convert `qty` from `from_unit` to `to_unit`, both
/// free-form strings. Returns `None` when either string
/// doesn't parse, or the two units live in incompatible
/// bases.
///
/// This is the entry point recipe-pos → pantry consumes
/// will use during phase 5's fulfillment math.
#[must_use]
pub fn convert_str(qty: f64, from_unit: &str, to_unit: &str) -> Option<f64> {
    if from_unit.trim().eq_ignore_ascii_case(to_unit.trim()) {
        return Some(qty);
    }
    let from = Unit::parse(from_unit)?;
    let to = Unit::parse(to_unit)?;
    from.convert(qty, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_aliases() {
        assert!(matches!(
            Unit::parse("kg"),
            Some(Unit::Mass(MassUnit::Kilogram))
        ));
        assert!(matches!(
            Unit::parse("Tbsp"),
            Some(Unit::Volume(VolumeUnit::Tablespoon))
        ));
        assert!(matches!(
            Unit::parse("cups"),
            Some(Unit::Volume(VolumeUnit::Cup))
        ));
        assert!(matches!(
            Unit::parse("each"),
            Some(Unit::Count(CountUnit::Each))
        ));
        assert!(matches!(
            Unit::parse(""),
            Some(Unit::Count(CountUnit::Each))
        ));
        assert!(Unit::parse("flubber").is_none());
    }

    #[test]
    fn mass_conversions() {
        assert!((convert_str(1.0, "kg", "g").unwrap() - 1000.0).abs() < 1e-6);
        assert!((convert_str(16.0, "oz", "lb").unwrap() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn volume_conversions() {
        assert!((convert_str(1.0, "cup", "tbsp").unwrap() - 16.0).abs() < 1e-2);
        assert!((convert_str(2.0, "tsp", "ml").unwrap() - 9.85784).abs() < 1e-3);
    }

    #[test]
    fn cross_base_returns_none() {
        assert!(convert_str(1.0, "g", "ml").is_none());
        assert!(convert_str(1.0, "each", "g").is_none());
    }

    #[test]
    fn package_units_dont_cross() {
        assert!(convert_str(1.0, "box", "can").is_none());
    }
}
