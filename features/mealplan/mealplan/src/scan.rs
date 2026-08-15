//! Walk a `vault::Vault` and collect every page that looks
//! like a meal.

use chrono::NaiveDate;
use vault::Vault;

use crate::model::Meal;
use crate::parse::{looks_like_meal, parse_page};

#[must_use]
pub fn scan_vault(vault: &Vault) -> Vec<Meal> {
    vault
        .pages
        .iter()
        .filter(|p| looks_like_meal(p))
        .filter_map(|p| match parse_page(p) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!(path = %p.rel_path, ?e, "meal parse failed");
                None
            }
        })
        .collect()
}

/// Meals scheduled on a specific day.
#[must_use]
pub fn meals_on(vault: &Vault, day: NaiveDate) -> Vec<Meal> {
    scan_vault(vault)
        .into_iter()
        .filter(|m| m.scheduled_for == day)
        .collect()
}

/// Meals scheduled in `[start, end)`. Useful for week views.
#[must_use]
pub fn meals_between(vault: &Vault, start: NaiveDate, end: NaiveDate) -> Vec<Meal> {
    scan_vault(vault)
        .into_iter()
        .filter(|m| m.scheduled_for >= start && m.scheduled_for < end)
        .collect()
}
