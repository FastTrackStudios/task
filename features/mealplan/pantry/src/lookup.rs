//! External barcode lookup against [OpenFoodFacts][off].
//!
//! [off]: https://world.openfoodfacts.org/
//!
//! Mirrors grocy's
//! `plugins/OpenFoodFactsBarcodeLookupPlugin.php` shape:
//! one HTTPS GET to OFF's v2 product endpoint, no fallback,
//! `None` on 404 / not-found. We extend grocy's mapping with
//! nutrition (OFF exposes per-100g macros via `nutriments`)
//! and brand.
//!
//! Network access is gated behind the `lookup_external`
//! free function — never called from `parse` or `scan` —
//! so vault-only workflows compile + run without
//! reqwest in the runtime path.

use reqwest::blocking::Client;
use thiserror::Error;

use crate::model::PantryItemDraft;
use cookbook::Nutrition;

const OFF_BASE: &str = "https://world.openfoodfacts.org/api/v2/product/";

/// Fields requested from OFF. Keep the list narrow so OFF's
/// rate-limit guidance ("ask for what you need") is honored.
const OFF_FIELDS: &str =
    "product_name,brands,image_url,categories_tags,quantity,nutriments,serving_size";

#[derive(Debug, Error)]
pub enum LookupError {
    #[error("http: {0}")]
    Http(String),
    #[error("decode: {0}")]
    Decode(String),
}

/// Look `barcode` up against `OpenFoodFacts` and return a
/// partly-populated [`PantryItemDraft`]. `Ok(None)` when OFF
/// returns 404 / status=0 (no product); `Err` on network or
/// decode failure. The caller layers local-vault precedence
/// in [`PantryService::resolve_barcode`].
pub fn lookup_external(barcode: &str) -> Result<Option<PantryItemDraft>, LookupError> {
    let digits: String = barcode.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return Ok(None);
    }
    let client = Client::builder()
        .user_agent("Task-Pantry/0.1 (https://git.starcommand.live/FastTrackStudios/task)")
        .build()
        .map_err(|e| LookupError::Http(e.to_string()))?;
    let url = format!("{OFF_BASE}{digits}?fields={OFF_FIELDS}");

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| LookupError::Http(e.to_string()))?;
    let status = resp.status();
    if status.as_u16() == 404 {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(LookupError::Http(format!("status: {status}")));
    }

    let raw: serde_json::Value = resp
        .json()
        .map_err(|e| LookupError::Decode(e.to_string()))?;
    if raw.get("status").and_then(serde_json::Value::as_u64) != Some(1) {
        return Ok(None);
    }
    let product = raw
        .get("product")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if product.is_null() {
        return Ok(None);
    }

    Ok(Some(map_product(&product, &digits)))
}

fn map_product(p: &serde_json::Value, barcode: &str) -> PantryItemDraft {
    let name = p
        .get("product_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let brand = p
        .get("brands")
        .and_then(|v| v.as_str())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .filter(|s| !s.is_empty());
    let image_url = p
        .get("image_url")
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
        .filter(|s| !s.is_empty());

    // `categories_tags` is OFF's normalized taxonomy
    // (e.g. `en:cheeses`, `en:hard-cheeses`). Pick the most
    // specific (last) entry as a category hint; map common
    // ones to our canonical `FoodCategory` set, leave the
    // rest as a raw tag.
    let food_category = p
        .get("categories_tags")
        .and_then(|v| v.as_array())
        .and_then(|seq| seq.iter().rev().find_map(|v| v.as_str()))
        .map(|s| s.trim_start_matches("en:").to_string())
        .map(map_food_category)
        .unwrap_or_default();

    let serving_size = p
        .get("serving_size")
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
        .filter(|s| !s.is_empty());
    let nutrition_per_unit = parse_nutriments(p.get("nutriments"));
    // Nutriments on OFF are *per 100g* / *per 100ml* by
    // default; the `serving_size` string (e.g. `"30 g"`) is
    // separate. We anchor to 100g/100ml since that's what
    // `nutriments` actually carries.
    let nutrition_unit = if nutrition_per_unit.is_some() {
        Some(
            // OFF doesn't expose a clean per-100g vs
            // per-100ml flag — `quantity` sometimes implies
            // it ("500 ml" → likely ml). Default to 100g
            // and let the user override; expose
            // serving_size so the UI can show it alongside.
            serving_size.unwrap_or_else(|| "100g".to_string()),
        )
    } else {
        None
    };

    PantryItemDraft {
        barcode: barcode.to_string(),
        name,
        brand,
        food_category,
        unit: String::new(),
        nutrition_per_unit,
        nutrition_unit,
        image_url,
    }
}

fn parse_nutriments(v: Option<&serde_json::Value>) -> Option<Nutrition> {
    let m = v?.as_object()?;
    let get = |k: &str| m.get(k).and_then(serde_json::Value::as_f64);
    let calories = get("energy-kcal_100g").or_else(|| get("energy-kcal"));
    let protein_g = get("proteins_100g");
    let carbs_g = get("carbohydrates_100g");
    let fat_g = get("fat_100g");
    let fiber_g = get("fiber_100g");
    let sugar_g = get("sugars_100g");
    if [calories, protein_g, carbs_g, fat_g, fiber_g, sugar_g]
        .iter()
        .all(std::option::Option::is_none)
    {
        return None;
    }
    Some(Nutrition {
        calories,
        protein_g,
        carbs_g,
        fat_g,
        fiber_g,
        sugar_g,
    })
}

/// Map an OFF category tag (without the `en:` prefix) to
/// our canonical [`crate::FoodCategory`] string. Falls
/// back to the raw tag for anything we don't recognize so
/// nothing is lost.
fn map_food_category(tag: String) -> String {
    let t = tag.to_lowercase();
    let canonical = if t.contains("dairy") || t.contains("cheese") || t.contains("yogurt") {
        Some("dairy")
    } else if t.contains("meat") || t.contains("fish") || t.contains("seafood") {
        Some("protein")
    } else if t.contains("vegetable") || t.contains("fruit") {
        Some("produce")
    } else if t.contains("bread")
        || t.contains("pasta")
        || t.contains("cereal")
        || t.contains("rice")
    {
        Some("grain")
    } else if t.contains("oil") {
        Some("oil")
    } else if t.contains("spice") || t.contains("herb") {
        Some("spice")
    } else if t.contains("snack") || t.contains("chocolate") {
        Some("snack")
    } else if t.contains("beverage") || t.contains("drink") || t.contains("water") {
        Some("drink")
    } else if t.contains("canned") || t.contains("tinned") {
        Some("canned")
    } else if t.contains("frozen") {
        Some("frozen")
    } else {
        None
    };
    canonical
        .map(std::string::ToString::to_string)
        .unwrap_or(tag)
}
