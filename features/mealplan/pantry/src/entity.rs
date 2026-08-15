//! `PantryItem`'s vault mapping.
//!
//! Everything generic — the frontmatter split, the YAML readers, the
//! slug rule, and the whole CRUD store — comes from `vault-entity`.
//! What stays here is the part that is genuinely about pantry stock:
//! which frontmatter keys map to which fields.
//!
//! Discriminator: a page is a pantry item when it carries inventory's
//! `type: item` (or the `item` tag) **and** `pantry` in `tags:`. That
//! compound rule keeps a single physical thing visible in both lists,
//! so [`VaultEntity::matches`] is overridden rather than taking the
//! shared single-tag default.

use chrono::{DateTime, Utc};
use cookbook::Nutrition;
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};

use crate::model::{PantryItem, StockEntry, SubReason, Substitution};

/// Vault mapping marker for [`PantryItem`].
pub struct PantryItems;

/// Inventory's own discriminator — a pantry page must satisfy it too,
/// or it isn't an inventory row at all.
const INVENTORY_TYPE: &str = "item";
/// The tag that narrows an inventory row down to food.
const PANTRY_TAG: &str = "pantry";

impl VaultEntity for PantryItems {
    type Model = PantryItem;

    /// The frontmatter `type:` written to disk is inventory's, not a
    /// pantry-specific one — one physical thing, one page.
    const TYPE: &'static str = INVENTORY_TYPE;
    const DEFAULT_FOLDER: &'static str = "Operations/Inventory/Pantry";
    const SLUG_FALLBACK: &'static str = "pantry-item";

    fn id(i: &PantryItem) -> Uuid {
        i.id
    }
    fn set_id(i: &mut PantryItem, id: Uuid) {
        i.id = id;
    }
    fn path(i: &PantryItem) -> &str {
        &i.path
    }
    fn set_path(i: &mut PantryItem, path: String) {
        i.path = path;
    }
    fn name(i: &PantryItem) -> &str {
        &i.name
    }

    fn on_create(i: &mut PantryItem, now: DateTime<Utc>) {
        i.date_created.get_or_insert(now);
    }

    fn on_update(i: &mut PantryItem, now: DateTime<Utc>) {
        i.date_modified = Some(now);
    }

    /// `type: item` (or the `item` tag) **and** the `pantry` tag.
    fn matches(page: &VaultPage) -> bool {
        let Some((map, _)) = frontmatter::mapping(&page.raw) else {
            return false;
        };
        let tags = yaml::string_list_at(&map, "tags");
        let is_inventory = yaml::str_at(&map, "type").as_deref() == Some(INVENTORY_TYPE)
            || tags.iter().any(|t| t == INVENTORY_TYPE);
        is_inventory && tags.iter().any(|t| t == PANTRY_TAG)
    }

    fn from_page(page: &VaultPage) -> Result<PantryItem, ParseError> {
        let (map, body) = frontmatter::mapping(&page.raw).ok_or(ParseError::NoFrontmatter)?;

        // A page with no `id:` gets a stable one derived from its path,
        // so hand-authored files keep the same identity across reads.
        let id = yaml::str_at(&map, "id")
            .and_then(|s| Uuid::parse_str(&s).ok())
            .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, page.rel_path.as_bytes()));

        Ok(PantryItem {
            path: page.rel_path.clone(),
            id,
            name: yaml::str_at(&map, "name").unwrap_or_else(|| page.basename.clone()),
            category: yaml::str_at(&map, "category").unwrap_or_else(|| "food".into()),
            location_id: yaml::str_at(&map, "location_id").and_then(|s| Uuid::parse_str(&s).ok()),
            condition: yaml::str_at(&map, "condition").unwrap_or_else(|| "good".into()),
            status: yaml::str_at(&map, "status").unwrap_or_else(|| "stored".into()),
            // The two discriminators are structure, not user tags, and
            // every other slice keeps its own out of this list. They
            // survive a round-trip regardless: `to_markdown` writes
            // `type: item` and re-asserts the `pantry` tag.
            tags: crate::model::StringList(
                yaml::string_list_at(&map, "tags")
                    .into_iter()
                    .filter(|t| t != INVENTORY_TYPE && t != PANTRY_TAG)
                    .collect(),
            ),
            date_created: yaml::timestamp_at(&map, "dateCreated"),
            date_modified: yaml::timestamp_at(&map, "dateModified"),
            food_category: yaml::str_at(&map, "foodCategory").unwrap_or_default(),
            qty: yaml::f64_at(&map, "qty"),
            unit: yaml::str_at(&map, "unit").unwrap_or_default(),
            purchase_unit: yaml::str_at(&map, "purchaseUnit"),
            purchase_to_stock_factor: yaml::f64_at(&map, "purchaseToStockFactor"),
            expiry: yaml::date_at(&map, "expiry"),
            opened: yaml::bool_at(&map, "opened").unwrap_or(false),
            opened_date: yaml::date_at(&map, "openedDate"),
            brand: yaml::str_at(&map, "brand"),
            nutrition_per_unit: map
                .get("nutritionPerUnit")
                .and_then(|v| serde_yaml::from_value::<Nutrition>(v.clone()).ok()),
            nutrition_unit: yaml::str_at(&map, "nutritionUnit"),
            minimum: yaml::f64_at(&map, "minimum"),
            default_best_before_days: shelf_days(&map, "defaultBestBeforeDays"),
            default_best_before_days_after_open: shelf_days(&map, "defaultBestBeforeDaysAfterOpen"),
            default_best_before_days_after_freezing: shelf_days(
                &map,
                "defaultBestBeforeDaysAfterFreezing",
            ),
            default_best_before_days_after_thawing: shelf_days(
                &map,
                "defaultBestBeforeDaysAfterThawing",
            ),
            due_type: yaml::str_at(&map, "dueType").unwrap_or_else(|| "best-before".into()),
            substitutes: crate::model::Substitutions(parse_substitutes(&map)),
            stock_entries: crate::model::StockEntries(parse_stock_entries(&map)),
            barcodes: crate::model::StringList(yaml::string_list_at(&map, "barcodes")),
            image_url: yaml::str_at(&map, "imageUrl"),
            details: body.to_string(),
        })
    }

    fn to_markdown(i: &PantryItem) -> Result<String, WriteError> {
        // Make sure the `pantry` tag is present before we hand the row
        // to YAML — pantry's discriminator depends on it.
        let mut owned = i.clone();
        if !owned.tags.iter().any(|t| t == PANTRY_TAG) {
            owned.tags.push(PANTRY_TAG.to_string());
        }
        // `path` and `details` are `#[serde(skip)]` on the model, so
        // serializing the whole `PantryItem` yields exactly the
        // frontmatter keys; `details` becomes the markdown body.
        let details = owned.details.clone();
        frontmatter::document(Self::TYPE, &owned, &details)
    }
}

fn shelf_days(map: &serde_yaml::Mapping, key: &str) -> Option<u32> {
    yaml::i64_at(map, key).and_then(|n| u32::try_from(n).ok())
}

fn parse_substitutes(map: &serde_yaml::Mapping) -> Vec<Substitution> {
    let Some(seq) = map.get("substitutes").and_then(|v| v.as_sequence()) else {
        return Vec::new();
    };
    seq.iter()
        .filter_map(|row| {
            let m = row.as_mapping()?;
            Some(Substitution {
                item_id: yaml::str_at(m, "itemId").and_then(|s| Uuid::parse_str(&s).ok())?,
                ratio: yaml::f64_at(m, "ratio").unwrap_or(1.0),
                reasons: m
                    .get("reasons")
                    .and_then(|v| v.as_sequence())
                    .map(|s| {
                        s.iter()
                            .filter_map(|v| v.as_str())
                            .filter_map(SubReason::from_str)
                            .collect()
                    })
                    .unwrap_or_default(),
                note: yaml::str_at(m, "note"),
            })
        })
        .collect()
}

fn parse_stock_entries(map: &serde_yaml::Mapping) -> Vec<StockEntry> {
    let Some(seq) = map.get("stockEntries").and_then(|v| v.as_sequence()) else {
        return Vec::new();
    };
    seq.iter()
        .filter_map(|row| {
            let m = row.as_mapping()?;
            Some(StockEntry {
                id: yaml::str_at(m, "id")
                    .and_then(|s| Uuid::parse_str(&s).ok())
                    .unwrap_or_else(Uuid::new_v4),
                qty: yaml::f64_at(m, "qty")?,
                purchased_date: yaml::date_at(m, "purchasedDate")?,
                best_before: yaml::date_at(m, "bestBefore"),
                opened: yaml::bool_at(m, "opened").unwrap_or(false),
                opened_date: yaml::date_at(m, "openedDate"),
                price: yaml::f64_at(m, "price"),
                location_id: yaml::str_at(m, "locationId").and_then(|s| Uuid::parse_str(&s).ok()),
                note: yaml::str_at(m, "note"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(raw: &str) -> VaultPage {
        VaultPage {
            rel_path: "Operations/Inventory/Pantry/rice.md".into(),
            basename: "rice".into(),
            folder: "Operations/Inventory/Pantry".into(),
            raw: raw.to_string(),
            mtime: std::time::SystemTime::UNIX_EPOCH,
        }
    }

    const RAW: &str = "---\ntype: item\nname: Rice\ntags:\n  - item\n  - pantry\n  - staple\n---\n";

    /// `item` and `pantry` are how a page is recognised, not tags the
    /// user typed — they used to show up in the UI's tag list.
    #[test]
    fn discriminators_are_not_user_tags() {
        let item = PantryItems::from_page(&page(RAW)).unwrap();
        assert_eq!(&*item.tags, &["staple".to_string()]);
    }

    /// Dropping them from the model must not stop the page matching:
    /// the writer puts `type: item` back in the frontmatter and
    /// re-asserts the `pantry` tag.
    #[test]
    fn a_round_trip_still_matches() {
        let item = PantryItems::from_page(&page(RAW)).unwrap();
        let rewritten = PantryItems::to_markdown(&item).unwrap();
        assert!(
            PantryItems::matches(&page(&rewritten)),
            "rewritten page no longer matches:\n{rewritten}"
        );
        let back = PantryItems::from_page(&page(&rewritten)).unwrap();
        assert_eq!(&*back.tags, &["staple".to_string()]);
    }
}
